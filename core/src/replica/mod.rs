//! The in-memory catalogue the till sells from.
//!
//! Measured in a browser engine before this was written: an in-memory lookup
//! costs 0.38 us on a 12x throttled CPU, while an IndexedDB index read costs 0.1
//! to 0.2 ms. Against a 50 ms scan-to-line budget the first is free and the
//! second is a thousand times more expensive for nothing. So the catalogue lives
//! here, in RAM, and storage exists only to survive a restart.
//!
//! Everything in this module is synchronous and allocation-free on the read path.
//! A scan must never await anything.

mod search;

use alloc::boxed::Box;
use alloc::vec::Vec;
use hashbrown::HashMap;

use crate::domain::{PriceMode, Supply, VatBase};
use crate::ids::Ulid;
use crate::money::{Bp, Milli, Minor};

pub use search::normalise;

pub type ItemId = Ulid;

/// One sellable thing, as the till needs it.
///
/// Deliberately not the back office's idea of an item: no images, no supplier
/// history, no audit trail. Those stay on the server. Twenty thousand of these
/// is a few megabytes; twenty thousand with images is gigabytes and the till
/// starts getting evicted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub code: Box<str>,
    pub name_en: Box<str>,
    pub name_bn: Box<str>,
    pub unit: Box<str>,
    pub price: Minor,
    pub cost: Minor,
    pub vat_rate: Bp,
    pub price_mode: PriceMode,
    /// Which amount VAT is charged on for this item.
    pub vat_base: VatBase,
    /// Standard rated, zero rated or exempt. The shop's classification, and the
    /// revenue's word: nothing here decides which goods are which.
    pub supply: Supply,
    /// What the shop calls this kind of thing: rice, oil, soap, whatever words
    /// the shop already uses. Empty for the ones nobody has sorted yet, which
    /// is most of them on the first day and is not a fault.
    ///
    /// The shop's own words rather than a list this project chose, because a
    /// grocer, a pharmacy and a hardware shop do not sort their shelves the
    /// same way and a fixed list would fit none of them.
    pub category: Box<str>,
    pub barcodes: Vec<Box<str>>,
    pub on_hand: Milli,
    pub active: bool,
}

/// A change pulled from the server, or made locally at the till.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemDelta {
    /// Insert or replace an item wholesale.
    Upsert(Item),
    /// The item was deleted upstream. Tombstones are carried explicitly so a
    /// deleted item disappears from the replica instead of lingering forever.
    Tombstone(ItemId),
}

/// How many items a search returns before giving up. A cashier scans the first
/// screen and refines; returning thousands of rows only costs time.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;

/// The catalogue, indexed for the three ways a till reaches an item: a scanned
/// barcode, a typed code, and a typed fragment of a name.
#[derive(Debug, Default)]
pub struct Replica {
    items: Vec<Item>,
    by_id: HashMap<ItemId, usize>,
    /// How many times the catalogue has gained or lost an item.
    ///
    /// Not a count of items: two of those can be equal across a change that
    /// swapped one item for another. The shelf is asked about by position, and
    /// removing an item moves the last one into its slot, so a catalogue that
    /// lost one item and gained another while a lap of the shelf was running is
    /// a lap that never asked about somebody's stock and cannot say it went
    /// round. A price change is not one of these: it moves nothing.
    shape_moved: u64,
    /// Stock this terminal has moved since the server last confirmed a sale.
    local_stock: HashMap<ItemId, Milli>,
    by_barcode: HashMap<Box<str>, usize>,
    by_code: HashMap<Box<str>, usize>,
    /// Sorted `(token, item index)` pairs, searched by binary search for a prefix
    /// range. A sorted vector beats a map here: it is one contiguous allocation,
    /// cache-friendly to scan, and prefix queries fall out of the ordering.
    tokens: Vec<(Box<str>, u32)>,
    /// Set when a delta batch invalidates the token index, cleared by `reindex`.
    tokens_dirty: bool,
}

impl Replica {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from a full snapshot, which is how the till boots.
    #[must_use]
    pub fn from_items(items: Vec<Item>) -> Self {
        let mut replica = Self {
            items,
            ..Self::default()
        };
        replica.rebuild_indices();
        replica
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Every item, for snapshotting back to storage.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The hot path. A scanned barcode to an item, with no I/O and no allocation.
    #[must_use]
    pub fn by_barcode(&self, barcode: &str) -> Option<&Item> {
        let index = self.by_barcode.get(barcode)?;
        self.items.get(*index)
    }

    #[must_use]
    pub fn by_code(&self, code: &str) -> Option<&Item> {
        let index = self.by_code.get(code)?;
        self.items.get(*index)
    }

    #[must_use]
    pub fn by_id(&self, id: ItemId) -> Option<&Item> {
        let index = self.by_id.get(&id)?;
        self.items.get(*index)
    }

    /// Search names and codes by prefix, in both scripts.
    ///
    /// Returns items whose name or code contains a token starting with each term
    /// in the query, so "rice min" finds "Rice Miniket". Sorted by the item's
    /// position in the catalogue so results are stable between keystrokes.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Item> {
        search::run(self, query, limit)
    }

    /// Apply a batch of deltas, then rebuild whatever the batch invalidated.
    ///
    /// Batched on purpose: sync pulls a page of changes at a time, and rebuilding
    /// the token index once per batch is far cheaper than per delta. Measured in
    /// the browser, writing a catalogue one record at a time costs 1.5 s on a
    /// desktop and an estimated 7 to 20 s on a cheap tablet, which is the stall
    /// this design exists to avoid.
    pub fn apply(&mut self, deltas: impl IntoIterator<Item = ItemDelta>) {
        let mut touched = false;
        for delta in deltas {
            match delta {
                ItemDelta::Upsert(item) => {
                    self.upsert(item);
                    touched = true;
                }
                ItemDelta::Tombstone(id) => {
                    if self.remove(id) {
                        touched = true;
                    }
                }
            }
        }
        if touched {
            self.rebuild_indices();
        }
    }

    /// How many times this catalogue has gained or lost an item.
    ///
    /// For whoever is going round the shelf: the answer only means anything if
    /// it is the same catalogue at the end of a lap as at the start, and the
    /// number of items is not enough to say that.
    #[must_use]
    pub fn shape_moved(&self) -> u64 {
        self.shape_moved
    }

    /// Adjust stock on hand, which the till does on every sale so the cashier
    /// sees a live figure without asking the server.
    ///
    /// Returns the new quantity, or `None` if the item is unknown. Stock is
    /// allowed to go negative: the server ledger is the source of truth, and a
    /// till that refuses to record reality is worse than one that shows a
    /// negative number.
    pub fn adjust_on_hand(&mut self, id: ItemId, delta: Milli) -> Option<Milli> {
        let index = *self.by_id.get(&id)?;
        let item = self.items.get_mut(index)?;
        item.on_hand = item.on_hand.checked_add(delta).ok()?;

        // Remembered so a catalogue pull does not undo it. The server's figure
        // is correct as of the last sale it has seen, which for a till with an
        // unsynced afternoon behind it is hours out of date.
        let running = self.local_stock.entry(id).or_insert(Milli::ZERO);
        *running = running.checked_add(delta).unwrap_or(*running);
        Some(item.on_hand)
    }

    /// Take the shop's own figure for what is on a shelf.
    ///
    /// The catalogue's copy of this is whatever somebody last typed on the item
    /// record, and it never moves: stock is what deliveries, sales and counts
    /// add up to, which is a different question the server answers separately.
    /// This is that answer arriving.
    ///
    /// What this terminal has sold and not yet sent is added back on top, the
    /// same way a catalogue pull is: the server's figure is correct as of the
    /// last sale it has seen, and a till with an unsent afternoon behind it
    /// would otherwise watch the shelf jump up while a cashier is looking at it.
    pub fn apply_on_hand(&mut self, figures: &[(ItemId, Milli)]) -> usize {
        let mut taken = 0_usize;
        for (id, held) in figures {
            let local = self.local_stock.get(id).copied().unwrap_or(Milli::ZERO);
            let Some(&index) = self.by_id.get(id) else {
                continue;
            };
            if let Some(item) = self.items.get_mut(index) {
                item.on_hand = held.checked_add(local).unwrap_or(*held);
                taken = taken.saturating_add(1);
            }
        }
        taken
    }

    /// Forget the local stock adjustments the server has now seen.
    ///
    /// Called when the outbox drains. Until then a pulled figure is re-adjusted
    /// by whatever this terminal has sold since, so the number on the screen
    /// does not jump backwards while the cashier is looking at it.
    pub fn settle_local_stock(&mut self) {
        self.local_stock.clear();
    }

    /// Insert or replace, keeping `by_id` true within the batch.
    ///
    /// The id index is maintained here rather than only at the end of the batch
    /// because everything else in the batch consults it. A server that sends one
    /// change per edit legitimately puts two upserts of the same new item in one
    /// page, and against a stale index the second is pushed as a second copy:
    /// the catalogue then holds the item twice, search shows it twice, and a
    /// later tombstone removes only one of them.
    ///
    /// A catalogue change says nothing about what is on a shelf, so an item
    /// this device already holds keeps the figure it has. The row carries an
    /// `on_hand`, and nothing in the shop ever puts a real number in it: the
    /// new-item form sends zero, a file import sends zero, and every other save
    /// forwards whatever the row already held. Stock is what deliveries, sales
    /// and counts add up to, which is a different question, answered
    /// separately, and restating it from the back office is a count.
    ///
    /// Taking the row's figure meant a price correction or a barcode added set
    /// that item's shelf to zero on every till in the shop until the next lap,
    /// which is up to five minutes. Under the rule that stops a sale, the item
    /// could not be sold in that window without a supervisor; under the softer
    /// one the cashier was told the shop had none of something the shelf was
    /// full of.
    fn upsert(&mut self, mut item: Item) {
        if let Some(local) = self.local_stock.get(&item.id) {
            item.on_hand = item.on_hand.checked_add(*local).unwrap_or(item.on_hand);
        }
        match self.by_id.get(&item.id) {
            Some(&index) => {
                if let Some(slot) = self.items.get_mut(index) {
                    // The name, the price and the tax are the shop's to change.
                    // What is on the shelf is not this message's to say, and
                    // what this device holds already includes whatever it has
                    // sold since the shop last told it.
                    item.on_hand = slot.on_hand;
                    *slot = item;
                }
            }
            None => {
                let id = item.id;
                self.items.push(item);
                self.by_id.insert(id, self.items.len().saturating_sub(1));
                self.shape_moved = self.shape_moved.saturating_add(1);
            }
        }
    }

    /// Remove, repairing the index of whichever item the swap moved.
    ///
    /// Without the repair, two tombstones in one page silently lose the second:
    /// removing the first swaps the last item into its slot, and the second
    /// lookup then reads an index that points past the end or at the wrong item.
    /// An item that survives its own tombstone stays sellable forever, because
    /// the cursor has moved past the only tombstone it will ever be sent.
    fn remove(&mut self, id: ItemId) -> bool {
        let Some(index) = self.by_id.remove(&id) else {
            return false;
        };
        if index >= self.items.len() {
            return false;
        }
        self.items.swap_remove(index);
        // swap_remove moved the former last item into `index`, unless the
        // removed item was itself last.
        if let Some(moved) = self.items.get(index) {
            self.by_id.insert(moved.id, index);
        }
        self.shape_moved = self.shape_moved.saturating_add(1);
        true
    }

    /// Rebuild every index from the item vector.
    ///
    /// Called after a batch rather than after each change. For 20,000 items this
    /// is a handful of milliseconds, and it removes a whole class of bug where an
    /// index disagrees with the data after a partial update.
    fn rebuild_indices(&mut self) {
        self.by_id.clear();
        self.by_barcode.clear();
        self.by_code.clear();
        self.by_id.reserve(self.items.len());
        self.by_barcode.reserve(self.items.len());
        self.by_code.reserve(self.items.len());

        for (index, item) in self.items.iter().enumerate() {
            self.by_id.insert(item.id, index);
            self.by_code.insert(item.code.clone(), index);
            for barcode in &item.barcodes {
                // Last writer wins on a duplicate barcode. The back office is
                // responsible for not issuing one; the till must not panic.
                self.by_barcode.insert(barcode.clone(), index);
            }
        }
        self.tokens_dirty = true;
        self.rebuild_tokens();
    }

    fn rebuild_tokens(&mut self) {
        self.tokens = search::build_index(&self.items);
        self.tokens_dirty = false;
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects
    )]

    use alloc::vec;

    use super::*;

    pub(crate) fn item(seed: u128, code: &str, name_en: &str, barcode: &str) -> Item {
        Item {
            id: Ulid::from_u128(seed),
            code: code.into(),
            name_en: name_en.into(),
            name_bn: "পণ্য".into(),
            unit: "Nos".into(),
            price: Minor::new(4_300),
            cost: Minor::new(3_800),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            vat_base: VatBase::Discounted,
            barcodes: vec![barcode.into()],
            on_hand: Milli::new(40_000),
            active: true,
            supply: crate::domain::Supply::Standard,
            category: "".into(),
        }
    }

    /// The shop's own figure arriving, on a till that has been selling.
    ///
    /// The catalogue's copy of stock is whatever somebody last typed on the item
    /// record and never moves, so this is the only figure worth deciding
    /// anything on. What this terminal has sold and not yet sent has to survive
    /// it: the server's answer is correct as of the last sale it has seen, and
    /// without the local part the shelf jumps back up while a cashier watches.
    #[test]
    fn a_shelf_figure_from_the_shop_keeps_what_this_till_has_sold_since() {
        let mut replica = sample();
        replica.adjust_on_hand(Ulid::from_u128(1), Milli::new(-3_000));

        let taken = replica.apply_on_hand(&[
            (Ulid::from_u128(1), Milli::new(10_000)),
            (Ulid::from_u128(2), Milli::new(0)),
            // An item this till has never heard of, which is a catalogue a
            // moment behind rather than an error.
            (Ulid::from_u128(99), Milli::new(5_000)),
        ]);

        assert_eq!(taken, 2, "the two it holds");
        assert_eq!(
            replica.by_id(Ulid::from_u128(1)).unwrap().on_hand,
            Milli::new(7_000),
            "ten from the shop, less the three sold here and not yet sent"
        );
        assert_eq!(
            replica.by_id(Ulid::from_u128(2)).unwrap().on_hand,
            Milli::ZERO,
            "and a shelf the shop says is empty is empty"
        );

        // Once the shop has the sales, its figure stands on its own.
        replica.settle_local_stock();
        replica.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(7_000))]);
        assert_eq!(
            replica.by_id(Ulid::from_u128(1)).unwrap().on_hand,
            Milli::new(7_000),
            "not seven less three again"
        );
    }

    fn sample() -> Replica {
        Replica::from_items(vec![
            item(1, "SKU001", "Rice Miniket 5kg", "8690000000012"),
            item(2, "SKU002", "Rice Nazirshail 5kg", "8690000000029"),
            item(3, "SKU003", "Soybean Oil 2L", "8690000000036"),
        ])
    }

    #[test]
    fn finds_an_item_by_barcode() {
        let replica = sample();
        let found = replica
            .by_barcode("8690000000029")
            .expect("barcode is indexed");
        assert_eq!(&*found.code, "SKU002");
        assert!(replica.by_barcode("nosuchbarcode").is_none());
    }

    #[test]
    fn finds_an_item_by_code() {
        let replica = sample();
        assert_eq!(
            replica.by_code("SKU003").map(|i| &*i.name_en),
            Some("Soybean Oil 2L")
        );
    }

    #[test]
    fn upserts_and_tombstones() {
        let mut replica = sample();
        assert_eq!(replica.len(), 3);

        let mut changed = item(2, "SKU002", "Rice Nazirshail 10kg", "8690000000029");
        changed.price = Minor::new(9_000);
        replica.apply([ItemDelta::Upsert(changed)]);
        assert_eq!(
            replica.len(),
            3,
            "an upsert replaces rather than duplicates"
        );
        assert_eq!(
            replica.by_code("SKU002").map(|i| i.price),
            Some(Minor::new(9_000))
        );

        replica.apply([ItemDelta::Tombstone(Ulid::from_u128(1))]);
        assert_eq!(replica.len(), 2);
        assert!(
            replica.by_barcode("8690000000012").is_none(),
            "tombstone clears the index"
        );
        // and the survivors are still reachable, which swap_remove could break
        assert!(replica.by_code("SKU002").is_some());
        assert!(replica.by_code("SKU003").is_some());
    }

    #[test]
    fn tracks_stock_as_items_are_sold() {
        let mut replica = sample();
        let id = Ulid::from_u128(3);
        assert_eq!(
            replica.adjust_on_hand(id, Milli::new(-3_000)),
            Some(Milli::new(37_000))
        );
        assert_eq!(
            replica.by_id(id).map(|i| i.on_hand),
            Some(Milli::new(37_000))
        );
        assert_eq!(
            replica.adjust_on_hand(Ulid::from_u128(99), Milli::ONE),
            None
        );
    }

    #[test]
    fn lets_stock_go_negative_rather_than_refusing_reality() {
        let mut replica = sample();
        let id = Ulid::from_u128(1);
        assert_eq!(
            replica.adjust_on_hand(id, Milli::new(-50_000)),
            Some(Milli::new(-10_000))
        );
    }

    #[test]
    fn every_tombstone_in_a_batch_takes_effect() {
        let mut replica = Replica::new();
        replica.apply([
            ItemDelta::Upsert(item(1, "A", "Rice", "1")),
            ItemDelta::Upsert(item(2, "B", "Dal", "2")),
            ItemDelta::Upsert(item(3, "C", "Oil", "3")),
        ]);

        // Removing the first swaps the last into its slot. Against a stale index
        // the second tombstone reads past the end and does nothing, leaving a
        // delisted item on the shelf forever: the cursor has moved past the only
        // tombstone the server will ever send for it.
        replica.apply([
            ItemDelta::Tombstone(Ulid::from_u128(1)),
            ItemDelta::Tombstone(Ulid::from_u128(3)),
        ]);

        assert_eq!(replica.len(), 1);
        assert!(
            replica.by_barcode("3").is_none(),
            "a tombstoned item is still sellable"
        );
        assert!(replica.by_barcode("2").is_some());
    }

    #[test]
    fn an_item_created_and_edited_in_one_batch_appears_once() {
        let mut replica = Replica::new();
        let mut edited = item(1, "A", "Rice", "1");
        edited.price = Minor::new(9_900);

        replica.apply([
            ItemDelta::Upsert(item(1, "A", "Rice", "1")),
            ItemDelta::Upsert(edited),
        ]);

        assert_eq!(
            replica.len(),
            1,
            "a per-change server feed must not duplicate items"
        );
        assert_eq!(
            replica.by_barcode("1").map(|found| found.price),
            Some(Minor::new(9_900)),
            "and the later edit is the one that sticks"
        );
    }

    #[test]
    fn a_price_change_lands_even_when_an_earlier_removal_moved_the_item() {
        let mut replica = Replica::new();
        replica.apply([
            ItemDelta::Upsert(item(1, "A", "Rice", "1")),
            ItemDelta::Upsert(item(2, "B", "Dal", "2")),
            ItemDelta::Upsert(item(3, "C", "Oil", "3")),
        ]);

        let mut repriced = item(3, "C", "Oil", "3");
        repriced.price = Minor::new(12_500);
        replica.apply([
            ItemDelta::Tombstone(Ulid::from_u128(1)),
            ItemDelta::Upsert(repriced),
        ]);

        assert_eq!(
            replica.by_barcode("3").map(|found| found.price),
            Some(Minor::new(12_500)),
            "a dropped price update means the till keeps charging the old price"
        );
    }

    #[test]
    fn a_pull_does_not_undo_stock_this_till_has_already_sold() {
        let mut replica = Replica::new();
        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);
        // Forty on the shelf, four sold this afternoon and not yet synced.
        replica.adjust_on_hand(Ulid::from_u128(1), Milli::new(-4_000));

        // A price change arrives. The server's stock figure is correct as of the
        // last sale it has seen, which is hours ago.
        let mut repriced = item(1, "A", "Rice", "1");
        repriced.price = Minor::new(9_900);
        replica.apply([ItemDelta::Upsert(repriced)]);

        assert_eq!(
            replica.by_barcode("1").map(|found| found.on_hand),
            Some(Milli::new(36_000)),
            "the count must not jump back up while the cashier is looking at it"
        );
    }

    #[test]
    fn once_the_server_has_the_sales_its_figure_is_taken_as_given() {
        let mut replica = Replica::new();
        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);
        replica.adjust_on_hand(Ulid::from_u128(1), Milli::new(-4_000));
        replica.settle_local_stock();

        // The shop's own answer about the shelf, arriving after those sales
        // reached it. It already has them in it.
        replica.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(100_000))]);

        assert_eq!(
            replica.by_barcode("1").map(|found| found.on_hand),
            Some(Milli::new(100_000)),
            "double-counting settled sales would be the opposite error"
        );
    }

    /// A withdrawal does not forget what this till has sold of that item.
    ///
    /// Raised as a defect in review: `remove` leaves the item's local stock
    /// behind, so an item withdrawn and put back on sale starts with the
    /// adjustment still on it. That is not a defect, and this says why. The
    /// adjustment is what this terminal has sold and not yet sent, and the
    /// shop's figure for that item still excludes those sales: the id is the
    /// same item throughout, a withdrawal is a shop deciding not to sell
    /// something rather than the item's history being deleted, and the stock
    /// ledger keeps every movement against that id. Dropping the adjustment
    /// would make the shelf read high by exactly what this till sold in the
    /// outage, which is the error the adjustment exists to prevent.
    ///
    /// It cannot linger: the whole map is cleared when the outbox drains.
    #[test]
    fn an_item_put_back_on_sale_still_knows_what_this_till_sold_of_it() {
        let mut replica = Replica::new();
        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);
        // Forty on the shelf, four sold in an outage and not yet sent.
        replica.adjust_on_hand(Ulid::from_u128(1), Milli::new(-4_000));

        // The shop withdraws it, then puts it back the same afternoon.
        replica.apply([ItemDelta::Tombstone(Ulid::from_u128(1))]);
        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);

        // The shop's figure has not seen those four sales, so the till adds
        // them back on top, as it does for any figure that arrives.
        replica.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(40_000))]);
        assert_eq!(
            replica.by_barcode("1").map(|found| found.on_hand),
            Some(Milli::new(36_000)),
            "the four this till sold are still gone from the shelf"
        );

        // And once the shop has them, the adjustment goes.
        replica.settle_local_stock();
        replica.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(36_000))]);
        assert_eq!(
            replica.by_barcode("1").map(|found| found.on_hand),
            Some(Milli::new(36_000)),
            "counted once, not twice"
        );
    }

    /// Withdrawing an item and adding another is not the same catalogue.
    ///
    /// The shelf is asked about by position, and withdrawing moves the last
    /// item into the withdrawn one's slot. So a lap of the shelf that ran
    /// across a withdrawal and an addition went past a slot whose occupant
    /// changed under it, and the number of items says nothing about that. This
    /// is what a lap is measured against instead.
    #[test]
    fn the_shape_moves_when_an_item_arrives_or_leaves_and_not_when_one_changes() {
        let mut replica = Replica::new();
        assert_eq!(replica.shape_moved(), 0);

        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);
        replica.apply([ItemDelta::Upsert(item(2, "B", "Oil", "2"))]);
        assert_eq!(replica.shape_moved(), 2, "two items arrived");

        // A price change moves nothing: the item is where it was.
        let mut repriced = item(1, "A", "Rice", "1");
        repriced.price = Minor::new(9_900);
        replica.apply([ItemDelta::Upsert(repriced)]);
        assert_eq!(replica.shape_moved(), 2, "a price is not a shape");

        // One out, one in, which leaves the count where it was.
        replica.apply([
            ItemDelta::Tombstone(Ulid::from_u128(1)),
            ItemDelta::Upsert(item(3, "C", "Soap", "3")),
        ]);
        assert_eq!(replica.len(), 2, "the same number of items");
        assert_eq!(replica.shape_moved(), 4, "and not the same catalogue");
    }

    /// A price change is not a statement about a shelf.
    ///
    /// The catalogue row carries an `on_hand` and nothing in the shop ever puts
    /// a real number in it: the new-item form sends zero, a file import sends
    /// zero, and every other save forwards whatever the row already held. So a
    /// price correction or a barcode added set that item's shelf to zero on
    /// every till in the shop until the next lap of stock, which is up to five
    /// minutes. Under the rule that stops a sale, the item could not be sold in
    /// that window without a supervisor; under the softer one the cashier was
    /// told the shop had none of something the shelf was full of.
    ///
    /// Restating stock from the back office is a count, and a count goes
    /// through the ledger the shelf answer is computed from, not through here.
    #[test]
    fn a_catalogue_change_does_not_restate_the_shelf() {
        let mut replica = Replica::new();
        replica.apply([ItemDelta::Upsert(item(1, "A", "Rice", "1"))]);
        // What the shop says is on the shelf.
        replica.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(61_000))]);

        // Somebody in the back office corrects the price, or adds a barcode.
        // The row they send carries the item record's own stock figure, which
        // is zero and has always been zero.
        let mut corrected = item(1, "A", "Rice", "1");
        corrected.price = Minor::new(9_900);
        corrected.on_hand = Milli::ZERO;
        replica.apply([ItemDelta::Upsert(corrected)]);

        let held = replica.by_barcode("1").expect("still in the catalogue");
        assert_eq!(held.price, Minor::new(9_900), "the price is theirs to change");
        assert_eq!(
            held.on_hand,
            Milli::new(61_000),
            "and the shelf is not: sixty-one is what the shop's own ledger said"
        );
    }
}
