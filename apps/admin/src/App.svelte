<script>
  import { onMount } from 'svelte';
  import { open, run, connect, enrol, sync, admin, adoptToken } from './till.js';
  import { money } from './format.js';

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
  let code = $state('');

  const enrolled = $derived(view?.enrolled ?? false);
  // Holding a credential the server will not accept. This device looks enrolled
  // and is not, and every form below would fail with a 401 nobody can read.
  const refused = $derived(view?.credential_refused ?? false);

  // Shop
  let shopName = $state('');
  let shopBin = $state('');
  let shopAddress = $state('');

  // A person
  let personName = $state('');
  let personPin = $state('');
  let personRole = $state('cashier');

  // An item. `editingId` is the id of the item being corrected, and null when
  // this is a new one. Without it every save minted a fresh id, so correcting a
  // price put a second copy on the shelf instead of fixing the first.
  let editingId = $state(null);
  // What the shop paid for the item being corrected. Held rather than shown,
  // because a form that omits it sends a zero and quietly wipes every margin.
  let editingCost = $state(0);
  let found = $state([]);
  let hunt = $state('');
  let itemCode = $state('');
  let itemName = $state('');
  let itemPrice = $state('');
  let itemVat = $state('15');
  let itemBarcode = $state('');
  let itemListedPrice = $state(false);

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
    }
    if (enrolled) await listTills();
    // The list is a health view: last heard from, sales, anything waiting to be
    // looked at. Loaded once it is a screenshot, and the one question it is
    // opened to answer is whether a till has stopped reporting. Slower than the
    // sync loop, because it is a whole-shop query and nobody watches it by the
    // second.
    setInterval(() => {
      if (enrolled && !busy) listTills();
    }, 15000);
    // The back office syncs too, so it holds the shop and the people and can
    // show what it is about to change rather than writing blind.
    setInterval(async () => {
      if (!enrolled || busy) return;
      try {
        const outcome = await sync(Date.now());
        if (outcome.view) view = outcome.view;
      } catch (error) {
        // The view still comes back, and it is what says whether the shop has
        // refused this device rather than merely gone quiet. Anything else here
        // is left to the next action to report.
        if (error.view) view = error.view;
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
      const adopted = await adoptToken(info.token);
      return { view: adopted.view ?? opened.view };
    }, 'Enrolled.');
    if (view?.enrolled) await listTills();
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
          },
          Date.now(),
        ),
      'Shop details saved. Tills pick them up within ten minutes.',
    );
  }

  async function savePerson() {
    if (!personName.trim() || personPin.length < 4) {
      fault = 'a name, and a PIN of at least four digits';
      return;
    }
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
    personName = '';
  }

  async function look(quiet = false) {
    const reply = await attempt(
      () => run({ op: 'catalogue', query: hunt.trim(), limit: 50 }),
      null,
      quiet,
    );
    if (reply) found = reply.view?.catalogue ?? [];
  }

  /// Load an item into the form so the next save corrects it.
  function correct(item) {
    editingId = item.id;
    editingCost = item.cost_minor;
    itemName = item.name;
    itemCode = item.code;
    itemPrice = (item.price_minor / 100).toFixed(2);
    itemVat = (item.vat_bp / 100).toString();
    itemBarcode = item.barcodes[0] ?? '';
    itemListedPrice = item.vat_on_undiscounted;
    scrollTo({ top: 0, behavior: 'smooth' });
  }

  function startFresh() {
    editingId = null;
    editingCost = 0;
    itemName = '';
    itemCode = '';
    itemPrice = '';
    itemVat = '15';
    itemBarcode = '';
    itemListedPrice = false;
  }

  async function saveItem() {
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
            item: {
              // The item's own id when this is a correction, a new one when it
              // is not. Minting one either way is what turned every price
              // change into a duplicate.
              id: editingId ?? newId(),
              code: itemCode.trim(),
              name: itemName.trim(),
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              barcodes: itemBarcode.trim() ? [itemBarcode.trim()] : [],
              on_hand_milli: 0,
            },
            price_minor: Math.round(price * 100),
            // The cost this item already had, when correcting one. Sending zero
            // here is how a price change becomes a margin nobody can explain.
            cost_minor: editingCost,
            vat_bp: Math.round(vat * 100),
            price_inclusive: false,
            vat_on_undiscounted: itemListedPrice,
          },
          Date.now(),
        ),
      editingId
        ? `${itemName.trim()} corrected. Tills pick it up within half a minute, and this list with them.`
        : `${itemName.trim()} added. Tills pick it up within half a minute.`,
    );
    // Only on success. Clearing the form after a refusal loses what the owner
    // typed and leaves them nothing to correct.
    if (!saved) return;
    startFresh();
    // The change reaches this device the way it reaches a till, on the next
    // pull, so the list is asked again rather than edited here to look right.
    // Quietly, or the confirmation is gone before it is read.
    await look(true);
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
  <h1>openpos back office</h1>

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
      <button onclick={savePerson} disabled={busy}>Add them</button>
      {#if (view?.people?.length ?? 0) > 0}
        <p class="why">Already here: {view.people.map((p) => p.name).join(', ')}</p>
      {/if}
    </section>

    <section>
      <h2>{editingId ? 'Correcting an item' : 'Something to sell'}</h2>
      {#if editingId}
        <p class="why">
          Saving changes this item everywhere. Tills pick it up on their next
          pull, and anything already rung keeps the price it was rung at.
        </p>
      {/if}
      <input bind:value={itemName} placeholder="Name" disabled={busy} />
      <div class="row">
        <input bind:value={itemPrice} placeholder="Price in taka" inputmode="decimal" disabled={busy} />
        <input bind:value={itemVat} placeholder="VAT %" inputmode="decimal" disabled={busy} />
      </div>
      <div class="row">
        <input bind:value={itemCode} placeholder="Code" disabled={busy} />
        <input bind:value={itemBarcode} placeholder="Barcode" inputmode="numeric" disabled={busy} />
      </div>
      <label>
        <input type="checkbox" bind:checked={itemListedPrice} disabled={busy} />
        Tax is fixed to the listed price, so a discount comes out of your margin
        rather than reducing the tax
      </label>
      <div class="row">
        <button onclick={saveItem} disabled={busy}>
          {editingId ? 'Save the correction' : 'Add it'}
        </button>
        {#if editingId}
          <button class="quiet" onclick={startFresh} disabled={busy}>Leave it alone</button>
        {/if}
      </div>
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
        <button onclick={look} disabled={busy}>Look</button>
      </div>
      {#if found.length > 0}
        <ul class="found">
          {#each found as item (item.id)}
            <li>
              <span class="name">{item.name}</span>
              <span class="detail">
                {item.code} &middot; {money(item.price_minor)}
                &middot; VAT {(item.vat_bp / 100).toFixed(item.vat_bp % 100 ? 2 : 0)}%
                {#if item.vat_on_undiscounted}&middot; taxed on the listed price{/if}
              </span>
              <button onclick={() => correct(item)} disabled={busy}>Correct it</button>
            </li>
          {/each}
        </ul>
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
  h2 { font-size: 1rem; margin: 0 0 0.25rem; }
  section {
    background: #fff; border: 1px solid #cfccbf; border-radius: 6px;
    padding: 0.9rem; margin-bottom: 1rem; display: grid; gap: 0.5rem;
  }
  .why { margin: 0; font-size: 0.85rem; color: #5a574a; }
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
  .found button { grid-row: 1 / 3; grid-column: 2; padding: 0.45rem 0.7rem; font-size: 0.9rem; }
  .quiet { background: #fff; color: #16150f; border-color: #cfccbf; }
  .tills { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
  .tills li {
    display: grid; grid-template-columns: 1fr auto; gap: 0.25rem 0.75rem;
    align-items: center; padding: 0.5rem 0; border-bottom: 1px solid #e6e3d8;
  }
  .tills .name { font-weight: 600; }
  .tills .seen { grid-column: 1; font-size: 0.8rem; color: #5a574a; }
  .tills button { grid-row: 1 / 3; grid-column: 2; padding: 0.45rem 0.7rem; font-size: 0.9rem; }
  .code {
    font: 1.6rem ui-monospace, Menlo, monospace; letter-spacing: 0.15em;
    margin: 0; padding: 0.5rem 0;
  }
</style>
