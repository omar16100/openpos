<script>
  /// The Mushak 6.3 tax invoice, as the National Board of Revenue prescribes it.
  ///
  /// A different document from the receipt beside it, and deliberately so. The
  /// counter receipt is 32 characters of thermal paper in English, because no
  /// ESC/POS code page carries Bengali and because it is read at a counter by
  /// somebody who is leaving. This is A4, in Bengali, and is read afterwards by
  /// a buyer claiming their input tax credit and by whoever audits them.
  ///
  /// The field order, the column order and the wording are the form's own, read
  /// off nbr.gov.bd/uploads/form/Mushak_6.3_.pdf. Nothing here is arranged to
  /// taste: a form filled in a different order is a form somebody has to argue
  /// about.
  ///
  /// What this cannot do is stated on the screen that offers it rather than
  /// hidden here. Supplementary duty has a column and this till cannot express
  /// one, so the column prints nothing at all rather than a nought: a nought is
  /// a claim that none was due, and a shop selling goods that carry one would be
  /// making it without knowing.
  let {
    /// The shop, as its own record has it.
    shop,
    /// The sale: the view's lines and totals, which the core priced.
    view,
    /// Who it is for, when the sale names somebody the shop wrote down.
    buyer,
    /// The receipt number this sale took, and when it was rung.
    receiptNo,
    rungAt,
    /// What this screen says, in the shop's language, for everything that is
    /// not the form itself.
    t,
    /// How this app writes money and quantities. Handed in rather than imported
    /// because this file is shared by the till and the back office, and each
    /// has its own: a document that formatted its own figures would be a second
    /// opinion about what 1,500.50 looks like.
    money,
    qty,
  } = $props();

  /// The rate as the form wants it: a percentage, or the words for a supply
  /// that carries no rate at all.
  ///
  /// Zero rated and exempt are different answers to the same column and the
  /// return puts them in different places, so "0%" for both would be saying
  /// neither.
  function rate(line) {
    if (line.supply === 1) return t('invoice.zero_rated');
    if (line.supply === 2) return t('invoice.exempt');
    return `${(line.vat_bp / 100).toString()}%`;
  }
</script>

<!-- Printed rather than shown: the screen behind it is the till, and this is a
     sheet of paper somebody is handed. The stylesheet in screen.css hides
     everything else when this exists. -->
<section class="mushak">
  <header>
    <p class="government">{t('invoice.government')}</p>
    <p class="board">{t('invoice.board')}</p>
    <p class="form">{t('invoice.form_no')}</p>
    <h1>{t('invoice.title')}</h1>
    <p class="rule">{t('invoice.rule')}</p>
  </header>

  <dl class="seller">
    <dt>{t('invoice.seller_name')}</dt>
    <dd>{shop?.name ?? ''}</dd>
    <dt>{t('invoice.seller_bin')}</dt>
    <dd>{shop?.bin ?? ''}</dd>
    <dt>{t('invoice.issued_from')}</dt>
    <dd>{shop?.address ?? ''}</dd>
  </dl>

  <div class="parties">
    <dl class="buyer">
      <dt>{t('invoice.buyer_name')}</dt>
      <dd>{buyer?.name ?? ''}</dd>
      <dt>{t('invoice.buyer_bin')}</dt>
      <dd>{buyer?.bin ?? ''}</dd>
      <!-- Where the supply is going, which the form asks for and which is not
           the same question as where the buyer lives. This shop holds an
           address and nothing about a destination, so what is written here is
           the buyer's address when there is one and a blank when there is not:
           a line for somebody to fill in by hand, which is what the form is
           for. -->
      <dt>{t('invoice.destination')}</dt>
      <dd>{buyer?.address ?? ''}</dd>
    </dl>
    <dl class="issued">
      <dt>{t('invoice.number')}</dt>
      <dd>{receiptNo ?? ''}</dd>
      <dt>{t('invoice.issued_on')}</dt>
      <dd>{rungAt.toLocaleDateString('en-GB')}</dd>
      <dt>{t('invoice.issued_at')}</dt>
      <dd>{rungAt.toLocaleTimeString('en-GB')}</dd>
    </dl>
  </div>

  <table>
    <thead>
      <tr>
        <th>{t('invoice.serial')}</th>
        <th>{t('invoice.description')}</th>
        <th>{t('invoice.unit')}</th>
        <th>{t('invoice.quantity')}</th>
        <th>{t('invoice.unit_price')}<sup>1</sup></th>
        <th>{t('invoice.line_value')}<sup>1</sup></th>
        <th>{t('invoice.duty')}</th>
        <th>{t('invoice.vat_rate')}</th>
        <th>{t('invoice.vat_amount')}</th>
        <th>{t('invoice.with_tax')}</th>
      </tr>
    </thead>
    <tbody>
      {#each view?.lines ?? [] as line, at (at)}
        <tr>
          <td>{at + 1}</td>
          <td>{line.name}</td>
          <td>{line.unit}</td>
          <td>{qty(line.qty_milli)}</td>
          <td>{money(line.unit_price_minor)}</td>
          <!-- The value excluding every tax, which is what the footnote on the
               form says this column and the one before it are. Taken from the
               crate that priced the sale rather than worked out here: where a
               shop prices inclusive of tax, it is not the total less a rate. -->
          <td>{money(line.net_minor)}</td>
          <td></td>
          <td>{rate(line)}</td>
          <td>{money(line.vat_minor)}</td>
          <td>{money(line.net_minor + line.vat_minor)}</td>
        </tr>
      {/each}
      <tr class="sum">
        <td colspan="5">{t('invoice.grand_total')}</td>
        <td>{money(view?.net_minor ?? 0)}</td>
        <td></td>
        <td></td>
        <td>{money(view?.vat_minor ?? 0)}</td>
        <td>{money(view?.total_minor ?? 0)}</td>
      </tr>
    </tbody>
  </table>
  <p class="footnote"><sup>1</sup> {t('invoice.price_note')}</p>

  <!-- Blank, and blank on purpose. The form asks for the name, designation,
       signature and seal of the person responsible for the establishment, and
       three of those four are made by a hand holding a pen. Printing a cashier's
       name into the first would be this till claiming somebody signed. -->
  <dl class="signed">
    <dt>{t('invoice.officer_name')}</dt>
    <dd></dd>
    <dt>{t('invoice.designation')}</dt>
    <dd></dd>
    <dt>{t('invoice.signature')}</dt>
    <dd></dd>
    <dt>{t('invoice.seal')}</dt>
    <dd></dd>
  </dl>
</section>
