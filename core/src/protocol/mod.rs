//! The network protocol, shared by till and server.
//!
//! Deliberately separate from the on-disk types in [`crate::storage::wire`].
//! Disk needs backward compatibility, because new code reads old bytes. The
//! network needs that *and* forward compatibility, because an old till has to
//! keep working against a newer server. Sharing one struct would mean a protocol
//! change forcing a disk migration, and a disk change breaking the wire.
//!
//! Forward compatibility here comes from negotiation rather than from a
//! self-describing format. Every request states the protocol version it speaks,
//! and the server answers in that version or refuses. postcard is positional and
//! cannot tolerate an unexpected field, so guessing is not an option; saying
//! which dialect you speak is cheap and leaves no ambiguity.
//!
//! A till two weeks offline, syncing into a server that has been upgraded twice,
//! is the case this exists for.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// Protocol this build speaks.
///
/// Bumped when a request or a reply changes shape, which is not the same as
/// adding a route. These bodies are positional: a field added to a struct makes
/// every older body undecodable, so the version is what tells the two sides
/// which shape they are looking at. Version 2 added who counted a drawer.
/// Version 3 added why a sale is held, as the reason itself beside the words,
/// so a screen can say it in the shop's own language: on the repair queue and
/// on a sale looked up by its receipt, which are the two places a shop is shown
/// that a sale is being held. Version 4 added the figures inside a refusal, so
/// a screen can say one in the shop's own words. Version 5 added which receipt
/// a reprint was of, on the trail travelling up from a till and on the trail
/// the back office reads back. Version 6 added, on a closed drawer, how much
/// of that evening's cash belongs to sales the shop has since struck out: the
/// drawer's own figures are deliberately left as the evening recorded them, so
/// the gap between them and the shop's sales is meant to be read, and this is
/// what it takes to read it. Version 7 added, on a line of a receipt, which
/// item it was, so a refund at a counter can put the same goods back on the
/// same shelf and charge back what was charged rather than what the catalogue
/// says today. Version 8 added two totals the back office was adding up for
/// itself: what a delivery cost in all, out of the quantities and the unit
/// costs, and what a month's VAT comes to, out of the rows. Both are the shop's
/// money answered in a second place and a second language, and the second of
/// them is a figure an owner writes on a return. Version 9 added, to the shop's
/// own details and to the request that sets them, which languages a shop offers
/// its own staff: the reply is the one a till reads before it can print, and the
/// request is one a back office running a cached build can still send. Version
/// 10 added, to the list of a shop's devices, which build each said it was
/// running: the first thing worth knowing when one till behaves differently
/// from the one beside it, and until now only answerable by walking to each
/// counter and looking. Version 11 added, to the same list, which counter each
/// device is: the number the shop handed out and the prefix on every receipt
/// that device prints. Without it there is no way from the paper a customer is
/// holding back to the device that printed it, because the list answers with
/// names somebody typed. Version 12 added, to a line of somebody's account, the
/// receipt it was rung on: without it a line says only a day and an amount, and
/// two sales of the same size on one day are two lines nobody can tell apart,
/// which is the line a customer stands at the counter disputing. Version 13
/// added, to a counted drawer, what came back while it was open: a drawer's
/// cash is already net of the goods a shop took back, so one short against a
/// day's selling read the same whether anything came back or not, and money
/// going back across a counter is the oldest way it leaves one.
pub const PROTOCOL_VERSION: u16 = 18;

/// Oldest protocol this build still answers. The server keeps enough slack that
/// a till can be a release behind without being cut off mid-day.
///
/// Slack is not free: every shape that changed since then needs a legacy struct
/// here and a branch where it is read, the same way the storage layer keeps one.
/// Three shapes differ across the versions this build answers, and all three
/// are below: the closed drawer, which version 1 sent without who counted it
/// and versions up to 5 sent without the struck-out cash in its window, the
/// repair queue, which versions 1 and 2 sent without why a sale is held, and
/// the trail, which versions up to 4 sent without which receipt a reprint was
/// of.
pub const MINIMUM_PROTOCOL_VERSION: u16 = 1;

/// Why a request could not be served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolError {
    /// The caller speaks a version this server no longer supports, or one from
    /// the future. Carries the range so the client can report something useful
    /// rather than "sync failed".
    UnsupportedVersion {
        requested: u16,
        minimum: u16,
        current: u16,
    },
    /// The terminal is not enrolled for this tenant, or not enrolled at all.
    UnknownTerminal,
    /// The payload did not decode.
    Malformed,
    /// No credential, or one the server does not recognise.
    ///
    /// Appended rather than inserted: these are encoded positionally, so
    /// reordering the variants would make an older till read one refusal as
    /// another.
    Unauthenticated,
    /// Too many attempts in too short a time. Carries when to try again, so a
    /// client waits rather than hammering.
    TooManyAttempts { retry_after_seconds: u64 },
    /// The credential is genuine but is not for this. A till may ring sales and
    /// sync; it may not reprice the shop.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    NotPermitted,
    /// What was sent was built on an older copy than the shop now holds:
    /// somebody else changed this while it was being edited. Refused rather
    /// than merged, because a whole-item save cannot be merged and the older
    /// answer would win by accident.
    Stale,
    /// A barcode on this item already belongs to another item the shop sells.
    ///
    /// Refused rather than allowed, because a till resolves a scan to one item
    /// and two claiming the same code means it rings whichever its index
    /// happened to keep: the wrong price, the wrong tax and the wrong thing off
    /// the shelf, with nothing on any screen to say why.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    BarcodeInUse { barcode: String },
    /// Deleting an item something has already happened to.
    ///
    /// A deletion is a tombstone: every till drops the item and the reports lose
    /// the name behind figures that are still in the shop's books. That is the
    /// right answer for a line typed by mistake and never sold, and the wrong
    /// one for anything a shop has traded, which is what withdrawing is for.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    ItemHasHistory,
    /// A tax rate that is not a rate, or a price below nothing.
    ///
    /// Refused where it is written rather than where it is read. Every till
    /// applies a page of catalogue changes as one batch and refuses the whole
    /// batch if any item in it is out of range, which is right: an item nobody
    /// can price must not reach a shelf. But it means one impossible rate
    /// stored here stops every till in the shop from receiving any catalogue
    /// change at all, and the cause is nowhere near the symptom.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    NotAPrice { said: String },
    /// A tax rate that is not a rate, said as the rate rather than as a
    /// sentence about it.
    ///
    /// `NotAPrice` above says the same thing in English prose, and stays for a
    /// caller a version behind. It is the last refusal here that carried a
    /// clause instead of a figure, so a shop reading Bangla got its own
    /// sentence with English inside it.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older till read one refusal as another.
    RateIsNotARate { bp: u32 },
    /// A selling price below nothing.
    PriceBelowNothing { minor: i64 },
    /// A cost below nothing.
    CostBelowNothing { minor: i64 },
    /// The caller speaks a version this server has not been upgraded to yet.
    ///
    /// The same facts as `UnsupportedVersion` and the opposite instruction. That
    /// one said "it needs updating" whichever way round the two were, which is
    /// right for the usual case, where a shop's server is upgraded first and
    /// serves the app, and wrong for the case that only happens during a
    /// rollout: a device holding a newer copy of itself than the server it
    /// talks to. Sending somebody to update the tablet then is sending them to
    /// the wrong room.
    ///
    /// Appended, never inserted: these encode positionally. Safe to append
    /// because of who receives it. Only a device newer than the server ever
    /// sees this variant, and a device newer than the server knows it.
    ShopNeedsUpdating {
        requested: u16,
        minimum: u16,
        current: u16,
    },
}

impl ProtocolError {
    /// A frozen name for what was refused, for a screen wording it in the
    /// shop's own language.
    ///
    /// The sentence below is English and stays English, because it is what a
    /// screen falls back to when it has never heard of the refusal: a back
    /// office one release behind a server says something imperfect rather than
    /// nothing. Everything else here is arranged so that the words a person
    /// reads are chosen where the language is known, which is the screen.
    ///
    /// Frozen: `core/tests/refusal_codes.rs` holds the list and refuses a code
    /// that is not on it. A code that changes is a shop reading English again
    /// with nothing anywhere to say why.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion { .. } => "device-needs-updating",
            Self::ShopNeedsUpdating { .. } => "shop-needs-updating",
            Self::UnknownTerminal => "unknown-terminal",
            Self::Malformed => "malformed",
            Self::Unauthenticated => "unauthenticated",
            Self::TooManyAttempts { .. } => "too-many-attempts",
            Self::NotPermitted => "device-not-permitted",
            Self::Stale => "stale",
            Self::BarcodeInUse { .. } => "barcode-in-use",
            Self::ItemHasHistory => "item-has-history",
            Self::NotAPrice { .. } => "not-a-price",
            Self::RateIsNotARate { .. } => "rate-is-not-a-rate",
            Self::PriceBelowNothing { .. } => "price-below-nothing",
            Self::CostBelowNothing { .. } => "cost-below-nothing",
        }
    }
}

impl core::fmt::Display for ProtocolError {
    /// What a refusal says to the person who caused it.
    ///
    /// Here rather than on each screen, for the reason every other message is:
    /// a screen that words a refusal is a second place deciding what the server
    /// meant, and it goes quiet the day a variant is added.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedVersion {
                requested,
                minimum,
                current,
            } => write!(
                f,
                "this device speaks version {requested} and the shop speaks {minimum} to {current}: \
                 it needs updating"
            ),
            Self::ShopNeedsUpdating {
                requested,
                minimum,
                current,
            } => write!(
                f,
                "this device speaks version {requested} and the shop speaks {minimum} to {current}: \
                 the shop's own server is the one to update"
            ),
            Self::UnknownTerminal => {
                f.write_str("the shop has no such till, or this one has been removed")
            }
            Self::Malformed => f.write_str("the shop could not read that request"),
            Self::Unauthenticated => {
                f.write_str("the shop does not recognise this device's credential")
            }
            Self::TooManyAttempts {
                retry_after_seconds,
            } => write!(f, "too many tries: wait {retry_after_seconds} seconds"),
            Self::NotPermitted => {
                f.write_str("this device may not do that: it is a till, not the back office")
            }
            Self::Stale => f.write_str(
                "somebody else changed that while you had it open: read it again before saving",
            ),
            Self::BarcodeInUse { barcode } => write!(
                f,
                "another item you sell already has the barcode {barcode}: one barcode belongs to \
                 one item, or a scan rings whichever the till happens to find"
            ),
            Self::RateIsNotARate { bp } => write!(
                f,
                "{} percent is not a tax rate: a till would refuse the whole page of changes this \
                 arrived in, and stop seeing any of your prices",
                f64::from(*bp) / 100.0
            ),
            Self::PriceBelowNothing { minor } => write!(
                f,
                "a price of {} is below nothing: a till would refuse the whole page of changes \
                 this arrived in, and stop seeing any of your prices",
                crate::receipt::money_of(*minor)
            ),
            Self::CostBelowNothing { minor } => write!(
                f,
                "a cost of {} is below nothing: a till would refuse the whole page of changes \
                 this arrived in, and stop seeing any of your prices",
                crate::receipt::money_of(*minor)
            ),
            Self::NotAPrice { said } => write!(
                f,
                "{said}: a till would refuse the whole page of changes this arrived in, and stop \
                 seeing any of your prices"
            ),
            Self::ItemHasHistory => f.write_str(
                "that has been sold, delivered or counted, so deleting it would take the name off \
                 figures the shop still has to answer for: stop selling it instead, which keeps \
                 the record and takes it off the tills",
            ),
        }
    }
}

/// Check a request's version before doing anything else with it.
pub fn negotiate(requested: u16) -> Result<u16, ProtocolError> {
    // Which of the two is behind, said as itself. Both refusals carry the same
    // three numbers and differ in what somebody should do about them, and that
    // is the whole difference between a shopkeeper updating the tablet in their
    // hand and updating the machine in the back room.
    if requested > PROTOCOL_VERSION {
        return Err(ProtocolError::ShopNeedsUpdating {
            requested,
            minimum: MINIMUM_PROTOCOL_VERSION,
            current: PROTOCOL_VERSION,
        });
    }
    if requested < MINIMUM_PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion {
            requested,
            minimum: MINIMUM_PROTOCOL_VERSION,
            current: PROTOCOL_VERSION,
        });
    }
    Ok(requested)
}

// ---------------------------------------------------------------------------
// Push: sales from a till to the server
// ---------------------------------------------------------------------------

/// One sale, forwarded exactly as it was committed.
///
/// The payload is the bytes already on the terminal's disk, not a re-encoding.
/// Re-encoding risks sending something subtly different from what is stored and
/// from what the receipt in the customer's hand says, and that difference would
/// only ever show up in a dispute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleEnvelope {
    pub id: u128,
    /// Schema of the payload, so the server picks the right decoder rather than
    /// assuming the newest.
    pub schema: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sales: Vec<SaleEnvelope>,
}

/// Why the server would not accept a sale as it stands.
///
/// None of these reject the sale outright. It happened: goods left the shop and
/// money changed hands. The server records it and raises a repair item, because
/// refusing it would mean the only copy is on a tablet that might not survive
/// the week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineReason {
    /// Stored totals disagree with what the shared arithmetic recomputes. Should
    /// be impossible, since both sides run the same crate, so it means tampering
    /// or corruption rather than a rounding difference.
    TotalsMismatch {
        stored_minor: i64,
        recomputed_minor: i64,
    },
    /// Another sale already carries this receipt number under this epoch. The
    /// usual cause is a terminal restored from a backup or a cloned tablet.
    DuplicateReceiptNumber { receipt_no: String },
    /// The payload could not be decoded under the schema it claimed.
    Undecodable,
    /// Carried in by hand from a device that could not send it: a till whose
    /// terminal the shop deleted, or bytes read back out of a torn log. The
    /// sale is stored and somebody is asked to look at it, because the ordinary
    /// path is a credential and this one is a person with a file.
    ///
    /// Appended, never inserted: these encode positionally, so reordering would
    /// make an older device read one reason as another.
    CarriedIn,
    /// The till says it rang this at a time it cannot have.
    ///
    /// A sale cannot be rung after the shop received it, and cannot be rung
    /// before the device that rang it existed. Either means a tablet whose
    /// clock is wrong, which is an ordinary thing for a cheap device that has
    /// been off for a week, and it is not a small matter: the timestamp decides
    /// which day's takings the sale lands in and which month's return it is
    /// declared on.
    ///
    /// Stored and counted like any other held sale. The goods left the shop and
    /// the money is real; what nobody can settle without a person is which day
    /// it belongs to.
    ClockOutOfRange {
        /// What the till said, and what the shop's own clock said when the sale
        /// arrived. Both, because the gap is the story and either alone is a
        /// number nobody can act on.
        rung_at_ms: u64,
        received_at_ms: u64,
    },
    /// A refund naming a receipt this shop does not have.
    ///
    /// A customer's paper the shop cannot find is an ordinary thing: a till
    /// whose sales have not arrived yet, a receipt from before the shop kept
    /// records here, a number read out wrong over a counter. It is also what a
    /// refund invented against no sale at all looks like, and nobody but a
    /// person can tell those apart.
    ///
    /// Held rather than refused, like everything else here: the goods came back
    /// and the money went out, and refusing it would leave the only record of
    /// that on a tablet.
    RefundAgainstNothing { receipt_no: String },
    /// More has been refunded against one receipt than it was ever rung for.
    ///
    /// The oldest trick at a counter: refund the same paper twice and keep the
    /// second one. Also what a customer bringing back half a basket twice looks
    /// like when the first refund was rung for the whole of it, which is why
    /// this is a question for a person rather than a refusal.
    RefundBeyondTheSale {
        receipt_no: String,
        /// What that receipt was rung for, and what has now been refunded
        /// against it including this one. Both, because either alone is a
        /// number nobody can act on.
        sale_minor: i64,
        refunded_minor: i64,
    },
    /// More of something has come back against a receipt than that receipt sold.
    ///
    /// The money can be right and the goods wrong: a refund for the same taka
    /// as the sale, made of something else, or of more of one thing than was
    /// ever bought. What that does is put stock on the shelf that never left it,
    /// which is how a count is made to agree with a shelf somebody emptied.
    MoreCameBackThanWentOut {
        receipt_no: String,
        item_id: u128,
        /// By how much, in thousandths, so a shop can see whether this is a
        /// typo in a quantity or a basket that was never sold.
        over_by_milli: i64,
    },
    /// What was handed over does not come to what the ticket says it was for.
    ///
    /// A till will not close a basket that has not been paid for, so this is
    /// not something a working one produces: it is a payload altered after the
    /// till wrote it, or bytes that rotted. What it would do if it went through
    /// is put a sale in the day's takings that nobody paid for and nobody owes,
    /// leaving a shop looking for money that was never taken.
    TendersDoNotAddUp {
        total_minor: i64,
        /// What the tenders on the ticket come to, and what it says was handed
        /// back as change. Both, because the sum only makes sense with the
        /// change taken out of it.
        tendered_minor: i64,
        change_minor: i64,
    },
}

/// Sales handed to the shop by somebody carrying them, rather than sent.
///
/// The one way out for a device that cannot sync: its terminal was deleted, or
/// it has to be re-enrolled as another and its outbox is the only record of
/// goods that left the shop. Owner only, because the credential that would
/// ordinarily prove where these came from is exactly what is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptSalesRequest {
    pub protocol: u16,
    /// The terminal that rang them, as the device believes itself to be. Kept
    /// as it is even where the shop no longer lists that terminal: it is what
    /// the receipts say, and a sale filed under the wrong till is a sale nobody
    /// can find again.
    pub terminal: u128,
    pub sales: Vec<SaleEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptSalesResponse {
    pub protocol: u16,
    /// Now safely stored, including ones already here from an earlier attempt.
    /// Safe for the device to be wiped once it has seen these.
    pub adopted: Vec<u128>,
    /// Stored, and waiting for somebody to look. Every carried sale is, by the
    /// fact of being carried; this names anything wrong with them beyond that.
    pub needing_attention: Vec<Quarantined>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantined {
    pub id: u128,
    pub reason: QuarantineReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub protocol: u16,
    /// Sales now safely stored, including ones already present from an earlier
    /// attempt. A till may drop these from its outbox.
    pub accepted: Vec<u128>,
    /// Stored, but needing a human. Also safe for the till to drop: the server
    /// has them.
    pub quarantined: Vec<Quarantined>,
}

impl PushResponse {
    /// Everything the till may now consider delivered.
    #[must_use]
    pub fn settled(&self) -> Vec<u128> {
        let mut ids = self.accepted.clone();
        ids.extend(self.quarantined.iter().map(|item| item.id));
        ids
    }
}

// ---------------------------------------------------------------------------
// Pull: catalogue changes from the server to a till
// ---------------------------------------------------------------------------

/// An item as it travels over the network.
///
/// A near-twin of the on-disk shape, and deliberately not the same type. They
/// evolve on different schedules, and the day they need to differ is the day
/// sharing one would have hurt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWire {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    /// True when VAT is charged on the price before discounts.
    ///
    /// Appended, never inserted: these encode positionally, and a field placed
    /// in the middle would make an older till read a barcode as a boolean.
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    /// True for an item a till wrote down at the counter, until somebody in the
    /// back office has looked at it.
    ///
    /// Appended, never inserted, like every field before it. A price typed to
    /// get a queue moving is not a price the shop agreed, and an owner should
    /// be able to find those without reading the whole catalogue.
    #[serde(default)]
    pub from_a_till: bool,
    /// Standard rated, zero rated or exempt, as the number `Supply` is stored
    /// as. A rate of zero cannot say which of the last two a shop meant, and a
    /// return needs them apart.
    ///
    /// Appended, never inserted, like every field before it. A till a release
    /// behind reads nothing here and sells the item at its rate, which is what
    /// that build did anyway.
    #[serde(default)]
    pub supply: u8,
    /// What the shop calls this kind of thing: its own words, not a list this
    /// project chose. Empty for the ones nobody has sorted, which is most of
    /// them on the first day and is not a fault.
    ///
    /// Appended, never inserted. What a thing is sorted under has never been
    /// part of what it costs, so a till a release behind sells it exactly as
    /// it did before.
    #[serde(default)]
    pub category: String,
}

/// An item as version 2 of the catalogue format wrote it when that number was
/// minted: with the tax base, and nothing after it.
///
/// Three fields were appended to `ItemWire` afterwards without the stored
/// schema number moving, so rows stamped 2 exist in three lengths and the
/// build could read only the newest. In this shop's own database seven rows
/// written on the seed date stopped decoding, the back office reported them as
/// written by a version it cannot read, and every till was selling those items
/// at whatever price it already held. The advice on the screen was to type the
/// prices in again.
///
/// So the vintages are written down, and the decoder tries them longest first
/// and takes only the one that consumes the whole payload. A shorter shape
/// reading a longer row would otherwise succeed and quietly drop the fields it
/// has no room for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWireV2 {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

impl ItemWireV2 {
    /// Nothing written this early said where an item came from, what kind of
    /// supply it is, or what the shop sorts it under. Those are what that
    /// build sold it as: the shop's own, standard rated, and unsorted.
    #[must_use]
    pub fn into_current(self) -> ItemWire {
        ItemWire {
            id: self.id,
            code: self.code,
            name_en: self.name_en,
            name_bn: self.name_bn,
            unit: self.unit,
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: self.vat_on_undiscounted,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            from_a_till: false,
            supply: 0,
            category: String::new(),
        }
    }
}

/// The same, once a till could write an item down at the counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWireV2FromATill {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    pub from_a_till: bool,
}

impl ItemWireV2FromATill {
    #[must_use]
    pub fn into_current(self) -> ItemWire {
        ItemWire {
            id: self.id,
            code: self.code,
            name_en: self.name_en,
            name_bn: self.name_bn,
            unit: self.unit,
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: self.vat_on_undiscounted,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            from_a_till: self.from_a_till,
            supply: 0,
            category: String::new(),
        }
    }
}

/// And again, once a shop could say which kind of supply an item is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWireV2Supply {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub vat_on_undiscounted: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
    pub from_a_till: bool,
    pub supply: u8,
}

impl ItemWireV2Supply {
    #[must_use]
    pub fn into_current(self) -> ItemWire {
        ItemWire {
            id: self.id,
            code: self.code,
            name_en: self.name_en,
            name_bn: self.name_bn,
            unit: self.unit,
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: self.vat_on_undiscounted,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            from_a_till: self.from_a_till,
            supply: self.supply,
            category: String::new(),
        }
    }
}

/// An item as version 1 of the catalogue format wrote it.
///
/// Kept only to read what version 1 wrote, and never written. postcard is
/// positional, so the field added for the tax base cannot be read out of these
/// bytes: without this, every catalogue row stored before that change would
/// stop decoding, and every till in every shop would stop pulling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemWireV1 {
    pub id: u128,
    pub code: String,
    pub name_en: String,
    pub name_bn: String,
    pub unit: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub vat_bp: u32,
    pub price_inclusive: bool,
    pub barcodes: Vec<String>,
    pub on_hand_milli: i64,
    pub active: bool,
}

impl ItemWireV1 {
    /// Every item written before the tax base was a choice was taxed the
    /// ordinary way, because that was the only way there was.
    #[must_use]
    pub fn into_current(self) -> ItemWire {
        ItemWire {
            id: self.id,
            code: self.code,
            name_en: self.name_en,
            name_bn: self.name_bn,
            unit: self.unit,
            price_minor: self.price_minor,
            cost_minor: self.cost_minor,
            vat_bp: self.vat_bp,
            price_inclusive: self.price_inclusive,
            vat_on_undiscounted: false,
            barcodes: self.barcodes,
            on_hand_milli: self.on_hand_milli,
            active: self.active,
            // Written before a till could add one, so nobody's counter typed it.
            from_a_till: false,
            // Nothing written before this existed was ever classified, and
            // standard is what that build sold it as.
            supply: 0,
            category: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// Server sequence the till already has.
    pub cursor: u64,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullResponse {
    pub protocol: u16,
    /// Sequence after applying this batch.
    pub cursor: u64,
    pub upserts: Vec<ItemWire>,
    pub tombstones: Vec<u128>,
    /// True when more changes are waiting, so the till knows to ask again rather
    /// than assuming it is current.
    pub more: bool,
}

// ---------------------------------------------------------------------------
// Enrolment: a new device trading a short code for a real credential
// ---------------------------------------------------------------------------

/// The one request that carries no credential, because it is how a device gets
/// one. It states no tenant and no terminal either: both are read from the code,
/// so a device cannot enrol itself into a shop it was not invited to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolRequest {
    pub protocol: u16,
    /// As typed by a person. The server normalises before comparing.
    pub code: String,
}

/// Ask for a code that will enrol a new device.
///
/// The terminal id is minted by the asking device, as sale ids and count ids
/// are. Identity is created where the work happens, so nothing waits on a
/// server to be allowed to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCodeRequest {
    pub protocol: u16,
    pub terminal_id: u128,
    /// What the shop calls this device. Printed in the terminal health list, so
    /// "the one by the door" beats a uuid.
    pub label: String,
    /// 1 till, 2 owner. A caller may not ask for more than it holds.
    pub role: i16,
    pub valid_for_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCodeResponse {
    pub protocol: u16,
    /// Shown once, to be read onto the new device. Never retrievable again:
    /// only its hash is kept.
    pub code: String,
    pub terminal_id: u128,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolResponse {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// Shown to the device once and never retrievable again.
    pub token: String,
}

// ---------------------------------------------------------------------------
// Operators: the people who stand at a till
// ---------------------------------------------------------------------------

/// A person, as they travel.
///
/// The PIN is not here and never is. What crosses is a salt, a round count and
/// a derived key, computed on the owner's device by the same code the till uses
/// to verify: the PIN itself does not go over the network, and a copy of this
/// message is worth no more than a copy of the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorWire {
    pub id: u128,
    pub name: String,
    pub pin_salt: Vec<u8>,
    pub pin_rounds: u32,
    pub pin_key: Vec<u8>,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutOperatorRequest {
    pub protocol: u16,
    pub operator: OperatorWire,
}

/// Change a person, except their PIN.
///
/// Its own request rather than a field on the upsert, because that one carries
/// the whole person including the derived PIN key, and the back office does not
/// have it: a PIN is hashed on the owner's device when it is set and never
/// leaves it. Asking an owner to retype somebody's PIN to correct their name,
/// or to take the drawer away from them, is asking them to know it.
///
/// One request rather than one per field. Suspending and renaming are the same
/// act from here - changing what can be changed without the PIN - and two
/// routes for that would be two places to forget the owner check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmendOperatorRequest {
    pub protocol: u16,
    pub operator_id: u128,
    pub name: String,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

/// Give somebody a new PIN.
///
/// Its own request, carrying a credential and nothing else, because amending a
/// person carries no credential and that is the point of it. Two acts, two
/// shapes, and neither can be used to do the other by leaving a field out.
///
/// The key is derived on the owner's device by the same code the till checks it
/// with, so the PIN itself never travels and this is worth nothing to somebody
/// who reads it off the wire without also having the PIN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetOperatorPinRequest {
    pub protocol: u16,
    pub operator_id: u128,
    pub pin_salt: Vec<u8>,
    /// Carried per person, so raising the cost later does not lock out
    /// everybody who set a PIN before.
    pub pin_rounds: u32,
    pub pin_key: Vec<u8>,
}

/// Ask for the people who may stand at this till.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorsRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorsResponse {
    pub protocol: u16,
    pub operators: Vec<OperatorWire>,
}

// ---------------------------------------------------------------------------
// Shop details: what goes at the top of a receipt
// ---------------------------------------------------------------------------

/// Ask for the shop's own details.
///
/// A separate exchange rather than a field added to the pull, because appending
/// to a reply everything already speaks is a protocol version bump and this is
/// not worth one. A new path costs an old till nothing: it simply never asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopResponse {
    pub protocol: u16,
    pub name: String,
    /// Absent rather than empty when the shop has none. A receipt omits what is
    /// missing; it does not print a label with nothing after it.
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// The wallets this shop takes, by the name a report should read.
    ///
    /// Appended, never inserted: these encode positionally, and a field placed
    /// in the middle would make an older till read a phone number as a list.
    ///
    /// Set here rather than typed at a till, because a shop that takes two will
    /// otherwise type both names all day, and one typo makes a third that then
    /// has its own line in every report and reconciles against nothing.
    #[serde(default)]
    pub wallets: Vec<String>,
    /// What this shop wants done when a basket asks for more than the shelf
    /// holds: 0 nothing, 1 say so, 2 refuse it and let a supervisor allow it.
    ///
    /// A number rather than the enum, and appended like the wallets: a till a
    /// release behind reads the fields it knows and goes on selling, which is
    /// the only acceptable behaviour for a setting about stock.
    #[serde(default)]
    pub stock_rule: u8,
    /// The languages this shop offers its own staff, by the codes the screens
    /// use: `en`, `bn`. Empty means every language the device has, which is
    /// what every shop meant before this field existed.
    ///
    /// A setting about the words this product chose, never about the words the
    /// shop chose: a shop that reads English in the back office still sells
    /// goods whose names are Bangla on the packet, and its catalogue, its
    /// search and its receipts are untouched by this.
    ///
    /// Appended, never inserted, like the two above it.
    #[serde(default)]
    pub languages: Vec<String>,
}

/// The shop's details as versions up to 8 sent them, before a shop could say
/// which languages it offers. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopResponseV8 {
    pub protocol: u16,
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub stock_rule: u8,
}

impl From<ShopResponse> for ShopResponseV8 {
    fn from(new: ShopResponse) -> Self {
        Self {
            protocol: new.protocol,
            name: new.name,
            bin: new.bin,
            address: new.address,
            phone: new.phone,
            wallets: new.wallets,
            stock_rule: new.stock_rule,
        }
    }
}

/// Set the shop's own details. Owner only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutShopRequest {
    pub protocol: u16,
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// Appended, never inserted, for the same reason as on the response.
    #[serde(default)]
    pub wallets: Vec<String>,
    /// What to do when a basket asks for more than the shelf holds. Appended
    /// like the wallets, and read the same way: anything this build does not
    /// know means do nothing.
    #[serde(default)]
    pub stock_rule: u8,
    /// The languages this shop offers its own staff. Empty means all of them,
    /// which is what a shop that has never said means. Appended like the rest.
    #[serde(default)]
    pub languages: Vec<String>,
}

/// The request as versions up to 8 sent it, before a shop could say which
/// languages it offers.
///
/// Read rather than written: a back office is served by the shop's own server,
/// so the two ship together, except that the back office keeps a copy of itself
/// to work with the line down. That copy is a build in the field, it can be a
/// release behind, and this is the body it sends when somebody corrects the
/// shop's address on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutShopRequestV8 {
    pub protocol: u16,
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub stock_rule: u8,
}

impl PutShopRequestV8 {
    /// The same request, with the languages the shop already had.
    ///
    /// Taken from the shop rather than left empty, and that is the whole point
    /// of this being a method rather than a `From`. Empty means "offer every
    /// language", which is a decision, and a screen that has never heard of the
    /// setting must not make it: a shopkeeper correcting an address on a build
    /// a release behind would otherwise turn a language back on at every till
    /// in the shop, and would have no way of knowing they had.
    #[must_use]
    pub fn with_the_languages_it_already_had(self, held: Vec<String>) -> PutShopRequest {
        PutShopRequest {
            protocol: self.protocol,
            name: self.name,
            bin: self.bin,
            address: self.address,
            phone: self.phone,
            wallets: self.wallets,
            stock_rule: self.stock_rule,
            languages: held,
        }
    }
}

// ---------------------------------------------------------------------------
// Purchasing: who the shop buys from, and what arrived
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierWire {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    /// Business Identification Number. Optional because most neighbourhood
    /// suppliers do not have one, and a required field would be filled with
    /// zeros.
    pub bin: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutSupplierRequest {
    pub protocol: u16,
    pub supplier: SupplierWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuppliersRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuppliersResponse {
    pub protocol: u16,
    pub suppliers: Vec<SupplierWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLineWire {
    pub item_id: u128,
    pub qty_milli: i64,
    /// What this delivery cost per unit, which is what a margin is measured
    /// against rather than the item's standing cost.
    pub unit_cost_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveGoodsRequest {
    pub protocol: u16,
    /// Minted on the device, so a delivery survives a dropped reply and can be
    /// resent without being booked twice.
    pub id: u128,
    pub supplier_id: Option<u128>,
    /// The supplier's own invoice or challan number.
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub note: Option<String>,
    pub lines: Vec<ReceiptLineWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveGoodsResponse {
    pub protocol: u16,
    /// False when this delivery was already booked. Not an error: a retry after
    /// a dropped reply is normal, and the caller needs to know it was recognised
    /// rather than silently counted again.
    pub recorded: bool,
    /// What each received item now holds, as the server computes it.
    pub on_hand: Vec<OnHandEntry>,
}

// ---------------------------------------------------------------------------
// Stock counts: asserting what the shelf holds
// ---------------------------------------------------------------------------

/// One counted line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountedItem {
    /// Minted on the device, so a count survives a dropped reply and can be
    /// resent without being recorded twice.
    pub id: u128,
    pub item_id: u128,
    pub counted_milli: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCountRequest {
    pub protocol: u16,
    /// Device clock at the moment of counting, shared by every line in one
    /// count. It decides which sales the count should already reflect, and is
    /// never used to order counts between terminals.
    pub counted_at_ms: u64,
    pub note: Option<String>,
    pub lines: Vec<CountedItem>,
}

/// A drawer that has been counted and closed.
///
/// Pushed rather than kept on the device. The whole point of counting a drawer
/// is that somebody who was not at the till reconciles it, and until this
/// existed a cashier counted, the till worked out the variance, and the owner
/// had to take their word for both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWire {
    pub id: u128,
    pub terminal: u128,
    /// Who counted it, and what they were called at the time. The name is
    /// carried rather than looked up, because somebody who has since left the
    /// shop is still the person this variance belongs to.
    pub closed_by: u128,
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    /// What the drawer should have held.
    pub expected_cash_minor: i64,
    /// What the shop's own sales say that till took in cash while the drawer
    /// was open, plus the float and the movements the till reported.
    ///
    /// The till's expectation is the till's word. This is the same figure
    /// worked out from the sales the shop holds, and the two agreeing is what
    /// makes a variance mean anything. They differ honestly while a till still
    /// has sales to send, which is why both are shown rather than one replacing
    /// the other.
    ///
    /// None where the shop cannot answer: a drawer holding sales from before it
    /// worked this out has no figure of its own, and zero there would read as a
    /// disagreement on every drawer in the shop's history. Appended, never
    /// inserted.
    #[serde(default)]
    pub expected_from_sales_minor: Option<i64>,
    /// How much cash in this drawer's window belongs to sales the shop has
    /// since struck out.
    ///
    /// The two figures above disagree honestly while a till still has sales to
    /// send, and they also disagree for good after somebody strikes a sale out:
    /// the drawer keeps what that evening recorded, deliberately, because a
    /// duplicate that inflated the expectation is exactly what the shortfall
    /// that evening was. Rewriting it would erase the evidence.
    ///
    /// So the gap is meant to be read, and this is what a person needs to read
    /// it: without it the screen names one cause, the till still sending, and
    /// sends an owner to ask a cashier about a difference the back office made.
    ///
    /// None where the shop cannot answer, which is a drawer holding sales from
    /// before the cash on a sale was recorded. Appended, never inserted.
    #[serde(default)]
    pub struck_out_cash_minor: Option<i64>,
    /// What was in it.
    pub counted_cash_minor: i64,
    /// Counted less expected. Negative is short, which is a fact to report
    /// rather than an error: a shift that could not be closed short would be
    /// closed dishonestly instead.
    pub variance_minor: i64,
    /// Goods that came back while this drawer was open: how many tickets, and
    /// what they gave back in cash as a positive figure.
    ///
    /// From the shop's own sales, like the two figures above and for the same
    /// reason. The drawer's cash is already net of these, which is exactly why
    /// a shop has to be told: a drawer short against a day's selling reads the
    /// same whether goods came back or not, and money going back across a
    /// counter is the oldest way it leaves one.
    ///
    /// Zero for a window with none, and for a drawer read by a shop that
    /// cannot say. Appended, never inserted.
    #[serde(default)]
    pub refunds: u32,
    #[serde(default)]
    pub refunded_cash_minor: i64,
}

/// A closed drawer as versions up to 12 sent one, before it said what came
/// back. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWireV12 {
    pub id: u128,
    pub terminal: u128,
    pub closed_by: u128,
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    #[serde(default)]
    pub expected_from_sales_minor: Option<i64>,
    #[serde(default)]
    pub struck_out_cash_minor: Option<i64>,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponseV12 {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWireV12>,
}

impl From<ClosedShiftWire> for ClosedShiftWireV12 {
    fn from(new: ClosedShiftWire) -> Self {
        Self {
            id: new.id,
            terminal: new.terminal,
            closed_by: new.closed_by,
            closed_by_name: new.closed_by_name,
            opened_at_ms: new.opened_at_ms,
            closed_at_ms: new.closed_at_ms,
            opening_float_minor: new.opening_float_minor,
            sales: new.sales,
            cash_sales_minor: new.cash_sales_minor,
            non_cash_sales_minor: new.non_cash_sales_minor,
            cash_in_minor: new.cash_in_minor,
            cash_out_minor: new.cash_out_minor,
            expected_cash_minor: new.expected_cash_minor,
            expected_from_sales_minor: new.expected_from_sales_minor,
            struck_out_cash_minor: new.struck_out_cash_minor,
            counted_cash_minor: new.counted_cash_minor,
            variance_minor: new.variance_minor,
        }
    }
}

/// A closed drawer as version 1 sent one, before it said who counted it.
///
/// Kept so a till a release behind can still hand over the drawer it counted.
/// Losing that is losing the only record that a cashier counted and the till
/// agreed, which is the whole reason it is pushed rather than kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWireV1 {
    pub id: u128,
    pub terminal: u128,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

/// A closed drawer as versions 2 to 5 sent one, before the struck-out cash in
/// its window travelled with it.
///
/// Kept so a till or a back office a release behind is still understood: a
/// drawer is the only record that a cashier counted and the till agreed, and
/// losing one because the shapes moved is losing it for good.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedShiftWireV5 {
    pub id: u128,
    pub terminal: u128,
    pub closed_by: u128,
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    #[serde(default)]
    pub expected_from_sales_minor: Option<i64>,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

impl From<ClosedShiftWireV5> for ClosedShiftWire {
    fn from(old: ClosedShiftWireV5) -> Self {
        Self {
            id: old.id,
            terminal: old.terminal,
            closed_by: old.closed_by,
            closed_by_name: old.closed_by_name,
            opened_at_ms: old.opened_at_ms,
            closed_at_ms: old.closed_at_ms,
            opening_float_minor: old.opening_float_minor,
            sales: old.sales,
            cash_sales_minor: old.cash_sales_minor,
            non_cash_sales_minor: old.non_cash_sales_minor,
            cash_in_minor: old.cash_in_minor,
            cash_out_minor: old.cash_out_minor,
            expected_cash_minor: old.expected_cash_minor,
            expected_from_sales_minor: old.expected_from_sales_minor,
            // A till never sends this and a back office a release behind never
            // asked for it. The shop works it out when it is asked.
            struck_out_cash_minor: None,
            counted_cash_minor: old.counted_cash_minor,
            variance_minor: old.variance_minor,
            // A drawer from a till that did not say what came back. Zero
            // rather than a guess: the shop works this out from its own sales
            // when it shows the drawer, and a figure invented here would be one
            // more thing to disbelieve.
            refunds: 0,
            refunded_cash_minor: 0,
        }
    }
}

impl From<ClosedShiftWire> for ClosedShiftWireV5 {
    fn from(new: ClosedShiftWire) -> Self {
        // The struck-out cash is dropped rather than sent: a reader on the
        // older shape has nowhere to put it and would misread the bytes.
        Self {
            id: new.id,
            terminal: new.terminal,
            closed_by: new.closed_by,
            closed_by_name: new.closed_by_name,
            opened_at_ms: new.opened_at_ms,
            closed_at_ms: new.closed_at_ms,
            opening_float_minor: new.opening_float_minor,
            sales: new.sales,
            cash_sales_minor: new.cash_sales_minor,
            non_cash_sales_minor: new.non_cash_sales_minor,
            cash_in_minor: new.cash_in_minor,
            cash_out_minor: new.cash_out_minor,
            expected_cash_minor: new.expected_cash_minor,
            expected_from_sales_minor: new.expected_from_sales_minor,
            counted_cash_minor: new.counted_cash_minor,
            variance_minor: new.variance_minor,
        }
    }
}

/// Drawers pushed by a till speaking versions 2 to 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsRequestV5 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shifts: Vec<ClosedShiftWireV5>,
}

/// Drawers read by a back office speaking versions 2 to 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponseV5 {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWireV5>,
}

impl From<ClosedShiftWireV1> for ClosedShiftWire {
    fn from(old: ClosedShiftWireV1) -> Self {
        Self {
            id: old.id,
            terminal: old.terminal,
            // A drawer counted by a till that did not write down who counted it.
            // The shop knows it happened and cannot be told by whom.
            closed_by: 0,
            closed_by_name: String::new(),
            opened_at_ms: old.opened_at_ms,
            closed_at_ms: old.closed_at_ms,
            opening_float_minor: old.opening_float_minor,
            sales: old.sales,
            cash_sales_minor: old.cash_sales_minor,
            non_cash_sales_minor: old.non_cash_sales_minor,
            cash_in_minor: old.cash_in_minor,
            cash_out_minor: old.cash_out_minor,
            expected_cash_minor: old.expected_cash_minor,
            // A back office a release behind never sent the shop's own figure.
            expected_from_sales_minor: None,
            // Nor the struck-out cash, which that build never sent either.
            struck_out_cash_minor: None,
            counted_cash_minor: old.counted_cash_minor,
            variance_minor: old.variance_minor,
            // A drawer from a till that did not say what came back. Zero
            // rather than a guess: the shop works this out from its own sales
            // when it shows the drawer, and a figure invented here would be one
            // more thing to disbelieve.
            refunds: 0,
            refunded_cash_minor: 0,
        }
    }
}

impl From<ClosedShiftWire> for ClosedShiftWireV1 {
    fn from(new: ClosedShiftWire) -> Self {
        // The name is dropped rather than translated: a version 1 reader has
        // nowhere to put it and would misread the bytes if it were sent.
        Self {
            id: new.id,
            terminal: new.terminal,
            opened_at_ms: new.opened_at_ms,
            closed_at_ms: new.closed_at_ms,
            opening_float_minor: new.opening_float_minor,
            sales: new.sales,
            cash_sales_minor: new.cash_sales_minor,
            non_cash_sales_minor: new.non_cash_sales_minor,
            cash_in_minor: new.cash_in_minor,
            cash_out_minor: new.cash_out_minor,
            expected_cash_minor: new.expected_cash_minor,
            counted_cash_minor: new.counted_cash_minor,
            variance_minor: new.variance_minor,
        }
    }
}

/// Drawers pushed by a version 1 till.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsRequestV1 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shifts: Vec<ClosedShiftWireV1>,
}

/// Drawers read by a version 1 back office.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponseV1 {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWireV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shifts: Vec<ClosedShiftWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushShiftsResponse {
    pub protocol: u16,
    /// Shifts the server now holds, including ones it already had: a till may
    /// drop these. A repeat is ordinary, not an error, because a dropped reply
    /// is the usual reason a till sends one twice.
    pub accepted: Vec<u128>,
}

/// A privileged action a device allowed, on its way to the shop.
///
/// Both names travel rather than only the ids. The shop can look an id up, but
/// the name at the time is what a person reads, and somebody since renamed or
/// gone from the shop is still who this belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedWire {
    /// The device's own count of what it has allowed. With the terminal, this
    /// is what makes one storable exactly once: two identical actions in the
    /// same millisecond are possible and are two different things.
    pub seq: u64,
    pub at_ms: u64,
    /// 1 discount, 2 price override, 3 refund, 4 void a line, 5 open the
    /// drawer, 6 close the drawer, 7 a PIN typed wrongly, 8 a PIN typed wrongly
    /// that locked that person out, 9 somebody signing in, 10 more sold than
    /// the shop has, 11 tried to take a line off a paid basket, 12 sold to
    /// somebody already past what they may owe, 13 tried to open the drawer,
    /// 14 a receipt printed again.
    ///
    /// Seven and eight, and eleven and thirteen, are not actions anybody was
    /// allowed to take: they are somebody failing to be allowed. Nine is
    /// somebody taking the till. They travel here because they belong in the
    /// same list for the person reading it, who is looking at one evening and
    /// asking what happened at that counter.
    ///
    /// Numbers are never reused. A shop's stored trail is read under this list,
    /// so a number that changes meaning is last year's evenings quietly saying
    /// something else.
    pub action: u8,
    /// Basis points, for a discount. Zero otherwise.
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    /// Zero when nobody had to allow it: the operator's own permission covered
    /// it, which is a different fact from a supervisor standing at the counter.
    pub authorised_by: u128,
    pub authorised_by_name: String,
    /// The receipt a reprint was of. `None` for every other kind, and for
    /// anything a device wrote before it carried one.
    pub receipt_no: Option<String>,
}

/// One privileged action as versions up to 4 sent it, before a reprint named
/// its receipt.
///
/// A till a release behind still has to be able to hand over what it allowed.
/// The alternative is not a missing field: postcard is positional, so its body
/// read as the current shape is a decode failure, and the device is left
/// holding the only record of who allowed what while its pushes fail on a
/// timer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedWireV4 {
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
}

impl From<AllowedWireV4> for AllowedWire {
    fn from(old: AllowedWireV4) -> Self {
        Self {
            seq: old.seq,
            at_ms: old.at_ms,
            action: old.action,
            bp: old.bp,
            operator: old.operator,
            operator_name: old.operator_name,
            authorised_by: old.authorised_by,
            authorised_by_name: old.authorised_by_name,
            // That build did not know which receipt, and nothing here may
            // decide for it: an answer invented on the way up lands on the
            // screen a shop reads to decide whether somebody took money.
            receipt_no: None,
        }
    }
}

/// The same push as versions up to 4 sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushAllowedRequestV4 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub allowed: Vec<AllowedWireV4>,
}

/// What a till allowed, sent so the shop holds it rather than the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushAllowedRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub allowed: Vec<AllowedWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushAllowedResponse {
    pub protocol: u16,
    /// The counts the server now holds, including ones it already had: a till
    /// may drop these. A repeat is ordinary rather than an error, because a
    /// dropped reply is the usual reason a till sends one twice.
    pub stored: Vec<u64>,
}

/// Where the shop's numbering jumps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptGapsRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// One run of numbers with no sale against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptGapWire {
    pub terminal: u128,
    pub epoch: u64,
    /// The number before the gap and the number after it, as they are printed,
    /// because those are the two a person can look up.
    pub after: String,
    pub before: String,
    pub missing: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptGapsResponse {
    pub protocol: u16,
    pub gaps: Vec<ReceiptGapWire>,
}

/// What the shop allowed, and who allowed it, for the back office to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
    pub limit: u32,
}

/// One line of the trail, as the back office reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedEntry {
    pub terminal: u128,
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
    /// The receipt a reprint was of. `None` for every other kind, and for
    /// anything a device wrote before it carried one.
    pub receipt_no: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedResponse {
    pub protocol: u16,
    pub allowed: Vec<AllowedEntry>,
}

/// One trail entry as versions up to 4 read it.
///
/// A back office a release behind reads who and when, which is what it could
/// show anyway. Sending the newer shape would not read as a missing field: it
/// would read as a decode failure, and the screen would show an error where
/// the trail should be, on the screen a shop opens when it suspects something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedEntryV4 {
    pub terminal: u128,
    pub seq: u64,
    pub at_ms: u64,
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    pub authorised_by: u128,
    pub authorised_by_name: String,
}

impl From<AllowedEntry> for AllowedEntryV4 {
    fn from(now: AllowedEntry) -> Self {
        Self {
            terminal: now.terminal,
            seq: now.seq,
            at_ms: now.at_ms,
            action: now.action,
            bp: now.bp,
            operator: now.operator,
            operator_name: now.operator_name,
            authorised_by: now.authorised_by,
            authorised_by_name: now.authorised_by_name,
        }
    }
}

/// The trail as versions up to 4 read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedResponseV4 {
    pub protocol: u16,
    pub allowed: Vec<AllowedEntryV4>,
}

/// Cut a device off.
///
/// What a shop needs the moment a tablet is lost or stolen: every credential
/// that terminal holds stops working. The terminal itself stays, because its
/// sales are still its sales and a shop investigating a theft wants to see that
/// a device existed rather than an absence.
///
/// The device is not wiped and cannot be: it may be holding sales nobody else
/// has, and if it is ever recovered those are read off it and carried in by
/// hand. Cutting it off is what stops it doing anything new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeTerminalRequest {
    pub protocol: u16,
    pub terminal: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeTerminalResponse {
    pub protocol: u16,
    /// How many credentials were withdrawn. Zero is an ordinary answer: a
    /// device enrolled and never used, or one already cut off.
    pub withdrawn: u32,
}

/// Ask what supervisors waived over a period.
///
/// The question an owner asks when the takings are light and everybody was on
/// shift: what was given away, on whose say-so. What was waived is on the
/// customer's receipt already; this is the shop's side of the same sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaivedRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
    pub limit: u32,
}

/// One thing a supervisor allowed, and the sale it was allowed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaivedWire {
    pub sale_id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    /// As the till wrote it, which is what the customer's paper says too.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaivedResponse {
    pub protocol: u16,
    pub waived: Vec<WaivedWire>,
}

/// Ask what sold over a period.
///
/// The question a shop asks before it orders: what moved, and how much of it.
/// Answered from the stock movements each sale wrote rather than from its
/// payload, because those are already the server's own reading of the lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
    pub limit: u32,
}

/// How much of one item left the shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldWire {
    pub item_id: u128,
    /// Positive is what left the shop. A period with more returns than sales of
    /// one thing shows negative, which is a fact worth seeing.
    pub qty_milli: i64,
    pub sales: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldResponse {
    pub protocol: u16,
    /// Most sold first. Names are not here: the device asking already holds the
    /// catalogue, and sending them again would be the same strings on every
    /// report for the life of the shop.
    pub rows: Vec<SoldWire>,
}

/// Ask what the shop owes its suppliers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingRequest {
    pub protocol: u16,
}

/// What the shop owes one supplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingWire {
    pub supplier_id: u128,
    pub name: String,
    /// Positive is owed by the shop. Negative means it has paid ahead, which
    /// happens and is worth showing rather than hiding.
    pub owed_minor: i64,
    pub deliveries: u32,
    pub since_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwingResponse {
    pub protocol: u16,
    pub owing: Vec<SupplierOwingWire>,
}

/// Ask which catalogue changes never reached the tills.
///
/// A change this build cannot read is passed over and the cursor moves on, so a
/// shop can lose a price change and have no way to find out. This is the way to
/// find out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangesRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// Say every item to the tills again. Owner only.
///
/// A till follows the catalogue by a cursor, and a row it passed over is a row
/// it will never be offered again. That is deliberate: stopping the whole
/// catalogue over one bad row stops every till in the shop. The way out is to
/// say everything again, which is what a shop was already being told to do by
/// hand, one item at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResendCatalogueRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResendCatalogueResponse {
    pub protocol: u16,
    /// How many items were said again, which is what the shop is told: a
    /// number it can compare with what it believes it sells.
    pub sent: u64,
}

/// Items a till wrote down at a counter that nobody has looked at yet. Owner
/// only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillItemsRequest {
    pub protocol: u16,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillItemsResponse {
    pub protocol: u16,
    /// As the shop holds them now, so the screen shows what it would be
    /// agreeing to rather than what was typed at the counter.
    pub items: Vec<ItemWire>,
}

/// One change every till has passed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangeWire {
    pub seq: u64,
    pub item_id: u128,
    /// The schema its payload was written under: one number naming the build
    /// that wrote it, which is what somebody needs to know to fix it.
    pub schema: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChangesResponse {
    pub protocol: u16,
    pub changes: Vec<UnreadableChangeWire>,
}

/// Ask what passed between the shop and one supplier over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierStatementRequest {
    pub protocol: u16,
    pub supplier_id: u128,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// One line of that: goods in, or money out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierEntryWire {
    pub at_ms: u64,
    /// True when goods came in, false when money went out.
    pub delivered: bool,
    /// Positive either way: what arrived, or what was handed over.
    pub amount_minor: i64,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierStatementResponse {
    pub protocol: u16,
    /// Oldest first, and a delivery before a payment made in the same moment:
    /// goods arrive and are then paid for.
    pub entries: Vec<SupplierEntryWire>,
    /// What the period ends owing, so the paper and the total agree without the
    /// screen adding the lines up itself.
    pub owed_minor: i64,
}

/// Record money paid to a supplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaySupplierRequest {
    pub protocol: u16,
    /// Minted by whoever recorded it, so a resent one is not counted twice.
    pub id: u128,
    pub supplier_id: u128,
    /// What was handed over. Positive.
    pub amount_minor: i64,
    pub paid_at_ms: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaySupplierResponse {
    pub protocol: u16,
    /// False when this payment was already recorded, which is ordinary: a
    /// dropped reply is the usual reason one is sent twice.
    pub paid: bool,
    /// What the shop owes them now, from the ledger rather than from the
    /// screen's own arithmetic.
    pub owed_minor: i64,
}

/// Ask what was sold at each tax rate over a period.
///
/// The figure a shop needs for its monthly return, which until now lived only
/// inside the sale payloads: answering it meant decoding every ticket of the
/// month, the most expensive way to answer a question asked twelve times a year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// What was sold at one rate, and the tax on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRowWire {
    /// Basis points, so fifteen percent is 1500 and a rate that changes next
    /// year is a different row rather than a rewrite of this one.
    pub vat_bp: u32,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub sales: u64,
    /// 0 standard rated, 1 zero rated, 2 exempt. Two rows can both be at
    /// nothing and belong in different places on a return, which a rate alone
    /// cannot say. Appended, never inserted.
    #[serde(default)]
    pub supply: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatResponse {
    pub protocol: u16,
    /// Smallest rate first. Refunds carry their own sign and subtract, which is
    /// what a return wants.
    pub rows: Vec<VatRowWire>,
    /// How much of that figure comes from sales still waiting on somebody to
    /// look at them. Counted rather than removed: a duplicate receipt
    /// over-declares and a sale nobody has looked at may be either, and this is
    /// a number a shop signs its name to. The machine says how much is
    /// uncertain; the person filing decides.
    pub waiting_sales: u64,
    pub waiting_vat_minor: i64,
    /// What the rows come to, added up where the rest of this shop's money is.
    /// Appended, never inserted. The screen was summing the rows itself, which
    /// is the one figure on that panel an owner writes on a return.
    pub vat_minor: i64,
}

/// The VAT summary as versions up to 7 sent it, before it carried its own
/// total. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatResponseV7 {
    pub protocol: u16,
    pub rows: Vec<VatRowWireV7>,
    pub waiting_sales: u64,
    pub waiting_vat_minor: i64,
}

impl From<VatResponse> for VatResponseV7 {
    fn from(new: VatResponse) -> Self {
        Self {
            protocol: new.protocol,
            rows: new
                .rows
                .into_iter()
                .map(|row| VatRowWireV7 {
                    vat_bp: row.vat_bp,
                    net_minor: row.net_minor,
                    vat_minor: row.vat_minor,
                    sales: row.sales,
                    supply: row.supply,
                })
                .collect(),
            waiting_sales: new.waiting_sales,
            waiting_vat_minor: new.waiting_vat_minor,
        }
    }
}

/// Ask what a day looked like.
///
/// The question an owner asks once, at closing: what was sold, what came back,
/// what the drawers held against what they should have, and what went on
/// account rather than into the till. Answered in one call because it is one
/// question, and from the headers and the two ledgers rather than by decoding
/// tickets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayRequest {
    pub protocol: u16,
    /// Inclusive, by the clock of whoever rang the sale. A shop's day ends when
    /// it closes, not at midnight.
    pub from_ms: u64,
    pub to_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayResponse {
    pub protocol: u16,
    pub sales: u64,
    pub total_minor: i64,
    pub refunds: u64,
    pub refunded_minor: i64,
    /// Drawers counted and closed in the period, and how they came out.
    pub drawers_counted: u32,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
    /// Put on somebody's account, taken off it, and struck off without money.
    /// Three numbers, because money the shop was given and money it gave up are
    /// not the same thing and a day that nets to zero because one balanced the
    /// other is a day somebody should look at.
    pub charged_minor: i64,
    /// Goods brought back by somebody who took them on account, as what came
    /// off the book. Counted apart from what was charged rather than netted
    /// into it: a day where three thousand went on and three thousand came back
    /// is not a day where nothing happened, and a report that shows one figure
    /// for both cannot be asked which it was.
    pub returned_minor: i64,
    pub paid_minor: i64,
    pub written_off_minor: i64,
    pub tills: Vec<TillTakings>,
    /// The same period by whoever rang the sale, biggest first. Appended.
    ///
    /// A till answers "which counter", and one counter is stood at by three
    /// people in a day. This is the other question, and it became askable the
    /// day a sale started recording who rang it.
    #[serde(default)]
    pub people: Vec<TakenByPersonWire>,
}

/// What one person rang in a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakenByPersonWire {
    /// Nil for the sales that name nobody, which is every sale rung before a
    /// till recorded it. Counted apart rather than shared out or left off: one
    /// would be inventing a name, and the other would make these rows add up to
    /// less than the day above them.
    pub operator: u128,
    /// As the shop calls them now, and empty for nobody. Resolved when the
    /// question is asked, like every other name this product prints, so that
    /// somebody renamed reads as they are called today.
    pub name: String,
    pub sales: u64,
    pub total_minor: i64,
    pub refunds: u64,
    pub refunded_minor: i64,
}

/// One till's part of a period, as versions up to 16 sent it.
///
/// Its own copy rather than a pointer at the live shape, because a frozen shape
/// written in terms of one that can still move stops reading the bytes it was
/// kept for the day that one grows. The guard in `protocol_shapes` is what says
/// so, and it said so about this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillTakingsV16 {
    pub terminal: u128,
    pub sales: u64,
    pub total_minor: i64,
    pub needing_attention: u64,
}

impl From<TillTakings> for TillTakingsV16 {
    fn from(new: TillTakings) -> Self {
        Self {
            terminal: new.terminal,
            sales: new.sales,
            total_minor: new.total_minor,
            needing_attention: new.needing_attention,
        }
    }
}

/// What a day looked like, as versions up to 16 were answered: without the
/// people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayResponseV16 {
    pub protocol: u16,
    pub sales: u64,
    pub total_minor: i64,
    pub refunds: u64,
    pub refunded_minor: i64,
    pub drawers_counted: u32,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
    pub charged_minor: i64,
    pub returned_minor: i64,
    pub paid_minor: i64,
    pub written_off_minor: i64,
    pub tills: Vec<TillTakingsV16>,
}

impl From<DayResponse> for DayResponseV16 {
    fn from(new: DayResponse) -> Self {
        Self {
            protocol: new.protocol,
            sales: new.sales,
            total_minor: new.total_minor,
            refunds: new.refunds,
            refunded_minor: new.refunded_minor,
            drawers_counted: new.drawers_counted,
            expected_cash_minor: new.expected_cash_minor,
            counted_cash_minor: new.counted_cash_minor,
            variance_minor: new.variance_minor,
            charged_minor: new.charged_minor,
            returned_minor: new.returned_minor,
            paid_minor: new.paid_minor,
            written_off_minor: new.written_off_minor,
            tills: new.tills.into_iter().map(Into::into).collect(),
        }
    }
}

/// Somebody the shop lets buy on account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerWire {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
    /// Their Business Identification Number, when the buyer is a business.
    /// Appended, never inserted: a till a release behind reads the fields it
    /// knows and goes on selling.
    #[serde(default)]
    pub bin: Option<String>,
    /// The most the shop will let them owe at once, in poisha. Zero is no cap,
    /// which is what every shop has until it says otherwise.
    ///
    /// A till a release behind reads nothing here and sells on account as it
    /// always did, which is the shop's own position until it sets one.
    #[serde(default)]
    pub limit_minor: i64,
    /// Where they are, for the invoice. Appended.
    ///
    /// Section 51(1)(c) of the Value Added Tax and Supplementary Duty Act, 2012
    /// asks for the buyer's name, address and business identification number
    /// once a supply is worth more than 25,000 taka, and 51(2) says no input tax
    /// credit is admissible against an invoice without them. This product held
    /// the name and the BIN and had nowhere to put the third, so the clause
    /// could not be met however carefully a shop filled the rest in.
    ///
    /// One line of text rather than parts. What goes on a receipt in this
    /// country is a line somebody wrote down, and splitting it into fields is a
    /// way of being wrong about addresses in a language whose addresses this
    /// code has no business modelling.
    #[serde(default)]
    pub address: Option<String>,
}

/// Somebody who buys on account, as versions up to 14 sent one: no address.
///
/// Frozen because three shapes carry it and all three travel. A till pushes
/// the people it wrote down at a counter, the shop answers a till and a back
/// office with its list, and the back office writes one back. postcard is
/// positional, so a field appended here moves every one of those bodies, and a
/// build on either end that has not been upgraded reads the next field as the
/// start of something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerWireV14 {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    pub active: bool,
    pub bin: Option<String>,
    pub limit_minor: i64,
}

impl From<CustomerWireV14> for CustomerWire {
    fn from(old: CustomerWireV14) -> Self {
        Self {
            id: old.id,
            name: old.name,
            phone: old.phone,
            active: old.active,
            bin: old.bin,
            limit_minor: old.limit_minor,
            // A build that had nowhere to put one. Nothing rather than an
            // empty line, which would print as a blank row on an invoice.
            address: None,
        }
    }
}

impl From<CustomerWire> for CustomerWireV14 {
    fn from(new: CustomerWire) -> Self {
        Self {
            id: new.id,
            name: new.name,
            phone: new.phone,
            active: new.active,
            bin: new.bin,
            limit_minor: new.limit_minor,
        }
    }
}

/// What a till up to 14 pushed: people it wrote down, without addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushCustomersRequestV14 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub customers: Vec<CustomerWireV14>,
}

/// What a device up to 14 was answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomersResponseV14 {
    pub protocol: u16,
    pub customers: Vec<CustomerWireV14>,
}

/// What a back office up to 14 wrote back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCustomerRequestV14 {
    pub protocol: u16,
    pub customer: CustomerWireV14,
}

/// People a till wrote down at the counter, on their way to the shop.
///
/// Somebody buys on account who is in nobody's list. Writing them down at the
/// till is what keeps two people with one name apart, and this is how the shop
/// comes to hold them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushCustomersRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub customers: Vec<CustomerWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushCustomersResponse {
    pub protocol: u16,
    /// The ids the shop now holds. A till drops only these.
    pub stored: Vec<u128>,
}

/// Ask where the shop's settings stand, as one number.
///
/// The people, the shop's own details and who buys on account move together
/// from a till's point of view: it re-reads all three or none. A till asks for
/// this on the cadence it pulls the catalogue at, and asks for the lists
/// themselves only when the number has moved. Suspending somebody then reaches
/// every till in half a minute instead of ten, without three large replies a
/// minute per till for data nobody touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub protocol: u16,
    pub seq: u64,
}

/// Items a till wrote down itself, on their way to the shop.
///
/// A delivery arrives during an outage with a barcode in nobody's catalogue.
/// The till writes the item down so the sale can happen, and sends it here when
/// it can. The shop keeps them marked as a till's work until somebody looks:
/// a price typed at a counter to get a queue moving is not a price the owner
/// has agreed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushItemsRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub items: Vec<ItemWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushItemsResponse {
    pub protocol: u16,
    /// The ids the shop now holds. A till drops only these, so a reply that
    /// went missing leaves the rest to be sent again.
    pub stored: Vec<u128>,
}

/// Ask who the shop lets buy on account.
///
/// A till's route as well as the back office's, like the people who may sign
/// in: a sale on account is written with the internet down, so the names have
/// to be on the device before they are wanted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomersRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomersResponse {
    pub protocol: u16,
    pub customers: Vec<CustomerWire>,
}

/// Ask what each of them owes.
///
/// Its own route rather than a field on the customer list, because the two move
/// at different speeds: a name is written down once and a balance changes every
/// time somebody takes a bag of rice. A till asking for names every few minutes
/// to learn a number would be asking the shop to send the same list all day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancesRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

/// What one person owes, as the shop's book stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceWire {
    pub customer: u128,
    /// Positive is owed to the shop.
    pub owed_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancesResponse {
    pub protocol: u16,
    /// Only the people who owe something. A shop with two hundred names on the
    /// list and four of them owing sends four numbers.
    pub balances: Vec<BalanceWire>,
}

/// Add or correct somebody who buys on account. Owner only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCustomerRequest {
    pub protocol: u16,
    pub customer: CustomerWire,
}

/// What a till has open right now.
///
/// Sent while a drawer is open rather than only when it closes, which was the
/// only moment the shop ever heard about one. A till left open overnight and
/// wiped in the morning took its whole takings summary with it, and nobody
/// could ask which tills still had a drawer open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDrawerRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub shift: u128,
    pub opened_at_ms: u64,
    /// The till's clock when it said this.
    pub at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDrawerResponse {
    pub protocol: u16,
}

/// Ask which tills have a drawer open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawersRequest {
    pub protocol: u16,
}

/// A drawer somebody has open, as that till last said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawerWire {
    pub terminal: u128,
    pub shift: u128,
    pub opened_at_ms: u64,
    /// When the till last said this. Four hours ago and four minutes ago are
    /// different things and only the shop can say which matters.
    pub reported_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawersResponse {
    pub protocol: u16,
    pub drawers: Vec<OpenDrawerWire>,
}

/// Ask for the drawers a shop has closed lately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsRequest {
    pub protocol: u16,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftsResponse {
    pub protocol: u16,
    pub shifts: Vec<ClosedShiftWire>,
}

// ---------------------------------------------------------------------------
// What people owe the shop
// ---------------------------------------------------------------------------

/// Ask who owes the shop money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedRequest {
    pub protocol: u16,
    pub limit: u32,
    /// Where the last page ended, so the next one carries on from it: what that
    /// person owed and their key. A shop that lets three hundred people buy on
    /// account had the rest of the list quietly cut off before this existed.
    ///
    /// Zero and an empty key mean the beginning, which is what a screen opening
    /// the list sends. Not an offset: the list is ordered by what is owed, and
    /// a payment taken between two pages would make an offset skip somebody.
    pub after_owed_minor: i64,
    pub after_person_key: String,
}

/// What one person owes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwingWire {
    /// The folded name, which is what a payment is recorded against. Sent so a
    /// screen names the same person the server did, rather than folding a
    /// display name again and hoping the two agree.
    pub person_key: String,
    pub person_name: String,
    /// Positive is owed to the shop. Negative means they are in credit, which
    /// is worth showing rather than hiding.
    pub owed_minor: i64,
    /// When the oldest entry still in this balance was made.
    pub since_ms: u64,
    pub last_at_ms: u64,
    pub entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedResponse {
    pub protocol: u16,
    pub owing: Vec<OwingWire>,
}

/// Take money off what somebody owes, or strike a debt off without money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakePaymentRequest {
    pub protocol: u16,
    /// Minted by whoever took the payment, so a resent one is not counted
    /// twice. A payment counted twice is money the shop believes it has been
    /// given and has not.
    pub id: u128,
    pub person_key: String,
    /// What to call them if this is the first entry under that key.
    pub person_name: String,
    /// What was handed over, or what is being struck off. Positive either way.
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: Option<String>,
    /// True when no money changed hands: a sale rung twice by a till restored
    /// from a backup, goods brought back, an argument settled. It needs a note,
    /// and it is never added in with money the shop was actually given.
    pub written_off: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakePaymentResponse {
    pub protocol: u16,
    /// False when this payment was already recorded, which is ordinary: a
    /// dropped reply is the usual reason one is sent twice.
    pub taken: bool,
    /// What they owe now, so a screen shows the truth rather than its own
    /// arithmetic.
    pub owed_minor: i64,
}

/// Ask what makes up one person's balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRequest {
    pub protocol: u16,
    pub person_key: String,
    pub limit: u32,
    /// Where the last page ended: when that entry was and what made it. Zero
    /// and zero mean the beginning. A year of a family's shopping is more than
    /// two hundred lines, and the older ones are exactly what somebody
    /// disputing a balance wants to see.
    pub after_at_ms: u64,
    pub after_source_id: u128,
}

/// One line of somebody's account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntryWire {
    /// The sale that created the debt, or the payment that reduced it.
    pub source_id: u128,
    pub is_sale: bool,
    /// True when it came off the account without money changing hands.
    pub written_off: bool,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
    /// The receipt this debt was rung on, and empty for a payment, a write-off,
    /// or a sale from before a device printed numbers.
    ///
    /// Without it a line says only a day and an amount, and two sales of the
    /// same size on one day are two lines nobody can tell apart. That is the
    /// line a customer disputes, standing at the counter saying they took goods
    /// once: the shop can point at the paper or it cannot, and the paper is the
    /// only thing both of them are holding. Appended, never inserted.
    #[serde(default)]
    pub receipt_no: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountResponse {
    pub protocol: u16,
    pub entries: Vec<AccountEntryWire>,
}

/// An account as versions up to 11 sent it, before a line said which receipt it
/// was rung on. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntryWireV11 {
    pub source_id: u128,
    pub is_sale: bool,
    pub written_off: bool,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountResponseV11 {
    pub protocol: u16,
    pub entries: Vec<AccountEntryWireV11>,
}

impl From<AccountResponse> for AccountResponseV11 {
    fn from(new: AccountResponse) -> Self {
        Self {
            protocol: new.protocol,
            entries: new
                .entries
                .into_iter()
                .map(|one| AccountEntryWireV11 {
                    source_id: one.source_id,
                    is_sale: one.is_sale,
                    written_off: one.written_off,
                    amount_minor: one.amount_minor,
                    at_ms: one.at_ms,
                    note: one.note,
                })
                .collect(),
        }
    }
}

/// What one till took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillTakings {
    pub terminal: u128,
    pub sales: u64,
    pub total_minor: i64,
    /// Sales in this period the server has quarantined. Carried per till,
    /// because one till producing all of them is a different problem from every
    /// till producing one.
    pub needing_attention: u64,
}

/// Ask what has been delivered lately.
///
/// Newest first and capped, because the question a shop asks is "what came in
/// this week" rather than "everything since we opened", and the answer to the
/// second would be a page nobody can read on a tablet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// One line of a delivery, as it comes back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredLineWire {
    pub item_id: u128,
    pub qty_milli: i64,
    pub unit_cost_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryWire {
    pub id: u128,
    pub supplier_id: Option<u128>,
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub lines: Vec<DeliveredLineWire>,
    /// What the whole delivery cost, added up where the rest of this shop's
    /// money is added up. Appended, never inserted. The back office screen used
    /// to multiply the quantities by the unit costs itself, which put one of
    /// the shop's figures in a language whose only number is a float.
    ///
    /// Absent when the lines cannot be added up in the money this build uses,
    /// which takes figures no shop has. A screen says so rather than showing a
    /// zero, because a delivery worth nothing and a delivery nobody could add
    /// up are different things and only one of them is worth a phone call.
    pub cost_minor: Option<i64>,
}

/// A delivery as versions up to 7 sent one, before it carried its own total.
///
/// Frozen because these bodies are positional: a reader on the older shape
/// would take the total as the start of the next delivery and answer the shop
/// with rubbish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryWireV7 {
    pub id: u128,
    pub supplier_id: Option<u128>,
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub lines: Vec<DeliveredLineWireV7>,
}

impl From<DeliveryWire> for DeliveryWireV7 {
    fn from(new: DeliveryWire) -> Self {
        Self {
            id: new.id,
            supplier_id: new.supplier_id,
            reference: new.reference,
            received_at_ms: new.received_at_ms,
            lines: new
                .lines
                .into_iter()
                .map(|line| DeliveredLineWireV7 {
                    item_id: line.item_id,
                    qty_milli: line.qty_milli,
                    unit_cost_minor: line.unit_cost_minor,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesResponse {
    pub protocol: u16,
    pub deliveries: Vec<DeliveryWire>,
}

/// What versions up to 7 were answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesResponseV7 {
    pub protocol: u16,
    pub deliveries: Vec<DeliveryWireV7>,
}

/// Ask what a set of items is believed to hold.
///
/// A separate question from the catalogue, because a sale is not a catalogue
/// change: the figure on an item record is whatever it was when somebody last
/// edited that item, and a screen that shows it as stock shows a number that
/// never moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandRequest {
    pub protocol: u16,
    /// Empty means everything the shop sells. A shop with ten thousand lines
    /// asks for the page it is looking at instead.
    pub item_ids: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandResponse {
    pub protocol: u16,
    pub on_hand: Vec<OnHandEntry>,
    /// Whether this is every item the shop sells, or as many as the server will
    /// answer for in one breath.
    ///
    /// The question is asked both ways: for the page on a screen, where the
    /// answer is obviously partial, and for the whole shelf, where a figure
    /// added up from part of it reads as a figure for all of it. A shop told it
    /// has twelve thousand taka sitting in stock that has not moved, when the
    /// count looked at two hundred of its eight hundred items, has been told
    /// something untrue. Appended, never inserted.
    #[serde(default)]
    pub whole: bool,
}

/// What one item is now believed to hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHandEntry {
    pub item_id: u128,
    pub qty_milli: i64,
    /// When it was last counted, if ever. Absent means the figure is a running
    /// total resting on no count, which a shop should be told.
    pub counted_at_ms: Option<u64>,
    /// Sales rung before the last count but which reached the server after it.
    /// Not included in `qty_milli`, because nobody can say whether the person
    /// counting saw those goods.
    pub unreconciled_milli: i64,
    pub unreconciled_sales: u32,
}

/// Stock leaving or entering for a reason that is neither a sale nor a
/// delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectStockRequest {
    pub protocol: u16,
    pub id: u128,
    pub item_id: u128,
    /// Signed: negative for goods gone, positive for a count that was under.
    pub qty_milli: i64,
    /// Why. Required, and refused when blank: an unexplained correction is
    /// indistinguishable from theft when the variance is read a month later.
    pub reason: String,
    pub occurred_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectStockResponse {
    pub protocol: u16,
    /// False when this correction was already recorded. A retry after a dropped
    /// reply is normal and is not an error.
    pub recorded: bool,
    pub on_hand: Option<OnHandEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCountResponse {
    pub protocol: u16,
    /// The figure for each counted item after the count, so the device shows
    /// what the server concluded rather than what it asserted.
    pub on_hand: Vec<OnHandEntry>,
}

// ---------------------------------------------------------------------------
// Renewal: a credential that would otherwise run out
// ---------------------------------------------------------------------------

/// Ask for a fresh credential, authenticated with the current one.
///
/// Carries no identity, like every other authenticated request: the server takes
/// the tenant and terminal from the token presented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewRequest {
    pub protocol: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewResponse {
    pub protocol: u16,
    /// Shown once. The device must store it before acting on this reply.
    pub token: String,
    /// Seconds until the new credential expires, so a till can decide when to
    /// ask again rather than each build hard-coding the server's policy.
    pub expires_in_seconds: u64,
    /// Seconds the old credential keeps working.
    ///
    /// Not zero, and that is the point: if this reply is lost, the device still
    /// holds only the old token, and revoking it immediately would strand a till
    /// with no way to authenticate and no way to ask again.
    pub previous_valid_for_seconds: u64,
}

// ---------------------------------------------------------------------------
// Lease: receipt numbers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// How many numbers the till wants. The server may grant fewer.
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseResponse {
    pub protocol: u16,
    /// Bumped when the server believes this terminal was replaced or restored,
    /// so numbers issued under the old epoch remain attributable.
    pub epoch: u64,
    pub prefix: String,
    pub first: u64,
    pub last: u64,
}

// ---------------------------------------------------------------------------
// Back office: the shop owner rather than the till
// ---------------------------------------------------------------------------
//
// These carry a tenant and a terminal like every other authenticated request,
// and for the same reason: the server compares both against the credential and
// refuses a mismatch, so a console configured against the wrong shop is told so
// instead of being quietly served somebody else's numbers.
//
// Times are milliseconds since the Unix epoch, matching `rung_at_ms` on a sale.
// Deliberately not a formatted string: the back office renders in the shop's own
// locale, and a server that pre-formats forces its own.

/// Ask for the sales that need a human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairQueueRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    /// How many entries to return. The server clamps this: a shop with a broken
    /// till can accumulate thousands, and a page nobody can download is a queue
    /// nobody can work through.
    pub limit: u32,
}

/// One sale waiting on a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairEntry {
    pub id: u128,
    /// Absent when the till sold without a leased block, or when the payload
    /// could not be decoded far enough to find one.
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    /// When the server received it, not when it was rung up. The gap between the
    /// two is how long the till was offline, which is usually the story.
    pub received_at_ms: u64,
    /// Prose, written for the person deciding what to do about the sale. What
    /// the server decided at the moment it held the sale, and what a screen
    /// falls back to.
    pub reason: String,
    /// The reason itself, so a screen can say it in the shop's own language
    /// rather than matching on the sentence above.
    ///
    /// Appended, and empty for a sale held before the shop stored it: those can
    /// only ever be shown as the words. Postcard is positional, so this goes at
    /// the end and an older back office reading a newer shop simply stops
    /// before it.
    #[serde(default)]
    pub held_for: Option<QuarantineReason>,
}

/// Ask what the shop made over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MadeRequest {
    pub protocol: u16,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// Turnover before tax, what the goods cost, and the difference.
///
/// With the part the shop cannot answer for kept separate rather than folded
/// in: a shop that has never entered what it pays for anything would otherwise
/// read a margin equal to its whole turnover and believe it for a week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MadeResponse {
    pub protocol: u16,
    pub net_minor: i64,
    pub cost_minor: i64,
    pub made_minor: i64,
    pub sales: u64,
    pub sales_without_cost: u64,
    pub net_without_cost_minor: i64,
}

/// Ask what was on a receipt.
///
/// The question a shop is asked across the counter: somebody comes back with a
/// piece of paper and says they were charged twice, or for something they did
/// not take. Until this existed the shop held every one of those sales and had
/// no way to look one up: the repair queue answers "which sales went wrong",
/// the day answers "what did we take", and neither answers "what was on this".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRequest {
    pub protocol: u16,
    /// As printed, including the terminal's prefix.
    pub receipt_no: String,
}

/// One line as the customer's paper shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperLineWire {
    /// Which item this was, so a refund can put the same goods back on the
    /// same shelf and charge back what was charged.
    ///
    /// A refund at a counter used to be rung by scanning the goods again, which
    /// prices them from today's catalogue: a basket sold with ten percent off
    /// the ticket came back at full price and the shop gave the discount away a
    /// second time. What the paper says is what they paid, and this is what
    /// says which shelf it came off.
    ///
    /// First rather than last, and the comment here said "appended, never
    /// inserted" for as long as it has existed, which is not what the code
    /// does and is how the rest of this went unnoticed. Where it sits does not
    /// matter between two builds of the same release, which is why nothing
    /// caught it; it matters entirely to a shape frozen to read what an older
    /// build sends, and `SaleOnPaperWireV2` named this type and was silently
    /// given the new field by the same commit that correctly froze
    /// `PaperLineWireV6` for `SaleOnPaperWireV6`. Left where it is, because
    /// moving it now would be a protocol change that buys nothing; what is
    /// fixed is the frozen shape and this sentence.
    #[serde(default)]
    pub item_id: u128,
    pub name: String,
    pub qty_milli: i64,
    pub unit: String,
    pub unit_price_minor: i64,
    /// What came off this line, as money, whatever it was expressed as.
    pub discount_minor: i64,
    pub vat_bp: u32,
    pub line_total_minor: i64,
    /// The taxable amount of this line after its discount, and the tax on it,
    /// both worked out by the crate that priced the sale. Appended.
    ///
    /// A tax invoice has a column for each, and a screen that recovered them
    /// from the total would be doing tax arithmetic: where a shop prices
    /// inclusive of tax, neither is a rate away from what the customer paid.
    /// The shop already computes both to answer the lookup at all, so this is
    /// carrying what it has rather than working anything out.
    #[serde(default)]
    pub net_minor: i64,
    #[serde(default)]
    pub vat_minor: i64,
    /// Standard, zero rated or exempt, as the line said on the day. The invoice
    /// and the return put the last two in different places, and a rate of zero
    /// does not tell them apart.
    #[serde(default)]
    pub supply: u8,
    /// What one unit of this line came to, tax and all. Appended.
    ///
    /// The একক মূল্য column of form মূসক-৬.৭, the credit note, by that form's own
    /// footnote: the price of one unit including VAT and supplementary duty.
    /// Form মূসক-৬.৩ asks for the opposite figure under a column of the same
    /// name, so the two documents need two figures and neither of them is
    /// `unit_price_minor`, which is what the catalogue held before any discount.
    ///
    /// Carried rather than divided out by a screen: it is a division, it rounds,
    /// and the shop already prices the whole ticket to answer this lookup. The
    /// back office printed 0.00 in that column for an afternoon because the
    /// document was written against the till's own view, where the figure
    /// exists, and handed a looked-up sale, where it did not.
    #[serde(default)]
    pub unit_with_tax_minor: i64,
}

/// One payment as the paper shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperTenderWire {
    /// Cash, a named wallet, a card, an account. In words, because the screen
    /// showing this is showing it to a person, and a wallet's name is the
    /// shop's own word rather than anything to translate.
    pub kind: String,
    /// Which of the three every shop has, for a screen saying it in the shop's
    /// language: `cash`, `card`, `credit`, or `wallet` for one the shop named.
    /// Empty from a server that predates this.
    #[serde(default)]
    pub kind_code: String,
    pub amount_minor: i64,
    /// A wallet transaction id or a card approval code, when there was one.
    pub reference: Option<String>,
}

/// A sale as the shop holds it, read out of the bytes the till committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWire {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLineWire>,
    pub tenders: Vec<PaperTenderWire>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    /// What was waived on this ticket and by whom, in the words the till wrote
    /// on the customer's copy.
    pub overrides: Vec<String>,
    /// Empty when the shop took the sale without question. Otherwise what it
    /// was held for, in the words the repair queue uses.
    pub held_for: String,
    /// The same thing as the reason itself, for a screen saying it in the
    /// shop's own language. Absent for a sale the shop took, and for one held
    /// before the shop stored the reason beside the words.
    #[serde(default)]
    pub held_for_kind: Option<QuarantineReason>,
    /// Set once somebody has decided about a held sale: what they said, and
    /// whether the sale still counts.
    pub decided: Option<String>,
    pub still_counts: bool,
    /// What has been given back against this receipt, as a positive amount.
    pub refunded_minor: i64,
    /// For a refund, the receipt it reverses.
    pub refund_of: Option<String>,
    /// Who was standing at the till when it was rung, as the shop calls them
    /// now. Appended.
    ///
    /// The name rather than the id, because the only thing on the other end of
    /// this is a person reading a screen, and a back office has no list of who
    /// was at a till last March to look an id up in.
    ///
    /// Absent for a sale rung before a till recorded it, for one rung with
    /// nobody signed in, and for one whose operator has since been removed from
    /// the shop's list. All three are the same answer to the person asking:
    /// nobody can say. Saying nothing is better than naming whoever holds that
    /// id today.
    #[serde(default)]
    pub served_by: Option<String>,
    /// Who bought it, when the sale names somebody the shop wrote down, and
    /// what a tax invoice has to say about them. Appended.
    ///
    /// Three fields rather than an id, because the only thing on the other end
    /// of this is a document being laid out, and looking a person up again from
    /// a screen is a second round trip for a name the shop has already read.
    /// Resolved when the lookup runs, so somebody renamed reads as they are
    /// called now, which is the rule the operator's name follows above.
    ///
    /// A shop cannot print the invoice for a sale without these: the buyer's
    /// name, address and BIN are what section 51(1)(c) asks for, and a customer
    /// who comes back next week for their copy is the ordinary case for that
    /// document.
    #[serde(default)]
    pub buyer_name: Option<String>,
    #[serde(default)]
    pub buyer_bin: Option<String>,
    #[serde(default)]
    pub buyer_address: Option<String>,
}

/// Why a sale was held, as versions 6 to 17 sent it.
///
/// A copy rather than a pointer at the live enum, for the reason every frozen
/// shape here is a copy: these encode positionally by variant order, and a
/// variant appended to the live one would change what an older body's bytes
/// claim to be. The live enum has grown four variants since version 6 and will
/// grow more; this is what it was when version 17 was the newest thing anybody
/// spoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineReasonV17 {
    TotalsMismatch {
        stored_minor: i64,
        recomputed_minor: i64,
    },
    DuplicateReceiptNumber {
        receipt_no: String,
    },
    Undecodable,
    CarriedIn,
    ClockOutOfRange {
        rung_at_ms: u64,
        received_at_ms: u64,
    },
    RefundAgainstNothing {
        receipt_no: String,
    },
    RefundBeyondTheSale {
        receipt_no: String,
        sale_minor: i64,
        refunded_minor: i64,
    },
    MoreCameBackThanWentOut {
        receipt_no: String,
        item_id: u128,
        over_by_milli: i64,
    },
    TendersDoNotAddUp {
        total_minor: i64,
        tendered_minor: i64,
        change_minor: i64,
    },
}

impl From<QuarantineReason> for QuarantineReasonV17 {
    fn from(reason: QuarantineReason) -> Self {
        // Exhaustive with no `..`, so a tenth reason stops this compiling until
        // somebody says what a build speaking 17 is told about it.
        match reason {
            QuarantineReason::TotalsMismatch {
                stored_minor,
                recomputed_minor,
            } => Self::TotalsMismatch {
                stored_minor,
                recomputed_minor,
            },
            QuarantineReason::DuplicateReceiptNumber { receipt_no } => {
                Self::DuplicateReceiptNumber { receipt_no }
            }
            QuarantineReason::Undecodable => Self::Undecodable,
            QuarantineReason::CarriedIn => Self::CarriedIn,
            QuarantineReason::ClockOutOfRange {
                rung_at_ms,
                received_at_ms,
            } => Self::ClockOutOfRange {
                rung_at_ms,
                received_at_ms,
            },
            QuarantineReason::RefundAgainstNothing { receipt_no } => {
                Self::RefundAgainstNothing { receipt_no }
            }
            QuarantineReason::RefundBeyondTheSale {
                receipt_no,
                sale_minor,
                refunded_minor,
            } => Self::RefundBeyondTheSale {
                receipt_no,
                sale_minor,
                refunded_minor,
            },
            QuarantineReason::MoreCameBackThanWentOut {
                receipt_no,
                item_id,
                over_by_milli,
            } => Self::MoreCameBackThanWentOut {
                receipt_no,
                item_id,
                over_by_milli,
            },
            QuarantineReason::TendersDoNotAddUp {
                total_minor,
                tendered_minor,
                change_minor,
            } => Self::TendersDoNotAddUp {
                total_minor,
                tendered_minor,
                change_minor,
            },
        }
    }
}

/// One line as versions 14 to 17 sent one.
///
/// Its own copy rather than a pointer at the live shape, which has now been
/// given a field twice: `PaperLineWireV13` exists because of the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperLineWireV17 {
    pub item_id: u128,
    pub name: String,
    pub qty_milli: i64,
    pub unit: String,
    pub unit_price_minor: i64,
    pub discount_minor: i64,
    pub vat_bp: u32,
    pub line_total_minor: i64,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub supply: u8,
}

impl From<PaperLineWire> for PaperLineWireV17 {
    fn from(new: PaperLineWire) -> Self {
        Self {
            item_id: new.item_id,
            name: new.name,
            qty_milli: new.qty_milli,
            unit: new.unit,
            unit_price_minor: new.unit_price_minor,
            discount_minor: new.discount_minor,
            vat_bp: new.vat_bp,
            line_total_minor: new.line_total_minor,
            net_minor: new.net_minor,
            vat_minor: new.vat_minor,
            supply: new.supply,
            // What one unit came to with the tax in it is dropped rather than
            // carried: these bodies are positional, and a back office that
            // predates the field would read it as the start of the next line.
        }
    }
}

/// A sale as versions 16 and 17 sent one: with who bought it, without what one
/// unit of a line came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWireV17 {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLineWireV17>,
    pub tenders: Vec<PaperTenderWireV6>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    pub held_for: String,
    pub held_for_kind: Option<QuarantineReasonV17>,
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
    pub served_by: Option<String>,
    pub buyer_name: Option<String>,
    pub buyer_bin: Option<String>,
    pub buyer_address: Option<String>,
}

impl From<SaleOnPaperWire> for SaleOnPaperWireV17 {
    fn from(new: SaleOnPaperWire) -> Self {
        Self {
            id: new.id,
            terminal: new.terminal,
            receipt_no: new.receipt_no,
            rung_at_ms: new.rung_at_ms,
            lines: new.lines.into_iter().map(Into::into).collect(),
            // The tender shape has not changed since version 6, so the copy
            // frozen then is still what this one carries.
            tenders: new
                .tenders
                .into_iter()
                .map(|tender| PaperTenderWireV6 {
                    kind: tender.kind,
                    kind_code: tender.kind_code,
                    amount_minor: tender.amount_minor,
                    reference: tender.reference,
                })
                .collect(),
            net_minor: new.net_minor,
            vat_minor: new.vat_minor,
            discount_minor: new.discount_minor,
            total_minor: new.total_minor,
            change_minor: new.change_minor,
            overrides: new.overrides,
            held_for: new.held_for,
            held_for_kind: new.held_for_kind.map(Into::into),
            decided: new.decided,
            still_counts: new.still_counts,
            refunded_minor: new.refunded_minor,
            refund_of: new.refund_of,
            served_by: new.served_by,
            buyer_name: new.buyer_name,
            buyer_bin: new.buyer_bin,
            buyer_address: new.buyer_address,
        }
    }
}

/// What versions 16 and 17 were answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponseV17 {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWireV17>,
}

/// What the shop holds under one receipt number.
///
/// A list rather than one, because two sales carrying one number is exactly the
/// thing a shop asks about: it is what the repair queue holds them for, and the
/// person at the counter is owed both of them rather than whichever came first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponse {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWire>,
}

/// A sale looked up by its receipt, as versions 1 and 2 sent one.
///
/// The same reason the repair queue keeps its older shape: these bodies are
/// positional, and a back office a release behind would read a field it does
/// not know as the start of the next one. What it loses is the ability to say
/// why a sale is held in its own language, which is a thing it could not do
/// anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponseV2 {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWireV2>,
}

/// One sale as versions 1 and 2 sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWireV2 {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    // The line as it was, not as it is. This said `PaperLineWire`, which gained
    // an item id at the front of it seventy four commits after this shape was
    // frozen, so a back office speaking 2 was being sent lines beginning with a
    // number it does not expect and reading every field of every line as the
    // one before it. `PaperLineWireV6` is that same line before the id, which
    // is what protocol 2 had: the type did not change between the two.
    pub lines: Vec<PaperLineWireV6>,
    pub tenders: Vec<PaperTenderWireV2>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    pub held_for: String,
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
}

/// A line as versions up to 6 sent one, before it said which item it was.
///
/// Frozen because these bodies are positional: a reader on the older shape
/// would take the id as the length of the name and answer a customer holding a
/// receipt with nonsense.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperLineWireV6 {
    pub name: String,
    pub qty_milli: i64,
    pub unit: String,
    pub unit_price_minor: i64,
    pub discount_minor: i64,
    pub vat_bp: u32,
    pub line_total_minor: i64,
}

impl From<PaperLineWire> for PaperLineWireV6 {
    fn from(new: PaperLineWire) -> Self {
        Self {
            name: new.name,
            qty_milli: new.qty_milli,
            unit: new.unit,
            unit_price_minor: new.unit_price_minor,
            discount_minor: new.discount_minor,
            vat_bp: new.vat_bp,
            line_total_minor: new.line_total_minor,
        }
    }
}

/// A sale as versions 3 to 6 sent one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWireV6 {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLineWireV6>,
    pub tenders: Vec<PaperTenderWireV6>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    pub held_for: String,
    pub held_for_kind: Option<QuarantineReasonV6>,
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
}

impl From<SaleOnPaperWire> for SaleOnPaperWireV6 {
    fn from(new: SaleOnPaperWire) -> Self {
        Self {
            id: new.id,
            terminal: new.terminal,
            receipt_no: new.receipt_no,
            rung_at_ms: new.rung_at_ms,
            lines: new.lines.into_iter().map(Into::into).collect(),
            tenders: new
                .tenders
                .into_iter()
                .map(|tender| PaperTenderWireV6 {
                    kind: tender.kind,
                    kind_code: tender.kind_code,
                    amount_minor: tender.amount_minor,
                    reference: tender.reference,
                })
                .collect(),
            net_minor: new.net_minor,
            vat_minor: new.vat_minor,
            discount_minor: new.discount_minor,
            total_minor: new.total_minor,
            change_minor: new.change_minor,
            overrides: new.overrides,
            held_for: new.held_for,
            // The reason as version 6 knew them, and nothing for one invented
            // since: the sentence above carries it either way, which is what
            // this product already does for a sale held before the reason
            // itself was kept.
            held_for_kind: new.held_for_kind.and_then(as_version_six_knew_it),
            decided: new.decided,
            still_counts: new.still_counts,
            refunded_minor: new.refunded_minor,
            refund_of: new.refund_of,
        }
    }
}

/// A sale as versions 14 and 15 sent one: with who rang it, without who bought.
///
/// Its lines and tenders are the copies frozen for the versions before it,
/// which have not changed since: a frozen shape may not name one that is still
/// growing, and this file has the scar to show for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWireV15 {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLineWireV13>,
    pub tenders: Vec<PaperTenderWireV6>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    pub held_for: String,
    pub held_for_kind: Option<QuarantineReasonV6>,
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
    pub served_by: Option<String>,
}

impl From<SaleOnPaperWire> for SaleOnPaperWireV15 {
    fn from(new: SaleOnPaperWire) -> Self {
        Self {
            id: new.id,
            terminal: new.terminal,
            receipt_no: new.receipt_no,
            rung_at_ms: new.rung_at_ms,
            lines: new.lines.into_iter().map(Into::into).collect(),
            tenders: new
                .tenders
                .into_iter()
                .map(|tender| PaperTenderWireV6 {
                    kind: tender.kind,
                    kind_code: tender.kind_code,
                    amount_minor: tender.amount_minor,
                    reference: tender.reference,
                })
                .collect(),
            net_minor: new.net_minor,
            vat_minor: new.vat_minor,
            discount_minor: new.discount_minor,
            total_minor: new.total_minor,
            change_minor: new.change_minor,
            overrides: new.overrides,
            held_for: new.held_for,
            held_for_kind: new.held_for_kind.and_then(as_version_six_knew_it),
            decided: new.decided,
            still_counts: new.still_counts,
            refunded_minor: new.refunded_minor,
            refund_of: new.refund_of,
            served_by: new.served_by,
            // Who bought it is dropped rather than carried: these bodies are
            // positional and a back office that predates the field would read
            // the name as the start of the next sale in the list.
        }
    }
}

/// What versions 14 and 15 were answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponseV15 {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWireV15>,
}

/// One line as versions 7 to 13 sent one.
///
/// Its own copy rather than a pointer at the live shape. The live one has been
/// given a field at the front once already, and the frozen shape that named it
/// went on reading a name where an id had appeared for seventy-four commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperLineWireV13 {
    pub item_id: u128,
    pub name: String,
    pub qty_milli: i64,
    pub unit: String,
    pub unit_price_minor: i64,
    pub discount_minor: i64,
    pub vat_bp: u32,
    pub line_total_minor: i64,
}

impl From<PaperLineWire> for PaperLineWireV13 {
    fn from(new: PaperLineWire) -> Self {
        Self {
            item_id: new.item_id,
            name: new.name,
            qty_milli: new.qty_milli,
            unit: new.unit,
            unit_price_minor: new.unit_price_minor,
            discount_minor: new.discount_minor,
            vat_bp: new.vat_bp,
            line_total_minor: new.line_total_minor,
        }
    }
}

/// A sale as versions 7 to 13 sent one: everything but who rang it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaperWireV13 {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLineWireV13>,
    pub tenders: Vec<PaperTenderWireV6>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    pub held_for: String,
    pub held_for_kind: Option<QuarantineReasonV6>,
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
}

impl From<SaleOnPaperWire> for SaleOnPaperWireV13 {
    fn from(new: SaleOnPaperWire) -> Self {
        Self {
            id: new.id,
            terminal: new.terminal,
            receipt_no: new.receipt_no,
            rung_at_ms: new.rung_at_ms,
            lines: new.lines.into_iter().map(Into::into).collect(),
            tenders: new
                .tenders
                .into_iter()
                .map(|tender| PaperTenderWireV6 {
                    kind: tender.kind,
                    kind_code: tender.kind_code,
                    amount_minor: tender.amount_minor,
                    reference: tender.reference,
                })
                .collect(),
            net_minor: new.net_minor,
            vat_minor: new.vat_minor,
            discount_minor: new.discount_minor,
            total_minor: new.total_minor,
            change_minor: new.change_minor,
            overrides: new.overrides,
            held_for: new.held_for,
            held_for_kind: new.held_for_kind.and_then(as_version_six_knew_it),
            decided: new.decided,
            still_counts: new.still_counts,
            refunded_minor: new.refunded_minor,
            refund_of: new.refund_of,
            // Who rang it is dropped rather than carried: these bodies are
            // positional and a back office that predates the field would read
            // it as the start of something else.
        }
    }
}

/// What versions 7 to 13 were answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponseV13 {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWireV13>,
}

/// What versions 3 to 6 were answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptResponseV6 {
    pub protocol: u16,
    pub found: Vec<SaleOnPaperWireV6>,
}

/// One payment as versions 1 and 2 sent one: named, without which of the three
/// kinds it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperTenderWireV2 {
    pub kind: String,
    pub amount_minor: i64,
    pub reference: Option<String>,
}

/// The nested shapes a frozen body is built out of, frozen with it.
///
/// A shape kept to read what an older build sends must not be written in terms
/// of one that can still move: the day a field is added to the inner type, the
/// outer one changes with it and stops reading the bytes it was kept for. The
/// disk learned this and wrote it down in `bytes_from_before.rs`; the wire had
/// no such check until one of these was found already broken, `PaperLineWire`
/// having gained a field at the front of it long after `SaleOnPaperWireV2` was
/// frozen around it.
///
/// These three were not broken, only able to be. They are copies of what their
/// outer shapes were written against, so that adding a field to the live type
/// moves nothing that is kept to read an older body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperTenderWireV6 {
    pub kind: String,
    pub kind_code: String,
    pub amount_minor: i64,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRowWireV7 {
    pub vat_bp: u32,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub sales: u64,
    #[serde(default)]
    pub supply: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredLineWireV7 {
    pub item_id: u128,
    pub qty_milli: i64,
    pub unit_cost_minor: i64,
}

/// Why a sale was held, as versions up to 6 knew the reasons.
///
/// A copy rather than an alias, because `SaleOnPaperWireV6` is kept to answer a
/// back office that old and carries four fields after this one: a variant
/// gaining a field would move all four, silently, which is the disease the
/// frozen shapes above exist to prevent and the one an enum hides best. A
/// variant *appended* moves nothing, which is the ordinary change and is why
/// this went unnoticed.
///
/// Identical to the live enum today: every reason there predates the freeze, so
/// nothing is lost in the crossing. The conversion below is what makes the next
/// one a decision rather than an accident, because it is exhaustive and will
/// not compile once a tenth reason exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineReasonV6 {
    TotalsMismatch {
        stored_minor: i64,
        recomputed_minor: i64,
    },
    DuplicateReceiptNumber {
        receipt_no: String,
    },
    Undecodable,
    CarriedIn,
    ClockOutOfRange {
        rung_at_ms: u64,
        received_at_ms: u64,
    },
    RefundAgainstNothing {
        receipt_no: String,
    },
    RefundBeyondTheSale {
        receipt_no: String,
        sale_minor: i64,
        refunded_minor: i64,
    },
    MoreCameBackThanWentOut {
        receipt_no: String,
        item_id: u128,
        over_by_milli: i64,
    },
    TendersDoNotAddUp {
        total_minor: i64,
        tendered_minor: i64,
        change_minor: i64,
    },
}

/// What a back office speaking 6 is told a sale is held for.
///
/// `None` for a reason invented after it, and the sentence beside it carries
/// the meaning: that is what this product already does for a sale held before
/// the reason itself was kept, and what an operator reads either way. The
/// alternative, which is what happens today, is that the whole body fails to
/// decode and the shop cannot look the receipt up at all.
///
/// Exhaustive on purpose. Adding a reason stops this compiling, and whoever
/// adds it says here what an older shop sees rather than finding out from one.
#[must_use]
pub fn as_version_six_knew_it(reason: QuarantineReason) -> Option<QuarantineReasonV6> {
    Some(match reason {
        QuarantineReason::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => QuarantineReasonV6::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        },
        QuarantineReason::DuplicateReceiptNumber { receipt_no } => {
            QuarantineReasonV6::DuplicateReceiptNumber { receipt_no }
        }
        QuarantineReason::Undecodable => QuarantineReasonV6::Undecodable,
        QuarantineReason::CarriedIn => QuarantineReasonV6::CarriedIn,
        QuarantineReason::ClockOutOfRange {
            rung_at_ms,
            received_at_ms,
        } => QuarantineReasonV6::ClockOutOfRange {
            rung_at_ms,
            received_at_ms,
        },
        QuarantineReason::RefundAgainstNothing { receipt_no } => {
            QuarantineReasonV6::RefundAgainstNothing { receipt_no }
        }
        QuarantineReason::RefundBeyondTheSale {
            receipt_no,
            sale_minor,
            refunded_minor,
        } => QuarantineReasonV6::RefundBeyondTheSale {
            receipt_no,
            sale_minor,
            refunded_minor,
        },
        QuarantineReason::MoreCameBackThanWentOut {
            receipt_no,
            item_id,
            over_by_milli,
        } => QuarantineReasonV6::MoreCameBackThanWentOut {
            receipt_no,
            item_id,
            over_by_milli,
        },
        QuarantineReason::TendersDoNotAddUp {
            total_minor,
            tendered_minor,
            change_minor,
        } => QuarantineReasonV6::TendersDoNotAddUp {
            total_minor,
            tendered_minor,
            change_minor,
        },
    })
}

impl From<SaleOnPaperWire> for SaleOnPaperWireV2 {
    fn from(sale: SaleOnPaperWire) -> Self {
        Self {
            id: sale.id,
            terminal: sale.terminal,
            receipt_no: sale.receipt_no,
            rung_at_ms: sale.rung_at_ms,
            // The item id is dropped rather than sent: a back office speaking 2
            // has nowhere to put it, and what it needs is the line it knows.
            lines: sale
                .lines
                .into_iter()
                .map(|line| PaperLineWireV6 {
                    name: line.name,
                    qty_milli: line.qty_milli,
                    unit: line.unit,
                    unit_price_minor: line.unit_price_minor,
                    discount_minor: line.discount_minor,
                    vat_bp: line.vat_bp,
                    line_total_minor: line.line_total_minor,
                })
                .collect(),
            tenders: sale
                .tenders
                .into_iter()
                .map(|tender| PaperTenderWireV2 {
                    kind: tender.kind,
                    amount_minor: tender.amount_minor,
                    reference: tender.reference,
                })
                .collect(),
            net_minor: sale.net_minor,
            vat_minor: sale.vat_minor,
            discount_minor: sale.discount_minor,
            total_minor: sale.total_minor,
            change_minor: sale.change_minor,
            overrides: sale.overrides,
            held_for: sale.held_for,
            decided: sale.decided,
            still_counts: sale.still_counts,
            refunded_minor: sale.refunded_minor,
            refund_of: sale.refund_of,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairQueueResponse {
    pub protocol: u16,
    pub entries: Vec<RepairEntry>,
}

/// The queue as versions 1 and 2 sent it, before a sale said why it was held in
/// anything but prose.
///
/// A back office a release behind reads the sentence, which is what it could
/// show anyway. Sending the newer shape would not read as a missing field: it
/// would read as a decode failure, and the screen would show an error where the
/// queue should be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairQueueResponseV2 {
    pub protocol: u16,
    pub entries: Vec<RepairEntryV2>,
}

/// One held sale as versions 1 and 2 sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairEntryV2 {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    pub received_at_ms: u64,
    pub reason: String,
}

impl From<RepairEntry> for RepairEntryV2 {
    fn from(entry: RepairEntry) -> Self {
        Self {
            id: entry.id,
            receipt_no: entry.receipt_no,
            total_minor: entry.total_minor,
            received_at_ms: entry.received_at_ms,
            reason: entry.reason,
        }
    }
}

/// Mark one quarantined sale as dealt with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRepairRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sale: u128,
    /// What the shop decided, kept beside the sale. The queue is worked by a
    /// person months before anyone asks why a total was wrong, and an entry that
    /// disappears without a note leaves that question unanswerable.
    pub note: String,
    /// Whether the sale stands.
    ///
    /// False means it was not a sale: a till restored from a backup rang the
    /// same goods twice, and one of the two did not happen. Everything that
    /// counted it stops, the takings and the tax and what left the shelf and
    /// anything it put on somebody's account. Nothing is deleted: the figures
    /// filter, and the sale stays exactly as it arrived. Decided once, so a
    /// second person working the queue is told nothing moved rather than
    /// overwriting the first one's answer.
    ///
    /// True is the old behaviour and the common one: the sale is real, the note
    /// records what was checked. The back office is served by the server that
    /// answers it, so unlike the till's shapes there is no older writer of these
    /// bytes to keep working.
    pub kept: bool,
}

/// What the shop has decided lately, so a wrong answer can be found again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedRequest {
    pub protocol: u16,
    pub limit: u32,
}

/// One sale somebody has already answered about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedEntry {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    /// Why it was held, in the words the server used at the time.
    pub reason: String,
    /// What the person wrote when they decided.
    pub note: String,
    /// False means every figure is ignoring this sale.
    pub kept: bool,
    pub decided_at_ms: u64,
    /// How many times it has been answered. Two or more is a shop that changed
    /// its mind, which is shown rather than hidden.
    pub decisions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedResponse {
    pub protocol: u16,
    pub decided: Vec<DecidedEntry>,
}

/// Change an answer already given about a sale.
///
/// Its own request rather than a second resolve, because it is its own act: a
/// strike-out takes a real debt off somebody's account, and a screen that let
/// that be undone by pressing the same button twice would be a way to lose one
/// quietly. Every answer is kept; the latest is what the figures read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideAgainRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sale: u128,
    /// Why the answer is changing. Required, like the first one: this is what
    /// somebody reads when they ask why a figure moved after the month closed.
    pub note: String,
    pub kept: bool,
    /// How many answers this sale had when whoever is changing it read the
    /// list. Two owners work the same queue, and a screen loaded before the
    /// other one answered would otherwise put its stale view back as the
    /// current one.
    ///
    /// Zero means "I did not look", which is what a script or an older screen
    /// sends, and is accepted: refusing those would be refusing every caller
    /// that has no way to know better yet.
    pub expected_decisions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideAgainResponse {
    pub protocol: u16,
    /// False when nobody had decided about this sale in the first place, which
    /// means it is still in the queue and belongs there.
    pub changed: bool,
    /// True when somebody else answered between the list being read and this
    /// arriving. Nothing was changed: the screen reads again and the person
    /// decides against what is actually there.
    pub stale: bool,
}

/// The same request as a screen loaded before a resolution could say anything
/// sends it: a note and nothing else.
///
/// postcard is positional, so those bytes are a strict prefix of the current
/// shape and decode as this. The server tries the current shape first and falls
/// back to this one, reading it as "the sale stands", which is what resolving
/// used to mean. Kept rather than versioned away because the tab that sends
/// these is a back office somebody left open across an upgrade, and the answer
/// to that is not an error message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRepairRequestV1 {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub sale: u128,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRepairResponse {
    pub protocol: u16,
    /// False when the sale was already resolved, or is not in the queue at all.
    /// The two are one answer because acting on either is the same: reload the
    /// queue and look again.
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
}

/// One terminal, as support sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthEntry {
    pub terminal: u128,
    pub label: String,
    /// Bumped when the server believes the device was replaced or restored. A
    /// jump here explains duplicate receipt numbers further down the queue.
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    /// When the server last heard this terminal sync. `None` for a device that
    /// has not been heard from since the column existed, which is not the same
    /// as a device that has never synced, and is not worth pretending otherwise.
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    /// Unresolved quarantined sales from this terminal. One till producing all
    /// of them is a device fault; every till producing some is a release fault.
    pub open_repairs: u64,
    /// The highest role this device still holds a live credential for: 2 for
    /// one that is the back office as well, 1 for a till, 0 for a device the
    /// shop has withdrawn every credential from.
    ///
    /// Appended, never inserted. A screen that cannot tell which device is the
    /// back office can only ever offer it a till's code, which is how a shop
    /// that lost the tablet running its back office would find it could not get
    /// back in.
    #[serde(default)]
    pub role: u8,
    /// The build this device last said it was running, and empty when it has
    /// not said: a build too old to carry one, or a browser that refuses the
    /// service worker that knows it.
    ///
    /// A hash of everything in the copy the device keeps of itself, which is
    /// the only honest name a build has here. Appended, never inserted.
    #[serde(default)]
    pub build: String,
    /// Which counter this is in its shop: 1, 2, 3, and what every receipt it
    /// prints is prefixed with.
    ///
    /// Without it there is no way from the paper to the device. A customer
    /// rings about T95-000003 and the shop's list of its devices says "a till
    /// enrolled from the command line" and "Demo front counter": names somebody
    /// typed, none of which is the number on the receipt. That list is read at
    /// exactly the moment somebody is holding the paper.
    ///
    /// Zero for a device enrolled before the shop handed these out, which is
    /// what a shop with no number for that counter should be told rather than
    /// shown a one that was made up. Appended, never inserted.
    #[serde(default)]
    pub counter_no: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthResponse {
    pub protocol: u16,
    pub terminals: Vec<TerminalHealthEntry>,
}

/// A shop's devices as versions up to 9 listed them, before one could say which
/// build it was running. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthEntryV9 {
    pub terminal: u128,
    pub label: String,
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    pub open_repairs: u64,
    #[serde(default)]
    pub role: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthResponseV9 {
    pub protocol: u16,
    pub terminals: Vec<TerminalHealthEntryV9>,
}

impl From<TerminalHealthResponse> for TerminalHealthResponseV9 {
    fn from(new: TerminalHealthResponse) -> Self {
        Self {
            protocol: new.protocol,
            terminals: new
                .terminals
                .into_iter()
                .map(|one| TerminalHealthEntryV9 {
                    terminal: one.terminal,
                    label: one.label,
                    epoch: one.epoch,
                    enrolled_at_ms: one.enrolled_at_ms,
                    last_seen_ms: one.last_seen_ms,
                    sales: one.sales,
                    open_repairs: one.open_repairs,
                    role: one.role,
                })
                .collect(),
        }
    }
}

/// A shop's devices as version 10 listed them, before one could say which
/// counter it is. Frozen: these bodies are positional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthEntryV10 {
    pub terminal: u128,
    pub label: String,
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    pub open_repairs: u64,
    #[serde(default)]
    pub role: u8,
    #[serde(default)]
    pub build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHealthResponseV10 {
    pub protocol: u16,
    pub terminals: Vec<TerminalHealthEntryV10>,
}

impl From<TerminalHealthResponse> for TerminalHealthResponseV10 {
    fn from(new: TerminalHealthResponse) -> Self {
        Self {
            protocol: new.protocol,
            terminals: new
                .terminals
                .into_iter()
                .map(|one| TerminalHealthEntryV10 {
                    terminal: one.terminal,
                    label: one.label,
                    epoch: one.epoch,
                    enrolled_at_ms: one.enrolled_at_ms,
                    last_seen_ms: one.last_seen_ms,
                    sales: one.sales,
                    open_repairs: one.open_repairs,
                    role: one.role,
                    build: one.build,
                })
                .collect(),
        }
    }
}

/// Create or replace one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertItemRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub item: ItemWire,
    /// Where this item stood when whoever is editing it read it.
    ///
    /// The back office edits a whole item, so a save built on a stale copy
    /// carries every field back, including the ones somebody else has just
    /// changed: a price corrected on one device and an item withdrawn on
    /// another, and the withdrawal is undone by the price. Sending what was
    /// read lets the server refuse rather than silently pick the older answer.
    ///
    /// Zero means "I did not look", which is what a script or an older screen
    /// sends, and is accepted: refusing those would be refusing every caller
    /// that has no way to know better yet.
    pub expected_seq: u64,
}

/// Read one item as the shop holds it now.
///
/// What somebody about to edit an item should be looking at. A device's own copy
/// of the catalogue is up to half a minute behind, and a whole-item save built
/// on it carries every stale field back with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemNowRequest {
    pub protocol: u16,
    pub item_id: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemNowResponse {
    pub protocol: u16,
    /// Absent when the shop has withdrawn it, which is not the same as never
    /// having had it and is worth telling apart on the screen.
    pub item: Option<ItemWire>,
    /// Where it stands. Sent back with a save so the server can refuse one
    /// built on an older copy.
    pub seq: u64,
}

/// Withdraw one item. The id travels alone: tills need a tombstone, not a copy
/// of what was deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteItemRequest {
    pub protocol: u16,
    pub tenant: u128,
    pub terminal: u128,
    pub item: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueEditResponse {
    pub protocol: u16,
    /// Sequence this edit was recorded at. A till that has already pulled past
    /// it is current; one behind it has work to do.
    pub cursor: u64,
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

    use alloc::vec;

    use super::*;

    /// A till a release behind, reading a shop reply from a server that knows
    /// about the shelf.
    ///
    /// The field is appended, which is only safe if a decoder that stops early
    /// stops rather than fails. Proved here rather than assumed: if postcard
    /// refused the trailing bytes, every till in every shop would stop learning
    /// its own name and address the day the server was upgraded, and would go on
    /// printing whatever it last heard.
    #[test]
    fn an_older_till_still_reads_a_shop_reply_that_grew_a_field() {
        /// The shape as the release before this one had it.
        #[derive(Debug, serde::Deserialize)]
        struct ShopResponseBefore {
            protocol: u16,
            name: String,
            bin: Option<String>,
            address: Option<String>,
            phone: Option<String>,
            #[serde(default)]
            wallets: Vec<String>,
        }

        let now = ShopResponse {
            protocol: PROTOCOL_VERSION,
            name: String::from("Karim General Store"),
            bin: Some(String::from("001234567-0101")),
            address: None,
            phone: None,
            wallets: vec![String::from("bKash")],
            stock_rule: 2,
            // The newest field of all, which is the one this is really about
            // today: a shop saying it works in English must not stop an older
            // till reading its own name.
            languages: vec![String::from("en")],
        };
        let bytes = postcard::to_allocvec(&now).expect("it encodes");

        let older: ShopResponseBefore =
            postcard::from_bytes(&bytes).expect("and an older till still reads it");
        assert_eq!(older.protocol, PROTOCOL_VERSION);
        assert_eq!(older.name, "Karim General Store");
        assert_eq!(older.bin.as_deref(), Some("001234567-0101"));
        assert!(older.address.is_none());
        assert!(older.phone.is_none());
        assert_eq!(older.wallets, vec![String::from("bKash")]);
    }

    #[test]
    fn accepts_the_versions_it_speaks() {
        assert_eq!(negotiate(PROTOCOL_VERSION), Ok(PROTOCOL_VERSION));
    }

    #[test]
    fn refuses_a_version_from_the_future_with_something_useful_to_say() {
        // From the future means the shop is the old one, which is what the
        // refusal says now: it used to tell whoever was standing at the device
        // to update the device, and during a rollout that is the wrong room.
        assert_eq!(
            negotiate(99),
            Err(ProtocolError::ShopNeedsUpdating {
                requested: 99,
                minimum: MINIMUM_PROTOCOL_VERSION,
                current: PROTOCOL_VERSION,
            })
        );
    }

    #[test]
    fn refuses_a_version_that_has_been_retired() {
        assert!(negotiate(0).is_err());
    }

    /// Whichever of the two is behind is the one named.
    ///
    /// The refusal used to say "it needs updating" both ways round, which is
    /// right when a shop's server is ahead of a device, the usual case because
    /// the server is what serves the app. During a rollout it is the other way
    /// round for a moment, and sending somebody to update the tablet in their
    /// hand when the machine in the back room is the old one sends them to the
    /// wrong room with a shop's queue waiting.
    #[test]
    fn the_one_that_is_behind_is_the_one_named() {
        assert!(matches!(
            negotiate(0),
            Err(ProtocolError::UnsupportedVersion { .. })
        ));
        assert_eq!(
            negotiate(0).unwrap_err().code(),
            "device-needs-updating",
            "a device older than the shop updates the device"
        );

        let ahead = PROTOCOL_VERSION.saturating_add(1);
        assert!(matches!(
            negotiate(ahead),
            Err(ProtocolError::ShopNeedsUpdating { .. })
        ));
        assert_eq!(
            negotiate(ahead).unwrap_err().code(),
            "shop-needs-updating",
            "a device newer than the shop updates the shop"
        );
        // And both carry the same three figures, because a screen that says
        // which to update still has to say what the two are speaking.
        let said = alloc::format!("{}", negotiate(ahead).unwrap_err());
        assert!(said.contains(&alloc::format!("{ahead}")), "{said}");
        assert!(said.contains(&alloc::format!("{PROTOCOL_VERSION}")), "{said}");
    }

    #[test]
    fn a_till_may_drop_everything_the_server_settled() {
        let response = PushResponse {
            protocol: PROTOCOL_VERSION,
            accepted: vec![1, 2],
            quarantined: vec![Quarantined {
                id: 3,
                reason: QuarantineReason::Undecodable,
            }],
        };
        // Quarantined sales are stored too. Holding them on the till would mean
        // the only copy sits on a tablet nobody has backed up.
        assert_eq!(response.settled(), vec![1, 2, 3]);
    }

    #[test]
    fn requests_and_responses_round_trip() {
        let request = PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 42,
            terminal: 7,
            sales: vec![SaleEnvelope {
                id: 900,
                schema: 1,
                payload: vec![1, 2, 3],
            }],
        };
        let bytes = postcard::to_allocvec(&request).unwrap();
        let restored: PushRequest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored, request);
    }

    #[test]
    fn a_terminal_never_heard_from_survives_the_wire_as_absent() {
        // postcard encodes an Option as a tag, so the difference between "never
        // synced" and "synced at time zero" is preserved rather than collapsing
        // into an epoch timestamp that would read as 1970 in the back office.
        let response = TerminalHealthResponse {
            protocol: PROTOCOL_VERSION,
            terminals: vec![TerminalHealthEntry {
                terminal: 7,
                label: alloc::string::String::from("Counter"),
                epoch: 1,
                enrolled_at_ms: 1_788_600_000_000,
                last_seen_ms: None,
                sales: 0,
                open_repairs: 0,
                role: 1,
                // A device that has not said which build it is running. Empty
                // rather than absent: what a shop does about it is nothing, and
                // a screen that has to tell two kinds of silence apart is a
                // screen with a distinction nobody can act on.
                build: alloc::string::String::new(),
                counter_no: 3,
            }],
        };
        let bytes = postcard::to_allocvec(&response).unwrap();
        let restored: TerminalHealthResponse = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored, response);
        assert_eq!(restored.terminals[0].last_seen_ms, None);
    }
}
