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
  // The same box as the cash one, labelled for what it is when the tender
  // being taken is not cash.
  'till.how_much_taken': { en: 'How much they paid', bn: 'তাঁরা কত দিলেন' },
  'till.say_which_wallet': {
    en: 'say which wallet it came through: the drawer report is read by name',
    bn: 'কোন ওয়ালেটে এসেছে বলুন: ড্রয়ারের হিসাব নাম ধরে পড়া হয়',
  },
  'till.exact': { en: 'Exact ({amount})', bn: 'সঠিক ({amount})' },
  'till.cash': { en: 'Cash', bn: 'নগদ' },
  'till.a_wallet': { en: 'A wallet', bn: 'মোবাইল ওয়ালেট' },
  'till.card': { en: 'Card', bn: 'কার্ড' },
  // The third of the three kinds every shop has. It was missing, so a drawer
  // report with a sale on account in it read "till.credit (not in the till)":
  // the key itself, on the screen a shop counts its money against.
  'till.credit': { en: 'On account', bn: 'বাকিতে' },
  'till.on_account': { en: 'On account', bn: 'বাকিতে' },
  'till.finish_sale': { en: 'Finish sale', bn: 'বিক্রয় শেষ করুন' },
  'till.start_a_refund': { en: 'Start a refund', bn: 'ফেরত শুরু করুন' },
  'till.open_drawer': { en: 'Open drawer', bn: 'ড্রয়ার খুলুন' },
  'till.try_now': { en: 'Try now', bn: 'এখনই চেষ্টা করুন' },
  // The button that starts a drawer for the day, which is a different act with
  // a similar name: it was called "Open drawer" and the drawer stayed shut.
  'till.start_the_drawer': { en: 'Start the drawer', bn: 'ড্রয়ার চালু করুন' },
  'till.opening_float': { en: 'Opening float in the drawer', bn: 'ড্রয়ারে শুরুর নগদ' },
  'till.who_is_at_the_till': { en: 'Who is at the till?', bn: 'কাউন্টারে কে আছেন?' },
  'till.enter_your_pin': { en: '{name}, enter your PIN', bn: '{name}, আপনার পিন দিন' },
  'till.sign_in': { en: 'Sign in', bn: 'ঢুকুন' },
  'till.sign_out': { en: '{name}, sign out', bn: '{name}, বেরিয়ে যান' },
  'till.back': { en: 'Back', bn: 'পিছনে' },
  'till.to_send': { en: '{count} to send', bn: 'পাঠানো বাকি {count}' },
  'till.numbers_left': { en: '{count} numbers', bn: '{count} রসিদ নম্বর' },
  'till.on_this_device': { en: 'on this device', bn: 'এই যন্ত্রে রাখা আছে' },
  // The states of the ledger that are not a place to keep things: the moment
  // before it opens, a till that has not been told who it is, and the one that
  // matters, which is a ledger that would not open at all. These were the
  // words the code uses, printed as they are, so a Bangla till said
  // "unavailable" in English in exactly the state a shopkeeper needs to read.
  // The shop has asked this till to warn or refuse on the shelf, and the till
  // has not been round its own figures yet, so it is saying nothing.
  'till.learning_the_shelf': { en: 'learning the shelf', bn: 'তাক কী আছে শিখছে' },
  'till.learning_the_shelf_why': {
    en: 'This till learns what the shelves hold a few hundred items at a time. Until it has been round once it says nothing about the shelf, because a figure nobody has sent it looks the same as none.',
    bn: 'এই কাউন্টার তাকের হিসাব একবারে কয়েকশো পণ্য করে শেখে। একবার পুরো ঘুরে আসার আগে তাক নিয়ে কিছু বলে না, কারণ যে হিসাব এখনো আসেনি আর শূন্য হিসাব দেখতে একরকম।',
  },
  // The refund built from the paper rather than by scanning the goods again.
  'till.what_was_on_this_one': {
    en: 'What was on that receipt. Say how much of each is coming back.',
    bn: 'ওই রসিদে যা ছিল। প্রতিটির কতটা ফেরত আসছে লিখুন।',
  },
  'till.charged_each': { en: '{qty} at {each}', bn: '{qty} টি, প্রতিটি {each}' },
  'till.came_off_it': { en: '{amount} came off', bn: '{amount} ছাড় ছিল' },
  'till.how_many_coming_back': { en: 'how many coming back', bn: 'কতটা ফেরত আসছে' },
  'till.bring_these_back': { en: 'Bring these back', bn: 'এগুলো ফেরত নিন' },
  // A refund started by mistake. There was no way out of one: the till stayed
  // in refund mode with nothing on the ticket, every scan came back as goods
  // returning, and the only escape was to reload the page.
  'till.not_a_refund_after_all': {
    en: 'Not a refund after all',
    bn: 'ফেরত নয়, বাতিল করুন',
  },
  // The number may be mistyped, or the till that rang it may not have reached
  // the shop yet. Either way the cashier is about to scan the goods instead,
  // which prices them at today's catalogue rather than at what was paid.
  'till.no_such_receipt_here': {
    en: 'The shop has no receipt {number}. Scan what is coming back, and check the number on the paper.',
    bn: 'দোকানে {number} নম্বরের কোনো রসিদ নেই। যা ফেরত আসছে তা স্ক্যান করুন, আর কাগজের নম্বরটি মিলিয়ে দেখুন।',
  },
  // Part of this receipt has already come back. The shop refuses more than the
  // whole of it, but that refusal arrives after the money has left the drawer.
  'till.already_given_back': {
    en: '{amount} has already been given back against this receipt.',
    bn: 'এই রসিদের বিপরীতে ইতিমধ্যে {amount} ফেরত দেওয়া হয়েছে।',
  },
  'till.scan_them_instead': { en: 'Scan them instead', bn: 'বরং স্ক্যান করুন' },
  'till.storage_opening': { en: 'opening', bn: 'খোলা হচ্ছে' },
  'till.storage_unavailable': { en: 'nowhere to keep this', bn: 'রাখার জায়গা মিলছে না' },
  'till.storage_not_enrolled': { en: 'not set up yet', bn: 'এখনো চালু করা হয়নি' },
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
  'till.tap_to_change': {
    en: 'Tap to change how many, or take it off',
    bn: 'কতগুলো বদলাতে বা বাদ দিতে চাপ দিন',
  },
  // What came off a line. The rate and the amount are kept apart on purpose: a
  // ticket discount is shared across the lines, so a line's amount is larger
  // than its own rate accounts for, and reading the two as one fact is a
  // cashier's phone call to the owner.
  'till.off_this_line_at_rate': {
    en: '{rate}% off this line, {off} in all',
    bn: 'এই লাইনে {rate}% ছাড়, সব মিলিয়ে {off}',
  },
  'till.off_this_line': { en: '{own} off this line', bn: 'এই লাইনে {own} ছাড়' },
  'till.off_this_line_in_all': {
    en: '{own} off this line, {off} in all',
    bn: 'এই লাইনে {own} ছাড়, সব মিলিয়ে {off}',
  },
  'till.share_of_ticket_discount': {
    en: "{off}, this line's share of the ticket discount",
    bn: '{off}, পুরো বিলের ছাড়ের মধ্যে এই লাইনের ভাগ',
  },
  // Under the line in the basket, where a cashier reads it with a customer
  // standing there. It was built as a sentence in the screen and so was English
  // whatever the shop had chosen.
  'till.shelf_short': {
    en: 'the shop has {on_hand}, this wants {wanted}',
    bn: 'দোকানে আছে {on_hand}, এখানে চাওয়া হচ্ছে {wanted}',
  },
  // A name inside a sentence, so it cannot be a sentence built around a
  // variable in the markup: Bangla puts the words in another order.
  'till.write_them_down': { en: 'Write {name} down', bn: '{name} কে খাতায় লিখুন' },
  'till.needs_a_supervisor': {
    en: 'That needs a supervisor. One of them can allow it here, for this one thing, without signing the cashier out.',
    bn: 'এর জন্য সুপারভাইজার লাগবে। ক্যাশিয়ারকে বের না করেই তিনি শুধু এই কাজটির অনুমতি এখানে দিতে পারেন।',
  },

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
  // "Whose is it?" beside a text box reads as the customer on the sale, which
  // is what it was mistaken for while walking this screen. It is the label a
  // parked basket is found again by: a name, a table number, "the man in the
  // blue shirt". Saying so costs four words and saves somebody typing a
  // customer's name into a box that will not put it on the sale.
  'till.whose_is_it': {
    en: 'A name to find it by',
    bn: 'কোন নামে খুঁজে পাবেন',
  },
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
  'till.on_this_device_not_promised': {
    en: 'on this device, not promised',
    bn: 'এই যন্ত্রে আছে, তবে নিশ্চয়তা নেই',
  },
  'till.memory_only': {
    en: 'memory only',
    bn: 'শুধু মেমরিতে',
  },
  'till.new_build_waiting': {
    en: 'a new version is ready',
    bn: 'নতুন সংস্করণ প্রস্তুত',
  },
  'till.new_build_waiting_why': {
    en: 'It starts as soon as there is no basket on the screen and nothing waiting to be sent, so it cannot reload under you mid-sale.',
    bn: 'পর্দায় কোনো ঝুড়ি না থাকলে আর পাঠানোর কিছু বাকি না থাকলেই এটি চালু হবে, যাতে বিক্রির মাঝখানে পর্দা রিলোড না হয়।',
  },
  'till.saved_as_file': {
    en: 'Saved as {name}. Do not wipe this device until the back office has taken them in.',
    bn: '{name} নামে রাখা হয়েছে। ব্যাক অফিস নিয়ে না নেওয়া পর্যন্ত এই যন্ত্র মুছবেন না।',
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
  // Before the first round has been run, which is a second or two and is not
  // "up to date": nothing has been sent or asked for yet. It was the English
  // word "idle" on a Bangla till, written straight into the screen rather than
  // asked for here, and it was the first thing a shopkeeper saw on opening.
  'sync.starting': { en: 'starting up', bn: 'চালু হচ্ছে' },
  'sync.idle': { en: 'up to date', bn: 'সব পাঠানো হয়েছে' },
  'sync.sending': { en: 'sending', bn: 'পাঠানো হচ্ছে' },
  'sync.reading': { en: 'catching up', bn: 'দোকান থেকে আনা হচ্ছে' },
  'sync.not_reaching': {
    en: 'not reaching the shop: trying again in {seconds}s',
    bn: 'দোকানে পৌঁছাচ্ছে না: আবার চেষ্টা {seconds} সেকেন্ড পরে',
  },
  'sync.held_up': { en: 'held up: {why}', bn: 'আটকে আছে: {why}' },
  // The commonest failure there is, and the one a browser words itself. What a
  // shop reads on the first failure of an outage should be the shop's own
  // language, not "Failed to fetch" inside a Bangla sentence.
  'sync.cannot_reach_the_shop': {
    en: 'not reaching the shop',
    bn: 'দোকানে পৌঁছাচ্ছে না',
  },

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
  // Beside the rule, because somebody who turns it on walks to the counter to
  // check. A till learns the shelf a few hundred items at a time, and until it
  // has been round once it says nothing: what looks like a rule that does not
  // work is a till waiting for figures.
  'admin.stock_rule_takes_a_while': {
    en: 'A till starts doing this once it has been round its own shelf figures, which takes a few minutes on a shop this size and longer on a big one. Until then it sells and says nothing.',
    bn: 'কাউন্টার নিজের তাকের হিসাব একবার পুরো ঘুরে আসার পর এটি করা শুরু করে; এই আকারের দোকানে তাতে কয়েক মিনিট লাগে, বড় দোকানে আরও বেশি। তার আগে পর্যন্ত বিক্রি করে যায়, কিছু বলে না।',
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
  'file.inclusive-unreadable': {
    en: 'a price rule this shop cannot read: yes or no, whether the price has the tax in it',
    bn: 'দামের নিয়মটি পড়া যাচ্ছে না: দামে ভ্যাট ধরা আছে কি না, হ্যাঁ বা না',
  },
  'file.supply-unreadable': {
    en: 'a supply this shop cannot read: standard, zero rated, or exempt',
    bn: 'সরবরাহের ধরন পড়া যাচ্ছে না: standard, zero rated, বা exempt',
  },
  // Why a file cannot be read at all, and why it is too early to read one. Both
  // come back from apps/shared/catalogue_file.js as keys: that file is shared
  // code and cannot know what language the shop reads, and it used to answer
  // with English prose that a Bangla back office showed as it stood.
  'file.nothing-in-it': {
    en: 'that file has nothing in it',
    bn: 'ওই ফাইলে কিছুই নেই',
  },
  'file.headings-needed': {
    en: 'the first row has to name the columns, and it needs at least a name and a price: try "name,price,code,barcode,vat,unit,cost,category"',
    bn: 'প্রথম সারিতে কলামের নাম থাকতে হবে, আর অন্তত নাম আর দাম লাগবেই: "name,price,code,barcode,vat,unit,cost,category" দিয়ে দেখুন',
  },
  'file.too-early-not-reaching-in': {
    en: 'this device cannot reach the shop just now, so what it holds may be behind. Wait until the line at the top says it has reached the shop, then bring the list in again: anything it has not read yet would be added a second time.',
    bn: 'এই যন্ত্র এখন দোকানে পৌঁছাতে পারছে না, তাই এর কাছে যা আছে তা পুরনো হতে পারে। উপরের লাইনে দোকানে পৌঁছেছে বলা পর্যন্ত অপেক্ষা করে আবার তালিকা আনুন: যা এখনো পড়া হয়নি তা দ্বিতীয়বার যোগ হয়ে যাবে।',
  },
  'file.too-early-not-reaching-out': {
    en: 'this device cannot reach the shop just now, so what it holds may be behind. Wait until the line at the top says it has reached the shop, then take the list out again: the list would be missing whatever it has not read.',
    bn: 'এই যন্ত্র এখন দোকানে পৌঁছাতে পারছে না, তাই এর কাছে যা আছে তা পুরনো হতে পারে। উপরের লাইনে দোকানে পৌঁছেছে বলা পর্যন্ত অপেক্ষা করে আবার তালিকা বের করুন: নয়তো যা পড়া হয়নি তা তালিকায় থাকবে না।',
  },
  'file.too-early-never-read-in': {
    en: 'this device has not read the shop yet. Wait for the line at the top to say it has reached the shop, then bring the list in again: anything it has not read yet would be added a second time.',
    bn: 'এই যন্ত্র এখনো দোকান পড়েনি। উপরের লাইনে দোকানে পৌঁছেছে বলা পর্যন্ত অপেক্ষা করে আবার তালিকা আনুন: যা এখনো পড়া হয়নি তা দ্বিতীয়বার যোগ হয়ে যাবে।',
  },
  'file.too-early-never-read-out': {
    en: 'this device has not read the shop yet. Wait for the line at the top to say it has reached the shop, then take the list out again: the list would be missing whatever it has not read.',
    bn: 'এই যন্ত্র এখনো দোকান পড়েনি। উপরের লাইনে দোকানে পৌঁছেছে বলা পর্যন্ত অপেক্ষা করে আবার তালিকা বের করুন: নয়তো যা পড়া হয়নি তা তালিকায় থাকবে না।',
  },
  'file.too-early-still-reading-in': {
    en: 'this device is still reading the shop’s catalogue. Wait for it to finish, then bring the list in again: anything it has not read yet would be added a second time.',
    bn: 'এই যন্ত্র এখনো দোকানের তালিকা পড়ছে। শেষ হওয়া পর্যন্ত অপেক্ষা করে আবার তালিকা আনুন: যা এখনো পড়া হয়নি তা দ্বিতীয়বার যোগ হয়ে যাবে।',
  },
  'file.too-early-still-reading-out': {
    en: 'this device is still reading the shop’s catalogue. Wait for it to finish, then take the list out again: the list would be missing whatever it has not read.',
    bn: 'এই যন্ত্র এখনো দোকানের তালিকা পড়ছে। শেষ হওয়া পর্যন্ত অপেক্ষা করে আবার তালিকা বের করুন: নয়তো যা পড়া হয়নি তা তালিকায় থাকবে না।',
  },
  'file.same-code-as': { en: 'the same code as line {line}', bn: '{line} নম্বর লাইনের মতো একই কোড' },
  'file.same-barcode-as': {
    en: 'the same barcode as line {line}',
    bn: '{line} নম্বর লাইনের মতো একই বারকোড',
  },

  // Bringing a list in and taking one out.
  'admin.bring_in_a_list': { en: 'Bring in a list you already have', bn: 'আপনার কাছে থাকা তালিকা আনুন' },
  'admin.bring_in_why': {
    en: 'A spreadsheet saved as CSV. The first row has to name the columns: it needs at least name and price, and will use code, barcode, vat, unit, cost, category, supply and price includes vat if they are there. Supply is standard, zero rated or exempt. Nothing is written until you have read what it says.',
    bn: 'CSV হিসেবে সংরক্ষণ করা স্প্রেডশিট। প্রথম সারিতে কলামের নাম থাকতে হবে: অন্তত name আর price লাগবে, আর থাকলে code, barcode, vat, unit, cost, category, supply ও price includes vat কাজে লাগবে। supply হলো standard, zero rated বা exempt। আপনি না দেখা পর্যন্ত কিছুই লেখা হয় না।',
  },
  'admin.price_has_vat_in_it': {
    en: 'the price has the tax in it',
    bn: 'দামে ভ্যাট ধরা আছে',
  },
  'admin.too_many_to_match': {
    en: 'this shop has more than {count} lines, which is more than this device can match a file against in one go. Bringing a list in would add a second copy of everything past that.',
    bn: 'এই দোকানে {count}-এর বেশি পণ্য আছে, যা এই যন্ত্র একবারে মিলিয়ে দেখতে পারে না। তালিকা আনলে তার বেশি যা আছে তার দ্বিতীয় কপি তৈরি হবে।',
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

  'admin.already_decided': { en: 'What you have already decided', bn: 'আপনি যা আগেই ঠিক করেছেন' },
  'admin.decided_why': {
    en: 'An answered sale leaves the queue, so this is the way back to one you answered wrongly. Striking out the wrong sale takes a real debt off somebody\u2019s account, and putting it back puts the debt back with it. Both answers are kept, so the record shows that you changed your mind and why.',
    bn: 'উত্তর দেওয়া বিক্রি তালিকা থেকে সরে যায়, তাই ভুল উত্তর দিলে এখান দিয়েই ফেরা যায়। ভুল বিক্রি বাতিল করলে কারও বাকির হিসাব থেকে সত্যিকারের টাকা কমে যায়, আর ফিরিয়ে আনলে সেই বাকিও ফিরে আসে। দুটি উত্তরই রাখা থাকে, তাই রেকর্ডে থাকে আপনি মত বদলেছেন এবং কেন।',
  },
  'admin.hide_them': { en: 'Hide them', bn: 'লুকান' },
  'admin.show_what_was_decided': { en: 'Show what was decided', bn: 'কী ঠিক হয়েছে দেখুন' },
  'admin.nothing_decided_yet': { en: 'Nothing has been decided yet.', bn: 'এখনো কিছু ঠিক করা হয়নি।' },
  'admin.counts': { en: 'counts', bn: 'গোনা হচ্ছে' },
  'admin.struck_out': { en: 'struck out', bn: 'বাতিল' },
  'admin.answered_times': { en: 'answered {count} times', bn: '{count} বার উত্তর দেওয়া হয়েছে' },
  'admin.why_answer_changing': {
    en: 'Why the answer is changing',
    bn: 'উত্তর কেন বদলাচ্ছে',
  },
  'admin.it_never_happened': { en: 'It never happened', bn: 'এটি কখনো হয়নি' },
  'admin.put_it_back': { en: 'Put it back', bn: 'ফিরিয়ে আনুন' },

  'admin.numbering_jumps': { en: 'Where your numbering jumps', bn: 'রসিদ নম্বরে যেখানে ফাঁক' },
  'admin.gaps_why': {
    en: 'Receipt numbers are meant to run unbroken, and this is where they do not. A gap is one of two things and only you can tell which: numbers rung on a till that has not synced yet, which close by themselves, or numbers that went with a device that was wiped or lost, which never will. Check the till against the list above, and if it has been quiet for days, that is your answer.',
    bn: 'রসিদ নম্বর ভাঙা ছাড়া চলার কথা, এখানে তা চলেনি। ফাঁক দুরকম, আর কোনটি তা কেবল আপনিই বলতে পারেন: যে কাউন্টার এখনো সব পাঠায়নি তার নম্বর, যা নিজেই ভরে যাবে; অথবা মুছে ফেলা বা হারিয়ে যাওয়া যন্ত্রের সঙ্গে চলে যাওয়া নম্বর, যা আর কখনো ভরবে না। উপরের তালিকার সঙ্গে কাউন্টারটি মিলিয়ে দেখুন; কয়েক দিন চুপ থাকলে সেটিই উত্তর।',
  },
  'admin.numbers_missing': { en: '{count} number(s) missing', bn: '{count} টি নম্বর নেই' },

  'admin.carried_in_by_hand': { en: 'Sales carried in by hand', bn: 'হাতে করে আনা বিক্রি' },
  'admin.carried_why': {
    en: 'For a till that cannot send: its terminal was removed, or it has to be enrolled again and would abandon what it is holding. On that device press "What is still on this device", then either save it to a file and open the file here, or paste what it shows. Line breaks a message added on the way do not matter. A sale taken in this way goes into the list of sales needing somebody to look, unless the shop already had it, because the usual proof of where a sale came from is what that device has lost.',
    bn: 'যে কাউন্টার পাঠাতে পারছে না তার জন্য: তার টার্মিনাল মুছে ফেলা হয়েছে, বা আবার যুক্ত করতে হবে আর তাতে ধরে রাখা বিক্রিগুলো হারিয়ে যাবে। ওই যন্ত্রে "এই যন্ত্রে এখনো কী আছে" চাপুন, তারপর হয় ফাইলে রেখে সেই ফাইল এখানে খুলুন, নয়তো যা দেখাচ্ছে তা পেস্ট করুন। পথে যোগ হওয়া লাইনব্রেকে কিছু যায় আসে না। এভাবে নেওয়া বিক্রি "কাউকে দেখতে হবে" তালিকায় যায়, যদি না দোকানে সেটি আগে থেকেই থাকে, কারণ বিক্রিটি কোথা থেকে এসেছে তার স্বাভাবিক প্রমাণটিই ওই যন্ত্র হারিয়েছে।',
  },

  'admin.items_tills_wrote': { en: 'Items your tills wrote down', bn: 'কাউন্টার থেকে লেখা পণ্য' },
  'admin.from_tills_why': {
    en: 'Somebody at a counter scanned a barcode this shop had never seen, said what it was, and sold it rather than losing the sale. They are in the catalogue and in every report already. Correct what is wrong, or say it is right and the mark comes off.',
    bn: 'কাউন্টারে কেউ এমন একটি বারকোড স্ক্যান করেছেন যা দোকান আগে দেখেনি, সেটি কী তা লিখে বিক্রিটি হাতছাড়া না করে সেরে ফেলেছেন। এগুলো এরই মধ্যে তালিকায় ও সব হিসাবে আছে। ভুল থাকলে ঠিক করুন, নয়তো ঠিক আছে বললে চিহ্নটি উঠে যাবে।',
  },
  'admin.no_barcode_code_taken': {
    en: 'no barcode: the shop already gave that code to something else',
    bn: 'বারকোড নেই: ওই কোড দোকান আগেই অন্য কিছুকে দিয়েছে',
  },
  'admin.correct_it': { en: 'Correct it', bn: 'সংশোধন করুন' },
  'admin.it_is_right': { en: 'It is right', bn: 'এটি ঠিক আছে' },

  'admin.changes_never_reached': {
    en: 'Price changes that never reached your tills',
    bn: 'যেসব দামের পরিবর্তন কাউন্টারে পৌঁছায়নি',
  },
  'admin.unreadable_why': {
    en: 'Written by a version of this software that this one cannot read, so every till has passed over them and is selling at the price it had before. "Send the whole list to the tills again", under what is on the shelves, is the way out, and it costs nothing: a till takes the prices it already has as they are.',
    bn: 'এই সফটওয়্যারের এমন একটি সংস্করণ থেকে লেখা যা এটি পড়তে পারে না, তাই প্রতিটি কাউন্টার সেগুলো বাদ দিয়ে আগের দামেই বিক্রি করছে। "তাকে যা আছে"-এর নিচে "পুরো তালিকা কাউন্টারে আবার পাঠান" চাপলেই সমাধান, আর তাতে কিছু নষ্ট হয় না: কাউন্টারে যে দাম আছে সেটিই থাকে।',
  },
  'admin.send_the_list_again': {
    en: 'Send the whole list to the tills again',
    bn: 'পুরো তালিকা কাউন্টারে আবার পাঠান',
  },
  'admin.list_sent_again': {
    en: '{count} item(s) sent to the tills again. Each till takes them on its next round.',
    bn: '{count} টি পণ্য কাউন্টারে আবার পাঠানো হয়েছে। প্রতিটি কাউন্টার পরের বারেই সেগুলো নেবে।',
  },
  'admin.no_name_for_item': {
    en: 'An item this device does not have a name for',
    bn: 'এই যন্ত্রে যার নাম নেই এমন একটি পণ্য',
  },
  'admin.written_by_version': {
    en: 'written by version {schema} of the catalogue format',
    bn: 'তালিকার {schema} নম্বর সংস্করণে লেখা',
  },

  'admin.what_sold': { en: 'What sold', bn: 'কী বিক্রি হয়েছে' },
  'admin.sold_why': {
    en: 'What left the shelves between two days, most first. This is what to order against: something given away at a discount still left the shelf and still has to be replaced. Returns are in it with their own sign.',
    bn: 'দুই তারিখের মধ্যে তাক থেকে যা গেছে, বেশি আগে। এর উপরেই মাল আনার হিসাব: ছাড়ে দেওয়া জিনিসও তাক থেকে গেছে এবং আবার আনতে হবে। ফেরত নিজের চিহ্নসহ এতেই আছে।',
  },
  'admin.look': { en: 'Look', bn: 'দেখুন' },
  'admin.waived_count': {
    en: '{count} thing(s) were allowed over a cashier\u2019s ceiling in that window.',
    bn: 'ওই সময়ে {count} টি ক্ষেত্রে ক্যাশিয়ারের সীমার বেশি অনুমতি দেওয়া হয়েছে।',
  },
  'admin.waived_why': {
    en: 'A ceiling exists so that giving money away is somebody\u2019s decision rather than everybody\u2019s habit, which only means anything if the decisions can be looked at afterwards.',
    bn: 'সীমা রাখা হয় যাতে টাকা ছেড়ে দেওয়া সবার অভ্যাস না হয়ে কারও সিদ্ধান্ত হয়; আর সেটির মানে থাকে কেবল তখনই, যখন সিদ্ধান্তগুলো পরে দেখা যায়।',
  },
  'admin.on_a_sale_of': { en: 'on a sale of {amount}', bn: '{amount} টাকার বিক্রিতে' },

  'admin.what_to_buy': { en: 'What to buy.', bn: 'কী আনতে হবে।' },
  'admin.what_to_buy_why': {
    en: 'How long each shelf lasts at the rate above, shortest first. How much to order is yours: it depends on when your supplier comes and what is in the drawer.',
    bn: 'উপরের হারে প্রতিটি তাক আর কত দিন চলবে, কমটি আগে। কতটা আনবেন তা আপনার সিদ্ধান্ত: সেটি নির্ভর করে সরবরাহকারী কবে আসেন আর ড্রয়ারে কত আছে তার উপর।',
  },
  'admin.nothing_left': { en: 'nothing left', bn: 'কিছু নেই' },
  'admin.left_and_days': { en: '{qty} left', bn: '{qty} আছে' },
  'admin.about_under_a_day': { en: 'about under a day', bn: 'এক দিনেরও কম' },
  'admin.about_days': { en: 'about {days} days', bn: 'প্রায় {days} দিন' },
  'admin.sold_over_window': { en: '{qty} sold over that window', bn: 'ওই সময়ে বিক্রি {qty}' },
  'admin.nothing_close_to_out': {
    en: 'Nothing is that close to running out. Ask for more days if you are going anyway.',
    bn: 'কিছুই ফুরানোর এত কাছে নয়। তবু যদি যেতেই হয়, বেশি দিনের হিসাব দেখুন।',
  },
  'admin.not_moving': { en: 'What is not moving.', bn: 'যা নড়ছে না।' },
  'admin.not_moving_why': {
    en: 'On the shelf and not sold at all over those days, at what you paid for it. This is money you cannot spend on what does sell.',
    bn: 'তাকে আছে অথচ ওই দিনগুলোতে একটিও বিক্রি হয়নি, আপনার কেনা দামে। এটি এমন টাকা যা যা বিক্রি হয় তার পেছনে খাটাতে পারছেন না।',
  },
  'admin.something_unnamed': {
    en: 'Something this device does not have a name for',
    bn: 'এমন কিছু যার নাম এই যন্ত্রে নেই',
  },
  'admin.on_the_shelf': { en: '{qty} on the shelf', bn: 'তাকে {qty}' },
  'admin.of_your_money': { en: '{amount} of your money', bn: 'আপনার {amount} টাকা' },
  'admin.cost_not_said': {
    en: 'you have not said what this costs you',
    bn: 'এটির ক্রয়মূল্য আপনি বলেননি',
  },
  'admin.dead_stock_total': {
    en: '{amount} in all, over {count} thing(s).',
    bn: 'সব মিলিয়ে {amount}, {count} টি জিনিসে।',
  },
  'admin.over_sales': { en: 'over {count} sale(s)', bn: '{count} টি বিক্রিতে' },

  'admin.what_was_allowed': { en: 'What was allowed, and by whom', bn: 'কী অনুমতি পেয়েছে, আর কার' },
  'admin.allowed_why': {
    en: 'Every discount over a ceiling, price typed over the catalogue\u2019s, refund, line taken off and drawer opened outside a sale, with who did it and who allowed it. A ceiling only means something if what got past it can be looked at afterwards, and until this existed the answer lived on the device and died when the tab closed.',
    bn: 'সীমার বেশি প্রতিটি ছাড়, তালিকার দামের বদলে হাতে লেখা দাম, ফেরত, বাদ দেওয়া লাইন আর বিক্রি ছাড়া ড্রয়ার খোলা: কে করেছেন আর কে অনুমতি দিয়েছেন তাসহ। সীমার মানে থাকে কেবল তখনই যখন তা পেরিয়ে যাওয়া জিনিসগুলো পরে দেখা যায়; এটি হওয়ার আগে সেই উত্তর যন্ত্রেই থাকত আর ট্যাব বন্ধ হলেই মুছে যেত।',
  },
  'admin.of_percent': { en: 'of {percent}%', bn: '{percent}%' },
  'admin.on_their_button': {
    en: 'on {name}\u2019s button',
    bn: '{name}-এর বোতামে',
  },
  'admin.a_name_unreadable': {
    en: 'a name this device cannot read',
    bn: 'এমন একটি নাম যা এই যন্ত্র পড়তে পারে না',
  },
  'admin.somebody_unnamed': {
    en: 'somebody this device cannot name',
    bn: 'এমন কেউ যাঁর নাম এই যন্ত্র বলতে পারে না',
  },
  'admin.allowed_by': { en: 'allowed by {name}', bn: 'অনুমতি দিয়েছেন {name}' },
  'admin.of_receipt': { en: 'receipt {number}', bn: 'রসিদ {number}' },
  'shared.try_again': { en: 'Try again', bn: 'আবার চেষ্টা করুন' },
  'admin.this_device_enrolled': { en: 'Enrolled.', bn: 'যন্ত্রটি যুক্ত হয়েছে।' },
  'admin.buyer_written_down': { en: 'Written down.', bn: 'লিখে রাখা হয়েছে।' },
  'admin.buyer_corrected': { en: 'Corrected.', bn: 'ঠিক করা হয়েছে।' },
  'admin.roles_not_ready': {
    en: 'This page is still reading what each role means. Try again in a moment.',
    bn: 'কোন ভূমিকার অর্থ কী, এই পাতা এখনও তা পড়ছে। একটু পরে আবার চেষ্টা করুন।',
  },

  // Why the till's own files would not open. Named by
  // apps/shared/storage_trouble.js, which is where these codes are born, and
  // held to that list by its test: the browser's own sentence for the first of
  // these is English, mentions access handles, and appeared on a real screen
  // above a box asking for an enrolment code.
  // Worded for both screens, because one dictionary answers both and the back
  // office is a device like a till. It said "this till" there, on a page a
  // shopkeeper opens at a desk, which sends them to look at the counter.
  'till-open-elsewhere': {
    en: 'This page is already open in another window on this device. Close that one, then try again. Nothing has been lost.',
    bn: 'এই পাতাটি এই যন্ত্রের অন্য একটি উইন্ডোতে খোলা আছে। সেটি বন্ধ করে আবার চেষ্টা করুন। কিছুই হারায়নি।',
  },
  // Said after the advice above has been followed and has not worked. A
  // browser can go on holding a shop's ledger for a window that has already
  // gone, and then there is no other window to close: the first instruction is
  // a dead end and the screen had nothing else to say. Met three times in one
  // day of walking, each time with one tab open in the whole browser.
  'storage.nothing-else-is-open': {
    en: 'If no other window is open, this device is still holding the shop from one that has gone. Switch the device off and on, and try again. Nothing has been lost.',
    bn: 'যদি অন্য কোনো উইন্ডো খোলা না থাকে, তবে এই যন্ত্র বন্ধ হয়ে যাওয়া একটি উইন্ডোর জন্য দোকানটি ধরে রেখেছে। যন্ত্রটি বন্ধ করে আবার চালু করুন, তারপর আবার চেষ্টা করুন। কিছুই হারায়নি।',
  },
  'no-room-on-this-device': {
    en: 'This device has no room left, so nothing can be written. Free some space and try again.',
    bn: 'এই যন্ত্রে আর জায়গা নেই, তাই কিছুই লেখা যাচ্ছে না। কিছু জায়গা খালি করে আবার চেষ্টা করুন।',
  },
  'this-browser-keeps-nothing': {
    en: 'This browser will not keep anything for this shop. Selling works, but nothing survives closing the tab.',
    bn: 'এই ব্রাউজার এই দোকানের কিছুই রাখবে না। বিক্রি চলবে, কিন্তু ট্যাব বন্ধ করলে কিছু থাকবে না।',
  },
  'admin.was_not_permitted': {
    en: 'and was not permitted to',
    bn: 'আর অনুমতি ছিল না',
  },
  'admin.own_permission': {
    en: 'their own permission covered it',
    bn: 'তাঁর নিজের অনুমতিতেই হয়েছে',
  },

  'admin.owe_the_revenue': { en: 'What you owe the revenue', bn: 'রাজস্বকে আপনি যা দেবেন' },
  'admin.vat_why': {
    en: 'What you sold at each rate in a month, and the tax on it. Worked out when each sale arrived rather than by reading a month of tickets, and by the day the goods were sold rather than the day a till got its sync in. Refunds are in it with their own sign.',
    bn: 'এক মাসে কোন হারে কত বিক্রি হয়েছে আর তার ভ্যাট কত। এক মাসের রসিদ পড়ে নয়, প্রতিটি বিক্রি আসার সময়েই হিসাব করা, আর কাউন্টার কবে সিঙ্ক করল তা নয়, মাল কবে বিক্রি হয়েছে সেই দিন ধরে। ফেরত নিজের চিহ্নসহ এতেই আছে।',
  },
  'admin.sold_amount': { en: '{net} sold', bn: 'বিক্রি {net}' },
  'admin.tax_amount': { en: '{vat} tax', bn: 'ভ্যাট {vat}' },
  'admin.sales_of': { en: '{count} sale(s)', bn: '{count} টি বিক্রি' },
  'admin.tax_in_all': { en: 'Tax in all, for that month.', bn: 'ওই মাসের মোট ভ্যাট।' },
  'admin.vat_waiting': {
    en: '{amount} of that is {count} sale(s) nobody has looked at yet.',
    bn: 'তার মধ্যে {amount} এমন {count} টি বিক্রির, যেগুলো এখনো কেউ দেখেননি।',
  },
  'admin.vat_waiting_why': {
    en: 'They are in the figure, because goods may well have left the shop. Deal with them in "Sales needing somebody to look" before you file, and this line will go.',
    bn: 'এগুলো হিসাবের মধ্যেই আছে, কারণ মাল দোকান থেকে বেরিয়ে গিয়ে থাকতে পারে। জমা দেওয়ার আগে "যেসব বিক্রি কাউকে দেখতে হবে"-তে এগুলোর মীমাংসা করুন, তাহলে এই লাইনটি থাকবে না।',
  },

  'admin.who_buys_on_account': { en: 'Who buys on account', bn: 'কারা বাকিতে কেনেন' },
  'admin.customers_why': {
    en: 'Writing somebody down is what keeps two people with one name apart. A sale that names one of these adds to that person\u2019s account whatever the cashier typed at the till, and every till is told the list so a sale can be written with the internet down.',
    bn: 'কাউকে লিখে রাখাই এক নামের দুজনকে আলাদা রাখে। এই তালিকার কারও নামে বিক্রি হলে ক্যাশিয়ার কাউন্টারে যা-ই লিখুন, সেটি ওই ব্যক্তির হিসাবেই যোগ হয়; আর তালিকাটি প্রতিটি কাউন্টারকে জানানো থাকে, যাতে ইন্টারনেট না থাকলেও বাকিতে বিক্রি লেখা যায়।',
  },
  'admin.their_name': { en: 'Their name', bn: 'তাঁর নাম' },
  'admin.their_phone': { en: 'Their phone, if you have it', bn: 'থাকলে তাঁর ফোন নম্বর' },
  'admin.their_bin': { en: 'Their BIN, if they are a business', bn: 'ব্যবসা হলে তাঁর বিআইএন' },
  'admin.their_limit': {
    en: 'Most they may owe at once, in taka',
    bn: 'একসঙ্গে সর্বোচ্চ কত বাকি রাখতে পারেন, টাকায়',
  },
  'admin.limit_why': {
    en: 'Leave that empty and there is no limit, which is where every shop starts. With one set, a till stops a sale on account that would take them past it, and a supervisor standing there can still allow it.',
    bn: 'খালি রাখলে কোনো সীমা নেই, আর প্রতিটি দোকান সেখান থেকেই শুরু করে। সীমা দিলে কাউন্টার এমন বাকির বিক্রি আটকাবে যা তাঁকে সীমা পার করিয়ে দেয়, তবু পাশে দাঁড়ানো সুপারভাইজার চাইলে অনুমতি দিতে পারবেন।',
  },
  'admin.correct_them': { en: 'Correct them', bn: 'সংশোধন করুন' },
  'admin.write_them_down': { en: 'Write them down', bn: 'লিখে রাখুন' },
  'admin.leave_it': { en: 'Leave it', bn: 'থাক' },
  'admin.no_phone': { en: 'no phone written down', bn: 'ফোন নম্বর লেখা নেই' },
  'admin.account_stopped': { en: 'account stopped', bn: 'বাকির হিসাব বন্ধ' },
  'admin.stop_their_account': { en: 'Stop their account', bn: 'বাকি বন্ধ করুন' },
  'admin.let_them_again': { en: 'Let them again', bn: 'আবার দিন' },

  'admin.who_owes_you': { en: 'Who owes you', bn: 'কার কাছে আপনার পাওনা' },
  'admin.owed_why': {
    en: 'What each person took on account and has not settled. It adds up the sales your tills rang on account and the payments you have taken since, so the notebook beside the till has nothing in it this does not.',
    bn: 'কে বাকিতে কী নিয়েছেন আর এখনো শোধ করেননি। কাউন্টারে তোলা বাকির বিক্রি আর তারপর নেওয়া টাকা যোগ-বিয়োগ করে এটি বলা হয়, তাই কাউন্টারের পাশের খাতায় এমন কিছু নেই যা এখানে নেই।',
  },
  'admin.owes_amount': { en: 'Owes {amount}', bn: 'বাকি {amount}' },
  'admin.in_credit': { en: 'In credit {amount}', bn: 'জমা আছে {amount}' },
  'admin.first_entry': { en: 'first entry {date}', bn: 'প্রথম হিসাব {date}' },
  'admin.entries_count': { en: '{count} entries', bn: '{count} টি হিসাব' },
  'admin.taka_handed_over': { en: 'Taka they handed over', bn: 'তিনি যত টাকা দিলেন' },
  'admin.took_payment': { en: 'Took payment', bn: 'টাকা নিলাম' },
  'admin.hide': { en: 'Hide', bn: 'লুকান' },
  'admin.what_is_this': { en: 'What is this', bn: 'এটি কী' },
  'admin.strike_off_why': {
    en: 'Or strike it off, and say why',
    bn: 'অথবা মাফ করে দিন, আর কেন তা লিখুন',
  },
  'admin.strike_off': { en: 'Strike off', bn: 'মাফ করুন' },
  'admin.brought_goods_back': { en: 'brought goods back', bn: 'মাল ফেরত দিয়েছেন' },
  'admin.took_goods': { en: 'took goods', bn: 'মাল নিয়েছেন' },
  'admin.struck_off': { en: 'struck off', bn: 'মাফ করা হয়েছে' },
  'admin.paid': { en: 'paid', bn: 'টাকা দিয়েছেন' },
  'admin.show_older_entries': { en: 'Show older entries', bn: 'আগের হিসাব দেখুন' },
  'admin.print_this_account': { en: 'Print this account', bn: 'এই হিসাব ছাপুন' },
  'admin.show_more_people': { en: 'Show more people', bn: 'আরও লোক দেখুন' },
  'admin.nobody_owes_you': {
    en: 'Nobody owes you anything, or nothing has been rung on account yet.',
    bn: 'কারও কাছে আপনার পাওনা নেই, অথবা এখনো বাকিতে কিছু তোলা হয়নি।',
  },

  'admin.drawers_open_now': { en: 'Drawers open now', bn: 'এখন খোলা ড্রয়ার' },
  'admin.open_drawers_why': {
    en: 'What each till says its drawer holds while it is still open, and when it last said so. A drawer nobody closes is never counted, and until a till reports one there is nothing to look at but the till itself.',
    bn: 'ড্রয়ার খোলা থাকা অবস্থায় প্রতিটি কাউন্টার বলছে তাতে কত আছে, আর সে কথা শেষ কখন বলেছে। যে ড্রয়ার কেউ বন্ধ করে না তা কখনো গোনাও হয় না; আর কাউন্টার নিজে না জানানো পর্যন্ত দেখার কিছুই থাকে না।',
  },
  'admin.a_till_not_listed_caps': {
    en: 'A till this shop no longer lists',
    bn: 'এমন একটি কাউন্টার যা দোকানের তালিকায় আর নেই',
  },
  'admin.open_since': { en: 'Open since {at}', bn: '{at} থেকে খোলা' },
  'admin.should_hold_amount': { en: 'should hold {amount}', bn: 'থাকার কথা {amount}' },
  'admin.as_that_till_said': {
    en: 'As that till said at {at}.',
    bn: 'কাউন্টারটি {at}-এ যা বলেছিল।',
  },
  'admin.no_drawer_open': { en: 'No till has a drawer open.', bn: 'কোনো কাউন্টারের ড্রয়ার খোলা নেই।' },

  'admin.drawers_counted': { en: 'Drawers counted', bn: 'গোনা ড্রয়ার' },
  'admin.drawers_why': {
    en: 'What each till expected to hold at closing, what was in it, and the difference. A drawer that is short is a fact to look at, not an error: one that could not be closed short would be closed dishonestly instead.',
    bn: 'বন্ধ করার সময় প্রতিটি কাউন্টারে কত থাকার কথা ছিল, কত ছিল, আর পার্থক্য কত। ড্রয়ার কম পড়া দেখার মতো একটি তথ্য, ভুল নয়: কম থাকলে বন্ধই করা যাবে না এমন হলে মানুষ অসৎভাবে বন্ধ করত।',
  },
  'admin.counted_by': { en: 'counted by {name}', bn: 'গুনেছেন {name}' },

  'admin.counted_exactly': { en: 'It counted exactly.', bn: 'ঠিকঠাক মিলেছে।' },
  'admin.short_by': { en: 'Short by {amount}.', bn: '{amount} কম।' },
  'admin.over_by': { en: 'Over by {amount}.', bn: '{amount} বেশি।' },
  'admin.sales_disagree': {
    en: 'Your own sales for this till come to {from_sales}, not {expected}.',
    bn: 'এই কাউন্টারের বিক্রি থেকে আসে {from_sales}, {expected} নয়।',
  },
  'admin.sales_disagree_why': {
    en: 'A till still sending sales will differ for a while. One that has finished sending and still differs is worth asking about.',
    bn: 'যে কাউন্টার এখনো বিক্রি পাঠাচ্ছে তার হিসাব কিছুক্ষণ আলাদা থাকবেই। পাঠানো শেষ হওয়ার পরও আলাদা থাকলে সেটি জিজ্ঞেস করার মতো।',
  },
  // The other honest reason the two figures differ, and the one that was
  // missing: a sale struck out here afterwards. The drawer keeps what the
  // evening recorded on purpose, because a duplicate that inflated what the
  // till expected is exactly what that evening was short by. Without this line
  // the screen named the till as the only cause, which points an owner at
  // whoever counted the drawer for a difference the back office made.
  'admin.struck_out_explains': {
    en: '{amount} of that difference is a sale you struck out afterwards. The drawer keeps what the evening recorded, so the two do not come back together.',
    bn: 'এই পার্থক্যের {amount} হলো পরে বাতিল করা একটি বিক্রি। ড্রয়ারে সেই সন্ধ্যার হিসাবই থেকে যায়, তাই দুটি আর মিলবে না।',
  },
  'admin.no_drawer_counted_yet': {
    en: 'No drawer has been counted and closed yet.',
    bn: 'এখনো কোনো ড্রয়ার গুনে বন্ধ করা হয়নি।',
  },

  'admin.what_you_took': { en: 'What you took', bn: 'আপনি কত পেয়েছেন' },
  'admin.nothing_rung_that_day': { en: 'Nothing rung on that day.', bn: 'ওই দিনে কিছু তোলা হয়নি।' },
  'admin.including_refunds': {
    en: 'including {count} refund(s) of {amount}, which are already in that figure',
    bn: 'এর মধ্যে {amount} টাকার {count} টি ফেরত আছে, যা ওই হিসাবেই ধরা',
  },
  'admin.made_amount': { en: 'Made {amount}', bn: 'লাভ {amount}' },
  'admin.made_why': {
    en: 'on {net} of selling before tax, against {cost} the goods cost you. Over {count} sale(s).',
    bn: 'ভ্যাট ছাড়া {net} টাকার বিক্রিতে, যার মাল আপনার কিনতে লেগেছে {cost}। {count} টি বিক্রিতে।',
  },
  'admin.sales_without_cost': {
    en: '{count} sale(s) of {amount} are not in that figure: something on them has no cost written down.',
    bn: '{amount} টাকার {count} টি বিক্রি ওই হিসাবে নেই: সেগুলোর কোনো কিছুর ক্রয়মূল্য লেখা নেই।',
  },
  'admin.put_what_you_pay': {
    en: 'Put what you pay on those items and the day answers for itself.',
    bn: 'ওই পণ্যগুলোর ক্রয়মূল্য বসিয়ে দিন, তাহলে দিনটির হিসাব নিজেই মিলে যাবে।',
  },
  'admin.drawers_counted_count': { en: '{count} drawer(s) counted', bn: '{count} টি ড্রয়ার গোনা হয়েছে' },
  'admin.expected_amount': { en: 'expected {amount}', bn: 'থাকার কথা {amount}' },
  'admin.counted_amount': { en: 'counted {amount}', bn: 'গোনা হয়েছে {amount}' },
  'admin.short_by_short': { en: 'short by {amount}', bn: '{amount} কম' },
  'admin.over_by_short': { en: 'over by {amount}', bn: '{amount} বেশি' },
  'admin.no_drawer_that_day': { en: 'No drawer was counted that day.', bn: 'ওই দিনে কোনো ড্রয়ার গোনা হয়নি।' },
  'admin.drawers_stay_as_counted': {
    en: 'A drawer\u2019s figures are what the till expected and what somebody counted that evening, and they stay as they were counted. Striking out a sale afterwards takes it out of the takings above and leaves these alone, on purpose: if that sale was rung and never happened, the cash was never there, and the shortfall the counter wrote down is the evidence of it. So these two can disagree, and the difference is the thing to read.',
    bn: 'ড্রয়ারের হিসাব হলো কাউন্টার যা আশা করেছিল আর সেই সন্ধ্যায় কেউ যা গুনেছিল, আর তা যেমন গোনা হয়েছিল তেমনই থাকে। পরে কোনো বিক্রি বাতিল করলে সেটি উপরের আয় থেকে বাদ যায়, কিন্তু এই হিসাব ইচ্ছে করেই বদলায় না: বিক্রিটি তোলা হয়েছিল অথচ হয়নি মানে টাকাটা কখনো ছিলই না, আর সেই সন্ধ্যার কম পড়াটাই তার প্রমাণ। তাই দুটি হিসাব আলাদা হতে পারে, আর পার্থক্যটাই পড়ার জিনিস।',
  },
  'admin.went_on_account': { en: '{amount} went on account', bn: 'বাকিতে গেছে {amount}' },
  'admin.came_back': { en: '{amount} of it came back', bn: 'তার মধ্যে ফেরত এসেছে {amount}' },
  'admin.was_paid_off': { en: '{amount} was paid off', bn: 'শোধ হয়েছে {amount}' },
  'admin.struck_off_amount': { en: '{amount} struck off', bn: '{amount} মাফ করা হয়েছে' },
  'admin.needing_a_look': { en: '{count} needing somebody to look', bn: '{count} টি কাউকে দেখতে হবে' },

  'admin.on_the_shelves': { en: 'What is on the shelves', bn: 'তাকে যা আছে' },
  'admin.shelves_why': {
    en: 'From this device\u2019s own copy of the catalogue, so it answers with the line down. Pick something to correct its price or its tax.',
    bn: 'এই যন্ত্রের নিজের তালিকা থেকে, তাই লাইন না থাকলেও উত্তর দেয়। দাম বা ভ্যাট ঠিক করতে একটি বেছে নিন।',
  },
  'admin.hunt_placeholder': {
    en: 'Name, code or the start of either',
    bn: 'নাম, কোড, বা দুটির শুরুর অংশ',
  },
  'admin.include_retired': {
    en: 'Include things you have stopped selling',
    bn: 'যেগুলো আর বিক্রি করেন না সেগুলোও দেখান',
  },
  'admin.move_prices_by': { en: 'Move these prices by %', bn: 'এই দামগুলো শতাংশে বদলান' },
  'admin.move_prices': { en: 'Move {count} price(s)', bn: '{count} টি দাম বদলান' },
  'admin.reprice_why': {
    en: 'Read this before agreeing. Each lands on the nearest taka, because that is what goes on a shelf label.',
    bn: 'রাজি হওয়ার আগে পড়ে নিন। প্রতিটি নিকটতম টাকায় গিয়ে বসে, কারণ তাকের লেবেলে সেটিই লেখা হয়।',
  },
  'admin.and_more_below': { en: 'and {count} more below.', bn: 'এবং নিচে আরও {count} টি।' },
  'admin.stop_booking_in': { en: 'Stop booking in', bn: 'মাল তোলা বন্ধ' },
  'admin.book_in_a_delivery': { en: 'Book in a delivery', bn: 'চালান তুলুন' },
  'admin.stop_writing_off': { en: 'Stop writing off', bn: 'বাদ দেওয়া বন্ধ' },
  'admin.write_something_off': { en: 'Write something off', bn: 'কিছু বাদ দিন' },
  'admin.stop_counting': { en: 'Stop counting', bn: 'গোনা বন্ধ' },
  'admin.count_the_shelves': { en: 'Count the shelves', bn: 'তাক গুনুন' },
  'admin.receiving_why': {
    en: 'What arrived, and what it cost you. A margin is measured against what these goods cost, not against the last price you paid.',
    bn: 'কী এসেছে আর তাতে আপনার কত লেগেছে। লাভ মাপা হয় এই মালের দামের সঙ্গে, আপনার শেষবার দেওয়া দামের সঙ্গে নয়।',
  },
  'admin.who_it_came_from': { en: 'Who it came from, if you know', bn: 'জানা থাকলে কার কাছ থেকে এসেছে' },
  'admin.challan_number': { en: 'Their challan or invoice number', bn: 'তাঁর চালান বা ইনভয়েস নম্বর' },
  'admin.book_it_in': { en: 'Book it in', bn: 'তুলুন' },
  'admin.counting_why': {
    en: 'What you found on the shelf. This replaces the running figure rather than adjusting it, which is how a number that has drifted gets fixed. What you type is kept on this device as you go, so you can search for the next shelf, close this, and come back to it.',
    bn: 'তাকে আপনি যা পেলেন। এটি চলতি হিসাবটিকে সংশোধন না করে বদলে দেয়, আর এভাবেই সরে যাওয়া সংখ্যা ঠিক হয়। আপনি যা লিখছেন তা যন্ত্রেই রাখা থাকে, তাই পরের তাক খুঁজতে গিয়ে বা এটি বন্ধ করেও ফিরে আসতে পারবেন।',
  },
  'admin.nothing_entered_yet': { en: 'Nothing entered yet', bn: 'এখনো কিছু লেখা হয়নি' },
  'admin.started_at': { en: 'started {at}', bn: 'শুরু {at}' },
  'admin.shelves_entered': { en: '{count} shelves entered', bn: '{count} টি তাক লেখা হয়েছে' },
  'admin.boxes_without_number': {
    en: '{count} box(es) do not hold a number yet',
    bn: '{count} টি ঘরে এখনো সংখ্যা নেই',
  },
  'admin.record_the_count': { en: 'Record the count', bn: 'গোনা লিখে রাখুন' },
  'admin.throw_it_away': { en: 'Throw it away', bn: 'ফেলে দিন' },
  'admin.press_again_to_throw': {
    en: 'Press again to throw it away',
    bn: 'ফেলে দিতে আবার চাপুন',
  },
  'admin.on_hand': { en: '{qty} on hand', bn: 'হাতে {qty}' },
  'admin.sold_after_count': {
    en: '{qty} sold after the last count and not in that figure',
    bn: 'শেষ গোনার পরে বিক্রি {qty}, যা ওই হিসাবে নেই',
  },

  'admin.counted_against': {
    en: 'Counted, against {qty} on the books',
    bn: 'গোনা, খাতায় আছে {qty}',
  },
  'admin.stop_selling': { en: 'Stop selling', bn: 'বিক্রি বন্ধ' },
  'admin.sell_it_again': { en: 'Sell it again', bn: 'আবার বিক্রি করুন' },
  'admin.delete_it': { en: 'Delete it', bn: 'মুছে ফেলুন' },
  'admin.press_again_to_delete': {
    en: 'Press again to delete it',
    bn: 'মুছতে আবার চাপুন',
  },

  'admin.who_you_buy_from': { en: 'Who you buy from', bn: 'কার কাছ থেকে কেনেন' },
  'admin.suppliers_why': {
    en: 'A delivery filed under a supplier can be queried when the goods or the invoice are wrong. One booked under nobody cannot.',
    bn: 'সরবরাহকারীর নামে তোলা চালান নিয়ে মাল বা বিল ভুল হলে কথা বলা যায়। কারও নামে না তোলা থাকলে যায় না।',
  },
  'admin.no_phone_short': { en: 'no phone', bn: 'ফোন নেই' },
  'admin.bin_is': { en: 'BIN {bin}', bn: 'বিআইএন {bin}' },
  'admin.no_longer_bought_from': { en: 'no longer bought from', bn: 'আর কেনা হয় না' },
  'admin.stop': { en: 'Stop', bn: 'বন্ধ' },
  'admin.buy_again': { en: 'Buy again', bn: 'আবার কিনুন' },
  'admin.phone': { en: 'Phone', bn: 'ফোন' },
  'admin.bin_if_any': { en: 'BIN, if they have one', bn: 'থাকলে তাঁর বিআইএন' },

  'admin.owe_suppliers': { en: 'What you owe your suppliers', bn: 'সরবরাহকারীদের আপনি যা দেবেন' },
  'admin.supplier_owing_why': {
    en: 'Everything booked in against a supplier, less what you have paid them. A delivery paid at the door is a delivery and a payment on the same day, which is what the paper says too. Nothing is stored as a balance: what anybody argues about is the deliveries, and they are listed below.',
    bn: 'সরবরাহকারীর নামে তোলা সব কিছু, তাঁকে দেওয়া টাকা বাদ দিয়ে। দরজায় দাম মিটিয়ে নেওয়া চালান একই দিনে একটি চালান আর একটি পরিশোধ, কাগজেও তা-ই লেখা থাকে। কোনো জের আলাদা করে রাখা হয় না: যা নিয়ে কথা হয় তা হলো চালানগুলো, আর সেগুলো নিচে দেওয়া আছে।',
  },
  'admin.a_supplier_not_listed': {
    en: 'A supplier this shop no longer lists',
    bn: 'এমন একজন সরবরাহকারী যিনি দোকানের তালিকায় আর নেই',
  },
  'admin.you_owe': { en: 'You owe {amount}', bn: 'আপনি দেবেন {amount}' },
  'admin.paid_ahead': { en: 'Paid ahead by {amount}', bn: 'আগাম দেওয়া আছে {amount}' },
  'admin.deliveries_count': { en: '{count} deliveries', bn: '{count} টি চালান' },
  'admin.since_date': { en: 'since {date}', bn: '{date} থেকে' },
  'admin.taka_you_handed_over': { en: 'Taka you handed over', bn: 'আপনি যত টাকা দিলেন' },
  'admin.paid_them': { en: 'Paid them', bn: 'টাকা দিলাম' },
  'admin.goods_in': { en: 'goods in', bn: 'মাল এসেছে' },

  'admin.what_came_in': { en: 'What came in', bn: 'কী এসেছে' },
  'admin.deliveries_why': {
    en: 'The last twenty deliveries, newest first. This is what a challan number is for: the goods and the invoice can be put side by side.',
    bn: 'শেষ কুড়িটি চালান, নতুনটি আগে। চালান নম্বর এই কাজেই লাগে: মাল আর বিল পাশাপাশি রেখে মেলানো যায়।',
  },
  'admin.nobody_recorded': { en: 'Nobody recorded', bn: 'কারও নাম লেখা নেই' },
  'admin.lines_count': { en: '{count} line(s)', bn: '{count} টি লাইন' },
  'admin.item_not_held': {
    en: 'an item this device does not hold',
    bn: 'এমন একটি পণ্য যা এই যন্ত্রে নেই',
  },
  'admin.nothing_booked_in': { en: 'Nothing booked in yet.', bn: 'এখনো কিছু তোলা হয়নি।' },

  'admin.tills': { en: 'Tills', bn: 'কাউন্টার' },
  'admin.tills_why': {
    en: 'A code works once. Read it onto the device.',
    bn: 'কোড একবারই কাজ করে। যন্ত্রে গিয়ে সেটি লিখুন।',
  },
  'admin.unnamed_till': { en: 'Unnamed till {id}', bn: 'নামহীন কাউন্টার {id}' },
  'admin.last_heard': { en: 'last heard {at}', bn: 'শেষ শোনা গেছে {at}' },
  'admin.not_heard_from': { en: 'not heard from', bn: 'কোনো খবর নেই' },
  'admin.to_look_at': { en: '{count} to look at', bn: '{count} টি দেখতে হবে' },
  'admin.the_back_office_too': { en: 'the back office as well', bn: 'ব্যাক অফিসও' },
  'admin.holds_nothing': {
    en: 'holds nothing: it needs a code',
    bn: 'কিছু ধরে নেই: এটির একটি কোড দরকার',
  },
  'admin.code_for_back_office': { en: 'Code for this back office', bn: 'এই ব্যাক অফিসের কোড' },
  'admin.code_for_till': { en: 'Code for this till', bn: 'এই কাউন্টারের কোড' },
  'admin.this_one_is_lost': { en: 'This one is lost', bn: 'এটি হারিয়ে গেছে' },
  'admin.press_again_stops_it': {
    en: 'Press again: this stops it dead',
    bn: 'আবার চাপুন: এটি সঙ্গে সঙ্গে বন্ধ হয়ে যাবে',
  },
  'admin.no_tills_yet': { en: 'No tills yet.', bn: 'এখনো কোনো কাউন্টার নেই।' },
  'admin.name_a_new_till': { en: 'Name a new till', bn: 'নতুন কাউন্টারের নাম' },
  'admin.add_a_till': { en: 'Add a till', bn: 'কাউন্টার যোগ করুন' },
  'admin.code_shown_once': {
    en: 'For {who}. Good for {minutes} minutes. Shown once: nobody can read it back, not even from here.',
    bn: '{who}-এর জন্য। {minutes} মিনিট চলবে। একবারই দেখানো হয়: কেউ এটি আর পড়তে পারবে না, এখান থেকেও নয়।',
  },
  'admin.enrolled_on': { en: 'took it on {when}', bn: 'যুক্ত হয়েছে {when}' },
  'admin.counted_on': { en: 'counted {when}', bn: 'গোনা হয়েছে {when}' },
  // The one worth saying. A figure nobody has ever counted against is
  // deliveries and sales added up, and a shop reading it as a shelf figure is
  // reading something else.
  'admin.never_counted': {
    en: 'never counted: this is what the books say, not the shelf',
    bn: 'কখনো গোনা হয়নি: এটি খাতার হিসাব, তাকের নয়',
  },
  'admin.device_refused': {
    en: 'The shop is refusing this device. Its access may have been withdrawn, or the server rebuilt. Nothing here will save until it is enrolled again with a new code.',
    bn: 'দোকান এই যন্ত্রটিকে আর গ্রহণ করছে না। এর অনুমতি তুলে নেওয়া হয়ে থাকতে পারে, বা সার্ভার নতুন করে বানানো হয়েছে। নতুন কোড দিয়ে আবার যুক্ত না করা পর্যন্ত এখানে কিছুই সংরক্ষণ হবে না।',
  },
  'admin.it_is_a_real_sale': { en: 'It is a real sale', bn: 'এটি সত্যিকারের বিক্রি' },
  'admin.it_never_happened_short': { en: 'It never happened', bn: 'এটি কখনো হয়নি' },

  'admin.paste_the_bundle': {
    en: 'Paste what the till showed you, or open the file above',
    bn: 'কাউন্টার যা দেখিয়েছে তা পেস্ট করুন, বা উপরের ফাইলটি খুলুন',
  },
  'admin.how_many_came': { en: 'How many came', bn: 'কতটা এসেছে' },
  'admin.cost_each': { en: 'Cost each', bn: 'প্রতিটির দাম' },
  'admin.how_many_gone': {
    en: 'How many gone, against {qty} on the books',
    bn: 'কতটা গেছে, খাতায় আছে {qty}',
  },

  'admin.owe_suppliers_nothing': {
    en: 'You owe your suppliers nothing, or nothing has been booked in against one.',
    bn: 'সরবরাহকারীদের আপনার কিছু দেওয়ার নেই, অথবা কারও নামে এখনো কিছু তোলা হয়নি।',
  },

  // What a till wrote in its trail, by the number it stores. English words for
  // these are built in the bindings and travel beside the number as a fallback:
  // a screen that has never heard of a new kind says the sentence it was sent.
  'allowed.1': { en: 'a discount', bn: 'একটি ছাড়' },
  'allowed.2': { en: "a price typed over the catalogue's", bn: 'তালিকার দামের বদলে হাতে লেখা দাম' },
  'allowed.3': { en: 'a refund', bn: 'একটি ফেরত' },
  'allowed.4': { en: 'a line taken off', bn: 'একটি লাইন বাদ' },
  'allowed.5': { en: 'the drawer opened', bn: 'ড্রয়ার খোলা হয়েছে' },
  'allowed.6': { en: 'the drawer counted and closed', bn: 'ড্রয়ার গুনে বন্ধ করা হয়েছে' },
  'allowed.7': { en: 'a PIN typed wrongly', bn: 'ভুল পিন দেওয়া হয়েছে' },
  'allowed.8': {
    en: 'a PIN typed wrongly, and that person locked out',
    bn: 'ভুল পিন, আর ওই ব্যক্তি আটকে গেছেন',
  },
  'allowed.9': { en: 'took the till', bn: 'কাউন্টারে বসেছেন' },
  'allowed.10': { en: 'more sold than the shop has', bn: 'দোকানে যত আছে তার বেশি বিক্রি' },
  'allowed.11': {
    en: 'tried to take a line off a basket that had been paid towards',
    bn: 'যে ঝুড়ির টাকা নেওয়া শুরু হয়েছে তার থেকে লাইন বাদ দিতে চেয়েছেন',
  },
  'allowed.12': {
    en: 'sold to somebody already past what they may owe',
    bn: 'যিনি ইতিমধ্যে বাকির সীমা পার করেছেন তাঁকে বিক্রি',
  },
  'allowed.13': {
    en: 'tried to open the drawer',
    bn: 'ড্রয়ার খুলতে চেয়েছেন',
  },
  'allowed.14': {
    en: 'printed a receipt again',
    bn: 'রসিদ আবার ছেপেছেন',
  },


  // What the back office itself refuses, before the shop is asked.
  'admin.say_shop_name': {
    en: 'a shop needs a name: it is what heads every receipt',
    bn: 'দোকানের একটি নাম দরকার: এটিই প্রতিটি রসিদের উপরে থাকে',
  },
  'admin.say_pin': {
    en: 'a PIN of at least four digits',
    bn: 'অন্তত চার অঙ্কের একটি পিন',
  },
  'admin.say_person_name': {
    en: 'a person needs a name: it is what a receipt and a shift are filed under',
    bn: 'মানুষটির একটি নাম দরকার: রসিদ আর ড্রয়ার এই নামেই জমা হয়',
  },
  'admin.say_name_and_pin': {
    en: 'a name, and a PIN of at least four digits',
    bn: 'একটি নাম, আর অন্তত চার অঙ্কের একটি পিন',
  },
  'admin.item_already_withdrawn': {
    en: 'the shop has withdrawn that item already',
    bn: 'দোকান পণ্যটি আগেই তুলে নিয়েছে',
  },
  'admin.item_withdrawn_since': {
    en: 'the shop has withdrawn that item since this list was read',
    bn: 'এই তালিকা পড়ার পর দোকান পণ্যটি তুলে নিয়েছে',
  },
  'admin.say_name_and_price': {
    en: 'a name and a price in taka',
    bn: 'একটি নাম আর টাকায় দাম',
  },
  'admin.say_rate_range': {
    en: 'a tax rate between nothing and a hundred percent',
    bn: 'ভ্যাটের হার শূন্য থেকে একশো শতাংশের মধ্যে',
  },
  'admin.nothing_to_take_out': {
    en: 'there is nothing in the catalogue to take out yet',
    bn: 'তালিকায় বের করার মতো এখনো কিছু নেই',
  },
  'admin.nothing_writable': {
    en: 'nothing in that file can be written as it stands',
    bn: 'ফাইলটির কিছুই এই অবস্থায় লেখা যাবে না',
  },
  'admin.say_fallback_rate': {
    en: 'a tax rate for the rows whose file does not say: nought to a hundred percent',
    bn: 'ফাইলে যেসব সারিতে ভ্যাট বলা নেই তাদের হার: শূন্য থেকে একশো শতাংশ',
  },
  'admin.paste_what_till_showed': {
    en: 'paste what the till showed you',
    bn: 'কাউন্টার যা দেখিয়েছে তা পেস্ট করুন',
  },
  'admin.say_a_name': {
    en: 'a name to write down',
    bn: 'লিখে রাখার মতো একটি নাম',
  },
  'admin.not_dates': {
    en: 'those are not dates',
    bn: 'এগুলো তারিখ নয়',
  },
  'admin.say_how_much': {
    en: 'say how much you handed over',
    bn: 'আপনি কত টাকা দিলেন তা লিখুন',
  },
  'admin.say_how_much_struck_off': {
    en: 'say how much to strike off',
    bn: 'কত টাকা বাদ দিতে হবে তা লিখুন',
  },
  'admin.say_how_much_handed_over': {
    en: 'say how much they handed over',
    bn: 'তাঁরা কত টাকা দিলেন তা লিখুন',
  },
  'admin.a_box_holds_no_number': {
    en: 'some boxes do not hold a number yet',
    bn: 'কিছু ঘরে এখনও কোনো সংখ্যা নেই',
  },
  'admin.nothing_counted_yet': {
    en: 'nothing counted yet',
    bn: 'এখনও কিছু গোনা হয়নি',
  },
  'admin.say_why_off': {
    en: 'say why it is coming off: this is the entry that makes money disappear',
    bn: 'কেন বাদ যাচ্ছে তা লিখুন: এই হিসাবেই টাকা উধাও হয়',
  },
  'admin.say_receipt_number': {
    en: 'the receipt number, as it is printed',
    bn: 'রসিদ নম্বর, যেমন ছাপা আছে',
  },
  'admin.say_why_changing': {
    en: 'say why the answer is changing: this is what explains a figure that moved',
    bn: 'উত্তর কেন বদলাচ্ছে তা লিখুন: সরে যাওয়া হিসাব এটিই বোঝায়',
  },
  'admin.say_what_decided': {
    en: 'say what you decided: this is what somebody reads in six months',
    bn: 'আপনি কী ঠিক করলেন লিখুন: ছয় মাস পরে কেউ এটিই পড়বে',
  },
  'admin.not_a_date': {
    en: 'that is not a date',
    bn: 'এটি তারিখ নয়',
  },
  'admin.not_a_month': {
    en: 'that is not a month',
    bn: 'এটি মাস নয়',
  },
  'admin.say_supplier_name': {
    en: 'a supplier needs a name: it is what a delivery is filed under',
    bn: 'সরবরাহকারীর একটি নাম দরকার: চালান এই নামেই জমা হয়',
  },
  'admin.say_how_many_gone': {
    en: 'how many are gone? A number, and not zero',
    bn: 'কতটা গেছে? একটি সংখ্যা, শূন্য নয়',
  },
  'admin.say_why_gone': {
    en: 'say why: broken, spoiled, taken, given away. A month later nobody remembers',
    bn: 'কেন লিখুন: ভেঙেছে, নষ্ট হয়েছে, নেওয়া হয়েছে, দিয়ে দেওয়া হয়েছে। এক মাস পরে কারও মনে থাকে না',
  },
  'admin.could_not_add_it_up': {
    en: 'too big to add up: read the lines',
    bn: 'যোগ করার মতো নয়, এত বড়: লাইনগুলো দেখুন',
  },
  'admin.not_a_limit': {
    en: '"{typed}" is not an amount. Digits, and up to two after a point',
    bn: '"{typed}" টাকার অঙ্ক নয়। অঙ্ক, দশমিকের পরে সর্বোচ্চ দুটি',
  },
  'admin.not_a_quantity': {
    en: '"{typed}" is not a quantity. Digits, and up to three after a point',
    bn: '"{typed}" পরিমাণ নয়। অঙ্ক, দশমিকের পরে সর্বোচ্চ তিনটি',
  },
  'admin.not_a_cost': {
    en: '"{typed}" is not a cost. Digits, and up to two after a point',
    bn: '"{typed}" দাম নয়। অঙ্ক, দশমিকের পরে সর্বোচ্চ দুটি',
  },
  'admin.nothing_to_book': {
    en: 'nothing to book: put a quantity against something',
    bn: 'তোলার কিছু নেই: কোনো কিছুর পাশে পরিমাণ লিখুন',
  },


  // What the back office says when something has happened.
  'admin.shop_saved': {
    en: 'Shop details saved. Tills pick them up within half a minute.',
    bn: 'দোকানের তথ্য সংরক্ষণ হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.taken_in': {
    en: 'Taken in. That device can be wiped now.',
    bn: 'নেওয়া হয়েছে। ওই যন্ত্রটি এখন মুছে ফেলা যাবে।',
  },
  'admin.kept_as_it_stands': {
    en: 'Kept as it stands. Your tills have it.',
    bn: 'যেমন আছে তেমনই রাখা হলো। আপনার কাউন্টারগুলোর কাছে আছে।',
  },
  'admin.kept_counts': {
    en: 'Kept. It counts as it did.',
    bn: 'রাখা হলো। আগের মতোই গোনা হবে।',
  },
  'admin.nobody_answered': {
    en: 'Nobody had answered about that one. It is still in the queue.',
    bn: 'ওটি নিয়ে কেউ উত্তর দেননি। এটি এখনো তালিকায় আছে।',
  },
  'admin.nothing_allowed_over': {
    en: 'Nothing was allowed over a ceiling in those days.',
    bn: 'ওই দিনগুলোতে সীমার বেশি কিছু অনুমতি পায়নি।',
  },
  'admin.put_back_counts': {
    en: 'Put back. It counts again, and so does anything it put on an account.',
    bn: 'ফিরিয়ে আনা হলো। এটি আবার গোনা হবে, আর এটি কারও বাকিতে যা তুলেছিল তাও।',
  },
  'admin.somebody_else_answered': {
    en: 'Somebody else answered that one while this was open. Nothing changed: look again.',
    bn: 'এটি খোলা থাকা অবস্থায় অন্য কেউ ওটির উত্তর দিয়েছেন। কিছু বদলায়নি: আবার দেখুন।',
  },
  'admin.struck_off_with_reason': {
    en: 'Struck off, with the reason.',
    bn: 'কারণসহ মাফ করা হলো।',
  },
  'admin.struck_out_removed': {
    en: 'Struck out. It has come out of your takings, your tax and your stock.',
    bn: 'বাতিল করা হলো। এটি আপনার আয়, ভ্যাট আর স্টক থেকে বাদ গেছে।',
  },
  'admin.taken_off_owing': {
    en: 'Taken off what they owe.',
    bn: 'তাঁর বাকি থেকে বাদ দেওয়া হলো।',
  },
  'admin.delivery_already_booked': {
    en: 'That delivery was already booked. Nothing was counted twice.',
    bn: 'ওই চালান আগেই তোলা হয়েছে। কিছু দুবার গোনা হয়নি।',
  },
  'admin.already_cut_off': {
    en: 'That device was already cut off, or had never been used.',
    bn: 'ওই যন্ত্র আগেই বন্ধ করা হয়েছে, অথবা কখনো ব্যবহারই হয়নি।',
  },
  'admin.already_dealt_with': {
    en: 'That one was already dealt with. Nothing changed.',
    bn: 'ওটির মীমাংসা আগেই হয়েছে। কিছু বদলায়নি।',
  },
  'admin.count_thrown_away': {
    en: 'The count was thrown away.',
    bn: 'গোনা বাতিল করা হলো।',
  },
  'admin.their_account_stopped': {
    en: 'Their account is stopped.',
    bn: 'তাঁর বাকির হিসাব বন্ধ করা হলো।',
  },
  'admin.can_buy_again': {
    en: 'They can buy on account again.',
    bn: 'তিনি আবার বাকিতে কিনতে পারবেন।',
  },


  // What the back office says about a thing it has just done, by name.
  'admin.new_pin_set': {
    en: '{name} has a new PIN. Tills accept it within half a minute.',
    bn: '{name}-এর নতুন পিন হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে সেটি মানবে।',
  },
  'admin.item_gone': {
    en: '{name} is gone. Tills drop it within half a minute.',
    bn: '{name} মুছে গেছে। কাউন্টারগুলো আধ মিনিটের মধ্যে বাদ দেবে।',
  },
  'admin.item_on_sale_again': {
    en: '{name} is on sale again. Tills pick it up within half a minute.',
    bn: '{name} আবার বিক্রি হবে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.item_withdrawn': {
    en: '{name} will not ring at a till any more. Refunds of it still work.',
    bn: '{name} আর কাউন্টারে উঠবে না। এর ফেরত আগের মতোই চলবে।',
  },
  'admin.item_added': {
    en: '{name} added. Tills pick it up within half a minute.',
    bn: '{name} যোগ হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.item_corrected': {
    en: '{name} corrected. Tills pick it up within half a minute, and this list with them.',
    bn: '{name} সংশোধন হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে, এই তালিকাও।',
  },
  'admin.supplier_back': {
    en: '{name} is back on the list.',
    bn: '{name} আবার তালিকায় এসেছেন।',
  },
  'admin.person_back': {
    en: '{name} can sign in again. Tills offer them within half a minute.',
    bn: '{name} আবার ঢুকতে পারবেন। কাউন্টারগুলো আধ মিনিটের মধ্যে তাঁকে দেখাবে।',
  },
  'admin.person_added': {
    en: '{name} can sign in once the tills refresh.',
    bn: 'কাউন্টারগুলো নতুন তথ্য পেলেই {name} ঢুকতে পারবেন।',
  },
  'admin.person_corrected': {
    en: '{name} corrected. Tills pick it up within half a minute.',
    bn: '{name} সংশোধন হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.supplier_added': {
    en: '{name} added.',
    bn: '{name} যোগ হয়েছেন।',
  },
  'admin.supplier_corrected': {
    en: '{name} corrected.',
    bn: '{name} সংশোধন হয়েছে।',
  },
  'admin.shelves_counted': {
    en: '{count} shelves counted.',
    bn: '{count} টি তাক গোনা হয়েছে।',
  },
  'admin.lines_booked_in': {
    en: '{count} line(s) booked in.',
    bn: '{count} টি লাইন তোলা হয়েছে।',
  },
  'admin.prices_moved': {
    en: '{count} price(s) moved.',
    bn: '{count} টি দাম বদলানো হয়েছে।',
  },
  'admin.some_prices_moved': {
    en: '{moved} of {wanted} moved. The rest were changed by somebody else while you were reading; look again.',
    bn: '{wanted} টির মধ্যে {moved} টি বদলেছে। বাকিগুলো আপনি পড়ার সময় অন্য কেউ বদলে দিয়েছেন; আবার দেখুন।',
  },
  'admin.no_such_receipt': {
    en: 'Nothing here carries {number}. Check the number on the paper.',
    bn: '{number} নম্বরে এখানে কিছু নেই। কাগজে লেখা নম্বরটি মিলিয়ে দেখুন।',
  },
  'admin.supplier_stopped': {
    en: '{name} will not be offered on a delivery. What they already delivered still says so.',
    bn: 'চালান তোলার সময় {name}-কে আর দেখানো হবে না। তিনি যা আগে দিয়েছেন তা যেমন ছিল তেমনই থাকবে।',
  },
  'admin.adopted_sales': {
    en: 'Taken in {count} sale(s), {waiting} of them waiting for you to look in the list below.',
    bn: '{count} টি বিক্রি নেওয়া হয়েছে, তার মধ্যে {waiting} টি নিচের তালিকায় আপনার দেখার অপেক্ষায়।',
  },
  // The shop already had them, by the ordinary route or by an earlier attempt
  // at this one. There is nothing in any list to look at, and saying otherwise
  // sends somebody hunting for an entry that is not there.
  'admin.adopted_already_had': {
    en: 'Taken in {count} sale(s). The shop already had them, so there is nothing waiting for you.',
    bn: '{count} টি বিক্রি নেওয়া হয়েছে। দোকানে সেগুলো আগে থেকেই ছিল, তাই আপনার দেখার মতো কিছু বাকি নেই।',
  },
  'admin.list_taken_out': {
    en: '{count} line(s) saved as {file}. Change what you need and bring the same file back.',
    bn: '{count} টি সারি {file} নামে রাখা হয়েছে। যা দরকার বদলে একই ফাইল ফিরিয়ে আনুন।',
  },


  // The rest of what the back office says after it has done something.
  'admin.you_still_owe_them': {
    en: ' You still owe them {amount}.',
    bn: ' তাঁদের আপনি এখনো {amount} দেবেন।',
  },
  'admin.you_are_paid_ahead': {
    en: ' You are paid ahead by {amount}.',
    bn: ' আপনি {amount} আগাম দিয়ে রেখেছেন।',
  },
  'admin.you_owe_them_nothing': {
    en: ' You owe them nothing now.',
    bn: ' তাঁদের আপনার আর কিছু দেওয়ার নেই।',
  },
  'admin.paid': {
    en: 'Paid.',
    bn: 'টাকা দেওয়া হয়েছে।',
  },
  'admin.already_recorded': {
    en: 'That one was already recorded.',
    bn: 'ওটি আগেই লেখা হয়েছে।',
  },
  'admin.person_still_owes': {
    en: ' {name} still owes {amount}.',
    bn: ' {name}-এর এখনো {amount} বাকি।',
  },
  'admin.person_in_credit': {
    en: ' {name} is in credit by {amount}.',
    bn: ' {name}-এর {amount} জমা আছে।',
  },
  'admin.person_owes_nothing': {
    en: ' {name} owes nothing now.',
    bn: ' {name}-এর আর কোনো বাকি নেই।',
  },
  'admin.device_cut_off': {
    en: 'That device is cut off. It can ring nothing into this shop now. If it turns up holding sales, read them off it and paste them in above.',
    bn: 'ওই যন্ত্র বন্ধ করা হয়েছে। এটি আর এই দোকানে কিছু তুলতে পারবে না। পরে যদি এর ভেতরে বিক্রি থেকে থাকে, সেগুলো পড়ে নিয়ে উপরে পেস্ট করুন।',
  },
  'admin.person_suspended': {
    en: '{name} is suspended. Tills stop offering them within half a minute, and their name still resolves on the sales they rang.',
    bn: '{name}-কে বন্ধ করা হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে তাঁকে আর দেখাবে না, আর তিনি যেসব বিক্রি তুলেছেন সেখানে তাঁর নাম আগের মতোই থাকবে।',
  },
  'admin.written_off_line': {
    en: '{name}: {qty} written off, {why}.',
    bn: '{name}: {qty} বাদ দেওয়া হয়েছে, {why}।',
  },
  'admin.would_be_gone': {
    en: '{name} would be gone from every till and from this list, and there is no way back. Press again if that is what you want.',
    bn: '{name} প্রতিটি কাউন্টার আর এই তালিকা থেকে মুছে যাবে, ফেরার কোনো পথ নেই। এটাই চাইলে আবার চাপুন।',
  },
  'admin.count_partly_filed': {
    en: '{why}. {count} counted so far, the rest is still here.',
    bn: '{why}। এ পর্যন্ত {count} টি গোনা হয়েছে, বাকিটা এখনো এখানেই আছে।',
  },
  'admin.shop_took_some': {
    en: 'the shop did not take all of it',
    bn: 'দোকান সবটা নেয়নি',
  },
  'admin.count_late_sales': {
    en: '{count} item(s) have sales that arrived after the count and are not in the figure.',
    bn: '{count} টি পণ্যের এমন বিক্রি আছে যা গোনার পরে এসেছে আর ওই হিসাবে নেই।',
  },
  // What a screen puts in an attribute: a tooltip, a placeholder, the label a
  // screen reader speaks. Invisible to a test that looks at what is assigned to
  // a message slot, and read by exactly the person who needs their own language.
  'till.keep_not_promised': {
    en: 'This browser would not promise to keep it: send what is waiting before you close',
    bn: 'এই ব্রাউজার এটি রেখে দেওয়ার নিশ্চয়তা দেয়নি: বন্ধ করার আগে যা পাঠানো বাকি আছে পাঠিয়ে দিন',
  },
  'till.keeps_through_close': {
    en: 'Sales survive this tab closing',
    bn: 'ট্যাব বন্ধ করলেও বিক্রিগুলো থেকে যাবে',
  },
  'till.keeps_nothing': {
    en: 'Nothing survives a reload',
    bn: 'রিলোড করলে কিছুই থাকবে না',
  },
  'till.hidden_tab_stops': {
    en: 'A browser stops a hidden tab. Bring this one to the front.',
    bn: 'ব্রাউজার আড়ালে থাকা ট্যাব থামিয়ে দেয়। এটিকে সামনে আনুন।',
  },
  'till.last_reached': {
    en: 'When a round last reached the shop',
    bn: 'শেষবার কখন দোকানে পৌঁছেছিল',
  },
  'till.language': {
    en: 'Language',
    bn: 'ভাষা',
  },
  'till.how_many': {
    en: 'how many',
    bn: 'কতগুলো',
  },
  'till.percent_off': {
    en: '% off',
    bn: '% ছাড়',
  },
  'till.amount_off': {
    en: 'off',
    bn: 'ছাড়',
  },
  'till.percent_off_ticket': {
    en: '% off the whole ticket, up to {ceiling}',
    bn: 'পুরো বিলে % ছাড়, সর্বোচ্চ {ceiling}',
  },
  // For somebody who may give nothing away on their own, which is every
  // cashier in every shop: the preset says nothing unaided. The box is offered
  // all the same, because a supervisor standing there is exactly what the till
  // is for, and it says so rather than letting them find out by being refused.
  'till.percent_off_ticket_asks': {
    en: '% off the whole ticket, a supervisor allows it',
    bn: 'পুরো বিলে % ছাড়, সুপারভাইজার অনুমতি দেবেন',
  },
  'till.percent_off_asks': {
    en: '% off, supervisor',
    bn: '% ছাড়, সুপারভাইজার',
  },
  'till.price_each': { en: 'price each', bn: 'প্রতিটির দাম' },
  'till.price_each_asks': {
    en: 'price each, a supervisor allows a change',
    bn: 'প্রতিটির দাম, বদলাতে সুপারভাইজারের অনুমতি লাগবে',
  },
  'till.amount_off_asks': {
    en: 'off, supervisor',
    bn: 'ছাড়, সুপারভাইজার',
  },
  'till.amount_off_ticket_asks': {
    en: 'or an amount off the whole ticket, a supervisor allows it',
    bn: 'অথবা পুরো বিল থেকে টাকার অঙ্কে ছাড়, সুপারভাইজার অনুমতি দেবেন',
  },
  'till.amount_off_ticket': {
    en: 'or an amount off the whole ticket',
    bn: 'অথবা পুরো বিল থেকে টাকার অঙ্কে ছাড়',
  },
  'till.their_phone': {
    en: 'Their phone, if you have it',
    bn: 'তাঁর ফোন নম্বর, থাকলে',
  },
  'admin.keep_not_promised': {
    en: 'This browser would not promise to keep what this device holds',
    bn: 'এই যন্ত্র যা ধরে রেখেছে তা রাখার নিশ্চয়তা এই ব্রাউজার দেয়নি',
  },
  // Words that were sitting in the markup as plain text, which neither the
  // scan for a sentence assigned to a message slot nor the one for an attribute
  // could see. A Bangla shop read every one of them in English.
  'admin.carried_mark_is': {
    en: 'Mark {mark}. The till that wrote this shows a mark too: if they differ, not all of it arrived, and taking it in would take in fewer sales than that device is holding.',
    bn: 'চিহ্ন {mark}। যে কাউন্টার এটি লিখেছে সেখানেও একটি চিহ্ন দেখাবে: দুটি আলাদা হলে পুরোটা আসেনি, আর তখন নিলে ওই যন্ত্রে যত বিক্রি আছে তার চেয়ে কম নেওয়া হবে।',
  },
  'admin.not_a_bundle': {
    en: 'That is not a bundle. Check the whole of it was copied.',
    bn: 'এটি বান্ডিল নয়। পুরোটা কপি হয়েছে কি না দেখে নিন।',
  },
  'admin.take_them_in': { en: 'Take them in', bn: 'নিয়ে নিন' },
  'admin.drawer_sales': { en: '{count} sale(s)', bn: '{count} টি বিক্রি' },
  'admin.drawer_float': { en: 'float {amount}', bn: 'শুরুর টাকা {amount}' },
  'admin.days_of_stock_left': {
    en: 'days or less of stock left',
    bn: 'দিন বা তার কম চলার মতো স্টক আছে',
  },
  'admin.zero_rated': { en: 'zero rated', bn: 'শূন্য হার' },
  'admin.taxed_on_listed_price': {
    en: 'taxed on the listed price',
    bn: 'তালিকার দামের উপর ভ্যাট',
  },
  'admin.no_longer_sold': { en: 'no longer sold', bn: 'আর বিক্রি হয় না' },
  'admin.exempt': { en: 'exempt', bn: 'ভ্যাটমুক্ত' },
  'admin.write_it_off': { en: 'Write it off', bn: 'বাদ দিয়ে দিন' },
  'admin.jump_to': { en: 'Jump to a section', bn: 'কোন অংশে যাবেন' },
  'admin.try_now': {
    en: 'Try now',
    bn: 'এখনই চেষ্টা করুন',
  },
  'admin.language': {
    en: 'Language',
    bn: 'ভাষা',
  },
  'admin.days': {
    en: 'Days',
    bn: 'দিন',
  },
  'admin.why_written_off': {
    en: 'Why: broken, spoiled, taken, given away',
    bn: 'কেন: ভেঙেছে, নষ্ট হয়েছে, চুরি গেছে, দিয়ে দেওয়া হয়েছে',
  },

  'admin.refused_row': {
    en: 'line {line}: {said}',
    bn: 'লাইন {line}: {said}',
  },
  'admin.withdrawn_row': {
    en: 'line {line}: the shop has withdrawn {name}',
    bn: 'লাইন {line}: দোকান {name} তুলে নিয়েছে',
  },
  'admin.not_read_back_yet': {
    en: '{count} row(s) written a moment ago have not reached this device yet. Wait for the line at the top to say the catalogue has been read, then try again: they would be added a second time.',
    bn: 'একটু আগে লেখা {count} টি সারি এখনো এই যন্ত্রে পৌঁছায়নি। উপরের লাইনে তালিকা পড়া হয়েছে বলা পর্যন্ত অপেক্ষা করে আবার চেষ্টা করুন: নয়তো ওগুলো দ্বিতীয়বার যোগ হয়ে যাবে।',
  },
  'admin.brought_in': {
    en: '{added} added, {corrected} corrected. Tills pick them up within half a minute.',
    bn: '{added} টি যোগ হয়েছে, {corrected} টি ঠিক হয়েছে। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.brought_in_refused': {
    en: '{added} added, {corrected} corrected, {refused} refused. Tills pick them up within half a minute.',
    bn: '{added} টি যোগ হয়েছে, {corrected} টি ঠিক হয়েছে, {refused} টি নেওয়া যায়নি। কাউন্টারগুলো আধ মিনিটের মধ্যে পেয়ে যাবে।',
  },
  'admin.name_already_signs_in': {
    en:
      'somebody who can sign in is already called that. Two identical buttons at a till is how a ' +
      'shift ends up attributed to the wrong person: give them a name that tells them apart, or ' +
      'press again to add them anyway.',
    bn:
      'এই নামে সাইন-ইন করতে পারে এমন একজন আগে থেকেই আছে। কাউন্টারে হুবহু এক রকম দুটি বোতাম থাকলে ' +
      'শিফট ভুল মানুষের নামে উঠে যায়: আলাদা করে চেনা যায় এমন নাম দিন, নয়তো আবার চাপ দিয়ে ' +
      'এভাবেই যোগ করুন।',
  },
  'admin.name_already_on_account': {
    en:
      'somebody with an account is already called that. Two records for one person is two ' +
      'accounts, and what they owe ends up split between them: give them a name that tells them ' +
      'apart, or press again to write this one down anyway.',
    bn:
      'এই নামে বাকির খাতা আছে এমন একজন আগে থেকেই আছে। এক মানুষের দুটি খাতা মানে দুটি হিসাব, আর ' +
      'তার বাকি টাকা দুই খাতায় ভাগ হয়ে যায়: আলাদা করে চেনা যায় এমন নাম দিন, নয়তো আবার চাপ ' +
      'দিয়ে এটাই লিখে রাখুন।',
  },
  'admin.somebody_else_changed_it': {
    en: 'somebody else changed that item while you had it open. Press “Correct it” again to see what it says now.',
    bn: 'আপনি খুলে রাখা অবস্থায় অন্য কেউ পণ্যটি বদলেছে। এখন কী আছে দেখতে আবার “ঠিক করুন” চাপুন।',
  },


  // ------------------------------------------------------------ the papers
  //
  // Nothing asks for these today. Paper is printed in English whatever the
  // screen is set to, and `paperWords` has no caller: no ESC/POS code page
  // carries Bangla so a thermal printer gets English regardless, the layout
  // pads by counting characters which Bangla defeats, and a shop with two
  // languages on its counter should not keep two shapes of receipt.
  //
  // Kept rather than deleted, because the mechanism is what makes paper
  // translatable at all and the raster path that Bangla thermal printing needs
  // will want it. They are still checked to exist in every language by the
  // test against paper_words.json, so they cannot rot while they wait.
  //
  // A receipt, a drawer slip and an account page. The core lays them out and
  // asks for each label by name, defaulting to English: it holds no
  // translations, so a thermal printer that cannot render Bangla is handed
  // nothing and prints what it always did. Covered by a test against
  // paper_words.json, which the core writes out.
  'paper:account.title': {
    en: 'ACCOUNT',
    bn: 'হিসাব',
  },
  'paper:account.name': {
    en: 'Name',
    bn: 'নাম',
  },
  'paper:account.nothing_on_it': {
    en: 'Nothing on this account',
    bn: 'এই হিসাবে কিছু নেই',
  },
  'paper:account.in_credit': {
    en: 'In credit',
    bn: 'জমা',
  },
  'paper:account.owing': {
    en: 'Owing',
    bn: 'বাকি',
  },
  'paper:paper.printed': {
    en: 'Printed',
    bn: 'ছাপা হয়েছে',
  },
  'paper:drawer.counted_title': {
    en: 'DRAWER COUNTED',
    bn: 'ড্রয়ার গোনা হয়েছে',
  },
  'paper:drawer.so_far_title': {
    en: 'DRAWER SO FAR',
    bn: 'এ পর্যন্ত ড্রয়ার',
  },
  'paper:drawer.till': {
    en: 'Till',
    bn: 'কাউন্টার',
  },
  'paper:drawer.counted_by': {
    en: 'Counted by',
    bn: 'গুনেছেন',
  },
  'paper:drawer.printed_by': {
    en: 'Printed by',
    bn: 'ছেপেছেন',
  },
  'paper:drawer.checked_by': {
    en: 'Checked by',
    bn: 'মিলিয়েছেন',
  },
  'paper:drawer.sales': {
    en: 'Sales',
    bn: 'বিক্রি',
  },
  'paper:drawer.opening_float': {
    en: 'Opening float',
    bn: 'শুরুর নগদ',
  },
  'paper:drawer.not_in_the_till': {
    en: 'not in the till',
    bn: 'ড্রয়ারে নেই',
  },
  'paper:drawer.cash_in': {
    en: 'Cash in',
    bn: 'নগদ জমা',
  },
  'paper:drawer.cash_out': {
    en: 'Cash out',
    bn: 'নগদ বের',
  },
  'paper:drawer.should_hold': {
    en: 'SHOULD HOLD',
    bn: 'থাকার কথা',
  },
  'paper:drawer.counted': {
    en: 'Counted',
    bn: 'গোনা হয়েছে',
  },
  'paper:drawer.exactly_right': {
    en: 'Exactly right',
    bn: 'ঠিক মিলেছে',
  },
  'paper:drawer.short_by': {
    en: 'Short by',
    bn: 'কম',
  },
  'paper:drawer.over_by': {
    en: 'Over by',
    bn: 'বেশি',
  },
  'paper:receipt.refund_title': {
    en: 'REFUND',
    bn: 'ফেরত',
  },
  'paper:receipt.against': {
    en: 'against',
    bn: 'যার বিপরীতে',
  },
  'paper:receipt.number': {
    en: 'Receipt',
    bn: 'রসিদ',
  },
  'paper:receipt.to_be_assigned': {
    en: 'to be assigned',
    bn: 'নম্বর পরে বসবে',
  },
  'paper:receipt.date': {
    en: 'Date',
    bn: 'তারিখ',
  },
  'paper:receipt.served_by': {
    en: 'Served by',
    bn: 'বিক্রি করেছেন',
  },
  'paper:receipt.customer': {
    en: 'Customer',
    bn: 'খদ্দের',
  },
  'paper:receipt.buyer_bin': {
    en: 'Buyer BIN',
    bn: 'ক্রেতার বিআইএন',
  },
  'paper:receipt.line_discount': {
    en: 'discount',
    bn: 'ছাড়',
  },
  'paper:receipt.net': {
    en: 'Net',
    bn: 'ভ্যাট ছাড়া',
  },
  'paper:receipt.discount': {
    en: 'Discount',
    bn: 'ছাড়',
  },
  'paper:receipt.vat': {
    en: 'VAT',
    bn: 'ভ্যাট',
  },
  'paper:receipt.on': {
    en: 'on',
    bn: 'যার উপর',
  },
  'paper:receipt.vat_in_all': {
    en: 'VAT in all',
    bn: 'মোট ভ্যাট',
  },
  'paper:receipt.total': {
    en: 'TOTAL',
    bn: 'সর্বমোট',
  },
  'paper:receipt.cash': {
    en: 'Cash',
    bn: 'নগদ',
  },
  'paper:receipt.card': {
    en: 'Card',
    bn: 'কার্ড',
  },
  'paper:receipt.on_account': {
    en: 'On account',
    bn: 'বাকিতে',
  },
  'paper:receipt.change': {
    en: 'Change',
    bn: 'ফেরত',
  },
  'paper:receipt.thank_you': {
    en: 'Thank you',
    bn: 'ধন্যবাদ',
  },

  // Why a sale is being held, keyed by what the shop said when it held it. The
  // English sentence travels beside it as the fallback: a sale held before the
  // shop kept the reason itself can only ever be shown as the words.
  'held.totals-mismatch': {
    en: 'the till stored {stored} and the shop recomputed {recomputed}',
    bn: 'কাউন্টার লিখেছে {stored}, দোকানের হিসাবে হয় {recomputed}',
  },
  'held.duplicate-receipt': {
    en: 'receipt number {receipt_no} was already used by another sale',
    bn: '{receipt_no} রসিদ নম্বরটি আগেই অন্য একটি বিক্রিতে ব্যবহার হয়েছে',
  },
  'held.undecodable': {
    en: 'the shop could not read what that till wrote',
    bn: 'ওই কাউন্টার যা লিখেছে দোকান তা পড়তে পারেনি',
  },
  'held.carried-in': {
    en: 'carried in by hand from a device that could not send it',
    bn: 'যে যন্ত্র পাঠাতে পারেনি তার থেকে হাতে করে আনা',
  },
  // Two sentences rather than one with the direction poured into it: "after"
  // and "before" are words, and a word interpolated into another language's
  // sentence is how "1 hours after" ends up in the middle of a Bangla screen.
  'held.clock-after': {
    en: 'the till says this was rung {how_far} after it reached the shop: that device’s clock is wrong, so which day this belongs to needs a person',
    bn: 'কাউন্টার বলছে এটি দোকানে পৌঁছানোর {how_far} পরে তোলা হয়েছে: ওই যন্ত্রের ঘড়ি ভুল, তাই এটি কোন দিনের তা একজন মানুষকেই ঠিক করতে হবে',
  },
  'held.clock-before': {
    en: 'the till says this was rung {how_far} before it reached the shop: that device’s clock is wrong, so which day this belongs to needs a person',
    bn: 'কাউন্টার বলছে এটি দোকানে পৌঁছানোর {how_far} আগে তোলা হয়েছে: ওই যন্ত্রের ঘড়ি ভুল, তাই এটি কোন দিনের তা একজন মানুষকেই ঠিক করতে হবে',
  },
  'unit.moment': { en: 'under a minute', bn: 'এক মিনিটেরও কম' },
  'unit.minute': { en: '{count} minute', bn: '{count} মিনিট' },
  'unit.minutes': { en: '{count} minutes', bn: '{count} মিনিট' },
  'unit.hour': { en: '{count} hour', bn: '{count} ঘণ্টা' },
  'unit.hours': { en: '{count} hours', bn: '{count} ঘণ্টা' },
  'unit.day': { en: '{count} day', bn: '{count} দিন' },
  'unit.days': { en: '{count} days', bn: '{count} দিন' },
  'held.refund-against-nothing': {
    en: 'this reverses receipt {receipt_no}, and no sale here carries that number: it may be on a till whose sales have not arrived, or it may be a refund against nothing',
    bn: 'এটি {receipt_no} রসিদের টাকা ফেরত দেয়, অথচ এখানে ওই নম্বরের কোনো বিক্রি নেই: হতে পারে সেটি এমন কোনো কাউন্টারে যার বিক্রি এখনো আসেনি, নয়তো এটি এমন ফেরত যার পেছনে কোনো বিক্রিই নেই',
  },
  'held.refund-beyond-the-sale': {
    en: 'receipt {receipt_no} was rung for {sale} and {refunded} has now been refunded against it',
    bn: '{receipt_no} রসিদটি {sale} টাকার, আর এর বিপরীতে এ পর্যন্ত ফেরত দেওয়া হয়েছে {refunded}',
  },
  'held.tenders-do-not-add-up': {
    en: 'this says it was for {total} and carries {tendered} handed over with {change} given back: nobody paid what the ticket says it was for',
    bn: 'এতে লেখা আছে {total} টাকার, নেওয়া হয়েছে {tendered} আর ফেরত দেওয়া হয়েছে {change}: রসিদে যা লেখা তা কেউ দেয়নি',
  },
  'held.more-came-back': {
    en: 'more {item} has come back against receipt {receipt_no} than that receipt sold, by {over_by}: the money may be right and the goods are not',
    bn: '{receipt_no} রসিদে {item} যত বিক্রি হয়েছিল তার চেয়ে {over_by} বেশি ফেরত এসেছে: টাকা ঠিক থাকলেও মাল ঠিক নেই',
  },

  // ------------------------------------------- the refusals the shop's server gives
  //
  // Keyed by the code the core froze for a ProtocolError, and covered by a test
  // against server_refusals.json. These were the last words here that could
  // only be English, and they are among the worst ones for that: a save built
  // on a stale copy, a barcode another item already holds, an item the shop has
  // traded, a rate no till could price. Each is a moment an owner has to decide
  // something, which is exactly when a sentence in a language they do not read
  // is worth nothing.
  //
  // Two of these are named apart from the till's own refusal of the same shape
  // on purpose. A till refusing "not permitted" is a cashier who may not do
  // that; the server refusing it is a device that may not, and telling a
  // shopkeeper to fetch a supervisor would send them looking for the wrong fix.
  'device-needs-updating': {
    en: 'this device speaks version {requested} and the shop speaks {minimum} to {current}: it needs updating',
    bn: 'এই যন্ত্র {requested} সংস্করণে কথা বলে আর দোকান বলে {minimum} থেকে {current}: যন্ত্রটি হালনাগাদ করতে হবে',
  },
  'unknown-terminal': {
    en: 'the shop has no such till, or this one has been removed',
    bn: 'দোকানে এমন কোনো কাউন্টার নেই, নয়তো এটি সরিয়ে ফেলা হয়েছে',
  },
  malformed: {
    en: 'the shop could not read that request',
    bn: 'দোকান অনুরোধটি পড়তে পারেনি',
  },
  unauthenticated: {
    en: 'the shop does not recognise this device’s credential',
    bn: 'দোকান এই যন্ত্রের পরিচয় চিনতে পারছে না',
  },
  'too-many-attempts': {
    en: 'too many tries: wait {seconds} seconds',
    bn: 'অনেকবার চেষ্টা হয়েছে: {seconds} সেকেন্ড অপেক্ষা করুন',
  },
  'device-not-permitted': {
    en: 'this device may not do that: it is a till, not the back office',
    bn: 'এই যন্ত্র সেটি করতে পারে না: এটি কাউন্টার, ব্যাক অফিস নয়',
  },
  stale: {
    en: 'somebody else changed that while you had it open: read it again before saving',
    bn: 'আপনি খুলে রাখা অবস্থায় অন্য কেউ সেটি বদলেছে: সংরক্ষণের আগে আবার দেখে নিন',
  },
  'barcode-in-use': {
    en: 'another item you sell already has the barcode {barcode}: one barcode belongs to one item, or a scan rings whichever the till happens to find',
    bn: 'আপনার বিক্রি করা আরেকটি পণ্যের বারকোড আগে থেকেই {barcode}: একটি বারকোড একটি পণ্যেরই, নয়তো স্ক্যানে কাউন্টার যেটি আগে পায় সেটিই তোলে',
  },
  'item-has-history': {
    en: 'that has been sold, delivered or counted, so deleting it would take the name off figures the shop still has to answer for: stop selling it instead, which keeps the record and takes it off the tills',
    bn: 'এটি বিক্রি হয়েছে, এসেছে বা গোনা হয়েছে, তাই মুছে ফেললে দোকানকে যেসব হিসাবের জবাব দিতে হবে সেগুলো থেকে নামটি চলে যাবে: বরং বিক্রি বন্ধ করুন, তাতে রেকর্ড থাকে আর কাউন্টার থেকে সরে যায়',
  },
  // The three that replaced 'not-a-price', each with its figure named so the
  // sentence can be built in the shop's own language rather than around an
  // English clause. The old one stays: a server a version behind still sends it.
  'rate-is-not-a-rate': {
    en: '{rate}% is not a tax rate: a till would refuse the whole page of changes this arrived in, and stop seeing any of your prices',
    bn: '{rate}% ভ্যাটের হার হতে পারে না: কাউন্টার এটি যে পাতায় এসেছে সেই পুরো পাতাটিই ফিরিয়ে দেবে, আর আপনার কোনো দামই আর দেখবে না',
  },
  'price-below-nothing': {
    en: 'a price of {price} is below nothing: a till would refuse the whole page of changes this arrived in, and stop seeing any of your prices',
    bn: '{price} দাম শূন্যের নিচে: কাউন্টার এটি যে পাতায় এসেছে সেই পুরো পাতাটিই ফিরিয়ে দেবে, আর আপনার কোনো দামই আর দেখবে না',
  },
  'cost-below-nothing': {
    en: 'a cost of {cost} is below nothing: a till would refuse the whole page of changes this arrived in, and stop seeing any of your prices',
    bn: '{cost} ক্রয়মূল্য শূন্যের নিচে: কাউন্টার এটি যে পাতায় এসেছে সেই পুরো পাতাটিই ফিরিয়ে দেবে, আর আপনার কোনো দামই আর দেখবে না',
  },
  'not-a-price': {
    en: '{said}: a till would refuse the whole page of changes this arrived in, and stop seeing any of your prices',
    bn: '{said}: কাউন্টার এটি যে পাতায় এসেছে সেই পুরো পাতাটিই ফিরিয়ে দেবে, আর আপনার কোনো দামই আর দেখবে না',
  },

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

/// Something to be said later, in whatever language the screen shows then.
///
/// Every label on a screen follows the language, because the screen words it
/// again each time it draws. A message does not: it is worded once, at the
/// moment something goes wrong, and then it sits there. So a cashier who is
/// refused in English and switches the screen to Bangla to read it watches
/// every label around the sentence change and the sentence itself stay put.
/// That is the one line on the screen they needed in their own language, and
/// it is the only line that will not follow, which reads like the shop's
/// language switch is broken.
///
/// This holds the key and the figures rather than the sentence, and words it
/// when something reads it, which is when the screen draws. `languageNow` is
/// asked at that moment rather than passed, so the answer is the language on
/// the screen and not the language at the time of the trouble.
///
/// It is an object that says itself as text. That is what lets it be assigned
/// where a string was assigned before and rendered where a string was
/// rendered, including inside another message: `say` turns whatever fills a
/// brace into text, so a refusal folded into a sentence is worded late too.
export function worded(languageNow, key, fill = {}, otherwise = null) {
  return {
    key,
    /// The language as it was when this was built. Nothing reads it back, and
    /// that is the point: asking now means a screen that builds one of these
    /// while it is drawing has read the language, so the screen redraws when
    /// the language changes.
    ///
    /// Without it, a line of text followed the switch and an attribute did not.
    /// Text is read out of this object as the screen draws, so the screen sees
    /// the language being asked for; an attribute is written from it into the
    /// page, so the screen sees nothing and never draws again. The till's
    /// discount box kept its English placeholder in a Bangla shop for exactly
    /// that reason, found by looking at one.
    at: languageNow(),
    toString: () => say(languageNow(), key, fill, otherwise),
  };
}

/// A refusal to be worded later, the same way.
export function wordedRefusal(languageNow, view) {
  if (!view?.error) return null;
  return worded(languageNow, view.error_code ?? '', view.error_parts ?? {}, view.error);
}

/// The words the core needs to lay out a paper, in the language asked for.
///
/// Keyed exactly as `core/tests/paper_words.rs` freezes them, which is what
/// `apps/shared/paper_words.json` holds and what the test above checks. English
/// is the core's own default, so a language that says nothing changes nothing.
export function paperWords(language, keys) {
  const said = {};
  const wanted =
    keys ??
    Object.keys(WORDS)
      .filter((key) => key.startsWith('paper:'))
      .map((key) => key.slice('paper:'.length));
  for (const key of wanted) {
    const held = WORDS[`paper:${key}`];
    const phrase = held?.[language];
    if (phrase) said[key] = phrase;
  }
  return said;
}
