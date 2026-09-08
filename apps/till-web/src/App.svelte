<script>
  import { onMount } from 'svelte';
  import { open, run, connect, enrol, keepSyncing, describeSync, adoptToken } from './till.js';
  import { money, qty } from './format.js';
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

  let view = $state(null);
  let storage = $state('opening');
  // Whether the browser promised to keep what this device holds. Without a
  // grant everything in the store is evictable, which is unsent sales and the
  // receipt numbers this terminal was given.
  let keeping = $state('unknown');
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
  // The last sale, laid out for paper. Held until the next sale replaces it, so
  // a cashier can reprint without hunting for anything.
  let receipt = $state(null);
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
      fault = 'that is not a quantity: digits, and up to three after a point';
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
      fault = 'a price in taka, and not a negative one';
      return;
    }
    await attemptWithOverride(() =>
      run({ op: 'set_unit_price', line: at, price_minor: Math.round(taka * 100) }),
    );
  }

  async function discountLine(at, typed) {
    const percent = Number(typed === '' ? 0 : typed);
    if (!Number.isFinite(percent)) {
      fault = 'a discount is a percentage';
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
      fault = 'an amount off is taka and poisha, and not a negative one';
      return;
    }
    await attemptWithOverride(() => run({ op: 'take_off_line', line: at, amount_minor: off }));
  }

  async function takeOffTicket() {
    const off = minorFrom(ticketOffAmount);
    if (off === null) {
      fault = 'an amount off is taka and poisha, and not a negative one';
      return;
    }
    await attemptWithOverride(() => run({ op: 'take_off_ticket', amount_minor: off }));
    ticketOffAmount = '';
    scanner?.focus();
  }

  async function discountTicket() {
    const percent = Number(ticketOff === '' ? 0 : ticketOff);
    if (!Number.isFinite(percent)) {
      fault = 'a discount is a percentage';
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
      // A refusal the core reported is shown as it was worded. Rewriting it
      // here would mean two places describe the same failure, and the one on
      // screen would be the one nobody tested.
      fault = view.error ?? null;
      return reply;
    } catch (error) {
      // A worker that failed outright, which is different from a till that
      // refused: the basket on screen may no longer be what the till holds.
      fault = error.message;
      return null;
    } finally {
      busy = false;
    }
  }

  onMount(async () => {
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
      syncing = round.ok ? describeSync(round.info) : `held up: ${round.error}`;
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
      fault = error.message;
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
    const day = new Date().toISOString().slice(0, 10);
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
    done = `Saved as ${name}. Do not wipe this device until the back office has taken them in.`;
  }

  /// Or straight to the clipboard, for the case where both are one device.
  async function copyCarried() {
    if (!carrying) return;
    try {
      await navigator.clipboard.writeText(carrying.bundle);
      done = 'Copied. Paste it into the back office, under "Sales carried in by hand".';
    } catch {
      // No clipboard permission, or an insecure origin. The text is on the
      // screen either way, which is why it is still shown.
      fault = 'this browser would not let me copy: select the text below instead';
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

  async function openShift() {
    const taka = Number(float_);
    if (!Number.isFinite(taka) || taka < 0) {
      fault = 'count the float and enter it in taka';
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
      fault = 'enter an amount in taka';
      return;
    }
    if (!reason.trim()) {
      // The core refuses this too. Saying so here saves a round trip and says
      // it in the words the cashier is looking at.
      fault = 'say why the cash moved: an unexplained movement reads as theft later';
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
      fault = 'count the drawer and enter what is in it';
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

  async function startRefund() {
    // Refused unless this person may, or a supervisor has allowed it. The
    // refusal is the core's own words, which name what is missing.
    await attemptWithOverride(() => run({ op: 'start_refund', now_ms: Date.now() }));
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

  async function ring(item) {
    await attemptWithOverride(() => run({ op: 'add', item_id: item.id, qty_milli: 1000 }));
    // Back to the scanner: the next thing a cashier does is almost always scan
    // the next item, and a screen left in a search box makes them hunt for it.
    hunt = '';
    found = [];
    lookingUp = false;
    scanner?.focus();
  }

  async function takeTender() {
    const amount = Number(cash);
    if (!Number.isFinite(amount) || amount <= 0) {
      fault = 'an amount in taka';
      return;
    }
    // A debt owed by nobody is money given away. This is the only record of it
    // anybody gets, on the customer's copy and on the shop's.
    if (payingBy === 'credit' && !view?.customer && !reference.trim()) {
      fault = 'say who owes it: a sale on account with no name cannot be chased';
      return;
    }
    cash = '';
    const owed = refunding ? -1 : 1;
    const reply = await attempt(() =>
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
      fault = 'a price is taka and poisha';
      return;
    }
    const rate = Number(newVat);
    if (!Number.isFinite(rate) || rate < 0 || rate > 100) {
      fault = 'a tax rate is between nothing and a hundred percent';
      return;
    }
    if (!newName.trim()) {
      fault = 'an item needs a name, or its line on the receipt says nothing';
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
      fault = 'enter an amount in taka';
      return;
    }
    cash = '';
    await attempt(() => run({ op: 'add_cash', amount_minor: Math.round(amount * 100) }));
    scanner?.focus();
  }

  async function exact() {
    if (outstanding === 0) return;
    // Negative on a refund, which is money going back across the counter.
    await attempt(() => run({ op: 'add_cash', amount_minor: outstanding }));
    scanner?.focus();
  }

  async function printReceipt() {
    // The width is the paper's, not the screen's. 32 characters is a 58mm roll,
    // which is what a small shop has.
    const reply = await attempt(() =>
      run({ op: 'receipt', width: 32, rung_at: new Date().toLocaleString('en-GB') }),
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
    const reply = await attempt(() => run({ op: 'checkout', ticket_id: id, rung_at_ms: Date.now() }));
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
      await printReceipt();
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
        <span class="warn" title="This browser would not promise to keep it: send what is waiting before you close">
          on this device, not promised
        </span>
      {:else if storage === 'opfs'}
        <span class="good" title="Sales survive this tab closing">on this device</span>
      {:else if storage === 'memory'}
        <span class="warn" title="Nothing survives a reload">memory only</span>
      {:else}
        <span class="warn">{storage}</span>
      {/if}
      <span>{view?.unsynced_sales ?? 0} to send</span>
      <span>{view?.receipt_numbers_left ?? 0} numbers</span>
      <span class={syncing.startsWith('held up') ? 'warn' : ''}>{syncing}</span>
      {#if operator}
        <button class="link" onclick={signOut}>{operator.name}, sign out</button>
      {/if}
    </div>
  </header>

  {#if refused}
    <!-- Above everything, because nothing below it is reaching the shop. -->
    <p class="fault" role="alert">
      The shop is refusing this device. Its terminal may have been removed, or
      its access withdrawn. Nothing it rings will arrive until it is enrolled
      again{#if waiting > 0}, and {waiting} {waiting === 1 ? 'sale is' : 'sales are'} still waiting to be sent{/if}.
    </p>
  {/if}

  {#if blocked}
    <!-- The one failure at a till that somebody standing behind the counter
         can fix in ten seconds. Until this existed the way through it was to
         sign out and back in as the supervisor, in front of the customer, and
         the cashier retyped what they had already typed. -->
    <section class="carry">
      <p class="why">
        That needs a supervisor. One of them can allow it here, for this one
        thing, without signing the cashier out.
      </p>
      <input
        bind:value={supervisorPin}
        type="password"
        inputmode="numeric"
        placeholder="Supervisor's PIN"
        disabled={busy}
      />
      <span class="row">
        {#each people.filter((one) => one.may_authorise) as one (one.id)}
          <button onclick={() => allowIt(one)} disabled={busy}>{one.name} allows it</button>
        {/each}
        <button class="quiet" onclick={() => { blocked = null; supervisorPin = ''; }} disabled={busy}>
          Leave it
        </button>
      </span>
      {#if people.filter((one) => one.may_authorise).length === 0}
        <p class="why">
          Nobody on this till may authorise anything. The shop sets that in the
          back office, under People.
        </p>
      {/if}
    </section>
  {/if}

  {#if refused || carrying}
    <!-- The way out. A device the shop will not take sales from is holding the
         only record of goods that left it, and enrolling again as another
         terminal abandons them. So they are read off it and carried. -->
    <section class="carry">
      <button onclick={showCarrying} disabled={busy}>
        {carrying ? 'Read them again' : 'What is still on this device'}
      </button>
      {#if carrying}
        {#if carrying.sales.length === 0}
          <p class="why">Nothing is waiting here. This device can be enrolled again safely.</p>
        {:else}
          <p class="why">
            {carrying.sales.length} {carrying.sales.length === 1 ? 'sale' : 'sales'},
            {money(carrying.total_minor)} in all.
            {#if carrying.sales.some((sale) => sale.salvaged)}
              Some were read back out of a damaged log and are marked for somebody to check.
            {/if}
            Copy the text below and paste it into the back office, under "Sales carried in by hand".
            Do not wipe this device until the back office says it has them.
          </p>
          <ul class="found">
            {#each carrying.sales as sale (sale.id)}
              <li>
                <span class="detail">
                  {money(sale.total_minor)}
                  {#if sale.salvaged}&middot; read back from a damaged log{/if}
                </span>
              </li>
            {/each}
          </ul>
          <p class="why">
            Mark <strong>{carrying.mark}</strong>, {carrying.letters} letters. The back office shows
            the mark of what it received: if the two differ, not all of it arrived.
          </p>
          <div class="row">
            <button onclick={saveCarried} disabled={busy}>Save it to a file</button>
            <button onclick={copyCarried} disabled={busy}>Copy it</button>
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
        placeholder={refused ? 'A new enrolment code from the shop owner' : 'Enrolment code from the shop owner'}
        autocomplete="off"
        disabled={busy}
      />
      <button onclick={join} disabled={busy}>Enrol</button>
    </div>
  {/if}

  {#if enrolled && !operator}
    <!-- Nobody is at the till. Every permission refuses until somebody is, and
         a screen that let a sale start anyway would refuse at the till point
         where it matters most. -->
    <section class="signin">
      {#if people.length === 0}
        <p class="fault">
          Nobody has been added to this shop yet, so nobody can sign in. That is
          a different problem from a forgotten PIN, and the owner fixes it.
        </p>
      {:else if !picked}
        <p>Who is at the till?</p>
        <div class="who">
          {#each people as person (person.id)}
            <button onclick={() => { picked = person; pin = ''; }}>
              {label(person, twiceOver)}
            </button>
          {/each}
        </div>
      {:else}
        <p>{label(picked, twiceOver)}, enter your PIN</p>
        <div class="row">
          <input
            type="password"
            bind:value={pin}
            onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); signIn(); } }}
            inputmode="numeric"
            autocomplete="off"
            disabled={busy}
          />
          <button onclick={signIn} disabled={busy}>Sign in</button>
          <button onclick={() => { picked = null; pin = ''; }}>Back</button>
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
    onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); scan(); } }}
    placeholder="Scan or type a barcode"
    autocomplete="off"
    inputmode="numeric"
    disabled={busy}
  />

  {#if operator}
    {#if lookingUp}
      <div class="row lookup">
        <input
          bind:value={hunt}
          oninput={look}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); look(); } }}
          placeholder="Part of the name or the code"
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
        <p class="empty">Nothing by that name.</p>
      {/if}
    {:else if unknown}
      <!-- A delivery that arrived while the line was down. Written here rather
           than lost: the customer is holding it. -->
      <section class="unknown">
        <p class="why">
          Nothing in the catalogue has the barcode {unknown}. Say what it is and
          it sells now; the shop sees it as something a till wrote down.
        </p>
        <input bind:value={newName} placeholder="What it is" disabled={busy} />
        <div class="row">
          <input bind:value={newPrice} placeholder="Price in taka" inputmode="decimal" disabled={busy} />
          <input bind:value={newVat} placeholder="Tax %" inputmode="decimal" disabled={busy} />
        </div>
        <div class="row">
          <button onclick={writeItDown} disabled={busy}>Write it down and sell it</button>
          <button class="quiet" onclick={() => { unknown = null; scanner?.focus(); }} disabled={busy}>
            Leave it
          </button>
        </div>
      </section>
    {:else}
      <button class="lookup" onclick={() => { lookingUp = true; }} disabled={busy}>
        No barcode? Look it up
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
              aria-label="how many"
              disabled={busy}
            />
            <button onclick={() => changeQty(at, line.qty_milli + 1000)} disabled={busy}>+</button>
            {#if ceiling > 0}
              <input
                class="off"
                value={line.discount_bp ? line.discount_bp / 100 : ''}
                onchange={(e) => discountLine(at, e.currentTarget.value)}
                placeholder="% off"
                inputmode="decimal"
                disabled={busy}
              />
              <!-- The same thing said the way a shop says it. Both are offered
                   because both are said: "ten percent" over a counter and
                   "twenty taka off" across it. -->
              <input
                class="off"
                onchange={(e) => takeOffLine(at, e.currentTarget.value)}
                placeholder="off"
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
            <button class="drop" onclick={() => drop(at)} disabled={busy}>Take it off</button>
          </div>
        {/if}
      </li>
    {:else}
      <li class="empty">Nothing rung yet</li>
    {/each}
  </ul>

  <section class="totals">
    <div><span>Net</span><span>{money(view?.net_minor ?? 0)}</span></div>
    {#if (view?.discount_minor ?? 0) !== 0}
      <div><span>Discount</span><span>{money(-view.discount_minor)}</span></div>
    {/if}
    <div><span>VAT</span><span>{money(view?.vat_minor ?? 0)}</span></div>
    <div class="due"><span>Total</span><span>{money(total)}</span></div>
    <div>
      <span>{refunding ? 'Given back' : 'Paid'}</span>
      <span>{money(view?.tendered_minor ?? 0)}</span>
    </div>
    <!-- One line, and only one: whichever of these the cashier is about to do is
         the only question they have. Showing change on a refund before anything
         has been handed over reads as money already given. -->
    {#if refunding && outstanding !== 0}
      <div class="owed"><span>To refund</span><span>{money(-outstanding)}</span></div>
    {:else if !refunding && outstanding > 0}
      <div class="owed"><span>Still owed</span><span>{money(outstanding)}</span></div>
    {:else if settled && !refunding && view.change_minor > 0}
      <div class="change"><span>Change</span><span>{money(view.change_minor)}</span></div>
    {/if}
  </section>

  {#if operator && parked.length > 0}
    <section class="parked">
      <p class="why">
        Parked, and still to be dealt with. Nothing here has been rung up or
        taken money.
      </p>
      <ul>
        {#each parked as held (held.id)}
          <li>
            <span class="name">{held.label}</span>
            <span class="each">
              {held.lines} {held.lines === 1 ? 'line' : 'lines'} &middot; {money(held.total_minor)}
            </span>
            <button onclick={() => resume(held)} disabled={busy}>Bring it back</button>
            <button class="drop" onclick={() => discard(held)} disabled={busy}>Throw away</button>
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
          placeholder="% off the whole ticket, up to {ceiling / 100}"
          inputmode="decimal"
          disabled={busy}
        />
        <button onclick={discountTicket} disabled={busy}>Discount</button>
      </div>
      <div class="row">
        <input
          bind:value={ticketOffAmount}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); takeOffTicket(); } }}
          placeholder="or an amount off the whole ticket"
          inputmode="decimal"
          disabled={busy}
        />
        <button onclick={takeOffTicket} disabled={busy}>Take it off</button>
      </div>
    {/if}
    <div class="row">
      <input
        bind:value={cash}
        onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); tender(); } }}
        placeholder="Cash taken"
        inputmode="decimal"
        disabled={busy}
      />
      <button onclick={tender} disabled={busy}>Take cash</button>
    </div>
    {#if operator && (view?.lines?.length ?? 0) > 0}
      <div class="row">
        <select bind:value={payingBy} disabled={busy}>
          <option value="cash">Cash</option>
          <option value="wallet">A wallet</option>
          <option value="card">Card</option>
          <option value="credit">On account</option>
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
            <input bind:value={walletName} placeholder="Which wallet" disabled={busy} />
          {/if}
        {/if}
        {#if payingBy === 'credit'}
          <!-- Somebody the shop wrote down, when it has. What they owe is then
               added up against a person rather than against the spelling a
               cashier used that day, which is how two Karims share an account.
               A shop that has written nobody down still types a name. -->
          {#if customers.length > 0}
            <select value={view?.customer ?? ''} onchange={(e) => chooseCustomer(e.currentTarget.value)} disabled={busy}>
              <option value="">Somebody not on the list</option>
              {#each customers as one (one.id)}
                <option value={one.id}>
                  {label(one, customersTwiceOver)}{one.owed_minor
                    ? ` — owes ${money(one.owed_minor)}`
                    : ''}
                </option>
              {/each}
            </select>
          {/if}
          {#if !view?.customer}
            <input bind:value={reference} placeholder="Who owes it" disabled={busy} />
            <!-- Writing them down is what keeps two people with one name apart:
                 a debt against a typed name is added up under the spelling, and
                 the second Karim pays for the first one's rice. -->
            {#if reference.trim()}
              <input
                bind:value={newPhone}
                placeholder="Their phone, if you have it"
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
              Put it on {wantsCustomer}'s account
            </button>
          {/if}
        {/if}
        {#if payingBy === 'credit' && chosen}
          <!-- What they owed when the shop last said so, and when. Never the
               number alone: another till may have sold to them since, and a
               cashier reads a bare figure out across the counter as true. -->
          <span class="detail">
            {#if chosen.owed_minor}
              Owes {money(chosen.owed_minor)} as of
              {new Date(chosen.owed_as_of_ms).toLocaleTimeString('en-GB')}
            {:else if chosen.owed_as_of_ms}
              Owes nothing as of {new Date(chosen.owed_as_of_ms).toLocaleTimeString('en-GB')}
            {:else}
              This till has not been told what they owe yet
            {/if}
          </span>
        {:else if payingBy === 'wallet' || payingBy === 'card'}
          <input bind:value={reference} placeholder="Their reference" disabled={busy} />
        {/if}
        <button onclick={takeTender} disabled={busy}>Take it</button>
      </div>
    {/if}
    <button onclick={exact} disabled={busy || outstanding === 0}>
      {refunding ? `Refund ${money(-outstanding)}` : `Exact (${money(outstanding)})`}
    </button>
    {#if operator && (view?.lines?.length ?? 0) === 0 && !refunding}
      <!-- Only on an empty basket: a refund is a whole ticket, never a line
           mixed into a sale. -->
      <button onclick={startRefund} disabled={busy}>Start a refund</button>
    {/if}
    {#if operator && (view?.lines?.length ?? 0) > 0 && !settled}
      <!-- Only while a sale is unpaid and has something on it. A parked sale is
           one nobody has taken money for, and there is nothing to park before
           the first scan. -->
      <div class="row">
        <input
          bind:value={parkAs}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); park(); } }}
          placeholder="Whose is it?"
          disabled={busy}
        />
        <button onclick={park} disabled={busy}>Park it</button>
      </div>
    {/if}
    {#if operator && (view?.tendered_minor ?? 0) !== 0}
      <!-- Whenever money has been entered, settled or not. The mis-key this
           exists for is five thousand where five hundred was meant, which is an
           overpayment, which counts as settled: hiding it then hid it exactly
           when it was wanted. -->
      <button class="quiet" onclick={clearTenders} disabled={busy}>Take that money back</button>
    {/if}
    <button class="finish" onclick={checkout} disabled={busy || !settled}>Finish sale</button>
    {#if operator && (view?.lines?.length ?? 0) > 0}
      <!-- Last, and set apart: it throws away the whole basket. Removing five
           lines one at a time is five chances to leave one behind, and the one
           left behind is rung to the next customer. -->
      <button class="abandon" onclick={cancelSale} disabled={busy}>Give up on this sale</button>
    {/if}
    {#if receipt}
      <button onclick={() => window.print()}>Print again</button>
    {/if}
  </div>

  {#if operator}
    <section class="drawer">
      {#if !drawer || !drawer.open}
        <div class="row">
          <input
            bind:value={float_}
            placeholder="Opening float in the drawer"
            inputmode="decimal"
            disabled={busy}
          />
          <button onclick={openShift} disabled={busy}>Open drawer</button>
        </div>

      {:else}
        <div class="drawerline">
          <span>Drawer: {drawer.sales} sales, should hold</span>
          <strong>{money(drawer.expected_cash_minor)}</strong>
        </div>
        <div class="row">
          <input bind:value={movement} placeholder="Amount" inputmode="decimal" disabled={busy} />
          <input bind:value={reason} placeholder="Why" disabled={busy} />
          <button onclick={() => moveCash(true)} disabled={busy}>In</button>
          <button onclick={() => moveCash(false)} disabled={busy}>Out</button>
        </div>
        <div class="row">
          <input bind:value={counted} placeholder="Counted cash" inputmode="decimal" disabled={busy} />
          <button onclick={closeShift} disabled={busy}>Close drawer</button>
          <button onclick={xReport} disabled={busy}>Totals</button>
        </div>
      {/if}

      {#if report}
        <!-- One block for both reports: a Z is an X plus what was counted, and
             two blocks would render the same figures twice and let them drift. -->
        <div class="report">
          <div><span>{report.closed_at_ms ? 'Z report' : 'Totals so far'}</span>
               <span>{report.sales} sales</span></div>
          <div><span>Opening float</span><span>{money(report.opening_float_minor)}</span></div>
          {#each report.tenders as row (row.name)}
            <div>
              <span>{row.name}{row.in_drawer ? '' : ' (not in the till)'}</span>
              <span>{money(row.amount_minor)}</span>
            </div>
          {/each}
          {#if report.cash_in_minor !== 0}
            <div><span>Cash in</span><span>{money(report.cash_in_minor)}</span></div>
          {/if}
          {#if report.cash_out_minor !== 0}
            <div><span>Cash out</span><span>{money(report.cash_out_minor)}</span></div>
          {/if}
          <div class="due"><span>Should hold</span><span>{money(report.expected_cash_minor)}</span></div>
          {#if report.counted_cash_minor !== undefined && report.counted_cash_minor !== null}
            <div><span>Counted</span><span>{money(report.counted_cash_minor)}</span></div>
            <!-- Negative is short, which is a fact to report rather than an
                 error to refuse: a shift that could not close short would be
                 closed dishonestly. -->
            <div class={report.variance_minor === 0 ? 'change' : 'owed'}>
              <span>{report.variance_minor === 0 ? 'Exactly right' : 'Out by'}</span>
              <span>{report.variance_minor === 0 ? '' : money(report.variance_minor)}</span>
            </div>
          {/if}
        </div>
        <!-- The slip goes in the drawer with the cash. Before this the figures
             were on the screen and nowhere else, so they were copied by hand. -->
        <button onclick={printDrawer} disabled={busy}>Print this</button>
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
