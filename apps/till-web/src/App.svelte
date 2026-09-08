<script>
  import { onMount } from 'svelte';
  import {
    open,
    run,
    connect,
    enrol,
    keepSyncing,
    describeSync,
    adoptToken,
    sync,
  } from './till.js';
  import { money, qty } from './format.js';
  // What this screen says, in the language the shop reads. The refusals come
  // from the core keyed on a code, because matching on an English sentence to
  // translate it goes quiet the day somebody improves the wording.
  import { LANGUAGES, paperWords, refusal, say } from '../../shared/words.js';
  import { keepACopy } from '../../shared/keep_a_copy.js';
  import { today } from '../../shared/days.js';
  // Telling two people with the same name apart, shared with the back office so
  // the mark on a person is the same in both places.
  import { label, shared } from '../../shared/people.js';
  import { milliFrom } from '../../shared/quantity.js';
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
  let language = $state(localStorage.getItem(LANGUAGE) ?? 'en');
  const t = $derived((key, fill) => say(language, key, fill));
  function speak(next) {
    language = next;
    localStorage.setItem(LANGUAGE, next);
  }

  let view = $state(null);
  let storage = $state('opening');
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
  let syncing = $state('idle');
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
  let refundAgainst = $state('');
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
  let float_ = $state('');
  let movement = $state('');
  let reason = $state('');
  let counted = $state('');
  const report = $derived(view?.report ?? null);

  // How much this cashier may give away. Zero for most of them, and the screen
  // hides what it would only refuse.
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
  // What still has to change hands. Positive means the customer owes the shop,
  // negative means the shop owes the customer, and it is the same subtraction
  // either way: a refund is a sale with the signs turned round.
  const outstanding = $derived(total - (view?.tendered_minor ?? 0));
  // A sale may be overpaid and the difference is change. A refund may not: any
  // difference is money leaving the shop unaccounted for.
  const settled = $derived(
    view !== null && total !== 0 && (refunding ? outstanding === 0 : outstanding <= 0),
  );

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
      return `${rate}% off this line, ${off} in all`;
    }
    if (line.discount_amount_minor) {
      const own = money(-line.discount_amount_minor);
      // "In all" when a discount off the whole basket has been shared out on
      // top of it, the same way the rate above reads.
      return line.discount_amount_minor === line.discount_minor
        ? `${own} off this line`
        : `${own} off this line, ${off} in all`;
    }
    return `${off}, this line's share of the ticket discount`;
  }

  async function changeQty(at, milli) {
    if (milli <= 0) {
      // Down to nothing is off the ticket. Sending a zero quantity would leave
      // a line reading "0 x Rice" that nobody can sell or clear.
      await drop(at);
      return;
    }
    await attemptWithOverride(() => run({ op: 'set_qty', line: at, qty_milli: milli }));
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
    const taka = Number(typed);
    if (!Number.isFinite(taka) || taka < 0) {
      fault = t('till.not_a_price');
      return;
    }
    await attemptWithOverride(() =>
      run({ op: 'set_unit_price', line: at, price_minor: Math.round(taka * 100) }),
    );
  }

  async function discountLine(at, typed) {
    const percent = Number(typed === '' ? 0 : typed);
    if (!Number.isFinite(percent)) {
      fault = t('till.not_a_percentage');
      return;
    }
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
    const percent = Number(ticketOff === '' ? 0 : ticketOff);
    if (!Number.isFinite(percent)) {
      fault = t('till.not_a_percentage');
      return;
    }
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
  async function attemptWithOverride(work) {
    blocked = null;
    const reply = await attempt(work);
    if (reply?.view?.needs_supervisor) {
      blocked = { work, action: reply.view.needs_supervisor };
    }
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
    await attempt(again.work);
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
      fault = refusal(language, view);
      return reply;
    } catch (error) {
      // A worker that failed outright, which is different from a till that
      // refused: the basket on screen may no longer be what the till holds.
      //
      // Worded the same way all the same. A refusal from the shop's own server
      // travels this path, and it carries a name and its figures beside the
      // English: this is the point where the language is known.
      fault = refusal(language, {
        error: error.message,
        error_code: error.code,
        error_parts: error.parts,
      });
      return null;
    } finally {
      busy = false;
    }
  }

  onMount(async () => {
    // Before anything else, because this is what lets the app be opened at all
    // during an outage. Everything below it is offline machinery that a tablet
    // switched on with the internet down could not reach: the browser would be
    // fetching the page and the wasm from a server that is not answering.
    keepACopy(
      () => ({
        lines: view?.lines?.length ?? 0,
        tendered: (view?.tendered_minor ?? 0) !== 0,
        counting: false,
        unsent: view?.unsynced_sales ?? 0,
      }),
      (waiting) => {
        newBuildWaiting = waiting;
      },
    );
    await connect(SERVER);
    const known = JSON.parse(localStorage.getItem(IDENTITY) ?? 'null');
    if (known) {
      const reply = await attempt(() => open(known.tenant, known.terminal));
      storage = reply?.info?.storage ?? 'unavailable';
      keeping = reply?.info?.keeping ?? 'unknown';
      enrolled = Boolean(reply?.view?.enrolled);
    } else {
      // Nothing has told this device who it is yet, so there is no ledger to
      // open: a till opened as a guess would present a credential for one
      // terminal and a request body for another.
      storage = 'not enrolled';
    }

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
      syncing = round.ok
        ? t(said.key, said.fill)
        : t('sync.held_up', { why: round.error });
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
      fault = refusal(language, {
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
  async function tryNow() {
    await attempt(() => run({ op: 'try_now' }));
  }

  async function openTheDrawer() {
    await attempt(() => run({ op: 'open_drawer', now_ms: Date.now() }));
  }

  async function openShift() {
    const taka = Number(float_);
    if (!Number.isFinite(taka) || taka < 0) {
      fault = t('till.count_the_float');
      return;
    }
    float_ = '';
    await attempt(() =>
      run({
        op: 'open_shift',
        shift_id: newId(),
        opening_float_minor: Math.round(taka * 100),
        at_ms: Date.now(),
      }),
    );
  }

  async function moveCash(inward) {
    const taka = Number(movement);
    if (!Number.isFinite(taka) || taka <= 0) {
      fault = t('till.an_amount_in_taka');
      return;
    }
    if (!reason.trim()) {
      // The core refuses this too. Saying so here saves a round trip and says
      // it in the words the cashier is looking at.
      fault = t('till.say_why_cash_moved');
      return;
    }
    const amount = Math.round(taka * 100);
    const why = reason.trim();
    movement = '';
    reason = '';
    await attempt(() =>
      run({ op: 'move_cash', inward, amount_minor: amount, reason: why, at_ms: Date.now() }),
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
        words: paperWords(language),
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
    const taka = Number(counted);
    if (!Number.isFinite(taka) || taka < 0) {
      fault = t('till.count_the_drawer');
      return;
    }
    counted = '';
    // The report comes back from the core, variance and all. Working it out
    // here would be a second arithmetic that can disagree with the first.
    await attempt(() =>
      run({ op: 'close_shift', counted_cash_minor: Math.round(taka * 100), at_ms: Date.now() }),
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

  async function startRefund() {
    askingReceipt = false;
    const against = refundAgainst.trim();
    refundAgainst = '';
    // Refused unless this person may, or a supervisor has allowed it. The
    // refusal is the core's own words, which name what is missing.
    //
    // Without a number when they have lost the paper, which happens and is not
    // a reason to refuse somebody their money at the counter: the shop takes it
    // back and the sale says nobody named the receipt.
    await attemptWithOverride(() =>
      run({
        op: 'start_refund',
        original_receipt: against === '' ? null : against,
        now_ms: Date.now(),
      }),
    );
    scanner?.focus();
  }

  async function look() {
    const asked = hunt.trim();
    if (!asked) {
      found = [];
      return;
    }
    const reply = await attempt(() => run({ op: 'catalogue', query: asked, limit: 12 }));
    found = reply?.view?.catalogue ?? [];
  }

  /// Answer what one costs, without touching the basket.
  async function check() {
    const code = barcode.trim();
    if (!code) return;
    barcode = '';
    await attempt(() => run({ op: 'check', code }));
    scanner?.focus();
  }

  /// The customer said yes. Ring what was just checked and go back to scanning.
  async function ringChecked(item) {
    checking = false;
    await ring(item);
  }

  async function ring(item) {
    await attemptWithOverride(() => run({ op: 'add', item_id: item.id, qty_milli: 1000 }));
    // Back to the scanner: the next thing a cashier does is almost always scan
    // the next item, and a screen left in a search box makes them hunt for it.
    hunt = '';
    found = [];
    lookingUp = false;
    scanner?.focus();
  }

  // The screen shows the first wallet whatever the binding holds, so the
  // binding is made to agree with the screen rather than the other way round.
  $effect(() => {
    if (wallets.length > 0 && !wallets.includes(walletName)) walletName = wallets[0];
  });

  async function takeTender() {
    const amount = Number(cash);
    if (!Number.isFinite(amount) || amount <= 0) {
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
        amount_minor: Math.round(amount * 100) * owed,
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
    await attempt(() => run({ op: 'cancel_sale' }));
    editing = null;
    scanner?.focus();
  }

  async function clearTenders() {
    await attempt(() => run({ op: 'clear_tenders' }));
    cash = '';
    scanner?.focus();
  }

  async function park() {
    const label = parkAs.trim() || 'no name';
    parkAs = '';
    // The id is minted here, as a sale's is. A ULID would come from the
    // platform layer in the finished product; this is the same placeholder.
    const id = crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
    await attempt(() => run({ op: 'hold', ticket_id: id, held_at_ms: Date.now(), label }));
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
    return `the shop has ${qty(short.on_hand_milli)}, this wants ${qty(short.wanted_milli)}`;
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

  async function scan() {
    const code = barcode.trim();
    if (!code) return;
    barcode = '';
    const reply = await attemptWithOverride(() => run({ op: 'scan', barcode: code, qty_milli: 1000 }));
    // A barcode in nobody's catalogue, which during an outage is a delivery
    // that arrived this morning. The cashier can write it down here rather than
    // lose the sale, which is the whole of the cold-start promise.
    if (reply?.view?.error?.includes('no item in the catalogue')) {
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
    const rate = Number(newVat);
    if (!Number.isFinite(rate) || rate < 0 || rate > 100) {
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
        vat_bp: Math.round(rate * 100),
      }),
    );
    if (!reply || reply.view?.error) return;
    unknown = null;
    // Straight onto the ticket: the customer is standing there, which is why
    // any of this exists.
    await attempt(() => run({ op: 'scan', barcode: code, qty_milli: 1000 }));
    scanner?.focus();
  }

  async function tender() {
    const amount = Number(cash);
    if (!Number.isFinite(amount) || amount <= 0) {
      fault = t('till.an_amount_in_taka');
      return;
    }
    cash = '';
    await attempt(() =>
      run({ op: 'add_cash', amount_minor: Math.round(amount * 100), at_ms: Date.now() }),
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
  async function printReceipt(rungAtMs = Date.now()) {
    // The width is the paper's, not the screen's. 32 characters is a 58mm roll,
    // which is what a small shop has.
    const reply = await attempt(() =>
      run({
        op: 'receipt',
        width: 32,
        rung_at: new Date(rungAtMs).toLocaleString('en-GB'),
        // The paper in the language the screen is in. The core holds no
        // translations and defaults to English, which is what the thermal path
        // gets: no ESC/POS code page carries Bangla.
        words: paperWords(language),
      }),
    );
    receipt = reply?.view?.receipt ?? null;
    if (receipt) {
      // Left to the browser's own dialog rather than driven from here: a
      // printer, a PDF and a preview are all the same button to a shopkeeper.
      await new Promise((settle) => setTimeout(settle, 50));
      window.print();
    }
  }

  async function checkout() {
    // The id and the clock come from here, because the core mints neither. A
    // ULID would be minted by the platform layer in the finished product; this
    // is a placeholder and is marked as one in todo.md.
    const id = crypto.randomUUID().replace(/-/g, '').toUpperCase().slice(0, 26);
    // Read once. The same number goes into the ledger and onto the paper, or
    // they are two answers to when this sale happened.
    const rungAtMs = Date.now();
    const reply = await attempt(() => run({ op: 'checkout', ticket_id: id, rung_at_ms: rungAtMs }));
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
        <span class="warn">{storage}</span>
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
      <span>{t('till.numbers_left', { count: view?.receipt_numbers_left ?? 0 })}</span>
      <span class={syncing.startsWith('held up') ? 'warn' : ''}>{syncing}</span>
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
      <button
        class="link"
        onclick={() => speak(language === 'bn' ? 'en' : 'bn')}
        title={t('till.language')}
      >
        {LANGUAGES.find((one) => one.code !== language)?.name}
      </button>
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

  {#if !enrolled || refused}
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

  {#if fault}
    <p class="fault" role="alert">{fault}</p>
  {/if}
  {#if done}
    <p class="why" role="status">{done}</p>
  {/if}

  <input
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
        onclick={() => { checking = !checking; scanner?.focus(); }}
        disabled={busy}
      >
        {checking ? t('till.back_to_scanning') : t('till.what_does_this_cost')}
      </button>
      {#if checking}
        <span class="why">{t('till.nothing_here_goes_in')}</span>
      {/if}
    </div>
  {/if}

  {#if checking && view?.checked}
    <section class="checked">
      <p class="name">
        {view.checked.item.name}
        {#if view.checked.item.name_bn && view.checked.item.name_bn !== view.checked.item.name}
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
        <button onclick={() => { lookingUp = false; hunt = ''; found = []; scanner?.focus(); }}>
          Back to scanning
        </button>
      </div>
      {#if found.length > 0}
        <ul class="found">
          {#each found as item (item.id)}
            <li>
              <button onclick={() => ring(item)} disabled={busy}>
                <span class="name">
                  {item.name}
                  {#if item.name_bn && item.name_bn !== item.name}
                    <!-- A screen renders Bangla; thermal paper is the thing that
                         cannot, and the receipt says so line by line. -->
                    <span class="bangla">{item.name_bn}</span>
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
            Leave it
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
        <button class="pick" onclick={() => (editing = editing === at ? null : at)} disabled={busy}>
          <span class="name">{line.name}</span>
          <span class="qty">{qty(line.qty_milli)}</span>
          <span class="each">{money(line.unit_price_minor)}</span>
          <span class="sum">{money(line.total_minor)}</span>
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
          <!-- Under the line it changes, not in a dialog over it: a cashier
               correcting the third of five things is looking at the third. -->
          <div class="edit">
            <button onclick={() => changeQty(at, line.qty_milli - 1000)} disabled={busy}>&minus;</button>
            <!-- Typed as well as stepped, because a shop sells rice by the kilo
                 and a kilo and a half is two presses of nothing. -->
            <input
              class="count"
              value={qty(line.qty_milli)}
              onchange={(e) => typeQty(at, e.currentTarget.value)}
              inputmode="decimal"
              aria-label={t('till.how_many')}
              disabled={busy}
            />
            <button onclick={() => changeQty(at, line.qty_milli + 1000)} disabled={busy}>+</button>
            {#if ceiling > 0}
              <input
                class="off"
                value={line.discount_bp ? line.discount_bp / 100 : ''}
                onchange={(e) => discountLine(at, e.currentTarget.value)}
                placeholder={t('till.percent_off')}
                inputmode="decimal"
                disabled={busy}
              />
              <!-- The same thing said the way a shop says it. Both are offered
                   because both are said: "ten percent" over a counter and
                   "twenty taka off" across it. -->
              <input
                class="off"
                onchange={(e) => takeOffLine(at, e.currentTarget.value)}
                placeholder={t('till.amount_off')}
                inputmode="decimal"
                disabled={busy}
              />
            {/if}
            {#if mayOverride}
              <!-- Damaged goods, a short weight, a price somebody was quoted.
                   Shown only to whoever may do it: a button that refuses is a
                   button that teaches people to press it and be refused. -->
              <input
                class="off"
                value={(line.unit_price_minor / 100).toFixed(2)}
                onchange={(e) => priceLine(at, e.currentTarget.value)}
                inputmode="decimal"
                disabled={busy}
              />
            {/if}
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
    <div>
      <span>{refunding ? t('till.given_back') : t('till.paid')}</span>
      <span>{money(view?.tendered_minor ?? 0)}</span>
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
    {#if ceiling > 0 && (view?.lines?.length ?? 0) > 0 && !refunding}
      <!-- The ceiling is shown rather than discovered. A cashier who may give
           five percent should not learn that by being refused ten. -->
      <div class="row">
        <input
          bind:value={ticketOff}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); discountTicket(); } }}
          placeholder={t('till.percent_off_ticket', { ceiling: ceiling / 100 })}
          inputmode="decimal"
          disabled={busy}
        />
        <button onclick={discountTicket} disabled={busy}>{t('till.discount')}</button>
      </div>
      <div class="row">
        <input
          bind:value={ticketOffAmount}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); takeOffTicket(); } }}
          placeholder={t('till.amount_off_ticket')}
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
        onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); tender(); } }}
        placeholder={payingBy === 'cash' ? t('till.cash_taken') : t('till.how_much_taken')}
        inputmode="decimal"
        disabled={busy}
      />
      <button onclick={tender} disabled={busy}>{t('till.take_cash')}</button>
    </div>
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
        {#if payingBy === 'credit'}
          <!-- Somebody the shop wrote down, when it has. What they owe is then
               added up against a person rather than against the spelling a
               cashier used that day, which is how two Karims share an account.
               A shop that has written nobody down still types a name. -->
          {#if customers.length > 0}
            <select value={view?.customer ?? ''} onchange={(e) => chooseCustomer(e.currentTarget.value)} disabled={busy}>
              <option value="">{t('till.somebody_not_on_the_list')}</option>
              {#each customers as one (one.id)}
                <option value={one.id}>
                  {label(one, customersTwiceOver)}{one.owed_minor
                    ? ` — ${t('till.owes_short', { amount: money(one.owed_minor) })}`
                    : ''}{one.limit_minor
                    ? ` of ${money(one.limit_minor)}`
                    : ''}
                </option>
              {/each}
            </select>
          {/if}
          {#if !view?.customer}
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
                Write {reference.trim()} down
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
      {refunding ? `Refund ${money(-outstanding)}` : t('till.exact', { amount: money(outstanding) })}
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
    <button class="finish" onclick={checkout} disabled={busy || !settled}>{t('till.finish_sale')}</button>
    {#if operator && (view?.lines?.length ?? 0) > 0}
      <!-- Last, and set apart: it throws away the whole basket. Removing five
           lines one at a time is five chances to leave one behind, and the one
           left behind is rung to the next customer. -->
      <button class="abandon" onclick={cancelSale} disabled={busy}>{t('till.give_up_on_this_sale')}</button>
    {/if}
    {#if receipt}
      <button onclick={() => window.print()}>{t('till.print_again')}</button>
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
</main>

<style>
  :global(body) {
    margin: 0;
    font: 16px/1.4 system-ui, sans-serif;
    background: #f6f6f4;
    color: #16150f;
  }
  main { max-width: 46rem; margin: 0 auto; padding: 1rem; }
  header { display: flex; justify-content: space-between; align-items: baseline; }
  h1 { font-size: 1.2rem; margin: 0; letter-spacing: 0.02em; }
  .state { display: flex; gap: 0.75rem; font-size: 0.85rem; color: #5a574a; }
  .good { color: #1d6b3a; }
  .warn { color: #8a5a00; }
  .fault {
    background: #fdeceb; border: 1px solid #e6b5b0; color: #8a2018;
    padding: 0.6rem 0.75rem; border-radius: 6px;
  }
  input {
    font: inherit; padding: 0.7rem 0.8rem; width: 100%; box-sizing: border-box;
    border: 1px solid #cfccbf; border-radius: 6px; background: #fff;
  }
  button.lookup {
    width: 100%; margin-top: 0.5rem; background: #fff; color: #16150f;
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
  button.quiet { background: #fff; color: #16150f; border-color: #cfccbf; }
  button.abandon {
    background: #fff; color: #8a2018; border-color: #c9a49f; margin-top: 0.75rem;
  }
  /* The answer to a question about a shelf, not a line in the basket: it sits
     apart from the ticket so nobody reads it as something already rung. */
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
  .lines li {
    display: grid; grid-template-columns: 1fr auto auto auto; gap: 1rem;
    padding: 0.5rem 0; border-bottom: 1px solid #e6e3d8;
  }
  .lines .empty { color: #8a877a; border: 0; }
  .qty, .each, .sum { font-variant-numeric: tabular-nums; }
  .totals { display: grid; gap: 0.25rem; margin: 1rem 0; }
  .totals div { display: flex; justify-content: space-between; font-variant-numeric: tabular-nums; }
  .due { font-weight: 700; font-size: 1.25rem; padding-top: 0.35rem; border-top: 2px solid #16150f; }
  .owed { color: #8a2018; font-weight: 600; }
  .change { color: #1d6b3a; font-weight: 700; font-size: 1.15rem; }
  .actions { display: grid; gap: 0.6rem; }
  .row { display: flex; gap: 0.6rem; }
  .enrol { margin-bottom: 0.75rem; }
  .signin { margin-bottom: 0.75rem; }
  .drawer { margin-bottom: 0.75rem; display: grid; gap: 0.5rem; }
  .drawerline { display: flex; justify-content: space-between; font-size: 0.9rem; }
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
  button {
    font: inherit; padding: 0.7rem 1rem; border-radius: 6px; cursor: pointer;
    border: 1px solid #cfccbf; background: #fff; white-space: nowrap;
  }
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
    main > *:not(.receipt) { display: none; }
    .receipt { border: 0; padding: 0; margin: 0; font-size: 12px; }
  }
</style>
