<script>
  import { onMount } from 'svelte';
  import { open, run, connect, enrol, sync, admin, adoptToken } from './till.js';
  import { money, qty } from './format.js';

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

  // A person
  let personName = $state('');
  let personPin = $state('');
  let personRole = $state('cashier');
  // Everybody, suspended included. The everyday list leaves them out, which is
  // right for a sign-in panel and leaves nowhere to let anybody back in.
  let everyone = $state([]);

  // An item. `editingId` is the id of the item being corrected, and null when
  // this is a new one. Without it every save minted a fresh id, so correcting a
  // price put a second copy on the shelf instead of fixing the first.
  let editingId = $state(null);
  // What the shop paid for the item being corrected. Held rather than shown,
  // because a form that omits it sends a zero and quietly wipes every margin.
  let editingCost = $state(0);
  // Whether the item being corrected is still sold. Carried through a
  // correction, or saving a price change would quietly put it back on sale.
  let editingActive = $state(true);
  // Whether the list includes what the shop has stopped selling. Off by
  // default: the everyday question is what is on the shelves.
  let showRetired = $state(false);
  // A delivery being built, and a count being taken. Keyed by item id, because
  // the same item must not appear twice in one delivery: the server books what
  // it is sent, and two lines for one item is a double delivery.
  let delivery = $state({});
  let counting = $state({});
  let reference = $state('');
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
    if (enrolled) {
      await listTills();
      await listPeople();
    }
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
        syncing = outcome.info?.did ?? 'idle';
      } catch (error) {
        // The view still comes back, and it is what says whether the shop has
        // refused this device rather than merely gone quiet.
        if (error.view) view = error.view;
        syncing = `held up: ${error.message}`;
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
    if (view?.enrolled) {
      await listTills();
      await listPeople();
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
    await listPeople();
  }

  /// Stop selling something, or start again.
  ///
  /// The whole item goes back with one field changed, because that is what the
  /// route takes. A till refuses to ring a retired item and still refunds one:
  /// the shop sold it last week and the customer is standing there with it.
  async function setSelling(item, selling) {
    await attempt(
      () =>
        admin(
          {
            what: 'item',
            item: {
              id: item.id,
              code: item.code,
              name: item.name,
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              // Copied, not passed. What comes out of the view is a reactive
              // proxy, and a proxy cannot be posted to a worker: it fails at the
              // boundary with a message about cloning that says nothing about
              // which field.
              barcodes: [...item.barcodes],
              on_hand_milli: item.on_hand_milli,
              active: selling,
            },
            price_minor: item.price_minor,
            cost_minor: item.cost_minor,
            vat_bp: item.vat_bp,
            price_inclusive: item.price_inclusive,
            vat_on_undiscounted: item.vat_on_undiscounted,
            active: selling,
          },
          Date.now(),
        ),
      selling
        ? `${item.name} is on sale again. Tills pick it up within half a minute.`
        : `${item.name} will not ring at a till any more. Refunds of it still work.`,
    );
    await look(true);
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
  function correct(item) {
    editingId = item.id;
    editingCost = item.cost_minor;
    editingActive = item.active;
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
    editingActive = true;
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
            // Carried through a correction. Sending true unconditionally is how
            // a price change would put a discontinued item back on the shelf.
            active: editingActive,
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

  /// Book a delivery, so the figures go up as well as down.
  ///
  /// Until this existed the only thing that moved stock was a sale, so every
  /// figure in the shop walked towards zero and stayed wrong.
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
    await look(true);
  }

  /// Record what a shelf was found to hold.
  ///
  /// A count replaces the running figure rather than adjusting it, which is the
  /// only way a figure that has drifted since the shop opened gets corrected.
  async function bookCount() {
    const lines = Object.entries(counting)
      // An empty box is a shelf nobody counted, and `Number('')` is zero: without
      // this, clearing a box books that shelf as empty, which is the one wrong
      // answer a count can give that looks like a real finding.
      .filter(([, typed]) => String(typed).trim() !== '')
      .map(([item_id, typed]) => ({
        id: newId(),
        item_id,
        qty_milli: Math.round(Number(typed) * 1000),
      }))
      .filter((line) => Number.isFinite(line.qty_milli) && line.qty_milli >= 0);
    if (lines.length === 0) {
      fault = 'nothing counted yet';
      return;
    }

    const reply = await attempt(
      () => admin({ what: 'count', counted_at_ms: Date.now(), lines }, Date.now()),
      `${lines.length} ${lines.length === 1 ? 'shelf' : 'shelves'} counted.`,
    );
    if (!reply) return;
    // Sales rung before the count that reached the server after it. Nobody can
    // say whether the person counting saw those goods, so the server leaves them
    // out of the figure and says so rather than quietly picking a side.
    const late = (reply.info?.on_hand ?? []).filter((entry) => entry.unreconciled_sales > 0);
    if (late.length > 0) {
      done = `${done} ${late.length} ${late.length === 1 ? 'item has' : 'items have'} sales that arrived after the count and are not in the figure.`;
    }
    counting = {};
    await look(true);
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
      () => admin({ what: 'operator_active', id: person.id, active: allowed }, Date.now()),
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
    <small>{syncing} &middot; catalogue read to {view?.catalogue_cursor ?? 0}</small>
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

      {#if everyone.length > 0}
        <ul class="found">
          {#each everyone as person (person.id)}
            <li class:retired={!person.active}>
              <span class="name">{person.name}</span>
              <span class="detail">
                {person.active ? 'can sign in' : 'suspended'}
              </span>
              <span class="acts">
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
          onclick={() => { stockMode = stockMode === 'receiving' ? 'off' : 'receiving'; counting = {}; }}
          disabled={busy}
        >
          {stockMode === 'receiving' ? 'Stop booking in' : 'Book in a delivery'}
        </button>
        <button
          class={stockMode === 'counting' ? '' : 'quiet'}
          onclick={() => { stockMode = stockMode === 'counting' ? 'off' : 'counting'; delivery = {}; }}
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
          <input bind:value={reference} placeholder="The supplier's challan or invoice number" disabled={busy} />
          <button onclick={bookDelivery} disabled={busy}>Book it in</button>
        </div>
      {:else if stockMode === 'counting'}
        <p class="why">
          What you found on the shelf. This replaces the running figure rather
          than adjusting it, which is how a number that has drifted gets fixed.
        </p>
        <button onclick={bookCount} disabled={busy}>Record the count</button>
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
                      value={counting[item.id] ?? ''}
                      oninput={(e) => (counting = { ...counting, [item.id]: e.currentTarget.value })}
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
  h1 small { font-weight: 400; font-size: 0.75rem; color: #5a574a; }
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
  .found .stock { grid-column: 1 / -1; display: flex; gap: 0.5rem; padding-top: 0.4rem; }
  .found .stock input { width: 12rem; padding: 0.5rem 0.6rem; }
  .found .acts { grid-row: 1 / 3; grid-column: 2; display: flex; gap: 0.4rem; }
  .found .acts button { padding: 0.45rem 0.7rem; font-size: 0.9rem; }
  .found .late { color: #7a5a1e; }
  .found li.retired .name { color: #8a877a; text-decoration: line-through; }
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
