<script>
  // Form মূসক-৬.২, laid out where the till's two documents are laid out.
  import SalesBook from '../../../shared/sales_book.svelte';
  import { theBookByDay } from '../../../shared/sales_book_rows.js';
  import { today, thisMonth } from '../../../shared/days.js';

  /// One item's page of the sales book.
  ///
  /// Rule 40(1)(খ) of the VAT and SD Rules, 2016 asks a registered person who
  /// sells the goods they buy — which is a shop like this one — to keep their
  /// sales, with the purchase details in them, on form মূসক-৬.২. Rule 41(ক) asks
  /// the same of an enlisted person paying turnover tax. It is kept per product,
  /// so this screen asks about one at a time and prints one page.
  ///
  /// The whole page is read from the movements the shop already holds. Nothing
  /// here is a second set of books: it is the same shelf arithmetic every other
  /// stock screen reads, laid out on the form somebody is asked for.
  let {
    t,
    money,
    qty,
    /// The shop's own name and BIN, for the head of the page.
    shop,
    /// The catalogue this device holds, as id to name, for picking an item and
    /// for naming it on the form.
    names,
    busy,
    attempt,
    admin,
    refuse,
    announce,
  } = $props();

  /// Which item the page is about, and what somebody typed to find it.
  let hunting = $state('');
  let item = $state('');
  /// The month the page covers. One month, because that is the period the Act
  /// counts in: a tax period is one month of the Christian calendar.
  let month = $state(thisMonth());
  /// The page itself, once the shop has answered.
  let page = $state(null);

  /// Items matching what somebody typed, a few at a time.
  ///
  /// From the catalogue this device already holds rather than a search on the
  /// shop, because the whole list is here and a book is looked up by a person
  /// who knows roughly what the thing is called.
  const found = $derived.by(() => {
    const asked = hunting.trim().toLowerCase();
    if (!asked) return [];
    return Object.entries(names)
      .filter(([, name]) => name.toLowerCase().includes(asked))
      .slice(0, 8);
  });

  /// The page, grouped into days the way the form reads.
  const rows = $derived(page ? theBookByDay(page.moved) : []);

  async function askForThePage() {
    if (!item) {
      refuse(t('admin.pick_an_item_for_the_book'));
      return;
    }
    const start = new Date(`${month}-01T00:00:00`);
    if (Number.isNaN(start.getTime())) {
      refuse(t('admin.not_a_month'));
      return;
    }
    const end = new Date(start);
    end.setMonth(end.getMonth() + 1);
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'stock_book',
            item,
            from_ms: start.getTime(),
            to_ms: end.getTime() - 1,
          },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    page = {
      opening_milli: reply.info?.book_opening_milli ?? 0,
      moved: reply.info?.book_moved ?? [],
      from: start,
    };
    if (page.moved.length === 0) {
      // A quiet month is an answer, and the page still has an opening balance
      // on it. Said out loud so nobody reads an empty table as a failed
      // question.
      announce(t('admin.nothing_moved_that_month'));
    }
  }

  async function printThePage() {
    if (!page) return;
    // The browser needs the page laid out before it is asked to print it, the
    // same wait the two documents at the till use.
    await new Promise((settle) => setTimeout(settle, 50));
    window.print();
  }
</script>

<section>
  <h2>{t('admin.the_sales_book')}</h2>
  <p class="why">{t('admin.sales_book_why')}</p>

  <div class="row">
    <input
      bind:value={hunting}
      placeholder={t('admin.hunt_placeholder')}
      disabled={busy}
    />
    <input type="month" bind:value={month} disabled={busy} />
    <button onclick={askForThePage} disabled={busy || !item}>
      {t('admin.read_the_book')}
    </button>
  </div>

  {#if found.length > 0}
    <ul class="found">
      {#each found as [id, name] (id)}
        <li>
          <span class="detail">
            <button
              class={item === id ? '' : 'quiet'}
              onclick={() => { item = id; hunting = name; }}
              disabled={busy}
            >
              {name}
            </button>
          </span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if page}
    <div class="row">
      <button onclick={printThePage} disabled={busy}>{t('admin.print_the_book')}</button>
    </div>
    <SalesBook
      {shop}
      itemName={names[item] ?? ''}
      openingMilli={page.opening_milli}
      {rows}
      {t}
      {money}
      {qty}
    />
  {/if}
</section>
