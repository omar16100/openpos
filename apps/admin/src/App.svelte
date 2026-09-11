<script>
  import { onMount, tick } from 'svelte';
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
    rolesOffered,
    whyTheRoundFailed,
  } from './till.js';
  import { money, qty } from './format.js';
  // Panels. A screen this size stopped fitting in one file long ago, and a
  // file nobody can hold in their head is a file every change is made blind
  // in. What comes out first is what is most nearly self-contained.
  import Drawers from './panels/drawers.svelte';
  import Periods from './panels/periods.svelte';
  import AnItem from './panels/an_item.svelte';
  import CatalogueFile from './panels/catalogue_file.svelte';
  import Repairs from './panels/repairs.svelte';
  import Selling from './panels/selling.svelte';
  import Takings from './panels/takings.svelte';
  import Accounts from './panels/accounts.svelte';
  import Suppliers from './panels/suppliers.svelte';
  import Tills from './panels/tills.svelte';
  import { LANGUAGES, worded, wordedRefusal } from '../../shared/words.js';
  import { alreadyOpenHere, whatElseToTry } from '../../shared/storage_trouble.js';
  import { keepACopy } from '../../shared/keep_a_copy.js';
  import { today } from '../../shared/days.js';
  // Money typed by a person, turned into integer poisha. Tested there, because
  // `Number()` accepts "1e3" and this is the one box on the screen that is money.
  import { minorFrom } from '../../shared/money.js';
  import { idForThisOne, whatIsOnTheForm } from '../../shared/one_id.js';
  // Reading a shelf label with the tablet's own camera, which is what this
  // screen is carried around the shop for.
  import { CANNOT_READ_HERE, readFromCamera } from '../../shared/camera_read.js';
  import { repriced } from '../../shared/repricing.js';
  // Telling two people with the same name apart, shared with the till so the
  // mark on a person is the same in both places.
  import { fold, nameTaken, shared } from '../../shared/people.js';
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
  /// Which language this screen shows. Its own key rather than the till's,
  /// because the two apps share an origin and a shopkeeper may well want the
  /// counter in Bangla and this in English, or the other way about.
  const LANGUAGE = 'openpos.admin.language';
  let language = $state(localStorage.getItem(LANGUAGE) ?? 'en');
  /// What to say, worded when it is read rather than when it is said. See the
  /// till's copy: a message assigned as a sentence keeps the language of the
  /// moment it went wrong, which is the one line on the screen that will not
  /// follow when a shopkeeper switches language to read it.
  const t = (key, fill, otherwise) => worded(() => language, key, fill, otherwise);
  /// The same, for a refusal the shop gave.
  const refusal = (view) => wordedRefusal(() => language, view);
  function speak(next) {
    language = next;
    localStorage.setItem(LANGUAGE, next);
  }

  let view = $state(null);
  let fault = $state(null);
  let done = $state(null);
  let busy = $state(false);
  // What the sync loop last did. A back office that cannot say what it is doing
  // is one where a change that never arrives looks like a change that never
  // saved, which cost an hour of looking at the wrong end of it.
  let syncing = $state(t('sync.starting'));
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
  /// The id the person being added will be written down under, kept while the
  /// form still describes them. See the till's basket id: a fresh one at each
  /// press is two people with one name where the shop could not tell which of
  /// them rang what, and a kept one over a changed form renames the first.
  let personId = $state(null);
  let personRole = $state('cashier');
  // What each role means, asked of the core rather than written here. This
  // screen held its own copy and the two disagreed: its cashier could open the
  // drawer and the core's could not, its supervisor was capped at a fifth off
  // and the core's at everything. Nothing a shop ran was inconsistent, because
  // every caller of the core's pair was a test, and nothing would have
  // complained until the first one that was not.
  //
  // Empty until the worker answers, and saving is refused until it has. What
  // would go otherwise is a request with no permissions field at all, which the
  // core refuses to decode: the shop would be told the save failed, on a screen
  // where the person had every reason to think it should have worked.
  let roles = $state({});
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
  // Where the item being corrected stood when it was read, so a save built on a
  // copy somebody else has since changed is refused rather than merged.
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
  // The supplier being corrected, and null when this is a new one. Without it
  // every save minted a fresh id, so fixing a phone number put a second copy of
  // the supplier in the list: the same bug the catalogue had.
  // What came in lately. Read back, because a delivery filed under a supplier is
  // only worth filing if somebody can ask which goods came on which challan.
  // Every item this device knows, by id, for naming goods on a delivery. The
  // search results are not enough: a delivery names whatever was received, and
  // that is rarely what is on the screen at the time.
  let names = $state({});
  /// Where the catalogue stood when those names were read.
  ///
  /// Names come from this device's own copy, and that copy fills in after the
  /// screen has already drawn: a back office that had just enrolled listed the
  /// shop's own delivery as seven of "an item this device does not hold" and
  /// stayed that way until somebody reloaded the page. Null means never read.
  let namesAt = $state(null);
  // And what the shop sorts each of them under, for reading a month's selling
  // by kind rather than as one long list of items.
  let kinds = $state({});
  // And what the shop pays for each, for valuing what is not moving. What it
  // hopes to sell for is not money it has.
  let costs = $state({});
  // The panels this screen is made of, held so that it can ask each of them to
  // read what it shows. Each owns its own state; what they are handed is the
  // way to ask the shop, the words, the money, and the one message line.
  /// The drawer panel, which holds its own two lists. Held so the screen can
  /// ask it to load them: see panels/drawers.svelte.
  let drawerPanel = $state(null);
  /// The supplier panel, which holds what the shop owes and what has come in.
  /// Held so the screen and the delivery form can ask it to read them back.
  let supplierPanel = $state(null);
  /// The accounts panel, which holds who buys on account and what they owe.
  let accountPanel = $state(null);
  /// The repair panel, which holds everything that needs a person to look.
  let repairPanel = $state(null);
  /// The takings panel, which holds the day it is showing.
  let takingsPanel = $state(null);
  /// The panel that takes the shop's list out and brings it back.
  let filePanel = $state(null);
  /// The form one item is added or corrected on.
  let itemPanel = $state(null);
  // What moved off the shelves over a period, which is what a shop orders
  // against. Named here from the catalogue this device already holds.
  // How long the window those sales came from was, which is what turns a
  // quantity into a rate a shelf can be measured against.
  // How close to running out is worth walking to the wholesaler for. The shop's
  // own answer: it depends on when the supplier comes.
  // What is sitting there instead. Whether it is shown at all depends on the
  // shop having asked for the whole shelf rather than a page of it.
  let shelfIsWhole = $state(false);
  // What supervisors allowed over the same window, which is the other half of
  // reading a quiet week: what was sold, and what was given away.
  // What this device's own store is, and whether the browser promised to keep
  // it. Shown because a back office is the device most likely to be evicted:
  // it is opened once a week, and Safari discards an origin's storage after
  // seven days of not being opened.
  let storage = $state('opening');
  // Set when this device's own store would not open because it is already open
  // in another window here. Its own state rather than a reading of the fault
  // text, because the enrolment box hangs on it and a screen that decided by
  // reading its own sentence would stop deciding the day somebody improved the
  // wording, or the day the shop switched to Bangla.
  let openElsewhere = $state(false);
  /// How many times somebody has pressed "try again" and been told the same
  /// thing. See the till's copy: the first advice has a dead end in it.
  let triedTheLedgerAgain = $state(0);
  // The name of the last failure, beside the words it was said in.
  let lastFaultCode = null;
  let keeping = $state('unknown');
  // Whether the owner has already been told this name is taken. Told once, then
  // out of the way: a shop that means it presses again.
  let nameWarned = $state(false);
  const twiceOver = $derived(shared(everyone));
  // The same for the people who buy on account, where the cost of confusing two
  // of them is a balance that belongs to neither.
  // A week back by default: the question is usually about something that
  // happened recently and is remembered vaguely.
  // The till armed for cutting off, waiting for a second press.
  // Price changes no till could read. Empty is the ordinary answer, and the
  // section says nothing at all when it is.
  // Items a till wrote down at a counter, which nobody has agreed to yet.
  // Sales somebody read off a device that cannot send them, pasted in here.
  // One customer's account laid out for paper, when somebody asked for it.
  // Printing shows this and hides the rest of the page.
  let accountPaper = $state(null);
  // What is being paid, keyed by the folded name, so two people being settled
  // in the same minute do not share a box.
  // The id minted for the payment being typed, kept until it is recorded. A
  // fresh id on every press would defeat the whole point of minting one: a
  // reply that never arrived is exactly when somebody presses again, and the
  // second press must be the same payment rather than a second one.
  // Why a debt is being struck off. Required, because this is the one entry
  // here that makes money disappear.
  // Off, receiving a delivery, or counting a shelf. One at a time, because the
  // two put different numbers in the same box and a screen that offers both at
  // once is a screen where a count gets booked as a delivery.
  let stockMode = $state('off');
  // What is being written off, by item: how many are gone and why. Held while
  // it is typed, like a delivery, and cleared once the shop has it.
  let writeOff = $state({});
  // Moving a lot of prices at once, which is what a shop does when the
  // wholesaler moves. Typed as a percentage, read as a list, and written only
  // when somebody has read it.
  let movePercent = $state('');
  const moving = $derived(repriced(found, Number(movePercent)));
  // What the shop believes it holds, keyed by item id. Asked for separately from
  // the catalogue, because a sale is not a catalogue change: the figure on an
  // item record is whatever it was when somebody last edited that item, and
  // showing it as stock shows a number that never moves.
  let onHand = $state({});

  function setDelivery(id, field, value) {
    delivery = { ...delivery, [id]: { ...(delivery[id] ?? {}), [field]: value } };
  }
  let found = $state([]);
  /// What sold, under the words the shop sorts its shelves by. One group when
  /// nothing is sorted, which is the first day and is not a fault. Quantities
  /// are not added up across a group: a kilo and a bar of soap are not four of
  /// anything, and a number nobody can act on is worse than no number.

  /// The words the shop already uses, so a second bag of rice is sorted under
  /// the same word as the first rather than under "Rice " with a space.
  let categories = $derived(
    [...new Set(found.map((item) => (item.category ?? '').trim()).filter(Boolean))].sort(),
  );
  let hunt = $state('');
  // The same thing in Bangla, for the people who read the screens. It has been
  // carried by the catalogue and indexed by the search since both were written,
  // and nothing could set it: every item's Bangla name was a copy of its
  // English one.
  /// A spreadsheet that has been read but not yet written: its name, and every
  /// row with what is wrong with it and whether the shop already sells it. Null
  /// until somebody chooses a file, because nothing here writes anything until
  /// they have looked at it.
  /// The most items this device will read in one answer when it matches a file
  /// against the shop. A shop larger than this is told so rather than matched
  /// against part of itself, because everything past the ceiling would look new
  /// and come back as a second copy of the shop.
  const MOST_ITEMS = 5000;
  /// The rate to give a row whose file says nothing about tax.
  ///
  /// Its own box rather than borrowed from the form above, which is what it was
  /// at first: an owner who had cleared that box would have imported a whole
  /// catalogue at nothing per cent and under-declared every sale of it, with no
  /// screen anywhere saying so.
  /// How far through the writing it is, so a shop importing eight hundred lines
  /// sees something move rather than a page that has stopped.
  /// Whether this device has pulled the shop's catalogue to the end. Two
  /// separate facts because they fail differently: a device that has never
  /// synced knows nothing, and one still pulling knows part.
  /// Which withdrawn item has been asked to be deleted once. The second press
  /// is the one that does it.
  let removing = $state(null);
  let everSynced = $state(false);
  /// Whether the last round got through at all. A device that cannot reach the
  /// shop is not behind, it is stopped, and the two need different sentences.
  let reaching = $state(true);
  let moreToPull = $state(true);
  /// A build downloaded and waiting for a moment nobody is mid-count.
  let newBuildWaiting = $state(false);
  /// Where somebody can jump to, taken from the sections that are on the page.
  ///
  /// The back office is nine screenfuls and twenty-two sections, and a
  /// shopkeeper wanting to see who owes them money scrolled past thirteen
  /// things they were not looking for. This is the shortest fix that is not a
  /// lie: the page keeps its order, and there is a way to get down it.
  ///
  /// Read off the page rather than written out here, because a hand-written
  /// list of sections is a list that goes stale the first time somebody adds
  /// one, and the symptom is a menu that quietly stops mentioning a thing the
  /// shop can do. The headings are already translated, so this costs no words.
  let jumps = $state([]);
  /// What `jumps` last held, as plain text and deliberately not state: see
  /// the effect below.
  let lastJumps = '';
  /// The page itself, bound rather than looked up. `querySelector` inside the
  /// effect returned null, so the watch below was never installed: the list
  /// was built once and then never again, and every section that appears only
  /// when a shop has something to show was missing from it.
  let page = $state(null);

  // The tills this shop already has. Kept here rather than in the panel that
  // lists them, because a drawer and a sale carried in by hand are both named
  // from it.
  let tills = $state([]);

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
        fault = refusal(reply.view);
        return null;
      }
      if (!quiet) done = said;
      return reply;
    } catch (error) {
      // Reported even when quiet: a refresh that failed is worth saying, and
      // the only message it can overwrite is one about the save it followed.
      //
      // Worded here rather than taken as it came. A refusal from the shop's own
      // server arrives with a name and its figures beside the English sentence,
      // and this is the point where the language is known. Anything with no
      // name, which is a browser that could not reach the shop at all, is its
      // own message and says itself.
      lastFaultCode = error.code ?? null;
      fault = refusal({
        error: error.message,
        error_code: error.code,
        error_parts: error.parts,
      });
      return null;
    } finally {
      busy = false;
    }
  }

  /// Open this device's own store, and say plainly if it could not be opened.
  ///
  /// The back office keeps a store like a till does, so it fails the same way:
  /// opened in two windows at once, the second one cannot take the files. It is
  /// the likelier of the two to be opened twice, because it is a page somebody
  /// leaves in a tab and comes back to.
  async function openTheLedger(known) {
    const reply = await attempt(() => open(known.tenant, known.terminal), null);
    view = reply?.view ?? view;
    storage = reply?.info?.storage ?? 'unavailable';
    keeping = reply?.info?.keeping ?? 'unknown';
    openElsewhere = !reply && alreadyOpenHere(lastFaultCode);
  }

  /// Try the store again, after whoever is there has closed the other window.
  async function openItAgain() {
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (!known) return;
    storage = 'opening';
    await openTheLedger(known);
    if (openElsewhere) triedTheLedgerAgain += 1;
    if (enrolled) await loadEverything();
  }

  /// Say every item to the tills again.
  ///
  /// For a shop whose tills passed over a change they could not read. Every
  /// item's current state goes back into the catalogue log, so a till picks it
  /// up on its next round. It costs nothing to press twice: a till applies the
  /// state it is given, and the state is what the shop already holds.
  async function sendTheListAgain() {
    const reply = await attempt(() => admin({ what: 'resend_catalogue' }, Date.now()), null);
    if (!reply) return;
    done = t('admin.list_sent_again', { count: reply.info?.resent ?? 0 });
    // Asked again straight after: the rows that could not be read are the ones
    // just sent, so the list either empties or says which are still beyond this
    // build, and a shop should not have to guess which happened.
    await repairPanel?.readAgain();
  }

  /// Recompute the list of jumps from the sections that are on the page.
  ///
  /// Written back only when it has actually changed, and compared against a
  /// plain variable rather than against `jumps` itself: reading the state this
  /// writes would make it depend on its own output, and it then either loops or
  /// never runs again. It never ran again.
  function findTheJumps() {
    const found = [];
    if (!page) return;
    const taken = new Set();
    for (const section of page.querySelectorAll(':scope > section')) {
      const heading = section.querySelector('h2');
      if (!heading) continue;
      const label = heading.textContent.trim();
      if (!label) continue;
      // Named for the heading rather than numbered by position. Numbering was
      // wrong in a way that hid itself: a section keeps the id it was given, so
      // when three more appeared later they were numbered by their new
      // positions and collided with sections that already held those numbers.
      // The list below is keyed on the id, and a keyed block with a repeated
      // key silently renders fewer things: twenty-three sections and twenty
      // ways down to them, with no error anywhere.
      const wanted = `at-${label.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')}`;
      let id = wanted;
      let again = 2;
      while (taken.has(id)) {
        id = `${wanted}-${again}`;
        again += 1;
      }
      taken.add(id);
      if (section.id !== id) section.id = id;
      found.push({ id, label });
    }
    const signature = found.map((one) => one.label).join('\u0000');
    if (signature !== lastJumps) {
      lastJumps = signature;
      jumps = found;
    }
  }

  // Watched rather than guessed at. Sections come and go with what the shop
  // has: "Items your tills wrote down" only exists once a till has written one
  // down, and the first version of this recomputed on enrolment and on the
  // language, so a section that appeared later never got a jump. Twenty-three
  // sections on the page and twenty ways down it, and the three missing were
  // the ones that only exist when a shop has something to look at.
  //
  // `childList` on `main` alone, without `subtree`: the only things that change
  // the set are sections being added and removed, and watching the whole tree
  // would fire on every keystroke in every box.
  $effect(() => {
    if (!page) return undefined;
    findTheJumps();
    const watch = new MutationObserver(findTheJumps);
    watch.observe(page, { childList: true });
    return () => watch.disconnect();
  });

  onMount(async () => {
    // The back office is opened once a week, which makes it the likeliest of
    // the two to be opened on the morning the line is down. It keeps a copy of
    // itself for the same reason the till does.
    keepACopy(
      () => ({ lines: 0, tendered: false, counting: stockMode !== 'off', unsent: 0 }),
      (waiting) => {
        newBuildWaiting = waiting;
      },
    );
    await connect(SERVER);
    // What each role means, from the core, before anybody can be added. Asked
    // once here rather than at every save: it is a fact about this build, and a
    // dropdown that had to wait for a round trip on press is a dropdown that
    // looks broken.
    const offered = await attempt(() => rolesOffered(), null, true);
    roles = offered?.info?.roles ?? {};
    // On its own store, like a till. A back office that forgot its credential
    // on every page load would have to be re-enrolled to change one price,
    // which is not a back office.
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (known) await openTheLedger(known);
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
        listTills(true);
        // A drawer open since this morning is the question this answers, and
        // the answer changes as tills report. Same cadence as the till list,
        // because they are read together.
        drawerPanel?.open(true);
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
      // English here until this screen learns the shop's language too. Said
      // through the same dictionary so there is one place the words live.
      const said = describeSync(round.info);
      syncing = round.ok
        ? t(said.key, said.fill)
        : t(whyTheRoundFailed(round.error, round.error_code) ?? 'sync.held_up', {
            why: round.error,
          });
      // What the import panel needs before it dares match a file against this
      // device's copy of the catalogue.
      //
      // A round is one of three things and they say different amounts. A pull
      // says outright whether more is waiting. A wait says the driver has
      // nothing left to do, which is only worth believing when nothing has been
      // failing: a device that cannot reach the shop also waits. Anything else
      // (a push, a shift) leaves what was already known alone.
      // Names again whenever the catalogue has moved under them. Every list on
      // this screen that shows an item shows a name read from this device's own
      // copy, and that copy grows after the screen has drawn.
      if (round.ok && !busy && (view?.catalogue_cursor ?? 0) !== namesAt) {
        learnNames();
      }
      // A round that waited because it is backing off after failures is `ok`
      // too. The same trap the till's "reached the shop" figure fell into: what
      // makes a device reachable is a round that got through, or a wait with
      // nothing failing behind it.
      reaching =
        round.ok &&
        (round.info?.did ? true : (round.info?.after_failures ?? 0) === 0);
      // Only when the round reached something, so this flag means what its name
      // says rather than being right by the order the gate happens to test in.
      if (reaching) {
        everSynced = true;
        const info = round.info ?? {};
        if (info.did === 'pull') moreToPull = info.more_to_pull ?? false;
        else if (!info.did) moreToPull = (info.after_failures ?? 0) !== 0;
      } else {
        moreToPull = true;
      }
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
    }, t('admin.this_device_enrolled'));
    // The same lock reaches this path: a second window of a device somebody is
    // setting up. Read from the name the failure carried rather than from the
    // sentence, and it hides the box that would otherwise tell them to enrol
    // again while their own store sits open behind another tab.
    openElsewhere = !view?.enrolled && alreadyOpenHere(lastFaultCode);
    if (view?.enrolled) {
      await loadEverything();
    }
  }


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
      fault = t('admin.say_shop_name');
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
      t('admin.shop_saved'),
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
    personId = null;
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
      fault = t('admin.say_pin');
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
      t('admin.new_pin_set', { name: editingPerson.name }),
    );
    if (!saved) return;
    newPerson();
    await listPeople();
  }

  /// Correct a name or what somebody may do, without their PIN.
  async function amendPerson() {
    if (!personName.trim()) {
      fault = t('admin.say_person_name');
      return;
    }
    if (!roles[personRole]) {
      // The core has not answered yet, or this build does not know that role.
      // Saving anyway would send no permissions at all, which adds somebody who
      // may do nothing and looks on every screen like an ordinary cashier.
      fault = t('admin.roles_not_ready');
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
      t('admin.person_corrected', { name: personName.trim() }),
    );
    if (!saved) return;
    newPerson();
    await listPeople();
  }

  async function savePerson() {
    if (!personName.trim() || personPin.length < 4) {
      fault = t('admin.say_name_and_pin');
      return;
    }
    // Two people called Karim make two identical buttons at every till, and a
    // cashier who presses the wrong one hands that whole shift to somebody
    // else. Said once, and then allowed: a shop can have two Karims, and the
    // answer is a name that tells them apart rather than a form that refuses.
    if (nameTaken(everyone, personName) && !nameWarned) {
      nameWarned = true;
      fault = t('admin.name_already_signs_in');
      return;
    }
    nameWarned = false;
    if (!roles[personRole]) {
      fault = t('admin.roles_not_ready');
      return;
    }
    // The id belongs to the person on the form. Pressing again after a reply
    // went missing sends the same one, which the shop reads as the repeat it
    // is; changing the form first makes it somebody else, because the shop
    // upserts on this id and adding Amina, losing the reply and typing Rahima
    // over the same form would rename Amina rather than add anybody.
    personId = idForThisOne(
      personId,
      whatIsOnTheForm(personName.trim(), personPin, personRole),
      newId,
    );
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'operator',
            id: personId.id,
            name: personName.trim(),
            pin: personPin,
            salt: newSalt(),
            permissions: roles[personRole],
            active: true,
          },
          Date.now(),
        ),
      t('admin.person_added', { name: personName.trim() }),
    );
    // Only when it worked. The form was emptied whatever happened, so an owner
    // whose shop could not be reached watched the name and the PIN they had
    // just chosen disappear, and a PIN is chosen rather than remembered.
    if (!saved) return;
    personId = null;
    newPerson();
    await listPeople();
  }

  /// Take a line off the books entirely.
  ///
  /// Offered only for something already withdrawn, so the ordinary act stays
  /// the ordinary one: a shop that wants an item off its tills stops selling
  /// it, and the record behind every figure survives. This is for the line
  /// typed by mistake, and the shop refuses it for anything it has traded.
  ///
  /// Two presses rather than a dialog. A browser dialog is a thing that blocks
  /// everything else on the page, and this is the one act here that cannot be
  /// undone.
  async function removeItem(item) {
    if (removing !== item.id) {
      removing = item.id;
      fault =
        t('admin.would_be_gone', { name: item.name });
      return;
    }
    removing = null;
    const gone = await attempt(
      () => admin({ what: 'delete_item', item_id: item.id }, Date.now()),
      t('admin.item_gone', { name: item.name }),
    );
    if (!gone) return;
    await look(true);
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
      fault = t('admin.item_already_withdrawn');
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
              // Everything the shop holds about this item, not the fields this
              // screen happens to show. What is left out is not left alone: it
              // arrives as the default and is saved over. Withdrawing an exempt
              // item and putting it back made it standard rated, because supply
              // was missing and nothing means standard; the shop's own word for
              // what shelf it belongs on went the same way, and so did its
              // Bangla name, which the layer below fills in from the English one
              // when it is empty.
              name_bn: held.name_bn,
              supply: held.supply,
              category: held.category,
              price_minor: held.price_minor,
              vat_bp: held.vat_bp,
              price_inclusive: held.price_inclusive,
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
        ? t('admin.item_on_sale_again', { name: item.name })
        : t('admin.item_withdrawn', { name: item.name }),
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
    await supplierPanel?.whatCameIn();
    await takingsPanel?.ask();
    await repairPanel?.queue();
    await drawerPanel?.counted();
    await accountPanel?.owed();
    await drawerPanel?.open();
    await accountPanel?.everybody();
    await supplierPanel?.owed();
    await repairPanel?.alsoTheRest();
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

  async function learnNames() {
    // Retired included: a delivery from last month can name something the shop
    // has since stopped selling, and "an item not on this page" is not an answer.
    // The whole catalogue, because every list on this screen names an item from
    // it: at five hundred, a shop of six hundred lines had a hundred items that
    // no report could name.
    const reply = await attempt(
      () => run({ op: 'catalogue', query: '', limit: MOST_ITEMS, retired: true }),
      null,
      true,
    );
    if (!reply) return;
    const map = {};
    const sorted = {};
    const paid = {};
    for (const item of reply.view?.catalogue ?? []) {
      map[item.id] = item.name;
      sorted[item.id] = (item.category ?? '').trim();
      paid[item.id] = item.cost_minor ?? 0;
    }
    names = map;
    kinds = sorted;
    costs = paid;
    namesAt = view?.catalogue_cursor ?? 0;
  }

  /// Who the shop buys from. Kept here rather than in the supplier panel,
  /// because the delivery form picks from the same list.
  async function listSuppliers(quiet = true) {
    const reply = await attempt(() => admin({ what: 'suppliers' }, Date.now()), null, quiet);
    if (reply) suppliers = reply.info?.suppliers ?? [];
  }

  /// Put a customer's statement on the page and print it.
  ///
  /// Here rather than in the panel that builds it, because what prints is the
  /// only thing on the page: the print rule hides `main`, and the panel is
  /// inside it. The wait is for the browser to lay the paper out before the
  /// dialog opens over it.
  async function printTheStatement(lines) {
    accountPaper = lines;
    if (!accountPaper) return;
    await new Promise((settle) => setTimeout(settle, 50));
    window.print();
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
    shelfIsWhole = Boolean(reply.info?.on_hand_whole);
  }

  /// Every shelf, for adding up what is sitting on them.
  ///
  /// An empty list asks the shop for all of it, and the answer says whether it
  /// managed all of it: a total added up from two hundred of eight hundred
  /// items is not the total, and a screen that shows it as one is lying
  /// quietly.
  async function askWholeShelf() {
    const reply = await attempt(
      () => admin({ what: 'on_hand', item_ids: [] }, Date.now()),
      null,
      true,
    );
    if (!reply) return;
    const figures = {};
    for (const entry of reply.info?.on_hand ?? []) figures[entry.item_id] = entry;
    onHand = figures;
    shelfIsWhole = Boolean(reply.info?.on_hand_whole);
  }

  function setWriteOff(itemId, field, value) {
    const held = writeOff[itemId] ?? { id: newId() };
    writeOff = { ...writeOff, [itemId]: { ...held, [field]: value } };
  }

  /// Goods gone, with the reason written down.
  ///
  /// A bottle of oil dropped, a bag of rice spoiled, something taken. Until now
  /// a shop had two ways to move a stock figure: sell it, or count the whole
  /// shelf. The route has existed since the week it was written and no screen
  /// could reach it, so a shop that broke something carried a wrong figure
  /// until its next count and had nowhere to say why.
  ///
  /// The reason is required, because an unexplained correction is
  /// indistinguishable from theft when somebody reads the variance a month
  /// later.
  async function writeItOff(item) {
    const row = writeOff[item.id] ?? {};
    const gone = Number(row.qty);
    const why = (row.reason ?? '').trim();
    if (!Number.isFinite(gone) || gone === 0) {
      fault = t('admin.say_how_many_gone');
      return;
    }
    if (!why) {
      fault = t('admin.say_why_gone');
      return;
    }
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'correct_stock',
            // Minted when the first key was pressed and kept with the row, so a
            // retry after a dropped reply is the same correction rather than a
            // second one.
            id: row.id,
            item_id: item.id,
            // Negative, because this button is for goods gone. A count that
            // read low is put right by counting again.
            qty_milli: -Math.round(Math.abs(gone) * 1000),
            reason: why,
            occurred_at_ms: Date.now(),
          },
          Date.now(),
        ),
      t('admin.written_off_line', { name: item.name, qty: Math.abs(gone), why }),
    );
    if (!saved) return;
    const rest = { ...writeOff };
    delete rest[item.id];
    writeOff = rest;
    await askStock([item]);
  }

  /// Write the prices somebody has just read.
  ///
  /// One save each, through the same door a single correction goes through, so
  /// a price moved in bulk is a price moved the ordinary way: the shop refuses
  /// any of them built on a copy somebody else has changed since, and says
  /// which.
  ///
  /// The list is what was on the screen. Nothing is recomputed here: agreeing
  /// to a list and having something else written is the failure this whole
  /// preview exists to prevent.
  async function moveThePrices() {
    if (moving.length === 0) return;
    const wanted = [...moving];
    let moved = 0;
    for (const row of wanted) {
      // Read fresh, exactly as correcting one price does. This page's copy is
      // up to half a minute old and holds every other field as well: saving
      // from it would carry a stale name or a withdrawn item back over
      // somebody else's work while only meaning to move a price.
      const reading = await attempt(
        () => admin({ what: 'item_now', item: row.id }, Date.now()),
        null,
        true,
      );
      const fresh = reading?.info?.item_now;
      if (!fresh) continue;
      const saved = await attempt(
        () =>
          admin(
            {
              what: 'item',
              // Where it stood a moment ago, so a price somebody else changed
              // while this list was being read is refused rather than
              // overwritten.
              expected_seq: reading?.info?.item_seq ?? 0,
              item: fresh,
              price_minor: row.now_minor,
              cost_minor: fresh.cost_minor ?? 0,
              active: fresh.active,
              vat_bp: fresh.vat_bp,
              price_inclusive: fresh.price_inclusive,
              vat_on_undiscounted: fresh.vat_on_undiscounted,
            },
            Date.now(),
          ),
        null,
        true,
      );
      if (saved) moved += 1;
    }
    movePercent = '';
    done =
      moved === wanted.length
        ? t('admin.prices_moved', { count: moved })
        : t('admin.some_prices_moved', { moved, wanted: wanted.length });
    await look();
  }

  /// The id this delivery will be booked under, kept while the form still
  /// describes it. See the till's basket id: the shop deduplicates on this, so
  /// a fresh one at each press means a dropped reply is booked twice, with the
  /// stock and what the shop owes its supplier counted twice with it, and a
  /// kept one over a changed form drops what was typed over it.
  let deliveryId = $state(null);

  async function bookDelivery() {
    // Read by the same two parsers the rest of the product uses, not by
    // Number() and a multiply. `Number("1e3")` is a thousand and
    // `Number("1.005") * 100` is 100.49999999999999, and both used to reach the
    // shop's stock and what it owes its supplier as though somebody had typed
    // them. A quantity or a cost that is not one is refused where it was typed.
    const lines = [];
    for (const [item_id, row] of Object.entries(delivery)) {
      const typed = String(row.qty ?? '').trim();
      if (typed === '') continue;
      const qty_milli = milliFrom(typed);
      if (qty_milli === null || qty_milli <= 0) {
        fault = t('admin.not_a_quantity', { typed });
        return;
      }
      const cost = String(row.cost ?? '').trim();
      const unit_cost_minor = cost === '' ? 0 : minorFrom(cost);
      if (unit_cost_minor === null) {
        fault = t('admin.not_a_cost', { typed: cost });
        return;
      }
      lines.push({ item_id, qty_milli, unit_cost_minor });
    }
    if (lines.length === 0) {
      fault = t('admin.nothing_to_book');
      return;
    }
    // The id belongs to this delivery, not to this screen. Pressing again after
    // a reply went missing sends the same one, which the shop reads as the
    // repeat it is; changing what is on the form first makes it a different
    // delivery, because the shop drops the lines of an id it already has and
    // the goods would be gone with them.
    deliveryId = idForThisOne(
      deliveryId,
      whatIsOnTheForm(lines, deliveredBy, reference.trim()),
      newId,
    );

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'receive',
            // Kept, so a dropped reply can be sent again without the goods
            // being counted twice: the shop deduplicates on this id, and a
            // fresh one at each press is a second delivery it cannot tell from
            // a real one. Minted at the first press rather than when the form
            // opens, because a form somebody opens and never books should not
            // burn an id.
            id: deliveryId.id,
            supplier_id: deliveredBy || null,
            reference: reference.trim() || null,
            received_at_ms: Date.now(),
            lines,
          },
          Date.now(),
        ),
      t('admin.lines_booked_in', { count: lines.length }),
    );
    if (!reply) return;
    if (reply.info?.already_booked) {
      done = t('admin.delivery_already_booked');
    }
    // Booked. The next delivery is a different one.
    deliveryId = null;
    delivery = {};
    reference = '';
    deliveredBy = '';
    await look(true);
    await supplierPanel?.whatCameIn();
    // And what the shop now owes for it. Without this a shopkeeper books six
    // hundred taka of goods from a named supplier, looks down the page at what
    // they owe, and reads "you owe your suppliers nothing": the figure was
    // right and only a reload showed it, so the reasonable conclusion is that
    // the delivery lost the supplier. Walked, and that is what it looked like.
    await supplierPanel?.owed(true);
  }

  /// The camera, while a shelf is being counted.
  ///
  /// Reading a label puts that item at the top of the list with its count box
  /// showing, and leaves the camera open: a person walking an aisle scans,
  /// types, scans the next one. Nothing is written by reading a label, which is
  /// the difference between this and the till: a count is a figure a person
  /// puts in, and a camera that typed one would be a camera counting the shop.
  let shelfCamera = $state(null);
  let scanningShelf = $state(false);
  let shelfReading = null;

  async function scanTheShelf() {
    if (scanningShelf) {
      stopScanningTheShelf();
      return;
    }
    scanningShelf = true;
    await tick();
    shelfReading = await readFromCamera({
      video: shelfCamera,
      // Kept open, because the next thing this person does is the next shelf.
      // The loop hands one label over once however long it sits in the frame,
      // so the box they are typing into is not pulled about while they type.
      keepLooking: true,
      onCode: (code) => putItAtTheTop(code),
      onTrouble: (why) => {
        scanningShelf = false;
        fault = why === CANNOT_READ_HERE ? t('admin.camera_not_here') : t('admin.camera_refused');
      },
    });
  }

  /// Put what was read at the top of the list, out of this device's own
  /// catalogue, so a shelf can be counted with the shop unreachable.
  async function putItAtTheTop(code) {
    const reply = await attempt(() => run({ op: 'check', code }), null, true);
    const item = reply?.view?.checked?.item;
    if (!item) {
      fault = t('admin.nothing_by_that_barcode');
      return;
    }
    found = [item, ...found.filter((one) => one.id !== item.id)];
    // What the shop believes is on that shelf, which is the figure the count is
    // typed against. Asked for this one item rather than the page, because the
    // person is standing in front of it.
    await askStock([item]);
  }

  /// A camera nobody is looking at reads nothing and costs the battery.
  $effect(() => {
    const stopIfHidden = () => {
      if (document.visibilityState !== 'visible' && scanningShelf) stopScanningTheShelf();
    };
    document.addEventListener('visibilitychange', stopIfHidden);
    return () => document.removeEventListener('visibilitychange', stopIfHidden);
  });

  function stopScanningTheShelf() {
    scanningShelf = false;
    shelfReading?.stop();
    shelfReading = null;
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
      // Which of the two it is matters to whoever is standing in the aisle: a
      // box holding something that is not a number is a typo to fix, and an
      // empty sheet is a count nobody has started. Both were English on a
      // screen a shop reads in Bangla.
      fault = counted.wrong > 0
        ? t('admin.a_box_holds_no_number')
        : t('admin.nothing_counted_yet');
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
        fault = t('admin.count_partly_filed', {
          why: fault ?? t('admin.shop_took_some'),
          count: filed,
        });
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

    done = t('admin.shelves_counted', { count: filed });
    if (late > 0) {
      done = `${done} ${t('admin.count_late_sales', { count: late })}`;
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
  /// Every way of leaving or entering the count sheet goes through here, so
  /// that the armed second press cannot be left lying about. It was possible to
  /// press "throw it away" once, walk off to book in a delivery, come back an
  /// afternoon later, and have the first press that looked innocent throw away
  /// the whole count.
  function goStockMode(next) {
    abandoning = false;
    // The camera goes with the mode it belongs to, or it reads shelves into a
    // list nobody is counting against.
    stopScanningTheShelf();
    stockMode = stockMode === next ? 'off' : next;
  }

  function abandonCount() {
    if (!abandoning) {
      abandoning = true;
      return;
    }
    abandoning = false;
    sheet = null;
    keepSheet();
    done = t('admin.count_thrown_away');
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
        ? t('admin.person_back', { name: person.name })
        : t('admin.person_suspended', { name: person.name }),
    );
    await listPeople();
  }

  /// The tills and what the shop last heard from each.
  ///
  /// Quiet when a timer asked for it. A refresh nobody pressed must not clear
  /// what is on the screen: this ran every fifteen seconds and wiped whatever
  /// the shop had just said, so a refusal telling somebody what to do instead
  /// had a life of fifteen seconds whether or not they had finished reading it.
  async function listTills(quiet = false) {
    const reply = await attempt(() => admin({ what: 'terminals' }, Date.now()), null, quiet);
    if (reply?.info?.terminals) tills = reply.info.terminals;
  }

</script>

<main bind:this={page}>
  <h1>
    {t('admin.title')}
    <small>
      {syncing}
      {#if !reaching}
        <!-- Only while it cannot reach the shop. A button offered when
             everything works is one somebody presses instead of trusting the
             loop, which is the opposite of what it is for. -->
        &middot;
        <button class="link" onclick={tryNow} disabled={busy}>{t('admin.try_now')}</button>
      {/if}
      &middot; {t('admin.catalogue_read_to', { cursor: view?.catalogue_cursor ?? 0 })}
      {#if keeping === 'evictable'}
        &middot;
        <span class="warn" title={t('admin.keep_not_promised')}>
          {t('admin.may_discard')}
        </span>
      {:else if storage === 'memory'}
        &middot; <span class="warn">{t('admin.memory_only')}</span>
      {/if}
      <!-- The other language, named in itself: somebody who cannot read this
           screen cannot be asked to find the word for their own language on
           it. -->
      &middot;
      <button class="link" onclick={() => speak(language === 'bn' ? 'en' : 'bn')} title={t('admin.language')}>
        {LANGUAGES.find((one) => one.code !== language)?.name}
      </button>
    </small>
  </h1>

  {#if jumps.length > 2}
    <!-- Sticky, because the reason it exists is that the page is nine
         screenfuls: a way down that you have to scroll back up to reach is not
         a way down. The names are the headings themselves, so they are already
         in the shop's language and cannot say something a section does not. -->
    <nav class="jumps" aria-label={t('admin.jump_to')}>
      {#each jumps as one (one.id)}
        <a href={`#${one.id}`}>{one.label}</a>
      {/each}
    </nav>
  {/if}

  {#if openElsewhere}
    <!-- Open in another window on this device, which is not a device that needs
         enrolling. The box below is hidden for exactly this case: enrolling
         again mints a second device against this shop while the one holding
         everything sits in a window nobody is looking at. -->
    <section>
      {#if whatElseToTry(triedTheLedgerAgain)}
        <!-- See the till's copy: the first advice can be a dead end, so the
             second one is offered once the first has been tried. -->
        <p class="fault">{t(whatElseToTry(triedTheLedgerAgain))}</p>
      {/if}
      <div class="row">
        <button onclick={openItAgain} disabled={busy}>{t('shared.try_again')}</button>
      </div>
    </section>
  {/if}

  {#if (!enrolled || refused) && !openElsewhere}
    <section>
      {#if refused}
        <p class="fault" role="alert">{t('admin.device_refused')}</p>
      {:else}
        <p>{t('admin.needs_a_code')}</p>
      {/if}
      <div class="row">
        <input
          bind:value={code}
          placeholder={t('admin.enrolment_code')}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); join(); } }}
          disabled={busy}
        />
        <button onclick={join} disabled={busy}>{t('admin.enrol')}</button>
      </div>
    </section>
  {/if}

  <!-- Over the page rather than at the top of it. This page is nine screenfuls,
       and a shopkeeper who pressed "Strike off" at the bottom of it was answered
       five thousand pixels above the fold: nothing appeared to happen, so the
       thing to do was press again. Measured before it was moved. -->
  {#if fault}<p class="fault floats" role="alert">{fault}</p>{/if}
  {#if done}<p class="done floats" role="status">{done}</p>{/if}

  {#if enrolled}
    <section>
      <h2>{t('admin.the_shop')}</h2>
      <p class="why">{t('admin.shop_why')}</p>
      <input bind:value={shopName} placeholder={t('admin.shop_name')} disabled={busy} />
      <input bind:value={shopBin} placeholder={t('admin.shop_bin')} disabled={busy} />
      <input bind:value={shopAddress} placeholder={t('admin.shop_address')} disabled={busy} />
      <input
        bind:value={shopWallets}
        placeholder={t('admin.shop_wallets')}
        disabled={busy}
      />
      <label class="rule">
        {t('admin.stock_rule')}
        <select bind:value={shopStockRule} disabled={busy}>
          <option value="0">{t('admin.stock_rule_allow')}</option>
          <option value="1">{t('admin.stock_rule_warn')}</option>
          <option value="2">{t('admin.stock_rule_block')}</option>
        </select>
      </label>
      <p class="why">{t('admin.stock_rule_why')}</p>
      {#if Number(shopStockRule) > 0}
        <!-- Only once they have asked for something, because it is about the
             wait between asking and seeing it happen at the counter. -->
        <p class="why">{t('admin.stock_rule_takes_a_while')}</p>
      {/if}
      <button onclick={saveShop} disabled={busy}>{t('admin.save_the_shop')}</button>
    </section>

    <section>
      <h2>{t('admin.people')}</h2>
      <p class="why">{t('admin.people_why')}</p>
      <input bind:value={personName} placeholder={t('admin.name')} disabled={busy} />
      <input
        bind:value={personPin}
        type="password"
        placeholder={t('admin.pin')}
        inputmode="numeric"
        disabled={busy}
      />
      <select bind:value={personRole} disabled={busy}>
        <option value="cashier">{t('admin.cashier')}</option>
        <option value="supervisor">{t('admin.supervisor')}</option>
      </select>
      {#if editingPerson}
        <p class="why">{t('admin.correcting_person', { name: editingPerson.name })}</p>
        <div class="row">
          <button onclick={amendPerson} disabled={busy}>{t('admin.save_the_correction')}</button>
          <button onclick={setPin} disabled={busy}>{t('admin.set_a_new_pin')}</button>
          <button class="quiet" onclick={newPerson} disabled={busy}>{t('admin.leave_them_alone')}</button>
        </div>
      {:else}
        <button onclick={savePerson} disabled={busy}>{t('admin.add_them')}</button>
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
                {person.active ? t('admin.can_sign_in') : t('admin.suspended')}
              </span>
              <span class="acts">
                <button onclick={() => correctPerson(person)} disabled={busy}>{t('admin.correct')}</button>
                {#if person.active}
                  <button class="quiet" onclick={() => setSignIn(person, false)} disabled={busy}>
                    {t('admin.suspend')}
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setSignIn(person, true)} disabled={busy}>
                    {t('admin.let_them_back_in')}
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <!-- Its own file: one item, added or corrected. The shelf list and the
         repair queue both open it, because correcting an item is the same act
         wherever it is started from. -->
    <AnItem
      bind:this={itemPanel}
      {t}
      {busy}
      {attempt}
      {admin}
      {newId}
      {categories}
      whatWentWrong={() => String(fault ?? '')}
      onSaved={() => look(true)}
      onWithdrawn={() => look(true)}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file: the shop's own list taken out and brought back, which is
         how a shop with eight hundred lines gets them in without typing. -->
    <CatalogueFile
      bind:this={filePanel}
      {t}
      {money}
      {refusal}
      {busy}
      setBusy={(held) => { busy = held; }}
      {attempt}
      {admin}
      {run}
      {newId}
      {everSynced}
      {moreToPull}
      {reaching}
      mostItems={MOST_ITEMS}
      onChanged={() => look(true)}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file: everything that went wrong and needs a person. Seven
         sections and one job, worked on a quiet afternoon. -->
    <Repairs
      bind:this={repairPanel}
      {t}
      {money}
      {qty}
      {busy}
      {attempt}
      {admin}
      {bundleMark}
      {names}
      {tills}
      onCorrect={(item) => itemPanel?.correct(item)}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file: what moved off the shelves, what is about to run out at
         that rate, and what is not moving at all. What this device knows about
         its own catalogue is handed in, because four panels name items from
         it and one copy is one answer. -->
    <Selling
      {t}
      {money}
      {qty}
      {busy}
      {attempt}
      {admin}
      {tills}
      {names}
      {kinds}
      {costs}
      {onHand}
      {shelfIsWhole}
      {askWholeShelf}
      {learnNames}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file: two questions about a period rather than about a thing,
         read together at the end of a month and touching nothing else here. -->
    <Periods
      {t}
      {money}
      {busy}
      {attempt}
      {admin}
      {tills}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file: who buys on account and what they owe, which are two
         sections and one book. The khata page it prints is handed up, because
         what prints is the only thing on the page and this panel sits inside
         `main`, which the print rule hides. -->
    <Accounts
      bind:this={accountPanel}
      {t}
      {money}
      {busy}
      {attempt}
      {admin}
      {run}
      {newId}
      showPaper={printTheStatement}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file. What a drawer panel needs is the tills, to name a drawer
         by the till it belongs to, and a way to ask the shop. -->
    <Drawers bind:this={drawerPanel} {t} {money} {attempt} {admin} {tills} />

    <!-- Its own file: what a day took and what was made on it, which is one
         question asked twice. -->
    <Takings
      bind:this={takingsPanel}
      {t}
      {money}
      {busy}
      {attempt}
      {admin}
      {tills}
      refuse={(why) => { fault = why; }}
    />

    <section>
      <h2>{t('admin.on_the_shelves')}</h2>
      <p class="why">{t('admin.shelves_why')}</p>
      <div class="row">
        <input
          bind:value={hunt}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); look(); } }}
          placeholder={t('admin.hunt_placeholder')}
          disabled={busy}
        />
        <button onclick={() => look()} disabled={busy}>{t('admin.look')}</button>
      </div>
      <label>
        <input
          type="checkbox"
          bind:checked={showRetired}
          onchange={() => look()}
          disabled={busy}
        />
        {t('admin.include_retired')}
      </label>

      <!-- Prices move together here: a sack goes up at the wholesaler and every
           rice line on the shelf goes with it. One at a time through the form
           above is an afternoon nobody has, so the prices stay wrong and the
           margin goes quietly. -->
      <div class="row">
        <input
          bind:value={movePercent}
          placeholder={t('admin.move_prices_by')}
          inputmode="decimal"
          disabled={busy}
        />
        {#if moving.length > 0}
          <button onclick={moveThePrices} disabled={busy}>
            {t('admin.move_prices', { count: moving.length })}
          </button>
        {/if}
      </div>
      {#if moving.length > 0}
        <p class="why">{t('admin.reprice_why')}</p>
        <ul class="found">
          {#each moving.slice(0, 12) as row (row.id)}
            <li>
              <span class="name">{row.name}</span>
              <span class="detail">
                {money(row.was_minor)} &rarr; <strong>{money(row.now_minor)}</strong>
              </span>
            </li>
          {/each}
        </ul>
        {#if moving.length > 12}
          <p class="why">{t('admin.and_more_below', { count: moving.length - 12 })}</p>
        {/if}
      {/if}

      <div class="row">
        <button
          class={stockMode === 'receiving' ? '' : 'quiet'}
          onclick={() => goStockMode('receiving')}
          disabled={busy}
        >
          {stockMode === 'receiving'
            ? t('admin.stop_booking_in')
            : t('admin.book_in_a_delivery')}
        </button>
        <button
          class={stockMode === 'losing' ? '' : 'quiet'}
          onclick={() => goStockMode('losing')}
          disabled={busy}
        >
          {stockMode === 'losing'
            ? t('admin.stop_writing_off')
            : t('admin.write_something_off')}
        </button>
        <button
          class={stockMode === 'counting' ? '' : 'quiet'}
          onclick={() => {
            goStockMode('counting');
            delivery = {};
            if (stockMode === 'counting' && !sheet) sheet = startSheet(Date.now());
          }}
          disabled={busy}
        >
          {stockMode === 'counting' ? t('admin.stop_counting') : t('admin.count_the_shelves')}
        </button>
        <!-- Here rather than beside the list of changes no till could read,
             because that list empties the moment the shop can read them again
             and the tills are still behind: a row a till passed over is one it
             is never offered twice. It is also the answer for a till that was
             wiped, or one that has been off for a month. -->
        <button class="quiet" onclick={sendTheListAgain} disabled={busy}>
          {t('admin.send_the_list_again')}
        </button>
      </div>

      {#if stockMode === 'receiving'}
        <p class="why">{t('admin.receiving_why')}</p>
        <div class="row">
          <select bind:value={deliveredBy} disabled={busy}>
            <option value="">{t('admin.who_it_came_from')}</option>
            {#each suppliers.filter((one) => one.active) as one (one.id)}
              <option value={one.id}>{one.name}</option>
            {/each}
          </select>
          <input bind:value={reference} placeholder={t('admin.challan_number')} disabled={busy} />
          <button onclick={bookDelivery} disabled={busy}>{t('admin.book_it_in')}</button>
        </div>
      {:else if stockMode === 'counting'}
        <p class="why">{t('admin.counting_why')}</p>
        <p class="why">
          {#if counted.total === 0}
            {t('admin.nothing_entered_yet')}{#if sheet}{' '}&middot; {t('admin.started_at', {
                at: new Date(sheet.started_at_ms).toLocaleString('en-GB'),
              })}{/if}.
          {:else}
            {t('admin.shelves_entered', { count: counted.counted })}
            {#if sheet} &middot; {t('admin.started_at', {
                at: new Date(sheet.started_at_ms).toLocaleString('en-GB'),
              })}{/if}
            {#if counted.wrong > 0}
              &middot; <span class="late">
                {t('admin.boxes_without_number', { count: counted.wrong })}
              </span>
            {/if}
          {/if}
        </p>
        <span class="row">
          <!-- Walking a shelf with a tablet is what this screen is carried
               around for, and searching for every item by name is how the
               wrong Rice gets the count. What the camera reads goes to the top
               of the list with its box ready. -->
          <button class="quiet" onclick={scanTheShelf} disabled={busy}>
            {scanningShelf ? t('admin.stop_scanning') : t('admin.scan_the_shelf')}
          </button>
          <button onclick={bookCount} disabled={busy}>{t('admin.record_the_count')}</button>
          <button class="quiet" onclick={abandonCount} disabled={busy}>
            {abandoning ? t('admin.press_again_to_throw') : t('admin.throw_it_away')}
          </button>
        </span>
        {#if scanningShelf}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video class="camera" bind:this={shelfCamera} muted playsinline autoplay></video>
          <p class="why">{t('admin.hold_the_shelf_label')}</p>
        {/if}
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
                  &middot; {t('admin.on_hand', { qty: qty(onHand[item.id].qty_milli) })}
                  <!-- How old the figure is, or that it rests on no count at
                       all. The second is the one worth saying: a figure nobody
                       has counted against is deliveries and sales added up, and
                       a shop reading it as a shelf figure is reading something
                       else. The shop has always sent this. -->
                  {#if onHand[item.id].counted_at_ms}
                    &middot; {t('admin.counted_on', {
                      when: new Date(onHand[item.id].counted_at_ms).toLocaleDateString('en-GB'),
                    })}
                  {:else}
                    &middot; <span class="late">{t('admin.never_counted')}</span>
                  {/if}
                  {#if onHand[item.id].unreconciled_sales > 0}
                    &middot; <span class="late">
                      {t('admin.sold_after_count', {
                        qty: qty(onHand[item.id].unreconciled_milli),
                      })}
                    </span>
                  {/if}
                {/if}
                {#if item.category}&middot; {item.category}{/if}
                {#if item.supply === 1}&middot; {t('admin.zero_rated')}{:else if item.supply === 2}&middot; {t('admin.exempt')}{/if}
                {#if item.vat_on_undiscounted}&middot; {t('admin.taxed_on_listed_price')}{/if}
                {#if !item.active}&middot; {t('admin.no_longer_sold')}{/if}
              </span>
              {#if stockMode !== 'off'}
                <span class="stock">
                  {#if stockMode === 'receiving'}
                    <input
                      placeholder={t('admin.how_many_came')}
                      inputmode="decimal"
                      value={delivery[item.id]?.qty ?? ''}
                      oninput={(e) => setDelivery(item.id, 'qty', e.currentTarget.value)}
                      disabled={busy}
                    />
                    <input
                      placeholder={t('admin.cost_each')}
                      inputmode="decimal"
                      value={delivery[item.id]?.cost ?? ''}
                      oninput={(e) => setDelivery(item.id, 'cost', e.currentTarget.value)}
                      disabled={busy}
                    />
                  {:else if stockMode === 'losing'}
                    <!-- A bottle dropped, a bag spoiled, something taken. The
                         reason is what makes this different from a shelf that
                         is quietly wrong. -->
                    <input
                      placeholder={t('admin.how_many_gone', {
                        qty: qty(onHand[item.id]?.qty_milli ?? 0),
                      })}
                      inputmode="decimal"
                      value={writeOff[item.id]?.qty ?? ''}
                      oninput={(e) => setWriteOff(item.id, 'qty', e.currentTarget.value)}
                      disabled={busy}
                    />
                    <input
                      placeholder={t('admin.why_written_off')}
                      value={writeOff[item.id]?.reason ?? ''}
                      oninput={(e) => setWriteOff(item.id, 'reason', e.currentTarget.value)}
                      disabled={busy}
                    />
                    <button onclick={() => writeItOff(item)} disabled={busy}>{t('admin.write_it_off')}</button>
                  {:else}
                    <input
                      placeholder={t('admin.counted_against', {
                        qty: qty(onHand[item.id]?.qty_milli ?? 0),
                      })}
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
                <button onclick={() => itemPanel?.correct(item)} disabled={busy}>{t('admin.correct_it')}</button>
                {#if item.active}
                  <button class="quiet" onclick={() => setSelling(item, false)} disabled={busy}>
                    {t('admin.stop_selling')}
                  </button>
                {:else}
                  <button class="quiet" onclick={() => setSelling(item, true)} disabled={busy}>
                    {t('admin.sell_it_again')}
                  </button>
                  <button class="quiet" onclick={() => removeItem(item)} disabled={busy}>
                    {removing === item.id
                      ? t('admin.press_again_to_delete')
                      : t('admin.delete_it')}
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <!-- Its own file: who the shop buys from, what it owes them, and what has
         come in, which are one question asked three ways. The list of suppliers
         stays here because the delivery form above picks from it. -->
    <Suppliers
      bind:this={supplierPanel}
      {t}
      {money}
      {qty}
      {busy}
      {attempt}
      {admin}
      {newId}
      {suppliers}
      {names}
      {learnNames}
      onSuppliers={(list) => { suppliers = list ?? suppliers; }}
      announce={(said) => { done = said; }}
      refuse={(why) => { fault = why; }}
    />

    <!-- Its own file. The list stays here because a drawer and a sale carried
         in by hand are both named from it; what moved is the part nothing else
         reads, which is issuing a code and cutting a device off. -->
    <Tills
      {t}
      {busy}
      {attempt}
      {admin}
      {newId}
      {tills}
      onChanged={() => listTills(true)}
      announce={(said) => { done = said; }}
    />
  {/if}
</main>

{#if accountPaper}
  <!-- On screen under everything else, and the only thing on the page when
       the browser prints. The back office had no print surface at all before
       this: what an owner could put on paper from here was a screenshot. -->
  <pre class="paper">{accountPaper.map((line) => line.text).join('\n')}</pre>
{/if}
