<script>
  import { onMount } from 'svelte';
  import { open, run, connect, enrol, sync, describeSync, adoptToken } from './till.js';
  import { money, qty } from './format.js';

  const SERVER = window.location.origin.replace(/:\d+$/, ':8099');
  // Which shop and terminal this device is. Not secret, and needed before the
  // ledger can be opened, which is why it is here and the credential is not:
  // the credential lives in the ledger it belongs to.
  const IDENTITY = 'openpos.identity';

  let view = $state(null);
  let storage = $state('opening');
  let fault = $state(null);
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
  // Looking an item up by name, for a barcode that will not read, loose goods
  // that carry none, or a label torn off. The catalogue is on the device, so
  // this works with the line down like everything else at the counter.
  let lookingUp = $state(false);
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
    if (!line.discount_bp) return `${off}, this line's share of the ticket discount`;
    const rate = (line.discount_bp / 100).toFixed(line.discount_bp % 100 ? 2 : 0);
    return `${rate}% off this line, ${off} in all`;
  }

  async function changeQty(at, milli) {
    if (milli <= 0) {
      // Down to nothing is off the ticket. Sending a zero quantity would leave
      // a line reading "0 x Rice" that nobody can sell or clear.
      await drop(at);
      return;
    }
    await attempt(() => run({ op: 'set_qty', line: at, qty_milli: milli }));
  }

  async function drop(at) {
    editing = null;
    await attempt(() => run({ op: 'remove_line', line: at }));
    scanner?.focus();
  }

  async function priceLine(at, typed) {
    const taka = Number(typed);
    if (!Number.isFinite(taka) || taka < 0) {
      fault = 'a price in taka, and not a negative one';
      return;
    }
    await attempt(() => run({ op: 'set_unit_price', line: at, price_minor: Math.round(taka * 100) }));
  }

  async function discountLine(at, typed) {
    const percent = Number(typed === '' ? 0 : typed);
    if (!Number.isFinite(percent)) {
      fault = 'a discount is a percentage';
      return;
    }
    await attempt(() => run({ op: 'set_line_discount', line: at, percent }));
  }

  async function discountTicket() {
    const percent = Number(ticketOff === '' ? 0 : ticketOff);
    if (!Number.isFinite(percent)) {
      fault = 'a discount is a percentage';
      return;
    }
    await attempt(() => run({ op: 'set_ticket_discount', percent }));
    ticketOff = '';
    scanner?.focus();
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
      enrolled = Boolean(reply?.view?.enrolled);
    } else {
      // Nothing has told this device who it is yet, so there is no ledger to
      // open: a till opened as a guess would present a credential for one
      // terminal and a request body for another.
      storage = 'not enrolled';
    }

    // One round every two seconds. The core decides whether a round does
    // anything; this only decides how often to ask, and asking costs nothing
    // when the answer is to wait.
    setInterval(async () => {
      if (!enrolled || busy) return;
      try {
        const outcome = await sync(Date.now());
        if (outcome.view) view = outcome.view;
        syncing = describeSync(outcome.info);
      } catch (error) {
        // Shown, not swallowed. A till that quietly stops syncing is the
        // failure the whole design is arranged against. The view comes back with
        // the failure, and it is the only thing that says whether the shop has
        // refused this device outright.
        if (error.view) view = error.view;
        syncing = `held up: ${error.message}`;
      }
    }, 2000);
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
    await attempt(() => run({ op: 'start_refund', now_ms: Date.now() }));
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
    await attempt(() => run({ op: 'add', item_id: item.id, qty_milli: 1000 }));
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
    if (payingBy === 'credit' && !reference.trim()) {
      fault = 'say who owes it: a sale on account with no name cannot be chased';
      return;
    }
    cash = '';
    const owed = refunding ? -1 : 1;
    await attempt(() =>
      run({
        op: 'add_tender',
        kind: payingBy,
        name: walletName,
        amount_minor: Math.round(amount * 100) * owed,
        reference,
      }),
    );
    reference = '';
    scanner?.focus();
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

  async function scan() {
    const code = barcode.trim();
    if (!code) return;
    barcode = '';
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
      {#if storage === 'opfs'}
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
            <button onclick={() => { picked = person; pin = ''; }}>{person.name}</button>
          {/each}
        </div>
      {:else}
        <p>{picked.name}, enter your PIN</p>
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
        {#if editing === at}
          <!-- Under the line it changes, not in a dialog over it: a cashier
               correcting the third of five things is looking at the third. -->
          <div class="edit">
            <button onclick={() => changeQty(at, line.qty_milli - 1000)} disabled={busy}>&minus;</button>
            <span class="count">{qty(line.qty_milli)}</span>
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
          <input bind:value={reference} placeholder="Who owes it" disabled={busy} />
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
  .gave { grid-column: 1 / -1; font-size: 0.85rem; color: #7a5a1e; }
  .edit { grid-column: 1 / -1; display: flex; gap: 0.4rem; align-items: center; padding: 0.4rem 0; }
  .edit button {
    font: inherit; min-width: 2.6rem; padding: 0.5rem 0.6rem; border-radius: 6px;
    border: 1px solid #cfccbf; background: #fff; color: #16150f; cursor: pointer;
  }
  .edit .count { min-width: 2.5rem; text-align: center; font-variant-numeric: tabular-nums; }
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
