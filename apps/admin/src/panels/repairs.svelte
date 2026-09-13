<script>
  /// Everything that went wrong and needs a person.
  ///
  /// Seven sections and one job: a receipt somebody has brought to the counter,
  /// the sales the shop could not take on trust, the answers already given, the
  /// gaps in the numbering, the sales carried in by hand off a device that
  /// cannot send, the items a till wrote down at a counter, and the catalogue
  /// changes no till could read. A shop works this list on a quiet afternoon,
  /// and every one of them ends with somebody saying what happened.
  ///
  /// `names` is the catalogue this device holds, because a queue that says a
  /// quantity and not a thing is a queue nobody can act on.
  let {
    t,
    money,
    qty,
    busy,
    attempt,
    admin,
    bundleMark,
    names,
    tills,
    /// Open the catalogue form on an item a till wrote down. The form belongs
    /// to the screen, because correcting an item is the same act wherever it
    /// is started from.
    onCorrect,
    announce,
    refuse,
  } = $props();

  /// What the shop could not take on trust, and what has already been answered.
  let repairs = $state([]);
  let decided = $state([]);
  let showDecided = $state(false);
  /// Why each was answered the way it was. The server requires a note and so
  /// does sense: the queue is worked months before anybody asks why a total was
  /// wrong, and an entry that disappears without one leaves that unanswerable.
  let notes = $state({});
  /// A receipt somebody has brought back, and what the shop says was on it.
  let receiptAsked = $state('');
  let receiptLookedFor = $state('');
  let onPaper = $state([]);
  /// A bundle read off a device that cannot send, and what it hashes to.
  let carried = $state('');
  let carriedMark = $state('');
  /// Where the numbering jumps, the catalogue rows no till could read, and the
  /// items a till wrote down that nobody has agreed to.
  let gaps = $state([]);
  let unreadable = $state([]);
  let fromTills = $state([]);

  /// Everything this panel shows, for whoever is loading the screen.
  export async function queue(quiet = true) {
    await listRepairs(quiet);
  }

  export async function alsoTheRest() {
    await listUnreadable();
    await listFromTills();
    await listGaps();
  }

  /// Asked again after the shop has been told to say its whole list: the rows
  /// that could not be read are the ones just sent, so the list either empties
  /// or says which are still beyond this build.
  export async function readAgain() {
    await listUnreadable();
  }

  async function listRepairs(quiet = true) {
    const reply = await attempt(() => admin({ what: 'repairs', limit: 50 }, Date.now()), null, quiet);
    if (reply) repairs = reply.info?.repairs ?? [];
  }

  /// What has already been answered, which is the only way back to a wrong
  /// answer: an entry that has been decided is out of the queue.
  async function listDecided(quiet = true) {
    const reply = await attempt(() => admin({ what: 'decided', limit: 50 }, Date.now()), null, quiet);
    if (reply) decided = reply.info?.decided ?? [];
  }

  /// Where the numbering jumps.
  ///
  /// Asked with the till list, because reading a gap needs the other half: a
  /// gap on a till that synced an hour ago is one thing, and a gap on a till
  /// nobody has heard from since Tuesday is another.
  async function listGaps(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'receipt_gaps', limit: 50 }, Date.now()),
      null,
      quiet,
    );
    if (reply) gaps = reply.info?.gaps ?? [];
  }

  async function listUnreadable(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'unreadable_changes', limit: 200 }, Date.now()),
      null,
      quiet,
    );
    if (reply) unreadable = reply.info?.unreadable ?? [];
  }

  /// Items a till wrote down at a counter, for somebody to look at.
  ///
  /// A price typed to get a queue moving is not a price the shop set, and the
  /// only thing that makes it one is somebody here saying so.
  async function listFromTills(quiet = true) {
    const reply = await attempt(
      () => admin({ what: 'items_from_tills', limit: 200 }, Date.now()),
      null,
      quiet,
    );
    if (reply) fromTills = reply.info?.from_tills ?? [];
  }

  /// What a refusal was about, said in this screen's language first.
  ///
  /// A gap in time arrives as a count, a unit and a direction, and all three
  /// would otherwise be poured into a Bangla sentence as English: "1 hours
  /// after". The direction picks the sentence, and the unit picks a phrase.
  function heldParts(entry) {
    const parts = { ...(entry.parts ?? {}) };
    if (parts.unit && parts.how_far !== undefined) {
      const many = Number(parts.how_far) !== 1;
      const unit = parts.unit.replace(/s$/, '');
      parts.how_far =
        unit === 'moment'
          ? t('unit.moment')
          : t(`unit.${unit}${many ? 's' : ''}`, { count: parts.how_far });
      delete parts.unit;
    }
    // An item arrives as its id, because the shop has the id and this device
    // has the names: a queue that says a quantity and not a thing is a queue
    // nobody can act on.
    if (parts.item) parts.item = names[parts.item] ?? t('admin.something_unnamed');
    return parts;
  }

  /// What was on a receipt somebody has brought back to the counter.
  ///
  /// The question a shop is actually asked: "you charged me twice", "I did not
  /// take this". Everything else here answers what went wrong or what was
  /// taken; nothing answered what was on this piece of paper.
  async function findReceipt() {
    const asked = receiptAsked.trim();
    if (!asked) {
      refuse(t('admin.say_receipt_number'));
      return;
    }
    const reply = await attempt(() => admin({ what: 'receipt', receipt_no: asked }, Date.now()));
    if (!reply) return;
    onPaper = reply.info?.on_paper ?? [];
    receiptLookedFor = asked;
    if (onPaper.length === 0) {
      announce(t('admin.no_such_receipt', { number: asked }));
    }
  }

  /// Say what was decided about one of them.
  ///
  /// A note is required by the server and by sense: the queue is worked months
  /// before anybody asks why a total was wrong, and an entry that disappears
  /// without one leaves that question unanswerable.
  async function resolve(entry, kept) {
    const note = (notes[entry.id] ?? '').trim();
    if (!note) {
      refuse(t('admin.say_what_decided'));
      return;
    }
    const reply = await attempt(
      () => admin({ what: 'resolve_repair', sale: entry.id, note, kept }, Date.now()),
      kept ? t('admin.kept_counts') : t('admin.struck_out_removed'),
    );
    if (!reply) return;
    if (reply.info?.already_resolved) {
      announce(t('admin.already_dealt_with'));
    }
    notes = { ...notes, [entry.id]: '' };
    await listRepairs();
    if (showDecided) await listDecided();
  }

  /// Change an answer. A separate act with its own note, because a strike-out
  /// took a real debt off somebody's account and getting it back has to be
  /// something a person chose to do.
  async function changeAnswer(entry, kept) {
    const note = (notes[entry.id] ?? '').trim();
    if (!note) {
      refuse(t('admin.say_why_changing'));
      return;
    }
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'decide_again',
            sale: entry.id,
            note,
            kept,
            // What this screen saw. If somebody else answered in the meantime
            // the server refuses rather than letting a stale view win.
            expected_decisions: entry.decisions,
          },
          Date.now(),
        ),
      kept ? t('admin.put_back_counts') : t('admin.struck_out_removed'),
    );
    if (!reply) return;
    if (reply.info?.decision_stale) {
      announce(t('admin.somebody_else_answered'));
    } else if (!reply.info?.decision_changed) {
      announce(t('admin.nobody_answered'));
    }
    notes = { ...notes, [entry.id]: '' };
    await listDecided();
    await listRepairs();
  }

  /// Read a bundle out of a file the till wrote.
  ///
  /// The two devices are usually not the same one, and the bundle is thousands
  /// of characters: a file goes on a memory stick or through an email, where
  /// selecting text on a tablet screen does not.
  async function openCarriedFile(event) {
    const file = event.currentTarget.files?.[0];
    if (!file) return;
    carried = await file.text();
    await markCarried();
    // Cleared so the same file can be chosen again after a failed attempt.
    event.currentTarget.value = '';
  }

  /// What the paste hashes to, worked out by the same code that marked it on the
  /// device it came from. A mark that differs is a paste that got cut short,
  /// which otherwise looks exactly like one that did not.
  async function markCarried() {
    const text = carried.trim();
    if (!text) {
      carriedMark = '';
      return;
    }
    const reply = await attempt(() => bundleMark(text), null, true);
    carriedMark = reply?.info?.mark ?? '';
  }

  /// Take in sales carried from a device that could not send them.
  ///
  /// The only way a shop gets the takings off a till whose terminal was deleted,
  /// or one that has to be enrolled again as another. Every one of them lands in
  /// the queue below, because the credential that would ordinarily say where a
  /// sale came from is exactly what such a device has lost.
  async function adoptCarried() {
    const bundle = carried.trim();
    if (!bundle) {
      refuse(t('admin.paste_what_till_showed'));
      return;
    }
    const reply = await attempt(
      () => admin({ what: 'adopt_sales', bundle }, Date.now()),
      t('admin.taken_in'),
    );
    if (!reply) return;
    carried = '';
    carriedMark = '';
    // Two different things, and telling a shop the wrong one sends somebody
    // hunting through a queue for an entry that is not there. The server is
    // careful about this and refuses to flag a sale it already had; saying "in
    // the list below" regardless undid that one layer up.
    const took = reply.info?.adopted ?? 0;
    const waiting = reply.info?.adopted_needing_attention ?? 0;
    await listRepairs(true);
    announce(
      waiting
        ? t('admin.adopted_sales', { count: took, waiting })
        : t('admin.adopted_already_had', { count: took }),
    );
  }

  /// Say that what a till wrote down is right, as it stands.
  ///
  /// The same save the item screen does, which is what clears the mark: there
  /// is no second way to agree to an item.
  async function agreeToItem(item) {
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'item',
            // Zero, because agreeing to it is not editing it: whatever the shop
            // holds now is what is being agreed to, and a sequence read a
            // moment ago would refuse the save if a till had touched it since.
            expected_seq: 0,
            item: {
              id: item.id,
              code: item.code,
              name: item.name,
              name_bn: item.name_bn,
              unit: item.unit,
              // Zeroed here and sent beside, which is how this request has
              // always carried the money.
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              barcodes: item.barcodes,
              on_hand_milli: item.on_hand_milli,
              active: item.active,
            },
            // Beside the item rather than in it, which is where this request
            // has always carried the money: the item shape a screen builds is
            // not the shape the catalogue stores.
            price_minor: item.price_minor,
            cost_minor: item.cost_minor,
            vat_bp: item.vat_bp,
            price_inclusive: item.price_inclusive,
            vat_on_undiscounted: item.vat_on_undiscounted,
          },
          Date.now(),
        ),
      t('admin.kept_as_it_stands'),
    );
    if (saved) await listFromTills();
  }
</script>

  <section>
    <h2>{t('admin.a_receipt_brought_back')}</h2>
    <p class="why">{t('admin.receipt_why')}</p>
    <div class="row">
      <input
        bind:value={receiptAsked}
        placeholder={t('admin.receipt_number')}
        disabled={busy}
        onkeydown={(event) => event.key === 'Enter' && findReceipt()}
      />
      <button onclick={findReceipt} disabled={busy}>{t('admin.find_it')}</button>
    </div>
    {#if onPaper.length > 1}
      <p class="why">
        <span class="late">{t('admin.two_sales_one_number', { number: receiptLookedFor })}</span>
      </p>
    {/if}
    {#each onPaper as sale (sale.id)}
      <ul class="found">
        <li class:retired={!sale.still_counts}>
          <span class="name">
            {sale.receipt_no} &middot; {new Date(sale.rung_at_ms).toLocaleString('en-GB')}
            &middot; {tills.find((till) => till.id === sale.terminal)?.label ??
              t('admin.a_till_not_listed')}
            <!-- Who was at the counter, beside which counter it was. The
                 customer's own copy has said this since the receipt was laid
                 out; this is the shop's side of the same line, for the moment
                 somebody comes back about the sale. Only when the shop can say:
                 a sale rung before a till recorded it, one rung with nobody
                 signed in, and one whose operator has been removed all read the
                 same here, and saying nothing is the honest answer to all
                 three. -->
            {#if sale.served_by}
              &middot; {t('admin.served_by', { name: sale.served_by })}
            {/if}
          </span>
          <span class="detail">
            {#each sale.lines as line, at (at)}
              {qty(line.qty_milli)} {line.unit} &times; {line.name}
              {#if line.discount_minor !== 0}({t('admin.less', {
                  amount: money(line.discount_minor),
                })}){/if}
              &middot; {money(line.line_total_minor)}<br />
            {/each}
          </span>
          <span class="detail">
            {t('admin.net', { amount: money(sale.net_minor) })}
            &middot; {t('admin.vat', { amount: money(sale.vat_minor) })}
            &middot; <strong>{t('admin.total', { amount: money(sale.total_minor) })}</strong>
          </span>
          <span class="detail">
            <!-- Between the tenders rather than after each, or a sale paid in
                 cash with no change to give reads "Cash 57.50 ·", which is a
                 separator promising something that is not there. -->
            {#each sale.tenders as tender, at (at)}
              {#if at > 0}&middot; {/if}
              {tender.kind_code && tender.kind_code !== 'wallet'
                ? t(`till.${tender.kind_code}`)
                : tender.kind} {money(tender.amount_minor)}
              {#if tender.reference}({tender.reference}){/if}
            {/each}
            {#if sale.change_minor !== 0}
              &middot; {t('admin.change', { amount: money(sale.change_minor) })}
            {/if}
          </span>
          {#each sale.overrides as said, at (at)}
            <span class="detail">{said}</span>
          {/each}
          {#if sale.refund_of}
            <span class="detail">{t('admin.gives_back_against', { number: sale.refund_of })}</span>
          {:else if sale.refunded_minor !== 0}
            <span class="detail">
              <span class="late">
                {t('admin.given_back_against_it', { amount: money(sale.refunded_minor) })}
              </span>
            </span>
          {/if}
          {#if sale.held_for}
            <span class="detail">
              <!-- The same reason the queue shows, said the same way: from
                   the name the shop gave it, falling back to the sentence. -->
              <span class="late">
                {t('admin.held_for', {
                  why: t(
                    `held.${sale.held_for_kind}`,
                    heldParts({ parts: sale.held_for_parts }),
                    sale.held_for,
                  ),
                })}
              </span>
            </span>
          {/if}
          {#if sale.decided}
            <span class="detail">
              {t('admin.somebody_answered', { what: sale.decided })}
              &middot; {sale.still_counts
                ? t('admin.it_still_counts')
                : t('admin.it_was_struck_out')}
            </span>
          {/if}
          {#if sale.lines.length === 0}
            <span class="detail">
              <span class="late">{t('admin.cannot_read_that_sale')}</span>
            </span>
          {/if}
        </li>
      </ul>
    {/each}
  </section>

  {#if repairs.length > 0}
    <section>
      <h2>{t('admin.sales_needing_a_look')}</h2>
      <p class="why">{t('admin.repairs_why')}</p>
      <ul class="found">
        {#each repairs as entry (entry.id)}
          <li>
            <span class="name">
              {entry.receipt_no ?? t('admin.no_receipt_number')} &middot; {money(entry.total_minor)}
            </span>
            <span class="detail">
              <!-- Said from the name the shop gave it, and falling back to
                   the shop's own sentence for a sale held before the reason
                   itself was kept. -->
              {t(`held.${entry.kind}`, heldParts(entry), entry.reason)}
              &middot; {t('admin.reached_the_shop_at', {
                at: new Date(entry.received_at_ms).toLocaleString('en-GB'),
              })}
            </span>
            <span class="stock">
              <input
                placeholder={t('admin.what_you_decided')}
                value={notes[entry.id] ?? ''}
                oninput={(e) => (notes = { ...notes, [entry.id]: e.currentTarget.value })}
                disabled={busy}
              />
              <button onclick={() => resolve(entry, true)} disabled={busy}>
                {t('admin.it_is_a_real_sale')}
              </button>
              <button class="quiet" onclick={() => resolve(entry, false)} disabled={busy}>
                {t('admin.it_never_happened_short')}
              </button>
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section>
    <h2>{t('admin.already_decided')}</h2>
    <p class="why">{t('admin.decided_why')}</p>
    <button
      onclick={async () => {
        showDecided = !showDecided;
        if (showDecided) await listDecided(false);
      }}
      disabled={busy}
    >
      {showDecided ? t('admin.hide_them') : t('admin.show_what_was_decided')}
    </button>
    {#if showDecided}
      {#if decided.length === 0}
        <p class="why">{t('admin.nothing_decided_yet')}</p>
      {:else}
        <ul class="found">
          {#each decided as entry (entry.id)}
            <li>
              <span class="name">
                {entry.receipt_no ?? t('admin.no_receipt_number')} &middot; {money(entry.total_minor)}
                &middot; {entry.kept ? t('admin.counts') : t('admin.struck_out')}
              </span>
              <span class="detail">
                "{entry.note}" &middot; {new Date(entry.decided_at_ms).toLocaleString('en-GB')}
                {#if entry.decisions > 1}&middot; {t('admin.answered_times', {
                    count: entry.decisions,
                  })}{/if}
              </span>
              <span class="stock">
                <input
                  placeholder={t('admin.why_answer_changing')}
                  value={notes[entry.id] ?? ''}
                  oninput={(e) => (notes = { ...notes, [entry.id]: e.currentTarget.value })}
                  disabled={busy}
                />
                {#if entry.kept}
                  <button class="quiet" onclick={() => changeAnswer(entry, false)} disabled={busy}>
                    {t('admin.it_never_happened')}
                  </button>
                {:else}
                  <button onclick={() => changeAnswer(entry, true)} disabled={busy}>
                    {t('admin.put_it_back')}
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>

  {#if gaps.length > 0}
    <section>
      <h2>{t('admin.numbering_jumps')}</h2>
      <p class="why">{t('admin.gaps_why')}</p>
      <ul class="found">
        {#each gaps as gap (gap.terminal + gap.after)}
          <li>
            <span class="name">
              {gap.after} &rarr; {gap.before}
              &middot; {t('admin.numbers_missing', { count: gap.missing })}
            </span>
            <span class="detail">
              {tills.find((till) => till.id === gap.terminal)?.label ??
                t('admin.a_till_not_listed')}
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section>
    <h2>{t('admin.carried_in_by_hand')}</h2>
    <p class="why">
      {t('admin.carried_why')}
    </p>
    <div class="row">
      <input type="file" accept=".txt,text/plain" onchange={openCarriedFile} disabled={busy} />
    </div>
    <textarea
      bind:value={carried}
      oninput={markCarried}
      rows="3"
      placeholder={t('admin.paste_the_bundle')}
    ></textarea>
    {#if carriedMark}
      <p class="why">
        {t('admin.carried_mark_is', { mark: carriedMark })}
      </p>
    {:else if carried.trim()}
      <p class="why">{t('admin.not_a_bundle')}</p>
    {/if}
    <button onclick={adoptCarried} disabled={busy}>{t('admin.take_them_in')}</button>
  </section>

  {#if fromTills.length > 0}
    <!-- Above the ordinary sections for the same reason as the one below it:
         these are selling now, at a price nobody here has agreed to. -->
    <section>
      <h2>{t('admin.items_tills_wrote')}</h2>
      <p class="why">{t('admin.from_tills_why')}</p>
      <ul class="found">
        {#each fromTills as item (item.id)}
          <li>
            <span class="name">{item.name}</span>
            <span class="detail">
              {item.code} &middot; {money(item.price_minor)} &middot; VAT {item.vat_bp / 100}%
              {#if item.barcodes.length === 0}
                &middot; {t('admin.no_barcode_code_taken')}
              {/if}
            </span>
            <button onclick={() => onCorrect(item)} disabled={busy}>{t('admin.correct_it')}</button>
            <button onclick={() => agreeToItem(item)} disabled={busy}>{t('admin.it_is_right')}</button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if unreadable.length > 0}
    <!-- Above the ordinary sections, because a price that never reached the
         tills is money going out at the wrong number every hour. -->
    <section>
      <h2>{t('admin.changes_never_reached')}</h2>
      <p class="why">{t('admin.unreadable_why')}</p>
      <ul class="found">
        {#each unreadable as change (change.seq)}
          <li>
            <span class="name">{names[change.item] ?? t('admin.no_name_for_item')}</span>
            <span class="detail">{t('admin.written_by_version', { schema: change.schema })}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
