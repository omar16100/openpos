<script>
  import { saving } from '../../../shared/records.js';
  import { minorFrom } from '../../../shared/money.js';
  import { idForThisOne, whatIsOnTheForm } from '../../../shared/one_id.js';
  import { label, nameTaken, shared } from '../../../shared/people.js';

  /// Who buys on account, and what they owe.
  ///
  /// Two sections and one book: the shop writes somebody down, sells to them
  /// over a week, and settles up. What they owe is never stored as a balance,
  /// so every figure here comes from the shop rather than from arithmetic done
  /// on this screen.
  ///
  /// The khata page is handed up rather than drawn here, because it prints, and
  /// what prints is the only thing on the page: see the media rule in
  /// screen.css, which hides `main` and leaves the paper. A panel inside `main`
  /// that drew its own would print nothing at all.
  let { t, money, busy, attempt, admin, run, newId, showPaper, announce, refuse } = $props();

  /// Everybody the shop lets buy on account, stopped accounts included.
  let buyers = $state([]);
  let buyerName = $state('');
  let buyerPhone = $state('');
  let buyerBin = $state('');
  let buyerLimit = $state('');
  let editingBuyer = $state(null);
  /// Whether the owner has already been told this name is taken. Told once,
  /// then out of the way: a shop that means it presses again.
  let buyerWarned = $state(false);
  /// The same name twice over, where the cost of confusing two people is a
  /// balance that belongs to neither.
  const buyersTwiceOver = $derived(shared(buyers));

  /// Who owes the shop, a page at a time.
  let owing = $state([]);
  let owedComplete = $state(true);
  const OWED_PAGE = 50;
  const ACCOUNT_PAGE = 50;
  /// Whose account is open, and what is in it.
  let openAccount = $state(null);
  let accountLines = $state([]);
  let accountComplete = $state(true);
  /// What is being typed against each person: money handed over, and why an
  /// amount is being struck off. `paying` is the id each payment will be
  /// recorded under, kept while it is the same payment.
  let handedOver = $state({});
  let paying = $state({});
  let writingOff = $state({});

  export async function everybody(quiet = true) {
    const reply = await attempt(() => admin({ what: 'customers' }, Date.now()), null, quiet);
    if (reply) buyers = reply.info?.every_customer ?? [];
  }

  /// A page of who owes, carrying on from the last one when asked.
  ///
  /// The server pages this rather than cutting it off, so a shop that lets three
  /// hundred families buy on account can read all of them instead of seeing the
  /// first page as though it were the whole list.
  export async function owed(quiet = true, more = false) {
    const from = more && owing.length > 0 ? owing[owing.length - 1] : null;
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'owed',
            limit: OWED_PAGE,
            after_owed_minor: from ? from.owed_minor : 0,
            after_person_key: from ? from.person_key : '',
          },
          Date.now(),
        ),
      null,
      quiet,
    );
    if (!reply) return;
    const page = reply.info?.owed ?? [];
    owing = more ? [...owing, ...page] : page;
    // A short page is the end of the list. Asking again would be one request to
    // be told nothing, every time.
    owedComplete = page.length < OWED_PAGE;
  }

  /// Add somebody who buys on account, or correct them.
  ///
  /// The shop writing a name down is what stops two Karims sharing an account:
  /// a sale that names one of these lands on that person whatever the cashier
  /// typed at the till.
  async function saveBuyer() {
    const name = buyerName.trim();
    if (!name) {
      refuse(t('admin.say_a_name'));
      return;
    }
    // Two records for one person is two accounts: what they took goes on one
    // and what they paid on the other, and neither balance is theirs. Said
    // once, then allowed, because a shop can have two customers of one name and
    // the answer is a name that tells them apart.
    if (nameTaken(buyers, name, editingBuyer?.id ?? null) && !buyerWarned) {
      buyerWarned = true;
      refuse(t('admin.name_already_on_account'));
      return;
    }
    buyerWarned = false;
    // Read by the same parser as every other amount typed on these screens. It
    // was `Number(...) * 100` rounded, which takes "1e3" as a thousand and
    // makes 100.49999999999999 out of 1.005, on the figure that decides how far
    // a cashier may let somebody go.
    const typed = buyerLimit.trim();
    const limit_minor = typed === '' ? 0 : minorFrom(typed);
    if (limit_minor === null) {
      refuse(t('admin.not_a_limit', { typed }));
      return;
    }
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'customer',
            ...saving(editingBuyer, newId, { active: true }),
            name,
            phone: buyerPhone.trim() === '' ? null : buyerPhone.trim(),
            bin: buyerBin.trim() === '' ? null : buyerBin.trim(),
            // Poisha, like every amount that crosses this boundary. An empty
            // box is no cap rather than a cap of nothing.
            limit_minor,
          },
          Date.now(),
        ),
      editingBuyer ? t('admin.buyer_corrected') : t('admin.buyer_written_down'),
    );
    if (!reply) return;
    buyers = reply.info?.every_customer ?? buyers;
    buyerName = '';
    buyerPhone = '';
    buyerBin = '';
    buyerLimit = '';
    editingBuyer = null;
  }

  function correctBuyer(buyer) {
    editingBuyer = buyer;
    buyerName = buyer.name;
    buyerPhone = buyer.phone ?? '';
    buyerBin = buyer.bin ?? '';
    buyerLimit = buyer.limit_minor ? (buyer.limit_minor / 100).toFixed(2) : '';
  }

  /// Stop somebody's account, or let them buy on account again. What they
  /// already owe is untouched: a stopped account is not a settled one.
  async function setAccountAllowed(buyer, allowed) {
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'customer',
            id: buyer.id,
            name: buyer.name,
            phone: buyer.phone ?? null,
            active: allowed,
            // Everything the shop holds about them, not the fields this button
            // is about. A cap left out arrives as nothing, and nothing means no
            // cap: stopping somebody's account and letting them buy again took
            // the owner's limit off, which is the opposite of what the button
            // is for. The BIN is kept by the shop when it is absent; the cap is
            // not, because zero is a real answer.
            bin: buyer.bin ?? null,
            limit_minor: buyer.limit_minor ?? 0,
          },
          Date.now(),
        ),
      allowed ? t('admin.can_buy_again') : t('admin.their_account_stopped'),
    );
    if (reply) buyers = reply.info?.every_customer ?? buyers;
  }

  /// What one person's balance is made of, which is what gets read out when
  /// somebody says they already paid.
  async function showAccount(person) {
    if (openAccount === person.person_key) {
      openAccount = null;
      accountLines = [];
      return;
    }
    await readAccount(person, false);
  }

  /// A page of one person's account, carrying on from the last one when asked.
  async function readAccount(person, more) {
    const from = more && accountLines.length > 0 ? accountLines[accountLines.length - 1] : null;
    const reply = await attempt(
      () =>
        admin(
          {
            what: 'account',
            person_key: person.person_key,
            limit: ACCOUNT_PAGE,
            after_at_ms: from ? from.at_ms : 0,
            after_source_id: from ? from.source : '',
          },
          Date.now(),
        ),
      null,
    );
    if (!reply) return;
    const page = reply.info?.account ?? [];
    openAccount = person.person_key;
    accountLines = more ? [...accountLines, ...page] : page;
    accountComplete = page.length < ACCOUNT_PAGE;
  }

  /// The khata page, for the customer to take away.
  ///
  /// A shop here sells on account all day and settles up weekly. The
  /// conversation is "how much do I owe", and the answer was a number on a
  /// screen the customer cannot take away: a figure they cannot check against
  /// their own memory is a figure they argue about at the counter.
  ///
  /// Every amount on it is what the shop sent. This passes only what a clock
  /// makes, one date per line, because the core has no timezone of its own.
  async function printAccount(person) {
    const reply = await attempt(() =>
      run({
        op: 'statement_paper',
        // Paper is English, whatever the screen is set to. Three reasons and
        // they all point the same way: no ESC/POS code page carries Bangla, so
        // a thermal printer gets English regardless; the layout pads by
        // counting characters, which Bangla defeats, so a Bangla slip comes out
        // ragged; and a shop with two languages on its counter should not have
        // two shapes of receipt in its records. `{}` is the core's own English.
        words: {},
        width: 32,
        customer: person.person_name || person.person_key,
        at: new Date().toLocaleString('en-GB'),
        dates: accountLines.map((line) => new Date(line.at_ms).toLocaleDateString('en-GB')),
      }),
    );
    await showPaper(reply?.view?.receipt ?? null);
  }

  /// Take money off what somebody owes.
  ///
  /// The id is kept while it is the same payment, so pressing again after a
  /// reply that never came records the same one rather than a second, and an
  /// amount corrected before the second press is a different payment rather
  /// than one the shop drops as a repeat.
  async function takePayment(person, writtenOff = false) {
    const typed = (handedOver[person.person_key] ?? '').trim();
    // Parsed from the digits rather than by Number(): that accepts 1e3 and
    // 0.001 and hands back something nobody typed, in the one place on this
    // screen where the number is money.
    const poisha = minorFrom(typed);
    if (poisha === null || poisha <= 0) {
      // Two sentences rather than one, because a payment and a strike-off are
      // two different acts: one is money the shop received and the other is
      // money it will never receive. Both were English, on a screen a shop
      // reads in Bangla, and behind a ternary where the scan could not see
      // them.
      refuse(
        writtenOff ? t('admin.say_how_much_struck_off') : t('admin.say_how_much_handed_over'),
      );
      return;
    }
    const why = (writingOff[person.person_key] ?? '').trim();
    if (writtenOff && !why) {
      refuse(t('admin.say_why_off'));
      return;
    }
    const kept = idForThisOne(
      paying[person.person_key] ?? null,
      whatIsOnTheForm(person.person_key, poisha, why, writtenOff),
      newId,
    );
    paying = { ...paying, [person.person_key]: kept };

    const reply = await attempt(
      () =>
        admin(
          {
            what: 'take_payment',
            id: kept.id,
            person_key: person.person_key,
            person_name: person.person_name,
            amount_minor: poisha,
            at_ms: Date.now(),
            note: writtenOff ? why : null,
            written_off: writtenOff,
          },
          Date.now(),
        ),
      // Said below instead, because the useful confirmation carries what they
      // owe now rather than only that something happened.
      null,
    );
    if (!reply) return;
    // What they owe now, straight from the book rather than from this screen's
    // arithmetic: another till may have sold to them while this was typed.
    const now = reply.info?.owed_now;
    const after = now === undefined || now === null
      ? ''
      : now > 0
        ? t('admin.person_still_owes', { name: person.person_name, amount: money(now) })
        : now < 0
          ? t('admin.person_in_credit', { name: person.person_name, amount: money(-now) })
          : t('admin.person_owes_nothing', { name: person.person_name });
    const said = reply.info?.already_paid
      ? `${t('admin.already_recorded')}${after}`
      : `${writtenOff ? t('admin.struck_off_with_reason') : t('admin.taken_off_owing')}${after}`;
    handedOver = { ...handedOver, [person.person_key]: '' };
    paying = { ...paying, [person.person_key]: null };
    writingOff = { ...writingOff, [person.person_key]: '' };
    // Asked again rather than adjusted here: the book is the answer, and a
    // screen doing its own arithmetic is a second opinion nobody wants.
    await owed(true);
    if (openAccount === person.person_key) {
      openAccount = null;
      await showAccount(person);
    }
    // Last, because every read above clears the last message.
    announce(said);
  }
</script>

  <section>
    <h2>{t('admin.who_buys_on_account')}</h2>
    <p class="why">{t('admin.customers_why')}</p>
    <input bind:value={buyerName} placeholder={t('admin.their_name')} />
    <input bind:value={buyerPhone} placeholder={t('admin.their_phone')} />
    <input
      bind:value={buyerBin}
      placeholder={t('admin.their_bin')}
    />
    <input
      bind:value={buyerLimit}
      placeholder={t('admin.their_limit')}
      inputmode="decimal"
    />
    <p class="why">{t('admin.limit_why')}</p>
    <span class="row">
      <button onclick={saveBuyer} disabled={busy}>
        {editingBuyer ? t('admin.correct_them') : t('admin.write_them_down')}
      </button>
      {#if editingBuyer}
        <button class="quiet" onclick={() => { editingBuyer = null; buyerName = ''; buyerPhone = ''; }}>
          {t('admin.leave_it')}
        </button>
      {/if}
    </span>
    {#if buyers.length > 0}
      <ul class="found">
        {#each buyers as buyer (buyer.id)}
          <li class:retired={!buyer.active}>
            <span class="name">{label(buyer, buyersTwiceOver)}</span>
            <span class="detail">
              {#if buyer.phone}{buyer.phone}{:else}{t('admin.no_phone')}{/if}
              {#if !buyer.active}&middot; {t('admin.account_stopped')}{/if}
            </span>
            <span class="acts">
              <button onclick={() => correctBuyer(buyer)} disabled={busy}>{t('admin.correct_it')}</button>
              {#if buyer.active}
                <button class="quiet" onclick={() => setAccountAllowed(buyer, false)} disabled={busy}>
                  {t('admin.stop_their_account')}
                </button>
              {:else}
                <button class="quiet" onclick={() => setAccountAllowed(buyer, true)} disabled={busy}>
                  {t('admin.let_them_again')}
                </button>
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section>
    <h2>{t('admin.who_owes_you')}</h2>
    <p class="why">{t('admin.owed_why')}</p>
    {#if owing.length > 0}
      <ul class="found">
        {#each owing as person (person.person_key)}
          <li>
            <span class="name">{person.person_name}</span>
            <span class="detail">
              {#if person.owed_minor >= 0}
                {t('admin.owes_amount', { amount: money(person.owed_minor) })}
              {:else}
                {t('admin.in_credit', { amount: money(-person.owed_minor) })}
              {/if}
              &middot; {t('admin.first_entry', {
                date: new Date(person.since_ms).toLocaleDateString('en-GB'),
              })}
              &middot; {t('admin.entries_count', { count: person.entries })}
            </span>
            <span class="row">
              <input
                placeholder={t('admin.taka_handed_over')}
                bind:value={handedOver[person.person_key]}
              />
              <button onclick={() => takePayment(person)} disabled={busy}>{t('admin.took_payment')}</button>
              <button onclick={() => showAccount(person)} disabled={busy}>
                {openAccount === person.person_key ? t('admin.hide') : t('admin.what_is_this')}
              </button>
            </span>
            <span class="row">
              <input
                placeholder={t('admin.strike_off_why')}
                bind:value={writingOff[person.person_key]}
              />
              <button onclick={() => takePayment(person, true)} disabled={busy}>
                {t('admin.strike_off')}
              </button>
            </span>
            {#if openAccount === person.person_key}
              <ul class="found">
                {#each accountLines as line (line.source)}
                  <li>
                    <span class="detail">
                      {new Date(line.at_ms).toLocaleString('en-GB')}
                      &middot; {line.is_sale
                        ? line.amount_minor < 0
                          ? t('admin.brought_goods_back')
                          : t('admin.took_goods')
                        : line.written_off
                          ? t('admin.struck_off')
                          : t('admin.paid')}
                      {money(Math.abs(line.amount_minor))}
                      {#if line.note}&middot; {line.note}{/if}
                    </span>
                  </li>
                {/each}
              </ul>
              {#if !accountComplete}
                <button class="quiet" onclick={() => readAccount(person, true)} disabled={busy}>
                  {t('admin.show_older_entries')}
                </button>
              {/if}
              <!-- What the customer takes away. A page they can check
                   against their own memory, away from the counter, which
                   is where that argument belongs. -->
              <button onclick={() => printAccount(person)} disabled={busy}>
                {t('admin.print_this_account')}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
      {#if !owedComplete}
        <button class="quiet" onclick={() => listOwed(false, true)} disabled={busy}>
          {t('admin.show_more_people')}
        </button>
      {/if}
    {:else}
      <p class="why">{t('admin.nobody_owes_you')}</p>
    {/if}
  </section>
