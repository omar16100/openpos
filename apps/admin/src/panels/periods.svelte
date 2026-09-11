<script>
  import { daysAgo, thisMonth, today } from '../../../shared/days.js';
  /// Two questions a shop asks about a period rather than about a thing: who
  /// allowed what, and what is owed the revenue.
  ///
  /// Together because they are read together at the end of a month, and apart
  /// from the rest because neither of them touches anything else on the screen:
  /// each is two date boxes, a button, and what came back.
  ///
  /// `announce` and `refuse` are the screen's one message line. A panel with
  /// its own would be a second place to look for the answer to "did that work".
  let { t, money, busy, attempt, admin, tills, announce, refuse } = $props();

  /// Who allowed what, over a week by default: long enough to cover a weekend
  /// somebody is asking about on the Monday.
  let allowedTrail = $state([]);
  let allowedFrom = $state(daysAgo(7));
  let allowedTo = $state(today());

  /// What the shop owes the revenue for a month, by rate.
  let vat = $state([]);
  let vatMonth = $state(thisMonth());
  let vatTotal = $state(0);
  let vatWaiting = $state({ sales: 0, minor: 0 });

  /// Who allowed what, between two days.
  async function askAllowed() {
    const start = new Date(`${allowedFrom}T00:00:00`);
    const end = new Date(`${allowedTo}T00:00:00`);
    if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) {
      refuse(t('admin.not_dates'));
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
    if (allowedTrail.length === 0) announce(t('admin.nothing_allowed_over'));
  }

  /// What the shop owes the revenue for a month, by rate.
  ///
  /// The total comes from the shop rather than being added up here. It is the
  /// one figure on this panel an owner copies onto a return, and the screen was
  /// summing the rows itself.
  async function askVat() {
    const start = new Date(`${vatMonth}-01T00:00:00`);
    if (Number.isNaN(start.getTime())) {
      refuse(t('admin.not_a_month'));
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
    vatTotal = reply.info?.vat_minor ?? 0;
    vatWaiting = {
      sales: reply.info?.vat_waiting_sales ?? 0,
      minor: reply.info?.vat_waiting_minor ?? 0,
    };
  }
</script>

  <section>
    <h2>{t('admin.what_was_allowed')}</h2>
    <p class="why">{t('admin.allowed_why')}</p>
    <div class="row">
      <input type="date" bind:value={allowedFrom} disabled={busy} />
      <input type="date" bind:value={allowedTo} disabled={busy} />
      <button onclick={askAllowed} disabled={busy}>{t('admin.look')}</button>
    </div>
    {#if allowedTrail.length > 0}
      <ul class="found">
        {#each allowedTrail as one (one.terminal + '/' + one.seq + '/' + one.at_ms)}
          <li>
            <span class="name">
              <!-- Said from the number the till stored, and falling back to
                   the sentence the bindings built: a screen older than the
                   till it is reading says something rather than nothing. -->
              {t(`allowed.${one.kind}`, {}, one.what)}{#if one.bp > 0}
                {t('admin.of_percent', { percent: one.bp / 100 })}{/if}
            </span>
            <span class="detail">
              {new Date(one.at_ms).toLocaleString('en-GB')}
              {#if one.refused}
                &middot; {t('admin.on_their_button', {
                  name: one.operator_name || t('admin.a_name_unreadable'),
                })}
              {:else if one.took_the_till || one.needed_no_permission}
                <!-- Signing in, and printing a receipt again. Neither is
                     something a permission covered, and saying one was
                     invites a shop to go looking for a permission to take
                     away that does not exist. -->
                &middot; {one.operator_name || t('admin.somebody_unnamed')}
              {:else if one.was_not_permitted}
                <!-- Somebody who tried and could not. This read "their own
                     permission covered it", which is the opposite of what
                     happened: the entry says they were stopped. -->
                &middot; {one.operator_name || t('admin.somebody_unnamed')}
                &middot; {t('admin.was_not_permitted')}
              {:else}
                &middot; {one.operator_name || t('admin.somebody_unnamed')}
                {#if one.authorised_by_name}
                  &middot; {t('admin.allowed_by', { name: one.authorised_by_name })}
                {:else}
                  &middot; {t('admin.own_permission')}
                {/if}
              {/if}
              {#if one.receipt_no}
                <!-- Which receipt was printed again. A trail that said only
                     that somebody printed something leaves a shop lining
                     times up against its own sales by hand, and the shape
                     worth seeing is one receipt printed three times rather
                     than three customers who lost their paper. Absent on
                     every other kind of entry, and on reprints written by a
                     till from before this was recorded: nothing is filled in
                     here after the fact. -->
                &middot; {t('admin.of_receipt', { number: one.receipt_no })}
              {/if}
              &middot; {tills.find((till) => till.id === one.terminal)?.label ??
                t('admin.a_till_not_listed')}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section>
    <h2>{t('admin.owe_the_revenue')}</h2>
    <p class="why">{t('admin.vat_why')}</p>
    <div class="row">
      <input type="month" bind:value={vatMonth} disabled={busy} />
      <button onclick={askVat} disabled={busy}>{t('admin.look')}</button>
    </div>
    {#if vat.length > 0}
      <ul class="found">
        {#each vat as row (row.vat_bp + '/' + (row.supply ?? 0))}
          <li>
            <span class="name">
              {#if row.supply === 1}
                {t('admin.supply_zero')}
              {:else if row.supply === 2}
                {t('admin.supply_exempt')}
              {:else}
                {(row.vat_bp / 100).toFixed(row.vat_bp % 100 ? 2 : 0)}%
              {/if}
            </span>
            <span class="detail">
              {t('admin.sold_amount', { net: money(row.net_minor) })}
              &middot; {t('admin.tax_amount', { vat: money(row.vat_minor) })}
              &middot; {t('admin.sales_of', { count: row.sales })}
            </span>
          </li>
        {/each}
      </ul>
      <!-- From the shop, not added up here. This is the figure an owner copies
           onto a return, and it was the screen's own arithmetic over rows the
           shop had sent: the same mistake as the delivery totals below it. -->
      <p class="figure">{money(vatTotal)}</p>
      <p class="why">{t('admin.tax_in_all')}</p>
      {#if vatWaiting.sales > 0}
        <p class="why">
          <span class="late">
            {t('admin.vat_waiting', {
              amount: money(vatWaiting.minor),
              count: vatWaiting.sales,
            })}
          </span>
          {t('admin.vat_waiting_why')}
        </p>
      {/if}
    {/if}
  </section>
