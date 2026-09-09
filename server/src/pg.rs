//! The Postgres repository.
//!
//! Every method runs inside a transaction that first sets `openpos.tenant_id`,
//! which is what the row-level security policies read. That is not decoration:
//! the policies use `current_setting(..., true)`, so an unset value makes the
//! comparison fail and the query return nothing. A transaction that forgets to
//! scope itself finds an empty database rather than somebody else's shop.
//!
//! The application must not connect as a superuser. Superusers bypass row-level
//! security entirely, and a deployment that does this has protection that looks
//! present in the schema and is absent at runtime. Table owners are covered,
//! because the migration marks every table `force row level security`.

use openpos_core::protocol::ItemWire;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use std::time::Duration;

use crate::auth::{Caller, Role, Token, TokenHash};
use openpos_core::protocol::QuarantineReason;

use crate::repo::{
    AccountEntry, AccountPayment, AccountRecord, Admission, AllowedAction, AmendedOperator,
    CATALOGUE_SCHEMA, CataloguePage, CatalogueRecord, ClosedShift, CustomerRecord, DaySummary,
    Decided, DecidedSale, GoodsReceipt, LeaseRecord, MadeSummary, OnHand, OpenDrawer,
    OperatorRecord, Owing, ReceiptGap, RepairItem, RepoError, Repository, Result, SaleOnPaper,
    SaleRecord, Settlement, ShopDetails, SoldRow, StockCorrection, StockCount, StockRecord,
    StoredSale, Supplier, SupplierEntry, SupplierOwing, SupplierPayment, TOKEN_LIFETIME,
    TakingsRow, TenantRecord, TerminalHealth, TerminalRecord, UnreadableChange, VatRow, VatSummary,
    WaivedRow, describe_quarantine,
};

/// Decode a stored catalogue payload under the schema it was written in.
///
/// Version 2 needs four attempts, because three fields were appended to
/// `ItemWire` while that number stayed put: rows stamped 2 exist in four
/// lengths, and a shop's oldest rows are the shortest. Tried longest first, and
/// only a decode that consumes the whole payload counts. postcard does not
/// complain about bytes left over, so a shorter shape reading a longer row
/// succeeds and silently drops the fields it has no room for, which is how a
/// category or a tax class would go missing without anybody being told.
pub(crate) fn decode_catalogue_payload(schema: i16, bytes: &[u8]) -> Option<ItemWire> {
    use openpos_core::protocol::{ItemWireV1, ItemWireV2, ItemWireV2FromATill, ItemWireV2Supply};

    /// Decode, and only accept it if nothing is left over.
    fn whole<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
        let (read, rest) = postcard::take_from_bytes::<T>(bytes).ok()?;
        rest.is_empty().then_some(read)
    }

    match schema {
        3 => whole(bytes),
        2 => whole::<ItemWire>(bytes)
            .or_else(|| whole::<ItemWireV2Supply>(bytes).map(ItemWireV2Supply::into_current))
            .or_else(|| {
                whole::<ItemWireV2FromATill>(bytes).map(ItemWireV2FromATill::into_current)
            })
            .or_else(|| whole::<ItemWireV2>(bytes).map(ItemWireV2::into_current)),
        1 => whole::<ItemWireV1>(bytes).map(ItemWireV1::into_current),
        // Written by a newer build than this one, on a shared database during a
        // rolling upgrade. Skipping is right: this build genuinely cannot read
        // it, and the newer one will send it again.
        _ => None,
    }
}

/// Migrations are embedded in the binary, so `docker compose up` needs no
/// separate migration step and cannot run a version that disagrees with the code.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub struct PgRepo {
    pool: PgPool,
}

impl PgRepo {
    /// Bring the schema up to date.
    ///
    /// Takes its own connection string because migrating needs rights the
    /// running application must not have. The app role can read and write rows;
    /// it cannot create or drop a table, which means a compromised server cannot
    /// quietly redefine the schema it is audited against. Run this once at
    /// deploy time with an administrative URL.
    pub async fn migrate(admin_url: &str) -> std::result::Result<(), sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        MIGRATOR.run(&pool).await?;
        pool.close().await;
        Ok(())
    }

    /// Connect as the application.
    ///
    /// Does not migrate. The schema is expected to be current already, which is
    /// what lets this role be restricted to reading and writing rows.
    pub async fn connect(
        url: &str,
        max_connections: u32,
    ) -> std::result::Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Whether this connection can see past the shop boundary.
    ///
    /// Every table here has row level security forced on it, and that is the
    /// whole of the isolation: the explicit tenant predicates in these queries
    /// are belt and braces, and taking all of them out changes no answer. A
    /// role that bypasses the policies, which a superuser does by definition,
    /// therefore has no boundary at all, and one shop reads another's takings
    /// with nothing anywhere saying so.
    ///
    /// The mistake is a single character in a connection string, `postgres`
    /// where `openpos_app` was meant, and it looks exactly like a working
    /// server. So it is asked at startup rather than discovered.
    pub async fn can_see_every_shop(&self) -> std::result::Result<bool, sqlx::Error> {
        let row: Option<(bool, bool)> = sqlx::query_as(
            "select rolsuper, rolbypassrls from pg_roles where rolname = current_user",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some_and(|(superuser, bypasses)| superuser || bypasses))
    }

    /// Begin a transaction scoped to one shop.
    ///
    /// `set_config` with `is_local = true` ties the setting to this transaction,
    /// so it cannot leak to the next request that borrows the same pooled
    /// connection. Using `SET LOCAL` with an interpolated string would be an
    /// injection hole; this form binds a parameter.
    async fn scoped(&self, tenant: u128) -> Result<Transaction<'_, Postgres>> {
        let mut transaction = self.pool.begin().await.map_err(|_| RepoError::Backend)?;
        sqlx::query("select set_config('openpos.tenant_id', $1, true)")
            .bind(Uuid::from_u128(tenant).to_string())
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        Ok(transaction)
    }

    /// Create a shop and a terminal. Used by the back office and by tests.
    pub async fn enrol(&self, tenant: u128, terminal: u128, label: &str) -> Result<()> {
        // Both rows are written inside the same scoped transaction, including
        // the tenant itself. The write policy permits creating exactly the shop
        // the transaction is scoped to, and nothing else, so this cannot be used
        // to conjure a row belonging to somebody else.
        let mut transaction = self.scoped(tenant).await?;

        sqlx::query("insert into tenant (id, name) values ($1, $2) on conflict (id) do nothing")
            .bind(Uuid::from_u128(tenant))
            .bind(label)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        sqlx::query(
            "insert into terminal (tenant_id, id, label) values ($1, $2, $3)
             on conflict (tenant_id, id) do nothing",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(terminal))
        .bind(label)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    /// Enrol a terminal and hand back its credential.
    ///
    /// The token is returned once and never again: only its hash is stored, so
    /// a lost token is replaced rather than recovered. That is the property
    /// worth having, because it means a copy of the database is not a set of
    /// working credentials.
    pub async fn enrol_with_token(
        &self,
        tenant: u128,
        terminal: u128,
        label: &str,
    ) -> Result<Token> {
        self.enrol(tenant, terminal, label).await?;
        let token = Token::generate();
        // The first credential a shop gets is an owner's. Somebody has to be
        // able to mint the rest, and issuing an owner code requires already
        // being one.
        self.store_token(
            Caller {
                tenant,
                terminal,
                role: Role::Owner,
            },
            &token.hash(),
        )
        .await?;
        Ok(token)
    }

    async fn append_change(
        &self,
        tenant: u128,
        kind: i16,
        item_id: u128,
        payload: Option<Vec<u8>>,
    ) -> Result<u64> {
        let mut transaction = self.scoped(tenant).await?;

        // Bump the tenant's own counter and take the new value in one statement,
        // so two concurrent edits cannot be handed the same sequence.
        let row = sqlx::query(
            "update tenant set catalogue_seq = catalogue_seq + 1
             where id = $1 returning catalogue_seq",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        let seq: i64 = row
            .try_get("catalogue_seq")
            .map_err(|_| RepoError::Backend)?;

        sqlx::query(
            "insert into catalogue_change (tenant_id, seq, kind, item_id, payload, schema)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(seq)
        .bind(kind)
        .bind(Uuid::from_u128(item_id))
        .bind(payload)
        .bind(i16::from(CATALOGUE_SCHEMA))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(u64::try_from(seq).unwrap_or_default())
    }

    /// Ids and reasons for the unresolved queue, in queue order.
    ///
    /// The narrow view. [`Repository::repair_queue`] is what the back office
    /// renders; this stays because it is the shape the schema-level tests assert
    /// on, and it is defined in terms of the same query so the two can never
    /// disagree about which sales are still outstanding.
    pub async fn quarantined(&self, tenant: u128, limit: i64) -> Result<Vec<(Uuid, String)>> {
        let queue = self
            .repair_queue(tenant, u32::try_from(limit).unwrap_or(u32::MAX))
            .await?;
        Ok(queue
            .into_iter()
            .map(|item| (Uuid::from_u128(item.id), item.reason))
            .collect())
    }
}

/// Read a timestamp as milliseconds since the Unix epoch.
///
/// Postgres hands back a `numeric` for `extract`, so the cast to bigint happens
/// in the query and this only widens. Absent stays absent: a terminal never
/// heard from must not be reported as having synced in 1970.
fn millis(row: &sqlx::postgres::PgRow, column: &str) -> Result<Option<u64>> {
    let raw: Option<i64> = row.try_get(column).map_err(|_| RepoError::Backend)?;
    Ok(raw.map(|value| u64::try_from(value).unwrap_or_default()))
}

impl PgRepo {
    /// Bring a token's expiry forward to now. Exists so a test can reach the
    /// expired state without waiting a year or sleeping.
    ///
    /// # Errors
    /// When the database is unreachable.
    pub async fn expire_token_for_test(&self, token: &TokenHash) -> Result<()> {
        sqlx::query("update terminal_token set expires_at = now() - interval '1 second' where token_hash = $1")
            .bind(token.as_bytes())
            .execute(&self.pool)
            .await
            .map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    /// When a token lapses, in milliseconds since the epoch. `None` for a token
    /// with no expiry, which is any issued before the lifetime existed.
    ///
    /// # Errors
    /// When the database is unreachable.
    pub async fn token_expiry_for_test(&self, token: &TokenHash) -> Result<Option<u64>> {
        let row: Option<Option<i64>> = sqlx::query_scalar(
            "select (extract(epoch from expires_at) * 1000)::bigint
             from terminal_token where token_hash = $1",
        )
        .bind(token.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(row
            .flatten()
            .map(|ms| u64::try_from(ms).unwrap_or_default()))
    }

    /// When a token was last presented, for the terminal health view and for
    /// tests.
    ///
    /// # Errors
    /// When the database is unreachable.
    pub async fn token_last_used_for_test(&self, token: &TokenHash) -> Result<Option<u64>> {
        let row: Option<Option<i64>> = sqlx::query_scalar(
            "select (extract(epoch from last_used_at) * 1000)::bigint
             from terminal_token where token_hash = $1",
        )
        .bind(token.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(row
            .flatten()
            .map(|ms| u64::try_from(ms).unwrap_or_default()))
    }
}

/// One counted drawer, out of a row. Shared by the reader the back office uses
/// and the one an export pages through, because two of these would drift and the
/// one used least would be the one wrong.
fn shift_from_row(row: sqlx::postgres::PgRow) -> Result<ClosedShift> {
    let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
    let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
    let opened: i64 = row
        .try_get("opened_at_ms")
        .map_err(|_| RepoError::Backend)?;
    let closed: i64 = row
        .try_get("closed_at_ms")
        .map_err(|_| RepoError::Backend)?;
    let sales: i32 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
    let closed_by: Option<Uuid> = row.try_get("closed_by").map_err(|_| RepoError::Backend)?;
    Ok(ClosedShift {
        id: id.as_u128(),
        terminal: terminal.as_u128(),
        // Nobody, for a drawer counted by a build that did not write it down.
        closed_by: closed_by.map_or(0, |who| who.as_u128()),
        closed_by_name: row
            .try_get("closed_by_name")
            .map_err(|_| RepoError::Backend)?,
        opened_at_ms: u64::try_from(opened).unwrap_or(0),
        closed_at_ms: u64::try_from(closed).unwrap_or(0),
        opening_float_minor: row
            .try_get("opening_float_minor")
            .map_err(|_| RepoError::Backend)?,
        sales: u32::try_from(sales).unwrap_or(0),
        cash_sales_minor: row
            .try_get("cash_sales_minor")
            .map_err(|_| RepoError::Backend)?,
        non_cash_sales_minor: row
            .try_get("non_cash_sales_minor")
            .map_err(|_| RepoError::Backend)?,
        cash_in_minor: row
            .try_get("cash_in_minor")
            .map_err(|_| RepoError::Backend)?,
        cash_out_minor: row
            .try_get("cash_out_minor")
            .map_err(|_| RepoError::Backend)?,
        expected_cash_minor: row
            .try_get("expected_cash_minor")
            .map_err(|_| RepoError::Backend)?,
        counted_cash_minor: row
            .try_get("counted_cash_minor")
            .map_err(|_| RepoError::Backend)?,
        variance_minor: row
            .try_get("variance_minor")
            .map_err(|_| RepoError::Backend)?,
    })
}

/// Move the shop's settings counter on, in the transaction that changed them.
///
/// One counter for the people, the shop and the account customers together: a
/// till that has to re-read one of them may as well re-read all three, and three
/// counters would be three chances to forget to move one.
async fn bump_settings(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: u128,
) -> Result<()> {
    sqlx::query("update tenant set settings_seq = settings_seq + 1 where id = $1")
        .bind(Uuid::from_u128(tenant))
        .execute(&mut **transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
    Ok(())
}

impl Repository for PgRepo {
    async fn has_sale(&self, tenant: u128, id: u128) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "-- every sale: a replay check has to recognise a sale the shop struck
             --   out, or the till would be told to send it again
             select 1 as found from sale where id = $1",
        )
        .bind(Uuid::from_u128(id))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(row.is_some())
    }

    async fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "-- every sale: a number printed on paper is used whatever was later
             --   decided about the sale it was printed on
             select 1 as found from sale where receipt_no = $1 and receipt_epoch = $2",
        )
        .bind(receipt_no)
        .bind(i64::try_from(epoch).unwrap_or(i64::MAX))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(row.is_some())
    }

    async fn store_sale(&self, sale: StoredSale) -> Result<()> {
        let mut transaction = self.scoped(sale.tenant).await?;

        // Returning the id says whether this actually stored a sale. The
        // account entries below hang off that: a sale already here has already
        // put whatever it put on somebody's account, and a second copy naming
        // somebody else would be a debt with no sale behind it.
        let stored = sqlx::query(
            "insert into sale (tenant_id, id, terminal_id, receipt_no, receipt_epoch,
                               rung_at_ms, total_minor, payload, quarantine, refund_of,
                               cash_minor, cost_minor, cost_known, quarantine_kind)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(sale.tenant))
        .bind(Uuid::from_u128(sale.id))
        .bind(Uuid::from_u128(sale.terminal))
        .bind(sale.receipt_no.as_deref())
        .bind(
            sale.receipt_epoch
                .map(|epoch| i64::try_from(epoch).unwrap_or(i64::MAX)),
        )
        .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
        .bind(sale.total_minor)
        .bind(&sale.payload)
        .bind(sale.quarantine.as_ref().map(describe_quarantine))
        .bind(sale.refund_of.as_deref())
        .bind(sale.cash_minor)
        .bind(sale.cost_minor)
        .bind(sale.cost_known)
        // The reason itself, beside the sentence. A screen cannot translate
        // prose, and a shop reading Bangla is being asked to judge a sale on
        // the strength of one English paragraph.
        .bind(
            sale.quarantine
                .as_ref()
                .and_then(|reason| postcard::to_allocvec(reason).ok()),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        for (item_id, qty_milli) in &sale.stock {
            sqlx::query(
                "insert into stock_movement
                    (tenant_id, source_id, source_kind, item_id, qty_milli, occurred_at_ms)
                 values ($1, $2, 1, $3, $4, $5)
                 on conflict (tenant_id, source_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(Uuid::from_u128(*item_id))
            .bind(*qty_milli)
            .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        for (seq, reason) in sale.overrides.iter().enumerate() {
            sqlx::query(
                "insert into sale_override (tenant_id, sale_id, seq, reason)
                 values ($1, $2, $3, $4)
                 on conflict (tenant_id, sale_id, seq) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(i32::try_from(seq).unwrap_or(i32::MAX))
            .bind(reason)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        for (bp, net, vat, supply) in &sale.vat {
            sqlx::query(
                "insert into sale_vat (tenant_id, sale_id, vat_bp, net_minor, vat_minor, supply)
                 values ($1, $2, $3, $4, $5, $6)
                 on conflict (tenant_id, sale_id, vat_bp, supply) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(i32::try_from(*bp).unwrap_or(i32::MAX))
            .bind(*net)
            .bind(*vat)
            .bind(i16::from(*supply))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        if stored.is_some() {
            for charge in &sale.on_account {
                // Keyed on the sale and the person, so a till resending a sale
                // it was not told about does not double what somebody owes.
                sqlx::query(
                    "insert into account_entry
                        (tenant_id, person_key, person_name, source_id, kind, amount_minor, at_ms)
                     values ($1, $2, $3, $4, 1, $5, $6)
                     on conflict do nothing",
                )
                .bind(Uuid::from_u128(sale.tenant))
                .bind(&charge.person_key)
                .bind(&charge.person_name)
                .bind(Uuid::from_u128(sale.id))
                .bind(charge.amount_minor)
                .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;
            }
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn admit_sale(&self, sale: StoredSale) -> Result<Admission> {
        let mut transaction = self.scoped(sale.tenant).await?;

        // The insert is the idempotency check. `do nothing` returning no row
        // means a sale with this id is already here, decided by the primary key
        // rather than by a read that another connection can race.
        let inserted = sqlx::query(
            "insert into sale (tenant_id, id, terminal_id, receipt_no, receipt_epoch,
                               rung_at_ms, total_minor, payload, quarantine, refund_of,
                               cash_minor, cost_minor, cost_known, quarantine_kind)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(sale.tenant))
        .bind(Uuid::from_u128(sale.id))
        .bind(Uuid::from_u128(sale.terminal))
        .bind(sale.receipt_no.as_deref())
        .bind(
            sale.receipt_epoch
                .map(|epoch| i64::try_from(epoch).unwrap_or(i64::MAX)),
        )
        .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
        .bind(sale.total_minor)
        .bind(&sale.payload)
        .bind(sale.quarantine.as_ref().map(describe_quarantine))
        .bind(sale.refund_of.as_deref())
        .bind(sale.cash_minor)
        .bind(sale.cost_minor)
        .bind(sale.cost_known)
        // The reason itself, beside the sentence. A screen cannot translate
        // prose, and a shop reading Bangla is being asked to judge a sale on
        // the strength of one English paragraph.
        .bind(
            sale.quarantine
                .as_ref()
                .and_then(|reason| postcard::to_allocvec(reason).ok()),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        if inserted.is_none() {
            transaction.commit().await.map_err(|_| RepoError::Backend)?;
            return Ok(Admission::AlreadyStored);
        }

        let mut admission = Admission::Stored;
        if let (Some(receipt), Some(epoch)) = (sale.receipt_no.as_deref(), sale.receipt_epoch) {
            // Claiming the number is one insert against a primary key, so two
            // pushes carrying the same number at the same moment cannot both
            // win. The loser is told who holds it.
            let claim = sqlx::query(
                "insert into receipt_claim (tenant_id, receipt_epoch, receipt_no, sale_id)
                 values ($1, $2, $3, $4)
                 on conflict (tenant_id, receipt_epoch, receipt_no) do nothing
                 returning sale_id",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(i64::try_from(epoch).unwrap_or(i64::MAX))
            .bind(receipt)
            .bind(Uuid::from_u128(sale.id))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            if claim.is_none() {
                let holder: Option<Uuid> = sqlx::query_scalar(
                    "select sale_id from receipt_claim
                     where tenant_id = $1 and receipt_epoch = $2 and receipt_no = $3",
                )
                .bind(Uuid::from_u128(sale.tenant))
                .bind(i64::try_from(epoch).unwrap_or(i64::MAX))
                .bind(receipt)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;

                let reason = QuarantineReason::DuplicateReceiptNumber {
                    receipt_no: receipt.to_owned(),
                };
                sqlx::query(
                    "update sale set quarantine = $1, quarantine_kind = $4
                     where tenant_id = $2 and id = $3",
                )
                .bind(describe_quarantine(&reason))
                .bind(Uuid::from_u128(sale.tenant))
                .bind(Uuid::from_u128(sale.id))
                .bind(postcard::to_allocvec(&reason).ok())
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;

                admission = Admission::DuplicateReceipt {
                    held_by: holder.map(|id| id.as_u128()).unwrap_or_default(),
                };
            }
        }

        for (item_id, qty_milli) in &sale.stock {
            sqlx::query(
                "insert into stock_movement
                    (tenant_id, source_id, source_kind, item_id, qty_milli, occurred_at_ms)
                 values ($1, $2, 1, $3, $4, $5)
                 on conflict (tenant_id, source_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(Uuid::from_u128(*item_id))
            .bind(*qty_milli)
            .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        for (seq, reason) in sale.overrides.iter().enumerate() {
            sqlx::query(
                "insert into sale_override (tenant_id, sale_id, seq, reason)
                 values ($1, $2, $3, $4)
                 on conflict (tenant_id, sale_id, seq) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(i32::try_from(seq).unwrap_or(i32::MAX))
            .bind(reason)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        for (bp, net, vat, supply) in &sale.vat {
            sqlx::query(
                "insert into sale_vat (tenant_id, sale_id, vat_bp, net_minor, vat_minor, supply)
                 values ($1, $2, $3, $4, $5, $6)
                 on conflict (tenant_id, sale_id, vat_bp, supply) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(i32::try_from(*bp).unwrap_or(i32::MAX))
            .bind(*net)
            .bind(*vat)
            .bind(i16::from(*supply))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        for charge in &sale.on_account {
            // Keyed on the sale and the person, so a till resending a sale it
            // was not told about does not double what somebody owes.
            sqlx::query(
                "insert into account_entry
                    (tenant_id, person_key, person_name, source_id, kind, amount_minor, at_ms)
                 values ($1, $2, $3, $4, 1, $5, $6)
                 on conflict do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(&charge.person_key)
            .bind(&charge.person_name)
            .bind(Uuid::from_u128(sale.id))
            .bind(charge.amount_minor)
            .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(admission)
    }

    async fn terminal_enrolled_at(&self, tenant: u128, terminal: u128) -> Result<Option<u64>> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "select (extract(epoch from enrolled_at) * 1000)::bigint as enrolled_ms
               from terminal where id = $1",
        )
        .bind(Uuid::from_u128(terminal))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        match row {
            Some(row) => Ok(Some(millis(&row, "enrolled_ms")?.unwrap_or_default())),
            None => Ok(None),
        }
    }

    async fn issue_lease(&self, tenant: u128, terminal: u128, count: u32) -> Result<LeaseRecord> {
        let mut transaction = self.scoped(tenant).await?;

        // One statement takes the block and advances the counter, so two tills
        // asking at the same moment cannot be handed overlapping numbers.
        let span = i64::from(count.max(1));
        let row = sqlx::query(
            "update terminal set next_receipt = next_receipt + $1
             where id = $2 returning next_receipt, epoch",
        )
        .bind(span)
        .bind(Uuid::from_u128(terminal))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?
        .ok_or(RepoError::UnknownTerminal)?;

        let after: i64 = row
            .try_get("next_receipt")
            .map_err(|_| RepoError::Backend)?;
        let epoch: i64 = row.try_get("epoch").map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;

        let last = after.saturating_sub(1);
        Ok(LeaseRecord {
            tenant,
            terminal,
            epoch: u64::try_from(epoch).unwrap_or(1),
            first: u64::try_from(last.saturating_sub(span).saturating_add(1)).unwrap_or(1),
            last: u64::try_from(last).unwrap_or(1),
        })
    }

    async fn authenticate(&self, token: &TokenHash) -> Result<Option<Caller>> {
        // Runs outside a tenant-scoped transaction on purpose: this query is
        // what establishes which tenant the request belongs to, so it cannot
        // itself be filtered by one. The table holds hashes and identifiers
        // only, which is why it is the single exception to row-level security.
        // One statement checks the credential, rejects an expired one, and
        // stamps it as used. Doing the stamp as a second query would either add
        // a round trip to every request a shop makes, or be skipped on the
        // error path, which is exactly the path worth knowing about.
        let row = sqlx::query(
            "update terminal_token set last_used_at = now()
             where token_hash = $1
               and revoked_at is null
               and (expires_at is null or expires_at > now())
             returning tenant_id, terminal_id, role",
        )
        .bind(token.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let tenant: Uuid = row.try_get("tenant_id").map_err(|_| RepoError::Backend)?;
        let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
        let role: i16 = row.try_get("role").map_err(|_| RepoError::Backend)?;
        Ok(Some(Caller {
            tenant: tenant.as_u128(),
            terminal: terminal.as_u128(),
            role: Role::from_i16(role),
        }))
    }

    async fn store_token(&self, caller: Caller, token: &TokenHash) -> Result<()> {
        sqlx::query(
            "insert into terminal_token (token_hash, tenant_id, terminal_id, expires_at, role)
             values ($1, $2, $3, now() + $4::interval, $5)
             on conflict (token_hash) do nothing",
        )
        .bind(token.as_bytes())
        .bind(Uuid::from_u128(caller.tenant))
        .bind(Uuid::from_u128(caller.terminal))
        .bind(format!("{} seconds", TOKEN_LIFETIME.as_secs()))
        .bind(caller.role.as_i16())
        .execute(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn renew_token(
        &self,
        caller: Caller,
        previous: &TokenHash,
        replacement: &TokenHash,
        overlap: Duration,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(|_| RepoError::Backend)?;

        sqlx::query(
            "insert into terminal_token (token_hash, tenant_id, terminal_id, expires_at)
             values ($1, $2, $3, now() + $4::interval)
             on conflict (token_hash) do nothing",
        )
        .bind(replacement.as_bytes())
        .bind(Uuid::from_u128(caller.tenant))
        .bind(Uuid::from_u128(caller.terminal))
        .bind(format!("{} seconds", TOKEN_LIFETIME.as_secs()))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        // `least` so renewing never extends a credential. A token already due to
        // lapse sooner than the overlap keeps its earlier deadline, or a device
        // could hold one alive indefinitely by renewing in a loop.
        sqlx::query(
            "update terminal_token
             set expires_at = least(coalesce(expires_at, 'infinity'::timestamptz),
                                    now() + $2::interval)
             where token_hash = $1",
        )
        .bind(previous.as_bytes())
        .bind(format!("{} seconds", overlap.as_secs()))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn record_count(&self, tenant: u128, count: &StockCount) -> Result<()> {
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "insert into stock_count
                (tenant_id, id, item_id, counted_milli, counted_at_ms, counted_by, note)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (tenant_id, id) do nothing",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(count.id))
        .bind(Uuid::from_u128(count.item_id))
        .bind(count.counted_milli)
        .bind(i64::try_from(count.counted_at_ms).unwrap_or(i64::MAX))
        .bind(Uuid::from_u128(count.counted_by))
        .bind(count.note.as_deref())
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn on_hand(&self, tenant: u128, item: u128) -> Result<OnHand> {
        let mut transaction = self.scoped(tenant).await?;

        // The newest count by the device clock. A count taken later describes a
        // later shelf, whatever order the counts reached the server in.
        let barrier = sqlx::query(
            "select counted_milli, counted_at_ms, recorded_at from stock_count
             where item_id = $1 order by counted_at_ms desc limit 1",
        )
        .bind(Uuid::from_u128(item))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(barrier) = barrier else {
            // Never counted, so there is no barrier and the running total from
            // the day the item appeared is the best answer available.
            let total: Option<i64> = sqlx::query_scalar(
                // A sale somebody struck out did not happen, so what it says
                // left the shelf did not leave it.
                "select coalesce(sum(m.qty_milli), 0)::bigint
                   from stock_movement m
                   left join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
                        and m.source_kind = 1
                  where m.item_id = $1 and s.resolution_kept is not false",
            )
            .bind(Uuid::from_u128(item))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            return Ok(OnHand {
                item_id: item,
                qty_milli: total.unwrap_or_default(),
                counted_at_ms: None,
                unreconciled_milli: 0,
                unreconciled_sales: 0,
            });
        };

        let counted: i64 = barrier
            .try_get("counted_milli")
            .map_err(|_| RepoError::Backend)?;
        let counted_at: i64 = barrier
            .try_get("counted_at_ms")
            .map_err(|_| RepoError::Backend)?;

        // Three buckets, by when the sale was rung against when it landed. The
        // barrier is re-selected inside the statement rather than passed back
        // in, so the comparison happens in the database's own time type and no
        // timestamp crosses the boundary to be rounded on the way.
        //
        // Rung at or after the count: the counter could not have seen it, so it
        // moves the figure. Rung before and already stored when the count was
        // recorded: the counter saw the shelf as it was, so applying it again
        // would decrement goods already missing from the count. Rung before but
        // arriving after: nobody can say, and that bucket is separated rather
        // than guessed at.
        let row = sqlx::query(
            "with barrier as (
                 select counted_at_ms, recorded_at from stock_count
                 where item_id = $1 order by counted_at_ms desc limit 1
             )
             select
                coalesce(sum(m.qty_milli) filter (
                    where m.occurred_at_ms >= b.counted_at_ms
                ), 0)::bigint as after_count,
                coalesce(sum(m.qty_milli) filter (
                    where m.occurred_at_ms < b.counted_at_ms and m.recorded_at > b.recorded_at
                ), 0)::bigint as late,
                count(distinct m.source_id) filter (
                    where m.occurred_at_ms < b.counted_at_ms and m.recorded_at > b.recorded_at
                ) as late_sales
             from barrier b
             left join stock_movement m on m.item_id = $1
             left join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
                  and m.source_kind = 1
             where s.resolution_kept is not false",
        )
        .bind(Uuid::from_u128(item))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let after: i64 = row.try_get("after_count").map_err(|_| RepoError::Backend)?;
        let late: i64 = row.try_get("late").map_err(|_| RepoError::Backend)?;
        let late_sales: i64 = row.try_get("late_sales").map_err(|_| RepoError::Backend)?;

        Ok(OnHand {
            item_id: item,
            qty_milli: counted.saturating_add(after),
            counted_at_ms: u64::try_from(counted_at).ok(),
            unreconciled_milli: late,
            unreconciled_sales: usize::try_from(late_sales).unwrap_or_default(),
        })
    }

    /// The same question as `on_hand`, asked once for many items.
    ///
    /// One transaction and two statements rather than one transaction and three
    /// statements per item. A till refreshing two hundred items was six hundred
    /// round trips, and a shop with eight hundred lines took twenty minutes to
    /// get round its own catalogue: the figure behind a refusal at the far end
    /// could be that stale, and the refusal is the whole point of asking.
    ///
    /// Every expression here is the one above, widened by a `group by` and a
    /// barrier picked per item rather than once. Not "a set-wide version", which
    /// is what this codebase keeps refusing to build, because a second query
    /// with its own idea of what a barrier means is a second answer that
    /// disagrees with the first on the day it matters. It is the same answer,
    /// and `postgres_repo.rs` runs both and compares them rather than taking
    /// this comment's word for it.
    async fn on_hand_many(&self, tenant: u128, items: &[u128]) -> Result<Vec<OnHand>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let mut transaction = self.scoped(tenant).await?;
        let wanted: Vec<Uuid> = items.iter().copied().map(Uuid::from_u128).collect();

        let rows = sqlx::query(
            // The newest count per item by the device clock, the same ordering
            // the single-item query uses: a count taken later describes a later
            // shelf, whatever order the counts reached the server in.
            "with barrier as (
                 select distinct on (item_id)
                        item_id, counted_milli, counted_at_ms, recorded_at
                 from stock_count
                 where item_id = any($1)
                 order by item_id, counted_at_ms desc
             ),
             moved as (
                 select m.item_id,
                    coalesce(sum(m.qty_milli) filter (
                        where b.counted_at_ms is null
                           or m.occurred_at_ms >= b.counted_at_ms
                    ), 0)::bigint as after_count,
                    coalesce(sum(m.qty_milli) filter (
                        where b.counted_at_ms is not null
                          and m.occurred_at_ms < b.counted_at_ms
                          and m.recorded_at > b.recorded_at
                    ), 0)::bigint as late,
                    count(distinct m.source_id) filter (
                        where b.counted_at_ms is not null
                          and m.occurred_at_ms < b.counted_at_ms
                          and m.recorded_at > b.recorded_at
                    ) as late_sales
                 from stock_movement m
                 left join barrier b on b.item_id = m.item_id
                 left join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
                      and m.source_kind = 1
                 where m.item_id = any($1) and s.resolution_kept is not false
                 group by m.item_id
             )
             select w.item_id,
                    b.counted_milli,
                    b.counted_at_ms,
                    coalesce(mo.after_count, 0)::bigint as after_count,
                    coalesce(mo.late, 0)::bigint as late,
                    coalesce(mo.late_sales, 0)::bigint as late_sales
             from unnest($1) as w(item_id)
             left join barrier b on b.item_id = w.item_id
             left join moved mo on mo.item_id = w.item_id",
        )
        .bind(&wanted)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        // Keyed by item and read back in the order asked, because a caller
        // matching answers to questions by position would silently mis-attribute
        // every figure the day the database returns them in another order. A
        // shelf figure against the wrong item is a refusal against the wrong
        // item.
        let mut by_item: std::collections::HashMap<u128, OnHand> =
            std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let counted: Option<i64> = row
                .try_get("counted_milli")
                .map_err(|_| RepoError::Backend)?;
            let counted_at: Option<i64> = row
                .try_get("counted_at_ms")
                .map_err(|_| RepoError::Backend)?;
            let after: i64 = row.try_get("after_count").map_err(|_| RepoError::Backend)?;
            let late: i64 = row.try_get("late").map_err(|_| RepoError::Backend)?;
            let late_sales: i64 = row.try_get("late_sales").map_err(|_| RepoError::Backend)?;
            let item = id.as_u128();
            by_item.insert(
                item,
                OnHand {
                    item_id: item,
                    qty_milli: counted.unwrap_or_default().saturating_add(after),
                    // Only when there was a count. An item nobody has counted has
                    // no barrier, and saying it was counted at the epoch is worse
                    // than saying nothing.
                    counted_at_ms: counted_at.and_then(|at| u64::try_from(at).ok()),
                    unreconciled_milli: late,
                    unreconciled_sales: usize::try_from(late_sales).unwrap_or_default(),
                },
            );
        }

        Ok(items
            .iter()
            .map(|item| {
                by_item.get(item).cloned().unwrap_or(OnHand {
                    item_id: *item,
                    qty_milli: 0,
                    counted_at_ms: None,
                    unreconciled_milli: 0,
                    unreconciled_sales: 0,
                })
            })
            .collect())
    }

    async fn operators(&self, tenant: u128) -> Result<Vec<OperatorRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, name, pin_salt, pin_rounds, pin_key, max_discount_bp,
                    may_override_price, may_refund, may_void_line, may_authorise,
                    may_open_drawer, may_close_shift, active
             from operator order by name",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let rounds: i32 = row.try_get("pin_rounds").map_err(|_| RepoError::Backend)?;
            let ceiling: i32 = row
                .try_get("max_discount_bp")
                .map_err(|_| RepoError::Backend)?;
            found.push(OperatorRecord {
                id: id.as_u128(),
                name: row.try_get("name").map_err(|_| RepoError::Backend)?,
                pin_salt: row.try_get("pin_salt").map_err(|_| RepoError::Backend)?,
                pin_rounds: u32::try_from(rounds).unwrap_or_default(),
                pin_key: row.try_get("pin_key").map_err(|_| RepoError::Backend)?,
                max_discount_bp: u32::try_from(ceiling).unwrap_or_default(),
                may_override_price: row
                    .try_get("may_override_price")
                    .map_err(|_| RepoError::Backend)?,
                may_refund: row.try_get("may_refund").map_err(|_| RepoError::Backend)?,
                may_void_line: row
                    .try_get("may_void_line")
                    .map_err(|_| RepoError::Backend)?,
                may_authorise: row
                    .try_get("may_authorise")
                    .map_err(|_| RepoError::Backend)?,
                may_open_drawer: row
                    .try_get("may_open_drawer")
                    .map_err(|_| RepoError::Backend)?,
                may_close_shift: row
                    .try_get("may_close_shift")
                    .map_err(|_| RepoError::Backend)?,
                active: row.try_get("active").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn put_operator(&self, tenant: u128, operator: &OperatorRecord) -> Result<()> {
        // Checked here as well as by the column, so a caller gets a refusal it
        // can act on rather than a database error it cannot read.
        if operator.name.trim().is_empty() || operator.pin_rounds < 1_000 {
            return Err(RepoError::Invalid);
        }
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "insert into operator
                (tenant_id, id, name, pin_salt, pin_rounds, pin_key, max_discount_bp,
                 may_override_price, may_refund, may_void_line, may_authorise,
                 may_open_drawer, may_close_shift, active)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             on conflict (tenant_id, id) do update
               set name = excluded.name,
                   pin_salt = excluded.pin_salt,
                   pin_rounds = excluded.pin_rounds,
                   pin_key = excluded.pin_key,
                   max_discount_bp = excluded.max_discount_bp,
                   may_override_price = excluded.may_override_price,
                   may_refund = excluded.may_refund,
                   may_void_line = excluded.may_void_line,
                   may_authorise = excluded.may_authorise,
                   may_open_drawer = excluded.may_open_drawer,
                   may_close_shift = excluded.may_close_shift,
                   active = excluded.active",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(operator.id))
        .bind(&operator.name)
        .bind(&operator.pin_salt)
        .bind(i32::try_from(operator.pin_rounds).unwrap_or(i32::MAX))
        .bind(&operator.pin_key)
        .bind(i32::try_from(operator.max_discount_bp).unwrap_or(i32::MAX))
        .bind(operator.may_override_price)
        .bind(operator.may_refund)
        .bind(operator.may_void_line)
        .bind(operator.may_authorise)
        .bind(operator.may_open_drawer)
        .bind(operator.may_close_shift)
        .bind(operator.active)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        bump_settings(&mut transaction, tenant).await?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn set_operator_pin(
        &self,
        tenant: u128,
        operator_id: u128,
        salt: &[u8],
        rounds: u32,
        key: &[u8],
    ) -> Result<()> {
        if rounds < 1_000 || salt.is_empty() || key.is_empty() {
            return Err(RepoError::Invalid);
        }
        let mut transaction = self.scoped(tenant).await?;
        // The three credential columns and nothing else, so a PIN change cannot
        // rename somebody or give them the drawer by carrying a stale field.
        let changed = sqlx::query(
            "update operator set pin_salt = $3, pin_rounds = $4, pin_key = $5
              where tenant_id = $1 and id = $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(operator_id))
        .bind(salt)
        .bind(i32::try_from(rounds).unwrap_or(i32::MAX))
        .bind(key)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?
        .rows_affected();

        if changed == 0 {
            return Err(RepoError::Invalid);
        }
        bump_settings(&mut transaction, tenant).await?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn amend_operator(&self, tenant: u128, amended: &AmendedOperator) -> Result<()> {
        // Checked here as well as by the column, so a caller gets a refusal it
        // can act on rather than a database error it cannot read.
        if amended.name.trim().is_empty() {
            return Err(RepoError::Invalid);
        }
        let mut transaction = self.scoped(tenant).await?;
        // Every column but the three that make up the PIN. Listing them rather
        // than writing the whole row is what makes it impossible to clear a
        // credential from here by forgetting a field.
        let changed = sqlx::query(
            "update operator
                set name = $3, max_discount_bp = $4, may_override_price = $5,
                    may_refund = $6, may_void_line = $7, may_authorise = $8,
                    may_open_drawer = $9, may_close_shift = $10, active = $11
              where tenant_id = $1 and id = $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(amended.id))
        .bind(&amended.name)
        .bind(i32::try_from(amended.max_discount_bp).unwrap_or(i32::MAX))
        .bind(amended.may_override_price)
        .bind(amended.may_refund)
        .bind(amended.may_void_line)
        .bind(amended.may_authorise)
        .bind(amended.may_open_drawer)
        .bind(amended.may_close_shift)
        .bind(amended.active)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?
        .rows_affected();

        // Nobody by that id, or somebody in another shop. Row-level security
        // makes the second look like the first from here, which is the point of
        // it, and either way the answer is that this did not happen.
        if changed == 0 {
            return Err(RepoError::Invalid);
        }
        bump_settings(&mut transaction, tenant).await?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn shop_details(&self, tenant: u128) -> Result<ShopDetails> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "select name, bin, address, phone, wallets, stock_rule from tenant where id = $1",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        // No such shop is a different answer from a shop with nothing filled
        // in, and a till told the second when the first is true would print
        // somebody else's blank header.
        let row = row.ok_or(RepoError::UnknownTerminal)?;
        Ok(ShopDetails {
            name: row.try_get("name").map_err(|_| RepoError::Backend)?,
            bin: row.try_get("bin").map_err(|_| RepoError::Backend)?,
            address: row.try_get("address").map_err(|_| RepoError::Backend)?,
            phone: row.try_get("phone").map_err(|_| RepoError::Backend)?,
            wallets: row.try_get("wallets").map_err(|_| RepoError::Backend)?,
            stock_rule: {
                let stored: i16 = row.try_get("stock_rule").map_err(|_| RepoError::Backend)?;
                u8::try_from(stored).unwrap_or(0)
            },
        })
    }

    async fn put_shop_details(&self, tenant: u128, details: &ShopDetails) -> Result<()> {
        if details.name.trim().is_empty() {
            // A receipt with no shop on it is not a receipt anybody can take
            // back to a shop.
            return Err(RepoError::Invalid);
        }
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "update tenant set name = $2, bin = $3, address = $4, phone = $5, wallets = $6,
                                stock_rule = $7
              where id = $1",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(&details.name)
        .bind(details.bin.as_deref())
        .bind(details.address.as_deref())
        .bind(details.phone.as_deref())
        .bind(&details.wallets)
        // Clamped here rather than at one caller: a rule this build does not
        // know would be read back as nothing anyway, and a bundle imported from
        // a file nobody wrote by hand is a caller too.
        .bind(i16::from(details.stock_rule.min(2)))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        bump_settings(&mut transaction, tenant).await?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn put_supplier(&self, tenant: u128, supplier: &Supplier) -> Result<()> {
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "insert into supplier (tenant_id, id, name, phone, bin, active)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (tenant_id, id) do update
               set name = excluded.name,
                   phone = excluded.phone,
                   bin = excluded.bin,
                   active = excluded.active",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(supplier.id))
        .bind(&supplier.name)
        .bind(supplier.phone.as_deref())
        .bind(supplier.bin.as_deref())
        .bind(supplier.active)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn suppliers(&self, tenant: u128) -> Result<Vec<Supplier>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query("select id, name, phone, bin, active from supplier order by name")
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            found.push(Supplier {
                id: id.as_u128(),
                name: row.try_get("name").map_err(|_| RepoError::Backend)?,
                phone: row.try_get("phone").map_err(|_| RepoError::Backend)?,
                bin: row.try_get("bin").map_err(|_| RepoError::Backend)?,
                active: row.try_get("active").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn takings(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<Vec<TakingsRow>> {
        let mut transaction = self.scoped(tenant).await?;

        // Grouped in the database rather than pulled and summed here: a busy
        // shop's day is thousands of rows, and the answer is one line per till.
        let rows = sqlx::query(
            "select terminal_id,
                    count(*)                                          as sales,
                    -- Cast, because sum() over bigint answers in numeric, and a
                    -- numeric read as an i64 is a panic in a request handler
                    -- rather than a wrong number. Only a real database says so.
                    coalesce(sum(total_minor), 0)::bigint              as total_minor,
                    count(*) filter (where quarantine is not null)    as needing_attention,
                    count(*) filter (where total_minor < 0)           as refunds,
                    coalesce(sum(total_minor) filter (where total_minor < 0), 0)::bigint
                                                                      as refunded_minor
               from sale
              where tenant_id = $1 and rung_at_ms between $2 and $3
                and resolution_kept is not false
              group by terminal_id
              order by terminal_id",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let found = rows
            .into_iter()
            .map(|row| {
                let terminal: Uuid = row.get("terminal_id");
                let sales: i64 = row.get("sales");
                let attention: i64 = row.get("needing_attention");
                let refunds: i64 = row.get("refunds");
                TakingsRow {
                    terminal: terminal.as_u128(),
                    sales: u64::try_from(sales).unwrap_or(0),
                    total_minor: row.get("total_minor"),
                    needing_attention: u64::try_from(attention).unwrap_or(0),
                    refunds: u64::try_from(refunds).unwrap_or(0),
                    refunded_minor: row.get("refunded_minor"),
                }
            })
            .collect();

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(found)
    }

    async fn put_shifts(&self, tenant: u128, shifts: &[ClosedShift]) -> Result<Vec<u128>> {
        let mut transaction = self.scoped(tenant).await?;
        let mut held = Vec::with_capacity(shifts.len());
        for shift in shifts {
            // Nothing to do on conflict: a counted drawer is a statement about a
            // period that has ended, and a till resending after a dropped reply
            // must not be able to restate it.
            sqlx::query(
                "insert into closed_shift
                    (tenant_id, id, terminal_id, opened_at_ms, closed_at_ms,
                     opening_float_minor, sales, cash_sales_minor, non_cash_sales_minor,
                     cash_in_minor, cash_out_minor, expected_cash_minor,
                     counted_cash_minor, variance_minor, closed_by, closed_by_name)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                 on conflict (tenant_id, id) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(shift.id))
            .bind(Uuid::from_u128(shift.terminal))
            .bind(i64::try_from(shift.opened_at_ms).unwrap_or(i64::MAX))
            .bind(i64::try_from(shift.closed_at_ms).unwrap_or(i64::MAX))
            .bind(shift.opening_float_minor)
            .bind(i32::try_from(shift.sales).unwrap_or(i32::MAX))
            .bind(shift.cash_sales_minor)
            .bind(shift.non_cash_sales_minor)
            .bind(shift.cash_in_minor)
            .bind(shift.cash_out_minor)
            .bind(shift.expected_cash_minor)
            .bind(shift.counted_cash_minor)
            .bind(shift.variance_minor)
            // Nobody, for a drawer counted by a build that did not write it down.
            .bind((shift.closed_by != 0).then(|| Uuid::from_u128(shift.closed_by)))
            .bind(&shift.closed_by_name)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            // Closed is closed. An open list that still shows a drawer somebody
            // counted an hour ago is a list an owner learns to ignore.
            sqlx::query(
                "delete from open_drawer
                  where tenant_id = $1 and terminal_id = $2 and shift_id = $3",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(shift.terminal))
            .bind(Uuid::from_u128(shift.id))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            held.push(shift.id);
        }
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(held)
    }

    async fn unreadable_changes(&self, tenant: u128, limit: u32) -> Result<Vec<UnreadableChange>> {
        let mut transaction = self.scoped(tenant).await?;
        // Read and tried, because "cannot decode" is not something SQL can ask.
        // Only upserts: a deletion carries no payload and cannot be unreadable.
        let rows = sqlx::query(
            "select seq, item_id, payload, schema from catalogue_change
              where kind = 1 order by seq limit $1",
        )
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::new();
        for row in rows {
            let schema: i16 = row.try_get("schema").map_err(|_| RepoError::Backend)?;
            let payload: Option<Vec<u8>> =
                row.try_get("payload").map_err(|_| RepoError::Backend)?;
            let Some(bytes) = payload else {
                continue;
            };
            if decode_catalogue_payload(schema, &bytes).is_some() {
                continue;
            }
            let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            found.push(UnreadableChange {
                seq: u64::try_from(seq).unwrap_or_default(),
                item_id: item.as_u128(),
                schema: u8::try_from(schema).unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn supplier_statement(
        &self,
        tenant: u128,
        supplier_id: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Vec<SupplierEntry>> {
        let mut transaction = self.scoped(tenant).await?;
        // One list, both directions, ordered by the clock: goods in and money
        // out are what the two sides put side by side when their figures
        // disagree. `round` matches `Minor::mul_qty`, as it does everywhere the
        // delivery total is computed.
        let rows = sqlx::query(
            "select r.received_at_ms as at_ms,
                    true             as delivered,
                    sum(round(l.unit_cost_minor::numeric * l.qty_milli / 1000))::bigint
                                     as amount_minor,
                    r.reference      as reference
               from goods_receipt r
               join goods_receipt_line l
                 on l.tenant_id = r.tenant_id and l.receipt_id = r.id
              where r.tenant_id = $1 and r.supplier_id = $2
                and r.received_at_ms between $3 and $4
              group by r.id, r.received_at_ms, r.reference
             union all
             select paid_at_ms, false, amount_minor,
                    case when note = '' then null else note end
               from supplier_payment
              where tenant_id = $1 and supplier_id = $2
                and paid_at_ms between $3 and $4
              order by at_ms asc, delivered desc",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(supplier_id))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let at_ms: i64 = row.try_get("at_ms").map_err(|_| RepoError::Backend)?;
            found.push(SupplierEntry {
                at_ms: u64::try_from(at_ms).unwrap_or_default(),
                delivered: row.try_get("delivered").map_err(|_| RepoError::Backend)?,
                amount_minor: row
                    .try_get("amount_minor")
                    .map_err(|_| RepoError::Backend)?,
                reference: row.try_get("reference").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn sold(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<SoldRow>> {
        let mut transaction = self.scoped(tenant).await?;
        // From the movements a sale wrote, which are the server's own reading of
        // what the lines said, joined to the sale for the clock: a shop buys
        // against what left the shelf on the day, not what a till synced later.
        // Negated, because stock moves the opposite way to a sale.
        let rows = sqlx::query(
            "select m.item_id,
                    (-sum(m.qty_milli))::bigint as qty_milli,
                    count(distinct m.source_id)::bigint as sales
               from stock_movement m
               join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
              where m.tenant_id = $1 and m.source_kind = 1
                and s.rung_at_ms between $2 and $3
                and s.resolution_kept is not false
              group by m.item_id
             having sum(m.qty_milli) <> 0
              order by qty_milli desc, m.item_id asc
              limit $4",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let sales: i64 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
            found.push(SoldRow {
                item_id: item.as_u128(),
                qty_milli: row.try_get("qty_milli").map_err(|_| RepoError::Backend)?,
                sales: u64::try_from(sales).unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn pay_supplier(&self, tenant: u128, payment: &SupplierPayment) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        // The insert is the idempotency check, as everywhere else here: money
        // the shop believes it has paid and has not is the same mistake as
        // money it believes it was given.
        let paid = sqlx::query(
            "insert into supplier_payment
                (tenant_id, id, supplier_id, amount_minor, paid_at_ms, note)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(payment.id))
        .bind(Uuid::from_u128(payment.supplier_id))
        .bind(payment.amount_minor)
        .bind(i64::try_from(payment.paid_at_ms).unwrap_or(i64::MAX))
        .bind(payment.note.as_deref().unwrap_or_default())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(paid.is_some())
    }

    async fn supplier_owing(&self, tenant: u128) -> Result<Vec<SupplierOwing>> {
        let mut transaction = self.scoped(tenant).await?;
        // `round` on numeric is half away from zero, which is what `mul_qty`
        // does in the core. The two have to agree: a delivery totalled one way
        // here and another way on a device is two answers to one question.
        let rows = sqlx::query(
            "with delivered as (
                 select r.supplier_id,
                        sum(round(l.unit_cost_minor::numeric * l.qty_milli / 1000))::bigint
                            as owed_minor,
                        count(distinct r.id)::bigint as deliveries,
                        min(r.received_at_ms)::bigint as since_ms
                   from goods_receipt r
                   join goods_receipt_line l
                     on l.tenant_id = r.tenant_id and l.receipt_id = r.id
                  where r.tenant_id = $1 and r.supplier_id is not null
                  group by r.supplier_id
             ),
             paid as (
                 select supplier_id,
                        sum(amount_minor)::bigint as paid_minor,
                        min(paid_at_ms)::bigint   as since_ms
                   from supplier_payment
                  where tenant_id = $1
                  group by supplier_id
             )
             select coalesce(d.supplier_id, p.supplier_id)          as supplier_id,
                    coalesce(s.name, '')                            as name,
                    (coalesce(d.owed_minor, 0) - coalesce(p.paid_minor, 0))::bigint
                                                                    as owed_minor,
                    coalesce(d.deliveries, 0)                       as deliveries,
                    least(coalesce(d.since_ms, p.since_ms), coalesce(p.since_ms, d.since_ms))
                                                                    as since_ms
               from delivered d
               full outer join paid p on p.supplier_id = d.supplier_id
               left join supplier s
                 on s.tenant_id = $1 and s.id = coalesce(d.supplier_id, p.supplier_id)
              where coalesce(d.owed_minor, 0) - coalesce(p.paid_minor, 0) <> 0
              order by owed_minor desc, supplier_id asc",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let supplier: Uuid = row.try_get("supplier_id").map_err(|_| RepoError::Backend)?;
            let deliveries: i64 = row.try_get("deliveries").map_err(|_| RepoError::Backend)?;
            let since: i64 = row.try_get("since_ms").map_err(|_| RepoError::Backend)?;
            found.push(SupplierOwing {
                supplier_id: supplier.as_u128(),
                name: row.try_get("name").map_err(|_| RepoError::Backend)?,
                owed_minor: row.try_get("owed_minor").map_err(|_| RepoError::Backend)?,
                deliveries: u32::try_from(deliveries).unwrap_or_default(),
                since_ms: u64::try_from(since).unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn waived(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<WaivedRow>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select o.sale_id, o.reason, s.terminal_id, s.rung_at_ms, s.total_minor
               from sale_override o
               join sale s on s.tenant_id = o.tenant_id and s.id = o.sale_id
              where o.tenant_id = $1 and s.rung_at_ms between $2 and $3
                and s.resolution_kept is not false
              order by s.rung_at_ms desc, o.sale_id desc, o.seq asc
              limit $4",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let sale: Uuid = row.try_get("sale_id").map_err(|_| RepoError::Backend)?;
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let rung: i64 = row.try_get("rung_at_ms").map_err(|_| RepoError::Backend)?;
            found.push(WaivedRow {
                sale_id: sale.as_u128(),
                terminal: terminal.as_u128(),
                rung_at_ms: u64::try_from(rung).unwrap_or_default(),
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                reason: row.try_get("reason").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn made(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<MadeSummary> {
        let mut transaction = self.scoped(tenant).await?;
        // Turnover before tax comes from the tax rows, which are the server's
        // own reading of the lines rather than anything a payload asserted.
        // Grouped by whether the shop can say what the goods cost, because a
        // margin over half a period is worse than no margin.
        let rows = sqlx::query(
            "select costed,
                    sum(net_minor)::bigint  as net_minor,
                    sum(cost_minor)::bigint as cost_minor,
                    count(*)::bigint        as sales
               from (
                 -- One row per sale, so a sale with two tax rows is one sale
                 -- and its cost is counted once. Joining and summing counts a
                 -- basket of rice and soap twice over.
                 select s.id,
                        coalesce(s.cost_known, false) as costed,
                        coalesce(s.cost_minor, 0)     as cost_minor,
                        coalesce((select sum(v.net_minor) from sale_vat v
                                   where v.tenant_id = s.tenant_id
                                     and v.sale_id = s.id), 0) as net_minor
                   from sale s
                  where s.tenant_id = $1 and s.rung_at_ms between $2 and $3
                    and s.resolution_kept is not false
               ) per_sale
              group by costed",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut summary = MadeSummary::default();
        for row in rows {
            let costed: bool = row.try_get("costed").map_err(|_| RepoError::Backend)?;
            let net: i64 = row.try_get("net_minor").map_err(|_| RepoError::Backend)?;
            let sales: i64 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
            let sales = u64::try_from(sales).unwrap_or_default();
            if costed {
                summary.net_minor = net;
                summary.cost_minor = row.try_get("cost_minor").map_err(|_| RepoError::Backend)?;
                summary.sales = sales;
            } else {
                summary.net_without_cost_minor = net;
                summary.sales_without_cost = sales;
            }
        }
        summary.made_minor = summary.net_minor.saturating_sub(summary.cost_minor);
        Ok(summary)
    }

    async fn vat_summary(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<VatSummary> {
        let mut transaction = self.scoped(tenant).await?;
        // Joined to the sale for the clock: a return covers a period by when
        // the goods were sold, not by when the server heard about them.
        let rows = sqlx::query(
            "-- every sale: grouped by the kind of supply as well as the rate,
             --   because zero rated and exempt are both nothing and are
             --   declared in different places
             select v.vat_bp, v.supply,
                    coalesce(sum(v.net_minor), 0)::bigint as net_minor,
                    coalesce(sum(v.vat_minor), 0)::bigint as vat_minor,
                    count(*)::bigint                      as sales
               from sale_vat v
               join sale s on s.tenant_id = v.tenant_id and s.id = v.sale_id
              where v.tenant_id = $1 and s.rung_at_ms between $2 and $3
                and s.resolution_kept is not false
              group by v.vat_bp, v.supply
              order by v.vat_bp, v.supply",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let bp: i32 = row.try_get("vat_bp").map_err(|_| RepoError::Backend)?;
            let sales: i64 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
            let supply: i16 = row.try_get("supply").map_err(|_| RepoError::Backend)?;
            found.push(VatRow {
                vat_bp: u32::try_from(bp).unwrap_or_default(),
                supply: u8::try_from(supply).unwrap_or_default(),
                net_minor: row.try_get("net_minor").map_err(|_| RepoError::Backend)?,
                vat_minor: row.try_get("vat_minor").map_err(|_| RepoError::Backend)?,
                sales: u64::try_from(sales).unwrap_or_default(),
            });
        }

        // How much of that is still waiting on somebody. In the figure and
        // counted apart from it: a sale nobody has looked at may be a duplicate
        // that over-declares, and the person signing the return decides.
        let waiting = sqlx::query(
            "-- every sale: what is counted here is what nobody has looked at yet,
             --   and a struck-out sale is one somebody has
             select count(distinct s.id)::bigint            as waiting_sales,
                    coalesce(sum(v.vat_minor), 0)::bigint   as waiting_vat_minor
               from sale s
               join sale_vat v on v.tenant_id = s.tenant_id and v.sale_id = s.id
              where s.tenant_id = $1 and s.rung_at_ms between $2 and $3
                and s.quarantine is not null and s.resolved_at is null",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let waiting_sales: i64 = waiting
            .try_get("waiting_sales")
            .map_err(|_| RepoError::Backend)?;
        Ok(VatSummary {
            rows: found,
            waiting_sales: u64::try_from(waiting_sales).unwrap_or_default(),
            waiting_vat_minor: waiting
                .try_get("waiting_vat_minor")
                .map_err(|_| RepoError::Backend)?,
        })
    }

    async fn day_summary(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<DaySummary> {
        let mut transaction = self.scoped(tenant).await?;
        let (from, to) = (
            i64::try_from(from_ms).unwrap_or(i64::MAX),
            i64::try_from(to_ms).unwrap_or(i64::MAX),
        );

        // A drawer is not adjusted by a sale struck out afterwards. What a till
        // expected and what a person counted are a record of one evening, and a
        // duplicate cash sale that inflated the expectation is exactly what the
        // shortfall that evening was. Rewriting the expectation now would erase
        // the evidence and make an evening that did not reconcile look as
        // though it had. So the takings can be lower than the cash a drawer
        // expected in the same report, and the difference is the thing somebody
        // is meant to read.
        let drawers = sqlx::query(
            "select count(*)::bigint                                as drawers,
                    coalesce(sum(expected_cash_minor), 0)::bigint   as expected_cash_minor,
                    coalesce(sum(counted_cash_minor), 0)::bigint    as counted_cash_minor,
                    coalesce(sum(variance_minor), 0)::bigint        as variance_minor
               from closed_shift
              where tenant_id = $1 and closed_at_ms between $2 and $3",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(from)
        .bind(to)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        // Three numbers rather than one: money the shop was given and money it
        // gave up are not the same thing, and a day that nets to zero because
        // one balanced the other is a day somebody should look at.
        let book = sqlx::query(
            "select coalesce(sum(amount_minor)
                        filter (where kind = 1 and amount_minor > 0), 0)::bigint
                        as charged_minor,
                    coalesce(-sum(amount_minor)
                        filter (where kind = 1 and amount_minor < 0), 0)::bigint
                        as returned_minor,
                    coalesce(-sum(amount_minor) filter (where kind = 2), 0)::bigint
                        as paid_minor,
                    coalesce(-sum(amount_minor) filter (where kind = 3), 0)::bigint
                        as written_off_minor
               from account_entry
              where tenant_id = $1 and at_ms between $2 and $3
                -- A charge from a sale somebody struck out is not a debt: the
                -- goods never left, so nothing is owed for them.
                and not (account_entry.kind = 1 and exists (
                    select 1 from sale s
                     where s.tenant_id = account_entry.tenant_id
                       and s.id = account_entry.source_id
                       and s.resolution_kept is false
                ))",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(from)
        .bind(to)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let counted: i64 = drawers.try_get("drawers").map_err(|_| RepoError::Backend)?;
        Ok(DaySummary {
            drawers_counted: u32::try_from(counted).unwrap_or(u32::MAX),
            expected_cash_minor: drawers
                .try_get("expected_cash_minor")
                .map_err(|_| RepoError::Backend)?,
            counted_cash_minor: drawers
                .try_get("counted_cash_minor")
                .map_err(|_| RepoError::Backend)?,
            variance_minor: drawers
                .try_get("variance_minor")
                .map_err(|_| RepoError::Backend)?,
            returned_minor: book
                .try_get("returned_minor")
                .map_err(|_| RepoError::Backend)?,
            charged_minor: book
                .try_get("charged_minor")
                .map_err(|_| RepoError::Backend)?,
            paid_minor: book.try_get("paid_minor").map_err(|_| RepoError::Backend)?,
            written_off_minor: book
                .try_get("written_off_minor")
                .map_err(|_| RepoError::Backend)?,
        })
    }

    async fn item_now(&self, tenant: u128, item_id: u128) -> Result<Option<(ItemWire, u64)>> {
        let mut transaction = self.scoped(tenant).await?;
        // The newest change naming that item, which is where it stands. A
        // deletion counts: it is the item's current state and its sequence.
        let row = sqlx::query(
            "select seq, kind, payload, schema from catalogue_change
              where item_id = $1 order by seq desc limit 1",
        )
        .bind(Uuid::from_u128(item_id))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
        let kind: i16 = row.try_get("kind").map_err(|_| RepoError::Backend)?;
        if kind != 1 {
            // Withdrawn. There is nothing to show, and the sequence still
            // matters: an edit built before this must not resurrect it.
            return Ok(None);
        }
        let schema: i16 = row.try_get("schema").map_err(|_| RepoError::Backend)?;
        let payload: Option<Vec<u8>> = row.try_get("payload").map_err(|_| RepoError::Backend)?;
        let Some(bytes) = payload else {
            return Ok(None);
        };
        Ok(decode_catalogue_payload(schema, &bytes)
            .map(|item| (item, u64::try_from(seq).unwrap_or_default())))
    }

    async fn settings_seq(&self, tenant: u128) -> Result<u64> {
        let mut transaction = self.scoped(tenant).await?;
        let seq: i64 = sqlx::query_scalar("select settings_seq from tenant where id = $1")
            .bind(Uuid::from_u128(tenant))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?
            .unwrap_or_default();
        Ok(u64::try_from(seq).unwrap_or_default())
    }

    async fn put_customer(&self, tenant: u128, customer: &CustomerRecord) -> Result<()> {
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "insert into customer (tenant_id, id, name, phone, active, bin, limit_minor)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (tenant_id, id) do update set
                name = excluded.name,
                phone = excluded.phone,
                active = excluded.active,
                -- Kept when the caller sends none, because a screen that does
                -- not offer the field would otherwise wipe it on every save.
                bin = coalesce(excluded.bin, customer.bin),
                limit_minor = excluded.limit_minor,
                updated_at = now()",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(customer.id))
        .bind(&customer.name)
        .bind(customer.phone.as_deref())
        .bind(customer.active)
        .bind(customer.bin.as_deref())
        .bind(customer.limit_minor)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        bump_settings(&mut transaction, tenant).await?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn customers(&self, tenant: u128) -> Result<Vec<CustomerRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, name, phone, active, bin, limit_minor from customer
              where tenant_id = $1
              order by name asc, id asc",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            found.push(CustomerRecord {
                id: id.as_u128(),
                name: row.try_get("name").map_err(|_| RepoError::Backend)?,
                phone: row.try_get("phone").map_err(|_| RepoError::Backend)?,
                active: row.try_get("active").map_err(|_| RepoError::Backend)?,
                bin: row.try_get("bin").map_err(|_| RepoError::Backend)?,
                limit_minor: row.try_get("limit_minor").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn put_open_drawer(&self, tenant: u128, drawer: &OpenDrawer) -> Result<()> {
        let mut transaction = self.scoped(tenant).await?;
        sqlx::query(
            "insert into open_drawer
                (tenant_id, terminal_id, shift_id, opened_at_ms, reported_at_ms,
                 opening_float_minor, sales, cash_sales_minor, non_cash_sales_minor,
                 cash_in_minor, cash_out_minor, expected_cash_minor)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             on conflict (tenant_id, terminal_id) do update set
                shift_id = excluded.shift_id,
                opened_at_ms = excluded.opened_at_ms,
                reported_at_ms = excluded.reported_at_ms,
                opening_float_minor = excluded.opening_float_minor,
                sales = excluded.sales,
                cash_sales_minor = excluded.cash_sales_minor,
                non_cash_sales_minor = excluded.non_cash_sales_minor,
                cash_in_minor = excluded.cash_in_minor,
                cash_out_minor = excluded.cash_out_minor,
                expected_cash_minor = excluded.expected_cash_minor",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(drawer.terminal))
        .bind(Uuid::from_u128(drawer.shift))
        .bind(i64::try_from(drawer.opened_at_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(drawer.reported_at_ms).unwrap_or(i64::MAX))
        .bind(drawer.opening_float_minor)
        .bind(i32::try_from(drawer.sales).unwrap_or(i32::MAX))
        .bind(drawer.cash_sales_minor)
        .bind(drawer.non_cash_sales_minor)
        .bind(drawer.cash_in_minor)
        .bind(drawer.cash_out_minor)
        .bind(drawer.expected_cash_minor)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn open_drawers(&self, tenant: u128) -> Result<Vec<OpenDrawer>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select terminal_id, shift_id, opened_at_ms, reported_at_ms, opening_float_minor,
                    sales, cash_sales_minor, non_cash_sales_minor, cash_in_minor,
                    cash_out_minor, expected_cash_minor
               from open_drawer
              where tenant_id = $1
              order by opened_at_ms asc",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let shift: Uuid = row.try_get("shift_id").map_err(|_| RepoError::Backend)?;
            let opened: i64 = row
                .try_get("opened_at_ms")
                .map_err(|_| RepoError::Backend)?;
            let reported: i64 = row
                .try_get("reported_at_ms")
                .map_err(|_| RepoError::Backend)?;
            let sales: i32 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
            found.push(OpenDrawer {
                terminal: terminal.as_u128(),
                shift: shift.as_u128(),
                opened_at_ms: u64::try_from(opened).unwrap_or_default(),
                reported_at_ms: u64::try_from(reported).unwrap_or_default(),
                opening_float_minor: row
                    .try_get("opening_float_minor")
                    .map_err(|_| RepoError::Backend)?,
                sales: u32::try_from(sales).unwrap_or_default(),
                cash_sales_minor: row
                    .try_get("cash_sales_minor")
                    .map_err(|_| RepoError::Backend)?,
                non_cash_sales_minor: row
                    .try_get("non_cash_sales_minor")
                    .map_err(|_| RepoError::Backend)?,
                cash_in_minor: row
                    .try_get("cash_in_minor")
                    .map_err(|_| RepoError::Backend)?,
                cash_out_minor: row
                    .try_get("cash_out_minor")
                    .map_err(|_| RepoError::Backend)?,
                expected_cash_minor: row
                    .try_get("expected_cash_minor")
                    .map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn put_allowed(
        &self,
        tenant: u128,
        terminal: u128,
        allowed: &[AllowedAction],
    ) -> Result<Vec<u64>> {
        let mut transaction = self.scoped(tenant).await?;
        let mut held = Vec::with_capacity(allowed.len());
        for one in allowed {
            // `do nothing` rather than an update: a resend after a dropped
            // reply must not rewrite what the shop already holds about who
            // allowed what. The count in the reply is what the shop holds,
            // which is what the till may drop.
            sqlx::query(
                "insert into allowed_action
                    (tenant_id, terminal_id, seq, at_ms, action, bp,
                     operator_id, operator_name, authorised_by, authorised_by_name,
                     receipt_no)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 on conflict (tenant_id, terminal_id, seq, at_ms) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(terminal))
            .bind(i64::try_from(one.seq).unwrap_or(i64::MAX))
            .bind(i64::try_from(one.at_ms).unwrap_or(i64::MAX))
            .bind(i16::from(one.action))
            .bind(i32::try_from(one.bp).unwrap_or(i32::MAX))
            .bind(Uuid::from_u128(one.operator))
            .bind(&one.operator_name)
            .bind(Uuid::from_u128(one.authorised_by))
            .bind(&one.authorised_by_name)
            .bind(one.receipt_no.as_deref())
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
            held.push(one.seq);
        }
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(held)
    }

    async fn tenant_created_at(&self, tenant: u128) -> Result<Option<u64>> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "select (extract(epoch from created_at) * 1000)::bigint as created_ms
               from tenant where id = $1",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        match row {
            Some(row) => Ok(Some(millis(&row, "created_ms")?.unwrap_or_default())),
            None => Ok(None),
        }
    }

    async fn refunded_against(&self, tenant: u128, receipt_no: &str) -> Result<Option<(i64, i64)>> {
        let mut transaction = self.scoped(tenant).await?;

        // The sale that carries the number, which is the one that is not itself
        // a refund of it: a refund has a receipt number of its own and names
        // this one as what it reverses. A struck-out sale is one somebody said
        // never happened, and a refund against it is a refund against nothing.
        let sold: Option<i64> = sqlx::query_scalar(
            "-- every sale: the question is about one receipt, and the filter is
             --   that receipt rather than a period
             select total_minor from sale
              where receipt_no = $1 and refund_of is null
                and resolution_kept is not false
              order by rung_at_ms asc limit 1",
        )
        .bind(receipt_no)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(sold) = sold else {
            return Ok(None);
        };

        let refunded: Option<i64> = sqlx::query_scalar(
            "-- every sale: as above, and a refund somebody struck out is one
             --   that did not happen
             select coalesce(sum(total_minor), 0)::bigint from sale
              where refund_of = $1 and resolution_kept is not false",
        )
        .bind(receipt_no)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        Ok(Some((sold, refunded.unwrap_or_default())))
    }

    async fn goods_against(&self, tenant: u128, receipt_no: &str) -> Result<Vec<(u128, i64)>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "-- every sale: the question is about one receipt and the sales that
             --   reverse it, and the filter is that receipt rather than a period
             select m.item_id, coalesce(sum(m.qty_milli), 0)::bigint as net
               from stock_movement m
               join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
              where m.source_kind = 1
                and ((s.receipt_no = $1 and s.refund_of is null) or s.refund_of = $1)
                and s.resolution_kept is not false
              group by m.item_id",
        )
        .bind(receipt_no)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut net = Vec::with_capacity(rows.len());
        for row in rows {
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let moved: i64 = row.try_get("net").map_err(|_| RepoError::Backend)?;
            net.push((item.as_u128(), moved));
        }
        Ok(net)
    }

    async fn drawer_takings(
        &self,
        tenant: u128,
        terminal: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Option<i64>> {
        let mut transaction = self.scoped(tenant).await?;
        // One sale in the window with no figure against it makes the whole
        // answer a guess, so the shop says it cannot answer rather than
        // answering low. Those are the sales stored before it worked this out.
        let taken: Option<Option<i64>> = sqlx::query_scalar(
            "-- every sale: this is one till between two moments, which is the
             --   drawer's own window rather than a period somebody chose
             select case when bool_or(cash_minor is null) then null
                         else coalesce(sum(cash_minor), 0) end::bigint from sale
              where terminal_id = $1 and rung_at_ms between $2 and $3
                and resolution_kept is not false",
        )
        .bind(Uuid::from_u128(terminal))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        // A drawer with no sales in it at all is answered, not declined: the
        // outer None is a row that did not come back, which cannot happen for
        // an aggregate, and the inner one is the question being unanswerable.
        Ok(taken.unwrap_or(Some(0)))
    }

    async fn barcode_holders(
        &self,
        tenant: u128,
        barcodes: &[String],
    ) -> Result<Vec<(String, u128)>> {
        if barcodes.is_empty() {
            return Ok(Vec::new());
        }
        let mut transaction = self.scoped(tenant).await?;
        // Where each item stands, which is its newest change. The catalogue is
        // a log and the barcodes are inside the payloads, so this decodes them
        // rather than asking the database: a shop has hundreds of items, an
        // item is saved rarely, and the alternative is a second copy of the
        // catalogue to keep in step.
        let rows = sqlx::query(
            "select distinct on (item_id) item_id, kind, payload, schema
               from catalogue_change
              where tenant_id = $1
              order by item_id, seq desc",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::new();
        for row in rows {
            let kind: i16 = row.try_get("kind").map_err(|_| RepoError::Backend)?;
            if kind != 1 {
                // Withdrawn, so it holds nothing: a shop that stops selling
                // something has its barcode back.
                continue;
            }
            let payload: Option<Vec<u8>> =
                row.try_get("payload").map_err(|_| RepoError::Backend)?;
            let schema: i16 = row.try_get("schema").map_err(|_| RepoError::Backend)?;
            let Some(item) = payload
                .as_deref()
                .and_then(|bytes| decode_catalogue_payload(schema, bytes))
            else {
                // A row this build cannot read is passed over here as it is
                // everywhere else. It cannot be checked against, and failing
                // the save over it would stop a shop editing its prices.
                continue;
            };
            if !item.active {
                continue;
            }
            for code in &item.barcodes {
                if barcodes.iter().any(|wanted| wanted == code) {
                    found.push((code.clone(), item.id));
                }
            }
        }
        Ok(found)
    }

    async fn receipt_gaps(&self, tenant: u128, limit: u32) -> Result<Vec<ReceiptGap>> {
        let mut transaction = self.scoped(tenant).await?;
        // Grouped by the series a number belongs to: the terminal, the epoch,
        // and the prefix the till prints. Two tills counting from one hundred
        // are not a hole in each other's numbering.
        //
        // The number is the digits after the last dash, which is the shape the
        // lease prints and the only shape a receipt number has ever had here.
        let rows = sqlx::query(
            "-- every sale: a number is used the moment it is printed. Leaving a
             --   struck-out sale out would open a gap where the shop has a
             --   receipt in its book, which is the opposite of what this asks
             with numbered as (
                 select terminal_id,
                        receipt_epoch,
                        left(receipt_no, length(receipt_no) - position('-' in reverse(receipt_no)))
                            as prefix,
                        (substring(receipt_no from '[0-9]+$'))::bigint as seq
                   from sale
                  where tenant_id = $1
                    and receipt_no is not null
                    and receipt_epoch is not null
                    and receipt_no ~ '-[0-9]+$'
             ),
             stepped as (
                 select terminal_id, receipt_epoch, prefix, seq,
                        lag(seq) over (
                            partition by terminal_id, receipt_epoch, prefix order by seq
                        ) as previous
                   from numbered
             )
             select terminal_id, receipt_epoch, prefix, previous, seq
               from stepped
              where previous is not null and seq > previous + 1
              order by previous, terminal_id
              limit $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let epoch: i64 = row
                .try_get("receipt_epoch")
                .map_err(|_| RepoError::Backend)?;
            let prefix: String = row.try_get("prefix").map_err(|_| RepoError::Backend)?;
            let previous: i64 = row.try_get("previous").map_err(|_| RepoError::Backend)?;
            let next: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let (before, after) = (
                u64::try_from(previous).unwrap_or_default(),
                u64::try_from(next).unwrap_or_default(),
            );
            found.push(ReceiptGap {
                terminal: terminal.as_u128(),
                epoch: u64::try_from(epoch).unwrap_or_default(),
                after: crate::repo::format_receipt(&prefix, before),
                before: crate::repo::format_receipt(&prefix, after),
                missing: after.saturating_sub(before).saturating_sub(1),
            });
        }
        Ok(found)
    }

    async fn allowed(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<AllowedAction>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select terminal_id, seq, at_ms, action, bp, receipt_no,
                    operator_id, operator_name, authorised_by, authorised_by_name
               from allowed_action
              where tenant_id = $1 and at_ms between $2 and $3
              order by at_ms desc, terminal_id desc, seq desc
              limit $4",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::try_from(from_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_ms).unwrap_or(i64::MAX))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let operator: Uuid = row.try_get("operator_id").map_err(|_| RepoError::Backend)?;
            let authorised_by: Uuid = row
                .try_get("authorised_by")
                .map_err(|_| RepoError::Backend)?;
            let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row.try_get("at_ms").map_err(|_| RepoError::Backend)?;
            let action: i16 = row.try_get("action").map_err(|_| RepoError::Backend)?;
            let bp: i32 = row.try_get("bp").map_err(|_| RepoError::Backend)?;
            found.push(AllowedAction {
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                terminal: terminal.as_u128(),
                seq: u64::try_from(seq).unwrap_or_default(),
                at_ms: u64::try_from(at_ms).unwrap_or_default(),
                action: u8::try_from(action).unwrap_or_default(),
                bp: u32::try_from(bp).unwrap_or_default(),
                operator: operator.as_u128(),
                operator_name: row
                    .try_get("operator_name")
                    .map_err(|_| RepoError::Backend)?,
                authorised_by: authorised_by.as_u128(),
                authorised_by_name: row
                    .try_get("authorised_by_name")
                    .map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn closed_shifts(&self, tenant: u128, limit: u32) -> Result<Vec<ClosedShift>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, terminal_id, opened_at_ms, closed_at_ms, opening_float_minor,
                    sales, cash_sales_minor, non_cash_sales_minor, cash_in_minor,
                    cash_out_minor, expected_cash_minor, counted_cash_minor,
                    variance_minor, closed_by, closed_by_name
               from closed_shift
              where tenant_id = $1
              order by closed_at_ms desc, id desc
              limit $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let found = rows
            .into_iter()
            .map(shift_from_row)
            .collect::<Result<Vec<_>>>()?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(found)
    }

    async fn take_payment(&self, tenant: u128, payment: &AccountPayment) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;

        // The insert is the idempotency check, as with a sale: a payment
        // counted twice is money the shop believes it has been given.
        let taken = sqlx::query(
            "insert into account_entry
                (tenant_id, person_key, person_name, source_id, kind, amount_minor, at_ms, note)
             values ($1, $2, $3, $4, $8, $5, $6, $7)
             on conflict do nothing
             returning source_id",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(&payment.person_key)
        .bind(&payment.person_name)
        .bind(Uuid::from_u128(payment.id))
        // Money handed over comes off what is owed.
        .bind(payment.amount_minor.saturating_neg())
        .bind(i64::try_from(payment.at_ms).unwrap_or(i64::MAX))
        .bind(payment.note.as_deref().unwrap_or_default())
        .bind(match payment.kind {
            Settlement::Paid => 2_i16,
            Settlement::WrittenOff => 3_i16,
        })
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(taken.is_some())
    }

    async fn customer_balances(&self, tenant: u128) -> Result<Vec<(u128, i64)>> {
        let mut transaction = self.scoped(tenant).await?;
        // Only the keys that name somebody the shop wrote down: those start
        // with a hash and carry an id. A debt against a name typed at a till
        // belongs to no record and cannot be shown against one.
        let rows = sqlx::query(
            "select person_key, sum(amount_minor)::bigint as owed_minor
               from account_entry
              where tenant_id = $1 and person_key like '#%'
                and not (account_entry.kind = 1 and exists (
                    select 1 from sale s
                     where s.tenant_id = account_entry.tenant_id
                       and s.id = account_entry.source_id
                       and s.resolution_kept is false
                ))
              group by person_key
             having sum(amount_minor) <> 0",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let key: String = row.try_get("person_key").map_err(|_| RepoError::Backend)?;
            let owed: i64 = row.try_get("owed_minor").map_err(|_| RepoError::Backend)?;
            if let Some(id) = crate::repo::customer_from_key(&key) {
                found.push((id, owed));
            }
        }
        Ok(found)
    }

    async fn balance(&self, tenant: u128, person_key: &str) -> Result<i64> {
        let mut transaction = self.scoped(tenant).await?;
        let total: Option<i64> = sqlx::query_scalar(
            "select sum(amount_minor)::bigint from account_entry
              where tenant_id = $1 and person_key = $2
                and not (account_entry.kind = 1 and exists (
                    select 1 from sale s
                     where s.tenant_id = account_entry.tenant_id
                       and s.id = account_entry.source_id
                       and s.resolution_kept is false
                ))",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(person_key)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(total.unwrap_or_default())
    }

    async fn owed(
        &self,
        tenant: u128,
        after: Option<(i64, String)>,
        limit: u32,
    ) -> Result<Vec<Owing>> {
        let mut transaction = self.scoped(tenant).await?;

        // Summed in the database rather than pulled and added here: a shop that
        // sells on account all year has thousands of entries and one line per
        // person is the answer. The name shown is the most recent spelling,
        // because a name is corrected by being written again.
        let rows = sqlx::query(
            "select person_key,
                    -- Latest by the till's clock, sale before payment when two
                    -- land in the same millisecond, and the id last so the
                    -- answer is the same every time it is asked.
                    (array_agg(person_name order by at_ms desc, kind asc, source_id desc)
                        filter (where person_name <> ''))[1] as person_name,
                    sum(amount_minor)::bigint as owed_minor,
                    min(at_ms)::bigint as since_ms,
                    max(at_ms)::bigint as last_at_ms,
                    count(*)::bigint as entries
               from account_entry
              where tenant_id = $1
                and not (account_entry.kind = 1 and exists (
                    select 1 from sale s
                     where s.tenant_id = account_entry.tenant_id
                       and s.id = account_entry.source_id
                       and s.resolution_kept is false
                ))
              group by person_key
             having sum(amount_minor) <> 0
                -- Where the last page ended. Written out rather than as a row
                -- comparison because the two columns run opposite ways: most
                -- owed first, and the key ascending inside a tie.
                and ($3 = 0 or sum(amount_minor) < $3
                     or (sum(amount_minor) = $3 and person_key > $4))
              order by sum(amount_minor) desc, person_key asc
              limit $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::from(limit.max(1)))
        .bind(after.as_ref().map_or(0, |(owed, _)| *owed))
        .bind(after.as_ref().map_or("", |(_, key)| key.as_str()))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let entries: i64 = row.try_get("entries").map_err(|_| RepoError::Backend)?;
            let since_ms: i64 = row.try_get("since_ms").map_err(|_| RepoError::Backend)?;
            let last_at_ms: i64 = row.try_get("last_at_ms").map_err(|_| RepoError::Backend)?;
            found.push(Owing {
                person_key: row.try_get("person_key").map_err(|_| RepoError::Backend)?,
                person_name: row.try_get("person_name").map_err(|_| RepoError::Backend)?,
                owed_minor: row.try_get("owed_minor").map_err(|_| RepoError::Backend)?,
                since_ms: u64::try_from(since_ms).unwrap_or_default(),
                last_at_ms: u64::try_from(last_at_ms).unwrap_or_default(),
                entries: u32::try_from(entries).unwrap_or(u32::MAX),
            });
        }
        Ok(found)
    }

    async fn account(
        &self,
        tenant: u128,
        person_key: &str,
        after: Option<(u64, u128)>,
        limit: u32,
    ) -> Result<Vec<AccountEntry>> {
        let mut transaction = self.scoped(tenant).await?;

        let rows = sqlx::query(
            "select source_id, kind, amount_minor, at_ms, note
               from account_entry
              where tenant_id = $1 and person_key = $2
                and not (account_entry.kind = 1 and exists (
                    select 1 from sale s
                     where s.tenant_id = account_entry.tenant_id
                       and s.id = account_entry.source_id
                       and s.resolution_kept is false
                ))
                -- Where the last page ended. One row per source per person, so
                -- this pair is unique and a page can neither repeat nor skip a
                -- line. `kind` is gone from the ordering with it: it could only
                -- ever break a tie that cannot happen.
                and ($4 = 0 or at_ms < $4 or (at_ms = $4 and source_id < $5))
              order by at_ms desc, source_id desc
              limit $3",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(person_key)
        .bind(i64::from(limit.max(1)))
        .bind(after.map_or(0, |(at, _)| i64::try_from(at).unwrap_or(i64::MAX)))
        .bind(Uuid::from_u128(after.map_or(0, |(_, source)| source)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let source: Uuid = row.try_get("source_id").map_err(|_| RepoError::Backend)?;
            let kind: i16 = row.try_get("kind").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row.try_get("at_ms").map_err(|_| RepoError::Backend)?;
            found.push(AccountEntry {
                source_id: source.as_u128(),
                is_sale: kind == 1,
                written_off: kind == 3,
                amount_minor: row
                    .try_get("amount_minor")
                    .map_err(|_| RepoError::Backend)?,
                at_ms: u64::try_from(at_ms).unwrap_or_default(),
                note: row.try_get("note").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn allowed_after(
        &self,
        tenant: u128,
        after: (u128, u64),
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<AllowedAction>> {
        let mut transaction = self.scoped(tenant).await?;
        // Row comparison, so the pair is one keyset cursor rather than two
        // predicates that would drop the rest of a terminal's trail.
        let rows = sqlx::query(
            "select terminal_id, seq, at_ms, action, bp, receipt_no,
                    operator_id, operator_name, authorised_by, authorised_by_name
               from allowed_action
              where (terminal_id, seq) > ($1, $2)
                and received_at <= to_timestamp($4 / 1000.0)
              order by terminal_id, seq limit $3",
        )
        .bind(Uuid::from_u128(after.0))
        .bind(i64::try_from(after.1).unwrap_or(i64::MAX))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let operator: Uuid = row.try_get("operator_id").map_err(|_| RepoError::Backend)?;
            let authorised_by: Uuid = row
                .try_get("authorised_by")
                .map_err(|_| RepoError::Backend)?;
            let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row.try_get("at_ms").map_err(|_| RepoError::Backend)?;
            let action: i16 = row.try_get("action").map_err(|_| RepoError::Backend)?;
            let bp: i32 = row.try_get("bp").map_err(|_| RepoError::Backend)?;
            found.push(AllowedAction {
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                terminal: terminal.as_u128(),
                seq: u64::try_from(seq).unwrap_or_default(),
                at_ms: u64::try_from(at_ms).unwrap_or_default(),
                action: u8::try_from(action).unwrap_or_default(),
                bp: u32::try_from(bp).unwrap_or_default(),
                operator: operator.as_u128(),
                operator_name: row
                    .try_get("operator_name")
                    .map_err(|_| RepoError::Backend)?,
                authorised_by: authorised_by.as_u128(),
                authorised_by_name: row
                    .try_get("authorised_by_name")
                    .map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn counts_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockCount>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, item_id, counted_milli, counted_at_ms, counted_by, note
               from stock_count
              where id > $1 and recorded_at <= to_timestamp($3 / 1000.0)
              order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let by: Uuid = row.try_get("counted_by").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row
                .try_get("counted_at_ms")
                .map_err(|_| RepoError::Backend)?;
            found.push(StockCount {
                id: id.as_u128(),
                item_id: item.as_u128(),
                counted_milli: row
                    .try_get("counted_milli")
                    .map_err(|_| RepoError::Backend)?,
                counted_at_ms: u64::try_from(at_ms).unwrap_or_default(),
                counted_by: by.as_u128(),
                note: row.try_get("note").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn corrections_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockCorrection>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, item_id, qty_milli, reason, occurred_at_ms, recorded_by
               from stock_correction
              where id > $1 and recorded_at <= to_timestamp($3 / 1000.0)
              order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let by: Uuid = row.try_get("recorded_by").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row
                .try_get("occurred_at_ms")
                .map_err(|_| RepoError::Backend)?;
            found.push(StockCorrection {
                id: id.as_u128(),
                item_id: item.as_u128(),
                qty_milli: row.try_get("qty_milli").map_err(|_| RepoError::Backend)?,
                reason: row.try_get("reason").map_err(|_| RepoError::Backend)?,
                occurred_at_ms: u64::try_from(at_ms).unwrap_or_default(),
                recorded_by: by.as_u128(),
            });
        }
        Ok(found)
    }

    async fn deliveries_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<GoodsReceipt>> {
        let mut transaction = self.scoped(tenant).await?;
        // In id order rather than by arrival, like the sales: a page boundary
        // cannot shift under a concurrent write the way an ordering by
        // timestamp can, which would skip or repeat a delivery mid-export.
        let headers = sqlx::query(
            "select id, supplier_id, reference, received_at_ms, received_by, note
               from goods_receipt
              where id > $1 and recorded_at <= to_timestamp($3 / 1000.0)
              order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(headers.len());
        for row in headers {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let lines = sqlx::query(
                "select item_id, qty_milli, unit_cost_minor
                   from goods_receipt_line
                  where tenant_id = $1 and receipt_id = $2
                  order by item_id",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            let received_at_ms: i64 = row
                .try_get("received_at_ms")
                .map_err(|_| RepoError::Backend)?;
            let supplier: Option<Uuid> =
                row.try_get("supplier_id").map_err(|_| RepoError::Backend)?;
            let received_by: Uuid = row.try_get("received_by").map_err(|_| RepoError::Backend)?;
            found.push(GoodsReceipt {
                id: id.as_u128(),
                supplier_id: supplier.map(|one| one.as_u128()),
                reference: row.try_get("reference").map_err(|_| RepoError::Backend)?,
                received_at_ms: u64::try_from(received_at_ms).unwrap_or_default(),
                received_by: received_by.as_u128(),
                note: row.try_get("note").map_err(|_| RepoError::Backend)?,
                lines: lines
                    .into_iter()
                    .map(|line| {
                        let item: Uuid = line.get("item_id");
                        crate::repo::ReceiptLine {
                            item_id: item.as_u128(),
                            qty_milli: line.get("qty_milli"),
                            unit_cost_minor: line.get("unit_cost_minor"),
                        }
                    })
                    .collect(),
            });
        }
        Ok(found)
    }

    async fn supplier_payments_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<SupplierPayment>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, supplier_id, amount_minor, paid_at_ms, note
               from supplier_payment
              where id > $1 and recorded_at <= to_timestamp($3 / 1000.0)
              order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let supplier: Uuid = row.try_get("supplier_id").map_err(|_| RepoError::Backend)?;
            let paid_at_ms: i64 = row.try_get("paid_at_ms").map_err(|_| RepoError::Backend)?;
            found.push(SupplierPayment {
                id: id.as_u128(),
                supplier_id: supplier.as_u128(),
                amount_minor: row
                    .try_get("amount_minor")
                    .map_err(|_| RepoError::Backend)?,
                paid_at_ms: u64::try_from(paid_at_ms).unwrap_or_default(),
                note: row.try_get("note").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn deliveries(&self, tenant: u128, limit: u32) -> Result<Vec<GoodsReceipt>> {
        let mut transaction = self.scoped(tenant).await?;

        // Newest first, and by id when two arrived in the same millisecond, so
        // the order does not change between two readings of the same shop.
        let headers = sqlx::query(
            "select id, supplier_id, reference, received_at_ms, received_by, note
               from goods_receipt
              where tenant_id = $1
              order by received_at_ms desc, id desc
              limit $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(headers.len());
        for row in headers {
            let id: Uuid = row.get("id");
            let lines = sqlx::query(
                "select item_id, qty_milli, unit_cost_minor
                   from goods_receipt_line
                  where tenant_id = $1 and receipt_id = $2
                  order by item_id",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            let received_at_ms: i64 = row.get("received_at_ms");
            let supplier: Option<Uuid> = row.get("supplier_id");
            let received_by: Uuid = row.get("received_by");
            found.push(GoodsReceipt {
                id: id.as_u128(),
                supplier_id: supplier.map(|one| one.as_u128()),
                reference: row.get("reference"),
                received_at_ms: u64::try_from(received_at_ms).unwrap_or(0),
                received_by: received_by.as_u128(),
                note: row.get("note"),
                lines: lines
                    .into_iter()
                    .map(|line| {
                        let item: Uuid = line.get("item_id");
                        crate::repo::ReceiptLine {
                            item_id: item.as_u128(),
                            qty_milli: line.get("qty_milli"),
                            unit_cost_minor: line.get("unit_cost_minor"),
                        }
                    })
                    .collect(),
            });
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(found)
    }

    async fn receive_goods(&self, tenant: u128, receipt: &GoodsReceipt) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;

        // The header insert is the idempotency check, as it is for a sale: a
        // back office retrying after a dropped reply must not book the same
        // delivery twice, and stock booked twice is a shop ordering against
        // goods it does not have.
        let inserted = sqlx::query(
            "insert into goods_receipt
                (tenant_id, id, supplier_id, reference, received_at_ms, received_by, note)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(receipt.id))
        .bind(receipt.supplier_id.map(Uuid::from_u128))
        .bind(receipt.reference.as_deref())
        .bind(i64::try_from(receipt.received_at_ms).unwrap_or(i64::MAX))
        .bind(Uuid::from_u128(receipt.received_by))
        .bind(receipt.note.as_deref())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        if inserted.is_none() {
            transaction.commit().await.map_err(|_| RepoError::Backend)?;
            return Ok(false);
        }

        for line in &receipt.lines {
            sqlx::query(
                "insert into goods_receipt_line
                    (tenant_id, receipt_id, item_id, qty_milli, unit_cost_minor)
                 values ($1, $2, $3, $4, $5)
                 on conflict (tenant_id, receipt_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(receipt.id))
            .bind(Uuid::from_u128(line.item_id))
            .bind(line.qty_milli)
            .bind(line.unit_cost_minor)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            // Source kind 2: a goods receipt. Positive, and carrying the arrival
            // time so a stock count can place it without knowing what kind of
            // thing moved the stock.
            sqlx::query(
                "insert into stock_movement
                    (tenant_id, source_id, source_kind, item_id, qty_milli, occurred_at_ms)
                 values ($1, $2, 2, $3, $4, $5)
                 on conflict (tenant_id, source_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(receipt.id))
            .bind(Uuid::from_u128(line.item_id))
            .bind(line.qty_milli)
            .bind(i64::try_from(receipt.received_at_ms).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(true)
    }

    async fn correct_stock(&self, tenant: u128, correction: &StockCorrection) -> Result<bool> {
        // A reason is required by the column and checked here too, so a caller
        // gets a refusal it can act on rather than a database error it cannot.
        if correction.reason.trim().is_empty() {
            return Err(RepoError::Invalid);
        }

        let mut transaction = self.scoped(tenant).await?;

        let inserted = sqlx::query(
            "insert into stock_correction
                (tenant_id, id, item_id, qty_milli, reason, occurred_at_ms, recorded_by)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(correction.id))
        .bind(Uuid::from_u128(correction.item_id))
        .bind(correction.qty_milli)
        .bind(&correction.reason)
        .bind(i64::try_from(correction.occurred_at_ms).unwrap_or(i64::MAX))
        .bind(Uuid::from_u128(correction.recorded_by))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        if inserted.is_none() {
            transaction.commit().await.map_err(|_| RepoError::Backend)?;
            return Ok(false);
        }

        // Source kind 3, the one reserved when the ledger stopped assuming every
        // movement was a sale.
        sqlx::query(
            "insert into stock_movement
                (tenant_id, source_id, source_kind, item_id, qty_milli, occurred_at_ms)
             values ($1, $2, 3, $3, $4, $5)
             on conflict (tenant_id, source_id, item_id) do nothing",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(correction.id))
        .bind(Uuid::from_u128(correction.item_id))
        .bind(correction.qty_milli)
        .bind(i64::try_from(correction.occurred_at_ms).unwrap_or(i64::MAX))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(true)
    }

    async fn register_terminal(&self, tenant: u128, terminal: u128, label: &str) -> Result<()> {
        self.enrol(tenant, terminal, label).await
    }

    async fn store_token_as(&self, caller: Caller, token: &TokenHash, role: Role) -> Result<()> {
        self.store_token(Caller { role, ..caller }, token).await
    }

    async fn revoke_token(&self, token: &TokenHash) -> Result<bool> {
        let result = sqlx::query(
            "update terminal_token set revoked_at = now()
             where token_hash = $1 and revoked_at is null",
        )
        .bind(token.as_bytes())
        .execute(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_all_tokens(&self, caller: Caller) -> Result<usize> {
        // Marked rather than deleted. A shop investigating a theft wants to see
        // that a credential existed and when it was withdrawn, not an absence.
        let result = sqlx::query(
            "update terminal_token set revoked_at = now()
             where tenant_id = $1 and terminal_id = $2 and revoked_at is null",
        )
        .bind(Uuid::from_u128(caller.tenant))
        .bind(Uuid::from_u128(caller.terminal))
        .execute(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
    }

    async fn issue_enrolment_code(
        &self,
        grants: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> Result<()> {
        let seconds = f64::from(u32::try_from(valid_for.as_secs()).unwrap_or(u32::MAX));
        sqlx::query(
            "insert into enrolment_code (code_hash, tenant_id, terminal_id, expires_at, role)
             values ($1, $2, $3, now() + make_interval(secs => $4), $5)
             on conflict (code_hash) do nothing",
        )
        .bind(code.as_bytes())
        .bind(Uuid::from_u128(grants.tenant))
        .bind(Uuid::from_u128(grants.terminal))
        .bind(seconds)
        // The role the redeeming device will carry. Whether the asker may grant
        // it is checked in the handler, not here: a repository that decided who
        // may grant what would put an authorisation rule somewhere no handler
        // thinks to look.
        .bind(grants.role.as_i16())
        .execute(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn redeem_enrolment_code(&self, code: &TokenHash) -> Result<Option<Caller>> {
        // Consuming and reading in one statement, so two devices racing to
        // redeem the same code cannot both succeed: the second update matches
        // nothing because consumed_at is no longer null.
        let row = sqlx::query(
            "update enrolment_code set consumed_at = now()
             where code_hash = $1 and consumed_at is null and expires_at > now()
             returning tenant_id, terminal_id, role",
        )
        .bind(code.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let tenant: Uuid = row.try_get("tenant_id").map_err(|_| RepoError::Backend)?;
        let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
        let role: i16 = row.try_get("role").map_err(|_| RepoError::Backend)?;
        Ok(Some(Caller {
            tenant: tenant.as_u128(),
            terminal: terminal.as_u128(),
            role: Role::from_i16(role),
        }))
    }

    async fn items_since(&self, tenant: u128, cursor: u64, limit: u32) -> Result<CataloguePage> {
        let mut transaction = self.scoped(tenant).await?;
        let after = i64::try_from(cursor).unwrap_or(i64::MAX);
        let take = i64::from(limit.max(1));

        let rows = sqlx::query(
            "select seq, kind, item_id, payload, schema from catalogue_change
             where seq > $1 order by seq limit $2",
        )
        .bind(after)
        .bind(take)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut page = CataloguePage {
            cursor,
            ..CataloguePage::default()
        };
        for row in rows {
            let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let kind: i16 = row.try_get("kind").map_err(|_| RepoError::Backend)?;
            let item_id: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;

            if kind == 1 {
                let schema: i16 = row.try_get("schema").map_err(|_| RepoError::Backend)?;
                let payload: Option<Vec<u8>> =
                    row.try_get("payload").map_err(|_| RepoError::Backend)?;
                let bytes = payload.ok_or(RepoError::Backend)?;

                // A row this build cannot read is skipped, not fatal. Failing
                // the page would make one bad row a permanent poison pill: every
                // pull for that shop returns a backend error, the HTTP layer
                // turns it into a 503, and every till stops syncing forever with
                // no way past it. The cursor still advances, so the shop loses
                // one catalogue change rather than all of them.
                match decode_catalogue_payload(schema, &bytes) {
                    Some(item) => page.upserts.push(item),
                    None => page.skipped = page.skipped.saturating_add(1),
                }
            } else {
                page.tombstones.push(item_id.as_u128());
            }
            page.cursor = u64::try_from(seq).unwrap_or(page.cursor);
        }

        // Whether anything is left after this page, so a till knows to ask again
        // rather than assuming it is current.
        let remaining =
            sqlx::query("select 1 as found from catalogue_change where seq > $1 limit 1")
                .bind(i64::try_from(page.cursor).unwrap_or(i64::MAX))
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;
        page.more = remaining.is_some();

        Ok(page)
    }

    async fn sales_on_receipt(&self, tenant: u128, receipt_no: &str) -> Result<Vec<SaleOnPaper>> {
        let mut transaction = self.scoped(tenant).await?;
        // What was given back against this number, by the same rule the refund
        // check uses: a refund names the receipt it reverses and carries a
        // negative total, and one somebody struck out gave nothing back.
        let refunded: Option<i64> = sqlx::query_scalar(
            "-- every sale: this is what came back against one receipt
             select coalesce(-sum(total_minor), 0)::bigint from sale
              where refund_of = $1 and resolution_kept is not false",
        )
        .bind(receipt_no)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        let refunded = refunded.unwrap_or_default();

        // Uses the receipt lookup index, which carries the epoch as well: two
        // sales under one number is exactly what this is asked about, so both
        // come back rather than whichever is first.
        let rows = sqlx::query(
            "-- every sale: a record of what was rung rather than a figure the
             --   shop declares. The person at the counter is holding the paper
             --   for a sale somebody may have struck out, and answering with
             --   nothing would be answering the wrong question
             select id, terminal_id, rung_at_ms, total_minor, payload, quarantine,
                    quarantine_kind,
                    resolution, resolution_kept, refund_of
               from sale
              where receipt_no = $1
              order by rung_at_ms, id",
        )
        .bind(receipt_no)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let decided: Option<String> =
                row.try_get("resolution").map_err(|_| RepoError::Backend)?;
            let kept: Option<bool> = row
                .try_get("resolution_kept")
                .map_err(|_| RepoError::Backend)?;
            let refund_of: Option<String> =
                row.try_get("refund_of").map_err(|_| RepoError::Backend)?;
            found.push(SaleOnPaper {
                id: id.as_u128(),
                terminal: terminal.as_u128(),
                receipt_no: receipt_no.to_owned(),
                rung_at_ms: u64::try_from(
                    row.try_get::<i64, _>("rung_at_ms")
                        .map_err(|_| RepoError::Backend)?,
                )
                .unwrap_or_default(),
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                payload: row.try_get("payload").map_err(|_| RepoError::Backend)?,
                held_for: row.try_get("quarantine").map_err(|_| RepoError::Backend)?,
                held_for_bytes: row
                    .try_get::<Option<Vec<u8>>, _>("quarantine_kind")
                    .map_err(|_| RepoError::Backend)?
                    .unwrap_or_default(),
                // A sale nobody has decided about is not "kept": it is
                // undecided, which is why the words and the flag travel
                // together rather than a bare boolean.
                decided: decided.map(|said| (said, kept.unwrap_or(true))),
                // Against the sale itself. A refund is the money given back; it
                // does not have money given back against it.
                refunded_minor: if refund_of.is_none() { refunded } else { 0 },
                refund_of,
            });
        }
        Ok(found)
    }

    async fn repair_queue(&self, tenant: u128, limit: u32) -> Result<Vec<RepairItem>> {
        let mut transaction = self.scoped(tenant).await?;
        // Matches the partial index added in 0004 exactly, including the
        // `resolved_at is null`. A predicate the index does not cover would make
        // this a sequential scan over every sale the shop has ever taken.
        let rows = sqlx::query(
            "-- every sale: this is the queue of what needs looking at, and what
             --   was decided is what takes a sale out of it
             select id, receipt_no, total_minor, quarantine, quarantine_kind,
                    (extract(epoch from received_at) * 1000)::bigint as received_ms
             from sale
             where quarantine is not null and resolved_at is null
             order by received_at, id limit $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            found.push(RepairItem {
                id: id.as_u128(),
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                received_at_ms: millis(&row, "received_ms")?.unwrap_or_default(),
                // The stored prose, not a re-description. This is what the
                // server decided at the moment it quarantined the sale, and a
                // later release that words a reason differently must not
                // silently rewrite what an operator already read.
                reason: row.try_get("quarantine").map_err(|_| RepoError::Backend)?,
                // Absent for a sale held before this column existed, which can
                // only ever be shown as the sentence beside it.
                reason_bytes: row
                    .try_get::<Option<Vec<u8>>, _>("quarantine_kind")
                    .map_err(|_| RepoError::Backend)?
                    .unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn resolve_quarantine(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
    ) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        // `resolved_at is null` in the predicate, so resolving twice reports
        // false rather than overwriting the first person's note with the
        // second's. Two people working one queue is the normal case.
        let result = sqlx::query(
            "update sale set resolved_at = now(), resolution = $2, resolution_kept = $3
             where id = $1 and quarantine is not null and resolved_at is null",
        )
        .bind(Uuid::from_u128(sale))
        .bind(note)
        .bind(kept)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        let moved = result.rows_affected() > 0;
        if moved {
            // The first row of this sale's history, in the same transaction as
            // the answer it records. A decision the shop can see and a decision
            // the figures read must not be able to disagree.
            sqlx::query(
                "insert into sale_resolution (tenant_id, sale_id, seq, note, kept)
                 values ($1, $2, 1, $3, $4)
                 on conflict do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(sale))
            .bind(note)
            .bind(kept)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(moved)
    }

    async fn decided(&self, tenant: u128, limit: u32) -> Result<Vec<DecidedSale>> {
        let mut transaction = self.scoped(tenant).await?;
        // The sale's own columns hold the latest answer, so the list is read
        // from those and the history is joined only to count how many times the
        // shop has answered. Newest first: somebody looking for the answer they
        // just gave finds it at the top.
        let rows = sqlx::query(
            "-- every sale: this lists what was decided, and half of it is the
             --   sales that were struck out
             select s.id, s.receipt_no, s.total_minor, s.quarantine, s.resolution,
                    s.resolution_kept,
                    (extract(epoch from s.resolved_at) * 1000)::bigint as decided_ms,
                    count(r.seq)::bigint                               as decisions
               from sale s
               left join sale_resolution r
                      on r.tenant_id = s.tenant_id and r.sale_id = s.id
              where s.tenant_id = $1 and s.resolved_at is not null
                and s.quarantine is not null
              group by s.id, s.receipt_no, s.total_minor, s.quarantine, s.resolution,
                       s.resolution_kept, s.resolved_at
              order by s.resolved_at desc, s.id desc
              limit $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let kept: Option<bool> = row
                .try_get("resolution_kept")
                .map_err(|_| RepoError::Backend)?;
            let note: Option<String> = row.try_get("resolution").map_err(|_| RepoError::Backend)?;
            let decisions: i64 = row.try_get("decisions").map_err(|_| RepoError::Backend)?;
            found.push(DecidedSale {
                id: id.as_u128(),
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                reason: row
                    .try_get::<Option<String>, _>("quarantine")
                    .map_err(|_| RepoError::Backend)?
                    .unwrap_or_default(),
                note: note.unwrap_or_default(),
                // A note written before a decision could say anything means the
                // sale stands: that was all resolving used to mean.
                kept: kept.unwrap_or(true),
                decided_at_ms: millis(&row, "decided_ms")?.unwrap_or_default(),
                // At least one: a shop that resolved before this table existed
                // has the row the migration wrote, and a count of zero would
                // read as "never decided" for something plainly decided.
                decisions: u32::try_from(decisions).unwrap_or(u32::MAX).max(1),
            });
        }
        Ok(found)
    }

    async fn decide_again(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
        expected: u32,
    ) -> Result<Decided> {
        let mut transaction = self.scoped(tenant).await?;
        // The row is locked before anything is read off it, so two owners
        // pressing at once are serialised here rather than racing over the
        // history's sequence. `quarantine is not null` because deciding is
        // answering the queue: a sale that never reached it has nothing to
        // answer, and an imported one carrying a note is not a queue entry.
        let current = sqlx::query(
            "-- every sale: changing an answer needs to read the answer that
             --   stands, struck out or not
             select s.resolution, s.resolution_kept,
                    (select count(*) from sale_resolution r
                      where r.tenant_id = s.tenant_id and r.sale_id = s.id) as answers,
                    (extract(epoch from s.resolved_at) * 1000)::bigint as decided_ms
               from sale s
              where s.id = $1 and s.resolved_at is not null and s.quarantine is not null
              for update of s",
        )
        .bind(Uuid::from_u128(sale))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(current) = current else {
            return Ok(Decided::Unanswered);
        };
        let answers: i64 = current.try_get("answers").map_err(|_| RepoError::Backend)?;
        // A sale answered before the history existed, or imported with one,
        // reads as one answer, which is what the list showed.
        let seen = u32::try_from(answers).unwrap_or(u32::MAX).max(1);
        if expected != 0 && expected != seen {
            // Somebody else answered while this screen was open. Changing it
            // would make a stale view the current one.
            return Ok(Decided::Stale);
        }

        // A sale answered before this table existed, or restored from a bundle,
        // has no history to append to. Its current answer becomes the first row
        // so the trail starts where the shop's own record does rather than
        // where this table happened to be created.
        if answers == 0 {
            let first: Option<String> = current
                .try_get("resolution")
                .map_err(|_| RepoError::Backend)?;
            let stood: Option<bool> = current
                .try_get("resolution_kept")
                .map_err(|_| RepoError::Backend)?;
            let at: Option<i64> = current
                .try_get("decided_ms")
                .map_err(|_| RepoError::Backend)?;
            sqlx::query(
                "insert into sale_resolution (tenant_id, sale_id, seq, decided_at, note, kept)
                 values ($1, $2, 1, to_timestamp($5 / 1000.0), $3, $4)
                 on conflict do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(sale))
            .bind(first.unwrap_or_default())
            .bind(stood.unwrap_or(true))
            .bind(at.unwrap_or_default())
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        sqlx::query(
            "update sale set resolved_at = now(), resolution = $2, resolution_kept = $3
             where id = $1",
        )
        .bind(Uuid::from_u128(sale))
        .bind(note)
        .bind(kept)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        // Appended, never overwritten. What a shop decided in March is part of
        // why it decided differently in April, and a record that only holds the
        // latest answer cannot explain a figure that changed.
        sqlx::query(
            "-- every sale: a write, about a sale somebody is deciding again
             insert into sale_resolution (tenant_id, sale_id, seq, note, kept)
             select $1, $2, coalesce(max(seq), 0) + 1, $3, $4
               from sale_resolution where tenant_id = $1 and sale_id = $2",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(sale))
        .bind(note)
        .bind(kept)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(Decided::Changed)
    }

    async fn terminal_health(&self, tenant: u128) -> Result<Vec<TerminalHealth>> {
        let mut transaction = self.scoped(tenant).await?;
        // One statement rather than a query per terminal. A shop has a handful
        // of tills, so the join is small, and the page an owner reloads while
        // waiting on the phone should not cost one round trip per device.
        //
        // A left join, so a terminal enrolled this morning and not yet used
        // appears with a count of zero instead of vanishing, which is precisely
        // the device someone is ringing up about.
        let rows = sqlx::query(
            "-- every sale: this is a support view of what a device has sent, and
             --   a sale it sent is a sale it sent whatever was decided later
             select t.id, t.label, t.epoch,
                    (extract(epoch from t.enrolled_at) * 1000)::bigint  as enrolled_ms,
                    (extract(epoch from t.last_seen_at) * 1000)::bigint as last_seen_ms,
                    count(s.id) as sales,
                    count(s.id) filter (
                        where s.quarantine is not null and s.resolved_at is null
                    ) as open_repairs,
                    -- The highest role this device still holds a live
                    -- credential for. A subquery rather than a second join:
                    -- joining the credentials would multiply the sale count by
                    -- however many a device has renewed.
                    coalesce((select max(k.role) from terminal_token k
                               where k.tenant_id = t.tenant_id
                                 and k.terminal_id = t.id
                                 and k.revoked_at is null), 0) as role
             from terminal t
             left join sale s on s.tenant_id = t.tenant_id and s.terminal_id = t.id
             group by t.id, t.tenant_id, t.label, t.epoch, t.enrolled_at, t.last_seen_at
             order by t.enrolled_at, t.id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let epoch: i64 = row.try_get("epoch").map_err(|_| RepoError::Backend)?;
            let sales: i64 = row.try_get("sales").map_err(|_| RepoError::Backend)?;
            let open_repairs: i64 = row
                .try_get("open_repairs")
                .map_err(|_| RepoError::Backend)?;

            found.push(TerminalHealth {
                terminal: terminal.as_u128(),
                label: row.try_get("label").map_err(|_| RepoError::Backend)?,
                epoch: u64::try_from(epoch).unwrap_or(1),
                enrolled_at_ms: millis(&row, "enrolled_ms")?.unwrap_or_default(),
                last_seen_ms: millis(&row, "last_seen_ms")?,
                sales: u64::try_from(sales).unwrap_or_default(),
                open_repairs: u64::try_from(open_repairs).unwrap_or_default(),
                role: u8::try_from(
                    row.try_get::<i32, _>("role")
                        .map_err(|_| RepoError::Backend)?,
                )
                .unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn mark_terminal_seen(&self, tenant: u128, terminal: u128) -> Result<()> {
        let mut transaction = self.scoped(tenant).await?;
        // A blind update. A terminal that authenticated and then vanished from
        // the table is not worth a second query to distinguish, and the caller
        // has already established that the credential resolves to this pair.
        sqlx::query("update terminal set last_seen_at = now() where id = $1")
            .bind(Uuid::from_u128(terminal))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn upsert_item(&self, tenant: u128, item: &ItemWire) -> Result<u64> {
        let payload = postcard::to_allocvec(item).map_err(|_| RepoError::Backend)?;
        self.append_change(tenant, 1, item.id, Some(payload)).await
    }

    async fn delete_item(&self, tenant: u128, item_id: u128) -> Result<u64> {
        self.append_change(tenant, 2, item_id, None).await
    }

    async fn resend_catalogue(&self, tenant: u128) -> Result<u64> {
        let mut transaction = self.scoped(tenant).await?;

        // How many rows this will be, so the counter moves by exactly that and
        // the sequences handed out are the ones the insert uses. One item, one
        // row: its latest state, whether that is a price or a tombstone.
        let count: i64 = sqlx::query_scalar(
            "select count(distinct item_id)::bigint from catalogue_change where tenant_id = $1",
        )
        .bind(Uuid::from_u128(tenant))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        if count == 0 {
            return Ok(0);
        }

        // Bumped by the whole batch in one statement, for the reason a single
        // change bumps it by one: two shops' worth of edits landing at once
        // must not be handed the same numbers.
        let row = sqlx::query(
            "update tenant set catalogue_seq = catalogue_seq + $2
             where id = $1 returning catalogue_seq",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(count)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        let last: i64 = row
            .try_get("catalogue_seq")
            .map_err(|_| RepoError::Backend)?;
        let first = last.saturating_sub(count).saturating_add(1);

        // The payload is copied rather than decoded and written again: a row
        // that could not be read by the build that stored it is exactly the row
        // this exists for, and re-encoding one would either fail or change what
        // it says. The schema travels with it for the same reason.
        let sent = sqlx::query(
            "with latest as (
                 select distinct on (item_id) item_id, kind, payload, schema
                   from catalogue_change
                  where tenant_id = $1
                  order by item_id, seq desc
             ),
             numbered as (
                 select item_id, kind, payload, schema,
                        row_number() over (order by item_id) as offset_in_batch
                   from latest
             )
             insert into catalogue_change (tenant_id, seq, kind, item_id, payload, schema)
             select $1, $2 + offset_in_batch - 1, kind, item_id, payload, schema from numbered",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(first)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(sent.rows_affected())
    }

    async fn item_has_history(&self, tenant: u128, item_id: u128) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        // A movement covers a sale, a delivery and a write-off in the ordinary
        // path, because all three write one. The other three tables are asked
        // for as well rather than trusted to imply a movement: a shop restored
        // from a backup, or one whose movement rows were rebuilt, can hold the
        // delivery, the correction or the count without the movement beside it,
        // and the whole point of this question is the record rather than the
        // arithmetic.
        let row = sqlx::query(
            "-- every sale: a deletion is a tombstone and this decides whether
             --   one is allowed at all, so a struck-out sale counts the same as
             --   any other. It happened, and somebody answered for it
             select exists(
                      select 1 from stock_movement
                       where tenant_id = $1 and item_id = $2
                    ) or exists(
                      select 1 from stock_count
                       where tenant_id = $1 and item_id = $2
                    ) or exists(
                      select 1 from goods_receipt_line
                       where tenant_id = $1 and item_id = $2
                    ) or exists(
                      select 1 from stock_correction
                       where tenant_id = $1 and item_id = $2
                    ) as traded",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(item_id))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        row.try_get("traded").map_err(|_| RepoError::Backend)
    }

    async fn tenant_record(&self, tenant: u128) -> Result<Option<TenantRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query("select name, catalogue_seq from tenant where id = $1")
            .bind(Uuid::from_u128(tenant))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let name: String = row.try_get("name").map_err(|_| RepoError::Backend)?;
        let seq: i64 = row
            .try_get("catalogue_seq")
            .map_err(|_| RepoError::Backend)?;
        Ok(Some(TenantRecord {
            id: tenant,
            name,
            catalogue_seq: u64::try_from(seq).unwrap_or_default(),
        }))
    }

    async fn terminal_records(&self, tenant: u128) -> Result<Vec<TerminalRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query("select id, label, epoch, next_receipt from terminal order by id")
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let label: String = row.try_get("label").map_err(|_| RepoError::Backend)?;
            let epoch: i64 = row.try_get("epoch").map_err(|_| RepoError::Backend)?;
            let next: i64 = row
                .try_get("next_receipt")
                .map_err(|_| RepoError::Backend)?;
            found.push(TerminalRecord {
                id: id.as_u128(),
                label,
                epoch: u64::try_from(epoch).unwrap_or(1),
                next_receipt: u64::try_from(next).unwrap_or(1),
            });
        }
        Ok(found)
    }

    async fn catalogue_after(
        &self,
        tenant: u128,
        after_seq: u64,
        limit: u32,
    ) -> Result<Vec<CatalogueRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select seq, kind, item_id, payload, schema from catalogue_change
             where seq > $1 order by seq limit $2",
        )
        .bind(i64::try_from(after_seq).unwrap_or(i64::MAX))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let seq: i64 = row.try_get("seq").map_err(|_| RepoError::Backend)?;
            let kind: i16 = row.try_get("kind").map_err(|_| RepoError::Backend)?;
            let item_id: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let payload: Option<Vec<u8>> =
                row.try_get("payload").map_err(|_| RepoError::Backend)?;
            let schema: i16 = row.try_get("schema").map_err(|_| RepoError::Backend)?;
            found.push(CatalogueRecord {
                seq: u64::try_from(seq).unwrap_or_default(),
                kind,
                item_id: item_id.as_u128(),
                payload,
                schema: u8::try_from(schema).unwrap_or(CATALOGUE_SCHEMA),
            });
        }
        Ok(found)
    }

    async fn now_ms(&self) -> Result<u64> {
        // The database's clock, not this process's: the two drift, and the
        // comparison this feeds happens in the database.
        let now: i64 = sqlx::query_scalar("select (extract(epoch from now()) * 1000)::bigint")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| RepoError::Backend)?;
        Ok(u64::try_from(now).unwrap_or_default())
    }

    async fn sales_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<SaleRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "-- every sale: a bundle carries what the shop holds, including what it
             --   struck out and the decision that struck it
             select id, terminal_id, receipt_no, receipt_epoch, rung_at_ms, total_minor,
                    payload, quarantine, quarantine_kind, resolution, resolution_kept
             from sale
              where id > $1 and received_at <= to_timestamp($3 / 1000.0)
              order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let epoch: Option<i64> = row
                .try_get("receipt_epoch")
                .map_err(|_| RepoError::Backend)?;
            let rung_at_ms: i64 = row.try_get("rung_at_ms").map_err(|_| RepoError::Backend)?;
            found.push(SaleRecord {
                overrides: Vec::new(),
                // Not carried out of the database: an export writes the sale
                // and its bytes, and the tax figures are recomputed on the way
                // back in from the same crate that computed them first.
                vat: Vec::new(),
                id: id.as_u128(),
                terminal: terminal.as_u128(),
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                receipt_epoch: epoch.map(|value| u64::try_from(value).unwrap_or_default()),
                rung_at_ms: u64::try_from(rung_at_ms).unwrap_or_default(),
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                payload: row.try_get("payload").map_err(|_| RepoError::Backend)?,
                quarantine: row.try_get("quarantine").map_err(|_| RepoError::Backend)?,
                // The reason itself as well as the sentence, so a shop put back
                // from a backup can still say why a sale is held in its own
                // language rather than dropping to the English it was stored in.
                quarantine_kind: row
                    .try_get::<Option<Vec<u8>>, _>("quarantine_kind")
                    .map_err(|_| RepoError::Backend)?
                    .unwrap_or_default(),
                // Carried, because nobody can work it out again from the bytes:
                // it is what a person decided about the sale.
                resolution: {
                    let note: Option<String> =
                        row.try_get("resolution").map_err(|_| RepoError::Backend)?;
                    let kept: Option<bool> = row
                        .try_get("resolution_kept")
                        .map_err(|_| RepoError::Backend)?;
                    // A note written before the decision existed means the sale
                    // stands: that was the only thing resolving could mean.
                    note.map(|note| (note, kept.unwrap_or(true)))
                },
                refund_of: None,
            });
        }
        Ok(found)
    }

    async fn stock_after(
        &self,
        tenant: u128,
        after: (u128, u128),
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        // Row comparison, so the pair is one keyset cursor rather than two
        // predicates that would drop the rest of a partially read sale.
        let rows = sqlx::query(
            // A movement belongs to whatever caused it. A sale's movements are
            // in the cut when the sale is: one without its sale is stock that
            // moved for no reason anybody can point at. A delivery or a
            // correction has no sale row and is taken as it stands, which is
            // what the left join says.
            "-- every sale: a bundle carries the movements as they were written,
             --   and the decision that struck one travels with its sale
             select m.source_id, m.source_kind, m.item_id, m.qty_milli, m.occurred_at_ms
               from stock_movement m
               left join sale s on s.tenant_id = m.tenant_id and s.id = m.source_id
              where (m.source_id, m.item_id) > ($1, $2)
                and (s.received_at is null or s.received_at <= to_timestamp($4 / 1000.0))
              order by m.source_id, m.item_id limit $3",
        )
        .bind(Uuid::from_u128(after.0))
        .bind(Uuid::from_u128(after.1))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let source: Uuid = row.try_get("source_id").map_err(|_| RepoError::Backend)?;
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let occurred: i64 = row
                .try_get("occurred_at_ms")
                .map_err(|_| RepoError::Backend)?;
            found.push(StockRecord {
                source: source.as_u128(),
                source_kind: row.try_get("source_kind").map_err(|_| RepoError::Backend)?,
                item: item.as_u128(),
                qty_milli: row.try_get("qty_milli").map_err(|_| RepoError::Backend)?,
                occurred_at_ms: u64::try_from(occurred).unwrap_or_default(),
            });
        }
        Ok(found)
    }

    async fn put_tenant(&self, record: &TenantRecord) -> Result<()> {
        let mut transaction = self.scoped(record.id).await?;
        // The counter is raised and never lowered. Restoring an older bundle
        // over a live shop must not rewind the sequence, because a till holding
        // a higher cursor would then never see the changes that follow.
        sqlx::query(
            "insert into tenant (id, name, catalogue_seq) values ($1, $2, $3)
             on conflict (id) do update set
                 name = excluded.name,
                 catalogue_seq = greatest(tenant.catalogue_seq, excluded.catalogue_seq)",
        )
        .bind(Uuid::from_u128(record.id))
        .bind(&record.name)
        .bind(i64::try_from(record.catalogue_seq).unwrap_or(i64::MAX))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
    }

    async fn put_terminals(&self, tenant: u128, records: &[TerminalRecord]) -> Result<usize> {
        let mut transaction = self.scoped(tenant).await?;
        for record in records {
            // Epoch and counter are raised, never lowered, for the same reason a
            // restore bumps an epoch: numbers already printed must not be handed
            // out a second time under the same epoch.
            sqlx::query(
                "insert into terminal (tenant_id, id, label, epoch, next_receipt)
                 values ($1, $2, $3, $4, $5)
                 on conflict (tenant_id, id) do update set
                     label = excluded.label,
                     epoch = greatest(terminal.epoch, excluded.epoch),
                     next_receipt = greatest(terminal.next_receipt, excluded.next_receipt)",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(record.id))
            .bind(&record.label)
            .bind(i64::try_from(record.epoch).unwrap_or(i64::MAX))
            .bind(i64::try_from(record.next_receipt).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(records.len())
    }

    async fn put_catalogue(&self, tenant: u128, records: &[CatalogueRecord]) -> Result<usize> {
        let mut transaction = self.scoped(tenant).await?;
        let mut added = 0_usize;
        let mut highest = 0_i64;

        for record in records {
            let seq = i64::try_from(record.seq).unwrap_or(i64::MAX);
            highest = highest.max(seq);
            let result = sqlx::query(
                "insert into catalogue_change (tenant_id, seq, kind, item_id, payload, schema)
                 values ($1, $2, $3, $4, $5, $6)
                 on conflict (tenant_id, seq) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(seq)
            .bind(record.kind)
            .bind(Uuid::from_u128(record.item_id))
            .bind(record.payload.as_deref())
            .bind(i16::from(record.schema))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
            added =
                added.saturating_add(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX));
        }

        // Sequence numbers are preserved, not renumbered, so a till's cursor
        // still points where it did. That leaves one hazard: the shop's counter
        // must end up past everything just written, or the next edit would mint
        // a number an imported row already holds and `do nothing` would discard
        // a real price change without a word.
        sqlx::query("update tenant set catalogue_seq = greatest(catalogue_seq, $2) where id = $1")
            .bind(Uuid::from_u128(tenant))
            .bind(highest)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(added)
    }

    async fn put_sales(&self, tenant: u128, records: &[SaleRecord]) -> Result<usize> {
        let mut transaction = self.scoped(tenant).await?;
        let mut added = 0_usize;

        for record in records {
            // The primary key is (tenant_id, id) and the id was minted on the
            // device, so a second import collides with the first and does
            // nothing. That is what stops a rerun doubling a shop's takings.
            // What it left in a drawer and what its goods cost, read out of
            // the bytes the till committed rather than out of the file or left
            // at nothing. A restore that skipped these would tell a shop its
            // own history made no money and that no drawer it ever counted can
            // be checked against its sales.
            let (cash, cost, costed) = crate::ingest::figures_from_payload(&record.payload);
            let result = sqlx::query(
                "insert into sale (tenant_id, id, terminal_id, receipt_no, receipt_epoch,
                                   rung_at_ms, total_minor, payload, quarantine,
                                   resolution, resolved_at, resolution_kept,
                                   cash_minor, cost_minor, cost_known, quarantine_kind)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                         case when $10 is null then null else now() end, $11,
                         $12, $13, $14, $15)
                 on conflict (tenant_id, id) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(record.id))
            .bind(Uuid::from_u128(record.terminal))
            .bind(record.receipt_no.as_deref())
            .bind(
                record
                    .receipt_epoch
                    .map(|epoch| i64::try_from(epoch).unwrap_or(i64::MAX)),
            )
            .bind(i64::try_from(record.rung_at_ms).unwrap_or(i64::MAX))
            .bind(record.total_minor)
            .bind(&record.payload)
            .bind(record.quarantine.as_deref())
            .bind(record.resolution.as_ref().map(|(note, _)| note.as_str()))
            .bind(record.resolution.as_ref().map(|(_, kept)| *kept))
            .bind(cash)
            .bind(cost)
            .bind(costed)
            .bind((!record.quarantine_kind.is_empty()).then(|| record.quarantine_kind.clone()))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

            // The decision that came in the bundle becomes the first row of the
            // history here, so a restored shop can find it again and change its
            // mind. Only for a sale that was held: deciding is answering the
            // queue, and a sale that never reached it has nothing to answer.
            if let (Some((note, kept)), true) =
                (record.resolution.as_ref(), record.quarantine.is_some())
            {
                sqlx::query(
                    "insert into sale_resolution (tenant_id, sale_id, seq, note, kept)
                     values ($1, $2, 1, $3, $4)
                     on conflict do nothing",
                )
                .bind(Uuid::from_u128(tenant))
                .bind(Uuid::from_u128(record.id))
                .bind(note.as_str())
                .bind(*kept)
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;
            }
            added =
                added.saturating_add(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX));

            // What it owed the revenue, recomputed on the way in rather than
            // carried in the file: a restored shop declares what the original
            // one did rather than what a bundle claimed.
            for (seq, reason) in record.overrides.iter().enumerate() {
                sqlx::query(
                    "insert into sale_override (tenant_id, sale_id, seq, reason)
                     values ($1, $2, $3, $4)
                     on conflict (tenant_id, sale_id, seq) do nothing",
                )
                .bind(Uuid::from_u128(tenant))
                .bind(Uuid::from_u128(record.id))
                .bind(i32::try_from(seq).unwrap_or(i32::MAX))
                .bind(reason)
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;
            }

            for (bp, net, vat, supply) in &record.vat {
                sqlx::query(
                    "insert into sale_vat (tenant_id, sale_id, vat_bp, net_minor, vat_minor,
                                           supply)
                     values ($1, $2, $3, $4, $5, $6)
                     on conflict (tenant_id, sale_id, vat_bp, supply) do nothing",
                )
                .bind(Uuid::from_u128(tenant))
                .bind(Uuid::from_u128(record.id))
                .bind(i32::try_from(*bp).unwrap_or(i32::MAX))
                .bind(*net)
                .bind(*vat)
                .bind(i16::from(*supply))
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepoError::Backend)?;
            }
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(added)
    }

    async fn shifts_after(
        &self,
        tenant: u128,
        after: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClosedShift>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, terminal_id, opened_at_ms, closed_at_ms, opening_float_minor,
                    sales, cash_sales_minor, non_cash_sales_minor, cash_in_minor,
                    cash_out_minor, expected_cash_minor, counted_cash_minor,
                    variance_minor, closed_by, closed_by_name
               from closed_shift
              where tenant_id = $1 and id > $2
                and received_at <= to_timestamp($4 / 1000.0)
              order by id asc
              limit $3",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(after))
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        rows.into_iter().map(shift_from_row).collect()
    }

    async fn account_after(
        &self,
        tenant: u128,
        after: (u128, String),
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<AccountRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select person_key, person_name, source_id, kind, amount_minor, at_ms, note
               from account_entry
              where tenant_id = $1 and (source_id, person_key) > ($2, $3)
                and received_at <= to_timestamp($5 / 1000.0)
              order by source_id asc, person_key asc
              limit $4",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(Uuid::from_u128(after.0))
        .bind(&after.1)
        .bind(i64::from(limit.max(1)))
        .bind(i64::try_from(cut_ms).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let source: Uuid = row.try_get("source_id").map_err(|_| RepoError::Backend)?;
            let at_ms: i64 = row.try_get("at_ms").map_err(|_| RepoError::Backend)?;
            found.push(AccountRecord {
                person_key: row.try_get("person_key").map_err(|_| RepoError::Backend)?,
                person_name: row.try_get("person_name").map_err(|_| RepoError::Backend)?,
                source: source.as_u128(),
                kind: row.try_get("kind").map_err(|_| RepoError::Backend)?,
                amount_minor: row
                    .try_get("amount_minor")
                    .map_err(|_| RepoError::Backend)?,
                at_ms: u64::try_from(at_ms).unwrap_or_default(),
                note: row.try_get("note").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn put_account(&self, tenant: u128, records: &[AccountRecord]) -> Result<usize> {
        let mut transaction = self.scoped(tenant).await?;
        let mut added = 0_usize;
        for record in records {
            // Import is a thing an operator runs twice, once because the first
            // attempt looked like it hung. A second run must not double a debt.
            let row = sqlx::query(
                "insert into account_entry
                    (tenant_id, person_key, person_name, source_id, kind, amount_minor, at_ms, note)
                 values ($1, $2, $3, $4, $5, $6, $7, $8)
                 on conflict do nothing
                 returning source_id",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(&record.person_key)
            .bind(&record.person_name)
            .bind(Uuid::from_u128(record.source))
            .bind(record.kind)
            .bind(record.amount_minor)
            .bind(i64::try_from(record.at_ms).unwrap_or(i64::MAX))
            .bind(&record.note)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
            if row.is_some() {
                added = added.saturating_add(1);
            }
        }
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(added)
    }

    async fn put_stock(&self, tenant: u128, records: &[StockRecord]) -> Result<usize> {
        let mut transaction = self.scoped(tenant).await?;
        let mut added = 0_usize;

        for record in records {
            let result = sqlx::query(
                "insert into stock_movement
                    (tenant_id, source_id, source_kind, item_id, qty_milli, occurred_at_ms)
                 values ($1, $2, $3, $4, $5, $6)
                 on conflict (tenant_id, source_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(tenant))
            .bind(Uuid::from_u128(record.source))
            .bind(record.source_kind)
            .bind(Uuid::from_u128(record.item))
            .bind(record.qty_milli)
            .bind(i64::try_from(record.occurred_at_ms).unwrap_or(i64::MAX))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
            added =
                added.saturating_add(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX));
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(added)
    }
}
