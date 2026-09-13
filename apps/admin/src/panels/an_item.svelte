<script>
  import { tick } from 'svelte';

  import { saving } from '../../../shared/records.js';
  import { barcodesKept } from '../../../shared/what_an_item_scans_as.js';
  import {
    NOT_SAID,
    TAX_COMES_ON_TOP,
    TAX_IS_IN_IT,
    howThisItemWasPriced,
    thePriceHasBeenExplained,
    theTaxIsInsideThePrice,
  } from '../../../shared/what_a_price_means.js';
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
    /// Whether this shop offers Bangla to its own staff.
    ///
    /// The box for a Bangla name is drawn only when it does. A shop that has
    /// said its people read English has said nobody there reads the other one,
    /// and a field they cannot use is a field that holds a script they cannot
    /// read. Nothing is deleted by hiding it: a name already typed is still
    /// stored, still sent, still what a search matches on, and the box comes
    /// back with it the day the shop offers Bangla again.
    offersBangla,
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
  /// The other barcodes this item already scans as.
  ///
  /// One box, and an item can have several: a shop that sells the same soap in
  /// a box with an old label and a new one has both numbers on it, and a
  /// spreadsheet brought in against an item it already has adds the new one
  /// beside the old rather than replacing it. This screen loaded the first and
  /// saved that one alone, so correcting a price on such an item deleted the
  /// rest, and the next time somebody scanned the old box at the counter the
  /// till said the shop had never heard of it. Nothing said anything; the
  /// number was simply gone.
  ///
  /// Carried here so a correction keeps them, and shown below the box so they
  /// are kept in sight rather than in secret, with a way to drop one on
  /// purpose.
  let otherBarcodes = $state([]);
  let itemListedPrice = $state(false);
  let itemSupply = $state('0');
  let itemCategory = $state('');
  let itemCost = $state('');
  /// Whether the price typed above is what the customer pays or what the tax
  /// goes on top of. Empty until somebody says, and a new item cannot be saved
  /// while it is: see `saveItem`.
  ///
  /// It was a checkbox, unticked, meaning "before tax". A shopkeeper typing the
  /// number written on the shelf and pressing Add therefore priced that item
  /// fifteen percent above its own shelf label, on every sale of it, until
  /// somebody noticed at a counter. Nothing in the number says which it is, so
  /// nothing here guesses: the two answers charge a customer different money.
  let itemTaxIncluded = $state(NOT_SAID);
  let itemUnit = $state('Nos');

  /// The camera, while it is reading a barcode into the box below.
  let camera = $state(null);
  let watching = $state(false);
  /// Whether the camera has given a picture yet, rather than merely been asked
  /// for. See the panel below.
  let picture = $state(false);
  let reading = null;
  /// Which press this is: see the till's. Opening a camera is not instant, and
  /// pressing twice because nothing has happened yet is the ordinary case.
  let cameraTurn = 0;

  async function readTheBarcode() {
    if (watching) {
      stopReading();
      return;
    }
    const mine = ++cameraTurn;
    watching = true;
    picture = false;
    // The picture appears with `watching`, so the stream is attached after the
    // screen has drawn rather than to a picture that is not there yet.
    await tick();
    const held = await readFromCamera({
      video: camera,
      onCode: (code) => {
        watching = false;
        picture = false;
        itemBarcode = code;
      },
      onTrouble: (why) => {
        watching = false;
        picture = false;
        refuse(why === CANNOT_READ_HERE ? t('admin.camera_not_here') : t('admin.camera_refused'));
      },
    });
    if (mine !== cameraTurn) {
      held.stop();
      return;
    }
    reading = held;
  }

  function stopReading() {
    cameraTurn += 1;
    watching = false;
    picture = false;
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
    itemTaxIncluded = howThisItemWasPriced(item);
    itemUnit = item.unit || 'Nos';
    itemName = item.name;
    // Blank when it is only a copy of the English name, so an owner sees an
    // empty box to fill in rather than the same words twice.
    itemNameBn = item.name_bn === item.name ? '' : item.name_bn;
    itemCode = item.code;
    itemPrice = (item.price_minor / 100).toFixed(2);
    itemVat = (item.vat_bp / 100).toString();
    itemBarcode = item.barcodes[0] ?? '';
    otherBarcodes = item.barcodes.slice(1);
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
    itemTaxIncluded = NOT_SAID;
    itemUnit = 'Nos';
    itemName = '';
    itemNameBn = '';
    itemCode = '';
    itemPrice = '';
    itemVat = '15';
    itemBarcode = '';
    otherBarcodes = [];
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
    // Asked rather than assumed, and asked here because it is the last thing
    // between a typed number and a shelf. Either answer is ordinary and the
    // number cannot tell them apart: 480 with the tax in it is 480 at the
    // counter, and 480 without it is 552.
    if (!thePriceHasBeenExplained(itemTaxIncluded)) {
      refuse(t('admin.say_what_the_price_is'));
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
              // The one in the box first, then the others this item already
              // scanned as. A correction that dropped them was a barcode
              // deleted by somebody changing a price, found at a counter by a
              // cashier holding a box the shop says it has never heard of.
              barcodes: barcodesKept(itemBarcode, otherBarcodes),
              on_hand_milli: 0,
              supply: Number(itemSupply),
              category: itemCategory.trim(),
            },
            price_minor,
            cost_minor: cost_minor === 0 ? where.cost_minor : cost_minor,
            active: where.active,
            vat_bp,
            price_inclusive: theTaxIsInsideThePrice(itemTaxIncluded),
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
    {#if offersBangla}
      <input bind:value={itemNameBn} placeholder={t('admin.item_name_bn')} disabled={busy} />
    {/if}
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
    <!-- The other numbers this item already scans as. One box and several
         barcodes is an ordinary shop: the same soap in a box with an old label
         and a new one, or a spreadsheet brought in against an item that already
         had one. They are shown rather than carried in secret, because a number
         a shopkeeper cannot see is one they cannot correct, and dropping one is
         a deliberate act with its own button rather than something that happens
         to them for pressing save. -->
    {#if otherBarcodes.length > 0}
      <p class="why">
        {t('admin.also_scans_as')}
        {#each otherBarcodes as code (code)}
          &middot; {code}
          <button
            class="quiet"
            onclick={() => { otherBarcodes = otherBarcodes.filter((one) => one !== code); }}
            disabled={busy}
          >
            {t('admin.forget_this_barcode')}
          </button>
        {/each}
      </p>
    {/if}
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
      <video
        class="camera"
        bind:this={camera}
        muted
        playsinline
        autoplay
        onloadedmetadata={() => { picture = true; }}
      ></video>
      <!-- Only once there is something to hold the box in front of. The browser
           asks whether the camera may be used the first time a device opens one,
           and until somebody answers, the frame is black. -->
      {#if picture}
        <p class="why">{t('admin.hold_the_label')}</p>
      {:else}
        <p class="why">{t('admin.camera_opening')}</p>
      {/if}
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
      {t('admin.what_that_price_is')}
      <select bind:value={itemTaxIncluded} disabled={busy}>
        <option value={NOT_SAID}>{t('admin.price_unsaid')}</option>
        <option value={TAX_IS_IN_IT}>{t('admin.price_is_inclusive')}</option>
        <option value={TAX_COMES_ON_TOP}>{t('admin.price_is_exclusive')}</option>
      </select>
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
