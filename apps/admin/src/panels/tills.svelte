<script>
  /// The devices the shop has, and the codes that put a new one on a counter.
  ///
  /// The list itself stays with the screen, because two other panels are named
  /// from it: a drawer belongs to a till, and so does a sale carried in by
  /// hand. What lives here is the part nothing else reads, which is issuing a
  /// code and cutting a device off.
  ///
  /// `announce` is how a panel says something on the screen's one message line.
  /// A panel with its own would be a second place to look for the answer to
  /// "did that work", and the eye goes to the wrong one.
  let { t, busy, attempt, admin, newId, tills, onChanged, announce } = $props();

  let tillLabel = $state('');
  /// The code, shown once. The server keeps only its hash, so a code lost off
  /// this screen is a code nobody can get back, and the screen says so.
  let issued = $state(null);
  let issuedFor = $state(null);
  /// Seconds the code on screen is good for, as the shop said when it issued it.
  let issuedLasts = $state(3_600);
  /// Which device the second press would cut off. See cutOff().
  let cuttingOff = $state(null);

  /// A code for a till that already exists, so a device that lost its credential
  /// comes back as itself. Issuing a new till id instead would give it an empty
  /// ledger and strand whatever the old one had not sent.
  /// A device, said the way somebody standing in the shop would find it.
  ///
  /// A till the shop has never named still has to be nameable, which is what
  /// the last six of its id is for on every row of this list. A till enrolled
  /// before the shop handed out counter numbers has none, and is said by its
  /// name alone rather than as counter zero.
  function whichOne(till) {
    const name = till.label || t('admin.unnamed_till', { id: till.id.slice(-6) });
    return till.counter_no > 0
      ? t('admin.counter_called', { no: till.counter_no, name })
      : name;
  }

  async function reissue(till) {
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'code',
            terminal_id: till.id,
            label: till.label,
            // As itself. A code that brings the back office back as a till is
            // a shop that has lost its back office: the only owner's code it
            // ever had was printed in the log the first time the server
            // started, and by then it is gone.
            role: till.role === 2 ? 2 : 1,
            valid_for_seconds: 900,
          },
          Date.now(),
        ),
      null,
    );
    issued = reply?.info?.issued_code ?? null;
    // Named by its counter as well as by its label. Somebody is about to carry
    // this code across a shop to one device out of several, and the label is
    // the shop's own word for it, which can be two tills away from the one the
    // owner meant. The number is the one printed on that till's receipts.
    issuedFor = issued ? whichOne(till) : null;
    issuedLasts = reply?.info?.code_lasts_seconds ?? issuedLasts;
  }

  async function issueCode() {
    const label = tillLabel.trim() || 'a till';
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'code',
            terminal_id: newId(),
            label,
            role: 1,
            valid_for_seconds: 900,
          },
          Date.now(),
        ),
      null,
    );
    // Shown once and never retrievable: the server keeps only its hash.
    issued = reply?.info?.issued_code ?? null;
    issuedFor = issued ? label : null;
    issuedLasts = reply?.info?.code_lasts_seconds ?? issuedLasts;
    tillLabel = '';
    await onChanged();
  }

  /// Cut a device off, because it is lost or stolen.
  ///
  /// Two presses: one press stops a working till dead in the middle of a
  /// trading day, and the person pressing is usually already flustered.
  ///
  /// The device is not wiped and cannot be. If it turns up still holding sales,
  /// they are read off it and pasted in above, which needs no credential.
  async function cutOff(till) {
    if (cuttingOff !== till.id) {
      cuttingOff = till.id;
      return;
    }
    cuttingOff = null;
    const reply = await attempt(
      () => admin({ what: 'revoke_terminal', terminal: till.id }, Date.now()),
      null,
    );
    if (!reply) return;
    const withdrawn = reply.info?.withdrawn ?? 0;
    // Said after the list is read back, not before it. The refresh runs through
    // the same reporting as everything else, and a refresh clears the last
    // message: an owner who cut off a lost device was told nothing at all,
    // because the sentence was written and then wiped by the read that
    // followed it. Found by moving this into a file of its own.
    await onChanged();
    announce(withdrawn > 0 ? t('admin.device_cut_off') : t('admin.already_cut_off'));
  }
  /// Devices the shop has not heard from in a month, kept out of the way.
  ///
  /// A shop that has been open two years has enrolled tablets it no longer
  /// owns: one replaced after a fall, one that went home with somebody, one
  /// from the month it tried a second counter. Every one of them stays on this
  /// list for ever, because a device is never deleted: its sales are written
  /// against it and a list that forgot it would leave those sales belonging to
  /// nothing.
  ///
  /// So they are folded away rather than removed, and counted where they were,
  /// because the one time a shop reads this list is when something is wrong
  /// with a device and half of those times the device is one of these.
  const A_MONTH = 30 * 24 * 60 * 60 * 1000;
  let alsoTheQuietOnes = $state(false);
  const quiet = $derived(
    tills.filter((one) => !one.last_seen_ms || Date.now() - one.last_seen_ms > A_MONTH),
  );
  const shown = $derived(
    alsoTheQuietOnes
      ? tills
      : tills.filter((one) => one.last_seen_ms && Date.now() - one.last_seen_ms <= A_MONTH),
  );
</script>

<section>
  <h2>{t('admin.tills')}</h2>
  <p class="why">{t('admin.tills_why')}</p>

  {#if tills.length > 0}
    <ul class="tills">
      {#each shown as till (till.id)}
        <li>
          <!-- A till enrolled before labels, or by something that did not
               set one. Its id is worse than a name and better than a blank
               row in a list whose whole purpose is telling them apart. -->
          <span class="name">
            {till.label || t('admin.unnamed_till', { id: till.id.slice(-6) })}
          </span>
          <!-- Which counter this is, and so what its receipts are prefixed
               with. First of the details rather than last, because this row
               is read while somebody is holding a receipt and asking which
               device printed it: everything else on it is a name somebody
               typed. Nothing is shown for a device with no number, which is
               one enrolled before the shop handed them out, because a number
               made up here would be worse than none. -->
          {#if till.counter_no > 0}
            <span class="counter">
              {t('admin.counter_no', { no: till.counter_no })}
            </span>
          {/if}
          <span class="seen">
            {#if till.last_seen_ms}
              {t('admin.last_heard', {
                at: new Date(till.last_seen_ms).toLocaleString('en-GB'),
              })}
            {:else}
              {t('admin.not_heard_from')}
            {/if}
            &middot; {t('admin.sales_of', { count: till.sales })}
            {#if till.open_repairs > 0}&middot; {t('admin.to_look_at', {
                count: till.open_repairs,
              })}{/if}
            {#if till.role === 2}&middot; {t('admin.the_back_office_too')}{/if}
            {#if till.role === 0}&middot; <span class="late">
                {t('admin.holds_nothing')}
              </span>{/if}
            <!-- When the shop took it on. This list is read when a device
                 is to be cut off, and the question then is which of two
                 tills with similar names is the one enrolled last week: the
                 shop has always known and no screen said. -->
            {#if till.enrolled_at_ms}&middot; {t('admin.enrolled_on', {
                when: new Date(till.enrolled_at_ms).toLocaleDateString('en-GB'),
              })}{/if}
            <!-- Which build it is running, when it has said. The first thing
                 worth knowing when one till behaves differently from the one
                 beside it, and until this the only way to find out was to walk
                 to each counter. Shown beside the rest rather than hidden
                 behind anything, because it is read at exactly the moment
                 somebody is already looking at this row. -->
            {#if till.build}&middot; {t('admin.running_build', { build: till.build })}{/if}
          </span>
          <!-- For a device that lost its credential. A new till id would
               give it an empty ledger and strand anything it had not sent,
               and a code for the wrong role would bring the back office
               back as a till. -->
          <button onclick={() => reissue(till)} disabled={busy}>
            {till.role === 2 ? t('admin.code_for_back_office') : t('admin.code_for_till')}
          </button>
          <!-- For a device that is gone. Two presses, because one press
               stops a working till in the middle of a trading day. -->
          <button class="quiet" onclick={() => cutOff(till)} disabled={busy}>
            {cuttingOff === till.id
              ? t('admin.press_again_stops_it')
              : t('admin.this_one_is_lost')}
          </button>
        </li>
      {/each}
    </ul>
    {#if quiet.length > 0}
      <!-- The long tail, out of the way rather than hidden. A shop that has
           been open two years has enrolled tablets it no longer owns, and a
           list that shows all of them ahead of the till somebody is standing
           at is a list nobody reads. They are one press away, counted, because
           a shop cutting off a device it has lost is looking for exactly one
           of these. -->
      <button class="quiet" onclick={() => { alsoTheQuietOnes = !alsoTheQuietOnes; }}>
        {alsoTheQuietOnes
          ? t('admin.hide_the_quiet_devices')
          : t('admin.show_the_quiet_devices', { count: quiet.length })}
      </button>
    {/if}
  {:else}
    <p class="why">{t('admin.no_tills_yet')}</p>
  {/if}

  <div class="row">
    <input bind:value={tillLabel} placeholder={t('admin.name_a_new_till')} disabled={busy} />
    <button onclick={issueCode} disabled={busy}>{t('admin.add_a_till')}</button>
  </div>
  {#if issued}
    <p class="code">{issued}</p>
    <!-- How long it lasts comes from the shop with the code. It used to be
         a sentence saying an hour, which is true until a shop changes its
         own policy and then is a screen lying to somebody standing at a
         device with a code in their hand. -->
    <p class="why">
      {t('admin.code_shown_once', {
        who: issuedFor,
        minutes: Math.max(1, Math.round(issuedLasts / 60)),
      })}
    </p>
  {/if}
</section>
