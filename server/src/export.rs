//! Taking one shop's data out, and putting it back.
//!
//! Three things depend on this, and none of them is an operational nicety.
//!
//! 1. **Leaving.** A shop on the hosted tier must be able to take its history to
//!    a self-hosted install, and arrive the same way. Without that the licence
//!    is a gesture: the code is free and the data is held.
//! 2. **Support.** Restoring one shop after a bad day. The alternative is
//!    restoring the whole database, which rolls every other shop back to the
//!    same moment and turns one shop's incident into everybody's.
//! 3. **Onboarding.** A shop arriving with a year of history needs the same
//!    path, so it is built once rather than twice.
//!
//! Everything here goes through [`Repository`], so an export runs inside the
//! same tenant-scoped transactions as the rest of the server and cannot read a
//! row an ordinary request could not. There is no privileged path that skips
//! row-level security, because a privileged path is the one that eventually gets
//! called with the wrong tenant.
//!
//! **An export is not a point-in-time snapshot, and this is worth knowing before
//! relying on one.** Each page is its own transaction, because the repository
//! opens one per call and a snapshot would mean holding a connection open for
//! the length of a shop's history while tills are still syncing through the same
//! pool. A sale that arrives mid-export carrying an id below the cursor is
//! therefore missed, which is not hypothetical here: ids are minted on the
//! device, and a tablet that was offline for a week syncs sales older than
//! anything the server has seen today.
//!
//! What makes that tolerable is that import is idempotent. Exporting again after
//! the tills have drained, and importing the same target a second time, adds
//! exactly the rows the first pass missed and touches nothing else. A migration
//! is therefore run twice on purpose, once to move the bulk and once to converge,
//! rather than once with the tills stopped.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{BufRead, ErrorKind, Write};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::repo::{CATALOGUE_SCHEMA,
    CatalogueRecord, RepoError, Repository, SaleRecord, StockRecord, TenantRecord, TerminalRecord,
};

/// Names the shape of the file, so a file from another tool, or from a future
/// version of this one, is refused rather than half read.
pub const FORMAT: &str = "openpos.tenant.export";

/// Bumped when a reader of this version could misread a newer file. A reader
/// refuses anything it does not recognise: guessing at an unknown record is how
/// an import silently drops half a shop.
pub const FORMAT_VERSION: u32 = 1;

/// Rows per database round trip. Large enough that a year of sales is not a
/// million queries, small enough that no single query holds a shop's history in
/// memory at once.
const PAGE: u32 = 500;

/// Rows per insert transaction on the way back in.
const BATCH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportError {
    /// The store failed.
    Backend,
    /// No such shop. Distinct from an empty shop on purpose: exporting a tenant
    /// id that does not exist would otherwise produce a valid, empty bundle,
    /// and a mistyped id would look like a shop with nothing in it.
    UnknownTenant,
    /// The file does not begin with a header naming this format.
    NotAnExport,
    /// A file from a version this build cannot read.
    UnsupportedVersion { found: u32 },
    /// The file ended before its trailer, or the trailer disagreed with what was
    /// read. Either way the file is incomplete and importing it would load part
    /// of a shop while reporting success.
    Truncated,
    /// A record did not parse, or held an identifier that is not a uuid.
    Malformed { line: usize },
    /// The reader or writer failed. Carries only the kind: the message would be
    /// a path, and a path in an error is a path in a log.
    Io(ErrorKind),
}

impl From<RepoError> for ExportError {
    fn from(_: RepoError) -> Self {
        Self::Backend
    }
}

pub type Result<T> = std::result::Result<T, ExportError>;

// ---------------------------------------------------------------------------
// The file format
// ---------------------------------------------------------------------------

/// One line of an export file.
///
/// JSON Lines, not one JSON document, and the reasons are the failures it
/// prevents.
///
/// A single document has to be complete before anything can be parsed, so a
/// shop with a year of sales would be built entirely in memory on the way out
/// and again on the way in, on a server that is also serving tills. One object
/// per line means the writer emits a sale and forgets it, and the reader handles
/// a sale and forgets it.
///
/// A truncated document is the worse hazard. A JSON array cut short by a full
/// disk is invalid, but a lenient reader can still hand back the elements it
/// managed to read, and an import that loads two thirds of a shop and reports
/// success is worse than one that fails. Here a cut mid-line fails to parse that
/// line, and a cut exactly at a line boundary is caught by the missing trailer.
/// Every file therefore ends with a record stating what should have been in it.
///
/// Identifiers travel as uuid strings rather than as JSON numbers. They are 128
/// bits, and a JSON number above 2^53 is silently rounded by most readers on
/// this planet, including every browser. A rounded sale id is a sale that
/// belongs to nobody.
///
/// Payload bytes travel as hex. They are postcard, not text, and hex is longer
/// than base64 but needs no dependency and cannot be misread; export files
/// compress before they travel anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    Header(Header),
    Tenant(TenantLine),
    Terminal(TerminalLine),
    Catalogue(CatalogueLine),
    Sale(SaleLine),
    Movement(MovementLine),
    Trailer(Trailer),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    /// The shop this file came from. Informational: an import re-homes under
    /// whatever [`IdentityPolicy`] says, and never reads its target from here.
    pub tenant: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantLine {
    pub id: String,
    pub name: String,
    pub catalogue_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLine {
    pub id: String,
    pub label: String,
    pub epoch: u64,
    pub next_receipt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueLine {
    pub seq: u64,
    pub kind: i16,
    pub item_id: String,
    pub payload: Option<String>,
    /// Which shape the payload bytes are in. Optional so a bundle written
    /// before this field existed still imports: those payloads are the only
    /// shape there has ever been.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleLine {
    pub id: String,
    pub terminal: String,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    pub payload: String,
    pub quarantine: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovementLine {
    /// What caused the movement. Named `sale` because that is what a bundle
    /// written before goods receipts existed calls it, and renaming the field
    /// would make those bundles unreadable for no gain.
    pub sale: String,
    pub item: String,
    pub qty_milli: i64,
    /// 1 sale, 2 goods receipt, 3 correction. Absent in older bundles, where
    /// every movement was a sale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_ms: Option<u64>,
}

/// What the file says it contained. Read last and checked against what was
/// actually read, which is what makes a truncation loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Trailer {
    pub terminals: u64,
    pub catalogue: u64,
    pub sales: u64,
    pub movements: u64,
}

// ---------------------------------------------------------------------------
// The bundle
// ---------------------------------------------------------------------------

/// Everything belonging to one shop.
///
/// Deliberately not credentials. Terminal tokens and enrolment codes are left
/// behind, so an export file is not a set of working keys. A shop's history is
/// commercially sensitive; a bundle that also let the holder push sales as one
/// of its terminals would make every copy of a backup a live credential, and
/// backups get emailed. A restored shop re-enrols its devices, which is a few
/// minutes of work once, against a class of compromise that would be permanent.
///
/// The same reasoning is why credentials are not merely unread but unreadable
/// here: the token table stores hashes, so there is nothing to export even if
/// somebody decided to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBundle {
    pub tenant: TenantRecord,
    pub terminals: Vec<TerminalRecord>,
    pub catalogue: Vec<CatalogueRecord>,
    pub sales: Vec<SaleRecord>,
    pub movements: Vec<StockRecord>,
}

impl ExportBundle {
    /// The records this bundle would be written as, header and trailer included.
    #[must_use]
    pub fn records(&self) -> Vec<Record> {
        let mut records = Vec::new();
        records.push(Record::Header(Header {
            format: FORMAT.to_owned(),
            version: FORMAT_VERSION,
            tenant: text_of(self.tenant.id),
        }));
        records.push(Record::Tenant(TenantLine {
            id: text_of(self.tenant.id),
            name: self.tenant.name.clone(),
            catalogue_seq: self.tenant.catalogue_seq,
        }));
        for terminal in &self.terminals {
            records.push(terminal_line(terminal));
        }
        for change in &self.catalogue {
            records.push(catalogue_line(change));
        }
        for sale in &self.sales {
            records.push(sale_line(sale));
        }
        for movement in &self.movements {
            records.push(movement_line(movement));
        }
        records.push(Record::Trailer(self.trailer()));
        records
    }

    fn trailer(&self) -> Trailer {
        Trailer {
            terminals: count(self.terminals.len()),
            catalogue: count(self.catalogue.len()),
            sales: count(self.sales.len()),
            movements: count(self.movements.len()),
        }
    }

    /// Write the bundle as JSON Lines.
    pub fn write_jsonl<W: Write>(&self, writer: &mut W) -> Result<()> {
        for record in self.records() {
            write_line(writer, &record)?;
        }
        writer.flush().map_err(|error| ExportError::Io(error.kind()))
    }

    /// Read a bundle back.
    ///
    /// Takes a `BufRead` rather than a `String`, so a file larger than memory is
    /// read a line at a time even though the bundle it builds is held whole.
    pub fn read_jsonl<R: BufRead>(reader: R) -> Result<Self> {
        let mut builder = Builder::default();
        for (index, line) in reader.lines().enumerate() {
            let number = index.saturating_add(1);
            let line = line.map_err(|error| ExportError::Io(error.kind()))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: Record = serde_json::from_str(&line)
                .map_err(|_| ExportError::Malformed { line: number })?;
            builder.accept(record, number)?;
        }
        builder.finish()
    }
}

fn terminal_line(terminal: &TerminalRecord) -> Record {
    Record::Terminal(TerminalLine {
        id: text_of(terminal.id),
        label: terminal.label.clone(),
        epoch: terminal.epoch,
        next_receipt: terminal.next_receipt,
    })
}

fn catalogue_line(change: &CatalogueRecord) -> Record {
    Record::Catalogue(CatalogueLine {
        seq: change.seq,
        kind: change.kind,
        item_id: text_of(change.item_id),
        payload: change.payload.as_deref().map(to_hex),
        schema: Some(change.schema),
    })
}

fn sale_line(sale: &SaleRecord) -> Record {
    Record::Sale(SaleLine {
        id: text_of(sale.id),
        terminal: text_of(sale.terminal),
        receipt_no: sale.receipt_no.clone(),
        receipt_epoch: sale.receipt_epoch,
        rung_at_ms: sale.rung_at_ms,
        total_minor: sale.total_minor,
        payload: to_hex(&sale.payload),
        quarantine: sale.quarantine.clone(),
    })
}

fn movement_line(movement: &StockRecord) -> Record {
    Record::Movement(MovementLine {
        sale: text_of(movement.source),
        item: text_of(movement.item),
        qty_milli: movement.qty_milli,
        source_kind: Some(movement.source_kind),
        occurred_at_ms: Some(movement.occurred_at_ms),
    })
}

/// Assembles a bundle from records, whichever end they arrive from.
///
/// One implementation for both the database and the file, so the checks a file
/// is subjected to are the same ones an export satisfies. A validator that only
/// runs on the import path is a validator nothing proves.
#[derive(Debug, Default)]
struct Builder {
    seen_header: bool,
    closed: bool,
    tenant: Option<TenantRecord>,
    terminals: Vec<TerminalRecord>,
    catalogue: Vec<CatalogueRecord>,
    sales: Vec<SaleRecord>,
    movements: Vec<StockRecord>,
    trailer: Option<Trailer>,
    // Keys already seen. A file naming one sale twice would import as one sale
    // and report two, because the second insert collides with the first and does
    // nothing. The trailer cannot catch that: it counts lines, not rows.
    terminal_ids: HashSet<u128>,
    catalogue_seqs: HashSet<u64>,
    sale_ids: HashSet<u128>,
    movement_keys: HashSet<(u128, u128)>,
}

impl Builder {
    fn accept(&mut self, record: Record, line: usize) -> Result<()> {
        if self.closed {
            // Anything after the trailer means two files were concatenated, or
            // one was written twice. Refuse rather than merge.
            return Err(ExportError::Malformed { line });
        }
        let malformed = || ExportError::Malformed { line };

        match record {
            Record::Header(header) => {
                if self.seen_header {
                    return Err(malformed());
                }
                if header.format != FORMAT {
                    return Err(ExportError::NotAnExport);
                }
                if header.version != FORMAT_VERSION {
                    return Err(ExportError::UnsupportedVersion {
                        found: header.version,
                    });
                }
                self.seen_header = true;
            }
            _ if !self.seen_header => return Err(ExportError::NotAnExport),
            Record::Tenant(row) => {
                if self.tenant.is_some() {
                    return Err(malformed());
                }
                self.tenant = Some(TenantRecord {
                    id: id_of(&row.id).ok_or_else(malformed)?,
                    name: row.name,
                    catalogue_seq: storable(row.catalogue_seq).ok_or_else(malformed)?,
                });
            }
            Record::Terminal(row) => {
                let id = id_of(&row.id).ok_or_else(malformed)?;
                if !self.terminal_ids.insert(id) {
                    return Err(malformed());
                }
                self.terminals.push(TerminalRecord {
                    id,
                    label: row.label,
                    epoch: storable(row.epoch).ok_or_else(malformed)?,
                    next_receipt: storable(row.next_receipt).ok_or_else(malformed)?,
                });
            }
            Record::Catalogue(row) => {
                // A delete carries no payload and an upsert must carry one:
                // an upsert with nothing to apply would delete an item by
                // arriving as an empty change.
                let payload = match row.payload.as_deref() {
                    Some(text) => Some(from_hex(text).ok_or_else(malformed)?),
                    None => None,
                };
                if (row.kind == 1) != payload.is_some() {
                    return Err(malformed());
                }
                // Sequences start at one, and two changes cannot share a number:
                // both would be one primary key, and the second would be
                // discarded on import without a word.
                let seq = storable(row.seq).ok_or_else(malformed)?;
                if seq == 0 || !self.catalogue_seqs.insert(seq) {
                    return Err(malformed());
                }
                self.catalogue.push(CatalogueRecord {
                    seq,
                    kind: row.kind,
                    item_id: id_of(&row.item_id).ok_or_else(malformed)?,
                    payload,
                    // A bundle written before payloads carried a schema holds
                    // the only shape that ever existed.
                    schema: row.schema.unwrap_or(CATALOGUE_SCHEMA),
                });
            }
            Record::Sale(row) => {
                let id = id_of(&row.id).ok_or_else(malformed)?;
                if !self.sale_ids.insert(id) {
                    return Err(malformed());
                }
                self.sales.push(SaleRecord {
                    id,
                    terminal: id_of(&row.terminal).ok_or_else(malformed)?,
                    receipt_no: row.receipt_no,
                    receipt_epoch: match row.receipt_epoch {
                        Some(epoch) => Some(storable(epoch).ok_or_else(malformed)?),
                        None => None,
                    },
                    rung_at_ms: storable(row.rung_at_ms).ok_or_else(malformed)?,
                    total_minor: row.total_minor,
                    payload: from_hex(&row.payload).ok_or_else(malformed)?,
                    quarantine: row.quarantine,
                });
            }
            Record::Movement(row) => {
                let movement = StockRecord {
                    source: id_of(&row.sale).ok_or_else(malformed)?,
                    // A bundle written before movements had a source is all
                    // sales: that was the only kind that existed.
                    source_kind: row.source_kind.unwrap_or(1),
                    item: id_of(&row.item).ok_or_else(malformed)?,
                    qty_milli: row.qty_milli,
                    occurred_at_ms: row.occurred_at_ms.unwrap_or_default(),
                };
                if !self.movement_keys.insert((movement.source, movement.item)) {
                    return Err(malformed());
                }
                self.movements.push(movement);
            }
            Record::Trailer(trailer) => {
                self.trailer = Some(trailer);
                self.closed = true;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<ExportBundle> {
        if !self.seen_header {
            return Err(ExportError::NotAnExport);
        }
        let (Some(tenant), Some(trailer)) = (self.tenant, self.trailer) else {
            // No trailer means the writer never finished, which is what a full
            // disk or a killed process leaves behind.
            return Err(ExportError::Truncated);
        };

        let counted = Trailer {
            terminals: count(self.terminals.len()),
            catalogue: count(self.catalogue.len()),
            sales: count(self.sales.len()),
            movements: count(self.movements.len()),
        };
        if counted != trailer {
            return Err(ExportError::Truncated);
        }

        Ok(ExportBundle {
            tenant,
            terminals: self.terminals,
            catalogue: self.catalogue,
            sales: self.sales,
            movements: self.movements,
        })
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Everything belonging to one shop: the tenant row, its terminals, its
/// catalogue history, its sales with their payloads and quarantine state, and
/// its stock movements.
///
/// Credentials are deliberately absent. See [`ExportBundle`].
///
/// The reads are paged, so the database is never asked for a year of sales in
/// one statement. The bundle itself is held whole, which is fine for a support
/// restore of one shop; [`stream_tenant`] writes the same records without
/// holding them, for a shop large enough that it matters.
pub async fn export_tenant<R: Repository + ?Sized>(
    repo: &R,
    tenant: u128,
) -> Result<ExportBundle> {
    let mut builder = Builder::default();
    drain(repo, tenant, |record| builder.accept(record, 0)).await?;
    builder.finish()
}

/// Export straight to a writer, without building the bundle first.
pub async fn stream_tenant<R: Repository + ?Sized, W: Write>(
    repo: &R,
    tenant: u128,
    writer: &mut W,
) -> Result<()> {
    drain(repo, tenant, |record| write_line(writer, &record)).await?;
    writer.flush().map_err(|error| ExportError::Io(error.kind()))
}

/// Walk a shop, handing every record to `sink` in the order an import needs
/// them: the shop, then its terminals, then the catalogue, then sales, then the
/// movements that hang off those sales.
async fn drain<R, F>(repo: &R, tenant: u128, mut sink: F) -> Result<()>
where
    R: Repository + ?Sized,
    F: FnMut(Record) -> Result<()>,
{
    let row = repo
        .tenant_record(tenant)
        .await?
        .ok_or(ExportError::UnknownTenant)?;

    sink(Record::Header(Header {
        format: FORMAT.to_owned(),
        version: FORMAT_VERSION,
        tenant: text_of(row.id),
    }))?;
    sink(Record::Tenant(TenantLine {
        id: text_of(row.id),
        name: row.name,
        catalogue_seq: row.catalogue_seq,
    }))?;

    let mut trailer = Trailer::default();

    for terminal in repo.terminal_records(tenant).await? {
        trailer.terminals = trailer.terminals.saturating_add(1);
        sink(terminal_line(&terminal))?;
    }

    let mut seq = 0_u64;
    loop {
        let page = repo.catalogue_after(tenant, seq, PAGE).await?;
        if page.is_empty() {
            break;
        }
        let mut furthest = seq;
        for change in &page {
            furthest = furthest.max(change.seq);
            trailer.catalogue = trailer.catalogue.saturating_add(1);
            sink(catalogue_line(change))?;
        }
        // A page that did not move the cursor would be read again for ever. The
        // query asks for rows strictly after it, so this can only happen if the
        // store is wrong, and looping silently is the worse of the two answers.
        if furthest <= seq {
            return Err(ExportError::Backend);
        }
        seq = furthest;
    }

    let mut after = 0_u128;
    loop {
        let page = repo.sales_after(tenant, after, PAGE).await?;
        if page.is_empty() {
            break;
        }
        let mut furthest = after;
        for sale in &page {
            furthest = furthest.max(sale.id);
            trailer.sales = trailer.sales.saturating_add(1);
            sink(sale_line(sale))?;
        }
        if furthest <= after {
            return Err(ExportError::Backend);
        }
        after = furthest;
    }

    let mut cursor = (0_u128, 0_u128);
    loop {
        let page = repo.stock_after(tenant, cursor, PAGE).await?;
        if page.is_empty() {
            break;
        }
        let mut furthest = cursor;
        for movement in &page {
            furthest = furthest.max((movement.source, movement.item));
            trailer.movements = trailer.movements.saturating_add(1);
            sink(movement_line(movement))?;
        }
        if furthest <= cursor {
            return Err(ExportError::Backend);
        }
        cursor = furthest;
    }

    sink(Record::Trailer(trailer))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// What to do with the identifiers in a bundle.
///
/// Both answers are needed, and choosing wrongly is not a small mistake, which
/// is why the caller has to state one rather than get a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityPolicy {
    /// Keep every identifier, the shop's own included. This is a restore: the
    /// tills still hold sales carrying these ids, and giving the shop a new one
    /// would orphan every outbox in the building.
    Preserve,
    /// Put the shop under a different id, keeping every identifier below it.
    ///
    /// This is a copy into an install that may already hold the shop, which is
    /// exactly the case when a hosted shop is duplicated for testing, or when
    /// two self-hosted shops are consolidated. Only the tenant id changes:
    /// terminal, sale and item ids are unique on their own and are part of what
    /// the receipts and the till outboxes refer to.
    Rehome(u128),
}

impl IdentityPolicy {
    /// Mint a fresh tenant id to re-home under.
    ///
    /// Separate from the enum so the caller keeps the id it minted. An import
    /// that generated one internally would leave the operator with a copy of a
    /// shop and no way to name it.
    #[must_use]
    pub fn mint() -> Self {
        use rand::RngCore;
        let mut bytes = [0_u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        Self::Rehome(Uuid::from_bytes(bytes).as_u128())
    }

    #[must_use]
    fn applied_to(self, bundle: &ExportBundle) -> u128 {
        match self {
            Self::Preserve => bundle.tenant.id,
            Self::Rehome(tenant) => tenant,
        }
    }
}

/// What an import actually changed.
///
/// The counts are rows created, not rows sent. A second import of the same
/// bundle reports zeros, which is the property worth being able to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct ImportOutcome {
    pub tenant: u128,
    pub terminals: usize,
    pub catalogue_added: usize,
    pub sales_added: usize,
    pub movements_added: usize,
}

impl ImportOutcome {
    /// Whether this import created anything at all.
    #[must_use]
    pub fn changed_anything(&self) -> bool {
        self.catalogue_added > 0 || self.sales_added > 0 || self.movements_added > 0
    }
}

/// Load a bundle into a shop.
///
/// Idempotent: running it twice does not double a shop's sales. Every write is
/// keyed on an identifier the till minted, so the second run collides with the
/// first and does nothing. That is not a nicety. Import is the operation someone
/// runs again because the first attempt looked stuck, and a shop's takings
/// silently doubling is the kind of error nobody notices until the VAT return.
///
/// Two things are raised rather than overwritten, both for the same reason: a
/// restore from an older backup must not hand out a number that has already been
/// printed. A terminal's `next_receipt` and epoch only ever move up, and so does
/// the shop's catalogue sequence, so a later edit cannot mint a sequence number
/// an imported row already holds. Catalogue numbers themselves are preserved,
/// not renumbered, because a till's pull cursor is a position in that sequence:
/// renumbering would make every till either re-pull the whole catalogue or skip
/// changes whose new numbers fall below where it already is.
///
/// The order is fixed: the shop, then terminals, then catalogue, then sales,
/// then movements. Movements last because a movement is only meaningful against
/// the sale that caused it.
///
/// A row that is already present wins, and is not compared with the one in the
/// bundle. That is the deliberate choice: the alternative, overwriting, would
/// let an old backup rewrite a sale that has since been repaired, and a sale is
/// the bytes a till committed under an id it minted. Two different sales under
/// one id is corruption, not a merge to be resolved here. The counts in
/// [`ImportOutcome`] are rows created, so a caller that expected a fresh shop
/// and got zeros knows the rows were already there.
pub async fn import_tenant<R: Repository + ?Sized>(
    repo: &R,
    bundle: &ExportBundle,
    policy: IdentityPolicy,
) -> Result<ImportOutcome> {
    let tenant = policy.applied_to(bundle);

    // Raised past every change in the bundle before a single one is written, not
    // after. The tenant row is read at the start of an export and the catalogue
    // after it, so a change made in between arrives with a sequence higher than
    // the counter that travelled with it. Writing the stale counter first would
    // leave a window in which a live edit mints a number the bundle is about to
    // use, and the imported row would then be discarded as a duplicate.
    let highest = bundle
        .catalogue
        .iter()
        .map(|change| change.seq)
        .max()
        .unwrap_or_default();

    repo.put_tenant(&TenantRecord {
        id: tenant,
        name: bundle.tenant.name.clone(),
        catalogue_seq: bundle.tenant.catalogue_seq.max(highest),
    })
    .await?;

    let terminals = repo.put_terminals(tenant, &bundle.terminals).await?;

    let mut catalogue_added = 0_usize;
    for chunk in bundle.catalogue.chunks(BATCH) {
        let added = repo.put_catalogue(tenant, chunk).await?;
        catalogue_added = catalogue_added.saturating_add(added);
    }

    let mut sales_added = 0_usize;
    for chunk in bundle.sales.chunks(BATCH) {
        let added = repo.put_sales(tenant, chunk).await?;
        sales_added = sales_added.saturating_add(added);
    }

    let mut movements_added = 0_usize;
    for chunk in bundle.movements.chunks(BATCH) {
        let added = repo.put_stock(tenant, chunk).await?;
        movements_added = movements_added.saturating_add(added);
    }

    Ok(ImportOutcome {
        tenant,
        terminals,
        catalogue_added,
        sales_added,
        movements_added,
    })
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

fn write_line<W: Write>(writer: &mut W, record: &Record) -> Result<()> {
    // Serialising these types cannot fail: no map has a non-string key and no
    // number is a float. The arm exists because the signature says it can.
    let text = serde_json::to_string(record).map_err(|_| ExportError::Io(ErrorKind::InvalidData))?;
    writer
        .write_all(text.as_bytes())
        .map_err(|error| ExportError::Io(error.kind()))?;
    writer
        .write_all(b"\n")
        .map_err(|error| ExportError::Io(error.kind()))
}

/// A ULID is 128 bits and maps onto a uuid exactly, so an id crosses the file in
/// the same shape it has in the column.
fn text_of(id: u128) -> String {
    Uuid::from_u128(id).to_string()
}

/// Read an identifier, refusing the nil uuid.
///
/// Nil is where the keyset pagination starts, and every reader asks for rows
/// strictly after its cursor. A row carrying it would be exported once and then
/// never again, so it is refused at the door rather than quietly lost later.
fn id_of(text: &str) -> Option<u128> {
    let id = Uuid::parse_str(text).ok()?;
    if id.is_nil() {
        return None;
    }
    Some(id.as_u128())
}

/// Refuse a number Postgres cannot hold.
///
/// These columns are `bigint`. A value above `i64::MAX` would have to be
/// clamped to fit, and two clamped values become one, which on a key column
/// means the second row is discarded on insert.
fn storable(value: u64) -> Option<u64> {
    let ceiling = u64::try_from(i64::MAX).unwrap_or(u64::MAX);
    (value <= ceiling).then_some(value)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Writing to a String cannot fail, and an error path that can never be
        // taken is an error path nothing tests.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() & 1 == 1 {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use openpos_core::protocol::{ItemWire, QuarantineReason};

    use super::*;
    use crate::repo::{MemoryRepo, StoredSale};

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    fn item(id: u128, price_minor: i64) -> ItemWire {
        ItemWire {
            id,
            code: format!("SKU{id:03}"),
            name_en: "Rice Miniket 5kg".to_owned(),
            name_bn: "মিনিকেট চাল ৫ কেজি".to_owned(),
            unit: "Nos".to_owned(),
            price_minor,
            cost_minor: price_minor / 2,
            vat_bp: 1_500,
            price_inclusive: false,
            vat_on_undiscounted: false,
            barcodes: vec!["8690000000012".to_owned()],
            on_hand_milli: 40_000,
            active: true,
        }
    }

    fn sale(id: u128, receipt: &str) -> StoredSale {
        StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id,
            receipt_no: Some(receipt.to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![1, 2, 3, 4],
            quarantine: None,
            stock: vec![(1, -1_000)],
            on_account: Vec::new(),
        }
    }

    /// A shop with a day's trading behind it.
    async fn shop() -> MemoryRepo {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1, 43_000));
        repo.upsert_item(TENANT, item(2, 47_500));
        repo.delete_item(TENANT, 2);
        repo.store_sale(sale(900, "T1-000100")).await.unwrap();
        repo.store_sale(sale(901, "T1-000101")).await.unwrap();

        let mut suspect = sale(902, "T1-000100");
        suspect.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
            receipt_no: "T1-000100".to_owned(),
        });
        repo.store_sale(suspect).await.unwrap();
        repo
    }

    #[tokio::test]
    async fn an_export_carries_the_whole_shop() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();

        assert_eq!(bundle.tenant.id, TENANT);
        assert_eq!(bundle.tenant.catalogue_seq, 3);
        assert_eq!(bundle.terminals.len(), 1);
        assert_eq!(bundle.catalogue.len(), 3);
        assert_eq!(bundle.sales.len(), 3);
        assert_eq!(bundle.movements.len(), 3);
        assert_eq!(bundle.catalogue[2].kind, 2, "a tombstone travels too");
        assert!(
            bundle.sales.iter().any(|sale| sale.quarantine.is_some()),
            "the repair queue must survive an export"
        );
    }

    #[tokio::test]
    async fn exporting_a_shop_that_does_not_exist_is_not_an_empty_shop() {
        let repo = MemoryRepo::new();
        assert_eq!(
            export_tenant(&repo, 999).await,
            Err(ExportError::UnknownTenant)
        );
    }

    #[tokio::test]
    async fn an_export_is_not_a_set_of_working_keys() {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        repo.store_sale(sale(900, "T1-000100")).await.unwrap();

        let mut file = Vec::new();
        stream_tenant(&repo, TENANT, &mut file).await.unwrap();
        let text = String::from_utf8(file).unwrap();

        assert!(
            !text.contains(token.as_str()),
            "an export file must not be a credential"
        );
        assert!(!text.contains("token"), "and must not carry one at all");
    }

    #[tokio::test]
    async fn a_bundle_survives_the_file_format_unchanged() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();

        let mut file = Vec::new();
        bundle.write_jsonl(&mut file).unwrap();
        let read = ExportBundle::read_jsonl(file.as_slice()).unwrap();

        assert_eq!(read, bundle);
    }

    #[tokio::test]
    async fn streaming_and_building_produce_the_same_file() {
        let repo = shop().await;
        let bundle = export_tenant(&repo, TENANT).await.unwrap();

        let mut built = Vec::new();
        bundle.write_jsonl(&mut built).unwrap();
        let mut streamed = Vec::new();
        stream_tenant(&repo, TENANT, &mut streamed).await.unwrap();

        assert_eq!(built, streamed);
    }

    #[tokio::test]
    async fn every_record_is_its_own_line() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let mut file = Vec::new();
        bundle.write_jsonl(&mut file).unwrap();
        let text = String::from_utf8(file).unwrap();

        // Header, tenant, one terminal, three changes, three sales, three
        // movements, trailer.
        assert_eq!(text.lines().count(), 13);
        for line in text.lines() {
            assert!(serde_json::from_str::<Record>(line).is_ok(), "{line}");
        }
    }

    #[tokio::test]
    async fn a_file_cut_at_a_line_boundary_is_refused() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let mut file = Vec::new();
        bundle.write_jsonl(&mut file).unwrap();
        let text = String::from_utf8(file).unwrap();

        // A writer killed part way through leaves whole lines and no trailer.
        let cut: String = text
            .lines()
            .take(6)
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            ExportBundle::read_jsonl(cut.as_bytes()),
            Err(ExportError::Truncated),
            "a file without its trailer must not import as a smaller shop"
        );
    }

    #[tokio::test]
    async fn a_file_cut_mid_record_is_refused() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let mut file = Vec::new();
        bundle.write_jsonl(&mut file).unwrap();
        let text = String::from_utf8(file).unwrap();
        let cut = text.get(..text.len() - 40).unwrap();

        assert!(matches!(
            ExportBundle::read_jsonl(cut.as_bytes()),
            Err(ExportError::Malformed { .. }) | Err(ExportError::Truncated)
        ));
    }

    #[test]
    fn a_trailer_that_disagrees_with_the_file_is_refused() {
        let file = concat!(
            r#"{"record":"header","format":"openpos.tenant.export","version":1,"tenant":"00000000-0000-0000-0000-00000000002a"}"#,
            "\n",
            r#"{"record":"tenant","id":"00000000-0000-0000-0000-00000000002a","name":"Shop","catalogue_seq":0}"#,
            "\n",
            r#"{"record":"trailer","terminals":0,"catalogue":0,"sales":9,"movements":0}"#,
            "\n",
        );
        assert_eq!(
            ExportBundle::read_jsonl(file.as_bytes()),
            Err(ExportError::Truncated),
            "a trailer promising nine sales over a file with none is a lost file"
        );
    }

    #[test]
    fn something_that_is_not_an_export_is_refused() {
        assert_eq!(
            ExportBundle::read_jsonl(&b"{\"hello\":\"world\"}\n"[..]),
            Err(ExportError::Malformed { line: 1 })
        );
        let wrong = concat!(
            r#"{"record":"header","format":"some.other.tool","version":1,"tenant":"00000000-0000-0000-0000-00000000002a"}"#,
            "\n",
        );
        assert_eq!(
            ExportBundle::read_jsonl(wrong.as_bytes()),
            Err(ExportError::NotAnExport)
        );
    }

    #[test]
    fn a_file_from_a_later_version_is_refused_rather_than_guessed_at() {
        let file = concat!(
            r#"{"record":"header","format":"openpos.tenant.export","version":2,"tenant":"00000000-0000-0000-0000-00000000002a"}"#,
            "\n",
        );
        assert_eq!(
            ExportBundle::read_jsonl(file.as_bytes()),
            Err(ExportError::UnsupportedVersion { found: 2 })
        );
    }

    #[tokio::test]
    async fn a_shop_arrives_in_an_empty_install_intact() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let fresh = MemoryRepo::new();

        let outcome = import_tenant(&fresh, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();
        assert_eq!(outcome.tenant, TENANT);
        assert_eq!(outcome.sales_added, 3);
        assert_eq!(outcome.catalogue_added, 3);
        assert_eq!(outcome.movements_added, 3);

        assert_eq!(export_tenant(&fresh, TENANT).await.unwrap(), bundle);
    }

    #[tokio::test]
    async fn importing_the_same_bundle_twice_changes_nothing() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let fresh = MemoryRepo::new();

        let _ = import_tenant(&fresh, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();
        let again = import_tenant(&fresh, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();

        assert!(!again.changed_anything(), "{again:?}");
        assert_eq!(fresh.sale_count(TENANT), 3, "a rerun must not double takings");
        assert_eq!(export_tenant(&fresh, TENANT).await.unwrap(), bundle);
    }

    #[tokio::test]
    async fn a_copy_lands_under_a_new_id_and_leaves_the_original_alone() {
        let repo = shop().await;
        let bundle = export_tenant(&repo, TENANT).await.unwrap();

        // The same install already holds the shop, which is the case a copy has
        // to survive.
        let policy = IdentityPolicy::mint();
        let outcome = import_tenant(&repo, &bundle, policy).await.unwrap();

        assert_ne!(outcome.tenant, TENANT);
        assert_eq!(outcome.sales_added, 3);
        assert_eq!(repo.sale_count(TENANT), 3, "the original is untouched");
        assert_eq!(repo.sale_count(outcome.tenant), 3);

        let copy = export_tenant(&repo, outcome.tenant).await.unwrap();
        assert_eq!(copy.sales, bundle.sales);
        assert_eq!(copy.catalogue, bundle.catalogue);
        assert_eq!(copy.tenant.id, outcome.tenant);
    }

    #[tokio::test]
    async fn the_repair_queue_arrives_with_the_shop() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let fresh = MemoryRepo::new();
        let _ = import_tenant(&fresh, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();

        let queue = fresh.quarantined(TENANT);
        assert_eq!(queue.len(), 1, "a restored shop still has its repair queue");
        assert_eq!(queue[0].id, 902);
    }

    #[tokio::test]
    async fn the_next_catalogue_edit_does_not_collide_with_an_imported_one() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let fresh = MemoryRepo::new();
        let _ = import_tenant(&fresh, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();

        // Had the counter not been raised past the imported rows, this edit
        // would take sequence 1 and be discarded as a duplicate.
        let seq = fresh.upsert_item(TENANT, item(3, 51_000));
        assert_eq!(seq, 4);

        let page = fresh.items_since(TENANT, 3, 10).await.unwrap();
        assert_eq!(page.upserts.len(), 1);
        assert_eq!(page.upserts[0].id, 3);
    }

    #[tokio::test]
    async fn a_restore_never_hands_back_a_receipt_number_already_printed() {
        let repo = shop().await;
        let bundle = export_tenant(&repo, TENANT).await.unwrap();

        // The shop keeps trading after the backup was taken.
        repo.issue_lease(TENANT, TERMINAL, 500).await.unwrap();

        let _ = import_tenant(&repo, &bundle, IdentityPolicy::Preserve)
            .await
            .unwrap();
        let next = repo.issue_lease(TENANT, TERMINAL, 10).await.unwrap();

        assert_eq!(
            next.first, 501,
            "an older bundle must not rewind the counter onto printed numbers"
        );
    }

    /// A file naming one sale twice would import as one sale and report two,
    /// because the second insert collides with the first and does nothing. The
    /// trailer cannot see it: it counts lines.
    #[tokio::test]
    async fn a_file_that_names_the_same_sale_twice_is_refused() {
        let bundle = export_tenant(&shop().await, TENANT).await.unwrap();
        let mut file = Vec::new();
        bundle.write_jsonl(&mut file).unwrap();
        let text = String::from_utf8(file).unwrap();

        let mut lines: Vec<&str> = text.lines().collect();
        let sale = lines[4];
        lines.insert(5, sale);
        let doubled: String = lines.iter().map(|line| format!("{line}\n")).collect();

        assert!(matches!(
            ExportBundle::read_jsonl(doubled.as_bytes()),
            Err(ExportError::Malformed { .. })
        ));
    }

    #[test]
    fn identifiers_and_numbers_the_store_cannot_hold_are_refused() {
        // The nil uuid is where the paging starts, so a row carrying it would be
        // written once and never read back.
        assert_eq!(id_of("00000000-0000-0000-0000-000000000000"), None);
        assert!(id_of("00000000-0000-0000-0000-00000000002a").is_some());
        assert_eq!(id_of("not a uuid"), None);

        // These columns are bigint, and two values clamped to fit become one.
        assert_eq!(storable(0), Some(0));
        assert_eq!(storable(u64::MAX), None);
    }

    #[test]
    fn hex_round_trips_and_refuses_what_is_not_hex() {
        assert_eq!(from_hex(&to_hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(from_hex(""), Some(Vec::new()));
        assert_eq!(from_hex("abc"), None, "an odd length is half a byte");
        assert_eq!(from_hex("zz"), None);
    }
}
