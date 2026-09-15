<script>
  /// Form মূসক-৬.২, the বিক্রয় হিসাব পুস্তক, as the VAT and SD Rules, 2016
  /// prescribe it.
  ///
  /// The book a shop is asked for when somebody comes to look at its records.
  /// Rule 40(1)(খ) asks a registered person who sells the goods they buy to keep
  /// their sales, with the purchase details of those goods in them, on this
  /// form; rule 41(ক) asks the same of an enlisted person paying turnover tax.
  /// Both describe a shop like the ones this product is for, and neither asks
  /// for money: every column here is a quantity.
  ///
  /// One page per product, which is the form's own shape: পণ্যের নাম stands at
  /// the head and the columns underneath are that product's shelf, day by day.
  ///
  /// The field order, the column order and the wording are the form's own, read
  /// off the Rules as gazetted at `data/research/vat_rules_2016_bn.pdf` page 93.
  /// Its text layer is legacy Bengali encoding, so the page was rendered and
  /// read as an image, the same way the two documents at the till were.
  ///
  /// The form numbers its own columns and says how two of them are arrived at:
  /// মোট is (৮=৩+৭) and সমাপনী জের is (১০=৮-৯). Those numbers are printed here
  /// because they are on the form, and because they are what somebody checking
  /// the page adds up.
  let {
    /// The shop, for the head of the page.
    shop,
    /// Which product this page is about.
    itemName,
    /// What the shelf held when the period opened.
    openingMilli,
    /// The days, already grouped, from `sales_book_rows.js`.
    rows,
    t,
    money,
    qty,
  } = $props();

  import { theBookWithBalances } from './sales_book_rows.js';

  /// The rows with their balances, worked out in the shared module rather than
  /// here: a page that added up differently from the test would be a page
  /// nobody had tested.
  const filled = $derived(theBookWithBalances(openingMilli, rows));

  /// A day as a person reads it, from the key the grouping made.
  function day(key) {
    const [year, month, at] = String(key).split('-');
    return `${at}/${month}/${year}`;
  }
</script>

<section class="mushak book">
  <header>
    <p class="government">{t('invoice.government')}</p>
    <p class="board">{t('invoice.board')}</p>
    <p class="form">{t('book.form_no')}</p>
    <h1>{t('book.title')}</h1>
    <p class="rule">{t('book.subtitle')}</p>
    <p class="rule">{t('book.rule')}</p>
  </header>

  <!-- The shop's own name and BIN. The form does not ask for them, which is
       what a book kept in a shop's own premises does not need to say; a page
       handed over on its own does, and this product prints pages. -->
  <dl class="whose">
    <dt>{t('note.name')}</dt>
    <dd>{shop?.name ?? ''}</dd>
    <dt>{t('note.bin')}</dt>
    <dd>{shop?.bin ?? ''}</dd>
    <dt>{t('book.item_name')}</dt>
    <dd>{itemName}</dd>
  </dl>

  <table>
    <thead>
      <tr>
        <th rowspan="2">{t('book.serial')}</th>
        <th rowspan="2">{t('book.date')}</th>
        <th rowspan="2">{t('book.opening')}</th>
        <th colspan="4">{t('book.came_in')}</th>
        <th rowspan="2">{t('book.total')}</th>
        <th rowspan="2">{t('book.sold')}</th>
        <th rowspan="2">{t('book.closing')}</th>
        <th rowspan="2">{t('book.remark')}</th>
      </tr>
      <tr>
        <th>{t('book.challan_no')}</th>
        <th>{t('book.date')}</th>
        <th>{t('book.seller')}</th>
        <th>{t('book.quantity')}</th>
      </tr>
      <tr class="numbers">
        <td>(১)</td>
        <td>(২)</td>
        <td>(৩)</td>
        <td>(৪)</td>
        <td>(৫)</td>
        <td>(৬)</td>
        <td>(৭)</td>
        <td>(৮=৩+৭)</td>
        <td>(৯)</td>
        <td>(১০=৮-৯)</td>
        <td>(১১)</td>
      </tr>
    </thead>
    <tbody>
      {#each filled as row, at (row.day)}
        <!-- A day with two deliveries has two invoice numbers to show, and the
             form gives one row one set of purchase columns. So the day's other
             figures sit on its first row, where somebody reading the closing
             column down the page sees one figure per day. -->
        {#each row.cameIn.length > 0 ? row.cameIn : [null] as arrival, n (n)}
          <tr>
            <td>{at + 1}{#if n > 0}.{n + 1}{/if}</td>
            <td>{n === 0 ? day(row.day) : ''}</td>
            <td>{n === 0 ? qty(row.openingMilli) : ''}</td>
            <td>{arrival?.reference ?? ''}</td>
            <td>{arrival ? new Date(arrival.atMs).toLocaleDateString('en-GB') : ''}</td>
            <!-- The name and the BIN are one column on the form, and the
                 separator between them needs the space either side that the
                 markup's own indentation ate: it read "Rahman Wholesale·
                 123456789-0202" on the first page this printed. -->
            <td>
              {#if arrival}
                {arrival.supplierBin
                  ? `${arrival.supplierName} · ${arrival.supplierBin}`
                  : arrival.supplierName}
              {/if}
            </td>
            <td>{arrival ? qty(arrival.qtyMilli) : ''}</td>
            <td>{n === 0 ? qty(row.totalMilli) : ''}</td>
            <td>{n === 0 ? qty(row.soldMilli) : ''}</td>
            <td>{n === 0 ? qty(row.closingMilli) : ''}</td>
            <td>
              <!-- What moved the shelf without being bought or sold. The form
                   has no column for it and leaves a remark column, which is
                   where the truth about it goes rather than nowhere. -->
              {#if n === 0 && row.correctedMilli !== 0}
                {t('book.corrected', { amount: qty(Math.abs(row.correctedMilli)) })}
              {/if}
            </td>
          </tr>
        {/each}
      {/each}
    </tbody>
  </table>

  {#if filled.length === 0}
    <!-- A month in which nothing moved is an answer, and the opening figure is
         still the shop's own. An empty table with no word under it reads as a
         question that failed. -->
    <p class="footnote">{t('book.nothing_moved', { amount: qty(openingMilli) })}</p>
  {/if}
</section>
