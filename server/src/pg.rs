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

use crate::auth::{Caller, Token, TokenHash};
use crate::repo::{
    describe_quarantine, CataloguePage, LeaseRecord, RepairItem, RepoError, Repository, Result,
    StoredSale, TerminalHealth,
};

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
        self.store_token(Caller { tenant, terminal }, &token.hash())
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

    async fn authenticate(&self, token: &TokenHash) -> Result<Option<Caller>> {
        // Runs outside a tenant-scoped transaction on purpose: this query is
        // what establishes which tenant the request belongs to, so it cannot
        // itself be filtered by one. The table holds hashes and identifiers
        // only, which is why it is the single exception to row-level security.
        let row = sqlx::query(
            "select tenant_id, terminal_id from terminal_token
             where token_hash = $1 and revoked_at is null",
        )
        .bind(token.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let tenant: Uuid = row.try_get("tenant_id").map_err(|_| RepoError::Backend)?;
        let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
        Ok(Some(Caller {
            tenant: tenant.as_u128(),
            terminal: terminal.as_u128(),
        }))
    }

    async fn store_token(&self, caller: Caller, token: &TokenHash) -> Result<()> {
        sqlx::query(
            "insert into terminal_token (token_hash, tenant_id, terminal_id)
             values ($1, $2, $3) on conflict (token_hash) do nothing",
        )
        .bind(token.as_bytes())
        .bind(Uuid::from_u128(caller.tenant))
        .bind(Uuid::from_u128(caller.terminal))
        .execute(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;
        Ok(())
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
        caller: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> Result<()> {
        let seconds = f64::from(u32::try_from(valid_for.as_secs()).unwrap_or(u32::MAX));
        sqlx::query(
            "insert into enrolment_code (code_hash, tenant_id, terminal_id, expires_at)
             values ($1, $2, $3, now() + make_interval(secs => $4))
             on conflict (code_hash) do nothing",
        )
        .bind(code.as_bytes())
        .bind(Uuid::from_u128(caller.tenant))
        .bind(Uuid::from_u128(caller.terminal))
        .bind(seconds)
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
             returning tenant_id, terminal_id",
        )
        .bind(code.as_bytes())
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepoError::Backend)?;

        let Some(row) = row else { return Ok(None) };
        let tenant: Uuid = row.try_get("tenant_id").map_err(|_| RepoError::Backend)?;
        let terminal: Uuid = row.try_get("terminal_id").map_err(|_| RepoError::Backend)?;
        Ok(Some(Caller {
            tenant: tenant.as_u128(),
            terminal: terminal.as_u128(),
        }))
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
}
