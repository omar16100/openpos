<script>
  import { tick } from 'svelte';

  import { saving } from '../../../shared/records.js';
  // The same loop the till reads a scan with. A barcode typed off a box by
  // hand is where the wrong digit gets in, and this is the screen where
  // somebody is holding the box.
  import { CANNOT_READ_HERE, readFromCamera } from '../../../shared/camera_read.js';
  import { minorFrom } from '../../../shared/money.js';

  /// One item, added or corrected.
  ///
  /// The form the shelf list opens and the repair queue opens: correcting an
  /// item is the same act wherever it is started from, so there is one of it.
  ///
  /// `categories` is what the shop already calls things, so a second bag of
  /// rice is sorted under the same word as the first rather than under "Rice "
  /// with a space. It comes from the list on screen, which is the screen's.
  let {
    t,
    busy,
    attempt,
    admin,
    newId,
    categories,
    /// What the shop said when the last thing failed, so a save that lost a
    /// race can say what actually happened rather than showing a status code.
    whatWentWrong,
    onSaved,
    onWithdrawn,
    announce,
    refuse,
  } = $props();

  /// The item being corrected, and where it stood when it was read. A save
  /// carries the whole item, so one built on an older copy would put back
  /// whatever somebody else changed in the meantime.
  let editing = $state(null);
  let editingSeq = $state(0);

  let itemName = $state('');
  // The same thing in Bangla, for the people who read the screens.
  let itemNameBn = $state('');
  let itemCode = $state('');
  let itemPrice = $state('');
  let itemVat = $state('15');
  let itemBarcode = $state('');
  let itemListedPrice = $state(false);
  let itemSupply = $state('0');
  let itemCategory = $state('');
  let itemCost = $state('');
  let itemTaxIncluded = $state(false);
  let itemUnit = $state('Nos');

  /// The camera, while it is reading a barcode into the box below.
  let camera = $state(null);
  let watching = $state(false);
  let reading = null;

  async function readTheBarcode() {
    if (watching) {
      stopReading();
      return;
    }
    watching = true;
    // The picture appears with `watching`, so the stream is attached after the
    // screen has drawn rather than to a picture that is not there yet.
    await tick();
    reading = await readFromCamera({
      video: camera,
      onCode: (code) => {
        watching = false;
        itemBarcode = code;
      },
      onTrouble: (why) => {
        watching = false;
        refuse(why === CANNOT_READ_HERE ? t('admin.camera_not_here') : t('admin.camera_refused'));
      },
    });
  }

  function stopReading() {
    watching = false;
    reading?.stop();
    reading = null;
  }

  /// A camera nobody is looking at reads nothing and costs the battery: a
  /// browser stops handing a hidden page its frames.
  $effect(() => {
    const stopIfHidden = () => {
      if (document.visibilityState !== 'visible' && watching) stopReading();
    };
    document.addEventListener('visibilitychange', stopIfHidden);
    return () => document.removeEventListener('visibilitychange', stopIfHidden);
  });

  /// Open an item for correction, reading it from the shop rather than from
  /// this device's copy.
  ///
  /// The copy here is up to half a minute behind, and a save carries the whole
  /// item: editing a price on a stale row would put back whatever somebody else
  /// changed in the meantime, including a withdrawal.
  export async function correct(item) {
    const reply = await attempt(
      () => admin({ what: 'item_now', item: item.id }, Date.now()),
      null,
    );
    const fresh = reply?.info?.item_now;
    if (!fresh) {
      refuse(t('admin.item_withdrawn_since'));
      await onWithdrawn();
      return;
    }
    editingSeq = reply?.info?.item_seq ?? 0;
    correctFrom(fresh);
  }

  function correctFrom(item) {
    editing = item;
    itemTaxIncluded = item.price_inclusive;
    itemUnit = item.unit || 'Nos';
    itemName = item.name;
    // Blank when it is only a copy of the English name, so an owner sees an
    // empty box to fill in rather than the same words twice.
    itemNameBn = item.name_bn === item.name ? '' : item.name_bn;
    itemCode = item.code;
    itemPrice = (item.price_minor / 100).toFixed(2);
    itemVat = (item.vat_bp / 100).toString();
    itemBarcode = item.barcodes[0] ?? '';
    itemListedPrice = item.vat_on_undiscounted;
    itemSupply = String(item.supply ?? 0);
    itemCategory = item.category ?? '';
    itemCost = item.cost_minor ? (item.cost_minor / 100).toFixed(2) : '';
    scrollTo({ top: 0, behavior: 'smooth' });
  }

  /// Open the form on something nobody has written down yet, with the barcode
  /// already in it.
  ///
  /// What a shelf count finds: a box on the shelf whose label is in nobody's
  /// catalogue, which during a shop's first count is most of them. The
  /// alternative was to tell the person holding it to go and type the number in
  /// by hand, which is the digit they get wrong.
  export function writeDownWhatWasRead(barcode) {
    startFresh();
    itemBarcode = barcode;
    scrollTo({ top: 0, behavior: 'smooth' });
  }

  export function startFresh() {
    editing = null;
    itemTaxIncluded = false;
    itemUnit = 'Nos';
    itemName = '';
    itemNameBn = '';
    itemCode = '';
    itemPrice = '';
    itemVat = '15';
    itemBarcode = '';
    itemListedPrice = false;
    itemSupply = '0';
    itemCategory = '';
    itemCost = '';
  }

  async function saveItem() {
    const where = saving(editing, newId, { active: true, cost_minor: 0 });
    // Read by the parser every other amount on these screens goes through.
    // `Number(...) * 100` rounded takes "1e3" for a thousand and makes
    // 100.49999999999999 out of 1.005, on the price that ends up on a shelf
    // label and in every sale of that item.
    const price_minor = minorFrom(itemPrice.trim());
    if (!itemName.trim() || price_minor === null) {
      refuse(t('admin.say_name_and_price'));
      return;
    }
    // Left alone when the box is empty, because the usual way this gets set is
    // a delivery and a blank box means "I am correcting the price, not the
    // cost". A zero typed on purpose is a shop saying it pays nothing, which
    // is not a thing, so it reads as blank too.
    const typedCost = itemCost.trim();
    const cost_minor = typedCost ? minorFrom(typedCost) : where.cost_minor;
    if (cost_minor === null) {
      refuse(t('admin.not_a_cost', { typed: typedCost }));
      return;
    }
    // The rate in basis points, read the same way: two places after the point
    // is exactly what a rate carries. Over a hundred percent, every till
    // refuses the whole page of changes this would arrive in and stops seeing
    // any prices at all. The shop refuses it too; this says so before the form
    // is sent.
    const vat_bp = minorFrom(itemVat.trim());
    if (vat_bp === null || vat_bp > 10_000) {
      refuse(t('admin.say_rate_range'));
      return;
    }
    const saved = await attempt(
      () =>
        admin(
          {
            what: 'item',
            // Where it stood when it was read for editing. The server refuses a
            // save built on an older copy rather than letting it put back
            // whatever somebody else changed.
            expected_seq: editing ? editingSeq : 0,
            item: {
              ...where,
              code: itemCode.trim(),
              name: itemName.trim(),
              name_bn: itemNameBn.trim(),
              unit: itemUnit.trim() || 'Nos',
              price_minor: 0,
              vat_bp: 0,
              price_inclusive: false,
              barcodes: itemBarcode.trim() ? [itemBarcode.trim()] : [],
              on_hand_milli: 0,
              supply: Number(itemSupply),
              category: itemCategory.trim(),
            },
            price_minor,
            cost_minor: cost_minor === 0 ? where.cost_minor : cost_minor,
            active: where.active,
            vat_bp,
            price_inclusive: itemTaxIncluded,
            vat_on_undiscounted: itemListedPrice,
          },
          Date.now(),
        ),
      editing
        ? t('admin.item_corrected', { name: itemName.trim() })
        : t('admin.item_added', { name: itemName.trim() }),
    );
    // Only on success. Clearing the form after a refusal loses what the owner
    // typed and leaves them nothing to correct.
    if (!saved) {
      // The shop's own words come back with the refusal now, so there is
      // nothing to guess at here. A bare status is all that is left when a
      // server one release ahead sends a refusal this build does not know.
      if (whatWentWrong().includes('409')) {
        refuse(t('admin.somebody_else_changed_it'));
      }
      return;
    }
    startFresh();
    // The change reaches this device the way it reaches a till, on the next
    // pull, so the list is asked again rather than edited here to look right.
    // Quietly, or the confirmation is gone before it is read.
    await onSaved();
  }
</script>

  <section>
    <h2>{editing ? t('admin.correcting_an_item') : t('admin.something_to_sell')}</h2>
    {#if editing}
      <p class="why">{t('admin.item_edit_why')}</p>
    {/if}
    <input bind:value={itemName} placeholder={t('admin.name')} disabled={busy} />
    <input bind:value={itemNameBn} placeholder={t('admin.item_name_bn')} disabled={busy} />
    <div class="row">
      <input bind:value={itemPrice} placeholder={t('admin.price_in_taka')} inputmode="decimal" disabled={busy} />
      <input bind:value={itemVat} placeholder={t('admin.vat_percent')} inputmode="decimal" disabled={busy} />
      <input
        bind:value={itemCost}
        placeholder={t('admin.what_you_pay')}
        inputmode="decimal"
        disabled={busy}
      />
    </div>
    <p class="why">{t('admin.cost_why')}</p>
    <div class="row">
      <input bind:value={itemCode} placeholder={t('admin.code')} disabled={busy} />
      <input bind:value={itemBarcode} placeholder={t('admin.barcode')} inputmode="numeric" disabled={busy} />
      <input bind:value={itemUnit} placeholder={t('admin.sold_by')} disabled={busy} />
    </div>
    <!-- A barcode typed off a box by hand is where the wrong digit gets in, and
         this is the screen where somebody is holding the box. Same loop as the
         till's, so the same number has to be read twice and check out. -->
    <div class="row">
      <button class="quiet" onclick={readTheBarcode} disabled={busy}>
        {watching ? t('admin.stop_reading') : t('admin.read_the_barcode')}
      </button>
    </div>
    {#if watching}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="camera" bind:this={camera} muted playsinline autoplay></video>
      <p class="why">{t('admin.hold_the_label')}</p>
    {/if}
    <input
      bind:value={itemCategory}
      placeholder={t('admin.what_kind')}
      list="the-categories"
      disabled={busy}
    />
    <datalist id="the-categories">
      {#each categories as name (name)}
        <option value={name}></option>
      {/each}
    </datalist>
    <label>
      <input type="checkbox" bind:checked={itemTaxIncluded} disabled={busy} />
      {t('admin.price_includes_tax')}
    </label>
    <label>
      <input type="checkbox" bind:checked={itemListedPrice} disabled={busy} />
      {t('admin.tax_on_listed_price')}
    </label>
    <label>
      {t('admin.kind_of_supply')}
      <select bind:value={itemSupply} disabled={busy}>
        <option value="0">{t('admin.supply_standard')}</option>
        <option value="1">{t('admin.supply_zero')}</option>
        <option value="2">{t('admin.supply_exempt')}</option>
      </select>
    </label>
    <p class="why">{t('admin.supply_why')}</p>
    <div class="row">
      <button onclick={saveItem} disabled={busy}>
        {editing ? t('admin.save_the_correction') : t('admin.add_it')}
      </button>
      {#if editing}
        <button class="quiet" onclick={startFresh} disabled={busy}>{t('admin.leave_it_alone')}</button>
      {/if}
    </div>
  </section>
