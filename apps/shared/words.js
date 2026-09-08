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

  // What the till says when something is wrong, rather than merely refused.
  // Longer than a label, and read at the worst moment of the day.
  'till.device_refused': {
    en: 'The shop is refusing this device. Its terminal may have been removed, or its access withdrawn. Nothing it rings will arrive until it is enrolled again.',
    bn: 'দোকান এই যন্ত্রটিকে আর গ্রহণ করছে না। এর কাউন্টার হয়তো মুছে ফেলা হয়েছে, নয়তো অনুমতি তুলে নেওয়া হয়েছে। আবার যুক্ত না করা পর্যন্ত এখানে তোলা কোনো বিক্রি দোকানে পৌঁছাবে না।',
  },
  'till.device_refused_waiting': {
    en: '{count} sale(s) are still waiting to be sent.',
    bn: '{count} টি বিক্রি এখনো পাঠানো বাকি।',
  },
  'till.nobody_may_authorise': {
    en: 'Nobody on this till may authorise anything. The shop sets that in the back office, under People.',
    bn: 'এই কাউন্টারে কারও অনুমতি দেওয়ার ক্ষমতা নেই। দোকান সেটি ব্যাক অফিসে, "People"-এ ঠিক করে দেয়।',
  },
  'till.nobody_added_yet': {
    en: 'Nobody has been added to this shop yet, so nobody can sign in.',
    bn: 'এই দোকানে এখনো কাউকে যোগ করা হয়নি, তাই কেউ ঢুকতে পারবেন না।',
  },
  'till.unknown_item': {
    en: 'Nothing in the catalogue has the barcode {barcode}. Say what it is and it sells now; the shop sees it as something a till wrote down.',
    bn: 'তালিকায় {barcode} বারকোডের কিছু নেই। এটি কী তা লিখে দিলে এখনই বিক্রি করা যাবে; দোকান দেখবে এটি কাউন্টার থেকে লেখা হয়েছে।',
  },
  'till.what_it_is': { en: 'What it is', bn: 'এটি কী' },
  'till.price_in_taka': { en: 'Price in taka', bn: 'টাকায় দাম' },
  'till.tax_percent': { en: 'Tax %', bn: 'ভ্যাট %' },
  'till.write_it_down_and_sell': {
    en: 'Write it down and sell it',
    bn: 'লিখে রেখে বিক্রি করুন',
  },
  'till.on_their_account': { en: "Put it on {name}'s account", bn: '{name}-এর বাকিতে তুলুন' },
  'till.owes_nothing': { en: 'Owes nothing as of {at}', bn: '{at} পর্যন্ত কোনো বাকি নেই' },
  'till.owes': { en: 'Owes {amount} as of {at}', bn: '{at} পর্যন্ত বাকি {amount}' },
  'till.you_allow_them': { en: 'you allow them {limit}', bn: 'আপনি দেন সর্বোচ্চ {limit}' },
  'till.their_reference': { en: 'Their reference', bn: 'তাঁর রেফারেন্স' },
  'till.which_wallet': { en: 'Which wallet', bn: 'কোন ওয়ালেট' },
  'till.who_owes_it': { en: 'Who owes it', bn: 'কার বাকি' },
  'till.whose_is_it': { en: 'Whose is it?', bn: 'এটি কার?' },
  'till.park_it': { en: 'Park it', bn: 'রেখে দিন' },
  'till.take_that_money_back': { en: 'Take that money back', bn: 'ওই টাকা ফিরিয়ে নিন' },
  'till.give_up_on_this_sale': { en: 'Give up on this sale', bn: 'এই বিক্রি বাতিল করুন' },
  'till.print_again': { en: 'Print again', bn: 'আবার ছাপুন' },
  'till.print_this': { en: 'Print this', bn: 'এটি ছাপুন' },

  // The drawer, which one person counts and another answers for.
  'till.drawer_holds': {
    en: 'Drawer: {sales} sales, should hold',
    bn: 'ড্রয়ার: {sales} টি বিক্রি, থাকার কথা',
  },
  'till.amount': { en: 'Amount', bn: 'টাকা' },
  'till.why': { en: 'Why', bn: 'কেন' },
  'till.in': { en: 'In', bn: 'জমা' },
  'till.out': { en: 'Out', bn: 'বের' },
  'till.counted_cash': { en: 'Counted cash', bn: 'গোনা নগদ' },
  'till.close_drawer': { en: 'Close drawer', bn: 'ড্রয়ার বন্ধ করুন' },
  'till.totals': { en: 'Totals', bn: 'হিসাব' },
  'till.z_report': { en: 'Z report', bn: 'দিনের শেষ হিসাব' },
  'till.totals_so_far': { en: 'Totals so far', bn: 'এ পর্যন্ত হিসাব' },
  'till.sales_count': { en: '{count} sales', bn: '{count} টি বিক্রি' },
  'till.opening_float_line': { en: 'Opening float', bn: 'শুরুর নগদ' },
  'till.not_in_the_till': { en: 'not in the till', bn: 'ড্রয়ারে নেই' },
  'till.cash_in': { en: 'Cash in', bn: 'নগদ জমা' },
  'till.cash_out': { en: 'Cash out', bn: 'নগদ বের' },
  'till.should_hold': { en: 'Should hold', bn: 'থাকার কথা' },
  'till.counted': { en: 'Counted', bn: 'গোনা হয়েছে' },
  'till.exactly_right': { en: 'Exactly right', bn: 'ঠিক মিলেছে' },
  'till.out_by': { en: 'Out by', bn: 'গরমিল' },

  // What the screen itself refuses, before the till is asked. These are the
  // ordinary mis-keys of a day and were the last English left on a Bangla till.
  'till.not_a_quantity': {
    en: 'that is not a quantity: digits, and up to three after a point',
    bn: 'এটি পরিমাণ নয়: সংখ্যা লিখুন, দশমিকের পরে তিন ঘর পর্যন্ত',
  },
  'till.not_a_price': {
    en: 'a price in taka, and not a negative one',
    bn: 'টাকায় দাম লিখুন, ঋণাত্মক নয়',
  },
  'till.not_a_percentage': {
    en: 'a discount is a percentage',
    bn: 'ছাড় শতাংশে লিখতে হয়',
  },
  'till.not_an_amount_off': {
    en: 'an amount off is taka and poisha, and not a negative one',
    bn: 'ছাড়ের টাকা লিখুন টাকা-পয়সায়, ঋণাত্মক নয়',
  },
  'till.could_not_copy': {
    en: 'this browser would not let me copy: select the text below instead',
    bn: 'এই ব্রাউজার কপি করতে দিল না: নিচের লেখাটি নিজে বেছে নিন',
  },
  'till.copied': {
    en: 'Copied. Paste it into the back office, under "Sales carried in by hand".',
    bn: 'কপি হয়েছে। ব্যাক অফিসে "Sales carried in by hand"-এ পেস্ট করুন।',
  },
  'till.count_the_float': {
    en: 'count the float and enter it in taka',
    bn: 'শুরুর নগদ গুনে টাকায় লিখুন',
  },
  'till.an_amount_in_taka': { en: 'enter an amount in taka', bn: 'টাকায় পরিমাণ লিখুন' },
  'till.say_why_cash_moved': {
    en: 'say why the cash moved: an unexplained movement reads as theft later',
    bn: 'টাকা কেন সরানো হলো লিখুন: কারণ ছাড়া সরানো পরে চুরির মতো দেখায়',
  },
  'till.count_the_drawer': {
    en: 'count the drawer and enter what is in it',
    bn: 'ড্রয়ার গুনে যা আছে তা লিখুন',
  },
  'till.say_who_owes_it': {
    en: 'say who owes it: a sale on account with no name cannot be chased',
    bn: 'কার বাকি তা লিখুন: নাম ছাড়া বাকির টাকা আদায় করা যায় না',
  },
  'till.price_is_taka_and_poisha': {
    en: 'a price is taka and poisha',
    bn: 'দাম টাকা-পয়সায় লিখুন',
  },
  // Carrying sales off a device the shop will not take them from. Read at the
  // worst moment there is, by whoever is standing in front of the counter.
  'till.what_is_still_here': {
    en: 'What is still on this device',
    bn: 'এই যন্ত্রে এখনো কী আছে',
  },
  'till.read_them_again': { en: 'Read them again', bn: 'আবার দেখুন' },
  'till.nothing_waiting_here': {
    en: 'Nothing is waiting here. This device can be enrolled again safely.',
    bn: 'এখানে কিছু আটকে নেই। এই যন্ত্রটি নিশ্চিন্তে আবার যুক্ত করা যাবে।',
  },
  'till.carrying_summary': {
    en: '{count} sale(s), {amount} in all.',
    bn: '{count} টি বিক্রি, সব মিলিয়ে {amount}।',
  },
  'till.some_were_salvaged': {
    en: 'Some were read back out of a damaged log and are marked for somebody to check.',
    bn: 'কিছু বিক্রি ক্ষতিগ্রস্ত রেকর্ড থেকে উদ্ধার করা হয়েছে, সেগুলো কাউকে দেখে নিতে হবে।',
  },
  'till.carry_instructions': {
    en: 'Copy the text below and paste it into the back office, under "Sales carried in by hand". Do not wipe this device until the back office says it has them.',
    bn: 'নিচের লেখাটি কপি করে ব্যাক অফিসে "Sales carried in by hand"-এ পেস্ট করুন। ব্যাক অফিস পাওয়ার কথা না বলা পর্যন্ত এই যন্ত্র মুছবেন না।',
  },
  'till.read_from_damaged_log': {
    en: 'read back from a damaged log',
    bn: 'ক্ষতিগ্রস্ত রেকর্ড থেকে উদ্ধার করা',
  },
  'till.carry_mark': {
    en: 'Mark {mark}, {letters} letters. The back office shows the mark of what it received: if the two differ, not all of it arrived.',
    bn: 'চিহ্ন {mark}, {letters} অক্ষর। ব্যাক অফিস যা পেয়েছে তার চিহ্ন দেখায়: দুটি না মিললে সবটা পৌঁছায়নি।',
  },
  'till.save_to_a_file': { en: 'Save it to a file', bn: 'ফাইলে রাখুন' },
  'till.copy_it': { en: 'Copy it', bn: 'কপি করুন' },
  'till.nobody_added_yet_long': {
    en: 'Nobody has been added to this shop yet, so nobody can sign in. That is a different problem from a forgotten PIN, and the owner fixes it.',
    bn: 'এই দোকানে এখনো কাউকে যোগ করা হয়নি, তাই কেউ ঢুকতে পারবেন না। এটি পিন ভুলে যাওয়া নয়, মালিককেই এটি ঠিক করতে হবে।',
  },

  'till.to_refund': { en: 'To refund', bn: 'ফেরত দিতে হবে' },
  'till.parked_still_to_deal_with': {
    en: 'Parked, and still to be dealt with. Nothing here has been rung up or taken money.',
    bn: 'রেখে দেওয়া, এখনো শেষ হয়নি। এগুলোর কোনোটির টাকা নেওয়া হয়নি।',
  },
  'till.lines_count': { en: '{count} line(s)', bn: '{count} টি লাইন' },
  'till.bring_it_back': { en: 'Bring it back', bn: 'ফিরিয়ে আনুন' },
  'till.throw_away': { en: 'Throw away', bn: 'ফেলে দিন' },
  'till.somebody_not_on_the_list': {
    en: 'Somebody not on the list',
    bn: 'তালিকায় নেই এমন কেউ',
  },
  'till.owes_short': { en: 'owes {amount}', bn: 'বাকি {amount}' },
  'till.refund_against_it': { en: 'Refund against it', bn: 'এর বিপরীতে ফেরত' },

  'till.tax_rate_range': {
    en: 'a tax rate is between nothing and a hundred percent',
    bn: 'ভ্যাটের হার শূন্য থেকে একশো শতাংশের মধ্যে',
  },
  'till.owed_unknown': {
    en: 'This till has not been told what they owe yet',
    bn: 'এই কাউন্টার এখনো জানে না তাঁর কত বাকি',
  },
  'till.they_have_not_got_it': { en: 'They have not got it', bn: 'তাঁর কাছে নেই' },
  'till.receipt_on_their_paper': {
    en: 'Receipt on their paper',
    bn: 'তাঁর রসিদের নম্বর',
  },

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

  // ------------------------------------------------------- the back office
  //
  // An owner reads this one, and in a one-room shop the owner is the person at
  // the counter: the same reason the till is translated at all.
  'admin.title': { en: 'openpos back office', bn: 'openpos ব্যাক অফিস' },
  'admin.catalogue_read_to': {
    en: 'catalogue read to {cursor}',
    bn: 'তালিকা পড়া হয়েছে {cursor} পর্যন্ত',
  },
  'admin.may_discard': {
    en: 'this browser may discard what is stored here',
    bn: 'এই ব্রাউজার এখানে রাখা জিনিস মুছে ফেলতে পারে',
  },
  'admin.memory_only': {
    en: 'memory only: nothing survives a reload',
    bn: 'শুধু মেমোরিতে: পাতা রিলোড করলে কিছু থাকবে না',
  },

  'admin.needs_a_code': {
    en: "This device needs an owner's enrolment code. The server prints one when it starts, and an owner can issue more from here afterwards.",
    bn: 'এই যন্ত্রের জন্য মালিকের একটি কোড দরকার। সার্ভার চালু হওয়ার সময় একটি ছাপে, আর তারপর মালিক এখান থেকেই আরও দিতে পারেন।',
  },
  'admin.enrolment_code': { en: 'Enrolment code', bn: 'যুক্ত করার কোড' },
  'admin.enrol': { en: 'Enrol', bn: 'যুক্ত করুন' },

  'admin.the_shop': { en: 'The shop', bn: 'দোকান' },
  'admin.shop_why': {
    en: 'What heads every receipt. A till cannot print without it.',
    bn: 'প্রতিটি রসিদের উপরে যা থাকে। এটি ছাড়া কাউন্টার রসিদ ছাপতে পারে না।',
  },
  'admin.shop_name': { en: 'Shop name', bn: 'দোকানের নাম' },
  'admin.shop_bin': {
    en: 'BIN (leave empty if you have none)',
    bn: 'বিআইএন (না থাকলে খালি রাখুন)',
  },
  'admin.shop_address': { en: 'Address', bn: 'ঠিকানা' },
  'admin.shop_wallets': {
    en: 'Wallets you take, separated by commas: bKash, Nagad',
    bn: 'যেসব ওয়ালেট নেন, কমা দিয়ে: bKash, Nagad',
  },
  'admin.stock_rule': {
    en: 'When a basket asks for more than the shelf holds',
    bn: 'তাকে যত আছে তার বেশি চাইলে',
  },
  'admin.stock_rule_allow': { en: 'Sell it and say nothing', bn: 'বিক্রি করুন, কিছু বলবেন না' },
  'admin.stock_rule_warn': {
    en: 'Sell it and warn the cashier',
    bn: 'বিক্রি করুন, ক্যাশিয়ারকে জানান',
  },
  'admin.stock_rule_block': {
    en: 'Refuse it until a supervisor allows it',
    bn: 'সুপারভাইজার অনুমতি না দেওয়া পর্যন্ত আটকান',
  },
  'admin.stock_rule_why': {
    en: 'Leave this at the first until your stock figures are worth trusting. A shop that has never counted holds none of everything here, and a till that refused on that basis is a till that cannot sell.',
    bn: 'আপনার স্টকের হিসাব বিশ্বাসযোগ্য না হওয়া পর্যন্ত প্রথমটিতেই রাখুন। যে দোকান কখনো গোনেনি, এখানে তার সব কিছুর পরিমাণ শূন্য, আর সেই হিসাবে আটকে দিলে কাউন্টার কিছুই বিক্রি করতে পারবে না।',
  },
  'admin.save_the_shop': { en: 'Save the shop', bn: 'দোকান সংরক্ষণ করুন' },

  'admin.people': { en: 'People', bn: 'কর্মীরা' },
  'admin.people_why': {
    en: 'Nobody can sign in at a till until somebody is added here. A cashier rings sales; a supervisor can also refund, override a price and close the drawer.',
    bn: 'এখানে কাউকে যোগ না করা পর্যন্ত কেউ কাউন্টারে ঢুকতে পারবেন না। ক্যাশিয়ার বিক্রি তোলেন; সুপারভাইজার ফেরত দিতে, দাম বদলাতে আর ড্রয়ার বন্ধ করতেও পারেন।',
  },
  'admin.name': { en: 'Name', bn: 'নাম' },
  'admin.pin': { en: 'PIN, four digits or more', bn: 'পিন, অন্তত চার অঙ্ক' },
  'admin.cashier': { en: 'Cashier', bn: 'ক্যাশিয়ার' },
  'admin.supervisor': { en: 'Supervisor', bn: 'সুপারভাইজার' },
  'admin.correcting_person': {
    en: 'Correcting {name}. Saving the correction leaves their PIN alone. To replace it, type a new one above and set it: a PIN cannot be read back from here or anywhere, which is why it can only be replaced.',
    bn: '{name}-এর তথ্য ঠিক করা হচ্ছে। সংরক্ষণ করলে তাঁর পিন অপরিবর্তিত থাকে। পিন বদলাতে উপরে নতুন একটি লিখে সেট করুন: পিন কোথাও থেকে পড়ে দেখা যায় না, তাই কেবল বদলানোই যায়।',
  },
  'admin.save_the_correction': { en: 'Save the correction', bn: 'সংশোধন সংরক্ষণ করুন' },
  'admin.set_a_new_pin': { en: 'Set a new PIN', bn: 'নতুন পিন দিন' },
  'admin.leave_them_alone': { en: 'Leave them alone', bn: 'থাক' },

  'admin.add_them': { en: 'Add them', bn: 'যোগ করুন' },
  'admin.can_sign_in': { en: 'can sign in', bn: 'ঢুকতে পারেন' },
  'admin.suspended': { en: 'suspended', bn: 'বন্ধ আছে' },
  'admin.correct': { en: 'Correct', bn: 'সংশোধন' },
  'admin.suspend': { en: 'Suspend', bn: 'বন্ধ করুন' },
  'admin.let_them_back_in': { en: 'Let them back in', bn: 'আবার ঢুকতে দিন' },

  'admin.correcting_an_item': { en: 'Correcting an item', bn: 'পণ্য সংশোধন' },
  'admin.something_to_sell': { en: 'Something to sell', bn: 'বিক্রির জন্য কিছু' },
  'admin.item_edit_why': {
    en: 'Saving changes this item everywhere. Tills pick it up on their next pull, and anything already rung keeps the price it was rung at.',
    bn: 'সংরক্ষণ করলে এই পণ্য সব জায়গায় বদলায়। কাউন্টারগুলো পরের বার আনার সময় পায়, আর আগে তোলা বিক্রি যে দামে তোলা হয়েছিল সেই দামেই থাকে।',
  },
  'admin.item_name_bn': {
    en: 'The same in Bangla, if you want it',
    bn: 'চাইলে একই নাম বাংলায়',
  },
  'admin.price_in_taka': { en: 'Price in taka', bn: 'টাকায় দাম' },
  'admin.vat_percent': { en: 'VAT %', bn: 'ভ্যাট %' },
  'admin.what_you_pay': { en: 'What you pay for one', bn: 'একটির জন্য আপনি যা দেন' },
  'admin.cost_why': {
    en: "What you pay is what tells you the day's margin. Leave it empty and a delivery will fill it in: booking goods in sets it to what that delivery charged you.",
    bn: 'আপনি যা দেন তা থেকেই দিনের লাভ বেরোয়। খালি রাখলে পরের চালান এটি পূরণ করে দেবে: মাল তোলার সময় ওই চালানের দামই বসে।',
  },
  'admin.code': { en: 'Code', bn: 'কোড' },
  'admin.barcode': { en: 'Barcode', bn: 'বারকোড' },
  'admin.sold_by': { en: 'Sold by: Nos, kg, litre', bn: 'যেভাবে বিক্রি: পিস, কেজি, লিটার' },
  'admin.what_kind': {
    en: 'What kind of thing this is: rice, oil, soap',
    bn: 'এটি কী জাতীয়: চাল, তেল, সাবান',
  },

  'admin.price_includes_tax': {
    en: 'The price above already includes the tax, as it is written on the shelf',
    bn: 'উপরের দামে ভ্যাট ধরা আছে, তাকে যেমন লেখা থাকে',
  },
  'admin.tax_on_listed_price': {
    en: 'Tax is fixed to the listed price, so a discount comes out of your margin rather than reducing the tax',
    bn: 'ভ্যাট তালিকার দামের উপর বসে, তাই ছাড় দিলে ভ্যাট কমে না, আপনার লাভ থেকেই যায়',
  },
  'admin.kind_of_supply': { en: 'What kind of supply this is', bn: 'এটি কোন ধরনের সরবরাহ' },
  'admin.supply_standard': { en: 'Taxed at the rate above', bn: 'উপরের হারে ভ্যাট' },
  'admin.supply_zero': { en: 'Zero rated', bn: 'শূন্য হারের' },
  'admin.supply_exempt': { en: 'Exempt', bn: 'ভ্যাটমুক্ত' },
  'admin.supply_why': {
    en: 'Zero rated and exempt both charge nothing, and your return puts them in different places. Which of your goods are which is for you and the revenue to settle; this only keeps the answer once you have given it.',
    bn: 'শূন্য হার আর ভ্যাটমুক্ত দুটোতেই ভ্যাট নেই, কিন্তু রিটার্নে দুটো আলাদা জায়গায় যায়। কোন পণ্য কোনটি, তা আপনার আর রাজস্ব বিভাগের বিষয়; এখানে শুধু আপনার দেওয়া উত্তরটি রাখা হয়।',
  },
  'admin.add_it': { en: 'Add it', bn: 'যোগ করুন' },
  'admin.leave_it_alone': { en: 'Leave it alone', bn: 'থাক' },

  // A row of a shop's own spreadsheet that cannot be written, named by what is
  // wrong with it. Named rather than worded for the same reason the till's
  // refusals are: the screen reading them may be in Bangla.
  'file.no-name': { en: 'no name', bn: 'নাম নেই' },
  'file.no-price': { en: 'no price anybody can read', bn: 'পড়ার মতো কোনো দাম নেই' },
  'file.price-below-nothing': { en: 'a price below nothing', bn: 'দাম শূন্যের নিচে' },
  'file.price-too-large': { en: 'a price too large to be one', bn: 'দাম হওয়ার পক্ষে সংখ্যাটি অনেক বড়' },
  'file.vat-unreadable': { en: 'a VAT rate nobody can read', bn: 'পড়ার মতো কোনো ভ্যাটের হার নেই' },
  'file.vat-not-a-rate': { en: 'a VAT rate that is not a rate', bn: 'ভ্যাটের হার হওয়ার মতো সংখ্যা নয়' },
  'file.cost-unreadable': { en: 'a cost nobody can read', bn: 'পড়ার মতো কোনো ক্রয়মূল্য নেই' },
  'file.cost-below-nothing': { en: 'a cost below nothing', bn: 'ক্রয়মূল্য শূন্যের নিচে' },
  'file.cost-too-large': { en: 'a cost too large to be one', bn: 'ক্রয়মূল্য হওয়ার পক্ষে সংখ্যাটি অনেক বড়' },
  'file.same-code-as': { en: 'the same code as line {line}', bn: '{line} নম্বর লাইনের মতো একই কোড' },
  'file.same-barcode-as': {
    en: 'the same barcode as line {line}',
    bn: '{line} নম্বর লাইনের মতো একই বারকোড',
  },

  // Bringing a list in and taking one out.
  'admin.bring_in_a_list': { en: 'Bring in a list you already have', bn: 'আপনার কাছে থাকা তালিকা আনুন' },
  'admin.bring_in_why': {
    en: 'A spreadsheet saved as CSV. The first row has to name the columns: it needs at least name and price, and will use code, barcode, vat, unit, cost and category if they are there. Nothing is written until you have read what it says.',
    bn: 'CSV হিসেবে সংরক্ষণ করা স্প্রেডশিট। প্রথম সারিতে কলামের নাম থাকতে হবে: অন্তত name আর price লাগবে, আর থাকলে code, barcode, vat, unit, cost ও category কাজে লাগবে। আপনি না দেখা পর্যন্ত কিছুই লেখা হয় না।',
  },
  'admin.take_the_list_out': { en: 'Take the list out', bn: 'তালিকা বের করুন' },
  'admin.take_out_why': {
    en: 'Taking it out gives you the same columns this reads back, every row with its code. Change a price in the spreadsheet, bring the file back, and it corrects what is here rather than adding a second copy of your shop.',
    bn: 'বের করলে ঠিক সেই কলামগুলোই পাবেন যেগুলো এটি আবার পড়তে পারে, প্রতিটি সারিতে তার কোডসহ। স্প্রেডশিটে দাম বদলে ফাইলটি ফেরত আনুন, তাতে এখানকার তথ্য সংশোধন হবে, দোকানের দ্বিতীয় কপি তৈরি হবে না।',
  },
  'admin.file_summary': {
    en: '{ready} row(s) can be written, {known} of which you already sell and will be corrected rather than added again.',
    bn: '{ready} টি সারি লেখা যাবে, তার মধ্যে {known} টি আপনি আগে থেকেই বিক্রি করেন, সেগুলো নতুন করে যোগ না হয়ে সংশোধন হবে।',
  },
  'admin.file_refused': {
    en: '{count} row(s) cannot be read and will be left alone.',
    bn: '{count} টি সারি পড়া যাচ্ছে না, সেগুলো বাদ থাকবে।',
  },
  'admin.file_jumped': {
    en: '{count} price(s) move by more than half or double. A shop may well mean that; a formula dragged one row too far looks exactly the same on this screen, so they are listed here first.',
    bn: '{count} টি দাম অর্ধেকের কম বা দ্বিগুণের বেশি বদলাচ্ছে। দোকান সত্যিই তা চাইতে পারে; কিন্তু স্প্রেডশিটে একটি সারি বেশি টেনে দেওয়া ভুলও ঠিক এমনই দেখায়, তাই এগুলো আগে দেখানো হচ্ছে।',
  },
  'admin.file_line': { en: 'Line {line}: {name}', bn: 'লাইন {line}: {name}' },
  'admin.file_becomes': { en: '{was} becomes {now}', bn: '{was} হয়ে যাচ্ছে {now}' },
  'admin.file_and_more': { en: 'and {count} more like those.', bn: 'এবং এমন আরও {count} টি।' },
  'admin.file_and_more_ready': { en: 'and {count} more.', bn: 'এবং আরও {count} টি।' },
  'admin.no_name': { en: 'no name', bn: 'নাম নেই' },
  'admin.already_sold_here': {
    en: 'already sold here, will be corrected',
    bn: 'এখানে আগে থেকেই বিক্রি হয়, সংশোধন হবে',
  },
  'admin.new_row': { en: 'new', bn: 'নতুন' },
  'admin.vat_left_as_is': { en: 'VAT left as it is', bn: 'ভ্যাট যেমন আছে তেমনই' },
  'admin.vat_from_box': {
    en: 'VAT {rate}%, because this file does not say',
    bn: 'ভ্যাট {rate}%, কারণ ফাইলে বলা নেই',
  },
  'admin.vat_of': { en: 'VAT {rate}%', bn: 'ভ্যাট {rate}%' },
  'admin.rate_for_rows': {
    en: 'Tax rate for the rows whose file does not say',
    bn: 'ফাইলে যেসব সারিতে ভ্যাট বলা নেই, তাদের হার',
  },
  'admin.write_rows': { en: 'Write {count} row(s)', bn: '{count} টি সারি লিখুন' },
  'admin.writing_rows': { en: 'Writing {done} of {total}', bn: '{total} টির মধ্যে {done} টি লেখা হচ্ছে' },

  // A receipt somebody brought back, and the sales nobody has answered for.
  'admin.a_receipt_brought_back': {
    en: 'A receipt somebody brought back',
    bn: 'কেউ ফেরত আনা রসিদ',
  },
  'admin.receipt_why': {
    en: 'The number as it is printed on the paper. What comes back is what that till wrote down at the time: the goods, the money, anything waived, and anything given back against it since.',
    bn: 'কাগজে যেমন ছাপা আছে সেই নম্বর। যা দেখানো হবে তা ওই কাউন্টার তখন যা লিখেছিল: পণ্য, টাকা, যা ছাড় দেওয়া হয়েছিল, আর তারপর এর বিপরীতে যা ফেরত দেওয়া হয়েছে।',
  },
  'admin.receipt_number': { en: 'Receipt number, as printed', bn: 'রসিদ নম্বর, যেমন ছাপা আছে' },
  'admin.find_it': { en: 'Find it', bn: 'খুঁজুন' },
  'admin.two_sales_one_number': {
    en: 'Two sales carry {number}. That is a till that rang the same number twice, and both are shown because the person at the counter is owed both.',
    bn: '{number} নম্বরে দুটি বিক্রি আছে। অর্থাৎ একটি কাউন্টার একই নম্বর দুবার দিয়েছে; দুটোই দেখানো হচ্ছে, কারণ কাউন্টারে দাঁড়ানো মানুষটির দুটোই প্রাপ্য।',
  },
  'admin.a_till_not_listed': {
    en: 'a till this shop no longer lists',
    bn: 'এমন একটি কাউন্টার যা দোকানের তালিকায় আর নেই',
  },
  'admin.less': { en: 'less {amount}', bn: '{amount} বাদ' },
  'admin.net': { en: 'net {amount}', bn: 'ভ্যাট ছাড়া {amount}' },
  'admin.vat': { en: 'VAT {amount}', bn: 'ভ্যাট {amount}' },
  'admin.total': { en: 'total {amount}', bn: 'মোট {amount}' },
  'admin.change': { en: 'change {amount}', bn: 'ফেরত {amount}' },
  'admin.gives_back_against': {
    en: 'This one gives back money against {number}.',
    bn: 'এটি {number}-এর বিপরীতে টাকা ফেরত দেয়।',
  },
  'admin.given_back_against_it': {
    en: '{amount} has been given back against it.',
    bn: 'এর বিপরীতে {amount} ফেরত দেওয়া হয়েছে।',
  },
  'admin.held_for': { en: 'Held: {why}', bn: 'আটকে রাখা: {why}' },
  'admin.somebody_answered': { en: 'Somebody answered: {what}', bn: 'কেউ উত্তর দিয়েছেন: {what}' },
  'admin.it_still_counts': { en: 'it still counts', bn: 'এটি এখনো গোনা হচ্ছে' },
  'admin.it_was_struck_out': { en: 'it was struck out', bn: 'এটি বাতিল করা হয়েছে' },
  'admin.cannot_read_that_sale': {
    en: 'This build cannot read what that till wrote. The number, the till, the hour and the money are what the shop knows about it.',
    bn: 'এই সংস্করণ ওই কাউন্টারের লেখা পড়তে পারছে না। নম্বর, কাউন্টার, সময় আর টাকাটুকুই দোকান জানে।',
  },

  'admin.sales_needing_a_look': {
    en: 'Sales needing somebody to look',
    bn: 'যেসব বিক্রি কাউকে দেখতে হবে',
  },
  'admin.repairs_why': {
    en: 'These are stored and counted in your takings until you say otherwise. They are here because the server could not accept them as they stood, and somebody has to say what happened. If a sale is real, keep it: the note records what you checked. If it never happened, say so, and it comes out of your takings, your tax, your stock and anything it put on somebody\u2019s account. Nothing is deleted either way, and you only get to answer once, so read it before you press.',
    bn: 'আপনি অন্য কিছু না বলা পর্যন্ত এগুলো রাখা আছে এবং আপনার আয়ে গোনা হচ্ছে। সার্ভার এগুলো যেভাবে এসেছে সেভাবে নিতে পারেনি, তাই কাউকে বলতে হবে আসলে কী হয়েছিল। বিক্রিটি সত্যি হলে রেখে দিন: আপনার লেখা নোটে থাকবে আপনি কী মিলিয়ে দেখেছেন। কখনো হয়নি বললে সেটি আপনার আয়, ভ্যাট, স্টক আর কারও বাকির হিসাব থেকে বাদ যাবে। কোনোভাবেই কিছু মুছে যায় না, আর উত্তর দেওয়া যায় একবারই, তাই চাপার আগে পড়ে নিন।',
  },
  'admin.no_receipt_number': { en: 'No receipt number', bn: 'রসিদ নম্বর নেই' },
  'admin.reached_the_shop_at': { en: 'reached the shop {at}', bn: 'দোকানে পৌঁছেছে {at}' },
  'admin.what_you_decided': { en: 'What you decided', bn: 'আপনি কী ঠিক করলেন' },

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
