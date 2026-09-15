<script>
  import { today } from '../../../shared/days.js';

  /// What a day took, and what was made on it.
  ///
  /// One question asked twice, which is why one button asks both: an owner
  /// reading what came in wants to know what of it was margin, and two buttons
  /// for one day is two chances to compare figures from different days.
  ///
  /// Every figure here is the shop's. Nothing on this panel is added up.
  let { t, money, busy, attempt, admin, tills, refuse } = $props();

  /// The day being read. Today by default, which is the day somebody asks
  /// about at closing.
  let day = $state(today());
  let takings = $state(null);
  let made = $state(null);
  /// The day the figures on screen were asked for, so that changing the date
  /// cannot leave one day's takings sitting under another day's heading.
  let takingsFor = $state(null);

  export async function ask() {
    const start = new Date(`${day}T00:00:00`);
    // Whatever is on screen belongs to the day it was asked for, and a date
    // that is not one is not that day: clearing the field and pressing Look
    // used to leave the last day's takings sitting under a blank date beside
    // the words "not a date".
    if (takingsFor !== day) {
      takings = null;
      made = null;
      takingsFor = day;
    }
    if (Number.isNaN(start.getTime())) {
      refuse(t('admin.not_a_date'));
      return;
    }
    const end = new Date(start);
    end.setDate(end.getDate() + 1);
    const reply = await attempt(
      () =>
        admin(
          { what: 'day', from_ms: start.getTime(), to_ms: end.getTime() - 1 },
          Date.now(),
        ),
      null,
    );
    takings = reply ? (reply.info?.day ?? null) : null;
    // The same day, asked the other way: what was made on it. Asked together
    // because an owner reading one wants the other, and two buttons for one
    // day is two chances to compare figures from different days.
    const second = await attempt(
      () =>
        admin(
          { what: 'made', from_ms: start.getTime(), to_ms: end.getTime() - 1 },
          Date.now(),
        ),
      null,
      true,
    );
    made = second?.info?.made ?? null;
  }
</script>

  <section>
    <h2>{t('admin.what_you_took')}</h2>
    <div class="row">
      <input type="date" bind:value={day} disabled={busy} />
      <button onclick={ask} disabled={busy}>{t('admin.look')}</button>
    </div>
    {#if takings}
      {#if takings.sales === 0}
        <p class="why">{t('admin.nothing_rung_that_day')}</p>
      {:else}
        <p class="figure">{money(takings.total_minor)}</p>
        <p class="why">
          {t('admin.sales_of', { count: takings.sales })}
          {#if takings.refunds > 0}
            &middot; {t('admin.including_refunds', {
              count: takings.refunds,
              amount: money(-takings.refunded_minor),
            })}
          {/if}
        </p>
        {#if made && (made.sales > 0 || made.sales_without_cost > 0)}
          <p class="why">
            <strong>{t('admin.made_amount', { amount: money(made.made_minor) })}</strong>
            {t('admin.made_why', {
              net: money(made.net_minor),
              cost: money(made.cost_minor),
              count: made.sales,
            })}
          </p>
          {#if made.sales_without_cost > 0}
            <p class="why">
              <span class="late">
                {t('admin.sales_without_cost', {
                  count: made.sales_without_cost,
                  amount: money(made.net_without_cost_minor),
                })}
              </span>
              {t('admin.put_what_you_pay')}
            </p>
          {/if}
        {/if}
        <p class="why">
          {#if takings.drawers_counted > 0}
            {t('admin.drawers_counted_count', { count: takings.drawers_counted })}
            &middot; {t('admin.expected_amount', { amount: money(takings.expected_cash_minor) })}
            &middot; {t('admin.counted_amount', { amount: money(takings.counted_cash_minor) })}
            {#if takings.variance_minor !== 0}
              &middot; <span class="late">
                {takings.variance_minor < 0
                  ? t('admin.short_by_short', {
                      amount: money(Math.abs(takings.variance_minor)),
                    })
                  : t('admin.over_by_short', {
                      amount: money(Math.abs(takings.variance_minor)),
                    })}
              </span>
            {/if}
          {:else}
            {t('admin.no_drawer_that_day')}
          {/if}
        </p>
        {#if takings.drawers_counted > 0}
          <p class="why">{t('admin.drawers_stay_as_counted')}</p>
        {/if}
        {#if takings.charged_minor !== 0 || takings.paid_minor !== 0 || takings.written_off_minor !== 0 || takings.returned_minor !== 0}
          <p class="why">
            {t('admin.went_on_account', { amount: money(takings.charged_minor) })}
            {#if takings.returned_minor !== 0}
              &middot; {t('admin.came_back', { amount: money(takings.returned_minor) })}
            {/if}
            &middot; {t('admin.was_paid_off', { amount: money(takings.paid_minor) })}
            {#if takings.written_off_minor !== 0}
              &middot; <span class="late">
                {t('admin.struck_off_amount', { amount: money(takings.written_off_minor) })}
              </span>
            {/if}
          </p>
        {/if}
        <ul class="found">
          {#each takings.tills as one (one.terminal)}
            <li>
              <span class="name">
                {tills.find((till) => till.id === one.terminal)?.label ??
                t('admin.a_till_not_listed_caps')}
              </span>
              <span class="detail">
                {t('admin.sales_of', { count: one.sales })} &middot; {money(one.total_minor)}
                {#if one.needing_attention > 0}
                  &middot; <span class="late">
                    {t('admin.needing_a_look', { count: one.needing_attention })}
                  </span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
        <!-- And the same period by whoever rang it. A till says which counter,
             and one counter is stood at by three people in a day: "how much did
             Rina take" is not answered by a figure per machine. Only since a
             sale started recording who rang it, so the sales from before that
             are one row saying so rather than a blank name or a figure quietly
             missing from the list. -->
        {#if takings.people?.length > 0}
          <h3>{t('admin.who_rang_it')}</h3>
          <ul class="found">
            {#each takings.people as one, at (at)}
              <li>
                <span class="name">
                  {one.name || t('admin.nobody_was_recorded')}
                </span>
                <span class="detail">
                  {t('admin.sales_of', { count: one.sales })} &middot; {money(one.total_minor)}
                  {#if one.refunds > 0}
                    &middot; {t('admin.gave_back_of', {
                      count: one.refunds,
                      amount: money(-one.refunded_minor),
                    })}
                  {/if}
                </span>
                {#if !one.name}
                  <span class="detail">{t('admin.rung_before_names_were_kept')}</span>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    {/if}
  </section>
