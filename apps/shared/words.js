/// What the screens say, in the languages a shop reads.
///
/// The first market is Bangladesh, and a cashier at a counter in Jessore does
/// not read English. Until this, every word on every screen was English and the
/// only Bangla anywhere was the item names a shop typed in itself.
///
/// Two kinds of entry, and they come from different places:
///
///   the screen's own words, keyed by short names this file invents, and
///   a refusal from the till, keyed by the code the core froze.
///
/// The second is why the core carries codes at all. Matching on an English
/// sentence to translate it goes quiet the day somebody improves the wording,
/// and a refusal is exactly the moment a cashier needs their own language.
///
/// Anything with no entry falls back to English, and a refusal with no entry
/// falls back to the sentence the till sent: a build whose screen is older than
/// its core says something imperfect rather than nothing.
///
/// The Bangla here is written to be read by a shopkeeper rather than to be
/// literary, and it has not been through a native speaker. That is worth doing
/// before a shop sees it.

/// The languages, in the order a picker offers them.
export const LANGUAGES = [
  { code: 'en', name: 'English' },
  { code: 'bn', name: 'বাংলা' },
];

/// Every word, keyed the way the screen asks for it.
export const WORDS = {
  // ---------------------------------------------------------------- the till
  'till.scan': { en: 'Scan or type a barcode', bn: 'বারকোড স্ক্যান করুন বা লিখুন' },
  'till.scan_to_check': { en: 'Scan to read the price', bn: 'দাম দেখতে স্ক্যান করুন' },
  'till.what_does_this_cost': { en: 'What does this cost?', bn: 'এটার দাম কত?' },
  'till.back_to_scanning': { en: 'Back to scanning', bn: 'স্ক্যানে ফিরুন' },
  'till.nothing_here_goes_in': {
    en: 'Nothing scanned here goes in the basket.',
    bn: 'এখানে স্ক্যান করা কিছু ঝুড়িতে যাবে না।',
  },
  'till.ring_one_up': { en: 'Ring one up', bn: 'একটি ঝুড়িতে দিন' },
  'till.each': { en: 'each', bn: 'প্রতিটি' },
  'till.including_tax': { en: 'including {vat} tax', bn: 'যার মধ্যে {vat} ভ্যাট' },
  'till.no_barcode': { en: 'No barcode? Look it up', bn: 'বারকোড নেই? খুঁজে দেখুন' },
  'till.look_up_placeholder': {
    en: 'Part of the name or the code',
    bn: 'নামের অংশ বা কোড',
  },
  'till.nothing_by_that_name': { en: 'Nothing by that name.', bn: 'এই নামে কিছু নেই।' },
  'till.nothing_rung': { en: 'Nothing rung yet', bn: 'এখনো কিছু তোলা হয়নি' },
  'till.net': { en: 'Net', bn: 'ভ্যাট ছাড়া' },
  'till.vat': { en: 'VAT', bn: 'ভ্যাট' },
  'till.total': { en: 'Total', bn: 'মোট' },
  'till.paid': { en: 'Paid', bn: 'জমা' },
  'till.given_back': { en: 'Given back', bn: 'ফেরত দেওয়া হয়েছে' },
  'till.change': { en: 'Change', bn: 'ফেরত' },
  'till.still_owed': { en: 'Still owed', bn: 'বাকি' },
  'till.discount': { en: 'Discount', bn: 'ছাড়' },
  'till.take_it_off': { en: 'Take it off', bn: 'বাদ দিন' },
  'till.cash_taken': { en: 'Cash taken', bn: 'নেওয়া নগদ' },
  'till.take_cash': { en: 'Take cash', bn: 'নগদ নিন' },
  'till.take_it': { en: 'Take it', bn: 'নিন' },
  'till.exact': { en: 'Exact ({amount})', bn: 'সঠিক ({amount})' },
  'till.cash': { en: 'Cash', bn: 'নগদ' },
  'till.a_wallet': { en: 'A wallet', bn: 'মোবাইল ওয়ালেট' },
  'till.card': { en: 'Card', bn: 'কার্ড' },
  'till.on_account': { en: 'On account', bn: 'বাকিতে' },
  'till.finish_sale': { en: 'Finish sale', bn: 'বিক্রয় শেষ করুন' },
  'till.start_a_refund': { en: 'Start a refund', bn: 'ফেরত শুরু করুন' },
  'till.open_drawer': { en: 'Open drawer', bn: 'ড্রয়ার খুলুন' },
  'till.opening_float': { en: 'Opening float in the drawer', bn: 'ড্রয়ারে শুরুর নগদ' },
  'till.who_is_at_the_till': { en: 'Who is at the till?', bn: 'কাউন্টারে কে আছেন?' },
  'till.enter_your_pin': { en: '{name}, enter your PIN', bn: '{name}, আপনার পিন দিন' },
  'till.sign_in': { en: 'Sign in', bn: 'ঢুকুন' },
  'till.sign_out': { en: '{name}, sign out', bn: '{name}, বেরিয়ে যান' },
  'till.back': { en: 'Back', bn: 'পিছনে' },
  'till.to_send': { en: '{count} to send', bn: 'পাঠানো বাকি {count}' },
  'till.numbers_left': { en: '{count} numbers', bn: '{count} রসিদ নম্বর' },
  'till.on_this_device': { en: 'on this device', bn: 'এই যন্ত্রে রাখা আছে' },
  'till.reached_the_shop': { en: 'reached the shop {at}', bn: 'দোকানে পৌঁছেছে {at}' },
  'till.not_reached': {
    en: 'nothing has reached the shop for {minutes} minutes',
    bn: '{minutes} মিনিট ধরে দোকানে কিছু পৌঁছায়নি',
  },
  'till.enrolment_code': {
    en: 'Enrolment code from the shop owner',
    bn: 'দোকানের মালিকের দেওয়া কোড',
  },
  'till.enrol': { en: 'Enrol', bn: 'যুক্ত করুন' },
  'till.supervisor_pin': { en: "Supervisor's PIN", bn: 'সুপারভাইজারের পিন' },
  'till.allows_it': { en: '{name} allows it', bn: '{name} অনুমতি দিচ্ছেন' },
  'till.leave_it': { en: 'Leave it', bn: 'থাক' },
  'till.needs_a_supervisor': {
    en: 'That needs a supervisor. One of them can allow it here, for this one thing, without signing the cashier out.',
    bn: 'এর জন্য সুপারভাইজার লাগবে। ক্যাশিয়ারকে বের না করেই তিনি শুধু এই কাজটির অনুমতি এখানে দিতে পারেন।',
  },
  'till.language': { en: 'বাংলা', bn: 'English' },

  // The sync line, in the two states somebody at a counter cares about. The
  // protocol's own words for a round (pull, customers, report_drawer) mean
  // nothing there.
  'sync.idle': { en: 'up to date', bn: 'সব পাঠানো হয়েছে' },
  'sync.sending': { en: 'sending', bn: 'পাঠানো হচ্ছে' },
  'sync.reading': { en: 'catching up', bn: 'দোকান থেকে আনা হচ্ছে' },
  'sync.not_reaching': {
    en: 'not reaching the shop: trying again in {seconds}s',
    bn: 'দোকানে পৌঁছাচ্ছে না: আবার চেষ্টা {seconds} সেকেন্ড পরে',
  },
  'sync.held_up': { en: 'held up: {why}', bn: 'আটকে আছে: {why}' },

  // ------------------------------------------------------------- the refusals
  //
  // Keyed by the code the core froze, and covered by a test against
  // refusals.json: a refusal the core can give and this cannot say is a cashier
  // reading English at the one moment it matters.
  'unknown-barcode': {
    en: 'no item in the catalogue has that barcode',
    bn: 'এই বারকোডের কোনো পণ্য তালিকায় নেই',
  },
  'no-longer-sold': {
    en: 'the shop has stopped selling that item',
    bn: 'দোকান এই পণ্যটি আর বিক্রি করে না',
  },
  'nothing-to-hold': {
    en: 'there is nothing on the screen to set aside',
    bn: 'রেখে দেওয়ার মতো কিছু পর্দায় নেই',
  },
  'no-such-held-ticket': {
    en: 'no basket is parked under that ticket',
    bn: 'ওই টিকিটে রাখা কোনো ঝুড়ি নেই',
  },
  'ticket-in-progress': {
    en: 'a basket is already on the screen; close or park it first',
    bn: 'পর্দায় আগে থেকেই একটি ঝুড়ি আছে; আগে সেটি শেষ করুন বা রেখে দিন',
  },
  'no-open-shift': {
    en: 'no drawer is open on this terminal',
    bn: 'এই যন্ত্রে কোনো ড্রয়ার খোলা নেই',
  },
  'nameless-shop': {
    en: 'the shop has no name set, so a receipt would have nothing at the top',
    bn: 'দোকানের নাম দেওয়া নেই, তাই রসিদের উপরে কিছু থাকবে না',
  },
  'nameless-item': {
    en: 'an item needs a name, or its line on the receipt says nothing',
    bn: 'পণ্যের একটি নাম দরকার, নইলে রসিদের লাইনটি কিছুই বলে না',
  },
  'nameless-customer': {
    en: 'somebody buying on account needs a name to write the debt against',
    bn: 'বাকিতে কেনার জন্য একটি নাম দরকার, নইলে বাকিটা কার নামে লেখা হবে',
  },
  'no-barcode-to-find-it-by': {
    en: 'an item written down here needs the barcode that was scanned',
    bn: 'এখানে লেখা পণ্যের সঙ্গে স্ক্যান করা বারকোডটি দরকার',
  },
  'nameless-operator': {
    en: 'that person has the id this device uses to mean nobody',
    bn: 'ওই ব্যক্তির পরিচয় নম্বরটি এই যন্ত্রে "কেউ নয়" বোঝাতে ব্যবহৃত হয়',
  },
  'unknown-customer': {
    en: 'this till has no such customer, or the shop has stopped their account',
    bn: 'এই কাউন্টারে এমন খদ্দের নেই, অথবা দোকান তার বাকির হিসাব বন্ধ করেছে',
  },
  'more-than-the-shelf-holds': {
    en: 'the shop has {on_hand} {name} and this basket wants {wanted}',
    bn: 'দোকানে {name} আছে {on_hand}, আর এই ঝুড়িতে চাওয়া হচ্ছে {wanted}',
  },
  'beyond-their-limit': {
    en: '{name} owes {owed} and you allow {limit}: this would take them to {wanted}',
    bn: '{name}-এর বাকি {owed}, আপনি দেন সর্বোচ্চ {limit}: এতে বাকি দাঁড়াবে {wanted}',
  },
  'write-it-against-them': {
    en: '{name} is written down here: choose them, or this goes on a second account under the same name',
    bn: '{name} এখানে লেখা আছেন: তাঁকে বেছে নিন, নইলে একই নামে দ্বিতীয় একটি হিসাবে উঠবে',
  },
  'no-such-line': { en: 'there is no line {line}', bn: '{line} নম্বর লাইনটি নেই' },
  'empty-basket': { en: 'the basket is empty', bn: 'ঝুড়ি খালি' },
  'mixed-sale-and-return': {
    en: 'a sale and a return cannot share one ticket',
    bn: 'একই রসিদে বিক্রি আর ফেরত একসঙ্গে হয় না',
  },
  'refund-not-settled': {
    en: '{outstanding} of this refund has not been handed over',
    bn: 'এই ফেরতের {outstanding} এখনো দেওয়া হয়নি',
  },
  'discount-above-ceiling': {
    en: 'that is {requested} percent and you may give {ceiling}',
    bn: 'ছাড় চাওয়া হয়েছে {requested} শতাংশ, আপনি দিতে পারেন {ceiling}',
  },
  'price-override-not-allowed': {
    en: 'this person may not type a price over the shop’s',
    bn: 'এই ব্যক্তি দোকানের দামের বদলে নিজে দাম লিখতে পারেন না',
  },
  'negative-price': {
    en: 'a price of {price} would pay the customer',
    bn: '{price} দাম মানে খদ্দেরকে টাকা দেওয়া',
  },
  'underpaid': {
    en: '{short_by} is still owed',
    bn: 'আরও {short_by} বাকি',
  },
  'change-from-a-promise': {
    en: 'that is {over_by} over and only {cash} of it is cash: change is banknotes',
    bn: '{over_by} বেশি নেওয়া হয়েছে আর তার মধ্যে নগদ মাত্র {cash}: ফেরত নগদেই দিতে হয়',
  },
  'unknown-operator': {
    en: 'this till has no such person',
    bn: 'এই কাউন্টারে এমন কেউ নেই',
  },
  'wrong-pin': {
    en: 'wrong PIN: {attempts_left} tries left',
    bn: 'ভুল পিন: আর {attempts_left} বার চেষ্টা করা যাবে',
  },
  'locked-out': {
    en: 'too many wrong PINs: wait before trying again',
    bn: 'বারবার ভুল পিন: কিছুক্ষণ পরে আবার চেষ্টা করুন',
  },
  'not-permitted': {
    en: 'this operator may not do that without a supervisor',
    bn: 'সুপারভাইজার ছাড়া এই কাজটি করা যাবে না',
  },
  'authorisation-expired': {
    en: 'the supervisor’s authorisation has expired',
    bn: 'সুপারভাইজারের অনুমতির সময় শেষ',
  },
  'drawer-already-closed': {
    en: 'that drawer has already been counted and closed',
    bn: 'ওই ড্রয়ার আগেই গোনা হয়ে বন্ধ হয়ে গেছে',
  },
  'drawer-still-open': {
    en: 'the drawer is still open',
    bn: 'ড্রয়ার এখনো খোলা',
  },
  'negative-amount': {
    en: '{amount} is below nothing',
    bn: '{amount} শূন্যের নিচে',
  },
  'no-reason': {
    en: 'cash leaving the drawer needs a reason beside it',
    bn: 'ড্রয়ার থেকে টাকা বেরোলে তার কারণ লিখতে হয়',
  },
  'money': {
    en: 'that arithmetic will not work out',
    bn: 'এই হিসাব মেলানো যাচ্ছে না',
  },
  'journal': {
    en: 'this device could not write that down',
    bn: 'এই যন্ত্র সেটি লিখে রাখতে পারেনি',
  },
  'sync': {
    en: 'the shop and this device could not agree',
    bn: 'দোকান আর এই যন্ত্রের মধ্যে মিল হয়নি',
  },
  'wire': {
    en: 'this device could not read what the shop sent',
    bn: 'দোকান যা পাঠিয়েছে এই যন্ত্র তা পড়তে পারেনি',
  },
};

/// Say something in the language asked for.
///
/// `fill` supplies whatever the phrase names in braces. A phrase this file does
/// not hold falls back to English, and one no language holds falls back to
/// whatever was passed as `otherwise`: a screen older than the core it talks to
/// says something imperfect rather than nothing at all.
export function say(language, key, fill = {}, otherwise = null) {
  const held = WORDS[key];
  const phrase = held?.[language] ?? held?.en ?? otherwise;
  if (phrase === null || phrase === undefined) return key;
  return String(phrase).replace(/\{([a-z_]+)\}/g, (whole, named) =>
    fill[named] === undefined ? whole : String(fill[named]),
  );
}

/// Word a refusal the till gave, from its code and the figures beside it.
///
/// The English sentence the till sent is the fallback, which is what makes a
/// new refusal readable before anybody has translated it.
export function refusal(language, view) {
  if (!view?.error) return null;
  return say(language, view.error_code ?? '', view.error_parts ?? {}, view.error);
}
