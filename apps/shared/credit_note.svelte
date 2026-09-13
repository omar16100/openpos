<script>
  /// Form মূসক-৬.৭, the credit note, as the VAT and SD Rules, 2016 prescribe it.
  ///
  /// The paper for goods coming back, and a different document from the tax
  /// invoice beside it. Section 52 of the Act lists what a credit note must
  /// carry and rule 40(1)(ছ) puts it on this form; section 52(2) is the sharp
  /// end of it: without the buyer's details a note cannot be used to claim the
  /// decreasing adjustment at all, and the buyer has already taken the credit
  /// on the invoice this reverses.
  ///
  /// The field order, the column order and the wording are the form's own, read
  /// off the Rules as gazetted at `data/research/vat_rules_2016_bn.pdf` page 98.
  /// Its text layer is legacy Bengali encoding, so the page was rendered and
  /// read as an image, the same way the invoice form was.
  ///
  /// Two things on it this product cannot fill and does not pretend to. The
  /// supplementary duty line is left empty, for the reason the invoice leaves
  /// its column empty: a shop that buys goods in and sells them on is none of
  /// the three persons section 56 makes liable for it, and a nought there would
  /// be a claim that none was due. The signature is made by a hand holding a
  /// pen.
  ///
  /// Every figure is printed as a size. A refund's arithmetic runs below
  /// nothing, and this document is about a return in the first place: the form
  /// says ফেরত in four places, and a page of minus signs would be saying it
  /// twice and contradicting itself once.
  let {
    /// The shop, as its own record has it: the person giving the refund.
    shop,
    /// The refund: the view's lines and totals, which the core priced.
    view,
    /// Who the goods came back from, when the sale names somebody the shop
    /// wrote down. Required by section 52(1)(f) once the VAT being given back
    /// is more than 5,000 taka, and the screen offering this is what enforces
    /// that.
    buyer,
    /// This note's own number and moment: the refund's receipt number, and when
    /// it was rung.
    noteNo,
    issuedAt,
    /// The invoice being adjusted: its number, and the day it was issued.
    ///
    /// Both are blank when the customer could not produce the receipt, which is
    /// a real thing that happens at a counter and is not a reason to refuse
    /// somebody their money. A blank line on a form is a line somebody fills in
    /// by hand; a wrong one is not.
    originalNo,
    originalAt,
    /// Why the goods came back, in the words of whoever is printing it. Section
    /// 52(1)(d) asks for the nature of the adjustment and the form gives it a
    /// box: ফেরতের কারণ.
    reason,
    /// What this screen says, in the shop's language, for everything that is
    /// not the form itself.
    t,
    /// How this app writes money and quantities. Handed in rather than imported
    /// because this file is shared by the till and the back office, and each has
    /// its own.
    money,
    qty,
  } = $props();

  /// The figures, as sizes. See the note above about minus signs.
  const size = (amount) => Math.abs(Number(amount) || 0);
</script>

<!-- Printed rather than shown, like the invoice: the stylesheet hides
     everything else when this exists. -->
<section class="mushak">
  <header>
    <p class="government">{t('invoice.government')}</p>
    <p class="board">{t('invoice.board')}</p>
    <p class="form">{t('note.form_no')}</p>
    <h1>{t('note.title')}</h1>
    <p class="rule">{t('note.rule')}</p>
  </header>

  <div class="parties">
    <dl class="seller">
      <dt class="party">{t('note.refunder')}</dt>
      <dd></dd>
      <dt>{t('note.name')}</dt>
      <dd>{shop?.name ?? ''}</dd>
      <dt>{t('note.bin')}</dt>
      <dd>{shop?.bin ?? ''}</dd>
      <dt>{t('note.original_number')}</dt>
      <dd>{originalNo ?? ''}</dd>
      <dt>{t('note.original_date')}</dt>
      <dd>{originalAt ? originalAt.toLocaleDateString('en-GB') : ''}</dd>
    </dl>
    <dl class="buyer">
      <dt class="party">{t('note.receiver')}</dt>
      <dd></dd>
      <dt>{t('note.name')}</dt>
      <dd>{buyer?.name ?? ''}</dd>
      <dt>{t('note.bin')}</dt>
      <dd>{buyer?.bin ?? ''}</dd>
      <dt>{t('note.number')}</dt>
      <dd>{noteNo ?? ''}</dd>
      <dt>{t('note.issued_on')}</dt>
      <dd>{issuedAt.toLocaleDateString('en-GB')}</dd>
      <dt>{t('note.issued_at')}</dt>
      <dd>{issuedAt.toLocaleTimeString('en-GB')}</dd>
    </dl>
  </div>

  <table>
    <thead>
      <tr>
        <th>{t('note.serial')}</th>
        <th>{t('note.description')}</th>
        <th>{t('note.unit')}</th>
        <th>{t('note.quantity')}</th>
        <th>{t('note.unit_price')}<sup>1</sup></th>
        <th>{t('note.line_value')}</th>
      </tr>
    </thead>
    <tbody>
      {#each view?.lines ?? [] as line, at (at)}
        <tr>
          <td>{at + 1}</td>
          <td>{line.name}</td>
          <td>{line.unit}</td>
          <td>{qty(size(line.qty_milli))}</td>
          <!-- One unit with the tax in it, which is what this form's own
               footnote asks for and is not what the invoice's column of the
               same name asks for. Worked out by the crate that priced the
               sale. -->
          <td>{money(size(line.unit_with_tax_minor))}</td>
          <!-- What the line came to, tax and all. Added rather than read from a
               field, because the two screens that print this hand it two
               shapes: a till's own view calls it `total_minor` and a sale
               looked up from the shop calls it `line_total_minor`. Both carry
               the net and the tax, and the invoice beside this adds the same
               two for the same reason. -->
          <td>{money(size(line.net_minor + line.vat_minor))}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <dl class="sums">
    <dt>{t('note.total_value')}</dt>
    <dd>{money(size(view?.total_minor))}</dd>
    <!-- Anything kept back out of the refund. This till hands back what the
         goods came to and nothing else, so the line is empty rather than a
         nought: empty is what a form asks somebody to fill in, and a nought is
         a claim. -->
    <dt>{t('note.deduction')}<sup>2</sup></dt>
    <dd></dd>
    <dt>{t('note.value_with_vat')}</dt>
    <dd>{money(size(view?.total_minor))}</dd>
    <dt>{t('note.vat_amount')}</dt>
    <dd>{money(size(view?.vat_minor))}</dd>
    <!-- Empty for the reason the invoice's duty column is empty. -->
    <dt>{t('note.sd_amount')}</dt>
    <dd></dd>
    <dt>{t('note.total_tax')}<sup>3</sup></dt>
    <dd>{money(size(view?.vat_minor))}</dd>
  </dl>

  <div class="reason">
    <p class="label">{t('note.reason')}</p>
    <p class="box">{reason ?? ''}</p>
  </div>

  <!-- Blank on purpose, like the invoice's: a signature is made by a hand. -->
  <p class="signed">{t('note.signature')}</p>

  <p class="footnote"><sup>1</sup> {t('note.price_note')}</p>
  <p class="footnote"><sup>2</sup> {t('note.deduction_note')}</p>
  <p class="footnote"><sup>3</sup> {t('note.tax_note')}</p>
</section>
