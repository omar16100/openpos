<script>
  import { onMount, tick } from 'svelte';
  import {
    open,
    run,
    sayWhichBuild,
    connect,
    enrol,
    keepSyncing,
    describeSync,
    adoptToken,
    sync,
    whyTheRoundFailed,
    admin,
    openAgainOnTheWayIn,
    answerWindowsAskingForTheStore,
  } from './till.js';
  // Asking the window that has the shop to let go of it, for the tablet where
  // the other window cannot be found.
  import { askForTheStore } from '../../shared/asking_for_the_store.js';
  import { money, qty } from './format.js';
  // The Mushak 6.3 tax invoice: a different document from the receipt, on A4
  // and in Bengali, for a buyer who needs one.
  import TaxInvoice from '../../shared/tax_invoice.svelte';
  // The Mushak 6.7 credit note: the paper for goods coming back.
  import CreditNote from '../../shared/credit_note.svelte';
  // Whether this sale can go on that form at all: a line whose tax is fixed to
  // its listed price and which was discounted cannot, and the form has no
  // column to say why.
  import {
    goodsCameBack,
    linesTheFormCannotCarry,
    theNoteWouldBeRefused,
  } from '../../shared/tax_invoice_check.js';
  // What this screen says, in the language the shop reads. The refusals come
  // from the core keyed on a code, because matching on an English sentence to
  // translate it goes quiet the day somebody improves the wording.
  import { languageNow, offeredLanguages, worded, wordedRefusal } from '../../shared/words.js';
  import { alreadyOpenHere, whatElseToTry } from '../../shared/storage_trouble.js';
  // Reading a barcode with the tablet's own camera, for a shop with no scanner
  // on a wire. The decoding is the browser's; what is here is the part that
  // decides whether to believe it.
  import { CANNOT_READ_HERE, readFromCamera } from '../../shared/camera_read.js';
  import { keepACopy } from '../../shared/keep_a_copy.js';
  import { fromAnotherDay, today } from '../../shared/days.js';
  // Telling two people with the same name apart, shared with the back office so
  // the mark on a person is the same in both places.
  import { label, shared } from '../../shared/people.js';
  import {
    askedForOnThisTicket,
    howManyOnTheLine,
    milliFrom,
    theWayThisTicketRuns,
  } from '../../shared/quantity.js';
  import { minorFrom } from '../../shared/money.js';

  const SERVER = window.location.origin.replace(/:\d+$/, ':8099');
  // Which shop and terminal this device is. Not secret, and needed before the
  // ledger can be opened, which is why it is here and the credential is not:
  // the credential lives in the ledger it belongs to.
  const IDENTITY = 'openpos.identity';
  /// Which language this device shows. Per device rather than per person: the
  /// tablet on the counter is read by whoever is standing at it, and asking a
  /// cashier to set it after every sign-in is asking them not to.
  const LANGUAGE = 'openpos.language';
  let remembered = $state(localStorage.getItem(LANGUAGE) ?? 'en');
  /// What the shop last said it offers, kept beside the identity.
  ///
  /// The shop's answer lives in the ledger, and the screen is drawn before the
  /// ledger is open: for the second or two that takes, a device that remembers
  /// Bangla drew Bangla and offered the button, in a shop that had turned
  /// Bangla off. Every frame after the first was right, which is what made it
  /// easy to miss and no less wrong to the person looking at it. Written down
  /// here so the first frame obeys the shop too, and read back as what the shop
  /// said until the shop says again.
  const OFFERS = 'openpos.languages';
  let offeredLast = $state(JSON.parse(localStorage.getItem(OFFERS) ?? 'null'));
  const shopOffers = $derived(view?.languages ?? offeredLast);
  /// The language this screen is actually drawn in.
  ///
  /// What this device remembers, when the shop still offers it, and otherwise
  /// the first language the shop does offer. Worked out on every draw rather
  /// than once at boot, because the shop's answer arrives after the screen has
  /// drawn and can change while it is open: a device somebody left in Bangla,
  /// in a shop that then turns Bangla off, is the device this setting exists
  /// for, and it must not be the one device left stranded in it.
  const language = $derived(languageNow(remembered, shopOffers));
  /// What the shop offers, for the button that switches. One language means no
  /// button: there is nothing to switch to.
  const offered = $derived(offeredLanguages(shopOffers));
  // Kept whenever the shop answers, so the next first frame has it. An empty
  // list is an answer too: it means every language this device has.
  $effect(() => {
    const now = view?.languages;
    if (!Array.isArray(now)) return;
    localStorage.setItem(OFFERS, JSON.stringify(now));
    offeredLast = now;
  });
  /// What to say, worded when it is read rather than when it is said.
  ///
  /// A label is worded every time the screen draws, so it follows the language.
  /// A message is assigned once, when something happens, and used to keep the
  /// language of that moment for as long as it stayed on screen: a cashier
  /// refused in English who switched to Bangla to read it watched every label
  /// around the sentence change and the sentence stay. `worded` holds the key
  /// and the figures and says itself when the screen reads it, so both follow.
  const t = (key, fill, otherwise) => worded(() => language, key, fill, otherwise);
  /// The same, for a refusal the till or the shop gave.
  const refusal = (view) => wordedRefusal(() => language, view);
  function speak(next) {
    remembered = next;
    localStorage.setItem(LANGUAGE, next);
  }

  let view = $state(null);
  let storage = $state('opening');
  // Set when the ledger would not open because this till is already open in
  // another window on this device. Its own state rather than a reading of the
  // fault text, because one thing hangs on it: the enrolment box is hidden.
  // Nothing is wrong with this device, and enrolling it again is the one move
  // that would cost the shop its unsent sales and its receipt numbers.
  /// The sale being laid out as a tax invoice, when somebody asked for one.
  ///
  /// Its own state rather than a flag on the receipt, because it holds the
  /// sale as it was at the moment it was rung: the basket is cleared by then,
  /// and a form built from what is on the screen afterwards would be a form
  /// about nothing.
  let taxInvoice = $state(null);
  /// The credit note being printed, and the words somebody typed for its
  /// ফেরতের কারণ box. The reason is not stored anywhere: it is typed by the
  /// person printing the note, which is the person who knows why the goods came
  /// back, and a reprint asks again.
  let creditNote = $state(null);
  let whyItCameBack = $state('');
  /// The last sale as it was rung, held for a tax invoice asked for afterwards.
  let lastSale = $state(null);
  /// The lines of the last sale that the Mushak 6.3 cannot carry, which is
  /// what decides whether it is offered at all.
  const awkwardLines = $derived(linesTheFormCannotCarry(lastSale?.lines ?? []));
  /// Whether the last thing rung was goods coming back rather than a supply.
  ///
  /// The form is a tax invoice and a return is not one: it is a decreasing
  /// adjustment, which section 52 gives a credit note for, and this product
  /// does not print one. The button was offered anyway and laid out a
  /// কর চালানপত্র with a quantity of -1 on it.
  const cameBack = $derived(goodsCameBack(lastSale));
  let openElsewhere = $state(false);
  /// Whether this window is waiting on an answer from the one that has the
  /// shop, and what it said.
  let asking = $state(false);
  let askingSaid = $state(null);
  /// What the window that refused is in the middle of, in its own word.
  let askingBecause = $state(null);
  /// Set when this window gave the shop up because another asked for it, so
  /// somebody coming back to this screen reads where it went.
  let shopMoved = $state(false);
  /// Whether this till knows which shop it is and has not finished opening. The
  /// offer to enrol waits on it: see the markup.
  let stillOpening = $state(false);
  /// How many times somebody has pressed "try again" and been told the same
  /// thing. The advice changes after the first one, because the first advice
  /// has a dead end in it: there may be no other window to close.
  let triedTheLedgerAgain = $state(0);
  // The name of the last failure, beside the words it was said in. A screen
  // that decided anything by reading its own sentence would stop deciding it
  // the day somebody improved the wording, or the day a shop switched to
  // Bangla.
  let lastFaultCode = null;
  // Whether the browser promised to keep what this device holds. Without a
  // grant everything in the store is evictable, which is unsent sales and the
  // receipt numbers this terminal was given.
  let keeping = $state('unknown');
  /// A build downloaded and waiting for a quiet moment to take over.
  let newBuildWaiting = $state(false);
  let fault = $state(null);
  // Something that went right and needs saying: a file written, a bundle
  // copied. Separate from a fault so a shop is not told off for succeeding.
  let done = $state(null);
  let barcode = $state('');
  let cash = $state('');
  let busy = $state(false);
  // Whether this device holds a credential. The credential itself never comes
  // here: it lives in the till's own standing state, beside the ledger it
  // belongs to, and travels with each request the core builds.
  let enrolled = $state(false);
  let code = $state('');
  let syncing = $state(t('sync.starting'));
  /// Whether that line is a round that failed rather than one that worked.
  ///
  /// Its own flag because the screen used to decide by reading the sentence for
  /// the words "held up". That is the shop's language, so on a Bangla till the
  /// line went black however long the till had been cut off, and the colour
  /// that says a till has stopped talking to its shop only appeared in English.
  let syncTrouble = $state(false);
  // When a round last reached the shop, and the clock that ages it.
  //
  // A browser freezes a hidden tab's timers and can stop them altogether. The
  // worker was moved off this thread for that reason, and a frozen tab still
  // stops its worker: the status line then says whatever it said when the
  // freezing started, which reads as a till that is fine. This is the figure
  // that cannot lie by standing still.
  let lastReached = $state(null);
  /// Whether the last round could not reach the shop, which is when a person
  /// standing there has something to fix and something to press.
  let roundsFailing = $state(false);
  let now = $state(Date.now());
  const sinceReached = $derived(lastReached === null ? null : now - lastReached);
  /// Five minutes. A till syncs every two seconds, so anything approaching this
  /// is a device that has stopped rather than a slow round.
  const TOO_LONG_MS = 5 * 60_000;
  // The last sale, laid out for paper. Held until the next sale replaces it, so
  // a cashier can reprint without hunting for anything.
  let receipt = $state(null);
  // The receipt a refund is against, while the cashier is being asked for it.
  let askingReceipt = $state(false);
  /// The id this basket will be rung under, minted once and kept until it is.
  ///
  /// See `checkout`: a sale can be durable and still come back as a failure,
  /// and a cashier who presses again would otherwise give the shop two sales
  /// for one basket. Cleared when the sale goes through, when the basket is
  /// parked, and when it is thrown away, because each of those ends the basket.
  let ticketId = $state(null);
  let refundAgainst = $state('');
  /// What the shop says was on the receipt somebody is holding, and how much of
  /// each line is coming back.
  ///
  /// A refund used to be rung by scanning the goods again, which prices them
  /// out of today's catalogue: a basket sold with something off it came back at
  /// full price and the shop gave the discount away a second time. What the
  /// customer is owed is what the customer paid, and this is where the till
  /// reads it.
  let broughtBack = $state(null);
  /// The invoice this refund adjusts: its number, and the day it was issued.
  ///
  /// Kept from the moment the cashier typed the number and the shop answered
  /// with the sale, because form মূসক-৬.৭ asks for both and the second is not on
  /// the customer's paper in a form this till could read back. Null when the
  /// customer could not produce the receipt, which is a real thing at a counter
  /// and leaves those two lines on the form for a hand to fill in.
  let refundOriginal = $state(null);
  let comingBack = $state({});
  // Who is picked on the sign-in panel, before their PIN is entered.
  let picked = $state(null);
  let pin = $state('');
  // Which line the cashier has open for correcting. One at a time: a screen
  // that expands every line is a screen where the wrong one gets pressed.
  let editing = $state(null);
  let ticketOff = $state('');
  // The same discount said the other way: an amount rather than a rate.
  let ticketOffAmount = $state('');
  // Looking an item up by name, for a barcode that will not read, loose goods
  // that carry none, or a label torn off. The catalogue is on the device, so
  // this works with the line down like everything else at the counter.
  let lookingUp = $state(false);
  /// Whether a scan answers "what does this cost" instead of ringing it.
  ///
  /// The question a cashier is asked twenty times a day. Until this the only
  /// way to answer it was to ring the thing and take it off again, which needs
  /// a supervisor once the customer has started paying and leaves a line on the
  /// trail saying somebody voided something.
  let checking = $state(false);
  /// Whether what the till holds as "checked" is the answer to the code this
  /// cashier just asked about. See check().
  let checkAnswered = $state(false);
  // A barcode the catalogue does not have, and what the cashier says it is.
  let unknown = $state(null);
  let newName = $state('');
  let newPrice = $state('');
  let newVat = $state('15');
  // A phone number for somebody written down at the counter, which is how a
  // shop here tells one Karim from another.
  let newPhone = $state('');
  let hunt = $state('');
  let found = $state([]);
  // What the till made of a whole phrase, when one was said to it. Held apart
  // from `found` because a cashier has to be able to see what it thought it
  // heard, and a list of items does not say that.
  let heard = $state(null);
  let scanner;

  // The server refuses this device's credential: the terminal was removed, the
  // token was revoked, or the server was rebuilt underneath it. The device looks
  // enrolled and is not, and nothing it does will reach the shop.
  const refused = $derived(view?.credential_refused ?? false);
  // What this device is holding that the shop has not got, once somebody asks.
  // Not asked for by itself: reading it means scanning the salvage blob, and a
  // till that is syncing has no use for the answer.
  let carrying = $state(null);
  const waiting = $derived(view?.unsynced_sales ?? 0);

  const operator = $derived(view?.operator ?? null);
  const people = $derived(view?.people ?? []);
  // Two people called Karim make two identical buttons, and a cashier who
  // presses the wrong one hands every sale of that shift to somebody else.
  // Worked out in `apps/shared/people.js`, with tests, because the back office
  // has to mark the same people the same way for the mark to mean anything.
  const twiceOver = $derived(shared(people));
  // And the same for the people who buy on account. Picking the wrong one of
  // two Karims puts a basket on somebody else's account, which is money.
  const customersTwiceOver = $derived(shared(customers));
  const drawer = $derived(view?.drawer ?? null);
  /// Whether the open drawer was opened on a day other than today.
  ///
  /// By the device's own clock and its own idea of a day, which is the clock
  /// the drawer was opened by and the one the cashier is standing next to. A
  /// shop's day does not end at midnight everywhere, and this is not trying to
  /// decide when it ends: it says the drawer is from another date, which is the
  /// thing somebody can check.
  const drawerFromAnotherDay = $derived(
    Boolean(drawer?.open) && fromAnotherDay(drawer.opened_at_ms),
  );
  let float_ = $state('');
  let movement = $state('');
  let reason = $state('');
  let counted = $state('');
  const report = $derived(view?.report ?? null);

  // How much this cashier may give away unaided. Zero for most of them, which
  // is what the preset says, so this decides how the boxes are worded rather
  // than whether they are offered: a refusal is how a supervisor gets asked,
  // and hiding the box hid the whole path.
  const ceiling = $derived(view?.operator?.max_discount_bp ?? 0);
  // Whether this cashier may sell a line at a price other than the shelf's.
  const mayOverride = $derived(view?.operator?.may_override_price ?? false);

  // Sales parked while the queue moved on.
  const parked = $derived(view?.held ?? []);
  let parkAs = $state('');
  // How the money came. Cash stays the default and the big button, because that
  // is still most of a day; a shop here also takes bKash and Nagad all day and
  // this till could not record either.
  let payingBy = $state('cash');
  /// Which wallet, when the shop takes more than one.
  ///
  /// Kept in step with the list the shop sent. A `bind:value` whose value
  /// matches no option leaves the binding alone and lets the browser show the
  /// first one, so a cashier who accepted the default, which is every cashier,
  /// rang a wallet tender with no name at all. The drawer report then reads
  /// "a wallet 2,400.00", which is the exact thing the comment beside that
  /// dropdown says it must never say. Found by walking a two-tender sale.
  let walletName = $state('');
  // What the shop says it takes. A cashier picks a name rather than spelling it,
  // and a till that has never been told falls back to letting them type one.
  const wallets = $derived(view?.wallets ?? []);
  // Who the shop lets buy on account, as this till was last told. A cashier
  // picks from these rather than typing, so what somebody owes is added up
  // against a person the shop has a record of.
  const customers = $derived(view?.customers ?? []);
  // The one this basket is for, when it is for anybody.
  const chosen = $derived(customers.find((one) => one.id === view?.customer) ?? null);
  let reference = $state('');

  const total = $derived(view?.total_minor ?? 0);
  const refunding = $derived(view?.is_refund ?? false);
  // Both from the till, not worked out here. The screen used to subtract one
  // figure from another and decide for itself what the difference meant, which
  // is a second answer about money: the day the core's rule changed, what the
  // customer was shown and what the drawer would accept were different things.
  const outstanding = $derived(view?.outstanding_minor ?? 0);
  const settled = $derived(view?.settled ?? false);

  /// What came off this line, worded so the rate and the amount cannot be read
  /// as the same fact. A ticket discount is apportioned across the lines, so the
  /// amount on a line is larger than its own rate accounts for, and "less 10%,
  /// 14.50" on a hundred-taka line is a cashier's phone call to the owner.
  function discountNote(line) {
    const off = money(-line.discount_minor);
    // Three different things, and the screen used to call two of them the same:
    // a line somebody took twenty taka off read as a line carrying its share of
    // a discount off the whole basket.
    if (line.discount_bp) {
      const rate = (line.discount_bp / 100).toFixed(line.discount_bp % 100 ? 2 : 0);
      return t('till.off_this_line_at_rate', { rate, off });
    }
    if (line.discount_amount_minor) {
      const own = money(-line.discount_amount_minor);
      // "In all" when a discount off the whole basket has been shared out on
      // top of it, the same way the rate above reads.
      return line.discount_amount_minor === line.discount_minor
        ? t('till.off_this_line', { own })
        : t('till.off_this_line_in_all', { own, off });
    }
    return t('till.share_of_ticket_discount', { off });
  }

  /// How many of this line there are to be, as a count rather than as a signed
  /// number.
  ///
  /// The sign is the ticket's and is put on here, in one place. A refund keeps
  /// its quantities below nothing, which is what makes a return the mirror of
  /// the sale it undoes, and none of that is the cashier's business: they are
  /// holding three bags and the box asks how many.
  async function changeQty(at, howManyMilli) {
    if (howManyMilli <= 0) {
      // Down to nothing is off the ticket. Sending a zero quantity would leave
      // a line reading "0 x Rice" that nobody can sell or clear.
      await drop(at);
      return;
    }
    await attemptWithOverride(() =>
      run({
        op: 'set_qty',
        line: at,
        qty_milli: askedForOnThisTicket(howManyMilli, view?.is_refund),
      }),
    );
  }

  /// A quantity somebody typed, for the things a shop sells by weight.
  ///
  /// The buttons either side of it move by one, which is right for packets and
  /// useless for a kilo and a half of dal. The same parser the back office
  /// counts shelves with, so 1.5 means the same thing in both places and
  /// 1.5005 is refused in both.
  async function typeQty(at, typed) {
    const milli = milliFrom(typed);
    if (milli === null) {
      fault = t('till.not_a_quantity');
      return;
    }
    await changeQty(at, milli);
  }

  /// Take a line off. Free while nobody has paid towards this basket, and a
  /// supervisor's business once money is on it: that is the shape of goods rung
  /// up, cash taken, and the line quietly removed.
  async function drop(at) {
    editing = null;
    await attemptWithOverride(() => run({ op: 'remove_line', line: at, at_ms: Date.now() }));
    scanner?.focus();
  }

  async function priceLine(at, typed) {
    // Read by the parser the rest of this product reads money with. `Number()`
    // was here: it takes "1e3" for a thousand, and this is a price that goes
    // straight onto a line a customer is about to pay.
    const price_minor = minorFrom(typed);
    if (price_minor === null || price_minor < 0) {
      fault = t('till.not_a_price');
      return;
    }
    await attemptWithOverride(() => run({ op: 'set_unit_price', line: at, price_minor }));
  }

  async function discountLine(at, typed) {
    // Through the money parser and back down, because a percentage is typed the
    // same way an amount is and `Number()` reads "1e3" as a thousand. Hundredths
    // of a percent is as fine as anybody types one, and an empty box is nothing
    // off rather than a refusal: it is how a discount is taken back.
    const hundredths = typed.trim() === '' ? 0 : minorFrom(typed);
    if (hundredths === null) {
      fault = t('till.not_a_percentage');
      return;
    }
    const percent = hundredths / 100;
    await attemptWithOverride(() => run({ op: 'set_line_discount', line: at, percent }));
  }

  /// A stated amount off, which is what a shop here says out loud: twenty taka
  /// off, not four point six five percent off. The till measures it against the
  /// same ceiling and asks for a supervisor by the same route.
  async function takeOffLine(at, typed) {
    const off = minorFrom(typed);
    if (off === null) {
      fault = t('till.not_an_amount_off');
      return;
    }
    await attemptWithOverride(() => run({ op: 'take_off_line', line: at, amount_minor: off }));
  }

  async function takeOffTicket() {
    const off = minorFrom(ticketOffAmount);
    if (off === null) {
      fault = t('till.not_an_amount_off');
      return;
    }
    await attemptWithOverride(() => run({ op: 'take_off_ticket', amount_minor: off }));
    ticketOffAmount = '';
    scanner?.focus();
  }

  async function discountTicket() {
    const hundredths = ticketOff.trim() === '' ? 0 : minorFrom(ticketOff);
    if (hundredths === null) {
      fault = t('till.not_a_percentage');
      return;
    }
    const percent = hundredths / 100;
    await attemptWithOverride(() => run({ op: 'set_ticket_discount', percent }));
    ticketOff = '';
    scanner?.focus();
  }

  /// What the cashier just tried, kept only long enough for a supervisor to
  /// allow it. A refusal for want of permission is the one failure at a till
  /// that somebody standing behind the counter can fix in ten seconds, and
  /// until now the only way through it was to sign out and sign back in as the
  /// supervisor, in front of the customer.
  let blocked = $state(null);
  let supervisorPin = $state('');

  /// Try something, and keep it if the till says a supervisor is needed.
  ///
  /// What is needed comes from the core, in the view: a screen matching on the
  /// words of a refusal would be deciding a second time what is permitted, in
  /// a place nobody tests, and would go quiet the day a message is reworded.
  /// `after` is what follows the work once it has actually happened. It runs
  /// here when nobody was blocked, and in allowIt when a supervisor unblocked
  /// it, and in neither case while the till is still refusing: a refund that
  /// was refused used to go on and put the old receipt's lines on screen with
  /// "bring these back" under them, which is a promise the till had not made.
  async function attemptWithOverride(work, after = null) {
    blocked = null;
    const reply = await attempt(work);
    if (reply?.view?.needs_supervisor) {
      blocked = { work, action: reply.view.needs_supervisor, after };
      return reply;
    }
    if (reply && after) await after(reply);
    return reply;
  }

  /// A supervisor allows this one action, on this till, for a moment.
  ///
  /// Then the thing they allowed happens, without the cashier retyping it in
  /// front of the customer.
  async function allowIt(supervisor) {
    if (!blocked) return;
    const pin = supervisorPin;
    supervisorPin = '';
    const allowed = await attempt(() =>
      run({
        op: 'authorise',
        supervisor_id: supervisor.id,
        pin,
        action: blocked.action,
        now_ms: Date.now(),
      }),
    );
    if (!allowed || allowed.view?.error) return;
    const again = blocked;
    blocked = null;
    const done = await attempt(again.work);
    if (done && !done.view?.needs_supervisor && again.after) await again.after(done);
  }

  async function attempt(work) {
    busy = true;
    try {
      const reply = await work();
      view = reply.view;
      // A refusal the core reported, said in the language this device shows.
      // The core carries a code and the figures beside it; the words come from
      // one dictionary, and a refusal nobody has translated yet falls back to
      // the sentence the core sent rather than to nothing.
      lastFaultCode = view?.error_code ?? null;
      fault = refusal(view);
      return reply;
    } catch (error) {
      // A worker that failed outright, which is different from a till that
      // refused: the basket on screen may no longer be what the till holds.
      //
      // Worded the same way all the same. A refusal from the shop's own server
      // travels this path, and it carries a name and its figures beside the
      // English: this is the point where the language is known.
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

  /// Open this device's ledger, and say plainly if it could not be opened.
  ///
  /// Its own function because it is called twice: once as the app boots and
  /// again when somebody presses "try again" after closing the other window.
  /// Retrying costs nothing and is the whole answer to the commonest failure
  /// here, which is not a failure at all.
  async function openTheLedger(known) {
    const reply = await attempt(() => open(known.tenant, known.terminal));
    storage = reply?.info?.storage ?? 'unavailable';
    keeping = reply?.info?.keeping ?? 'unknown';
    enrolled = Boolean(reply?.view?.enrolled);
    openElsewhere = !reply && alreadyOpenHere(lastFaultCode);
  }

  /// Try the ledger again, after whoever is standing there has closed the other
  /// window.
  /// Ask whoever has the shop to let go of it.
  ///
  /// The answer is one of three and each is a different sentence: the other
  /// window gave it up and this one opens, it has a sale in progress and kept
  /// it, or nothing answered at all, which is a store held by a window that has
  /// gone and is what the advice above is for.
  async function askTheOtherWindow() {
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (!known) return;
    asking = true;
    askingSaid = null;
    const answer = await askForTheStore(known.terminal);
    asking = false;
    askingSaid = answer.said;
    askingBecause = answer.because;
    if (answer.said === 'let_go') await openItAgain();
  }

  async function openItAgain() {
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (!known) return;
    storage = 'opening';
    await openTheLedger(known);
    if (openElsewhere) triedTheLedgerAgain += 1;
  }

  onMount(async () => {
    // A page the browser froze let its files go so another window could sell.
    // This is the way back in.
    openAgainOnTheWayIn(openItAgain);

    // And answer the windows that ask this one for the shop. Refused while
    // there is a basket rung, money tendered, or a drawer half counted: this
    // window is the only one that knows, and dropping any of those loses work a
    // person did standing at a counter.
    const whoWeAre = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (whoWeAre) {
      answerWindowsAskingForTheStore(whoWeAre.terminal, {
        // What this window is in the middle of, in a word, because the
        // screen that asked turns it into a sentence and "a sale in
        // progress" is the wrong sentence for a drawer being counted.
        busy: () =>
          (view?.lines?.length ?? 0) > 0 || (view?.tendered_minor ?? 0) !== 0
            ? 'selling'
            : counted.trim() !== ''
              ? 'counting'
              : null,
        lost: () => {
          shopMoved = true;
          openElsewhere = true;
          storage = 'unavailable';
          // What this window was told last time it asked is about a shop it no
          // longer has. Left standing, a window that took the shop and then
          // gave it up read "the other window gave it up, opening the shop
          // here" underneath the sentence saying the shop had just left.
          askingSaid = null;
        },
      });
    }

    // Before anything else, because this is what lets the app be opened at all
    // during an outage. Everything below it is offline machinery that a tablet
    // switched on with the internet down could not reach: the browser would be
    // fetching the page and the wasm from a server that is not answering.
    keepACopy(
      () => ({
        lines: view?.lines?.length ?? 0,
        tendered: (view?.tendered_minor ?? 0) !== 0,
        // A drawer being counted. This said `false` on this screen while the
        // back office worked its own out, and the till has the state as much as
        // the back office does: a cashier at the end of a shift with the notes
        // in one hand and a figure half typed into the box. A build taking over
        // there reloads the screen and the figure is gone, and what it costs is
        // counting the drawer again, which is minutes and is the last thing
        // anybody wants to do twice.
        counting: counted.trim() !== '',
        unsent: view?.unsynced_sales ?? 0,
      }),
      (waiting) => {
        newBuildWaiting = waiting;
      },
    );
    await connect(SERVER);
    // Which build this is, told to the worker as soon as the page knows, and
    // before anything is opened or enrolled. The worker sends it with every
    // request it posts afterwards, whatever the device does next: a device
    // enrolled a minute ago is exactly the one somebody is likely to be asking
    // about, and asking only when an already known store is opened left those
    // silent until their next reload. Nothing waits on it.
    void sayWhichBuild();
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    // Held back while a till that has been enrolled before opens its store.
    stillOpening = Boolean(known);
    if (known) {
      await openTheLedger(known);
    } else {
      // Nothing has told this device who it is yet, so there is no ledger to
      // open: a till opened as a guess would present a credential for one
      // terminal and a request body for another.
      storage = 'not enrolled';
    }
    // Whatever happened, the answer is in: a till with no credential now sees
    // the box, and one that is merely waiting on a held store sees the sentence
    // about the other window instead.
    stillOpening = false;

    // One round every two seconds, run by the worker rather than by this
    // thread. A browser throttles a hidden page's timers to about once a minute
    // and can stop them altogether, so a till whose tab is not in front was a
    // till that had quietly stopped sending: seen twice, both times cured by
    // reloading. The core decides whether a round does anything; this only says
    // how often to ask.
    keepSyncing((round) => {
      if (round.view) view = round.view;
      // Shown, not swallowed. A till that quietly stops syncing is the failure
      // the whole design is arranged against, and the view that comes back with
      // a failure is the only thing that says whether the shop has refused this
      // device outright.
      const said = describeSync(round.info);
      syncTrouble = !round.ok;
      syncing = round.ok
        ? t(said.key, said.fill)
        : t(
            whyTheRoundFailed(round.error, round.error_code) ?? 'sync.held_up',
            { why: round.error },
          );
      // Only a round that actually exchanged something with the shop. A round
      // that decided to wait is `ok` too, and a till backing off after failing
      // decides to wait every two seconds: counting those was this figure
      // telling the cashier it had reached the shop, in the very outage it
      // exists to make visible. Found by walking a five minute outage, which is
      // the walk this feature shipped without.
      if (round.ok && round.info?.did) lastReached = Date.now();
      // The driver's own failure count, not whether this round succeeded. A
      // round that decides to wait is `ok` too, and during a backoff most of
      // them are: reading `ok` made the button appear for two seconds and
      // vanish for the next four minutes, which is worse than not having it.
      // Found by walking an outage and watching for a button that never came.
      roundsFailing = (round.info?.after_failures ?? (round.ok ? 0 : 1)) > 0;
    });
    // The clock that ages the figure above. Its own timer, on this thread,
    // because it is allowed to stop when the tab is hidden: nobody is reading
    // it then, and what matters is that it is right the moment somebody looks.
    setInterval(() => {
      now = Date.now();
    }, 1000);

    // A tab coming back to the front syncs at once rather than waiting for the
    // worker's next round, because the round it was waiting for is exactly the
    // one a browser may have stopped.
    document.addEventListener('visibilitychange', () => {
      now = Date.now();
      if (document.visibilityState === 'visible') {
        // Quietly: a round that fails while the tab was away is not something
        // to interrupt a cashier with, and the next one says so anyway.
        sync(Date.now())
          .then((round) => {
            if (round?.view) view = round.view;
            // Same rule as the loop above: a round that failed or waited is not
            // contact, and a tab coming back to the front must not be able to
            // clear a warning by asking once and getting nowhere.
            if (round?.ok && round.info?.did) lastReached = Date.now();
          })
          .catch(() => {});
      }
    });

    // A scanner is a keyboard. The field takes focus at once and takes it back
    // after every action, because a scan that lands nowhere is a scan the
    // cashier does not know was lost.
    scanner?.focus();
  });

  async function join() {
    const typed = code.trim();
    if (!typed) return;
    busy = true;
    try {
      // The code decides who this device is. Only then is there a ledger to
      // open, and only then is there somewhere to keep the credential.
      const { info } = await enrol(typed);

      // A code for the same till gives this device a new credential and leaves
      // its ledger where it is, which is the whole of recovering from a revoked
      // token. A code for a different till gives it a different store, and
      // anything the old one has not sent is left in a ledger nothing will open
      // again. Those are sales that happened, so this refuses, and says why.
      //
      // The check is here and not before the code is sent because only the reply
      // says which till the code is for. It costs a spent code in the case it
      // refuses, which is the cheaper of the two things to lose.
      const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
      if (known && known.terminal !== info.terminal && waiting > 0) {
        throw new Error(
          `That code is for a different till, and ${waiting} ${waiting === 1 ? 'sale on this one has' : 'sales on this one have'} not reached the shop yet. Enrolling as a different till would abandon them. Ask the owner for a code for this till.`,
        );
      }

      localStorage.setItem(
        IDENTITY,
        JSON.stringify({ tenant: info.tenant, terminal: info.terminal }),
      );
      const opened = await open(info.tenant, info.terminal);
      storage = opened.info?.storage ?? 'unavailable';
      keeping = opened.info?.keeping ?? 'unknown';
      const adopted = await adoptToken(info.token);
      view = adopted.view;
      enrolled = true;
      code = '';
      fault = null;
    } catch (error) {
      // Enrolling is where a device meets a server that may be newer than it
      // is, so it is exactly where a named refusal matters: "this device speaks
      // version 2 and the shop speaks 3" in the language of whoever is standing
      // at the counter setting it up.
      // Enrolling opens a ledger too, so it meets the same lock: a second
      // window of a device somebody is setting up. Answered the same way here
      // as on the boot path, because the box on this screen is the one telling
      // them to do the thing that would cost them their sales.
      openElsewhere = alreadyOpenHere(error.code ?? null);
      fault = refusal({
        error: error.message,
        error_code: error.code,
        error_parts: error.parts,
      });
    } finally {
      busy = false;
      scanner?.focus();
    }
  }

  async function signIn() {
    if (!picked || !pin) return;
    const entered = pin;
    pin = '';
    await attempt(() =>
      run({ op: 'sign_in', operator_id: picked.id, pin: entered, now_ms: Date.now() }),
    );
    picked = null;
    scanner?.focus();
  }

  async function signOut() {
    await attempt(() => run({ op: 'sign_out' }));
  }

  /// Put what this device is holding into a file.
  ///
  /// A file rather than only text on a screen, because the text is thousands of
  /// characters and the two devices are usually not the same one: a file goes
  /// onto a memory stick, into an email, or through whatever the shop has.
  /// Named for the terminal and the day, so a folder of them can be told apart.
  function saveCarried() {
    if (!carrying) return;
    const day = today();
    const name = `openpos-${carrying.terminal}-${day}.txt`;
    const blob = new Blob([carrying.bundle], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    link.click();
    // Released on the next turn: revoking it while the click is still being
    // handled cancels the download on some browsers.
    setTimeout(() => URL.revokeObjectURL(url), 0);
    done = t('till.saved_as_file', { name });
  }

  /// Or straight to the clipboard, for the case where both are one device.
  async function copyCarried() {
    if (!carrying) return;
    try {
      await navigator.clipboard.writeText(carrying.bundle);
      done = t('till.copied');
    } catch {
      // No clipboard permission, or an insecure origin. The text is on the
      // screen either way, which is why it is still shown.
      fault = t('till.could_not_copy');
    }
  }

  /// Read off what this device is still holding, so it can be carried.
  async function showCarrying() {
    const reply = await attempt(() => run({ op: 'carrying' }));
    carrying = reply?.view?.carrying ?? null;
  }

  /// A ULID-shaped id minted here, because the core mints none.
  function newId() {
    return crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
  }

  /// Open the cash drawer without selling anything.
  ///
  /// The bytes come back as a print job, because almost every drawer in a shop
  /// here is on the end of a cable in the printer's socket: opening one is
  /// something the printer does. A browser cannot send them to a printer, so
  /// what this proves today is that the till allowed it and wrote it down; the
  /// thermal path is what carries the bytes, and it is the same job.
  /// Try the shop now, because somebody has just fixed the line.
  ///
  /// A fallback and never the path: the loop syncs on its own and a shop should
  /// not have to press anything. It exists for the one moment the loop is
  /// wrong, which is a shopkeeper who has restarted the router looking at a
  /// till that says it will try again in four minutes.
  /// Print the receipt on screen a second time, and tell the shop it happened.
  ///
  /// The paper is already laid out, so this prints what is there rather than
  /// building it again: rebuilding would read the clock afresh and put a
  /// different time on the customer's second copy than on their first.
  ///
  /// The record goes first. A reprint that printed and then failed to be
  /// written down is the one a shop would want to know about most.
  async function printAgain() {
    // The paper comes back marked as a copy, so the screen takes what the till
    // hands over rather than printing what it was already holding. A reprint
    // that looks exactly like the original is two receipts for one sale, which
    // is how a refund gets claimed twice, and the shop's own record of the
    // reprint is somewhere neither the customer nor the person handed the paper
    // can see.
    const reply = await attempt(() => run({ op: 'reprinted', now_ms: Date.now() }));
    if (reply?.view?.receipt) receipt = reply.view.receipt;
    // Waited for, like the first print: the browser prints what is on the page,
    // and the page has just been told to draw something else.
    await new Promise((settle) => setTimeout(settle, 50));
    window.print();
  }

  async function tryNow() {
    await attempt(() => run({ op: 'try_now' }));
  }

  async function openTheDrawer() {
    // Through the override path, like everything else a supervisor can allow.
    // A shop that has taken the drawer off somebody refuses this, and the
    // refusal is written into the trail under its own number: without this the
    // screen said no and offered nothing, so the way to open the drawer was to
    // sign the cashier out and the supervisor in.
    await attemptWithOverride(() => run({ op: 'open_drawer', now_ms: Date.now() }));
  }

  async function openShift() {
    const opening_float_minor = minorFrom(float_);
    if (opening_float_minor === null || opening_float_minor < 0) {
      fault = t('till.count_the_float');
      return;
    }
    float_ = '';
    await attempt(() =>
      run({
        op: 'open_shift',
        shift_id: newId(),
        opening_float_minor,
        at_ms: Date.now(),
      }),
    );
  }

  async function moveCash(inward) {
    const amount_minor = minorFrom(movement);
    if (amount_minor === null || amount_minor <= 0) {
      fault = t('till.an_amount_in_taka');
      return;
    }
    if (!reason.trim()) {
      // The core refuses this too. Saying so here saves a round trip and says
      // it in the words the cashier is looking at.
      fault = t('till.say_why_cash_moved');
      return;
    }
    const why = reason.trim();
    movement = '';
    reason = '';
    await attemptWithOverride(() =>
      run({ op: 'move_cash', inward, amount_minor, reason: why, at_ms: Date.now() }),
    );
  }

  /// The slip that goes in the drawer with the cash.
  ///
  /// Everything on it is on the screen already and none of it could be printed,
  /// so a cashier copied the figures onto a piece of paper by hand at the one
  /// moment of the day when the shop most wants a record nobody rewrote. Laid
  /// out by the same crate that lays out a receipt, so what comes off a thermal
  /// printer is what is on the screen.
  async function printDrawer() {
    const reply = await attempt(() =>
      run({
        op: 'drawer_paper',
        // Paper is English, whatever the screen is set to. Three reasons and
        // they all point the same way: no ESC/POS code page carries Bangla, so
        // a thermal printer gets English regardless; the layout pads by
        // counting characters, which Bangla defeats, so a Bangla slip comes out
        // ragged; and a shop with two languages on its counter should not have
        // two shapes of receipt in its records. `{}` is the core's own English.
        words: {},
        width: 32,
        at: new Date().toLocaleString('en-GB'),
        counted_by: view?.operator?.name ?? null,
      }),
    );
    receipt = reply?.view?.receipt ?? null;
    if (receipt) {
      await new Promise((settle) => setTimeout(settle, 50));
      window.print();
    }
  }

  async function closeShift() {
    const counted_minor = minorFrom(counted);
    if (counted_minor === null || counted_minor < 0) {
      fault = t('till.count_the_drawer');
      return;
    }
    counted = '';
    // The report comes back from the core, variance and all. Working it out
    // here would be a second arithmetic that can disagree with the first.
    //
    // Through the override path, because a cashier may not close a drawer and
    // counting one is the last thing they do at the end of a shift. The screen
    // refused and offered nothing, so the count was retyped by a supervisor
    // who had to sign in to do it, and the shop's record of who counted said
    // the supervisor. The figure is already captured, so the supervisor allows
    // it and the same count goes through.
    await attemptWithOverride(() =>
      run({ op: 'close_shift', counted_cash_minor: counted_minor, at_ms: Date.now() }),
    );
  }

  async function xReport() {
    // Totals without closing, which is what a cashier checks against the till
    // in the middle of a shift.
    await attempt(() => run({ op: 'x_report' }));
  }

  /// Ask which receipt before starting one. The paper is usually in their hand.
  ///
  /// The core has taken the original receipt since it was written and this
  /// screen never asked for it, so every refund this shop ever rang arrived at
  /// the server with nothing to check it against: whether the sale exists,
  /// whether it has already been refunded, whether more is coming back than
  /// went out. All of that was written, tested, and never reached by anything a
  /// cashier could do.
  function askForTheReceipt() {
    refundAgainst = '';
    askingReceipt = true;
  }

  /// Ask the shop what was on the paper in the customer's hand.
  ///
  /// The money comes from the paper and the tax treatment from the item, which
  /// is the whole point: scanning the goods again prices them out of today's
  /// catalogue, so a basket sold with something off comes back at full price
  /// and a price that has moved since comes back at the new one.
  ///
  /// A shop that cannot be reached still refunds. The cashier scans the goods
  /// as before, and the screen says which of the two it is doing, because the
  /// difference is money.
  async function findWhatWasOnIt(number) {
    broughtBack = null;
    comingBack = {};
    const reply = await attempt(() => admin({ what: 'receipt', receipt_no: number }, Date.now()));
    if (!reply) return;
    const found = reply.info?.on_paper ?? [];
    // The sale itself, not a refund already rung against it.
    broughtBack = found.find((one) => !one.refund_of) ?? null;
    refundOriginal = broughtBack
      ? { no: broughtBack.receipt_no, at: new Date(broughtBack.rung_at_ms) }
      : null;
    if (!broughtBack) {
      // The number may be mistyped, or the till that rang it may not have
      // reached the shop yet. Either way the cashier is about to scan the goods
      // instead, and should know that is what is happening: what they scan is
      // priced at today's catalogue rather than at what this customer paid.
      done = t('till.no_such_receipt_here', { number });
      return;
    }
    for (const [at, line] of broughtBack.lines.entries()) {
      comingBack[at] = String(qty(Math.abs(line.qty_milli)));
    }
    if (broughtBack.refunded_minor !== 0) {
      // Part of this receipt has already come back. The shop refuses more than
      // the whole of it, but that refusal arrives after the money has left the
      // drawer, so the person deciding should be told before.
      done = t('till.already_given_back', {
        amount: money(Math.abs(broughtBack.refunded_minor)),
      });
    }
  }

  /// Ring the lines the cashier has said are coming back.
  async function bringThemBack() {
    if (!broughtBack) return;
    for (const [at, line] of broughtBack.lines.entries()) {
      const wanted = milliFrom(comingBack[at] ?? '');
      if (!wanted || wanted <= 0) continue;
      const held = Math.abs(line.qty_milli);
      const back = Math.min(wanted, held);
      // The whole line as the paper has it goes across, and the till takes the
      // share that belongs to what is coming back. This screen used to do that
      // division itself, which put one of the shop's money answers in a
      // language whose rounding is not the shop's.
      const reply = await attempt(() =>
        run({
          op: 'return_line',
          item_id: line.item_id,
          qty_milli: back,
          charged_each_minor: line.unit_price_minor,
          came_off_minor: line.discount_minor,
          was_on_milli: held,
        }),
      );
      if (!reply) return;
    }
    broughtBack = null;
    comingBack = {};
    scanner?.focus();
  }

  /// Step back out of a refund nobody has put anything into.
  async function leaveTheRefund() {
    broughtBack = null;
    refundOriginal = null;
    comingBack = {};
    ticketId = null;
    await attempt(() => run({ op: 'cancel_sale' }));
    scanner?.focus();
  }

  async function startRefund() {
    askingReceipt = false;
    const against = refundAgainst.trim();
    refundAgainst = '';
    // The number, even when the shop cannot produce the sale behind it: the
    // form has a line for it, and a number the customer read off their own
    // paper is better on that line than a blank. The date stays blank until
    // the lookup below answers, because this till has no way to know it.
    refundOriginal = against === '' ? null : { no: against, at: null };
    // Refused unless this person may, or a supervisor has allowed it. The
    // refusal is the core's own words, which name what is missing.
    //
    // Without a number when they have lost the paper, which happens and is not
    // a reason to refuse somebody their money at the counter: the shop takes it
    // back and the sale says nobody named the receipt.
    await attemptWithOverride(
      () =>
        run({
          op: 'start_refund',
          original_receipt: against === '' ? null : against,
          now_ms: Date.now(),
        }),
      against === '' ? null : () => findWhatWasOnIt(against),
    );
    scanner?.focus();
  }

  async function look() {
    const asked = hunt.trim();
    if (!asked) {
      found = [];
      heard = null;
      return;
    }
    const reply = await attempt(() => run({ op: 'catalogue', query: asked, limit: 12 }));
    found = reply?.view?.catalogue ?? [];
    heard = null;
  }

  // A whole phrase, the way somebody would say it rather than the way somebody
  // would type it. The core does the reading: which words are worth looking up,
  // which are politeness, and whether a number said was a quantity or part of a
  // name. Nothing here decides any of that, and nothing here reaches the ticket.
  //
  // Typed for now, on purpose. A microphone is the only part of this that cannot
  // be tested without a person in a room, so it is the last part to arrive: with
  // a keyboard, the whole of the understanding can be put in front of a
  // shopkeeper and found wanting before anybody downloads a model for it.
  async function listen() {
    const said = hunt.trim();
    if (!said) {
      heard = null;
      found = [];
      return;
    }
    const reply = await attempt(() => run({ op: 'heard', transcript: said }));
    heard = reply?.view?.heard ?? null;
    found = heard?.candidates ?? [];
  }

  /// Answer what one costs, without touching the basket.
  async function check() {
    const code = barcode.trim();
    if (!code) return;
    barcode = '';
    // The answer on screen belongs to the code just asked, and to nothing else.
    // The till keeps the last one it was asked about, so without this a cashier
    // who checked rice, went back to scanning, then opened the price check again
    // was shown the price of rice until the next scan replied, and the customer
    // was standing there holding soap.
    checkAnswered = false;
    const reply = await attempt(() => run({ op: 'check', code }));
    checkAnswered = Boolean(reply);
    scanner?.focus();
  }

  /// The customer said yes. Ring what was just checked and go back to scanning.
  async function ringChecked(item) {
    checking = false;
    checkAnswered = false;
    await ring(item);
  }

  // The quantity is whatever was offered for it and one otherwise, so a row
  // pressed after something was said rings what the button was showing.
  async function ring(item, qtyMilli = 1000) {
    await attemptWithOverride(() => run({ op: 'add', item_id: item.id, qty_milli: qtyMilli }));
    // Back to the scanner: the next thing a cashier does is almost always scan
    // the next item, and a screen left in a search box makes them hunt for it.
    hunt = '';
    found = [];
    heard = null;
    lookingUp = false;
    scanner?.focus();
  }

  // The screen shows the first wallet whatever the binding holds, so the
  // binding is made to agree with the screen rather than the other way round.
  $effect(() => {
    if (wallets.length > 0 && !wallets.includes(walletName)) walletName = wallets[0];
  });

  async function takeTender() {
    // The box a cashier types what was handed over into. `Number()` was here,
    // and "1e3" in it registered a thousand taka against a two hundred and
    // fifty three taka sale: the till then offered seven hundred and forty
    // seven in change, which is money out of the drawer for three characters.
    const amount = minorFrom(cash);
    if (amount === null || amount <= 0) {
      fault = t('till.an_amount_in_taka');
      return;
    }
    // A debt owed by nobody is money given away. This is the only record of it
    // anybody gets, on the customer's copy and on the shop's.
    if (payingBy === 'credit' && !view?.customer && !reference.trim()) {
      fault = t('till.say_who_owes_it');
      return;
    }
    // And a wallet with no name is the same thing one step removed: the money
    // is somewhere, and the drawer report cannot say where. Belt and braces
    // beside the default above, because this is the one that survives somebody
    // changing how the list is loaded.
    if (payingBy === 'wallet' && !walletName.trim()) {
      fault = t('till.say_which_wallet');
      return;
    }
    cash = '';
    const owed = refunding ? -1 : 1;
    // With the supervisor prompt, because a sale on account past what the shop
    // lets somebody owe is refused here and a supervisor standing at the
    // counter can allow that one. Without this the cashier was told no and
    // offered nothing.
    const reply = await attemptWithOverride(() =>
      run({
        op: 'add_tender',
        kind: payingBy,
        name: walletName,
        amount_minor: amount * owed,
        // The chosen customer's name goes on the paper, because a receipt in
        // somebody's hand says who took the goods. The id is what the account
        // is added up against, and it is already on the ticket.
        reference: payingBy === 'credit' && view?.customer
          ? (customers.find((one) => one.id === view.customer)?.name ?? reference)
          : reference,
        // A sale on account past what the shop lets somebody owe is a
        // supervisor's to allow, and an allowance is written down with its
        // hour.
        at_ms: Date.now(),
      }),
    );
    // The till refused because the name typed belongs to somebody the shop
    // wrote down. Which person it means comes from the core rather than from
    // the words of the refusal: the amount stays in the box, so choosing them
    // and pressing again is two presses rather than typing it all over.
    if (reply?.view?.needs_customer) {
      cash = String(amount);
      return;
    }
    reference = '';
    scanner?.focus();
  }

  // The person the till is asking the cashier to choose, when it has refused a
  // credit tender for naming somebody written down.
  const wantsCustomer = $derived(view?.needs_customer ?? null);

  /// Say who this basket is for, or nobody.
  async function chooseCustomer(id) {
    await attempt(() => run({ op: 'set_customer', customer: id === '' ? null : id }));
  }

  async function cancelSale() {
    // The basket is gone, so the camera is too: left reading it would put the
    // next customer's goods into one nobody has started.
    stopTheCamera();
    await attempt(() => run({ op: 'cancel_sale' }));
    // That basket is over, so the id it would have been rung under is too.
    ticketId = null;
    editing = null;
    scanner?.focus();
  }

  async function clearTenders() {
    await attempt(() => run({ op: 'clear_tenders' }));
    cash = '';
    scanner?.focus();
  }

  async function park() {
    // The basket is off the counter, so the camera is too.
    stopTheCamera();
    const label = parkAs.trim() || 'no name';
    parkAs = '';
    // The id is minted here, as a sale's is. A ULID would come from the
    // platform layer in the finished product; this is the same placeholder.
    const id = crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
    await attempt(() => run({ op: 'hold', ticket_id: id, held_at_ms: Date.now(), label }));
    // Parked, so this basket is not being rung now. Coming back off the shelf
    // is a new sale, and gets a new id when somebody presses.
    ticketId = null;
    scanner?.focus();
  }

  async function resume(held) {
    await attempt(() => run({ op: 'resume', ticket_id: held.id }));
    scanner?.focus();
  }

  async function discard(held) {
    await attempt(() => run({ op: 'discard_held', ticket_id: held.id }));
    scanner?.focus();
  }

  /// What the till says about this line and the shelf, if anything.
  ///
  /// Read off the view rather than worked out here: the till knows what the
  /// shop has and what the basket wants, and a screen doing the arithmetic
  /// again would be a second answer to disagree with the first.
  function shelfShortOf(at) {
    return (view?.beyond_the_shelf ?? []).find((short) => short.line === at) ?? null;
  }

  function shelfNote(short) {
    return t('till.shelf_short', {
      on_hand: qty(short.on_hand_milli),
      wanted: qty(short.wanted_milli),
    });
  }

  /// Write down somebody who is buying on account and is in nobody's list.
  ///
  /// The id is minted here because the core has no entropy, like a ticket's.
  /// The basket is pointed at them straight after, because that is the whole
  /// point: the debt goes against a person rather than a spelling.
  async function writeThemDown() {
    const name = reference.trim();
    if (!name) return;
    const id = crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
    const written = await attempt(() =>
      run({ op: 'write_customer', id, name, phone: newPhone.trim() || null }),
    );
    if (!written || written.view?.error) return;
    newPhone = '';
    await chooseCustomer(id);
  }

  /// The camera, when this shop has no scanner on a wire.
  ///
  /// Held open while it is reading and shut the moment it is not. The loop
  /// itself is in camera_read.js, because the back office reads a barcode the
  /// same way when somebody is writing an item down and two loops would be two
  /// answers to "when do we believe it".
  let camera = $state(null);
  let watching = $state(false);
  /// Whether the camera has given a picture yet, rather than merely been asked
  /// for. See the panel below.
  let picture = $state(false);
  let reading = null;
  /// Which press this is, so a second one landing while the first is still
  /// opening the camera cannot leave a reader running behind a screen with no
  /// picture on it. Opening is not instant: the camera has to be asked for, and
  /// a person pressing twice because nothing happened yet is the ordinary case,
  /// not the odd one.
  let cameraTurn = 0;

  async function readWithTheCamera() {
    if (watching) {
      stopTheCamera();
      return;
    }
    fault = null;
    const mine = ++cameraTurn;
    watching = true;
    // Nothing yet. Set when the picture arrives, so the panel says what it is
    // doing rather than telling somebody to hold a label up to a black frame.
    picture = false;
    // The picture element appears with `watching`, so the stream is attached
    // after the screen has drawn rather than to a picture that is not there.
    await tick();
    // A basket is more than one thing. A cashier with no scanner on a wire
    // would otherwise press a button for every item, with a customer standing
    // there, so the camera stays open while goods are being rung and the next
    // one is the next thing held in front of it. A price check is one question
    // about one thing and stops after it, which is the other half of the same
    // rule: what the camera does next is what the cashier does next.
    const held = await readFromCamera({
      video: camera,
      // Kept open, and the decision about what to do with the next label is
      // made when it arrives rather than when the camera opened: a cashier who
      // switches to "what does this cost" with the camera running would
      // otherwise go on ringing goods into the basket.
      keepLooking: true,
      onCode: async (code) => {
        // Through the same door the scanner's digits go through, so the shelf
        // rule, the refund and the price check are one path and not two.
        barcode = code;
        if (checking) {
          // One question about one thing, so the camera has done its job.
          stopTheCamera();
          await check();
          return;
        }
        await scan();
      },
      onTrouble: (why) => {
        watching = false;
        picture = false;
        fault = why === CANNOT_READ_HERE ? t('till.camera_not_here') : t('till.camera_refused');
      },
    });
    if (mine !== cameraTurn) {
      // Stopped while it was opening. The handle is the only way to let the
      // camera go, and nothing else is holding it.
      held.stop();
      return;
    }
    reading = held;
  }

  /// A camera nobody is looking at is a light on the counter and a flat battery
  /// by the afternoon, and it reads nothing anyway: a browser stops handing a
  /// hidden page its frames. So the camera goes when the page does, and the
  /// cashier presses the button again when they come back.
  $effect(() => {
    const stopIfHidden = () => {
      if (document.visibilityState !== 'visible' && watching) stopTheCamera();
    };
    document.addEventListener('visibilitychange', stopIfHidden);
    return () => document.removeEventListener('visibilitychange', stopIfHidden);
  });

  function stopTheCamera() {
    // Counted up here as well, so a start still in flight knows it is stale.
    cameraTurn += 1;
    watching = false;
    picture = false;
    reading?.stop();
    reading = null;
  }

  async function scan() {
    const code = barcode.trim();
    if (!code) return;
    barcode = '';
    const reply = await attemptWithOverride(() => run({ op: 'scan', barcode: code, qty_milli: 1000 }));
    // A barcode in nobody's catalogue, which during an outage is a delivery
    // that arrived this morning. The cashier can write it down here rather than
    // lose the sale, which is the whole of the cold-start promise.
    if (reply?.view?.error?.includes('no item in the catalogue')) {
      // The camera stops here, because what happens next is a person typing a
      // name and a price. There is one of this panel, so a second unknown label
      // read while the first is still being written down would take its place
      // and the first would be neither in the basket nor on the screen: the
      // cashier would have typed a name for something that is no longer there.
      stopTheCamera();
      unknown = code;
      newName = '';
      newPrice = '';
      newVat = '15';
    }
    scanner?.focus();
  }

  /// Write down what was just scanned, and sell it.
  ///
  /// The id is minted here because the core has no entropy, like a ticket's.
  /// What the shop later agrees replaces this, which is why the id matters more
  /// than anything typed into it.
  async function writeItDown() {
    const price = minorFrom(newPrice);
    if (price === null) {
      fault = t('till.price_is_taka_and_poisha');
      return;
    }
    // Basis points, read the way the back office reads the same field. The
    // ceiling matters: a rate over a hundred percent is refused by every till
    // that reads the item afterwards.
    const vat_bp = minorFrom(newVat);
    if (vat_bp === null || vat_bp > 10_000) {
      fault = t('till.tax_rate_range');
      return;
    }
    if (!newName.trim()) {
      fault = t('nameless-item');
      return;
    }
    const code = unknown;
    const reply = await attempt(() =>
      run({
        op: 'quick_add',
        id: crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26),
        barcode: code,
        name: newName.trim(),
        price_minor: price,
        vat_bp,
      }),
    );
    if (!reply || reply.view?.error) return;
    unknown = null;
    // Straight onto the ticket: the customer is standing there, which is why
    // any of this exists. Through the override path like every other scan: an
    // item written down at the till with nothing on the shelf behind it is
    // exactly the one a shop that blocks on stock will refuse.
    await attemptWithOverride(() => run({ op: 'scan', barcode: code, qty_milli: 1000 }));
    scanner?.focus();
  }

  /// Take the amount in the box as whatever the cashier chose to take it as.
  ///
  /// One box serves both rows: the amount, and the kind underneath it. Pressing
  /// Enter used to mean cash however the kind was set.
  async function takeWhatWasChosen() {
    if (payingBy === 'cash') {
      await tender();
      return;
    }
    await takeTender();
  }

  /// Cash, the amount in the box, the way this ticket runs.
  ///
  /// The direction was on the other path and not on this one, and this is the
  /// path a cashier presses: the button beside the box. So a refund of 90.00
  /// where the cashier typed 90 and pressed it recorded ninety taka coming in
  /// on a ticket handing ninety out, and the screen answered that 180.00 was
  /// still to hand back. The core refuses that now as well, by the same rule
  /// and in its own words, which is what makes this a fix rather than a patch
  /// on one screen.
  async function tender() {
    const amount_minor = minorFrom(cash);
    if (amount_minor === null || amount_minor <= 0) {
      fault = t('till.an_amount_in_taka');
      return;
    }
    cash = '';
    await attempt(() =>
      run({
        op: 'add_cash',
        amount_minor: theWayThisTicketRuns(amount_minor, refunding),
        at_ms: Date.now(),
      }),
    );
    scanner?.focus();
  }

  async function exact() {
    if (outstanding === 0) return;
    // Negative on a refund, which is money going back across the counter.
    await attempt(() => run({ op: 'add_cash', amount_minor: outstanding, at_ms: Date.now() }));
    scanner?.focus();
  }

  /// Lay the last sale out on paper.
  ///
  /// `rungAtMs` is the moment the sale was committed, passed in rather than
  /// read again here. Reading the clock a second time means the paper and the
  /// ledger are two readings of one fact, and a sale committed at 23:59:59.9
  /// and printed a fifth of a second later puts the customer's copy in a
  /// different day from the shop's books. That is the one disagreement a
  /// receipt exists to prevent.
  /// Lay the last sale out as a Mushak 6.3 tax invoice and print it.
  ///
  /// A4 and Bengali, because the form is. Everything on it comes from the sale
  /// as it was rung and from the shop's own record, and the one thing the form
  /// asks for that this till cannot supply, supplementary duty, is left blank
  /// rather than printed as a nought.
  async function printTaxInvoice() {
    if (!lastSale) return;
    // Belt and braces beside the markup above: the two protect against
    // different mistakes, and the one this stops is a later screen calling
    // this without the gate.
    if (goodsCameBack(lastSale)) return;
    receipt = null;
    creditNote = null;
    taxInvoice = lastSale;
    // The same wait the receipt uses: the browser needs the page laid out
    // before it is asked to print it.
    await new Promise((settle) => setTimeout(settle, 50));
    window.print();
  }

  /// Lay the last refund out as a Mushak 6.7 credit note and print it.
  ///
  /// The other document, and the one the Act asks for when goods come back.
  /// Refused rather than printed when section 52(1)(f) wants the buyer named
  /// and the sale names nobody: a note without them cannot be used to claim the
  /// adjustment, which is the whole reason a buyer asks for it.
  async function printCreditNote() {
    if (!lastSale || !goodsCameBack(lastSale)) return;
    if (theNoteWouldBeRefused(lastSale, lastSale.buyer)) return;
    receipt = null;
    creditNote = lastSale;
    // The same wait the receipt uses: the browser needs the page laid out
    // before it is asked to print it.
    await new Promise((settle) => setTimeout(settle, 50));
    window.print();
  }

  async function printReceipt(rungAtMs = Date.now()) {
    // The width is the paper's, not the screen's. 32 characters is a 58mm roll,
    // which is what a small shop has.
    const reply = await attempt(() =>
      run({
        op: 'receipt',
        width: 32,
        rung_at: new Date(rungAtMs).toLocaleString('en-GB'),
        // Paper is English, whatever the screen is set to. Three reasons and
        // they all point the same way: no ESC/POS code page carries Bangla, so
        // a thermal printer gets English regardless; the layout pads by
        // counting characters, which Bangla defeats, so a Bangla slip comes out
        // ragged; and a shop with two languages on its counter should not have
        // two shapes of receipt in its records. `{}` is the core's own English.
        words: {},
      }),
    );
    receipt = reply?.view?.receipt ?? null;
    // One document on the screen at a time. Whichever A4 page was last printed
    // is not what this button asked for.
    taxInvoice = null;
    creditNote = null;
    if (receipt) {
      // Left to the browser's own dialog rather than driven from here: a
      // printer, a PDF and a preview are all the same button to a shopkeeper.
      await new Promise((settle) => setTimeout(settle, 50));
      window.print();
    }
  }

  async function checkout() {
    // The basket is done, so the camera is done. Left open it would read the
    // next customer's goods into a sale nobody has started.
    stopTheCamera();
    // One id for this basket, however many times it is tried.
    //
    // The id and the clock come from here, because the core mints neither. A
    // ULID would be minted by the platform layer in the finished product; this
    // is a placeholder and is marked as one in todo.md.
    //
    // Minted per basket rather than per press, and that is the whole of the
    // idempotency this shop has. A checkout can be durable and still come back
    // as a failure: the sale is written and flushed, and something after that
    // fails, so the cashier is told it did not happen and presses again. With
    // a fresh id each press the shop takes two sales for one basket and cannot
    // tell; with this one it takes the same sale twice, which it deduplicates
    // on the shop and the id, and the second press is a replay.
    ticketId ??= crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
    // Read once. The same number goes into the ledger and onto the paper, or
    // they are two answers to when this sale happened.
    const rungAtMs = Date.now();
    // What this sale was, held before it is rung, because ringing clears the
    // basket. A tax invoice asked for afterwards is about this sale, and the
    // screen by then is about the next customer.
    const asItWasRung = {
      lines: view?.lines ?? [],
      net_minor: view?.net_minor ?? 0,
      vat_minor: view?.vat_minor ?? 0,
      total_minor: view?.total_minor ?? 0,
    };
    const soldTo = customers.find((one) => one.id === view?.customer) ?? null;
    const reply = await attempt(() =>
      run({ op: 'checkout', ticket_id: ticketId, rung_at_ms: rungAtMs }),
    );
    if (reply && !reply.view.error) {
      // Rung. The next basket is a different sale.
      ticketId = null;
    }
    // Between customers, which is the only safe moment: it rewrites a couple of
    // megabytes and the till decides whether the log is long enough to bother.
    // Nothing called it before, so the log grew for the life of the device and
    // every boot replayed all of it.
    run({ op: 'checkpoint' }).catch(() => {
      // Housekeeping. A till that could not tidy up still sells, and the next
      // sale will try again.
    });
    // A line left open belongs to a basket that no longer exists, and the next
    // sale would open with the second item of the last one expanded.
    editing = null;
    if (reply && !reply.view.error) {
      // Held for the tax invoice, if anybody asks for one. The receipt number
      // comes from the reply rather than from the screen: a sale rung with no
      // numbers left is numbered by the shop afterwards, and the form would
      // otherwise carry whatever the last sale had.
      lastSale = {
        ...asItWasRung,
        buyer: soldTo,
        receiptNo: reply.view?.receipt_no ?? null,
        rungAt: new Date(rungAtMs),
        // What this one adjusts, for the credit note. Read here rather than at
        // print time because the refund's own state is cleared by ringing it,
        // the same reason the lines above are held.
        original: refundOriginal,
      };
      refundOriginal = null;
      await printReceipt(rungAtMs);
    }
    scanner?.focus();
  }
</script>

<main>
  <header>
    <h1>openpos</h1>
    <div class="state">
      {#if storage === 'opfs' && keeping === 'evictable'}
        <!-- On the device and evictable. What is in there is unsent sales and
             the receipt numbers this terminal was given, so a shop that leaves
             them here for a week is trusting a promise the browser refused to
             make. -->
        <span class="warn" title={t('till.keep_not_promised')}>
          {t('till.on_this_device_not_promised')}
        </span>
      {:else if storage === 'opfs'}
        <span class="good" title={t('till.keeps_through_close')}>{t('till.on_this_device')}</span>
      {:else if storage === 'memory'}
        <span class="warn" title={t('till.keeps_nothing')}>{t('till.memory_only')}</span>
      {:else}
        <!-- Opening, unavailable, or not enrolled yet. The code itself was
             printed here, so a Bangla till in the one state that matters, the
             one where its ledger could not be opened, said "unavailable" in
             English beside a screen of Bangla. The code is the fallback for a
             state this build has not been taught to say. -->
        <span class="warn">{t(`till.storage_${storage.replace(/ /g, '_')}`, {}, storage)}</span>
      {/if}
      <span>{t('till.to_send', { count: view?.unsynced_sales ?? 0 })}</span>
      {#if roundsFailing}
        <!-- Only while rounds are failing. A button offered when everything
             works is a button somebody presses instead of trusting the loop,
             which is the opposite of what this is for. -->
        <button class="link" onclick={tryNow} disabled={busy}>{t('till.try_now')}</button>
      {/if}
      {#if newBuildWaiting}
        <!-- Downloaded and waiting. It takes over at the first moment there is
             no basket, no money on a ticket and nothing unsent, which is what
             stops a screen reloading under a cashier mid-sale. -->
        <span class="good" title={t('till.new_build_waiting_why')}>{t('till.new_build_waiting')}</span>
      {/if}
      {#if (view?.stock_rule ?? 0) > 0 && view?.shelf_known === false}
        <!-- The shop has asked this till to do something about the shelf and
             the till has nothing to do it with yet: the figures arrive two
             hundred items at a time, five minutes apart. It says nothing about
             the shelf until it has been round once, which is right, and this
             says so rather than leaving an owner who has just turned the rule
             on to watch the counter and conclude it does not work. -->
        <span class="warn" title={t('till.learning_the_shelf_why')}>
          {t('till.learning_the_shelf')}
        </span>
      {/if}
      <span>{t('till.numbers_left', { count: view?.receipt_numbers_left ?? 0 })}</span>
      <!-- Sales already rung with no number on them, and only when there are
           any. The count beside this one says how many numbers are left, which
           says the shape of that problem and not its size: a shop cannot tell
           one sale waiting from forty, and forty is a morning's trading with an
           inspector's question attached. -->
      {#if (view?.unnumbered_sales ?? 0) > 0}
        <span class="warn" title={t('till.waiting_for_numbers_why')}>
          {t('till.waiting_for_numbers', { count: view.unnumbered_sales })}
        </span>
      {/if}
      <span class={syncTrouble ? 'warn' : ''}>{syncing}</span>
      <!-- The figure that cannot lie by standing still. A frozen tab stops its
           worker, and the line beside this one then keeps saying whatever it
           said when the freezing started. -->
      {#if sinceReached !== null && sinceReached >= TOO_LONG_MS}
        <span class="warn" title={t('till.hidden_tab_stops')}>
          {t('till.not_reached', { minutes: Math.floor(sinceReached / 60_000) })}
        </span>
      {:else if lastReached !== null}
        <span title={t('till.last_reached')}>
          {t('till.reached_the_shop', {
            at: new Date(lastReached).toLocaleTimeString('en-GB'),
          })}
        </span>
      {/if}
      {#if operator}
        <button class="link" onclick={signOut}>{t('till.sign_out', { name: operator.name })}</button>
      {/if}
      <!-- The other language, named in itself: somebody who cannot read this
           screen cannot be asked to find a word for their own language in it.
           Two languages, so the button is the other one rather than a list. -->
      {#if offered.length > 1}
        <button
          class="link"
          onclick={() => speak(offered.find((one) => one.code !== language)?.code)}
          title={t('till.language')}
        >
          {offered.find((one) => one.code !== language)?.name}
        </button>
      {/if}
    </div>
  </header>

  {#if refused}
    <!-- Above everything, because nothing below it is reaching the shop. -->
    <p class="fault" role="alert">
      {t('till.device_refused')}
      {#if waiting > 0}
        {t('till.device_refused_waiting', { count: waiting })}
      {/if}
    </p>
  {/if}

  {#if blocked}
    <!-- The one failure at a till that somebody standing behind the counter
         can fix in ten seconds. Until this existed the way through it was to
         sign out and back in as the supervisor, in front of the customer, and
         the cashier retyped what they had already typed. -->
    <section class="carry">
      <p class="why">{t('till.needs_a_supervisor')}</p>
      <input
        bind:value={supervisorPin}
        type="password"
        inputmode="numeric"
        placeholder={t('till.supervisor_pin')}
        disabled={busy}
      />
      <span class="row">
        {#each people.filter((one) => one.may_authorise) as one (one.id)}
          <button onclick={() => allowIt(one)} disabled={busy}>
            {t('till.allows_it', { name: one.name })}
          </button>
        {/each}
        <button class="quiet" onclick={() => { blocked = null; supervisorPin = ''; }} disabled={busy}>
          {t('till.leave_it')}
        </button>
      </span>
      {#if people.filter((one) => one.may_authorise).length === 0}
        <p class="why">{t('till.nobody_may_authorise')}</p>
      {/if}
    </section>
  {/if}

  {#if refused || carrying}
    <!-- The way out. A device the shop will not take sales from is holding the
         only record of goods that left it, and enrolling again as another
         terminal abandons them. So they are read off it and carried. -->
    <section class="carry">
      <button onclick={showCarrying} disabled={busy}>
        {carrying ? t('till.read_them_again') : t('till.what_is_still_here')}
      </button>
      {#if carrying}
        {#if carrying.sales.length === 0}
          <p class="why">{t('till.nothing_waiting_here')}</p>
        {:else}
          <p class="why">
            {t('till.carrying_summary', {
              count: carrying.sales.length,
              amount: money(carrying.total_minor),
            })}
            {#if carrying.sales.some((sale) => sale.salvaged)}
              {t('till.some_were_salvaged')}
            {/if}
            {t('till.carry_instructions')}
          </p>
          <ul class="found">
            {#each carrying.sales as sale (sale.id)}
              <li>
                <span class="detail">
                  {money(sale.total_minor)}
                  {#if sale.salvaged}&middot; {t('till.read_from_damaged_log')}{/if}
                </span>
              </li>
            {/each}
          </ul>
          <p class="why">
            {t('till.carry_mark', { mark: carrying.mark, letters: carrying.letters })}
          </p>
          <div class="row">
            <button onclick={saveCarried} disabled={busy}>{t('till.save_to_a_file')}</button>
            <button onclick={copyCarried} disabled={busy}>{t('till.copy_it')}</button>
          </div>
          <textarea readonly rows="4" value={carrying.bundle}></textarea>
        {/if}
      {/if}
    </section>
  {/if}

  {#if openElsewhere}
    <!-- The till is fine and open in another window on this device. The
         enrolment box below is hidden for exactly this case: a shopkeeper who
         followed it would mint a second terminal with its own receipt numbers,
         while the sales, the parked baskets and the numbers already handed out
         stayed in the window nobody is looking at. Seen on a real screen,
         underneath a sentence about access handles. -->
    {#if whatElseToTry(triedTheLedgerAgain)}
      <!-- The first advice can be a dead end: a browser goes on holding the
           shop for a window that has already gone, and then there is no other
           window to close. Said only after that advice has been tried, so a
           shopkeeper reads one instruction at a time. -->
      <p class="fault">{t(whatElseToTry(triedTheLedgerAgain))}</p>
    {/if}
    <!-- Said on the window that gave the shop up, so somebody coming back to it
         reads where it went rather than meeting a screen that will not sell. -->
    {#if shopMoved}
      <p class="why">{t('shared.the_shop_moved')}</p>
    {/if}
    <!-- What the window that has it said. Three answers and three sentences:
         it gave it up, it has a sale in progress, or nothing answered at all,
         which is a store held by a window that has already gone and is what the
         advice above is for. -->
    {#if askingSaid === 'busy'}
      <p class="fault">
        {askingBecause === 'counting'
          ? t('shared.the_other_window_is_counting')
          : t('shared.the_other_window_is_selling')}
      </p>
    {:else if askingSaid === 'nobody'}
      <p class="fault">{t('shared.no_window_answered')}</p>
    {:else if askingSaid === 'let_go'}
      <p class="why">{t('shared.the_other_window_let_go')}</p>
    {/if}
    <div class="row">
      <button onclick={openItAgain} disabled={busy || asking}>{t('shared.try_again')}</button>
      <!-- The other window is alive and running this same code, so it can be
           asked. It is the window nobody is standing at, by definition: the
           person is standing at this one. -->
      <button onclick={askTheOtherWindow} disabled={busy || asking}>
        {t('shared.ask_the_other_window')}
      </button>
    </div>
  {/if}

  <!-- Never to a till that already knows which shop it is. A reload draws this
       before the store is open, and a store that is slow, or held for a moment
       by a window that has just gone, left a till with an identity showing a box
       asking for an enrolment code. Somebody at a counter who types one there is
       not fixing anything: they are minting a second terminal with its own block
       of receipt numbers, and the sales on the first one stay where they are.
       A till that has never been enrolled has no identity written down and sees
       the box at once, which is the only time it is the right thing to show. -->
  {#if (!enrolled || refused) && !openElsewhere && !stillOpening}
    <div class="row enrol">
      <input
        bind:value={code}
        onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); join(); } }}
        placeholder={t('till.enrolment_code')}
        autocomplete="off"
        disabled={busy}
      />
      <button onclick={join} disabled={busy}>{t('till.enrol')}</button>
    </div>
  {/if}

  {#if enrolled && !operator}
    <!-- Nobody is at the till. Every permission refuses until somebody is, and
         a screen that let a sale start anyway would refuse at the till point
         where it matters most. -->
    <section class="signin">
      {#if people.length === 0}
        <p class="fault">{t('till.nobody_added_yet_long')}</p>
      {:else if !picked}
        <p>{t('till.who_is_at_the_till')}</p>
        <div class="who">
          {#each people as person (person.id)}
            <button onclick={() => { picked = person; pin = ''; }}>
              {label(person, twiceOver)}
            </button>
          {/each}
        </div>
      {:else}
        <p>{t('till.enter_your_pin', { name: label(picked, twiceOver) })}</p>
        <div class="row">
          <input
            type="password"
            bind:value={pin}
            onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); signIn(); } }}
            inputmode="numeric"
            autocomplete="off"
            disabled={busy}
          />
          <button onclick={signIn} disabled={busy}>{t('till.sign_in')}</button>
          <button onclick={() => { picked = null; pin = ''; }}>{t('till.back')}</button>
        </div>
      {/if}
    </section>
  {/if}

  <!-- Over the page rather than at the top of it, for the reason the back
       office's is: a message a cashier cannot see is a message that did not
       happen, and they press the button again. -->
  {#if fault}
    <p class="fault floats" role="alert">{fault}</p>
  {/if}
  {#if done}
    <p class="why floats done" role="status">{done}</p>
  {/if}

  <input
    class="scan"
    bind:this={scanner}
    bind:value={barcode}
    onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); checking ? check() : scan(); } }}
    placeholder={checking ? t('till.scan_to_check') : t('till.scan')}
    autocomplete="off"
    inputmode="numeric"
    disabled={busy}
  />

  {#if operator}
    <div class="row">
      <button
        class="quiet"
        onclick={() => { checking = !checking; checkAnswered = false; scanner?.focus(); }}
        disabled={busy}
      >
        {checking ? t('till.back_to_scanning') : t('till.what_does_this_cost')}
      </button>
      <!-- For a shop with no scanner on a wire, and for a second counter on a
           market day. What it reads goes through the same door the scanner's
           digits go through. -->
      <button class="quiet" onclick={readWithTheCamera} disabled={busy}>
        {watching ? t('till.stop_the_camera') : t('till.read_with_the_camera')}
      </button>
      {#if checking}
        <span class="why">{t('till.nothing_here_goes_in')}</span>
      {/if}
    </div>
  {/if}

  {#if watching}
    <section class="camera">
      <!-- Muted and inline, or a tablet takes the picture full screen and the
           cashier loses the basket behind it. -->
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        bind:this={camera}
        muted
        playsinline
        autoplay
        onloadedmetadata={() => { picture = true; }}
      ></video>
      <!-- Only once there is something to hold a label in front of. The
           browser asks whether the camera may be used the first time a device
           opens one, and until somebody answers, the frame is black: telling a
           cashier to hold the label in it is telling them to hold a packet up
           to nothing. -->
      {#if picture}
        <p class="why">{t('till.hold_the_label_still')}</p>
      {:else}
        <p class="why">{t('till.camera_opening')}</p>
      {/if}
    </section>
  {/if}

  {#if checking && checkAnswered && view?.checked}
    <section class="checked">
      <p class="name">
        {view.checked.item.name}
        {#if language === 'bn' && view.checked.item.name_bn && view.checked.item.name_bn !== view.checked.item.name}
          <span class="bangla">{view.checked.item.name_bn}</span>
        {/if}
      </p>
      <p class="each">
        {money(view.checked.each_minor)} {t('till.each')}, {view.checked.item.unit}
        {#if view.checked.vat_minor > 0}
          &middot; {t('till.including_tax', { vat: money(view.checked.vat_minor) })}
        {/if}
      </p>
      <button onclick={() => ringChecked(view.checked.item)} disabled={busy}>
        {t('till.ring_one_up')}
      </button>
    </section>
  {/if}

  {#if operator}
    {#if lookingUp}
      <div class="row lookup">
        <input
          bind:value={hunt}
          oninput={look}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); look(); } }}
          placeholder={t('till.look_up_placeholder')}
          autocomplete="off"
          disabled={busy}
        />
        <button onclick={listen} disabled={busy || !hunt.trim()}>
          {t('till.take_it_as_a_phrase')}
        </button>
        <button onclick={() => { lookingUp = false; hunt = ''; found = []; heard = null; scanner?.focus(); }}>
          {t('till.back_to_scanning')}
        </button>
      </div>
      {#if heard}
        <!-- What it thought it heard, always, whether it found anything or not.
             A cashier who cannot see this has no way to tell a wrong item from
             a misheard word, and no way to learn what the till listens to. -->
        <p class="heard">
          <span class="took">{heard.used.join(' ') || t('till.heard_nothing_usable')}</span>
          {#if heard.ignored.length > 0}
            <span class="set-aside">{t('till.heard_set_aside', { dropped: heard.ignored.join(' ') })}</span>
          {/if}
        </p>
        {#if heard.qty_note}
          <!-- Worded by the core. A screen inventing its own would be a second
               place the rule lives, and the two would drift. -->
          <p class="empty">{heard.qty_note}</p>
        {/if}
      {/if}
      {#if found.length > 0}
        <ul class="found">
          {#each found as item, at (item.id)}
            <li>
              <!-- The quantity is whatever the core was willing to stand behind,
                   and one otherwise. It is on the button, so what is about to be
                   rung is what the cashier is looking at when they press it. -->
              <button onclick={() => ring(item, heard?.qty_milli ?? 1000)} disabled={busy}>
                <span class="name">
                  {#if heard?.qty_milli && heard.qty_milli !== 1000}
                    <span class="count">{heard.qty_milli / 1000} ×</span>
                  {/if}
                  {item.name}
                  {#if language === 'bn' && item.name_bn && item.name_bn !== item.name}
                    <!-- A screen renders Bangla; thermal paper is the thing that
                         cannot, and the receipt says so line by line.
                         Shown beside the English only on a Bangla screen. A till
                         set to English shows one name per line: the second one
                         is there so somebody who reads Bangla can find the item,
                         and on an English till it is a second line of a script
                         nobody at that counter reads, on every row of a list a
                         cashier reads at speed with a customer waiting. The name
                         itself is untouched: it is still typed here, still
                         stored, still what a Bangla search matches on. -->
                    <span class="bangla">{item.name_bn}</span>
                  {/if}
                  {#if heard && at === 0 && !heard.sure}
                    <!-- Said only when it is not sure, and never the reverse: a
                         mark on every row is a mark nobody reads. -->
                    <span class="unsure">{t('till.heard_not_certain')}</span>
                  {/if}
                </span>
                <span class="each">{money(item.price_minor)}</span>
              </button>
            </li>
          {/each}
        </ul>
      {:else if hunt.trim()}
        <p class="empty">{t('till.nothing_by_that_name')}</p>
      {/if}
    {:else if unknown}
      <!-- A delivery that arrived while the line was down. Written here rather
           than lost: the customer is holding it. -->
      <section class="unknown">
        <p class="why">{t('till.unknown_item', { barcode: unknown })}</p>
        <input bind:value={newName} placeholder={t('till.what_it_is')} disabled={busy} />
        <div class="row">
          <input bind:value={newPrice} placeholder={t('till.price_in_taka')} inputmode="decimal" disabled={busy} />
          <input bind:value={newVat} placeholder={t('till.tax_percent')} inputmode="decimal" disabled={busy} />
        </div>
        <div class="row">
          <button onclick={writeItDown} disabled={busy}>{t('till.write_it_down_and_sell')}</button>
          <button class="quiet" onclick={() => { unknown = null; scanner?.focus(); }} disabled={busy}>
            {t('till.leave_it')}
          </button>
        </div>
      </section>
    {:else}
      <button class="lookup" onclick={() => { lookingUp = true; }} disabled={busy}>
        {t('till.no_barcode')}
      </button>
    {/if}
  {/if}

  <ul class="lines">
    {#each view?.lines ?? [] as line, at (line.item_id + line.name)}
      <li class:picked={editing === at}>
        <!-- The whole line is the button. Nothing said so, and a cashier who
             wanted three of something had to guess that tapping the line was
             how: walked, and the quantity control could not be found. The mark
             on the right is the only thing on the row that is not a fact about
             the sale, so it stays faint until the row is touched. -->
        <button
          class="pick"
          title={t('till.tap_to_change')}
          onclick={() => (editing = editing === at ? null : at)}
          disabled={busy}
        >
          <span class="name">{line.name}</span>
          <span class="qty">{qty(line.qty_milli)}</span>
          <span class="each">{money(line.unit_price_minor)}</span>
          <span class="sum">{money(line.total_minor)}</span>
          <span class="more" aria-hidden="true">{editing === at ? '\u2013' : '\u203a'}</span>
        </button>
        {#if line.discount_minor !== 0}
          <span class="gave">{discountNote(line)}</span>
        {/if}
        {#if shelfShortOf(at)}
          <!-- What the shop believes is there, against what this basket wants.
               Under the line rather than as a banner: a cashier told "something
               is short" has to work out which of five things it was. -->
          <span class="shelf">{shelfNote(shelfShortOf(at))}</span>
        {/if}
        {#if editing === at}
          {@const howMany = howManyOnTheLine(line.qty_milli)}
          <!-- Under the line it changes, not in a dialog over it: a cashier
               correcting the third of five things is looking at the third. -->
          <div class="edit">
            <!-- How many, not which way. A refund's line holds a quantity below
                 nothing and this box used to show it: a cashier taking back
                 three saw "-1", and neither "3" nor "-3" could be typed back
                 into it, because the first is a sale on a return ticket and the
                 second is a sign in a quantity box, which is the thing that
                 wrote a thousand off a shelf. What was left was the buttons,
                 where minus meant one more coming back and plus took the line
                 off. So the whole editor counts, the line above it shows the
                 direction, and `askedForOnThisTicket` puts the sign back on. -->
            <button onclick={() => changeQty(at, howMany - 1000)} disabled={busy}>&minus;</button>
            <!-- Typed as well as stepped, because a shop sells rice by the kilo
                 and a kilo and a half is two presses of nothing. -->
            <input
              class="count"
              value={qty(howMany)}
              onchange={(e) => typeQty(at, e.currentTarget.value)}
              inputmode="decimal"
              aria-label={t('till.how_many')}
              disabled={busy}
            />
            <button onclick={() => changeQty(at, howMany + 1000)} disabled={busy}>+</button>
            <!-- Offered whatever this person's ceiling is, for the reason the
                 whole-ticket boxes below are: a cashier's ceiling is zero, so
                 gating on it hid the box from everybody who would ever need a
                 supervisor for it. The refusal names the rate that was asked
                 for, and the supervisor's PIN allows that rate and no more. -->
            <input
              class="off"
              value={line.discount_bp ? line.discount_bp / 100 : ''}
              onchange={(e) => discountLine(at, e.currentTarget.value)}
              placeholder={ceiling > 0 ? t('till.percent_off') : t('till.percent_off_asks')}
              inputmode="decimal"
              disabled={busy}
            />
            <!-- The same thing said the way a shop says it. Both are offered
                 because both are said: "ten percent" over a counter and
                 "twenty taka off" across it. -->
            <input
              class="off"
              value={line.discount_amount_minor && line.discount_bp === 0
                ? (line.discount_amount_minor / 100).toFixed(2)
                : ''}
              onchange={(e) => takeOffLine(at, e.currentTarget.value)}
              placeholder={ceiling > 0 ? t('till.amount_off') : t('till.amount_off_asks')}
              inputmode="decimal"
              disabled={busy}
            />
            <!-- Damaged goods, a short weight, a price somebody was quoted.
                 Offered to whoever is at the till, for the reason the discount
                 boxes beside it are: it was shown only to somebody already
                 permitted, and a cashier is not, so the supervisor's PIN could
                 never be asked for. A supervisor standing at the counter is
                 what this till is for, and a control with one behind it is
                 offered rather than hidden.
                 
                 It also carries what this line is priced at, so hiding it hid
                 the price as well as the box. -->
            <input
              class="off"
              value={(line.unit_price_minor / 100).toFixed(2)}
              onchange={(e) => priceLine(at, e.currentTarget.value)}
              aria-label={mayOverride ? t('till.price_each') : t('till.price_each_asks')}
              inputmode="decimal"
              disabled={busy}
            />
            <button class="drop" onclick={() => drop(at)} disabled={busy}>{t('till.take_it_off')}</button>
          </div>
        {/if}
      </li>
    {:else}
      <li class="empty">{t('till.nothing_rung')}</li>
    {/each}
  </ul>

  <section class="totals">
    <div><span>{t('till.net')}</span><span>{money(view?.net_minor ?? 0)}</span></div>
    {#if (view?.discount_minor ?? 0) !== 0}
      <div><span>{t('till.discount')}</span><span>{money(-view.discount_minor)}</span></div>
    {/if}
    <div><span>{t('till.vat')}</span><span>{money(view?.vat_minor ?? 0)}</span></div>
    <div class="due"><span>{t('till.total')}</span><span>{money(total)}</span></div>
    <!-- The label says which way the money went, so the figure beside it is a
         size. It used to be the signed number, which read "Given back -90.00"
         the moment the tender itself was signed correctly: the minus was
         arithmetic showing through, and the line under it has always shown "To
         refund" as a size for the same reason. -->
    <div>
      <span>{refunding ? t('till.given_back') : t('till.paid')}</span>
      <span>{money(Math.abs(view?.tendered_minor ?? 0))}</span>
    </div>
    <!-- One line, and only one: whichever of these the cashier is about to do is
         the only question they have. Showing change on a refund before anything
         has been handed over reads as money already given. -->
    {#if refunding && outstanding !== 0}
      <div class="owed"><span>{t('till.to_refund')}</span><span>{money(-outstanding)}</span></div>
    {:else if !refunding && outstanding > 0}
      <div class="owed"><span>{t('till.still_owed')}</span><span>{money(outstanding)}</span></div>
    {:else if settled && !refunding && view.change_minor > 0}
      <div class="change"><span>{t('till.change')}</span><span>{money(view.change_minor)}</span></div>
    {/if}
    <!-- Once a supply passes the value at which a tax invoice has to name the
         buyer. Said here, under the total, because that is the figure it is
         about and because the customer is still standing at the counter: after
         they have gone, nobody can ask them for a BIN. Not a refusal. The goods
         leave either way, and what the shop loses by not asking is its
         customer's input tax credit, which the customer finds out about later
         and comes back about. -->
    {#if view?.buyer_wanted}
      <!-- Which sentence depends on which way the goods are going, because the
           Act asks a different question each way: the value of the supply on
           the way out, the tax being given back on the way in. A shopkeeper
           reading "over 25,000" beside a basket of six thousand would be right
           to ignore it. -->
      <p class="why late">
        {view.is_refund ? t('till.name_the_buyer_getting_it_back') : t('till.name_the_buyer')}
      </p>
    {/if}
  </section>

  {#if operator && parked.length > 0}
    <section class="parked">
      <p class="why">
        {t('till.parked_still_to_deal_with')}
      </p>
      <ul>
        {#each parked as held (held.id)}
          <li>
            <span class="name">{held.label}</span>
            <span class="each">
              {t('till.lines_count', { count: held.lines })} &middot; {money(held.total_minor)}
            </span>
            <button onclick={() => resume(held)} disabled={busy}>{t('till.bring_it_back')}</button>
            <button class="drop" onclick={() => discard(held)} disabled={busy}>{t('till.throw_away')}</button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <div class="actions">
    {#if (view?.lines?.length ?? 0) > 0 && !refunding}
      <!-- The ceiling is shown rather than discovered. A cashier who may give
           five percent should not learn that by being refused ten, and one who
           may give nothing is told that a supervisor allows it.
           
           Offered whatever the ceiling is, which it was not: the box appeared
           only for somebody with a ceiling above zero, and every cashier in
           every shop has a ceiling of zero. So the customer asked for ten
           percent off, the cashier had nowhere to type it, and the supervisor's
           PIN could not be offered because nothing had been refused. The way
           round it was for the supervisor to sign in and ring the sale
           themselves, which puts it under their name and is the workaround the
           trail exists to make unnecessary. Found by walking as a cashier. -->
      <div class="row">
        <input
          bind:value={ticketOff}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); discountTicket(); } }}
          placeholder={ceiling > 0
            ? t('till.percent_off_ticket', { ceiling: ceiling / 100 })
            : t('till.percent_off_ticket_asks')}
          inputmode="decimal"
          disabled={busy}
        />
        <button onclick={discountTicket} disabled={busy}>{t('till.discount')}</button>
      </div>
      <div class="row">
        <input
          bind:value={ticketOffAmount}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); takeOffTicket(); } }}
          placeholder={ceiling > 0
            ? t('till.amount_off_ticket')
            : t('till.amount_off_ticket_asks')}
          inputmode="decimal"
          disabled={busy}
        />
        <button onclick={takeOffTicket} disabled={busy}>{t('till.take_it_off')}</button>
      </div>
    {/if}
    <div class="row">
      <!-- One box, and it is the amount for whichever tender is being taken:
           the button beside it takes cash, and the row below takes a wallet, a
           card or an account from the same figure. It was labelled "Cash taken"
           whatever was selected, so a cashier taking 27.50 on bKash had to type
           it into a box that said cash, and one who read the label and did not
           was refused with "enter an amount in taka" and nothing to say where.
           Found by walking a two-tender sale. -->
      <input
        bind:value={cash}
        onkeydown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            // Whatever the cashier chose below, not always cash. This box is
            // the amount for both, and pressing Enter after choosing a wallet
            // recorded the money as cash: the drawer then expected notes
            // nobody had put in it, and the evening count came up short by
            // exactly the wallet payment.
            takeWhatWasChosen();
          }
        }}
        placeholder={payingBy === 'cash' ? t('till.cash_taken') : t('till.how_much_taken')}
        inputmode="decimal"
        disabled={busy}
      />
      <button onclick={tender} disabled={busy}>{t('till.take_cash')}</button>
    </div>
    {#if operator && (view?.lines?.length ?? 0) > 0 && (!drawer || !drawer.open)}
      <!-- Beside the cash, not at the bottom with the drawer controls, because
           the moment it matters is the moment somebody is taking notes. -->
      <p class="why">{t('till.no_drawer_for_this_cash')}</p>
    {/if}
    {#if operator && (view?.lines?.length ?? 0) > 0}
      <div class="row">
        <select bind:value={payingBy} disabled={busy}>
          <option value="cash">{t('till.cash')}</option>
          <option value="wallet">{t('till.a_wallet')}</option>
          <option value="card">{t('till.card')}</option>
          <option value="credit">{t('till.on_account')}</option>
        </select>
        {#if payingBy === 'wallet'}
          <!-- Which one. A shop may take several, and the drawer report is read
               by name: "wallet 2,400.00" tells nobody who to chase. -->
          {#if wallets.length > 0}
            <select bind:value={walletName} disabled={busy}>
              {#each wallets as one (one)}
                <option value={one}>{one}</option>
              {/each}
            </select>
          {:else}
            <input bind:value={walletName} placeholder={t('till.which_wallet')} disabled={busy} />
          {/if}
        {/if}
        <!-- Also when the invoice has to name the buyer, whatever they are
             paying with. The picker used to hang off "on account" alone, so a
             wholesaler paying cash for thirty thousand of rice was told to pick
             who it was for on a screen with nowhere to pick: the advice asked
             for something the till did not offer. Nobody owes anything on a
             cash sale, so what follows the picker, the typed name and the
             phone, stays where it was. -->
        {#if payingBy === 'credit' || view?.buyer_wanted}
          <!-- Somebody the shop wrote down, when it has. What they owe is then
               added up against a person rather than against the spelling a
               cashier used that day, which is how two Karims share an account.
               A shop that has written nobody down still types a name. -->
          {#if customers.length > 0}
            <select value={view?.customer ?? ''} onchange={(e) => chooseCustomer(e.currentTarget.value)} disabled={busy}>
              <option value="">{t('till.somebody_not_on_the_list')}</option>
              {#each customers as one (one.id)}
                <!-- The name, then what they owe, then what they may owe at
                     most, each of them a phrase the dictionary holds. The limit
                     used to be appended as " of {amount}" in English: beside an
                     amount it read "owes 400.00 of 50.00", and with nothing
                     owed it read "Walk Limit Buyer of 1,500.50". -->
                <option value={one.id}>
                  {[
                    label(one, customersTwiceOver),
                    one.owed_minor
                      ? t('till.owes_short', { amount: money(one.owed_minor) })
                      : null,
                    one.limit_minor
                      ? t('till.limit_short', { amount: money(one.limit_minor) })
                      : null,
                  ]
                    .filter(Boolean)
                    .join(' — ')}
                </option>
              {/each}
            </select>
          {/if}
          {#if !view?.customer && payingBy === 'credit'}
            <input bind:value={reference} placeholder={t('till.who_owes_it')} disabled={busy} />
            <!-- Writing them down is what keeps two people with one name apart:
                 a debt against a typed name is added up under the spelling, and
                 the second Karim pays for the first one's rice. -->
            {#if reference.trim()}
              <input
                bind:value={newPhone}
                placeholder={t('till.their_phone')}
                inputmode="tel"
                disabled={busy}
              />
              <button onclick={writeThemDown} disabled={busy}>
                {t('till.write_them_down', { name: reference.trim() })}
              </button>
            {/if}
          {/if}
          {#if wantsCustomer}
            <!-- The till refused the name because the shop has written that
                 person down. Offered as a button rather than left to the
                 cashier to find in the list, because they are mid-sale with
                 somebody waiting. -->
            <button
              onclick={() => {
                const one = customers.find((person) => person.name === wantsCustomer);
                if (one) chooseCustomer(one.id);
              }}
              disabled={busy}
            >
              {t('till.on_their_account', { name: wantsCustomer })}
            </button>
          {/if}
        {/if}
        {#if payingBy === 'credit' && chosen}
          <!-- What they owed when the shop last said so, and when. Never the
               number alone: another till may have sold to them since, and a
               cashier reads a bare figure out across the counter as true. -->
          <span class="detail">
            {#if chosen.owed_minor}
              {t('till.owes', {
                amount: money(chosen.owed_minor),
                at: new Date(chosen.owed_as_of_ms).toLocaleTimeString('en-GB'),
              })}
            {:else if chosen.owed_as_of_ms}
              {t('till.owes_nothing', {
                at: new Date(chosen.owed_as_of_ms).toLocaleTimeString('en-GB'),
              })}
            {:else}
              {t('till.owed_unknown')}
            {/if}
            {#if chosen.limit_minor}
              &middot; {t('till.you_allow_them', { limit: money(chosen.limit_minor) })}
            {/if}
          </span>
        {:else if payingBy === 'wallet' || payingBy === 'card'}
          <input bind:value={reference} placeholder={t('till.their_reference')} disabled={busy} />
        {/if}
        <button onclick={takeTender} disabled={busy}>{t('till.take_it')}</button>
      </div>
    {/if}
    <!-- Disabled once a sale has been overpaid. What this button means is "the
         customer handed over exactly this", and on an overpaid sale it read
         "Exact (-287.25)" and quietly took the overpayment back out: the drawer
         came to the same figure and the receipt then said they paid the exact
         amount when they had handed over a five hundred note and taken change.
         A refund is the other way round and is what the negative is for. -->
    <button
      onclick={exact}
      disabled={busy || outstanding === 0 || (!refunding && outstanding < 0)}
    >
      {refunding
        ? t('till.refund_amount', { amount: money(-outstanding) })
        : t('till.exact', { amount: money(outstanding) })}
    </button>
    {#if operator && (view?.lines?.length ?? 0) === 0 && !refunding}
      <!-- Only on an empty basket: a refund is a whole ticket, never a line
           mixed into a sale. -->
      {#if askingReceipt}
        <!-- The paper is usually in their hand, and the number on it is what
             lets the shop check the refund against the sale. Skipping it is
             allowed: somebody who lost the receipt is still owed their money,
             and the sale says nobody named one. -->
        <div class="row">
          <input
            bind:value={refundAgainst}
            placeholder={t('till.receipt_on_their_paper')}
            disabled={busy}
            onkeydown={(event) => event.key === 'Enter' && startRefund()}
          />
          <button onclick={startRefund} disabled={busy}>{t('till.refund_against_it')}</button>
          <button class="quiet" onclick={startRefund} disabled={busy}>
            {t('till.they_have_not_got_it')}
          </button>
        </div>
      {:else}
        <button onclick={askForTheReceipt} disabled={busy}>{t('till.start_a_refund')}</button>
      {/if}
    {/if}
    {#if refunding && (view?.lines?.length ?? 0) === 0}
      <!-- A refund started by mistake, or one the customer changed their mind
           about. There was no way out of it: the till stayed in refund mode
           with nothing on the ticket, every scan came back as goods returning,
           and the only escape a cashier had was to reload the page. Found by
           walking into it. -->
      <button class="quiet" onclick={leaveTheRefund} disabled={busy}>
        {t('till.not_a_refund_after_all')}
      </button>
    {/if}
    {#if broughtBack}
      <!-- What the shop says was on that paper. The money here is what the
           customer paid, which is the whole reason for asking: rung by
           scanning the goods again they come back at today's catalogue price,
           so a basket sold with something off comes back at full price and the
           shop gives the discount away a second time. -->
      <section class="brought-back">
        <p>{t('till.what_was_on_this_one')}</p>
        <ul>
          {#each broughtBack.lines as line, at (at)}
            <li>
              <span class="what">{line.name}</span>
              <span class="detail">
                {t('till.charged_each', {
                  each: money(line.unit_price_minor),
                  qty: qty(Math.abs(line.qty_milli)),
                })}
                {#if line.discount_minor > 0}
                  · {t('till.came_off_it', { amount: money(line.discount_minor) })}
                {/if}
              </span>
              <input
                bind:value={comingBack[at]}
                aria-label={t('till.how_many_coming_back')}
                inputmode="decimal"
                disabled={busy}
              />
            </li>
          {/each}
        </ul>
        <div class="row">
          <button onclick={bringThemBack} disabled={busy}>{t('till.bring_these_back')}</button>
          <button class="quiet" onclick={() => { broughtBack = null; comingBack = {}; }} disabled={busy}>
            {t('till.scan_them_instead')}
          </button>
        </div>
      </section>
    {/if}
    {#if operator && (view?.lines?.length ?? 0) > 0 && !settled}
      <!-- Only while a sale is unpaid and has something on it. A parked sale is
           one nobody has taken money for, and there is nothing to park before
           the first scan. -->
      <div class="row">
        <input
          bind:value={parkAs}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); park(); } }}
          placeholder={t('till.whose_is_it')}
          disabled={busy}
        />
        <button onclick={park} disabled={busy}>{t('till.park_it')}</button>
      </div>
    {/if}
    {#if operator && (view?.tendered_minor ?? 0) !== 0}
      <!-- Whenever money has been entered, settled or not. The mis-key this
           exists for is five thousand where five hundred was meant, which is an
           overpayment, which counts as settled: hiding it then hid it exactly
           when it was wanted. -->
      <button class="quiet" onclick={clearTenders} disabled={busy}>{t('till.take_that_money_back')}</button>
    {/if}
    <!-- Held only while no money has been offered at all, which is not a sum.
         Whether what has been offered covers the basket is the core's answer
         and it comes worded. Short by so much, a refund that does not balance,
         change larger than there is cash to give it out of. That last
         one is reachable after a discount is taken off a basket already paid by
         wallet, and a button that greys itself out at that moment tells the
         cashier nothing. -->
    <button
      class="finish"
      onclick={checkout}
      disabled={busy || (view?.tendered_minor ?? 0) === 0}
    >{t('till.finish_sale')}</button>
    {#if operator && (view?.lines?.length ?? 0) > 0}
      <!-- Last, and set apart: it throws away the whole basket. Removing five
           lines one at a time is five chances to leave one behind, and the one
           left behind is rung to the next customer. -->
      <button class="abandon" onclick={cancelSale} disabled={busy}>{t('till.give_up_on_this_sale')}</button>
    {/if}
    {#if receipt}
      <!-- Held while anything else is in flight, like every other button here.
           Two taps in a row wrote the shop two reprints and opened the print
           window twice. -->
      <button onclick={printAgain} disabled={busy}>{t('till.print_again')}</button>
      <!-- The other document. Offered beside the reprint rather than instead of
           it: a customer takes the receipt, and a business buyer takes this as
           well, which is the paper their input tax credit hangs on. -->
      {#if lastSale && awkwardLines.length === 0 && !cameBack}
        <button onclick={printTaxInvoice} disabled={busy}>
          {t('till.print_tax_invoice')}
        </button>
      {/if}
      <!-- And the other document, for goods that came back. Offered only when
           the note would not be refused by the rule it exists for: over five
           thousand taka of tax it has to name the buyer, and one that does not
           cannot be used to claim the adjustment. -->
      {#if cameBack && !theNoteWouldBeRefused(lastSale, lastSale?.buyer)}
        <button onclick={printCreditNote} disabled={busy}>
          {t('till.print_credit_note')}
        </button>
      {/if}
    {/if}
  </div>
  <!-- What this document is and what it cannot do, beside the button rather
       than in a note somebody reads afterwards. A shop selling goods that carry
       supplementary duty must not hand this out, and the only place that can be
       said usefully is here. -->
  {#if receipt && lastSale && awkwardLines.length === 0 && !cameBack}
    <p class="why">{t('till.tax_invoice_why')}</p>
  {/if}
  <!-- And what the shop is not being handed, where the button would have been.
       Goods coming back need the other document, and a shop that is not told
       that will hand over the receipt and believe the paper side is done. -->
  {#if receipt && cameBack}
    <!-- The box for the form's ফেরতের কারণ, which section 52(1)(d) asks for as
         the nature of the adjustment. Typed by whoever is printing the note,
         because they are the person who knows, and not stored: a reprint asks
         again rather than repeating what somebody typed a week ago. -->
    <div class="row">
      <input
        bind:value={whyItCameBack}
        placeholder={t('till.why_it_came_back')}
        disabled={busy}
      />
    </div>
    {#if theNoteWouldBeRefused(lastSale, lastSale?.buyer)}
      <p class="why late">{t('till.credit_note_needs_the_buyer')}</p>
    {:else}
      <p class="why">{t('till.credit_note_why')}</p>
    {/if}
  {/if}
  <!-- And why not, when it cannot. Said where the button would have been, with
       the line named, because the shopkeeper has to know which item it is
       about. -->
  {#if receipt && lastSale && awkwardLines.length > 0 && !cameBack}
    <p class="why late">
      {t('till.tax_invoice_will_not_add_up', { name: awkwardLines[0].name })}
    </p>
  {/if}
  <div class="row">
    {#if false}
    {/if}
  </div>

  {#if operator}
    <section class="drawer">
      {#if !drawer || !drawer.open}
        <div class="row">
          <input
            bind:value={float_}
            placeholder={t('till.opening_float')}
            inputmode="decimal"
            disabled={busy}
          />
          <button onclick={openShift} disabled={busy}>{t('till.start_the_drawer')}</button>
        </div>

      {:else}
        <div class="drawerline">
          <span>{t('till.drawer_holds', { sales: drawer.sales })}</span>
          <strong>{money(drawer.expected_cash_minor)}</strong>
        </div>
        <!-- Money put in or taken out for a stated reason, while this drawer has
             been open. The figure above is already net of both, which is exactly
             why they have to be said here: a cashier watching "should hold" sit
             lower than the selling felt has no way, on this screen, to tell that
             somebody paid the delivery boy out of the till at four o'clock. The
             back office says it about a drawer already counted, which is the
             wrong end of the shop and the wrong end of the day to find it out
             from. The till knew both figures from the day the drawer was written
             and showed neither. -->
        {#if drawer.cash_in_minor || drawer.cash_out_minor}
          <p class="why">
            {[
              drawer.cash_in_minor
                ? t('till.drawer_cash_in', { amount: money(drawer.cash_in_minor) })
                : null,
              drawer.cash_out_minor
                ? t('till.drawer_cash_out', { amount: money(drawer.cash_out_minor) })
                : null,
            ]
              .filter(Boolean)
              .join(', ')}
          </p>
        {/if}
        <!-- Which day this drawer belongs to, when it is not this one. A drawer
             nobody closes stays open, so a cashier arriving in the morning reads
             what it holds and has no way to tell that the sales in it are
             yesterday's: the figure is right and belongs to another day, and
             closing it counts two days as one with a variance nobody can act
             on. The shop's own screen has said "open since" all along, which is
             the wrong end of the shop to find it out from. -->
        <!-- The figure this drawer is counted against, when the till knows it
             is wrong. It cannot happen at any figure a shop reaches, and if it
             ever does, somebody counts a drawer against a number that is
             quietly wrong and a variance nobody can explain is how a shop
             stops believing its till. -->
        {#if view?.drawer_is_behind}
          <p class="why late">{t('till.drawer_is_behind')}</p>
        {/if}
        {#if drawerFromAnotherDay}
          <p class="why late">
            {t('till.drawer_since', {
              when: new Date(drawer.opened_at_ms).toLocaleString('en-GB'),
            })}
          </p>
        {/if}
        <div class="row">
          <!-- Opening it to give change for something bought next door rings no
               sale, so this is its own button rather than a side effect of one.
               Written into the trail either way. -->
          <button onclick={openTheDrawer} disabled={busy}>{t('till.open_drawer')}</button>
        </div>
        <div class="row">
          <input bind:value={movement} placeholder={t('till.amount')} inputmode="decimal" disabled={busy} />
          <input bind:value={reason} placeholder={t('till.why')} disabled={busy} />
          <button onclick={() => moveCash(true)} disabled={busy}>{t('till.in')}</button>
          <button onclick={() => moveCash(false)} disabled={busy}>{t('till.out')}</button>
        </div>
        <div class="row">
          <input bind:value={counted} placeholder={t('till.counted_cash')} inputmode="decimal" disabled={busy} />
          <button onclick={closeShift} disabled={busy}>{t('till.close_drawer')}</button>
          <button onclick={xReport} disabled={busy}>{t('till.totals')}</button>
        </div>
      {/if}

      {#if report}
        <!-- One block for both reports: a Z is an X plus what was counted, and
             two blocks would render the same figures twice and let them drift. -->
        <div class="report">
          <div><span>{report.closed_at_ms ? t('till.z_report') : t('till.totals_so_far')}</span>
               <span>{t('till.sales_count', { count: report.sales })}</span></div>
          <div><span>{t('till.opening_float_line')}</span><span>{money(report.opening_float_minor)}</span></div>
          {#each report.tenders as row (row.name)}
            <div>
              <!-- A wallet is called what the shop calls it; the three kinds
                   every shop has are said in the language on the screen. -->
              <span>
                {row.kind && row.kind !== 'wallet' ? t(`till.${row.kind}`) : row.name}{row.in_drawer
                  ? ''
                  : ` (${t('till.not_in_the_till')})`}
              </span>
              <span>{money(row.amount_minor)}</span>
            </div>
          {/each}
          {#if report.cash_in_minor !== 0}
            <div><span>{t('till.cash_in')}</span><span>{money(report.cash_in_minor)}</span></div>
          {/if}
          {#if report.cash_out_minor !== 0}
            <div><span>{t('till.cash_out')}</span><span>{money(report.cash_out_minor)}</span></div>
          {/if}
          <!-- What came back, beside the figure it is already inside. A drawer
               holding five hundred less than the day felt is the first question
               anybody asks about a cashier, and two customers given their money
               back is the commonest answer to it. This screen said nothing at
               all: the cash was simply lower. Shown only where there were any,
               because a line reading "0 refunds" on every drawer is a line a
               supervisor stops reading, and the evening it matters is the
               evening they have stopped. -->
          {#if report.refunds}
            <div>
              <span>{t('till.refunds_given_back', { count: report.refunds })}</span>
              <span>{money(report.refunded_cash_minor)}</span>
            </div>
          {/if}
          <div class="due"><span>{t('till.should_hold')}</span><span>{money(report.expected_cash_minor)}</span></div>
          {#if report.counted_cash_minor !== undefined && report.counted_cash_minor !== null}
            <div><span>{t('till.counted')}</span><span>{money(report.counted_cash_minor)}</span></div>
            <!-- Negative is short, which is a fact to report rather than an
                 error to refuse: a shift that could not close short would be
                 closed dishonestly. -->
            <div class={report.variance_minor === 0 ? 'change' : 'owed'}>
              <span>{report.variance_minor === 0 ? t('till.exactly_right') : t('till.out_by')}</span>
              <span>{report.variance_minor === 0 ? '' : money(report.variance_minor)}</span>
            </div>
          {/if}
        </div>
        <!-- The slip goes in the drawer with the cash. Before this the figures
             were on the screen and nowhere else, so they were copied by hand. -->
        <button onclick={printDrawer} disabled={busy}>{t('till.print_this')}</button>
      {/if}
    </section>
  {/if}

  {#if receipt}
    <!-- On screen for the cashier, and the only thing on the page when the
         browser prints. -->
    <pre class="receipt">{receipt.map((line) => line.text).join('\n')}</pre>
  {/if}

  <!-- The tax invoice, when somebody asked for one. The same rule as the
       receipt above: on the screen so a cashier can see what will come out, and
       the only thing on the page when the browser prints. -->
  <!-- The credit note, when somebody asked for one. -->
  {#if creditNote}
    <CreditNote
      shop={view?.shop ?? null}
      view={creditNote}
      buyer={creditNote.buyer}
      noteNo={creditNote.receiptNo}
      issuedAt={creditNote.rungAt}
      originalNo={creditNote.original?.no ?? null}
      originalAt={creditNote.original?.at ?? null}
      reason={whyItCameBack}
      {t}
      {money}
      {qty}
    />
  {/if}

  {#if taxInvoice}
    <TaxInvoice
      shop={view?.shop ?? null}
      view={taxInvoice}
      buyer={taxInvoice.buyer}
      receiptNo={taxInvoice.receiptNo}
      rungAt={taxInvoice.rungAt}
      {t}
      {money}
      {qty}
    />
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    font: 16px/1.4 system-ui, sans-serif;
    background: #f6f6f4;
    color: #16150f;
  }
  main { max-width: 46rem; margin: 0 auto; padding: 1rem; }
  /* A gap, because without one the name and the first word of the status ran
     together and the top of the screen read "openposon this device". */
  header {
    display: flex; justify-content: space-between; align-items: baseline;
    gap: 1rem; flex-wrap: wrap; margin-bottom: 0.75rem;
  }
  h1 { font-size: 1.2rem; margin: 0; letter-spacing: 0.02em; }
  /* Quiet on purpose. A cashier reads the basket and the total all day and
     this line perhaps twice: once when something is wrong, and once when
     somebody asks whether the shop has the sales yet. It is here so it can be
     looked at, not so it competes with the money. */
  .state {
    display: flex; gap: 0.9rem; font-size: 0.8rem; color: #6d6a5c;
    flex-wrap: wrap; align-items: baseline; row-gap: 0.25rem;
  }
  .good { color: #1d6b3a; }
  .warn { color: #8a5a00; }
  /* Fixed to the screen, not to the page. A message that renders at the top of
     something nine screenfuls long is a message nobody standing at the bottom
     ever sees, and what they do instead is press the button again. Narrow
     enough to read, wide enough not to hide the thing behind it, and it goes
     when the next action replaces it. */
  .floats {
    position: fixed; top: 0.75rem; left: 50%; transform: translateX(-50%);
    z-index: 30; width: min(40rem, calc(100% - 1.5rem)); margin: 0;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
  }
  /* The till's "done" is a quiet note in the flow everywhere else, so it needs
     the colours the back office's already has when it floats. */
  .why.floats {
    background: #eaf5ec; border: 1px solid #b3d6bd; color: #1d6b3a;
    padding: 0.6rem 0.75rem; border-radius: 6px;
  }
  /* Paper never carries either of them. */
  @media print { .floats { display: none; } }
  .fault {
    background: #fdeceb; border: 1px solid #e6b5b0; color: #8a2018;
    padding: 0.6rem 0.75rem; border-radius: 6px;
  }
  input {
    font: inherit; padding: 0.85rem 0.9rem; width: 100%; box-sizing: border-box;
    border: 1px solid #cfccbf; border-radius: 8px; background: #fff;
    min-height: 3rem;
  }
  /* Where a cashier's cursor lives all day, and where a scanner types. It is
     the one field on the screen that is always the right one to be in, so it
     looks like it. */
  input.scan { font-size: 1.15rem; border-color: #a8a495; }
  input.scan:focus { outline: 3px solid #16150f; outline-offset: 1px; }
  /* Beside the other helper rather than a slab of its own. These two are
     occasional: a cashier scans, and asks what something costs or hunts for it
     without a barcode now and then. Stacked full width they pushed the goods a
     hundred pixels down the screen on every sale, so the thing a cashier
     actually reads sat below two things they rarely touch. */
  button.lookup {
    margin-top: 0.5rem; background: #fff; color: #16150f;
    border-color: #cfccbf;
  }
  .row.lookup { margin-top: 0.5rem; }
  .found { list-style: none; margin: 0.5rem 0 0; padding: 0; display: grid; gap: 0.4rem; }
  .found button {
    width: 100%; display: flex; justify-content: space-between; gap: 1rem;
    background: #fff; color: #16150f; border-color: #cfccbf; text-align: left;
  }
  .empty { color: #8a877a; margin: 0.5rem 0 0; }
  .bangla { display: block; color: #5a574a; font-size: 0.9rem; }
  .heard {
    margin: 0.5rem 0 0; padding: 0.5rem 0.65rem; background: #f3f1e8;
    border-radius: 6px; font-size: 0.9rem;
  }
  .heard .took { font-weight: 600; }
  .heard .set-aside { display: block; color: #8a877a; }
  .count { font-variant-numeric: tabular-nums; margin-right: 0.35rem; }
  /* Loud on purpose, and only on the row it applies to. A till that hedges on
     every row is a till nobody reads the hedging on. */
  .unsure { display: block; color: #8a2018; font-size: 0.85rem; }
  button.quiet { background: #fff; color: #16150f; border-color: #cfccbf; }
  button.abandon {
    background: #fff; color: #8a2018; border-color: #c9a49f; margin-top: 0.75rem;
  }
  /* The answer to a question about a shelf, not a line in the basket: it sits
     apart from the ticket so nobody reads it as something already rung. */
  /* The camera, while it is reading. Sized so the label is big enough to read
     and the basket behind it is still on the screen: a cashier who cannot see
     what they have rung has lost the thing they are checking against. */
  .camera {
    background: #fff; border: 1px solid #cfccbf; border-radius: 6px;
    padding: 0.6rem; margin: 0.5rem 0; display: grid; gap: 0.4rem;
  }
  .camera video {
    width: 100%; max-height: 40vh; border-radius: 6px; background: #16150f;
  }
  .checked {
    margin: 0.75rem 0; padding: 0.75rem 0.9rem; background: #eef2e8;
    border: 1px solid #c6cfba; border-radius: 6px;
  }
  .checked .name { margin: 0; font-weight: 600; font-size: 1.1rem; }
  .checked .each { margin: 0.2rem 0 0.6rem; color: #3f4a35; font-size: 1.25rem; }
  .checked button { padding: 0.5rem 0.8rem; }
  .parked { margin: 1rem 0; padding: 0.6rem 0.75rem; background: #f3f1e8; border-radius: 6px; }
  .parked .why { margin: 0 0 0.5rem; font-size: 0.85rem; color: #5a574a; }
  .parked ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
  .parked li { display: flex; gap: 0.6rem; align-items: center; }
  .parked .name { font-weight: 600; }
  .parked .each { color: #5a574a; font-size: 0.9rem; margin-right: auto; }
  .parked button { padding: 0.4rem 0.7rem; font-size: 0.9rem; }
  .parked .drop { background: #fff; color: #8a2018; border-color: #c9a49f; }
  select {
    font: inherit; padding: 0.6rem 0.7rem; border: 1px solid #cfccbf;
    border-radius: 6px; background: #fff;
  }
  .lines { list-style: none; margin: 1rem 0; padding: 0; }
  .pick {
    display: contents; font: inherit; color: inherit; background: none;
    border: 0; padding: 0; text-align: left; cursor: pointer;
  }
  .lines li.picked { background: #f3f1e8; }
  /* Faint, because it is the only thing on the row that is not a fact about
     the sale. It is there to say the row can be touched, not to be read. */
  .more { color: #a5a294; font-size: 1.1rem; line-height: 1; align-self: center; }
  .lines li.picked .more { color: #16150f; }
  .sum { text-align: right; }
  .unknown { display: grid; gap: 0.5rem; padding: 0.5rem 0; }

  .shelf {
    display: block;
    padding: 0 0.9rem 0.5rem;
    font-size: 0.85rem;
    color: #8a4b00;
  }

  .gave { grid-column: 1 / -1; font-size: 0.85rem; color: #7a5a1e; }
  .edit { grid-column: 1 / -1; display: flex; gap: 0.4rem; align-items: center; padding: 0.4rem 0; }
  .edit button {
    font: inherit; min-width: 2.6rem; padding: 0.5rem 0.6rem; border-radius: 6px;
    border: 1px solid #cfccbf; background: #fff; color: #16150f; cursor: pointer;
  }
  .edit .count {
    width: 4.5rem; text-align: center; font-variant-numeric: tabular-nums;
    padding: 0.5rem 0.4rem;
  }
  .edit .off { width: 6rem; padding: 0.5rem 0.6rem; }
  .edit .drop { margin-left: auto; border-color: #c9a49f; color: #8a2018; }
  /* Room to read and room to hit. A basket line is the thing a cashier checks
     against what is in front of them, and it was set at the same size and
     spacing as the housekeeping below it. */
  .lines li {
    display: grid; grid-template-columns: 1fr auto auto auto auto; gap: 1rem;
    padding: 0.7rem 0; border-bottom: 1px solid #e6e3d8;
    font-size: 1.05rem;
  }
  .lines .empty { color: #8a877a; border: 0; }
  .qty, .each, .sum { font-variant-numeric: tabular-nums; }
  .totals { display: grid; gap: 0.25rem; margin: 1rem 0; }
  .totals div { display: flex; justify-content: space-between; font-variant-numeric: tabular-nums; }
  /* The number a cashier says out loud, and the one a customer leans over the
     counter to read. It was the same size as the word beside it. */
  .due {
    font-weight: 700; font-size: 2rem; line-height: 1.2;
    padding-top: 0.5rem; margin-top: 0.25rem; border-top: 2px solid #16150f;
  }
  .owed { color: #8a2018; font-weight: 600; font-size: 1.15rem; }
  /* What to hand back. Wrong change is the mistake a customer notices at the
     counter and a shop finds at the evening count, so it is as large as the
     total. */
  .change { color: #1d6b3a; font-weight: 700; font-size: 1.6rem; }
  .actions { display: grid; gap: 0.6rem; }
  /* Wraps, for the reason the back office's does: a shop reads this on
     whatever it has, and a row that cannot wrap puts a button off the edge of
     a narrow screen with nothing to say it is there. */
  .row { display: flex; gap: 0.6rem; flex-wrap: wrap; }
  /* A box in one of those rows shares the line with the button beside it. They
     are `width: 100%` everywhere else, which is right when they are alone and
     wrong here: once the row could wrap, a full-width box pushed its own button
     onto the next line at every width, not just narrow ones. Found by looking
     at the screen after fixing the narrow case, which is the only way it would
     have been found. */
  .row input, .row select { flex: 1 1 8rem; width: auto; min-width: 6rem; }
  .enrol { margin-bottom: 0.75rem; }
  .signin { margin-bottom: 0.75rem; }
  /* Housekeeping, set apart from the sale above it. A cashier rings baskets all
     day and counts a drawer twice, and the two were stacked at the same weight
     so the selling screen ran on past the money into the float and the cash
     movements. This does not hide anything: it says which part of the screen
     is which. */
  .drawer {
    margin: 1.5rem 0 0.75rem; display: grid; gap: 0.5rem;
    padding: 0.9rem 1rem; background: #efeee8; border: 1px solid #dedbd0;
    border-radius: 10px;
  }
  .drawerline {
    display: flex; justify-content: space-between; align-items: baseline;
    font-size: 1rem; font-weight: 600;
  }
  .drawerline strong { font-size: 1.2rem; font-variant-numeric: tabular-nums; }
  .drawer p { margin: 0; font-size: 0.9rem; }
  .report {
    display: grid; gap: 0.2rem; padding: 0.6rem 0.75rem;
    background: #fff; border: 1px solid #cfccbf; border-radius: 6px; font-size: 0.9rem;
  }
  .report div { display: flex; justify-content: space-between; font-variant-numeric: tabular-nums; }
  .signin p { margin: 0 0 0.5rem; }
  .who { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .link {
    border: 0; background: none; padding: 0; font: inherit; font-size: 0.85rem;
    color: #5a574a; text-decoration: underline; cursor: pointer;
  }
  /* Sized for a thumb on a cheap tablet, not a mouse on a desk. Three
     millimetres of extra padding is the difference between a cashier hitting
     "Take cash" and hitting "Take it" with a queue watching. */
  button {
    font: inherit; padding: 0.85rem 1.1rem; border-radius: 8px; cursor: pointer;
    border: 1px solid #cfccbf; background: #fff; white-space: nowrap;
    min-height: 3rem;
  }
  /* Except the ones that are deliberately small: a link, and the stepper
     beside a quantity. */
  button.link { min-height: 0; }
  button:disabled { opacity: 0.45; cursor: not-allowed; }
  .finish { background: #16150f; color: #fff; border-color: #16150f; font-weight: 600; }
  .receipt {
    margin: 1.25rem 0 0;
    padding: 0.75rem;
    background: #fff;
    border: 1px solid #cfccbf;
    font: 13px/1.35 ui-monospace, "SF Mono", Menlo, monospace;
    white-space: pre;
    overflow-x: auto;
  }
  /* Paper gets the receipt and nothing else: a shopkeeper printing a sale does
     not want the scan field and the buttons on the roll. */
  @media print {
    :global(body) { background: #fff; }
    /* Whichever of the two documents is on the screen, and only that one. The
       receipt is a 58mm roll and the tax invoice is A4, so they are never
       printed together: asking for one clears the other. */
    main > *:not(.receipt):not(.mushak) { display: none; }
    .receipt { border: 0; padding: 0; margin: 0; font-size: 12px; }
  }

  /* The Mushak 6.3, which is a sheet of paper rather than a screen. Laid out
     here because it is printed from this app; the markup and the field order
     are in tax_invoice.svelte, where the form is quoted. */
  :global(.mushak) {
    background: #fff;
    border: 1px solid #cfccbf;
    border-radius: 6px;
    padding: 1.5rem;
    margin-top: 1rem;
    font-size: 13px;
  }
  :global(.mushak header) { display: block; text-align: center; margin-bottom: 1rem; }
  :global(.mushak header p) { margin: 0.1rem 0; }
  :global(.mushak h1) { font-size: 1.1rem; margin: 0.5rem 0 0.2rem; }
  :global(.mushak .form) {
    float: right; border: 1px solid #16150f; padding: 0.2rem 0.6rem;
  }
  :global(.mushak .rule) { font-size: 0.85em; }
  :global(.mushak dl) { display: grid; grid-template-columns: auto 1fr; gap: 0.2rem 0.6rem; margin: 0; }
  :global(.mushak dd) { margin: 0; border-bottom: 1px dotted #cfccbf; min-height: 1.2em; }
  :global(.mushak .parties) { display: flex; gap: 2rem; margin: 1rem 0; }
  :global(.mushak .parties dl) { flex: 1; }
  :global(.mushak table) { width: 100%; border-collapse: collapse; margin-top: 0.5rem; }
  :global(.mushak th), :global(.mushak td) {
    border: 1px solid #16150f; padding: 0.3rem; font-size: 0.8em; text-align: left;
    vertical-align: top;
  }
  /* Four empty rows under the goods, because the form has them and because a
     shop writing a line in by hand is a shop using the form as intended. */
  :global(.mushak tbody tr:not(.sum)) { height: 1.6rem; }
  :global(.mushak .sum td) { font-weight: 600; }
  :global(.mushak .footnote) { font-size: 0.75em; margin: 0.3rem 0 1.2rem; }
  /* The Mushak 6.7's own two shapes: the block of totals down the right, and
     the box for why the goods came back. Everything else it shares with the
     invoice above, because the two forms are laid out the same way. */
  :global(.mushak .sums) {
    grid-template-columns: 1fr auto;
    max-width: 26rem;
    margin: 0.6rem 0 0 auto;
  }
  :global(.mushak .sums dt) { text-align: right; }
  :global(.mushak .sums dd) { min-width: 8rem; border-bottom: 1px solid #16150f; }
  :global(.mushak .party) { font-weight: 600; grid-column: 1 / -1; }
  :global(.mushak .reason) { margin-top: 1rem; }
  :global(.mushak .reason .label) { margin: 0 0 0.2rem; }
  :global(.mushak .reason .box) {
    border: 1px solid #16150f; min-height: 3.5rem; margin: 0; padding: 0.4rem;
  }
  :global(.mushak .signed) { max-width: 22rem; margin-top: 1.5rem; }
  @media print {
    :global(.mushak) { border: 0; padding: 0; margin: 0; font-size: 11px; }
    @page { size: A4; margin: 12mm; }
  }
</style>
