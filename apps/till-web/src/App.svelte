<script>
  import { onMount } from 'svelte';
  import { open, run, connect, enrol, sync } from './till.js';
  import { money, qty } from './format.js';

  // The demo server enrols shop 1, terminal 1. A real device would learn these
  // from the enrolment reply; that is in todo.md.
  const TENANT = '00000000000000000000000001';
  const TERMINAL = '00000000000000000000000001';
  const SERVER = window.location.origin.replace(/:\d+$/, ':8099');

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
  let scanner;

  const operator = $derived(view?.operator ?? null);
  const people = $derived(view?.people ?? []);

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
    const reply = await attempt(() => open(TENANT, TERMINAL));
    storage = reply?.info?.storage ?? 'unavailable';
    await connect(SERVER);
    enrolled = Boolean(reply?.view?.enrolled);

    // One round every two seconds. The core decides whether a round does
    // anything; this only decides how often to ask, and asking costs nothing
    // when the answer is to wait.
    setInterval(async () => {
      if (!enrolled || busy) return;
      try {
        const outcome = await sync(Date.now());
        if (outcome.view) view = outcome.view;
        syncing = outcome.info?.did ?? 'idle';
      } catch (error) {
        // Shown, not swallowed. A till that quietly stops syncing is the
        // failure the whole design is arranged against.
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
      const reply = await enrol(typed, Date.now());
      if (reply.view) view = reply.view;
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

  async function startRefund() {
    // Refused unless this person may, or a supervisor has allowed it. The
    // refusal is the core's own words, which name what is missing.
    await attempt(() => run({ op: 'start_refund', now_ms: Date.now() }));
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

  {#if !enrolled}
    <div class="row enrol">
      <input
        bind:value={code}
        onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); join(); } }}
        placeholder="Enrolment code from the shop owner"
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

  <ul class="lines">
    {#each view?.lines ?? [] as line (line.item_id + line.name)}
      <li>
        <span class="name">{line.name}</span>
        <span class="qty">{qty(line.qty_milli)}</span>
        <span class="each">{money(line.unit_price_minor)}</span>
      </li>
    {:else}
      <li class="empty">Nothing rung yet</li>
    {/each}
  </ul>

  <section class="totals">
    <div><span>Net</span><span>{money(view?.net_minor ?? 0)}</span></div>
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

  <div class="actions">
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
    <button onclick={exact} disabled={busy || outstanding === 0}>
      {refunding ? `Refund ${money(-outstanding)}` : `Exact (${money(outstanding)})`}
    </button>
    {#if operator && (view?.lines?.length ?? 0) === 0 && !refunding}
      <!-- Only on an empty basket: a refund is a whole ticket, never a line
           mixed into a sale. -->
      <button onclick={startRefund} disabled={busy}>Start a refund</button>
    {/if}
    <button class="finish" onclick={checkout} disabled={busy || !settled}>Finish sale</button>
    {#if receipt}
      <button onclick={() => window.print()}>Print again</button>
    {/if}
  </div>

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
  .lines { list-style: none; margin: 1rem 0; padding: 0; }
  .lines li {
    display: grid; grid-template-columns: 1fr auto auto; gap: 1rem;
    padding: 0.5rem 0; border-bottom: 1px solid #e6e3d8;
  }
  .lines .empty { color: #8a877a; border: 0; }
  .qty, .each { font-variant-numeric: tabular-nums; }
  .totals { display: grid; gap: 0.25rem; margin: 1rem 0; }
  .totals div { display: flex; justify-content: space-between; font-variant-numeric: tabular-nums; }
  .due { font-weight: 700; font-size: 1.25rem; padding-top: 0.35rem; border-top: 2px solid #16150f; }
  .owed { color: #8a2018; font-weight: 600; }
  .change { color: #1d6b3a; font-weight: 700; font-size: 1.15rem; }
  .actions { display: grid; gap: 0.6rem; }
  .row { display: flex; gap: 0.6rem; }
  .enrol { margin-bottom: 0.75rem; }
  .signin { margin-bottom: 0.75rem; }
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
