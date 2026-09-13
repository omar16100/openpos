<script>
  /// What the tills' cash drawers are doing: the ones standing open now, and
  /// the ones somebody has counted and closed.
  ///
  /// The first panel to live in its own file. What it owns is its own two
  /// lists and the two questions that fill them; what it is given is the way
  /// to ask the shop anything (`attempt` and `admin`, which between them carry
  /// this screen's one message line and its busy flag), the words, the money,
  /// and the tills, because a drawer is named by the till it belongs to and
  /// that list is read by half the screen.
  ///
  /// The look comes from screen.css rather than from a `<style>` block here.
  /// A component's styles are scoped to its own markup, so a panel with its
  /// own block would be a panel that quietly lost every rule the rest of the
  /// screen is drawn with.
  let { t, money, attempt, admin, tills } = $props();

  // Drawers counted and closed. The point of counting one is that somebody who
  // was not standing at the till reconciles it afterwards.
  let drawers = $state([]);
  // Drawers standing open right now, as each till last said. A drawer left open
  // overnight used to be invisible until somebody looked at the till itself.
  let openDrawers = $state([]);

  /// Both questions, for whoever is loading the screen.
  ///
  /// Exported rather than run on mount: the screen loads its panels in an order
  /// it decides, and a panel that fetched for itself as soon as it appeared
  /// would put a dozen requests on the wire the moment a device enrolled.
  export async function counted(quiet = true) {
    const reply = await attempt(() => admin({ what: 'shifts', limit: 20 }, Date.now()), null, quiet);
    if (reply) drawers = reply.info?.shifts ?? [];
  }

  export async function open(quiet = true) {
    const reply = await attempt(() => admin({ what: 'open_drawers' }, Date.now()), null, quiet);
    if (reply) openDrawers = reply.info?.open_drawers ?? [];
  }

  /// The till a drawer belongs to, by name.
  function whose(drawer) {
    return tills.find((till) => till.id === drawer.terminal)?.label ?? t('admin.a_till_not_listed_caps');
  }
</script>

<section>
  <h2>{t('admin.drawers_open_now')}</h2>
  <p class="why">{t('admin.open_drawers_why')}</p>
  {#if openDrawers.length > 0}
    <ul class="found">
      {#each openDrawers as drawer (drawer.terminal)}
        <li>
          <span class="name">{whose(drawer)}</span>
          <span class="detail">
            {t('admin.open_since', {
              at: new Date(drawer.opened_at_ms).toLocaleString('en-GB'),
            })}
            &middot; {t('admin.sales_of', { count: drawer.sales })}
            &middot; {t('admin.should_hold_amount', {
              amount: money(drawer.expected_cash_minor),
            })}
          </span>
          <span class="detail">
            {t('admin.as_that_till_said', {
              at: new Date(drawer.reported_at_ms).toLocaleString('en-GB'),
            })}
          </span>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="why">{t('admin.no_drawer_open')}</p>
  {/if}
</section>

<section>
  <h2>{t('admin.drawers_counted')}</h2>
  <p class="why">{t('admin.drawers_why')}</p>
  {#if drawers.length > 0}
    <ul class="found">
      {#each drawers as drawer (drawer.id)}
        <li class:retired={drawer.variance_minor !== 0}>
          <span class="name">
            {whose(drawer)}
            &middot; {new Date(drawer.closed_at_ms).toLocaleString('en-GB')}
            {#if drawer.closed_by_name}
              &middot; {t('admin.counted_by', { name: drawer.closed_by_name })}
            {/if}
          </span>
          <span class="detail">
            {t('admin.drawer_sales', { count: drawer.sales })}
            &middot; {t('admin.drawer_float', { amount: money(drawer.opening_float_minor) })}
            &middot; {t('admin.expected_amount', { amount: money(drawer.expected_cash_minor) })}
            &middot; {t('admin.counted_amount', { amount: money(drawer.counted_cash_minor) })}
            <!-- Money that crossed the drawer for a reason rather than for
                 goods, and only when there was any. Both figures came to this
                 screen from the day it was written and neither was shown, so a
                 drawer that came up short read the same whether somebody had
                 taken money out of it for a stated reason or not. The expected
                 figure already accounts for it, which is exactly why the row
                 has to say so: otherwise the arithmetic on the screen cannot be
                 followed without the till's own paper. -->
            {#if drawer.cash_in_minor}
              &middot; {t('admin.drawer_cash_in', { amount: money(drawer.cash_in_minor) })}
            {/if}
            {#if drawer.cash_out_minor}
              &middot; {t('admin.drawer_cash_out', { amount: money(drawer.cash_out_minor) })}
            {/if}
          </span>
          <!-- What came back while it was open, from the shop's own sales
               rather than from the till's word, which is where every other
               cross-check on this row comes from. The cash above is already net
               of it, so a drawer short against a day's selling would otherwise
               read the same whether goods came back or not. -->
          {#if drawer.refunds > 0}
            <span class="detail">
              {t('admin.drawer_refunds', {
                count: drawer.refunds,
                amount: money(drawer.refunded_cash_minor),
              })}
            </span>
          {/if}
          <span class="detail">
            {#if drawer.variance_minor === 0}
              {t('admin.counted_exactly')}
            {:else if drawer.variance_minor < 0}
              <span class="late">
                {t('admin.short_by', { amount: money(-drawer.variance_minor) })}
              </span>
            {:else}
              <span class="late">
                {t('admin.over_by', { amount: money(drawer.variance_minor) })}
              </span>
            {/if}
          </span>
          {#if drawer.expected_from_sales_minor !== null && drawer.expected_from_sales_minor !== undefined && drawer.expected_from_sales_minor !== drawer.expected_cash_minor}
            <span class="detail">
              <span class="late">
                {t('admin.sales_disagree', {
                  from_sales: money(drawer.expected_from_sales_minor),
                  expected: money(drawer.expected_cash_minor),
                })}
              </span>
              {#if drawer.struck_out_cash_minor}
                <!-- Named before the general advice, because when it is here it
                     is usually the whole of the difference and the advice above
                     would send somebody to ask a cashier about it.

                     Negated on purpose. The shop carries the cash those sales
                     moved, which is a fact about them; what is being explained
                     here is the difference between two figures, and taking a
                     sale out of one of them moves it the other way. A
                     struck-out refund of 57.50 makes the shop's figure 57.50
                     higher than the till's. -->
                {t('admin.struck_out_explains', {
                  amount: money(-drawer.struck_out_cash_minor),
                })}
              {/if}
              {t('admin.sales_disagree_why')}
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <p class="why">{t('admin.no_drawer_counted_yet')}</p>
  {/if}
</section>
