<script>
  import { today } from '../../../shared/days.js';
  // A shop's catalogue as it already exists: in a spreadsheet somebody keeps.
  import {
    against,
    movedALot,
    notReadBackYet,
    readCatalogue,
    tooEarlyToMatch,
    whatWillBeWritten,
    writeCatalogue,
  } from '../../../shared/catalogue_file.js';
  import {
    NOT_SAID,
    TAX_COMES_ON_TOP,
    TAX_IS_IN_IT,
    howThisRowWillBePriced,
    theFileMustSayWhatItsPricesAre,
    thePriceHasBeenExplained,
  } from '../../../shared/what_a_price_means.js';

  /// A shop's own list, taken out and brought back.
  ///
  /// The half that makes the other half safe: take the list out, change the
  /// column in the spreadsheet they already know, bring it back. A shop with
  /// eight hundred lines was otherwise being asked to type them into the form
  /// above, one at a time, which is where the conversation ended.
  ///
  /// Nothing is written by choosing a file. What it says goes on the screen
  /// first, with the rows nobody can read named by their line number.
  ///
  /// `everSynced`, `moreToPull` and `reaching` are the screen's own sync state,
  /// because matching is only as good as this device's copy of the catalogue
  /// and an empty copy calls every row new.
  let {
    t,
    money,
    refusal,
    busy,
    setBusy,
    attempt,
    admin,
    run,
    newId,
    everSynced,
    moreToPull,
    reaching,
    mostItems,
    onChanged,
    announce,
    refuse,
  } = $props();

  const MOST_ITEMS = mostItems;

  /// What a file said, once it has been read and matched, and how far through
  /// writing it the shop is. Null until a file is chosen.
  let bringingIn = $state(null);
  let bringingInDone = $state(0);
  /// The rate to give a row whose file says nothing about tax. Standard, which
  /// is what almost everything is.
  let bringingInVat = $state('15');
  /// Whether the prices in the file are what a customer pays. Asked only when
  /// the file itself does not say and the file brings in rows the shop has
  /// never seen: an item already here keeps its own answer, and a file with an
  /// `inclusive` column has answered row by row.
  ///
  /// No default, for the reason the form above has none. A whole shelf priced
  /// under the wrong rule is the same mistake as one item, eight hundred times,
  /// and the number in the column cannot say which rule it was written under.
  let bringingInPrices = $state(NOT_SAID);

  /// Whether the box has to be answered before anything is written.
  ///
  /// Only when the file says nothing about tax and brings in a row the shop
  /// does not already hold. A file that only corrects prices on items the shop
  /// has leaves every one of them with the answer it already had, so there is
  /// nothing to ask about.
  const needsToSayAboutTax = $derived(
    Boolean(bringingIn) &&
      theFileMustSayWhatItsPricesAre(
        bringingIn.saidAboutTax,
        whatWillBeWritten(bringingIn.rows).ready,
      ),
  );
  /// What this device wrote and has not read back yet. The rows written a
  /// moment ago live on the shop's server and reach this copy of the catalogue
  /// on the next pull: until they do they look new all over again, and a second
  /// read of the same file adds a second copy of every one of them.
  let wroteButHaveNotRead = $state([]);

  /// Hand the shop its own list, in the shape this screen reads back.
  ///
  /// The other half of bringing one in, and the half that makes the first safe
  /// to use on a price rise: take the list out, change the column in the
  /// spreadsheet they already know, bring it back. Every row carries its code,
  /// so what returns corrects what is here rather than adding a second shop.
  ///
  /// Written from this device's own copy, so it works with the line down.
  export async function takeTheListOut() {
    // Cleared first. What follows either refuses in words or succeeds in words,
    // and a refusal left over from the last press sitting beside a success is
    // two messages disagreeing about what just happened.
    refuse(null);
    announce(null);
    const tooEarly = tooEarlyToMatch({ everSynced, moreToPull, reaching }, 'out');
    if (tooEarly) {
      refuse(t(tooEarly));
      return;
    }
    const reply = await attempt(
      () => run({ op: 'catalogue', query: '', limit: MOST_ITEMS, retired: true }),
      null,
      true,
    );
    const held = reply?.view?.catalogue ?? [];
    // A list that stopped at the ceiling is not the shop's list, and a shop that
    // edited it and brought it back would be bringing in a part of itself.
    if (held.length >= MOST_ITEMS) {
      refuse(t('admin.too_many_to_match', { count: MOST_ITEMS }));
      return;
    }
    if (held.length === 0) {
      refuse(t('admin.nothing_to_take_out'));
      return;
    }
    const file = new Blob([writeCatalogue(held)], { type: 'text/csv;charset=utf-8' });
    const to = document.createElement('a');
    to.href = URL.createObjectURL(file);
    const day = today();
    to.download = `catalogue-${day}.csv`;
    to.click();
    URL.revokeObjectURL(to.href);
    announce(t('admin.list_taken_out', { count: held.length, file: `catalogue-${day}.csv` }));
  }

  /// Read a shop's own spreadsheet, show what it says, and only then write it.
  ///
  /// A shop with eight hundred lines was being asked to type them into the form
  /// above, one at a time, which is where the conversation ended. Every one of
  /// those lines is already in a file: a wholesaler's price list, a stock sheet,
  /// an export from whatever they ran before.
  ///
  /// Nothing is written by choosing the file. What it says goes on the screen
  /// first, with the rows nobody can read named by their line number, because an
  /// import somebody watched go past is how a shop ends up with the wrong price
  /// on the shelf and no idea which row did it.
  async function openCatalogueFile(event) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = '';
    if (!file) return;
    refuse(null);
    announce(null);
    bringingIn = null;
    // Before anything is read, because the matching below is only as good as
    // this device's copy of the catalogue and an empty copy calls every row new.
    const tooEarly = tooEarlyToMatch({ everSynced, moreToPull, reaching });
    if (tooEarly) {
      refuse(t(tooEarly));
      return;
    }
    const read = readCatalogue(await file.text());
    if (read.fault) {
      refuse(t(read.fault));
      return;
    }
    // Matched against the whole catalogue, retired rows and all, so an item
    // somebody withdrew last month is corrected rather than added a second
    // time.
    const held = await attempt(
      () => run({ op: 'catalogue', query: '', limit: MOST_ITEMS, retired: true }),
      null,
      true,
    );
    const known = held?.view?.catalogue ?? [];
    // A shop bigger than this device will hand over in one answer. Refused
    // rather than matched against a part of the catalogue: everything past the
    // ceiling would look new, and the shop would get a second copy of it.
    if (known.length >= MOST_ITEMS) {
      refuse(t('admin.too_many_to_match', { count: MOST_ITEMS }));
      return;
    }
    // The rows this device wrote a moment ago live on the shop's server and
    // reach this copy of the catalogue on the next pull. Until they do, they
    // look new all over again and a second read of the same file adds a second
    // copy of every one of them.
    const unread = notReadBackYet(wroteButHaveNotRead, known);
    if (unread) {
      refuse(t('admin.not_read_back_yet', { count: unread }));
      return;
    }
    wroteButHaveNotRead = [];
    bringingIn = {
      name: file.name,
      rows: against(read.rows, known),
      // Whether the file had a column about tax at all. A file that has one has
      // answered for every row it fills in; a file without one leaves the
      // question to the box below.
      saidAboutTax: read.columns.inclusive !== undefined,
    };
    bringingInPrices = NOT_SAID;
    announce(null);
  }

  /// Write what was read, one row at a time, and say what happened to each.
  ///
  /// One at a time on purpose. Each row is an ordinary save, so a row the shop
  /// refuses is refused for its own stated reason and the rest still land: a
  /// single batch that fails at row four hundred leaves a shop with no way to
  /// tell what got in.
  async function bringCatalogueIn() {
    refuse(null);
    announce(null);
    const { ready } = whatWillBeWritten(bringingIn?.rows ?? []);
    if (ready.length === 0) {
      refuse(t('admin.nothing_writable'));
      return;
    }
    // Refused before anything is written rather than defaulted quietly: a rate
    // nobody can read would go in as zero and the shop would under-declare
    // every sale of every row this file adds.
    const typedVat = Number(bringingInVat);
    if (!bringingInVat.trim() || !Number.isFinite(typedVat) || typedVat < 0 || typedVat > 100) {
      // The upper bound matters as much as the lower one. A rate over a hundred
      // percent is refused by every till when it reads the page of changes this
      // would arrive in, and it refuses the whole page: one number typed here
      // would stop every device in the shop from seeing any price change. The
      // shop refuses it too, now; this is so nobody sends eight hundred rows to
      // find that out.
      refuse(t('admin.say_fallback_rate'));
      return;
    }
    const fallbackVat = Math.round(typedVat * 100);
    // And the same refusal for the other half of a price. A row the file says
    // nothing about, on an item the shop does not hold, has nothing to inherit
    // from: writing it as tax exclusive because that is what a boolean starts
    // as puts every one of those prices above the shelf label it came off.
    if (needsToSayAboutTax && !thePriceHasBeenExplained(bringingInPrices)) {
      refuse(t('admin.say_what_the_file_prices_are'));
      return;
    }

    setBusy(true);
    bringingInDone = 0;
    let added = 0;
    let corrected = 0;
    const refused = [];
    try {
      for (const row of ready) {
        // The row it stands at now, not when the file was matched: another
        // person may have touched the item while this ran, and the shop refuses
        // a save built on an older copy rather than putting back what they did.
        let seq = 0;
        let held = null;
        if (row.matched) {
          const now = await admin({ what: 'item_now', item: row.matched.id }, Date.now()).catch(
            () => null,
          );
          held = now?.info?.item_now ?? null;
          seq = now?.info?.item_seq ?? 0;
          if (!held) {
            refused.push(t('admin.withdrawn_row', { line: row.line, name: row.name }));
            continue;
          }
        }
        const item = {
          id: row.matched?.id ?? newId(),
          // A file that leaves the VAT column empty is not saying "nothing":
          // it has no opinion, so an item already in the shop keeps the rate
          // the shop set, and a new one takes the ordinary rate on the form.
          code: row.code || held?.code || '',
          name: row.name,
          // A Bangla name the shop never typed is a copy of the English one,
          // and carrying that copy through a rename leaves the old name sitting
          // under the new one and findable by the search. Treated as absent,
          // the way the form above treats it.
          name_bn:
            row.name_bn ||
            (held?.name_bn && held.name_bn !== held.name ? held.name_bn : row.name),
          unit: row.unit || held?.unit || 'Nos',
          price_minor: 0,
          vat_bp: 0,
          price_inclusive: false,
          // Added to what the shop holds rather than replacing it. A file
          // carries one barcode per row and an item can have several: the pack
          // and the piece, the old label and the new. Replacing the list meant
          // taking a list out, bringing it back unchanged, and finding that
          // half the shop's labels had stopped scanning. A barcode is taken off
          // in the form above, where somebody is looking at that one item.
          barcodes:
            row.barcode && !(held?.barcodes ?? []).includes(row.barcode)
              ? [...(held?.barcodes ?? []), row.barcode]
              : (held?.barcodes ?? (row.barcode ? [row.barcode] : [])),
          on_hand_milli: 0,
          // What the file says, or what the shop already holds, or standard,
          // which is what almost everything is. Zero rated and exempt are
          // declared in different places on a return, so a file that says which
          // is a file the shop can trust its return to.
          supply: row.supply ?? held?.supply ?? 0,
          category: row.category || held?.category || '',
          active: held?.active ?? true,
          cost_minor: 0,
        };
        const vat_bp = row.vat_bp !== null ? row.vat_bp : (held?.vat_bp ?? fallbackVat);
        // What the next import has to be able to see before it reads this file
        // again, recorded before the save rather than after it.
        //
        // Before, because a save that throws may still have landed: a reply lost
        // on the way back looks identical here to a refusal, and a row recorded
        // only on success is a row added twice by the retry somebody makes
        // straight afterwards. Waiting for a row the shop never took costs one
        // pull; not waiting for one it did take costs the shop a duplicate.
        //
        // A row with neither a code nor a barcode is not recorded at all. There
        // is nothing about it for a later import to match on, so waiting for it
        // would be waiting for something that can never arrive, and every import
        // after it would be refused until somebody reloaded the page.
        //
        // A row that matched an item the shop already has is recorded by its
        // barcode alone, and only when the file's barcode is a new one. The code
        // is already in this device's catalogue, so recording it would clear the
        // wait immediately and the appended barcode would look new to the next
        // import.
        const proof = row.matched
          ? { barcode: row.barcode && !(held?.barcodes ?? []).includes(row.barcode) ? row.barcode : '' }
          : { code: item.code, barcode: row.barcode };
        if (proof.code || proof.barcode) {
          wroteButHaveNotRead = [...wroteButHaveNotRead, proof];
        }
        try {
          await admin(
            {
              what: 'item',
              expected_seq: seq,
              item,
              price_minor: row.price_minor,
              // A blank cost column leaves whatever the deliveries have taught
              // the shop alone, the same as the form above.
              cost_minor: row.cost_minor || held?.cost_minor || 0,
              active: item.active,
              vat_bp,
              // What the file says, or what the shop already holds, or exclusive,
            // which is what the form defaults to. Read under the wrong rule,
            // every price on every shelf is wrong by the tax.
            price_inclusive: howThisRowWillBePriced(row, held, bringingInPrices),
              vat_on_undiscounted: held?.vat_on_undiscounted ?? false,
            },
            Date.now(),
          );
          if (row.matched) corrected += 1;
          else added += 1;
        } catch (trouble) {
          // The line number and what the shop said, worded here. Built as a
          // sentence it read "line 4: ..." in a Bangla shop, and no test could
          // see it: it is assembled into an array rather than assigned to the
          // line somebody reads.
          refused.push(
            t('admin.refused_row', {
              line: row.line,
              said: refusal({
                error: trouble?.message ?? String(trouble),
                error_code: trouble?.code,
                error_parts: trouble?.parts,
              }),
            }),
          );
        }
        bringingInDone += 1;
      }
    } finally {
      setBusy(false);
    }
    bringingIn = null;
    announce(
      refused.length
        ? t('admin.brought_in_refused', { added, corrected, refused: refused.length })
        : t('admin.brought_in', { added, corrected }),
    );
    if (refused.length) refuse(refused.slice(0, 5).join('; '));
    await onChanged();
  }
</script>

  <section>
    <h2>{t('admin.bring_in_a_list')}</h2>
    <p class="why">{t('admin.bring_in_why')}</p>
    <div class="row">
      <input type="file" accept=".csv,text/csv,text/plain" onchange={openCatalogueFile} disabled={busy} />
      <button class="quiet" onclick={takeTheListOut} disabled={busy}>{t('admin.take_the_list_out')}</button>
    </div>
    <p class="why">{t('admin.take_out_why')}</p>

    {#if bringingIn}
      {@const sorted = whatWillBeWritten(bringingIn.rows)}
      {@const known = sorted.ready.filter((row) => row.matched)}
      {@const jumped = movedALot(bringingIn.rows)}
      <p class="why">
        <strong>{bringingIn.name}</strong>:
        {t('admin.file_summary', { ready: sorted.ready.length, known: known.length })}
        {#if sorted.refused.length}
          <span class="late">{t('admin.file_refused', { count: sorted.refused.length })}</span>
        {/if}
      </p>
      {#if jumped.length}
        <p class="why">
          <span class="late">{t('admin.file_jumped', { count: jumped.length })}</span>
        </p>
        <ul class="found">
          {#each jumped.slice(0, 20) as row (row.line)}
            <li>
              <span class="name">{t('admin.file_line', { line: row.line, name: row.name })}</span>
              <span class="detail late">
                {t('admin.file_becomes', {
                  was: money(row.was_minor),
                  now: money(row.price_minor),
                })}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
      {#if sorted.refused.length}
        <ul class="found">
          {#each sorted.refused.slice(0, 20) as row (row.line)}
            <li>
              <span class="name">
                {t('admin.file_line', { line: row.line, name: row.name || t('admin.no_name') })}
              </span>
              <span class="detail late">
                {row.wrong.map((one) => t(`file.${one.code}`, one.fill)).join(', ')}
              </span>
            </li>
          {/each}
        </ul>
        {#if sorted.refused.length > 20}
          <p class="why">{t('admin.file_and_more', { count: sorted.refused.length - 20 })}</p>
        {/if}
      {/if}
      <ul class="found">
        {#each sorted.ready.slice(0, 20) as row (row.line)}
          <li>
            <span class="name">{row.name} &middot; {money(row.price_minor)}</span>
            <span class="detail">
              {row.matched ? t('admin.already_sold_here') : t('admin.new_row')}
              {row.code ? ` · ${row.code}` : ''}
              <!-- A rate on a line that is exempt or zero rated says
                   nothing: the arithmetic charges nothing whatever rate the
                   item carries, and showing "VAT 15%" beside "exempt" reads
                   as a contradiction the shop has to think about. -->
              <!-- What this row will be written as, not only what it said.
                   A file that says nothing and a box that says "what the
                   customer pays" is a shelf price, and a preview that showed
                   nothing there would be describing the old rule. -->
              {#if howThisRowWillBePriced(row, row.matched, bringingInPrices)}
                &middot; {t('admin.price_has_vat_in_it')}
              {/if}
              {#if row.supply === 1}
                &middot; {t('admin.supply_zero')}
              {:else if row.supply === 2}
                &middot; {t('admin.supply_exempt')}
              {:else if row.vat_bp !== null}
                &middot; {t('admin.vat_of', { rate: row.vat_bp / 100 })}
              {:else if row.matched}
                &middot; {t('admin.vat_left_as_is')}
              {:else}
                &middot; {t('admin.vat_from_box', { rate: bringingInVat })}
              {/if}
            </span>
          </li>
        {/each}
      </ul>
      {#if sorted.ready.length > 20}
        <p class="why">{t('admin.file_and_more_ready', { count: sorted.ready.length - 20 })}</p>
      {/if}
      <label>
        {t('admin.rate_for_rows')}
        <input bind:value={bringingInVat} inputmode="decimal" disabled={busy} />
      </label>
      {#if needsToSayAboutTax}
        <label>
          {t('admin.prices_in_this_file')}
          <select bind:value={bringingInPrices} disabled={busy}>
            <option value={NOT_SAID}>{t('admin.price_unsaid')}</option>
            <option value={TAX_IS_IN_IT}>{t('admin.price_is_inclusive')}</option>
            <option value={TAX_COMES_ON_TOP}>{t('admin.price_is_exclusive')}</option>
          </select>
        </label>
      {/if}
      <div class="row">
        <button onclick={bringCatalogueIn} disabled={busy || sorted.ready.length === 0}>
          {busy && bringingInDone > 0
            ? t('admin.writing_rows', { done: bringingInDone, total: sorted.ready.length })
            : t('admin.write_rows', { count: sorted.ready.length })}
        </button>
        <button class="quiet" onclick={() => (bringingIn = null)} disabled={busy}>
          {t('admin.leave_it_alone')}
        </button>
      </div>
    {/if}
  </section>
