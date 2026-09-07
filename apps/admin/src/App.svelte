<script>
  import { onMount } from 'svelte';
  import {
    open,
    run,
    connect,
    enrol,
    keepSyncing,
    describeSync,
    admin,
    adoptToken,
    bundleMark,
  } from './till.js';
  import { money, qty } from './format.js';
  // Where a save is addressed and what it must not quietly change. One place,
  // with tests: this app got it wrong for items and again for suppliers,
  // because the second form was written by copying the first.
  import { saving } from '../../shared/records.js';
  // Money typed by a person, turned into integer poisha. Tested there, because
  // `Number()` accepts "1e3" and this is the one box on the screen that is money.
  import { minorFrom } from '../../shared/money.js';
  // Telling two people with the same name apart, shared with the till so the
  // mark on a person is the same in both places.
  import { fold, label, nameTaken, shared } from '../../shared/people.js';
  // A stock count that survives the screen it is typed into: written down as it
  // is entered, kept per shop, and filed in batches so an interrupted count
  // carries on rather than starting again.
  import {
    fileable,
    milliFrom,
    sheetKey,
    startSheet,
    summary,
    unusable,
    without,
    writeLine,
  } from '../../shared/counting.js';

  // The back office is a device like any other: it enrols with a code and gets
  // a credential. The difference is the role on that code, which is what the
  // server checks before it lets anything here through.
  const SERVER = window.location.origin.replace(/:\d+$/, ':8099');
  // Which shop and terminal this device is. Not secret, and needed before the
  // store can be opened; the credential lives in the store itself.
  const IDENTITY = 'openpos.admin.identity';

  let view = $state(null);
  let fault = $state(null);
  let done = $state(null);
  let busy = $state(false);
  // What the sync loop last did. A back office that cannot say what it is doing
  // is one where a change that never arrives looks like a change that never
  // saved, which cost an hour of looking at the wrong end of it.
  let syncing = $state('idle');
  let code = $state('');

  const enrolled = $derived(view?.enrolled ?? false);
  // Holding a credential the server will not accept. This device looks enrolled
  // and is not, and every form below would fail with a 401 nobody can read.
  const refused = $derived(view?.credential_refused ?? false);

  // Shop
  let shopName = $state('');
  let shopBin = $state('');
  let shopAddress = $state('');
  // The wallets this shop takes, typed once here rather than at a till on every
  // sale, where a typo becomes a third wallet in every report.
  let shopWallets = $state('');
  // What a till does when a basket asks for more than the shelf holds. Nothing
  // until this shop says otherwise: a shop that has never counted holds zero of
  // everything as far as the system knows, and a till that refused on that
  // basis would be a till that cannot sell.
  let shopStockRule = $state('0');

  // A person
  let personName = $state('');
  let personPin = $state('');
  let personRole = $state('cashier');
  // Everybody, suspended included. The everyday list leaves them out, which is
  // right for a sign-in panel and leaves nowhere to let anybody back in.
  let everyone = $state([]);
  // The person being corrected, or null when this is a new one. A PIN is never
  // part of a correction: it is hashed on this device when it is set and the
  // shop has no way to read it back, which is the point of hashing it here.
  let editingPerson = $state(null);

  // The item being corrected, or null when this is a new one. The whole record,
  // not the fields the form shows: what a correction must not change is decided
  // by `saving`, and it can only decide it if it has the record.
  let editing = $state(null);
  // Where the item being corrected stood when it was read, so a save built on a
  // copy somebody else has since changed is refused rather than merged.
  let editingSeq = $state(0);
  // Whether the list includes what the shop has stopped selling. Off by
  // default: the everyday question is what is on the shelves.
  let showRetired = $state(false);
  // A delivery being built, and a count being taken. Keyed by item id, because
  // the same item must not appear twice in one delivery: the server books what
  // it is sent, and two lines for one item is a double delivery.
  let delivery = $state({});
  // The count sheet, as it is being written. Held here and in this device's own
  // storage: a shop counts three hundred shelves over an afternoon, and a
  // reload used to take the afternoon with it.
  let sheet = $state(null);
  const counted = $derived(sheet ? summary(sheet) : { counted: 0, wrong: 0, total: 0 });
  const wrongLines = $derived(sheet ? new Set(unusable(sheet)) : new Set());
  // Armed, waiting for a second press before an afternoon's counting is binned.
  let abandoning = $state(false);
  let reference = $state('');
  // Who the shop buys from, and who this delivery came from.
  let suppliers = $state([]);
  let deliveredBy = $state('');
  let supplierName = $state('');
  let supplierPhone = $state('');
  let supplierBin = $state('');
  // The supplier being corrected, and null when this is a new one. Without it
  // every save minted a fresh id, so fixing a phone number put a second copy of
  // the supplier in the list: the same bug the catalogue had.
  let editingSupplier = $state(null);
  // What came in lately. Read back, because a delivery filed under a supplier is
  // only worth filing if somebody can ask which goods came on which challan.
  let deliveries = $state([]);
  // Every item this device knows, by id, for naming goods on a delivery. The
  // search results are not enough: a delivery names whatever was received, and
  // that is rarely what is on the screen at the time.
  let names = $state({});
  // What the shop took, and which day it was asked about. A shop's day ends when
  // it closes, so the boundaries are the caller's to choose; this defaults to
  // today and lets an owner change it.
  let takings = $state(null);
  // What was sold at each tax rate over a month, which is what a return needs.
  let vat = $state([]);
  let vatMonth = $state(new Date().toISOString().slice(0, 7));
  // How much of that figure is sales nobody has looked at yet.
  let vatWaiting = $state({ sales: 0, minor: 0 });
  let day = $state(new Date().toISOString().slice(0, 10));
  // Sales the server would not accept as they stood. Stored anyway: the goods
  // left the shop and the money changed hands, so refusing them would leave the
  // only copy on a tablet.
  let repairs = $state([]);
  let carriedMark = $state('');
  let decided = $state([]);
  let showDecided = $state(false);
  // Drawers counted and closed. The point of counting one is that somebody who
  // was not standing at the till reconciles it afterwards.
  let drawers = $state([]);
  // Drawers standing open right now, as each till last said. A drawer left open
  // overnight used to be invisible until somebody looked at the till itself.
  let openDrawers = $state([]);
  // What the shop owes its suppliers: the deliveries less what has been paid.
  let supplierOwing = $state([]);
  // What moved off the shelves over a period, which is what a shop orders
  // against. Named here from the catalogue this device already holds.
  let sold = $state([]);
  // What supervisors allowed over the same window, which is the other half of
  // reading a quiet week: what was sold, and what was given away.
  let waived = $state([]);
  // What this device's own store is, and whether the browser promised to keep
  // it. Shown because a back office is the device most likely to be evicted:
  // it is opened once a week, and Safari discards an origin's storage after
  // seven days of not being opened.
  let storage = $state('opening');
  let keeping = $state('unknown');
  // Whether the owner has already been told this name is taken. Told once, then
  // out of the way: a shop that means it presses again.
  let nameWarned = $state(false);
  let buyerWarned = $state(false);
  const twiceOver = $derived(shared(everyone));
  // The same for the people who buy on account, where the cost of confusing two
  // of them is a balance that belongs to neither.
  const buyersTwiceOver = $derived(shared(buyers));
  let allowedTrail = $state([]);
  let gaps = $state([]);
  // A week back by default: the question is usually about something that
  // happened recently and is remembered vaguely.
  let allowedFrom = $state(new Date(Date.now() - 7 * 86_400_000).toISOString().slice(0, 10));
  let allowedTo = $state(new Date().toISOString().slice(0, 10));
  // The till armed for cutting off, waiting for a second press.
  let cuttingOff = $state(null);
  // Price changes no till could read. Empty is the ordinary answer, and the
  // section says nothing at all when it is.
  let unreadable = $state([]);
  // Items a till wrote down at a counter, which nobody has agreed to yet.
  let fromTills = $state([]);
  let soldFrom = $state(new Date(Date.now() - 7 * 86_400_000).toISOString().slice(0, 10));
  let soldTo = $state(new Date().toISOString().slice(0, 10));
  let payingSupplier = $state({});
  let payingSupplierId = $state({});
  // The supplier whose statement is open, and what it says.
  let statementFor = $state(null);
  let statement = $state([]);
  // Everybody the shop lets buy on account, stopped accounts included.
  let buyers = $state([]);
  let buyerName = $state('');
  let buyerPhone = $state('');
  // The buyer's own BIN, when the buyer is a business. A tax invoice here names
  // both, the shop's and theirs.
  let buyerBin = $state('');
  // The buyer being corrected, or null when this is somebody new.
  let editingBuyer = $state(null);
  // Who owes the shop, and whose account is open on the screen. A shop here
  // sells on account all day and the book for it was on paper until now.
  let owing = $state([]);
  // A page each, and a button when there is more. Small on purpose: the first
  // page is what an owner reads, and a shop on a phone should not wait for
  // three hundred rows to find the four people who owe most.
  const OWED_PAGE = 50;
  const ACCOUNT_PAGE = 50;
  let owedComplete = $state(true);
  let accountComplete = $state(true);
  // Sales somebody read off a device that cannot send them, pasted in here.
  let carried = $state('');
  let openAccount = $state(null);
  let accountLines = $state([]);
  // What is being paid, keyed by the folded name, so two people being settled
  // in the same minute do not share a box.
  let paying = $state({});
  // The id minted for the payment being typed, kept until it is recorded. A
  // fresh id on every press would defeat the whole point of minting one: a
  // reply that never arrived is exactly when somebody presses again, and the
  // second press must be the same payment rather than a second one.
  let payingId = $state({});
  // Why a debt is being struck off. Required, because this is the one entry
  // here that makes money disappear.
  let writingOff = $state({});
  let notes = $state({});
  // Off, receiving a delivery, or counting a shelf. One at a time, because the
  // two put different numbers in the same box and a screen that offers both at
  // once is a screen where a count gets booked as a delivery.
  let stockMode = $state('off');
  // What the shop believes it holds, keyed by item id. Asked for separately from
  // the catalogue, because a sale is not a catalogue change: the figure on an
  // item record is whatever it was when somebody last edited that item, and
  // showing it as stock shows a number that never moves.
  let onHand = $state({});

  function setDelivery(id, field, value) {
    delivery = { ...delivery, [id]: { ...(delivery[id] ?? {}), [field]: value } };
  }
  let found = $state([]);
  let hunt = $state('');
  let itemCode = $state('');
  let itemName = $state('');
  // The same thing in Bangla, for the people who read the screens. It has been
  // carried by the catalogue and indexed by the search since both were written,
  // and nothing could set it: every item's Bangla name was a copy of its
  // English one.
  let itemNameBn = $state('');
  let itemPrice = $state('');
  let itemVat = $state('15');
  let itemBarcode = $state('');
  let itemListedPrice = $state(false);
  // Whether the price on the shelf already has the tax in it. Common in retail
  // here, and hardcoded false until now: a shop that prices inclusive and could
  // not say so would have had fifteen percent added on top of prices that
  // already carried it, on every sale.
  let itemTaxIncluded = $state(false);
  // What it is sold by. "Nos" was hardcoded, so a shop selling rice by the kilo
  // or oil by the litre had no way to say which.
  let itemUnit = $state('Nos');

  // A new till
  let tillLabel = $state('');
  let issued = $state(null);
  // The tills this shop already has. Needed before a code can be issued for one
  // of them, which is how a device whose credential was revoked gets its own
  // ledger back instead of a new and empty one.
  let tills = $state([]);
  let issuedFor = $state(null);

  /// Run something and report what happened.
  ///
  /// `quiet` leaves the last message where it is, for a refresh that follows a
  /// save. Clearing it there is how a failed save came to look like nothing at
  /// all: the search that ran next wiped the reason it failed.
  async function attempt(work, said, quiet = false) {
    busy = true;
    if (!quiet) {
      fault = null;
      done = null;
    }
    try {
      const reply = await work();
      if (reply?.view) view = reply.view;
      if (reply?.view?.error) {
        fault = reply.view.error;
        return null;
      }
      if (!quiet) done = said;
      return reply;
    } catch (error) {
      // Reported even when quiet: a refresh that failed is worth saying, and
      // the only message it can overwrite is one about the save it followed.
      fault = error.message;
      return null;
    } finally {
      busy = false;
    }
  }

  onMount(async () => {
    await connect(SERVER);
    // On its own store, like a till. A back office that forgot its credential
    // on every page load would have to be re-enrolled to change one price,
    // which is not a back office.
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (known) {
      const reply = await attempt(() => open(known.tenant, known.terminal), null);
      view = reply?.view ?? view;
      storage = reply?.info?.storage ?? 'unavailable';
      keeping = reply?.info?.keeping ?? 'unknown';
    }
    if (enrolled) {
      await loadEverything();
      // A count somebody was half way through when this screen was last closed.
      resumeSheet();
    }
    // The list is a health view: last heard from, sales, anything waiting to be
    // looked at. Loaded once it is a screenshot, and the one question it is
    // opened to answer is whether a till has stopped reporting. Slower than the
    // sync loop, because it is a whole-shop query and nobody watches it by the
    // second.
    setInterval(() => {
      if (enrolled && !busy) {
        listTills();
        // A drawer open since this morning is the question this answers, and
        // the answer changes as tills report. Same cadence as the till list,
        // because they are read together.
        listOpenDrawers();
      }
    }, 15000);
    // The back office syncs too, so it holds the shop and the people and can
    // show what it is about to change rather than writing blind. Run by the
    // worker rather than by this thread, for the reason the till's is: a hidden
    // tab's timers are throttled to about once a minute and can stop.
    keepSyncing((round) => {
      if (round.view) view = round.view;
      // The view still comes back on a failure, and it is what says whether the
      // shop has refused this device rather than merely gone quiet.
      syncing = round.ok ? describeSync(round.info) : `held up: ${round.error}`;
      // The back office pulls the catalogue like any other device, so its own
      // log grows the same way. The till folds its log between customers; this
      // has no equivalent moment, so it asks after every round and the core
      // decides whether the log is long enough to bother. Without it the log
      // grew for the life of the device and every boot replayed all of it: the
      // same defect the till had before anything called this.
      if (round.ok && !busy) {
        run({ op: 'checkpoint' }).catch(() => {
          // Housekeeping. A back office that could not tidy up still works, and
          // the next round tries again.
        });
      }
    }, 3000);
  });

  async function join() {
    const typed = code.trim();
    if (!typed) return;
    code = '';
    await attempt(async () => {
      // The code says which shop and which terminal this device is. Nothing is
      // opened before that answer arrives.
      const { info } = await enrol(typed);
      localStorage.setItem(
        IDENTITY,
        JSON.stringify({ tenant: info.tenant, terminal: info.terminal }),
      );
      const opened = await open(info.tenant, info.terminal);
      storage = opened.info?.storage ?? 'unavailable';
      keeping = opened.info?.keeping ?? 'unknown';
      const adopted = await adoptToken(info.token);
      return { view: adopted.view ?? opened.view };
    }, 'Enrolled.');
    if (view?.enrolled) {
      await loadEverything();
    }
  }

  const roles = {
    cashier: { max_discount_bp: 0, may_open_drawer: true },
    supervisor: {
      max_discount_bp: 2000,
      may_override_price: true,
      may_refund: true,
      may_void_line: true,
      may_authorise: true,
      may_open_drawer: true,
      may_close_shift: true,
    },
  };

  function newId() {
    return crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
  }

  /// Sixteen random bytes, per person. A shared salt means one search cracks
  /// every PIN in the shop at once, so this is generated here and never reused.
  function newSalt() {
    return Array.from(crypto.getRandomValues(new Uint8Array(16)));
  }

  async function saveShop() {
    if (!shopName.trim()) {
      fault = 'a shop needs a name: it is what heads every receipt';
      return;
    }
    await attempt(
      () =>
        admin(
          {
            what: 'shop',
            name: shopName.trim(),
            bin: shopBin,
            address: shopAddress,
            phone: null,
            wallets: shopWallets
              .split(',')
              .map((one) => one.trim())
              .filter(Boolean),
            stock_rule: Number(shopStockRule),
          },
          Date.now(),
        ),
      'Shop details saved. Tills pick them up within ten minutes.',
    );
    // Read back rather than assumed: the server trims and de-duplicates the
    // wallets and clamps the rule, so what was typed and what the shop now
    // holds are not always the same thing.
    await loadShop();
  }

  /// What a person may do, as the request wants it.
  function permissionsOf(person) {
    return {
      max_discount_bp: person.max_discount_bp,
      may_override_price: person.may_override_price,
      may_refund: person.may_refund,
      may_void_line: person.may_void_line,
      may_authorise: person.may_authorise,
      may_open_drawer: person.may_open_drawer,
      may_close_shift: person.may_close_shift,
    };
  }

  function correctPerson(person) {
    editingPerson = person;
    personName = person.name;
    // The nearest preset, for the dropdown. Saving sends the preset, so a
    // correction does change what they may do: that is what the dropdown is
    // for, and the screen shows which one is selected before it is saved.
    personRole = person.may_refund ? 'supervisor' : 'cashier';
    scrollTo({ top: 0, behavior: 'smooth' });
  }

  function newPerson() {
    editingPerson = null;
    personName = '';
    personPin = '';
    personRole = 'cashier';
  }

  /// Give somebody a new PIN.
  ///
  /// Separate from correcting them, because it is a different act: this one
  /// carries a credential and the other carries none. The digits are hashed on
  /// this device and never travel, which is also why a forgotten PIN cannot be
  /// looked up, only replaced.
  async function setPin() {
    if (personPin.length < 4) {
      fault = 'a PIN of at least four digits';
      return;
    }
    const pin = personPin;
    personPin = '';
    const saved = await attempt(
      () =>
        admin(
          { what: 'operator_pin', id: editingPerson.id, pin, salt: newSalt() },
          Date.now(),
        ),
      `${editingPerson.name} has a new PIN. Tills accept it within ten minutes.`,
    );
    if (!saved) return;
    newPerson();
    await listPeople();
  }

  /// Correct a name or what somebody may do, without their PIN.
  async function amendPerson() {
    if (!personName.trim()) {
      fault = 'a person needs a name: it is what a receipt and a shift are filed under';
      return;
    }
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'amend_operator',
            id: editingPerson.id,
            name: personName.trim(),
            permissions: roles[personRole],
            active: editingPerson.active,
          },
          Date.now(),
        ),
      `${personName.trim()} corrected. Tills pick it up within ten minutes.`,
    );
    if (!saved) return;
    newPerson();
    await listPeople();
  }

  async function savePerson() {
    if (!personName.trim() || personPin.length < 4) {
      fault = 'a name, and a PIN of at least four digits';
      return;
    }
    // Two people called Karim make two identical buttons at every till, and a
    // cashier who presses the wrong one hands that whole shift to somebody
    // else. Said once, and then allowed: a shop can have two Karims, and the
    // answer is a name that tells them apart rather than a form that refuses.
    if (nameTaken(everyone, personName) && !nameWarned) {
      nameWarned = true;
      fault =
        'somebody who can sign in is already called that. Two identical buttons at a till is how' +
        ' a shift ends up attributed to the wrong person: give them a name that tells them apart,' +
        ' or press again to add them anyway.';
      return;
    }
    nameWarned = false;
    const pin = personPin;
    personPin = '';
    await attempt(
      () =>
        admin(
          {
            what: 'operator',
            id: newId(),
            name: personName.trim(),
            pin,
            salt: newSalt(),
            permissions: roles[personRole],
            active: true,
          },
          Date.now(),
        ),
      `${personName.trim()} can sign in once the tills refresh.`,
    );
    newPerson();
    await listPeople();
  }

  /// Stop selling something, or start again.
  ///
  /// The whole item goes back with one field changed, because that is what the
  /// route takes. A till refuses to ring a retired item and still refunds one:
  /// the shop sold it last week and the customer is standing there with it.
  async function setSelling(item, selling) {
    // Read from the shop first. This list is up to half a minute behind, and
    // this route sends the whole item: withdrawing something from a stale row
    // would put back whatever somebody else changed in the meantime.
    const read = await attempt(() => admin({ what: 'item_now', item: item.id }, Date.now()), null);
    const held = read?.info?.item_now;
    if (!held) {
      fault = 'the shop has withdrawn that item already';
      await look(true);
      return;
    }

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'item',
            // Where it stood a moment ago. The server refuses a save built on
            // an older copy rather than letting it undo somebody else's change.
            expected_seq: read?.info?.item_seq ?? 0,
            item: {
              id: held.id,
              code: held.code,
              name: held.name,
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              unit: held.unit,
              // Copied, not passed. What comes out of the view is a reactive
              // proxy, and a proxy cannot be posted to a worker: it fails at the
              // boundary with a message about cloning that says nothing about
              // which field.
              barcodes: [...held.barcodes],
              on_hand_milli: held.on_hand_milli,
              active: selling,
            },
            price_minor: held.price_minor,
            cost_minor: held.cost_minor,
            vat_bp: held.vat_bp,
            price_inclusive: held.price_inclusive,
            vat_on_undiscounted: held.vat_on_undiscounted,
            active: selling,
          },
          Date.now(),
        ),
      selling
        ? `${item.name} is on sale again. Tills pick it up within half a minute.`
        : `${item.name} will not ring at a till any more. Refunds of it still work.`,
    );
    if (!reply) return;
    // Changed here as well as at the server, because the list is read back from
    // this device's own copy of the catalogue and that copy is up to half a
    // minute behind. Without this, correcting a price in that window would carry
    // the stale flag back and quietly put a withdrawn item on sale again: the
    // form would be faithfully preserving something that was no longer true.
    // And no reload after it. Asking the replica again would read the stale
    // copy straight back over this, which is what the first version of this fix
    // did: the row flipped back before anybody saw it change.
    found = found.map((one) => (one.id === item.id ? { ...one, active: selling } : one));
  }

  async function look(quiet = false) {
    const reply = await attempt(
      () => run({ op: 'catalogue', query: hunt.trim(), limit: 50, retired: showRetired }),
      null,
      quiet,
    );
    if (!reply) return;
    found = reply.view?.catalogue ?? [];
    await askStock(found);
  }

  /// Load an item into the form so the next save corrects it.
  /// Open an item for correction, reading it from the shop rather than from
  /// this device's copy.
  ///
  /// The copy here is up to half a minute behind, and a save carries the whole
  /// item: editing a price on a stale row would put back whatever somebody else
  /// changed in the meantime, including a withdrawal.
  async function correct(item) {
    const reply = await attempt(
      () => admin({ what: 'item_now', item: item.id }, Date.now()),
      null,
    );
    const fresh = reply?.info?.item_now;
    if (!fresh) {
      fault = 'the shop has withdrawn that item since this list was read';
      await look(true);
      return;
    }
    editingSeq = reply?.info?.item_seq ?? 0;
    correctFrom(fresh);
  }

  function correctFrom(item) {
    editing = item;
    itemTaxIncluded = item.price_inclusive;
    itemUnit = item.unit || 'Nos';
    itemName = item.name;
    // Blank when it is only a copy of the English name, so an owner sees an
    // empty box to fill in rather than the same words twice.
    itemNameBn = item.name_bn === item.name ? '' : item.name_bn;
    itemCode = item.code;
    itemPrice = (item.price_minor / 100).toFixed(2);
    itemVat = (item.vat_bp / 100).toString();
    itemBarcode = item.barcodes[0] ?? '';
    itemListedPrice = item.vat_on_undiscounted;
    scrollTo({ top: 0, behavior: 'smooth' });
  }

  function startFresh() {
    editing = null;
    itemTaxIncluded = false;
    itemUnit = 'Nos';
    itemName = '';
    itemNameBn = '';
    itemCode = '';
    itemPrice = '';
    itemVat = '15';
    itemBarcode = '';
    itemListedPrice = false;
  }

  async function saveItem() {
    const where = saving(editing, newId, { active: true, cost_minor: 0 });
    const price = Number(itemPrice);
    const vat = Number(itemVat);
    if (!itemName.trim() || !Number.isFinite(price) || price < 0) {
      fault = 'a name and a price in taka';
      return;
    }
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'item',
            // Where it stood when it was read for editing. The server refuses a
            // save built on an older copy rather than letting it put back
            // whatever somebody else changed.
            expected_seq: editing ? editingSeq : 0,
            item: {
              ...where,
              code: itemCode.trim(),
              name: itemName.trim(),
              name_bn: itemNameBn.trim(),
              unit: itemUnit.trim() || 'Nos',
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              barcodes: itemBarcode.trim() ? [itemBarcode.trim()] : [],
              on_hand_milli: 0,
            },
            price_minor: Math.round(price * 100),
            cost_minor: where.cost_minor,
            active: where.active,
            vat_bp: Math.round(vat * 100),
            price_inclusive: itemTaxIncluded,
            vat_on_undiscounted: itemListedPrice,
          },
          Date.now(),
        ),
      editing
        ? `${itemName.trim()} corrected. Tills pick it up within half a minute, and this list with them.`
        : `${itemName.trim()} added. Tills pick it up within half a minute.`,
    );
    // Only on success. Clearing the form after a refusal loses what the owner
    // typed and leaves them nothing to correct.
    if (!saved) {
      // The shop's own words come back with the refusal now, so there is
      // nothing to guess at here. A bare status is all that is left when a
      // server one release ahead sends a refusal this build does not know.
      if (String(fault ?? '').includes('409')) {
        fault =
          'somebody else changed that item while you had it open. Press "Correct it" again to see what it says now.';
      }
      return;
    }
    startFresh();
    // The change reaches this device the way it reaches a till, on the next
    // pull, so the list is asked again rather than edited here to look right.
    // Quietly, or the confirmation is gone before it is read.
    await look(true);
  }

  /// Book a delivery, so the figures go up as well as down.
  ///
  /// Until this existed the only thing that moved stock was a sale, so every
  /// figure in the shop walked towards zero and stayed wrong.
  async function listDrawers(quiet = true) {
    const reply = await attempt(() => admin({ what: 'shifts', limit: 20 }, Date.now()), null, quiet);
    if (reply) drawers = reply.info?.shifts ?? [];
  }

  /// Take in sales carried from a device that could not send them.
  ///
  /// The only way a shop gets the takings off a till whose terminal was deleted,
  /// or one that has to be enrolled again as another. Every one of them lands in
  /// the queue below, because the credential that would ordinarily say where a
  /// sale came from is exactly what such a device has lost.
  /// Read a bundle out of a file the till wrote.
  ///
  /// The two devices are usually not the same one, and the bundle is thousands
  /// of characters: a file goes on a memory stick or through an email, where
  /// selecting text on a tablet screen does not.
  async function openCarriedFile(event) {
    const file = event.currentTarget.files?.[0];
    if (!file) return;
    carried = await file.text();
    await markCarried();
    // Cleared so the same file can be chosen again after a failed attempt.
    event.currentTarget.value = '';
  }

  /// What the paste hashes to, worked out by the same code that marked it on the
  /// device it came from. A mark that differs is a paste that got cut short,
  /// which otherwise looks exactly like one that did not.
  async function markCarried() {
    const text = carried.trim();
    if (!text) {
      carriedMark = '';
      return;
    }
    const reply = await attempt(() => bundleMark(text), null, true);
    carriedMark = reply?.info?.mark ?? '';
  }

  async function adoptCarried() {
    const bundle = carried.trim();
    if (!bundle) {
      fault = 'paste what the till showed you';
      return;
    }
    const reply = await attempt(
      () => admin({ what: 'adopt_sales', bundle }, Date.now()),
      'Taken in. That device can be wiped now.',
    );
    if (!reply) return;
    carried = '';
    carriedMark = '';
    done = `Taken in ${reply.info?.adopted ?? 0} sale(s). They are in the list below for you to check.`;
    await listRepairs(true);
  }

  /// Add somebody who buys on account, or correct them.
  ///
  /// The shop writing a name down is what stops two Karims sharing an account:
  /// a sale that names one of these lands on that person whatever the cashier
  /// typed at the till.
  async function saveBuyer() {
    const name = buyerName.trim();
    if (!name) {
      fault = 'a name to write down';
      return;
    }
    // Two records for one person is two accounts: what they took goes on one
    // and what they paid on the other, and neither balance is theirs. Said
    // once, then allowed, because a shop can have two customers of one name and
    // the answer is a name that tells them apart.
    if (nameTaken(buyers, name, editingBuyer?.id ?? null) && !buyerWarned) {
      buyerWarned = true;
      fault =
        'somebody with an account is already called that. Two records for one person is two' +
        ' accounts, and what they owe ends up split between them: give them a name that tells' +
        ' them apart, or press again to write this one down anyway.';
      return;
    }
    buyerWarned = false;
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'customer',
            ...saving(editingBuyer, newId, { active: true }),
            name,
            phone: buyerPhone.trim() === '' ? null : buyerPhone.trim(),
            bin: buyerBin.trim() === '' ? null : buyerBin.trim(),
          },
          Date.now(),
        ),
      editingBuyer ? 'Corrected.' : 'Written down.',
    );
    if (!reply) return;
    buyers = reply.info?.every_customer ?? buyers;
    buyerName = '';
    buyerPhone = '';
    buyerBin = '';
    editingBuyer = null;
  }

  function correctBuyer(buyer) {
    editingBuyer = buyer;
    buyerName = buyer.name;
    buyerPhone = buyer.phone ?? '';
    buyerBin = buyer.bin ?? '';
  }

  /// Stop somebody's account, or let them buy on account again. What they
  /// already owe is untouched: a stopped account is not a settled one.
  async function setAccountAllowed(buyer, allowed) {
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'customer',
            id: buyer.id,
            name: buyer.name,
            phone: buyer.phone ?? null,
            active: allowed,
          },
          Date.now(),
        ),
      allowed ? 'They can buy on account again.' : 'Their account is stopped.',
    );
    if (reply) buyers = reply.info?.every_customer ?? buyers;
  }

  /// Everything this screen shows, in one place.
  ///
  /// Called on opening and again after enrolling, which are the two moments a
  /// device has a credential and an empty screen. Two lists of loaders is two
  /// lists to keep in step, and the one forgotten was the shop's own settings: a
  /// device that had just enrolled showed an empty form over a shop that has a
  /// name, an address and a rule about the shelf.
  async function loadEverything() {
    await loadShop();
    await listTills();
    await listPeople();
    await listSuppliers();
    await listDeliveries();
    await askTakings();
    await listRepairs();
    await listDrawers();
    await listOwed();
    await listOpenDrawers();
    await listBuyers();
    await listSupplierOwing();
    await listUnreadable();
    await listFromTills();
    await listGaps();
  }

  /// The shop as it stands, into the form that overwrites it.
  ///
  /// A form that opens empty is a form that saves an empty shop, and a rule
  /// nobody can see is a rule nobody can tell is on. Read from the same route a
  /// till reads, so what this shows and what a till obeys are one answer.
  async function loadShop(quiet = true) {
    const reply = await attempt(() => admin({ what: 'shop_now' }, Date.now()), null, quiet);
    const shop = reply?.info?.shop;
    if (!shop) return;
    shopName = shop.name ?? '';
    shopBin = shop.bin ?? '';
    shopAddress = shop.address ?? '';
    shopWallets = (shop.wallets ?? []).join(', ');
    shopStockRule = String(shop.stock_rule ?? 0);
  }

  async function listBuyers(quiet = true) {
    const reply = await attempt(() => admin({ what: 'customers' }, Date.now()), null, quiet);
    if (reply) buyers = reply.info?.every_customer ?? [];
  }

  /// What sold between two days, most sold first.
  async function askSold() {
    const start = new Date(`${soldFrom}T00:00:00`);
    const end = new Date(`${soldTo}T00:00:00`);
    if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) {
      fault = 'those are not dates';
      return;
    }
    end.setDate(end.getDate() + 1);
    const reply = await attempt(
      () =>
        admin(
          { what: 'sold', from_ms: start.getTime(), to_ms: end.getTime() - 1, limit: 100 },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    sold = reply.info?.sold ?? [];
    // Asked for the same window, and asked at all: this list was rendered and
    // never fetched, so a report the shop was told it had showed nothing for as
    // long as it existed.
    const given = await attempt(
      () =>
        admin(
          { what: 'waived', from_ms: start.getTime(), to_ms: end.getTime() - 1, limit: 100 },
          Date.now(),
        ),
      null,
      true,
    );
    waived = given?.info?.waived ?? [];
    // The names come from this device's own catalogue, so a report is not the
    // same strings sent again on every request for the life of the shop.
    if (sold.length > 0 && Object.keys(names).length === 0) await learnNames();
  }

  /// Where the numbering jumps.
  ///
  /// Asked with the till list, because reading a gap needs the other half: a
  /// gap on a till that synced an hour ago is one thing, and a gap on a till
  /// nobody has heard from since Tuesday is another.
  async function listGaps(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'receipt_gaps', limit: 50 }, Date.now()),
      null,
      quiet,
    );
    if (reply) gaps = reply.info?.gaps ?? [];
  }

  /// Who allowed what, between two days.
  async function askAllowed() {
    const start = new Date(`${allowedFrom}T00:00:00`);
    const end = new Date(`${allowedTo}T00:00:00`);
    if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) {
      fault = 'those are not dates';
      return;
    }
    end.setDate(end.getDate() + 1);
    const reply = await attempt(
      () =>
        admin(
          { what: 'allowed', from_ms: start.getTime(), to_ms: end.getTime() - 1, limit: 200 },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    allowedTrail = reply.info?.allowed ?? [];
    if (allowedTrail.length === 0) done = 'Nothing was allowed over a ceiling in those days.';
  }

  /// Cut a device off, because it is lost or stolen.
  ///
  /// Two presses: one press stops a working till dead in the middle of a
  /// trading day, and the person pressing is usually already flustered.
  ///
  /// The device is not wiped and cannot be. If it turns up still holding sales,
  /// they are read off it and pasted in above, which needs no credential.
  async function cutOff(till) {
    if (cuttingOff !== till.id) {
      cuttingOff = till.id;
      return;
    }
    cuttingOff = null;
    const reply = await attempt(
      () => admin({ what: 'revoke_terminal', terminal: till.id }, Date.now()),
      null,
    );
    if (!reply) return;
    const withdrawn = reply.info?.withdrawn ?? 0;
    done = withdrawn > 0
      ? `That device is cut off. It can ring nothing into this shop now. If it turns up holding sales, read them off it and paste them in above.`
      : 'That device was already cut off, or had never been used.';
    await listTills();
  }

  async function listUnreadable(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'unreadable_changes', limit: 200 }, Date.now()),
      null,
      quiet,
    );
    if (reply) unreadable = reply.info?.unreadable ?? [];
  }

  /// Items a till wrote down at a counter, for somebody to look at.
  ///
  /// A price typed to get a queue moving is not a price the shop set, and the
  /// only thing that makes it one is somebody here saying so.
  async function listFromTills(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'items_from_tills', limit: 200 }, Date.now()),
      null,
      quiet,
    );
    if (reply) fromTills = reply.info?.from_tills ?? [];
  }

  /// Say that what a till wrote down is right, as it stands.
  ///
  /// The same save the item screen does, which is what clears the mark: there
  /// is no second way to agree to an item.
  async function agreeToItem(item) {
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'item',
            // Zero, because agreeing to it is not editing it: whatever the shop
            // holds now is what is being agreed to, and a sequence read a
            // moment ago would refuse the save if a till had touched it since.
            expected_seq: 0,
            item: {
              id: item.id,
              code: item.code,
              name: item.name,
              name_bn: item.name_bn,
              unit: item.unit,
              // Zeroed here and sent beside, which is how this request has
              // always carried the money.
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              barcodes: item.barcodes,
              on_hand_milli: item.on_hand_milli,
              active: item.active,
            },
            // Beside the item rather than in it, which is where this request
            // has always carried the money: the item shape a screen builds is
            // not the shape the catalogue stores.
            price_minor: item.price_minor,
            cost_minor: item.cost_minor,
            vat_bp: item.vat_bp,
            price_inclusive: item.price_inclusive,
            vat_on_undiscounted: item.vat_on_undiscounted,
          },
          Date.now(),
        ),
      'Kept as it stands. Your tills have it.',
    );
    if (saved) await listFromTills();
  }

  async function listSupplierOwing(quiet = true) {
    const reply = await attempt(() => admin({ what: 'supplier_owing' }, Date.now()), null, quiet);
    if (reply) supplierOwing = reply.info?.supplier_owing ?? [];
  }

  /// Record what was handed to a supplier.
  ///
  /// The id is minted once and kept until it is recorded, so pressing again
  /// after a reply that never came is the same payment rather than a second one.
  async function paySupplier(owing) {
    const poisha = minorFrom(payingSupplier[owing.supplier] ?? '');
    if (poisha === null || poisha <= 0) {
      fault = 'say how much you handed over';
      return;
    }
    const id = payingSupplierId[owing.supplier] ?? newId();
    payingSupplierId = { ...payingSupplierId, [owing.supplier]: id };

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'pay_supplier',
            id,
            supplier: owing.supplier,
            amount_minor: poisha,
            paid_at_ms: Date.now(),
            note: null,
          },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    const now = reply.info?.owed_now;
    const after = now === undefined || now === null
      ? ''
      : now > 0
        ? ` You still owe them ${money(now)}.`
        : now < 0
          ? ` You are paid ahead by ${money(-now)}.`
          : ' You owe them nothing now.';
    done = reply.info?.already_paid
      ? `That one was already recorded.${after}`
      : `Paid.${after}`;
    payingSupplier = { ...payingSupplier, [owing.supplier]: '' };
    payingSupplierId = { ...payingSupplierId, [owing.supplier]: null };
    await listSupplierOwing(true);
  }

  /// What passed between the shop and one supplier, so the two figures can be
  /// put side by side when they disagree.
  async function showStatement(owing) {
    if (statementFor === owing.supplier) {
      statementFor = null;
      statement = [];
      return;
    }
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'supplier_statement',
            supplier: owing.supplier,
            from_ms: 0,
            to_ms: Date.now(),
          },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    statementFor = owing.supplier;
    statement = reply.info?.statement ?? [];
  }

  async function listOpenDrawers(quiet = true) {
    const reply = await attempt(() => admin({ what: 'open_drawers' }, Date.now()), null, quiet);
    if (reply) openDrawers = reply.info?.open_drawers ?? [];
  }

  /// A page of who owes, carrying on from the last one when asked.
  ///
  /// The server pages this rather than cutting it off, so a shop that lets three
  /// hundred families buy on account can read all of them instead of seeing the
  /// first page as though it were the whole list.
  async function listOwed(quiet = true, more = false) {
    const from = more && owing.length > 0 ? owing[owing.length - 1] : null;
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'owed',
            limit: OWED_PAGE,
            after_owed_minor: from ? from.owed_minor : 0,
            after_person_key: from ? from.person_key : '',
          },
          Date.now(),
        ),
      null,
      quiet,
    );
    if (!reply) return;
    const page = reply.info?.owed ?? [];
    owing = more ? [...owing, ...page] : page;
    // A short page is the end of the list. Asking again would be one request to
    // be told nothing, every time.
    owedComplete = page.length < OWED_PAGE;
  }

  /// What one person's balance is made of, which is what gets read out when
  /// somebody says they already paid.
  async function showAccount(person) {
    if (openAccount === person.person_key) {
      openAccount = null;
      accountLines = [];
      return;
    }
    await readAccount(person, false);
  }

  /// A page of one person's account, carrying on from the last one when asked.
  async function readAccount(person, more) {
    const from = more && accountLines.length > 0 ? accountLines[accountLines.length - 1] : null;
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'account',
            person_key: person.person_key,
            limit: ACCOUNT_PAGE,
            after_at_ms: from ? from.at_ms : 0,
            after_source_id: from ? from.source : '',
          },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    const page = reply.info?.account ?? [];
    openAccount = person.person_key;
    accountLines = more ? [...accountLines, ...page] : page;
    accountComplete = page.length < ACCOUNT_PAGE;
  }

  /// Take money off what somebody owes.
  ///
  /// The id is minted here, so pressing this twice because the first reply was
  /// slow does not count the money twice.
  async function takePayment(person, writtenOff = false) {
    const typed = (paying[person.person_key] ?? '').trim();
    // Parsed from the digits rather than by Number(): that accepts 1e3 and
    // 0.001 and hands back something nobody typed, in the one place on this
    // screen where the number is money.
    const poisha = minorFrom(typed);
    if (poisha === null || poisha <= 0) {
      fault = writtenOff ? 'say how much to strike off' : 'say how much they handed over';
      return;
    }
    const why = (writingOff[person.person_key] ?? '').trim();
    if (writtenOff && !why) {
      fault = 'say why it is coming off: this is the entry that makes money disappear';
      return;
    }
    // Minted once and kept until it is recorded, so pressing again after a
    // reply that never came sends the same payment rather than a second one.
    const id = payingId[person.person_key] ?? newId();
    payingId = { ...payingId, [person.person_key]: id };

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'take_payment',
            id,
            person_key: person.person_key,
            person_name: person.person_name,
            amount_minor: poisha,
            at_ms: Date.now(),
            note: writtenOff ? why : null,
            written_off: writtenOff,
          },
          Date.now(),
        ),
      // Said below instead, because the useful confirmation carries what they
      // owe now rather than only that something happened.
      null,
    );
    if (!reply) return;
    // What they owe now, straight from the book rather than from this screen's
    // arithmetic: another till may have sold to them while this was typed.
    const now = reply.info?.owed_now;
    const after = now === undefined || now === null
      ? ''
      : now > 0
        ? ` ${person.person_name} still owes ${money(now)}.`
        : now < 0
          ? ` ${person.person_name} is in credit by ${money(-now)}.`
          : ` ${person.person_name} owes nothing now.`;
    done = reply.info?.already_paid
      ? `That one was already recorded.${after}`
      : `${writtenOff ? 'Struck off, with the reason.' : 'Taken off what they owe.'}${after}`;
    paying = { ...paying, [person.person_key]: '' };
    payingId = { ...payingId, [person.person_key]: null };
    writingOff = { ...writingOff, [person.person_key]: '' };
    // Asked again rather than adjusted here: the book is the answer, and a
    // screen doing its own arithmetic is a second opinion nobody wants.
    await listOwed(true);
    if (openAccount === person.person_key) {
      openAccount = null;
      await showAccount(person);
    }
  }

  async function listRepairs(quiet = true) {
    const reply = await attempt(() => admin({ what: 'repairs', limit: 50 }, Date.now()), null, quiet);
    if (reply) repairs = reply.info?.repairs ?? [];
  }

  /// What has already been answered, which is the only way back to a wrong
  /// answer: an entry that has been decided is out of the queue.
  async function listDecided(quiet = true) {
    const reply = await attempt(() => admin({ what: 'decided', limit: 50 }, Date.now()), null, quiet);
    if (reply) decided = reply.info?.decided ?? [];
  }

  /// Change an answer. A separate act with its own note, because a strike-out
  /// took a real debt off somebody's account and getting it back has to be
  /// something a person chose to do.
  async function changeAnswer(entry, kept) {
    const note = (notes[entry.id] ?? '').trim();
    if (!note) {
      fault = 'say why the answer is changing: this is what explains a figure that moved';
      return;
    }
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'decide_again',
            sale: entry.id,
            note,
            kept,
            // What this screen saw. If somebody else answered in the meantime
            // the server refuses rather than letting a stale view win.
            expected_decisions: entry.decisions,
          },
          Date.now(),
        ),
      kept
        ? 'Put back. It counts again, and so does anything it put on an account.'
        : 'Struck out. It has come out of your takings, your tax and your stock.',
    );
    if (!reply) return;
    if (reply.info?.decision_stale) {
      done = 'Somebody else answered that one while this was open. Nothing changed: look again.';
    } else if (!reply.info?.decision_changed) {
      done = 'Nobody had answered about that one. It is still in the queue.';
    }
    notes = { ...notes, [entry.id]: '' };
    await listDecided();
    await listRepairs();
  }

  /// Say what was decided about one of them.
  ///
  /// A note is required by the server and by sense: the queue is worked months
  /// before anybody asks why a total was wrong, and an entry that disappears
  /// without one leaves that question unanswerable.
  async function resolve(entry, kept) {
    const note = (notes[entry.id] ?? '').trim();
    if (!note) {
      fault = 'say what you decided: this is what somebody reads in six months';
      return;
    }
    const reply = await attempt(
      () => admin({ what: 'resolve_repair', sale: entry.id, note, kept }, Date.now()),
      kept
        ? 'Kept. It counts as it did.'
        : 'Struck out. It has come out of your takings, your tax and your stock.',
    );
    if (!reply) return;
    if (reply.info?.already_resolved) {
      done = 'That one was already dealt with. Nothing changed.';
    }
    notes = { ...notes, [entry.id]: '' };
    await listRepairs();
    if (showDecided) await listDecided();
  }

  async function askTakings() {
    const start = new Date(`${day}T00:00:00`);
    if (Number.isNaN(start.getTime())) {
      fault = 'that is not a date';
      return;
    }
    const end = new Date(start);
    end.setDate(end.getDate() + 1);
    const reply = await attempt(
      () =>
        admin(
          { what: 'day', from_ms: start.getTime(), to_ms: end.getTime() - 1 },
          Date.now(),
        ),
      null,
    );
    if (reply) takings = reply.info?.day ?? null;
  }

  /// What the shop owes the revenue for a month, by rate.
  async function askVat() {
    const start = new Date(`${vatMonth}-01T00:00:00`);
    if (Number.isNaN(start.getTime())) {
      fault = 'that is not a month';
      return;
    }
    const end = new Date(start);
    end.setMonth(end.getMonth() + 1);
    const reply = await attempt(
      () => admin({ what: 'vat', from_ms: start.getTime(), to_ms: end.getTime() - 1 }, Date.now()),
      null,
    );
    if (!reply) return;
    vat = reply.info?.vat ?? [];
    vatWaiting = {
      sales: reply.info?.vat_waiting_sales ?? 0,
      minor: reply.info?.vat_waiting_minor ?? 0,
    };
  }

  async function learnNames() {
    // Retired included: a delivery from last month can name something the shop
    // has since stopped selling, and "an item not on this page" is not an answer.
    const reply = await attempt(
      () => run({ op: 'catalogue', query: '', limit: 500, retired: true }),
      null,
      true,
    );
    if (!reply) return;
    const map = {};
    for (const item of reply.view?.catalogue ?? []) map[item.id] = item.name;
    names = map;
  }

  async function listDeliveries(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'deliveries', limit: 20 }, Date.now()),
      null,
      quiet,
    );
    if (!reply) return;
    deliveries = reply.info?.deliveries ?? [];
    await learnNames();
  }

  async function listSuppliers(quiet = true) {
    const reply = await attempt(() => admin({ what: 'suppliers' }, Date.now()), null, quiet);
    if (reply) suppliers = reply.info?.suppliers ?? [];
  }

  function correctSupplier(one) {
    editingSupplier = one;
    supplierName = one.name;
    supplierPhone = one.phone ?? '';
    supplierBin = one.bin ?? '';
  }

  function newSupplier() {
    editingSupplier = null;
    supplierName = '';
    supplierPhone = '';
    supplierBin = '';
  }

  async function saveSupplier() {
    if (!supplierName.trim()) {
      fault = 'a supplier needs a name: it is what a delivery is filed under';
      return;
    }
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'supplier',
            ...saving(editingSupplier, newId, { active: true }),
            name: supplierName.trim(),
            phone: supplierPhone.trim() || null,
            bin: supplierBin.trim() || null,
          },
          Date.now(),
        ),
      editingSupplier ? `${supplierName.trim()} corrected.` : `${supplierName.trim()} added.`,
    );
    if (!reply) return;
    suppliers = reply.info?.suppliers ?? suppliers;
    newSupplier();
  }

  /// Stop buying from somebody, or start again.
  ///
  /// Kept rather than deleted, so the deliveries already filed under them still
  /// name somebody in six months.
  async function setBuying(one, buying) {
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'supplier',
            id: one.id,
            name: one.name,
            phone: one.phone,
            bin: one.bin,
            active: buying,
          },
          Date.now(),
        ),
      buying
        ? `${one.name} is back on the list.`
        : `${one.name} will not be offered on a delivery. What they already delivered still says so.`,
    );
    if (reply) suppliers = reply.info?.suppliers ?? suppliers;
  }

  async function askStock(items) {
    if (items.length === 0) return;
    const reply = await attempt(
      () => admin({ what: 'on_hand', item_ids: items.map((item) => item.id) }, Date.now()),
      null,
      true,
    );
    if (!reply) return;
    const figures = {};
    for (const entry of reply.info?.on_hand ?? []) figures[entry.item_id] = entry;
    onHand = figures;
  }

  async function bookDelivery() {
    const lines = Object.entries(delivery)
      .filter(([, row]) => String(row.qty ?? '').trim() !== '')
      .map(([item_id, row]) => ({
        item_id,
        qty_milli: Math.round(Number(row.qty) * 1000),
        unit_cost_minor: Math.round(Number(row.cost || 0) * 100),
      }))
      .filter((line) => Number.isFinite(line.qty_milli) && line.qty_milli > 0);
    if (lines.length === 0) {
      fault = 'nothing to book: put a quantity against something';
      return;
    }

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'receive',
            // Minted here, so a dropped reply can be sent again without the
            // goods being counted twice.
            id: newId(),
            supplier_id: deliveredBy || null,
            reference: reference.trim() || null,
            received_at_ms: Date.now(),
            lines,
          },
          Date.now(),
        ),
      `${lines.length} ${lines.length === 1 ? 'line' : 'lines'} booked in.`,
    );
    if (!reply) return;
    if (reply.info?.already_booked) {
      done = 'That delivery was already booked. Nothing was counted twice.';
    }
    delivery = {};
    reference = '';
    deliveredBy = '';
    await look(true);
    await listDeliveries();
  }

  /// Write one shelf into the sheet, and keep it.
  ///
  /// Saved on every keystroke rather than on a button, because the thing that
  /// loses a count is not somebody forgetting to press save: it is a tab
  /// closing, a battery dying, or a phone deciding to reload the page.
  function countShelf(itemId, typed) {
    if (!sheet) sheet = startSheet(Date.now());
    sheet = writeLine(sheet, itemId, typed, newId);
    keepSheet();
  }

  function keepSheet() {
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (!known) return;
    if (sheet) {
      localStorage.setItem(sheetKey(known.tenant), JSON.stringify(sheet));
    } else {
      localStorage.removeItem(sheetKey(known.tenant));
    }
  }

  function resumeSheet() {
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (!known) return;
    const held = JSON.parse(localStorage.getItem(sheetKey(known.tenant)) ?? 'null');
    if (held && held.lines && Object.keys(held.lines).length > 0) {
      sheet = held;
      // A count in progress is the reason this screen was opened. Say so rather
      // than leaving it to be discovered.
      stockMode = 'counting';
    }
  }

  /// Record what the shelves were found to hold.
  ///
  /// A count replaces the running figure rather than adjusting it, which is the
  /// only way a figure that has drifted since the shop opened gets corrected.
  ///
  /// Sent in batches, and each batch that is accepted comes out of the sheet. A
  /// count of three hundred shelves interrupted at a hundred and forty is one
  /// that carries on from a hundred and forty.
  async function bookCount() {
    const lines = sheet ? fileable(sheet) : [];
    if (lines.length === 0) {
      fault = counted.wrong > 0
        ? 'some boxes do not hold a number yet'
        : 'nothing counted yet';
      return;
    }

    const BATCH = 100;
    let filed = 0;
    let late = 0;
    for (let at = 0; at < lines.length; at += BATCH) {
      const batch = lines.slice(at, at + BATCH);
      const reply = await attempt(
        () => admin({ what: 'count', counted_at_ms: Date.now(), lines: batch }, Date.now()),
        null,
      );
      if (!reply) {
        // What went is gone from the sheet, and what did not is still in it.
        keepSheet();
        fault = `${fault ?? 'the shop did not take all of it'}. ${filed} counted so far, the rest is still here.`;
        await look(true);
        return;
      }
      // Sales rung before the count that reached the server after it. Nobody can
      // say whether the person counting saw those goods, so the server leaves
      // them out of the figure and says so rather than quietly picking a side.
      late += (reply.info?.on_hand ?? []).filter((entry) => entry.unreconciled_sales > 0).length;
      sheet = without(sheet, batch);
      filed += batch.length;
      keepSheet();
    }

    done = `${filed} ${filed === 1 ? 'shelf' : 'shelves'} counted.`;
    if (late > 0) {
      done = `${done} ${late} ${late === 1 ? 'item has' : 'items have'} sales that arrived after the count and are not in the figure.`;
    }
    if (counted.total === 0) sheet = null;
    keepSheet();
    await look(true);
  }

  /// Throw the sheet away.
  ///
  /// Two presses, because this is an afternoon of walking the shelves and a
  /// button that does it on one press will eventually be leant on. Not a browser
  /// dialog: those block the tab, and this screen is also driven by scripts.
  function abandonCount() {
    if (!abandoning) {
      abandoning = true;
      return;
    }
    abandoning = false;
    sheet = null;
    keepSheet();
    done = 'The count was thrown away.';
  }

  async function listPeople(quiet = true) {
    const reply = await attempt(() => run({ op: 'everyone' }), null, quiet);
    if (reply) everyone = reply.view?.everyone ?? [];
  }

  /// Suspend somebody, or let them back in.
  ///
  /// No PIN goes with it, because the back office does not have one: a PIN is
  /// hashed on the device where it is set and never travels. Taking the drawer
  /// away from a cashier should not require knowing their PIN.
  async function setSignIn(person, allowed) {
    await attempt(
      () =>
        admin(
          {
            what: 'amend_operator',
            id: person.id,
            name: person.name,
            // Theirs, sent back exactly as it came. Rebuilding it from a role
            // name would flatten anybody whose permissions do not match a
            // preset, and suspending somebody is no place to change what they
            // may do.
            permissions: permissionsOf(person),
            active: allowed,
          },
          Date.now(),
        ),
      // The time matters and is not immediate: a till re-reads the people every
      // ten minutes. Saying "cannot sign in any more" without it would be a
      // promise this does not keep, and the one time it matters is the one time
      // somebody is being locked out in a hurry.
      allowed
        ? `${person.name} can sign in again. Tills offer them within ten minutes.`
        : `${person.name} is suspended. Tills stop offering them within ten minutes, and their name still resolves on the sales they rang.`,
    );
    await listPeople();
  }

  async function listTills() {
    const reply = await attempt(() => admin({ what: 'terminals' }, Date.now()), null);
    if (reply?.info?.terminals) tills = reply.info.terminals;
  }

  /// A code for a till that already exists, so a device that lost its credential
  /// comes back as itself. Issuing a new till id instead would give it an empty
  /// ledger and strand whatever the old one had not sent.
  async function reissue(till) {
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'code',
            terminal_id: till.id,
            label: till.label,
            role: 1,
            valid_for_seconds: 900,
          },
          Date.now(),
        ),
      null,
    );
    issued = reply?.info?.issued_code ?? null;
    issuedFor = issued ? till.label : null;
  }

  async function issueCode() {
    const label = tillLabel.trim() || 'a till';
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'code',
            terminal_id: newId(),
            label,
            role: 1,
            valid_for_seconds: 900,
          },
          Date.now(),
        ),
      null,
    );
    // Shown once and never retrievable: the server keeps only its hash.
    issued = reply?.info?.issued_code ?? null;
    issuedFor = issued ? label : null;
    tillLabel = '';
    await listTills();
  }
</script>

<main>
  <h1>
    openpos back office
    <small>
      {syncing} &middot; catalogue read to {view?.catalogue_cursor ?? 0}
      {#if keeping === 'evictable'}
        &middot;
        <span class="warn" title="This browser would not promise to keep what this device holds">
          this browser may discard what is stored here
        </span>
      {:else if storage === 'memory'}
        &middot; <span class="warn">memory only: nothing survives a reload</span>
      {/if}
    </small>
  </h1>

  {#if !enrolled || refused}
    <section>
      {#if refused}
        <p class="fault" role="alert">
          The shop is refusing this device. Its access may have been withdrawn,
          or the server rebuilt. Nothing here will save until it is enrolled
          again with a new code.
        </p>
      {:else}
        <p>
          This device needs an owner's enrolment code. The server prints one when
          it starts, and an owner can issue more from here afterwards.
        </p>
      {/if}
      <div class="row">
        <input
          bind:value={code}
          placeholder="Enrolment code"
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); join(); } }}
          disabled={busy}
        />
        <button onclick={join} disabled={busy}>Enrol</button>
      </div>
    </section>
  {/if}

  {#if fault}<p class="fault" role="alert">{fault}</p>{/if}
  {#if done}<p class="done">{done}</p>{/if}

  {#if enrolled}
    <section>
      <h2>The shop</h2>
      <p class="why">What heads every receipt. A till cannot print without it.</p>
      <input bind:value={shopName} placeholder="Shop name" disabled={busy} />
      <input bind:value={shopBin} placeholder="BIN (leave empty if you have none)" disabled={busy} />
      <input bind:value={shopAddress} placeholder="Address" disabled={busy} />
      <input
        bind:value={shopWallets}
        placeholder="Wallets you take, separated by commas: bKash, Nagad"
        disabled={busy}
      />
      <label class="rule">
        When a basket asks for more than the shelf holds
        <select bind:value={shopStockRule} disabled={busy}>
          <option value="0">Sell it and say nothing</option>
          <option value="1">Sell it and warn the cashier</option>
          <option value="2">Refuse it until a supervisor allows it</option>
        </select>
      </label>
      <p class="why">
        Leave this at the first until your stock figures are worth trusting. A shop that has never
        counted holds none of everything here, and a till that refused on that basis is a till that
        cannot sell.
      </p>
      <button onclick={saveShop} disabled={busy}>Save the shop</button>
    </section>

    <section>
      <h2>People</h2>
      <p class="why">
        Nobody can sign in at a till until somebody is added here. A cashier
        rings sales; a supervisor can also refund, override a price and close
        the drawer.
      </p>
      <input bind:value={personName} placeholder="Name" disabled={busy} />
      <input
        bind:value={personPin}
        type="password"
        placeholder="PIN, four digits or more"
        inputmode="numeric"
        disabled={busy}
      />
      <select bind:value={personRole} disabled={busy}>
        <option value="cashier">Cashier</option>
        <option value="supervisor">Supervisor</option>
      </select>
      {#if editingPerson}
        <p class="why">
          Correcting {editingPerson.name}. Saving the correction leaves their PIN
          alone. To replace it, type a new one above and set it: a PIN cannot be
          read back from here or anywhere, which is why it can only be replaced.
        </p>
        <div class="row">
          <button onclick={amendPerson} disabled={busy}>Save the correction</button>
          <button onclick={setPin} disabled={busy}>Set a new PIN</button>
          <button class="quiet" onclick={newPerson} disabled={busy}>Leave them alone</button>
        </div>
      {:else}
        <button onclick={savePerson} disabled={busy}>Add them</button>
      {/if}

      {#if everyone.length > 0}
        <ul class="found">
          {#each everyone as person (person.id)}
            <li class:retired={!person.active}>
              <span class="name">
                {person.name}
                {#if twiceOver.has(fold(person.name))}
                  <!-- Shown only where it is needed. A shop with one Karim
                       should not be reading identifiers off a screen, and a
                       shop with two needs to know which is which here as well
                       as at the till. -->
                  &middot; {person.id.slice(-4)}
                {/if}
              </span>
              <span class="detail">
                {person.active ? 'can sign in' : 'suspended'}
              </span>
              <span class="acts">
                <button onclick={() => correctPerson(person)} disabled={busy}>Correct</button>
                {#if person.active}
                  <button class="quiet" onclick={() => setSignIn(person, false)} disabled={busy}>
                    Suspend
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setSignIn(person, true)} disabled={busy}>
                    Let them back in
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section>
      <h2>{editing ? 'Correcting an item' : 'Something to sell'}</h2>
      {#if editing}
        <p class="why">
          Saving changes this item everywhere. Tills pick it up on their next
          pull, and anything already rung keeps the price it was rung at.
        </p>
      {/if}
      <input bind:value={itemName} placeholder="Name" disabled={busy} />
      <input bind:value={itemNameBn} placeholder="The same in Bangla, if you want it" disabled={busy} />
      <div class="row">
        <input bind:value={itemPrice} placeholder="Price in taka" inputmode="decimal" disabled={busy} />
        <input bind:value={itemVat} placeholder="VAT %" inputmode="decimal" disabled={busy} />
      </div>
      <div class="row">
        <input bind:value={itemCode} placeholder="Code" disabled={busy} />
        <input bind:value={itemBarcode} placeholder="Barcode" inputmode="numeric" disabled={busy} />
        <input bind:value={itemUnit} placeholder="Sold by: Nos, kg, litre" disabled={busy} />
      </div>
      <label>
        <input type="checkbox" bind:checked={itemTaxIncluded} disabled={busy} />
        The price above already includes the tax, as it is written on the shelf
      </label>
      <label>
        <input type="checkbox" bind:checked={itemListedPrice} disabled={busy} />
        Tax is fixed to the listed price, so a discount comes out of your margin
        rather than reducing the tax
      </label>
      <div class="row">
        <button onclick={saveItem} disabled={busy}>
          {editing ? 'Save the correction' : 'Add it'}
        </button>
        {#if editing}
          <button class="quiet" onclick={startFresh} disabled={busy}>Leave it alone</button>
        {/if}
      </div>
    </section>

    {#if repairs.length > 0}
      <section>
        <h2>Sales needing somebody to look</h2>
        <p class="why">
          These are stored and counted in your takings until you say otherwise.
          They are here because the server could not accept them as they stood,
          and somebody has to say what happened. If a sale is real, keep it: the
          note records what you checked. If it never happened, say so, and it
          comes out of your takings, your tax, your stock and anything it put on
          somebody's account. Nothing is deleted either way, and you only get to
          answer once, so read it before you press.
        </p>
        <ul class="found">
          {#each repairs as entry (entry.id)}
            <li>
              <span class="name">
                {entry.receipt_no ?? 'No receipt number'} &middot; {money(entry.total_minor)}
              </span>
              <span class="detail">
                {entry.reason} &middot; reached the shop
                {new Date(entry.received_at_ms).toLocaleString('en-GB')}
              </span>
              <span class="stock">
                <input
                  placeholder="What you decided"
                  value={notes[entry.id] ?? ''}
                  oninput={(e) => (notes = { ...notes, [entry.id]: e.currentTarget.value })}
                  disabled={busy}
                />
                <button onclick={() => resolve(entry, true)} disabled={busy}>
                  It is a real sale
                </button>
                <button class="quiet" onclick={() => resolve(entry, false)} disabled={busy}>
                  It never happened
                </button>
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h2>What you have already decided</h2>
      <p class="why">
        An answered sale leaves the queue, so this is the way back to one you
        answered wrongly. Striking out the wrong sale takes a real debt off
        somebody's account, and putting it back puts the debt back with it. Both
        answers are kept, so the record shows that you changed your mind and
        why.
      </p>
      <button
        onclick={async () => {
          showDecided = !showDecided;
          if (showDecided) await listDecided(false);
        }}
        disabled={busy}
      >
        {showDecided ? 'Hide them' : 'Show what was decided'}
      </button>
      {#if showDecided}
        {#if decided.length === 0}
          <p class="why">Nothing has been decided yet.</p>
        {:else}
          <ul class="found">
            {#each decided as entry (entry.id)}
              <li>
                <span class="name">
                  {entry.receipt_no ?? 'No receipt number'} &middot; {money(entry.total_minor)}
                  &middot; {entry.kept ? 'counts' : 'struck out'}
                </span>
                <span class="detail">
                  "{entry.note}" &middot; {new Date(entry.decided_at_ms).toLocaleString('en-GB')}
                  {#if entry.decisions > 1}&middot; answered {entry.decisions} times{/if}
                </span>
                <span class="stock">
                  <input
                    placeholder="Why the answer is changing"
                    value={notes[entry.id] ?? ''}
                    oninput={(e) => (notes = { ...notes, [entry.id]: e.currentTarget.value })}
                    disabled={busy}
                  />
                  {#if entry.kept}
                    <button class="quiet" onclick={() => changeAnswer(entry, false)} disabled={busy}>
                      It never happened
                    </button>
                  {:else}
                    <button onclick={() => changeAnswer(entry, true)} disabled={busy}>
                      Put it back
                    </button>
                  {/if}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    {#if gaps.length > 0}
      <section>
        <h2>Where your numbering jumps</h2>
        <p class="why">
          Receipt numbers are meant to run unbroken, and this is where they do
          not. A gap is one of two things and only you can tell which: numbers
          rung on a till that has not synced yet, which close by themselves, or
          numbers that went with a device that was wiped or lost, which never
          will. Check the till against the list above, and if it has been quiet
          for days, that is your answer.
        </p>
        <ul class="found">
          {#each gaps as gap (gap.terminal + gap.after)}
            <li>
              <span class="name">
                {gap.after} &rarr; {gap.before}
                &middot; {gap.missing} {gap.missing === 1 ? 'number' : 'numbers'} missing
              </span>
              <span class="detail">
                {tills.find((till) => till.id === gap.terminal)?.label ?? 'a till this shop no longer lists'}
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h2>Sales carried in by hand</h2>
      <p class="why">
        For a till that cannot send: its terminal was removed, or it has to be
        enrolled again and would abandon what it is holding. On that device press
        "What is still on this device", then either save it to a file and open
        the file here, or paste what it shows. Line breaks a message added on the
        way do not matter. Every sale taken in this way goes into the list of
        sales needing somebody to look, because the usual proof of where a sale
        came from is what that device has lost.
      </p>
      <div class="row">
        <input type="file" accept=".txt,text/plain" onchange={openCarriedFile} disabled={busy} />
      </div>
      <textarea
        bind:value={carried}
        oninput={markCarried}
        rows="3"
        placeholder="Paste what the till showed you, or open the file above"
      ></textarea>
      {#if carriedMark}
        <p class="why">
          Mark <strong>{carriedMark}</strong>. The till that wrote this shows a mark too: if they
          differ, not all of it arrived, and taking it in would take in fewer sales than that device
          is holding.
        </p>
      {:else if carried.trim()}
        <p class="why">That is not a bundle. Check the whole of it was copied.</p>
      {/if}
      <button onclick={adoptCarried} disabled={busy}>Take them in</button>
    </section>

    {#if fromTills.length > 0}
      <!-- Above the ordinary sections for the same reason as the one below it:
           these are selling now, at a price nobody here has agreed to. -->
      <section>
        <h2>Items your tills wrote down</h2>
        <p class="why">
          Somebody at a counter scanned a barcode this shop had never seen, said
          what it was, and sold it rather than losing the sale. They are in the
          catalogue and in every report already. Correct what is wrong, or say
          it is right and the mark comes off.
        </p>
        <ul class="found">
          {#each fromTills as item (item.id)}
            <li>
              <span class="name">{item.name}</span>
              <span class="detail">
                {item.code} &middot; {money(item.price_minor)} &middot; VAT {item.vat_bp / 100}%
                {#if item.barcodes.length === 0}
                  &middot; no barcode: the shop already gave that code to something else
                {/if}
              </span>
              <button onclick={() => correct(item)} disabled={busy}>Correct it</button>
              <button onclick={() => agreeToItem(item)} disabled={busy}>It is right</button>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if unreadable.length > 0}
      <!-- Above the ordinary sections, because a price that never reached the
           tills is money going out at the wrong number every hour. -->
      <section>
        <h2>Price changes that never reached your tills</h2>
        <p class="why">
          Written by a version of this software that this one cannot read, so
          every till has passed over them and is selling at the price it had
          before. Set those prices again from "What is on the shelves" and they
          will go out in the ordinary way.
        </p>
        <ul class="found">
          {#each unreadable as change (change.seq)}
            <li>
              <span class="name">{names[change.item] ?? 'An item this device does not have a name for'}</span>
              <span class="detail">written by version {change.schema} of the catalogue format</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h2>What sold</h2>
      <p class="why">
        What left the shelves between two days, most first. This is what to
        order against: something given away at a discount still left the shelf
        and still has to be replaced. Returns are in it with their own sign.
      </p>
      <div class="row">
        <input type="date" bind:value={soldFrom} disabled={busy} />
        <input type="date" bind:value={soldTo} disabled={busy} />
        <button onclick={askSold} disabled={busy}>Look</button>
      </div>
      {#if waived.length > 0}
        <p class="why">
          <span class="late">
            {waived.length} {waived.length === 1 ? 'thing was' : 'things were'} allowed over a
            cashier's ceiling in that window.
          </span>
          A ceiling exists so that giving money away is somebody's decision
          rather than everybody's habit, which only means anything if the
          decisions can be looked at afterwards.
        </p>
        <ul class="found">
          {#each waived as one (one.sale + one.reason)}
            <li>
              <span class="name">{one.reason}</span>
              <span class="detail">
                {new Date(one.rung_at_ms).toLocaleString('en-GB')}
                &middot; on a sale of {money(one.total_minor)}
                &middot; {tills.find((till) => till.id === one.terminal)?.label ?? 'a till this shop no longer lists'}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
      {#if sold.length > 0}
        <ul class="found">
          {#each sold as row (row.item)}
            <li>
              <span class="name">{names[row.item] ?? 'Something this device does not have a name for'}</span>
              <span class="detail">
                {qty(row.qty_milli)} &middot; over {row.sales} {row.sales === 1 ? 'sale' : 'sales'}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section>
      <h2>What was allowed, and by whom</h2>
      <p class="why">
        Every discount over a ceiling, price typed over the catalogue's, refund,
        line taken off and drawer opened outside a sale, with who did it and who
        allowed it. A ceiling only means something if what got past it can be
        looked at afterwards, and until this existed the answer lived on the
        device and died when the tab closed.
      </p>
      <div class="row">
        <input type="date" bind:value={allowedFrom} disabled={busy} />
        <input type="date" bind:value={allowedTo} disabled={busy} />
        <button onclick={askAllowed} disabled={busy}>Look</button>
      </div>
      {#if allowedTrail.length > 0}
        <ul class="found">
          {#each allowedTrail as one (one.terminal + '/' + one.seq + '/' + one.at_ms)}
            <li>
              <span class="name">
                {one.what}{#if one.bp > 0} of {one.bp / 100}%{/if}
              </span>
              <span class="detail">
                {new Date(one.at_ms).toLocaleString('en-GB')}
                {#if one.refused}
                  &middot; on {one.operator_name || 'a name this device cannot read'}'s button
                {:else if one.took_the_till}
                  &middot; {one.operator_name || 'somebody this device cannot name'}
                {:else}
                  &middot; {one.operator_name || 'somebody this device cannot name'}
                  {#if one.authorised_by_name}
                    &middot; allowed by {one.authorised_by_name}
                  {:else}
                    &middot; their own permission covered it
                  {/if}
                {/if}
                &middot; {tills.find((till) => till.id === one.terminal)?.label ?? 'a till this shop no longer lists'}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section>
      <h2>What you owe the revenue</h2>
      <p class="why">
        What you sold at each rate in a month, and the tax on it. Worked out
        when each sale arrived rather than by reading a month of tickets, and by
        the day the goods were sold rather than the day a till got its sync in.
        Refunds are in it with their own sign.
      </p>
      <div class="row">
        <input type="month" bind:value={vatMonth} disabled={busy} />
        <button onclick={askVat} disabled={busy}>Look</button>
      </div>
      {#if vat.length > 0}
        <ul class="found">
          {#each vat as row (row.vat_bp)}
            <li>
              <span class="name">
                {(row.vat_bp / 100).toFixed(row.vat_bp % 100 ? 2 : 0)}%
              </span>
              <span class="detail">
                {money(row.net_minor)} sold &middot; {money(row.vat_minor)} tax
                &middot; {row.sales} {row.sales === 1 ? 'sale' : 'sales'}
              </span>
            </li>
          {/each}
        </ul>
        <p class="figure">{money(vat.reduce((sum, row) => sum + row.vat_minor, 0))}</p>
        <p class="why">Tax in all, for that month.</p>
        {#if vatWaiting.sales > 0}
          <p class="why">
            <span class="late">
              {money(vatWaiting.minor)} of that is {vatWaiting.sales}
              {vatWaiting.sales === 1 ? 'sale' : 'sales'} nobody has looked at yet.
            </span>
            They are in the figure, because goods may well have left the shop.
            Deal with them in "Sales needing somebody to look" before you file,
            and this line will go.
          </p>
        {/if}
      {/if}
    </section>

    <section>
      <h2>Who buys on account</h2>
      <p class="why">
        Writing somebody down is what keeps two people with one name apart. A
        sale that names one of these adds to that person's account whatever the
        cashier typed at the till, and every till is told the list so a sale can
        be written with the internet down.
      </p>
      <input bind:value={buyerName} placeholder="Their name" />
      <input bind:value={buyerPhone} placeholder="Their phone, if you have it" />
      <input
        bind:value={buyerBin}
        placeholder="Their BIN, if they are a business"
      />
      <span class="row">
        <button onclick={saveBuyer} disabled={busy}>
          {editingBuyer ? 'Correct them' : 'Write them down'}
        </button>
        {#if editingBuyer}
          <button class="quiet" onclick={() => { editingBuyer = null; buyerName = ''; buyerPhone = ''; }}>
            Leave it
          </button>
        {/if}
      </span>
      {#if buyers.length > 0}
        <ul class="found">
          {#each buyers as buyer (buyer.id)}
            <li class:retired={!buyer.active}>
              <span class="name">{label(buyer, buyersTwiceOver)}</span>
              <span class="detail">
                {#if buyer.phone}{buyer.phone}{:else}no phone written down{/if}
                {#if !buyer.active}&middot; account stopped{/if}
              </span>
              <span class="acts">
                <button onclick={() => correctBuyer(buyer)} disabled={busy}>Correct it</button>
                {#if buyer.active}
                  <button class="quiet" onclick={() => setAccountAllowed(buyer, false)} disabled={busy}>
                    Stop their account
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setAccountAllowed(buyer, true)} disabled={busy}>
                    Let them again
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section>
      <h2>Who owes you</h2>
      <p class="why">
        What each person took on account and has not settled. It adds up the
        sales your tills rang on account and the payments you have taken since,
        so the notebook beside the till has nothing in it this does not.
      </p>
      {#if owing.length > 0}
        <ul class="found">
          {#each owing as person (person.person_key)}
            <li>
              <span class="name">{person.person_name}</span>
              <span class="detail">
                {#if person.owed_minor >= 0}
                  Owes {money(person.owed_minor)}
                {:else}
                  In credit {money(-person.owed_minor)}
                {/if}
                &middot; first entry {new Date(person.since_ms).toLocaleDateString('en-GB')}
                &middot; {person.entries} {person.entries === 1 ? 'entry' : 'entries'}
              </span>
              <span class="row">
                <input
                  placeholder="Taka they handed over"
                  bind:value={paying[person.person_key]}
                />
                <button onclick={() => takePayment(person)} disabled={busy}>Took payment</button>
                <button onclick={() => showAccount(person)} disabled={busy}>
                  {openAccount === person.person_key ? 'Hide' : 'What is this'}
                </button>
              </span>
              <span class="row">
                <input
                  placeholder="Or strike it off, and say why"
                  bind:value={writingOff[person.person_key]}
                />
                <button onclick={() => takePayment(person, true)} disabled={busy}>
                  Strike off
                </button>
              </span>
              {#if openAccount === person.person_key}
                <ul class="found">
                  {#each accountLines as line (line.source)}
                    <li>
                      <span class="detail">
                        {new Date(line.at_ms).toLocaleString('en-GB')}
                        &middot; {line.is_sale
                          ? line.amount_minor < 0
                            ? 'brought goods back'
                            : 'took goods'
                          : line.written_off
                            ? 'struck off'
                            : 'paid'}
                        {money(Math.abs(line.amount_minor))}
                        {#if line.note}&middot; {line.note}{/if}
                      </span>
                    </li>
                  {/each}
                </ul>
                {#if !accountComplete}
                  <button class="quiet" onclick={() => readAccount(person, true)} disabled={busy}>
                    Show older entries
                  </button>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>
        {#if !owedComplete}
          <button class="quiet" onclick={() => listOwed(false, true)} disabled={busy}>
            Show more people
          </button>
        {/if}
      {:else}
        <p class="why">Nobody owes you anything, or nothing has been rung on account yet.</p>
      {/if}
    </section>

    <section>
      <h2>Drawers open now</h2>
      <p class="why">
        What each till says its drawer holds while it is still open, and when it
        last said so. A drawer nobody closes is never counted, and until a till
        reports one there is nothing to look at but the till itself.
      </p>
      {#if openDrawers.length > 0}
        <ul class="found">
          {#each openDrawers as drawer (drawer.terminal)}
            <li>
              <span class="name">
                {tills.find((till) => till.id === drawer.terminal)?.label ?? 'A till this shop no longer lists'}
              </span>
              <span class="detail">
                Open since {new Date(drawer.opened_at_ms).toLocaleString('en-GB')}
                &middot; {drawer.sales} {drawer.sales === 1 ? 'sale' : 'sales'}
                &middot; should hold {money(drawer.expected_cash_minor)}
              </span>
              <span class="detail">
                As that till said at {new Date(drawer.reported_at_ms).toLocaleString('en-GB')}.
              </span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">No till has a drawer open.</p>
      {/if}
    </section>

    <section>
      <h2>Drawers counted</h2>
      <p class="why">
        What each till expected to hold at closing, what was in it, and the
        difference. A drawer that is short is a fact to look at, not an error:
        one that could not be closed short would be closed dishonestly instead.
      </p>
      {#if drawers.length > 0}
        <ul class="found">
          {#each drawers as drawer (drawer.id)}
            <li class:retired={drawer.variance_minor !== 0}>
              <span class="name">
                {tills.find((till) => till.id === drawer.terminal)?.label ?? 'A till this shop no longer lists'}
                &middot; {new Date(drawer.closed_at_ms).toLocaleString('en-GB')}
                {#if drawer.closed_by_name}
                  &middot; counted by {drawer.closed_by_name}
                {/if}
              </span>
              <span class="detail">
                {drawer.sales} {drawer.sales === 1 ? 'sale' : 'sales'}
                &middot; float {money(drawer.opening_float_minor)}
                &middot; expected {money(drawer.expected_cash_minor)}
                &middot; counted {money(drawer.counted_cash_minor)}
              </span>
              <span class="detail">
                {#if drawer.variance_minor === 0}
                  It counted exactly.
                {:else if drawer.variance_minor < 0}
                  <span class="late">Short by {money(-drawer.variance_minor)}.</span>
                {:else}
                  <span class="late">Over by {money(drawer.variance_minor)}.</span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">No drawer has been counted and closed yet.</p>
      {/if}
    </section>

    <section>
      <h2>What you took</h2>
      <div class="row">
        <input type="date" bind:value={day} disabled={busy} />
        <button onclick={askTakings} disabled={busy}>Look</button>
      </div>
      {#if takings}
        {#if takings.sales === 0}
          <p class="why">Nothing rung on that day.</p>
        {:else}
          <p class="figure">{money(takings.total_minor)}</p>
          <p class="why">
            {takings.sales} {takings.sales === 1 ? 'sale' : 'sales'}
            {#if takings.refunds > 0}
              &middot; including {takings.refunds}
              {takings.refunds === 1 ? 'refund' : 'refunds'} of
              {money(-takings.refunded_minor)}, which are already in that figure
            {/if}
          </p>
          <p class="why">
            {#if takings.drawers_counted > 0}
              {takings.drawers_counted} {takings.drawers_counted === 1 ? 'drawer' : 'drawers'} counted
              &middot; expected {money(takings.expected_cash_minor)}
              &middot; counted {money(takings.counted_cash_minor)}
              {#if takings.variance_minor !== 0}
                &middot; <span class="late">
                  {takings.variance_minor < 0 ? 'short by' : 'over by'}
                  {money(Math.abs(takings.variance_minor))}
                </span>
              {/if}
            {:else}
              No drawer was counted that day.
            {/if}
          </p>
          {#if takings.drawers_counted > 0}
            <p class="why">
              A drawer's figures are what the till expected and what somebody
              counted that evening, and they stay as they were counted. Striking
              out a sale afterwards takes it out of the takings above and leaves
              these alone, on purpose: if that sale was rung and never happened,
              the cash was never there, and the shortfall the counter wrote down
              is the evidence of it. So these two can disagree, and the
              difference is the thing to read.
            </p>
          {/if}
          {#if takings.charged_minor !== 0 || takings.paid_minor !== 0 || takings.written_off_minor !== 0 || takings.returned_minor !== 0}
            <p class="why">
              {money(takings.charged_minor)} went on account
              {#if takings.returned_minor !== 0}
                &middot; {money(takings.returned_minor)} of it came back
              {/if}
              &middot; {money(takings.paid_minor)} was paid off
              {#if takings.written_off_minor !== 0}
                &middot; <span class="late">{money(takings.written_off_minor)} struck off</span>
              {/if}
            </p>
          {/if}
          <ul class="found">
            {#each takings.tills as one (one.terminal)}
              <li>
                <span class="name">
                  {tills.find((till) => till.id === one.terminal)?.label ?? 'A till this shop no longer lists'}
                </span>
                <span class="detail">
                  {one.sales} {one.sales === 1 ? 'sale' : 'sales'} &middot; {money(one.total_minor)}
                  {#if one.needing_attention > 0}
                    &middot; <span class="late">
                      {one.needing_attention} needing somebody to look
                    </span>
                  {/if}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    <section>
      <h2>What is on the shelves</h2>
      <p class="why">
        From this device's own copy of the catalogue, so it answers with the line
        down. Pick something to correct its price or its tax.
      </p>
      <div class="row">
        <input
          bind:value={hunt}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); look(); } }}
          placeholder="Name, code or the start of either"
          disabled={busy}
        />
        <button onclick={() => look()} disabled={busy}>Look</button>
      </div>
      <label>
        <input
          type="checkbox"
          bind:checked={showRetired}
          onchange={() => look()}
          disabled={busy}
        />
        Include things you have stopped selling
      </label>

      <div class="row">
        <button
          class={stockMode === 'receiving' ? '' : 'quiet'}
          onclick={() => { stockMode = stockMode === 'receiving' ? 'off' : 'receiving'; }}
          disabled={busy}
        >
          {stockMode === 'receiving' ? 'Stop booking in' : 'Book in a delivery'}
        </button>
        <button
          class={stockMode === 'counting' ? '' : 'quiet'}
          onclick={() => {
            stockMode = stockMode === 'counting' ? 'off' : 'counting';
            delivery = {};
            if (stockMode === 'counting' && !sheet) sheet = startSheet(Date.now());
          }}
          disabled={busy}
        >
          {stockMode === 'counting' ? 'Stop counting' : 'Count the shelves'}
        </button>
      </div>

      {#if stockMode === 'receiving'}
        <p class="why">
          What arrived, and what it cost you. A margin is measured against what
          these goods cost, not against the last price you paid.
        </p>
        <div class="row">
          <select bind:value={deliveredBy} disabled={busy}>
            <option value="">Who it came from, if you know</option>
            {#each suppliers.filter((one) => one.active) as one (one.id)}
              <option value={one.id}>{one.name}</option>
            {/each}
          </select>
          <input bind:value={reference} placeholder="Their challan or invoice number" disabled={busy} />
          <button onclick={bookDelivery} disabled={busy}>Book it in</button>
        </div>
      {:else if stockMode === 'counting'}
        <p class="why">
          What you found on the shelf. This replaces the running figure rather
          than adjusting it, which is how a number that has drifted gets fixed.
          What you type is kept on this device as you go, so you can search for
          the next shelf, close this, and come back to it.
        </p>
        <p class="why">
          {#if counted.total === 0}
            Nothing entered yet{#if sheet} &middot; started {new Date(sheet.started_at_ms).toLocaleString('en-GB')}{/if}.
          {:else}
            {counted.counted} {counted.counted === 1 ? 'shelf' : 'shelves'} entered
            {#if sheet} &middot; started {new Date(sheet.started_at_ms).toLocaleString('en-GB')}{/if}
            {#if counted.wrong > 0}
              &middot; <span class="late">{counted.wrong} {counted.wrong === 1 ? 'box does' : 'boxes do'} not hold a number yet</span>
            {/if}
          {/if}
        </p>
        <span class="row">
          <button onclick={bookCount} disabled={busy}>Record the count</button>
          <button class="quiet" onclick={abandonCount} disabled={busy}>
            {abandoning ? 'Press again to throw it away' : 'Throw it away'}
          </button>
        </span>
      {/if}
      {#if found.length > 0}
        <ul class="found">
          {#each found as item (item.id)}
            <li class:retired={!item.active}>
              <span class="name">{item.name}</span>
              <span class="detail">
                {item.code} &middot; {money(item.price_minor)}
                &middot; VAT {(item.vat_bp / 100).toFixed(item.vat_bp % 100 ? 2 : 0)}%
                {#if onHand[item.id]}
                  &middot; {qty(onHand[item.id].qty_milli)} on hand
                  {#if onHand[item.id].unreconciled_sales > 0}
                    &middot; <span class="late">
                      {qty(onHand[item.id].unreconciled_milli)} sold after the last count and not in that figure
                    </span>
                  {/if}
                {/if}
                {#if item.vat_on_undiscounted}&middot; taxed on the listed price{/if}
                {#if !item.active}&middot; no longer sold{/if}
              </span>
              {#if stockMode !== 'off'}
                <span class="stock">
                  {#if stockMode === 'receiving'}
                    <input
                      placeholder="How many came"
                      inputmode="decimal"
                      value={delivery[item.id]?.qty ?? ''}
                      oninput={(e) => setDelivery(item.id, 'qty', e.currentTarget.value)}
                      disabled={busy}
                    />
                    <input
                      placeholder="Cost each"
                      inputmode="decimal"
                      value={delivery[item.id]?.cost ?? ''}
                      oninput={(e) => setDelivery(item.id, 'cost', e.currentTarget.value)}
                      disabled={busy}
                    />
                  {:else}
                    <input
                      placeholder="Counted, against {qty(onHand[item.id]?.qty_milli ?? 0)} on the books"
                      inputmode="decimal"
                      class={wrongLines.has(item.id) ? 'wrong' : ''}
                      value={sheet?.lines?.[item.id]?.typed ?? ''}
                      oninput={(e) => countShelf(item.id, e.currentTarget.value)}
                      disabled={busy}
                    />
                  {/if}
                </span>
              {/if}
              <span class="acts">
                <button onclick={() => correct(item)} disabled={busy}>Correct it</button>
                {#if item.active}
                  <button class="quiet" onclick={() => setSelling(item, false)} disabled={busy}>
                    Stop selling
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setSelling(item, true)} disabled={busy}>
                    Sell it again
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section>
      <h2>Who you buy from</h2>
      <p class="why">
        A delivery filed under a supplier can be queried when the goods or the
        invoice are wrong. One booked under nobody cannot.
      </p>
      {#if suppliers.length > 0}
        <ul class="found">
          {#each suppliers as one (one.id)}
            <li class:retired={!one.active}>
              <span class="name">{one.name}</span>
              <span class="detail">
                {one.phone ?? 'no phone'}{#if one.bin} &middot; BIN {one.bin}{/if}
                {#if !one.active}&middot; no longer bought from{/if}
              </span>
              <span class="acts">
                <button onclick={() => correctSupplier(one)} disabled={busy}>Correct</button>
                {#if one.active}
                  <button class="quiet" onclick={() => setBuying(one, false)} disabled={busy}>
                    Stop
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setBuying(one, true)} disabled={busy}>
                    Buy again
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
      <input bind:value={supplierName} placeholder="Name" disabled={busy} />
      <div class="row">
        <input bind:value={supplierPhone} placeholder="Phone" inputmode="tel" disabled={busy} />
        <input bind:value={supplierBin} placeholder="BIN, if they have one" disabled={busy} />
      </div>
      <div class="row">
        <button onclick={saveSupplier} disabled={busy}>
          {editingSupplier ? 'Save the correction' : 'Add them'}
        </button>
        {#if editingSupplier}
          <button class="quiet" onclick={newSupplier} disabled={busy}>Leave them alone</button>
        {/if}
      </div>
    </section>

    <section>
      <h2>What you owe your suppliers</h2>
      <p class="why">
        Everything booked in against a supplier, less what you have paid them.
        A delivery paid at the door is a delivery and a payment on the same day,
        which is what the paper says too. Nothing is stored as a balance: what
        anybody argues about is the deliveries, and they are listed below.
      </p>
      {#if supplierOwing.length > 0}
        <ul class="found">
          {#each supplierOwing as owing (owing.supplier)}
            <li>
              <span class="name">{owing.name || 'A supplier this shop no longer lists'}</span>
              <span class="detail">
                {#if owing.owed_minor >= 0}
                  You owe {money(owing.owed_minor)}
                {:else}
                  Paid ahead by {money(-owing.owed_minor)}
                {/if}
                &middot; {owing.deliveries} {owing.deliveries === 1 ? 'delivery' : 'deliveries'}
                &middot; since {new Date(owing.since_ms).toLocaleDateString('en-GB')}
              </span>
              <span class="row">
                <input
                  placeholder="Taka you handed over"
                  bind:value={payingSupplier[owing.supplier]}
                />
                <button onclick={() => paySupplier(owing)} disabled={busy}>Paid them</button>
                <button onclick={() => showStatement(owing)} disabled={busy}>
                  {statementFor === owing.supplier ? 'Hide' : 'What is this'}
                </button>
              </span>
              {#if statementFor === owing.supplier}
                <ul class="found">
                  {#each statement as line (line.at_ms + String(line.delivered) + line.amount_minor)}
                    <li>
                      <span class="detail">
                        {new Date(line.at_ms).toLocaleDateString('en-GB')}
                        &middot; {line.delivered ? 'goods in' : 'paid'}
                        {money(line.amount_minor)}
                        {#if line.reference}&middot; {line.reference}{/if}
                      </span>
                    </li>
                  {/each}
                </ul>
                {#if !accountComplete}
                  <button class="quiet" onclick={() => readAccount(person, true)} disabled={busy}>
                    Show older entries
                  </button>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">You owe your suppliers nothing, or nothing has been booked in against one.</p>
      {/if}
    </section>

    <section>
      <h2>What came in</h2>
      <p class="why">
        The last twenty deliveries, newest first. This is what a challan number
        is for: the goods and the invoice can be put side by side.
      </p>
      {#if deliveries.length > 0}
        <ul class="found">
          {#each deliveries as one (one.id)}
            <li>
              <span class="name">
                {suppliers.find((who) => who.id === one.supplier_id)?.name ?? 'Nobody recorded'}
                {#if one.reference} &middot; {one.reference}{/if}
              </span>
              <span class="detail">
                {new Date(one.received_at_ms).toLocaleString('en-GB')}
                &middot; {one.lines.length} {one.lines.length === 1 ? 'line' : 'lines'}
                &middot; {money(one.lines.reduce((total, line) => total + Math.round((line.qty_milli * line.unit_cost_minor) / 1000), 0))}
              </span>
              <span class="detail">
                {one.lines
                  .map((line) => `${qty(line.qty_milli)} × ${names[line.item_id] ?? 'an item this device does not hold'}`)
                  .join(', ')}
              </span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">Nothing booked in yet.</p>
      {/if}
    </section>

    <section>
      <h2>Tills</h2>
      <p class="why">
        A code lasts an hour and works once. Read it onto the device.
      </p>

      {#if tills.length > 0}
        <ul class="tills">
          {#each tills as till (till.id)}
            <li>
              <!-- A till enrolled before labels, or by something that did not
                   set one. Its id is worse than a name and better than a blank
                   row in a list whose whole purpose is telling them apart. -->
              <span class="name">{till.label || `Unnamed till ${till.id.slice(-6)}`}</span>
              <span class="seen">
                {#if till.last_seen_ms}
                  last heard {new Date(till.last_seen_ms).toLocaleString('en-GB')}
                {:else}
                  not heard from
                {/if}
                &middot; {till.sales} {till.sales === 1 ? 'sale' : 'sales'}
                {#if till.open_repairs > 0}&middot; {till.open_repairs} to look at{/if}
              </span>
              <!-- For a device that lost its credential. A new till id would
                   give it an empty ledger and strand anything it had not sent. -->
              <button onclick={() => reissue(till)} disabled={busy}>Code for this till</button>
              <!-- For a device that is gone. Two presses, because one press
                   stops a working till in the middle of a trading day. -->
              <button class="quiet" onclick={() => cutOff(till)} disabled={busy}>
                {cuttingOff === till.id ? 'Press again: this stops it dead' : 'This one is lost'}
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">No tills yet.</p>
      {/if}

      <div class="row">
        <input bind:value={tillLabel} placeholder="Name a new till" disabled={busy} />
        <button onclick={issueCode} disabled={busy}>Add a till</button>
      </div>
      {#if issued}
        <p class="code">{issued}</p>
        <p class="why">
          For {issuedFor}. Shown once. Nobody can read it back, not even from here.
        </p>
      {/if}
    </section>
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    font: 16px/1.45 system-ui, sans-serif;
    background: #f6f6f4;
    color: #16150f;
  }
  main { max-width: 40rem; margin: 0 auto; padding: 1rem 1rem 3rem; }
  h1 { font-size: 1.2rem; letter-spacing: 0.02em; }
  h1 small { font-weight: 400; font-size: 0.75rem; color: #5a574a; }
  h2 { font-size: 1rem; margin: 0 0 0.25rem; }
  section {
    background: #fff; border: 1px solid #cfccbf; border-radius: 6px;
    padding: 0.9rem; margin-bottom: 1rem; display: grid; gap: 0.5rem;
  }
  .why { margin: 0; font-size: 0.85rem; color: #5a574a; }
  .rule { display: grid; gap: 0.35rem; font-size: 0.9rem; color: #3d3a30; }
  .row { display: flex; gap: 0.5rem; }
  input[type='text'], input:not([type]), input[type='password'], select {
    font: inherit; padding: 0.6rem 0.7rem; width: 100%; box-sizing: border-box;
    border: 1px solid #cfccbf; border-radius: 6px; background: #fff;
  }
  label { display: flex; gap: 0.5rem; align-items: flex-start; font-size: 0.85rem; color: #5a574a; }
  label input { width: auto; }
  button {
    font: inherit; padding: 0.6rem 1rem; border-radius: 6px; cursor: pointer;
    border: 1px solid #16150f; background: #16150f; color: #fff; justify-self: start;
  }
  button:disabled { opacity: 0.45; cursor: not-allowed; }
  .fault {
    background: #fdeceb; border: 1px solid #e6b5b0; color: #8a2018;
    padding: 0.6rem 0.75rem; border-radius: 6px;
  }
  .done {
    background: #eaf5ec; border: 1px solid #b3d6bd; color: #1d6b3a;
    padding: 0.6rem 0.75rem; border-radius: 6px;
  }
  .found { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
  .found li {
    display: grid; grid-template-columns: 1fr auto; gap: 0.25rem 0.75rem;
    align-items: center; padding: 0.5rem 0; border-bottom: 1px solid #e6e3d8;
  }
  .found .name { font-weight: 600; }
  .found .detail { grid-column: 1; font-size: 0.8rem; color: #5a574a; }
  .found .stock { grid-column: 1 / -1; display: flex; gap: 0.5rem; padding-top: 0.4rem; }
  .found .stock input { width: 12rem; padding: 0.5rem 0.6rem; }
  .found .acts { grid-row: 1 / 3; grid-column: 2; display: flex; gap: 0.4rem; }
  .found .acts button { padding: 0.45rem 0.7rem; font-size: 0.9rem; }
  .found .late { color: #7a5a1e; }
  /* A box holding something that is not a quantity. Marked rather than
     corrected: it is somebody mid-keystroke or a typo they will come back to,
     and a screen that fixes it for them books a number nobody counted. */
  .stock input.wrong { border-color: #a4442f; }
  .figure { font-size: 2rem; font-weight: 700; margin: 0; font-variant-numeric: tabular-nums; }
  .found li.retired .name { color: #8a877a; text-decoration: line-through; }
  .quiet { background: #fff; color: #16150f; border-color: #cfccbf; }
  .tills { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
  .tills li {
    /* A column each for the two buttons. Both were placed in column 2 and the
       second was drawn over the first, so the way to give a device that lost
       its credential a new code was a button nobody could press, under the one
       that stops a till dead. */
    display: grid; grid-template-columns: 1fr auto auto; gap: 0.25rem 0.75rem;
    align-items: center; padding: 0.5rem 0; border-bottom: 1px solid #e6e3d8;
  }
  .tills .name { grid-column: 1; grid-row: 1; font-weight: 600; }
  .tills .seen { grid-column: 1; grid-row: 2; font-size: 0.8rem; color: #5a574a; }
  /* Placed rather than left to flow: the name is what a person reads first and
     belongs on the left, and the two buttons each need a column of their own. */
  .tills button { grid-row: 1 / 3; padding: 0.45rem 0.7rem; font-size: 0.9rem; }
  .tills button:first-of-type { grid-column: 2; }
  .tills button:last-of-type { grid-column: 3; }
  .code {
    font: 1.6rem ui-monospace, Menlo, monospace; letter-spacing: 0.15em;
    margin: 0; padding: 0.5rem 0;
  }
</style>
