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

use openpos_core::protocol::{ItemWire, QuarantineReason};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::repo::{CataloguePage, LeaseRecord, RepoError, Repository, Result, StoredSale};

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

    /// Record a catalogue upsert and return the new cursor.
    pub async fn upsert_item(&self, tenant: u128, item: &ItemWire) -> Result<u64> {
        let payload = postcard::to_allocvec(item).map_err(|_| RepoError::Backend)?;
        self.append_change(tenant, 1, item.id, Some(payload)).await
    }

    /// Record a catalogue deletion and return the new cursor.
    pub async fn delete_item(&self, tenant: u128, item_id: u128) -> Result<u64> {
        self.append_change(tenant, 2, item_id, None).await
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
            "insert into catalogue_change (tenant_id, seq, kind, item_id, payload)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::from_u128(tenant))
        .bind(seq)
        .bind(kind)
        .bind(Uuid::from_u128(item_id))
        .bind(payload)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(u64::try_from(seq).unwrap_or_default())
    }

    /// Quarantined sales, oldest first, which is what the repair queue lists.
    pub async fn quarantined(&self, tenant: u128, limit: i64) -> Result<Vec<(Uuid, String)>> {
        let mut transaction = self.scoped(tenant).await?;
        let rows = sqlx::query(
            "select id, quarantine from sale
             where quarantine is not null order by received_at limit $1",
        )
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(|_| RepoError::Backend)?;
            let reason: String = row.try_get("quarantine").map_err(|_| RepoError::Backend)?;
            found.push((id, reason));
        }
        Ok(found)
    }
}

/// Quarantine reasons are stored as text rather than as a structured column.
///
/// They are read by a human deciding what to do about a sale, not queried on,
/// and a text column cannot drift out of step with the enum the way a numeric
/// code would after a release that adds a variant.
fn describe(reason: &QuarantineReason) -> String {
    match reason {
        QuarantineReason::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => format!(
            "totals mismatch: the till stored {stored_minor} and the server recomputed {recomputed_minor}"
        ),
        QuarantineReason::DuplicateReceiptNumber { receipt_no } => {
            format!("receipt number {receipt_no} was already used by another sale")
        }
        QuarantineReason::Undecodable => {
            "the payload could not be decoded under the schema it claimed".to_owned()
        }
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
        .bind(sale.quarantine.as_ref().map(describe))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepoError::Backend)?;

        for (item_id, qty_milli) in &sale.stock {
            sqlx::query(
                "insert into stock_movement (tenant_id, sale_id, item_id, qty_milli)
                 values ($1, $2, $3, $4)
                 on conflict (tenant_id, sale_id, item_id) do nothing",
            )
            .bind(Uuid::from_u128(sale.tenant))
            .bind(Uuid::from_u128(sale.id))
            .bind(Uuid::from_u128(*item_id))
            .bind(*qty_milli)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepoError::Backend)?;
        }

        transaction.commit().await.map_err(|_| RepoError::Backend)?;
        Ok(())
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

    async fn items_since(&self, tenant: u128, cursor: u64, limit: u32) -> Result<CataloguePage> {
        let mut transaction = self.scoped(tenant).await?;
        let after = i64::try_from(cursor).unwrap_or(i64::MAX);
        let take = i64::from(limit.max(1));

        let rows = sqlx::query(
            "select seq, kind, item_id, payload from catalogue_change
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
                let payload: Option<Vec<u8>> =
                    row.try_get("payload").map_err(|_| RepoError::Backend)?;
                let bytes = payload.ok_or(RepoError::Backend)?;
                let item: ItemWire =
                    postcard::from_bytes(&bytes).map_err(|_| RepoError::Backend)?;
                page.upserts.push(item);
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
}
