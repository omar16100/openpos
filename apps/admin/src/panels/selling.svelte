<script>
  import { daysAgo, today } from '../../../shared/days.js';
  import { notMoving, runningLow } from '../../../shared/buying.js';
  import { groupSold } from '../../../shared/sorting.js';

  /// What moved off the shelves over a period, which is what a shop orders
  /// against.
  ///
  /// Three answers to one question, and the last two are the reason the first
  /// is worth asking: what sold, what is about to run out at that rate, and
  /// what is sitting there not moving at all. The arithmetic for the last two
  /// lives in buying.js with tests, because it is the shop deciding what to buy.
  ///
  /// What this device knows about its own catalogue is handed in rather than
  /// read here: four panels name items from it, and one copy is one answer.
  let {
    t,
    money,
    qty,
    busy,
    attempt,
    admin,
    tills,
    names,
    kinds,
    costs,
    onHand,
    shelfIsWhole,
    askWholeShelf,
    learnNames,
    refuse,
  } = $props();

  /// A week back by default: the question is usually about something that
  /// happened recently and is remembered vaguely.
  let soldFrom = $state(daysAgo(7));
  let soldTo = $state(today());
  let sold = $state([]);
  /// How long the window those sales came from was, which is what turns a
  /// quantity into a rate a shelf can be measured against.
  let soldWindowMs = $state(7 * 86_400_000);
  /// How close to running out is worth walking to the wholesaler for. The
  /// shop's own answer: it depends on when the supplier comes.
  let daysWanted = $state('7');
  /// What supervisors allowed over the same window, which is the other half of
  /// reading a quiet week: what was sold, and what was given away.
  let waived = $state([]);

  const lowOnStock = $derived(runningLow(sold, onHand, soldWindowMs, Number(daysWanted) || 7));
  // What is sitting there instead. Shown only when the shop asked for the whole
  // shelf rather than a page of it: a figure added up from two hundred of eight
  // hundred items is not the figure, and a screen that shows it as one is lying
  // quietly.
  const deadStock = $derived(shelfIsWhole ? notMoving(sold, onHand, costs) : []);
  const soldByKind = $derived(groupSold(sold, kinds));

  /// What sold between two days, most sold first.
  export async function askSold() {
    const start = new Date(`${soldFrom}T00:00:00`);
    const end = new Date(`${soldTo}T00:00:00`);
    if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) {
      refuse(t('admin.not_dates'));
      return;
    }
    end.setDate(end.getDate() + 1);
    const reply = await attempt(
      () =>
        admin(
          { what: 'sold', from_ms: start.getTime(), to_ms: end.getTime() - 1, limit: 100 },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    sold = reply.info?.sold ?? [];
    // How long the shelf lasts at that rate, which needs what is on it now.
    // Asked for the same items and the same window, so the two halves of the
    // answer cannot be about different weeks.
    soldWindowMs = end.getTime() - start.getTime();
    // The whole shelf rather than the items that sold, because the other half
    // of this question is what did not sell at all, and those are exactly the
    // rows a list of what sold does not have.
    await askWholeShelf();
    // Asked for the same window, and asked at all: this list was rendered and
    // never fetched, so a report the shop was told it had showed nothing for as
    // long as it existed.
    const given = await attempt(
      () =>
        admin(
          { what: 'waived', from_ms: start.getTime(), to_ms: end.getTime() - 1, limit: 100 },
          Date.now(),
        ),
      null,
      true,
    );
    waived = given?.info?.waived ?? [];
    // The names come from this device's own catalogue, so a report is not the
    // same strings sent again on every request for the life of the shop.
    if (sold.length > 0 && Object.keys(names).length === 0) await learnNames();
  }
</script>

  <section>
    <h2>{t('admin.what_sold')}</h2>
    <p class="why">{t('admin.sold_why')}</p>
    <div class="row">
      <input type="date" bind:value={soldFrom} disabled={busy} />
      <input type="date" bind:value={soldTo} disabled={busy} />
      <button onclick={askSold} disabled={busy}>{t('admin.look')}</button>
    </div>
    {#if waived.length > 0}
      <p class="why">
        <span class="late">{t('admin.waived_count', { count: waived.length })}</span>
        {t('admin.waived_why')}
      </p>
      <ul class="found">
        {#each waived as one (one.sale + one.reason)}
          <li>
            <span class="name">{one.reason}</span>
            <span class="detail">
              {new Date(one.rung_at_ms).toLocaleString('en-GB')}
              &middot; {t('admin.on_a_sale_of', { amount: money(one.total_minor) })}
              &middot; {tills.find((till) => till.id === one.terminal)?.label ??
                t('admin.a_till_not_listed')}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if sold.length > 0}
      <p class="why">
        <strong>{t('admin.what_to_buy')}</strong> {t('admin.what_to_buy_why')}
      </p>
      <div class="row">
        <input
          bind:value={daysWanted}
          inputmode="numeric"
          placeholder={t('admin.days')}
          disabled={busy}
        />
        <span class="why">{t('admin.days_of_stock_left')}</span>
      </div>
      {#if lowOnStock.length > 0}
        <ul class="found">
          {#each lowOnStock as row (row.item)}
            <li>
              <span class="name">{names[row.item] ?? t('admin.something_unnamed')}</span>
              <span class="detail">
                {#if row.on_hand_milli <= 0}
                  <span class="late">{t('admin.nothing_left')}</span>
                {:else}
                  {t('admin.left_and_days', { qty: qty(row.on_hand_milli) })} &middot;
                  {row.days_left < 1
                    ? t('admin.about_under_a_day')
                    : t('admin.about_days', { days: Math.floor(row.days_left) })}
                {/if}
                &middot; {t('admin.sold_over_window', { qty: qty(row.sold_milli) })}
              </span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="why">{t('admin.nothing_close_to_out')}</p>
      {/if}
      {#if deadStock.length > 0}
        <p class="why">
          <strong>{t('admin.not_moving')}</strong> {t('admin.not_moving_why')}
        </p>
        <ul class="found">
          {#each deadStock.slice(0, 20) as row (row.item)}
            <li>
              <span class="name">{names[row.item] ?? t('admin.something_unnamed')}</span>
              <span class="detail">
                {t('admin.on_the_shelf', { qty: qty(row.on_hand_milli) })}
                {#if row.costed}
                  &middot; {t('admin.of_your_money', { amount: money(row.worth_minor) })}
                {:else}
                  &middot; <span class="late">{t('admin.cost_not_said')}</span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
        <p class="why">
          {t('admin.dead_stock_total', {
            amount: money(deadStock.reduce((total, row) => total + row.worth_minor, 0)),
            count: deadStock.length,
          })}
        </p>
      {/if}
    {/if}
    {#if sold.length > 0}
      {#each soldByKind as group (group.kind)}
        <p class="why"><strong>{group.kind}</strong></p>
        <ul class="found">
          {#each group.rows as row (row.item)}
            <li>
              <span class="name">{names[row.item] ?? t('admin.something_unnamed')}</span>
              <span class="detail">
                {qty(row.qty_milli)} &middot; {t('admin.over_sales', { count: row.sales })}
              </span>
            </li>
          {/each}
        </ul>
      {/each}
    {/if}
  </section>
