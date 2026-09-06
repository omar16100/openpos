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
    describe_quarantine, Admission, CataloguePage, GoodsReceipt, OnHand, StockCorrection, StockCount,
    Supplier,
    CATALOGUE_SCHEMA, TOKEN_LIFETIME, CatalogueRecord, LeaseRecord, RepairItem, RepoError,
    Repository, Result, SaleRecord, StockRecord, StoredSale, TenantRecord, TerminalHealth,
    TerminalRecord,
};

/// Decode a stored catalogue payload under the schema it was written in.
fn decode_catalogue_payload(schema: i16, bytes: &[u8]) -> Option<ItemWire> {
    match schema {
        1 => postcard::from_bytes(bytes).ok(),
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
    pub async fn connect(url: &str, max_connections: u32) -> std::result::Result<Self, sqlx::Error> {
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
        let seq: i64 = row.try_get("catalogue_seq").map_err(|_| RepoError::Backend)?;

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
        Ok(row.flatten().map(|ms| u64::try_from(ms).unwrap_or_default()))
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
        Ok(row.flatten().map(|ms| u64::try_from(ms).unwrap_or_default()))
    }
}

impl Repository for PgRepo {
    async fn has_sale(&self, tenant: u128, id: u128) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query("select 1 as found from sale where id = $1")
            .bind(Uuid::from_u128(id))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        Ok(row.is_some())
    }

    async fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query(
            "select 1 as found from sale where receipt_no = $1 and receipt_epoch = $2",
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

        sqlx::query(
            "insert into sale (tenant_id, id, terminal_id, receipt_no, receipt_epoch,
                               rung_at_ms, total_minor, payload, quarantine)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             on conflict (tenant_id, id) do nothing",
        )
        .bind(Uuid::from_u128(sale.tenant))
        .bind(Uuid::from_u128(sale.id))
        .bind(Uuid::from_u128(sale.terminal))
        .bind(sale.receipt_no.as_deref())
        .bind(sale.receipt_epoch.map(|epoch| i64::try_from(epoch).unwrap_or(i64::MAX)))
        .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
        .bind(sale.total_minor)
        .bind(&sale.payload)
        .bind(sale.quarantine.as_ref().map(describe_quarantine))
        .execute(&mut *transaction)
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
                               rung_at_ms, total_minor, payload, quarantine)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             on conflict (tenant_id, id) do nothing
             returning id",
        )
        .bind(Uuid::from_u128(sale.tenant))
        .bind(Uuid::from_u128(sale.id))
        .bind(Uuid::from_u128(sale.terminal))
        .bind(sale.receipt_no.as_deref())
        .bind(sale.receipt_epoch.map(|epoch| i64::try_from(epoch).unwrap_or(i64::MAX)))
        .bind(i64::try_from(sale.rung_at_ms).unwrap_or(i64::MAX))
        .bind(sale.total_minor)
        .bind(&sale.payload)
        .bind(sale.quarantine.as_ref().map(describe_quarantine))
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
                sqlx::query("update sale set quarantine = $1 where tenant_id = $2 and id = $3")
                    .bind(describe_quarantine(&reason))
                    .bind(Uuid::from_u128(sale.tenant))
                    .bind(Uuid::from_u128(sale.id))
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

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(admission)
    }

    async fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query("select 1 as found from terminal where id = $1")
            .bind(Uuid::from_u128(terminal))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        Ok(row.is_some())
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

        let after: i64 = row.try_get("next_receipt").map_err(|_| RepoError::Backend)?;
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
                "select coalesce(sum(qty_milli), 0)::bigint from stock_movement
                 where item_id = $1",
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

        let counted: i64 = barrier.try_get("counted_milli").map_err(|_| RepoError::Backend)?;
        let counted_at: i64 = barrier.try_get("counted_at_ms").map_err(|_| RepoError::Backend)?;

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
             left join stock_movement m on m.item_id = $1",
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
        let remaining = sqlx::query("select 1 as found from catalogue_change where seq > $1 limit 1")
            .bind(i64::try_from(page.cursor).unwrap_or(i64::MAX))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        page.more = remaining.is_some();

        Ok(page)
    }

    async fn repair_queue(&self, tenant: u128, limit: u32) -> Result<Vec<RepairItem>> {
        let mut transaction = self.scoped(tenant).await?;
        // Matches the partial index added in 0004 exactly, including the
        // `resolved_at is null`. A predicate the index does not cover would make
        // this a sequential scan over every sale the shop has ever taken.
        let rows = sqlx::query(
            "select id, receipt_no, total_minor, quarantine,
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
            });
        }
        Ok(found)
    }

    async fn resolve_quarantine(&self, tenant: u128, sale: u128, note: &str) -> Result<bool> {
        let mut transaction = self.scoped(tenant).await?;
        // `resolved_at is null` in the predicate, so resolving twice reports
        // false rather than overwriting the first person's note with the
        // second's. Two people working one queue is the normal case.
        let result = sqlx::query(
            "update sale set resolved_at = now(), resolution = $2
             where id = $1 and quarantine is not null and resolved_at is null",
        )
        .bind(Uuid::from_u128(sale))
        .bind(note)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;
        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(result.rows_affected() > 0)
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
            "select t.id, t.label, t.epoch,
                    (extract(epoch from t.enrolled_at) * 1000)::bigint  as enrolled_ms,
                    (extract(epoch from t.last_seen_at) * 1000)::bigint as last_seen_ms,
                    count(s.id) as sales,
                    count(s.id) filter (
                        where s.quarantine is not null and s.resolved_at is null
                    ) as open_repairs
             from terminal t
             left join sale s on s.tenant_id = t.tenant_id and s.terminal_id = t.id
             group by t.id, t.label, t.epoch, t.enrolled_at, t.last_seen_at
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
            let open_repairs: i64 = row.try_get("open_repairs").map_err(|_| RepoError::Backend)?;

            found.push(TerminalHealth {
                terminal: terminal.as_u128(),
                label: row.try_get("label").map_err(|_| RepoError::Backend)?,
                epoch: u64::try_from(epoch).unwrap_or(1),
                enrolled_at_ms: millis(&row, "enrolled_ms")?.unwrap_or_default(),
                last_seen_ms: millis(&row, "last_seen_ms")?,
                sales: u64::try_from(sales).unwrap_or_default(),
                open_repairs: u64::try_from(open_repairs).unwrap_or_default(),
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

    async fn tenant_record(&self, tenant: u128) -> Result<Option<TenantRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let row = sqlx::query("select name, catalogue_seq from tenant where id = $1")
            .bind(Uuid::from_u128(tenant))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let name: String = row.try_get("name").map_err(|_| RepoError::Backend)?;
        let seq: i64 = row.try_get("catalogue_seq").map_err(|_| RepoError::Backend)?;
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
            let next: i64 = row.try_get("next_receipt").map_err(|_| RepoError::Backend)?;
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
            let payload: Option<Vec<u8>> = row.try_get("payload").map_err(|_| RepoError::Backend)?;
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

    async fn sales_after(&self, tenant: u128, after_id: u128, limit: u32) -> Result<Vec<SaleRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, terminal_id, receipt_no, receipt_epoch, rung_at_ms, total_minor,
                    payload, quarantine
             from sale where id > $1 order by id limit $2",
        )
        .bind(Uuid::from_u128(after_id))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
            let epoch: Option<i64> = row.try_get("receipt_epoch").map_err(|_| RepoError::Backend)?;
            let rung_at_ms: i64 = row.try_get("rung_at_ms").map_err(|_| RepoError::Backend)?;
            found.push(SaleRecord {
                id: id.as_u128(),
                terminal: terminal.as_u128(),
                receipt_no: row.try_get("receipt_no").map_err(|_| RepoError::Backend)?,
                receipt_epoch: epoch.map(|value| u64::try_from(value).unwrap_or_default()),
                rung_at_ms: u64::try_from(rung_at_ms).unwrap_or_default(),
                total_minor: row.try_get("total_minor").map_err(|_| RepoError::Backend)?,
                payload: row.try_get("payload").map_err(|_| RepoError::Backend)?,
                quarantine: row.try_get("quarantine").map_err(|_| RepoError::Backend)?,
            });
        }
        Ok(found)
    }

    async fn stock_after(
        &self,
        tenant: u128,
        after: (u128, u128),
        limit: u32,
    ) -> Result<Vec<StockRecord>> {
        let mut transaction = self.scoped(tenant).await?;
        // Row comparison, so the pair is one keyset cursor rather than two
        // predicates that would drop the rest of a partially read sale.
        let rows = sqlx::query(
            "select source_id, source_kind, item_id, qty_milli, occurred_at_ms
             from stock_movement
             where (source_id, item_id) > ($1, $2)
             order by source_id, item_id limit $3",
        )
        .bind(Uuid::from_u128(after.0))
        .bind(Uuid::from_u128(after.1))
        .bind(i64::from(limit.max(1)))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let source: Uuid = row.try_get("source_id").map_err(|_| RepoError::Backend)?;
            let item: Uuid = row.try_get("item_id").map_err(|_| RepoError::Backend)?;
            let occurred: i64 = row.try_get("occurred_at_ms").map_err(|_| RepoError::Backend)?;
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
            added = added.saturating_add(
                usize::try_from(result.rows_affected()).unwrap_or(usize::MAX),
            );
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
            let result = sqlx::query(
                "insert into sale (tenant_id, id, terminal_id, receipt_no, receipt_epoch,
                                   rung_at_ms, total_minor, payload, quarantine)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
            added = added.saturating_add(
                usize::try_from(result.rows_affected()).unwrap_or(usize::MAX),
            );
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
            added = added.saturating_add(
                usize::try_from(result.rows_affected()).unwrap_or(usize::MAX),
            );
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(added)
    }
}
