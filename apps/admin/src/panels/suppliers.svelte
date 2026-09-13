<script>
  import { saving } from '../../../shared/records.js';
  import { minorFrom } from '../../../shared/money.js';
  import { idForThisOne, whatIsOnTheForm } from '../../../shared/one_id.js';

  /// Who the shop buys from, what it owes them, and what has come in.
  ///
  /// One panel because they are one question asked three ways: a delivery is
  /// filed against a supplier, what is owed is the deliveries less what has
  /// been paid, and the statement is both of those side by side.
  ///
  /// The list of suppliers itself belongs to the screen, because the delivery
  /// form on the shelf panel picks from it. This panel is handed it and says
  /// when it has changed.
  let {
    t,
    money,
    qty,
    busy,
    attempt,
    admin,
    newId,
    suppliers,
    names,
    learnNames,
    onSuppliers,
    announce,
    refuse,
  } = $props();

  // What the shop owes its suppliers: the deliveries less what has been paid.
  let supplierOwing = $state([]);
  // What has come in, newest first.
  let deliveries = $state([]);
  // What is being typed against each supplier, and the id that payment will be
  // recorded under. See idForThisOne: the same payment pressed twice is one
  // payment, and a different amount typed after a reply went missing is a
  // different payment, which the shop would otherwise drop as a repeat.
  let payingSupplier = $state({});
  let paying = $state({});
  // The supplier form: correcting somebody, or adding them.
  let editingSupplier = $state(null);
  let supplierName = $state('');
  let supplierPhone = $state('');
  let supplierBin = $state('');
  // Which supplier's statement is open, and what is in it.
  let statementFor = $state(null);
  let statement = $state([]);

  export async function owed(quiet = true) {
    const reply = await attempt(() => admin({ what: 'supplier_owing' }, Date.now()), null, quiet);
    if (reply) supplierOwing = reply.info?.supplier_owing ?? [];
  }

  export async function whatCameIn(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'deliveries', limit: 20 }, Date.now()),
      null,
      quiet,
    );
    if (!reply) return;
    deliveries = reply.info?.deliveries ?? [];
    await learnNames();
  }

  /// Record what was handed to a supplier.
  ///
  /// The id is kept while the amount is the same one, so pressing again after a
  /// reply that never came is the same payment rather than a second one, and an
  /// amount corrected before the second press is a different payment rather
  /// than one the shop drops as a repeat.
  async function paySupplier(owing) {
    const typed = payingSupplier[owing.supplier] ?? '';
    const poisha = minorFrom(typed);
    if (poisha === null || poisha <= 0) {
      refuse(t('admin.say_how_much'));
      return;
    }
    const kept = idForThisOne(
      paying[owing.supplier] ?? null,
      whatIsOnTheForm(owing.supplier, poisha),
      newId,
    );
    paying = { ...paying, [owing.supplier]: kept };

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'pay_supplier',
            id: kept.id,
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
        ? t('admin.you_still_owe_them', { amount: money(now) })
        : now < 0
          ? t('admin.you_are_paid_ahead', { amount: money(-now) })
          : t('admin.you_owe_them_nothing');
    payingSupplier = { ...payingSupplier, [owing.supplier]: '' };
    paying = { ...paying, [owing.supplier]: null };
    await owed(true);
    // Said after the list is read back: the read clears the last message, so a
    // sentence written before it is a sentence nobody sees.
    announce(
      reply.info?.already_paid
        ? `${t('admin.already_recorded')}${after}`
        : `${t('admin.payment_written_down')}${after}`,
    );
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
      refuse(t('admin.say_supplier_name'));
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
      editingSupplier
        ? t('admin.supplier_corrected', { name: supplierName.trim() })
        : t('admin.supplier_added', { name: supplierName.trim() }),
    );
    if (!reply) return;
    onSuppliers(reply.info?.suppliers);
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
        ? t('admin.supplier_back', { name: one.name })
        : t('admin.supplier_stopped', { name: one.name }),
    );
    if (reply) onSuppliers(reply.info?.suppliers);
  }
</script>

  <section>
    <h2>{t('admin.who_you_buy_from')}</h2>
    <p class="why">{t('admin.suppliers_why')}</p>
    {#if suppliers.length > 0}
      <ul class="found">
        {#each suppliers as one (one.id)}
          <li class:retired={!one.active}>
            <span class="name">{one.name}</span>
            <span class="detail">
              {one.phone ?? t('admin.no_phone_short')}{#if one.bin}{' '}&middot; {t('admin.bin_is', {
                  bin: one.bin,
                })}{/if}
              {#if !one.active}&middot; {t('admin.no_longer_bought_from')}{/if}
            </span>
            <span class="acts">
              <button onclick={() => correctSupplier(one)} disabled={busy}>{t('admin.correct')}</button>
              {#if one.active}
                <button class="quiet" onclick={() => setBuying(one, false)} disabled={busy}>
                  {t('admin.stop')}
                </button>
              {:else}
                <button class="quiet" onclick={() => setBuying(one, true)} disabled={busy}>
                  {t('admin.buy_again')}
                </button>
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    <input bind:value={supplierName} placeholder={t('admin.name')} disabled={busy} />
    <div class="row">
      <input bind:value={supplierPhone} placeholder={t('admin.phone')} inputmode="tel" disabled={busy} />
      <input bind:value={supplierBin} placeholder={t('admin.bin_if_any')} disabled={busy} />
    </div>
    <div class="row">
      <button onclick={saveSupplier} disabled={busy}>
        {editingSupplier ? t('admin.save_the_correction') : t('admin.add_them')}
      </button>
      {#if editingSupplier}
        <button class="quiet" onclick={newSupplier} disabled={busy}>{t('admin.leave_them_alone')}</button>
      {/if}
    </div>
  </section>

  <section>
    <h2>{t('admin.owe_suppliers')}</h2>
    <p class="why">{t('admin.supplier_owing_why')}</p>
    {#if supplierOwing.length > 0}
      <ul class="found">
        {#each supplierOwing as owing (owing.supplier)}
          <li>
            <span class="name">{owing.name || t('admin.a_supplier_not_listed')}</span>
            <span class="detail">
              {#if owing.owed_minor >= 0}
                {t('admin.you_owe', { amount: money(owing.owed_minor) })}
              {:else}
                {t('admin.paid_ahead', { amount: money(-owing.owed_minor) })}
              {/if}
              &middot; {t('admin.deliveries_count', { count: owing.deliveries })}
              &middot; {t('admin.since_date', {
                date: new Date(owing.since_ms).toLocaleDateString('en-GB'),
              })}
            </span>
            <span class="row">
              <input
                placeholder={t('admin.taka_you_handed_over')}
                bind:value={payingSupplier[owing.supplier]}
              />
              <button onclick={() => paySupplier(owing)} disabled={busy}>{t('admin.paid_them')}</button>
              <button onclick={() => showStatement(owing)} disabled={busy}>
                {statementFor === owing.supplier ? t('admin.hide') : t('admin.what_is_this')}
              </button>
            </span>
            {#if statementFor === owing.supplier}
              <ul class="found">
                {#each statement as line (line.at_ms + String(line.delivered) + line.amount_minor)}
                  <li>
                    <span class="detail">
                      {new Date(line.at_ms).toLocaleDateString('en-GB')}
                      &middot; {line.delivered ? t('admin.goods_in') : t('admin.paid')}
                      {money(line.amount_minor)}
                      {#if line.reference}&middot; {line.reference}{/if}
                    </span>
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="why">{t('admin.owe_suppliers_nothing')}</p>
    {/if}
  </section>

  <section>
    <h2>{t('admin.what_came_in')}</h2>
    <p class="why">
      {t('admin.deliveries_why')}
    </p>
    {#if deliveries.length > 0}
      <ul class="found">
        {#each deliveries as one (one.id)}
          <li>
            <span class="name">
              {suppliers.find((who) => who.id === one.supplier_id)?.name ??
                t('admin.nobody_recorded')}
              {#if one.reference} &middot; {one.reference}{/if}
            </span>
            <span class="detail">
              {new Date(one.received_at_ms).toLocaleString('en-GB')}
              &middot; {t('admin.lines_count', { count: one.lines.length })}
              <!-- From the shop, not added up here: the money on this screen
                   is the money the shop has, answered once. Absent means the
                   shop could not add it up, which is said rather than shown
                   as a zero: a delivery worth nothing and a delivery nobody
                   could add up are different things. -->
              &middot; {one.cost_minor === null || one.cost_minor === undefined
                ? t('admin.could_not_add_it_up')
                : money(one.cost_minor)}
            </span>
            <span class="detail">
              {one.lines
                .map(
                  (line) =>
                    `${qty(line.qty_milli)} × ${names[line.item_id] ?? t('admin.item_not_held')}`,
                )
                .join(', ')}
            </span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="why">{t('admin.nothing_booked_in')}</p>
    {/if}
  </section>
