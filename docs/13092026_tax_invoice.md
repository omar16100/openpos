# The tax invoice, and what the Act actually says

**Purpose.** Every rule this product follows about Bangladeshi VAT, with the section it comes from,
so that nobody has to read the Act twice or work from a vendor blog again.
**Status.** Current. Nothing here is a claim of compliance; see *What is not claimed*.
**Last updated.** 13 September 2026.

## What it is for

Until today the notes in `todo.md` said the NBR rules in this project were vendor-blog sourced and
unverified, and that no claim of compliance should be made until somebody read a primary source.
Somebody has. This doc is what was read, what was done about it, and what was deliberately not done.

Two primary sources, both published by the National Board of Revenue:

- **The Act**: *The Value Added Tax and Supplementary Duty Act, 2012* (Act No. 47 of 2012), at
  `nbr.gov.bd/uploads/acts/18.pdf`. Its own title page marks it an **unofficial English
  translation**; the Bengali text governs. Every section number below is from it. A copy is kept at
  `data/research/vat_sd_act_2012_nbr_english.pdf` with its text beside it, so a later reading is of
  the same words this one was: the site serves it only to a browser-shaped request, and a document
  that has to be re-fetched to be checked is a document nobody checks.
- **The form**: *মূসক-৬.৩*, the কর চালানপত্র, at `nbr.gov.bd/uploads/form/Mushak_6.3_.pdf`, issued
  under rule 40(1)(c) and (f). Its text layer is legacy Bengali encoding and extracts as fragments,
  so the page was rendered and read as an image.

## What the Act says, and what this product does about it

### Section 51(1): what a tax invoice carries

A registered supplier issues a **serially numbered** tax invoice at or before the date VAT becomes
payable, carrying:

| Clause | What it asks for | Where it is in this product |
|---|---|---|
| (a) | date and time of issue | On the receipt and on the invoice, from the moment the sale was rung |
| (b) | supplier's name, address and BIN | The shop's own record, at the head of both papers |
| (c) | buyer's name, address and BIN, **where the value of the supply is more than 25,000 taka** | A written-down customer carries all three. The till says so at the counter once the sale's **net** passes the figure, which is the value clause (e) below defines |
| (d) | description and quantity, and the time and date of supply | The lines, and the same clock as (a) |
| (e) | value of the supply, excluding VAT | Computed by the pricing crate, never by a screen |
| (f) | the VAT rate | Per rate, and zero rated and exempt are named rather than both printed as 0% |
| (g) | the VAT payable | As above |
| (h) | the two added together | As above |
| (i) | anything else the Board prescribes | The form itself: see below |

The figure clause (c) measures is the supply **exclusive of VAT**, which is the Act reading itself:
clause (e) of the same list is "the value of the supply (exclusive of VAT)", and section 32(1) makes
the value of a taxable supply the consideration less the tax fraction of it. The till read the total
across the counter until 13 September 2026, which is that figure with the tax added back on: at
fifteen percent it asked for a BIN from 21,740 taka of goods upwards, three thousand early. Asking
early is not harmless. A message that appears when the law does not require it is a message
cashiers learn to wave away, including on the sale where it was right.

**Section 51(2)** is what makes (c) bite, and it bites the buyer rather than the shop: without those
details, **no input tax credit is admissible** against the invoice. A business customer finds that
out weeks later and comes back about it, which is why the till says it while they are still at the
counter rather than refusing the sale. The goods leave either way.

The figure lives in `core/src/domain/mod.rs` as `NAME_THE_BUYER_ABOVE`, beside the citation, and the
comparison is *more than*, tested at exactly 25,000 and one poisha either side.

### Sections 15(2) and 15(3): what the tax on a supply is

The rate is **15 percent** "unless otherwise provided in this Act" (15(3)), and 15(2) says how the
tax is arrived at: the payable VAT is the rate **multiplied by the value of the taxable supply**.

That one sentence is the arithmetic behind the Mushak's columns, and it is why this product refuses
to put certain lines on the form. An item whose tax is fixed to its listed price declares a tax that
is not the rate times the value charged for, so a line like it does not satisfy 15(2) on its face.
The product does not claim to know which regime such an item is under: it keeps the shop's own
answer, charges what the shop said, and refuses the form rather than printing a line an inspector
can disprove with a calculator. See *What will surprise you* below.

### Section 52: goods coming back, and the note that gives the tax back

A credit note is "a document issued by a taxpayer in support of a decreasing adjustment" (section
2(39)), and section 52(1) lists what one carries:

| Clause | What it asks for | Where it is on the note |
|---|---|---|
| (a) | the note's own serial number, and the date and time of issue | ক্রেডিট নোট নম্বর, ইস্যুর তারিখ, ইস্যুর সময়: the refund's receipt number and moment |
| (b) | supplier's name, address and BIN | ফেরত প্রদানকারী ব্যক্তির নাম and বিআইএন, from the shop's own record. The form has no address line where the Act asks for one; the receipt beside it carries the address |
| (c) | the serial number, **date and time** of the original tax invoice | মূল চালান নম্বর and মূল চালান ইস্যুর তারিখ. Both blank when the customer could not produce the receipt, and the date is known only when the refund was started from the number, which is how the shop finds the sale |
| (d) | the nature of the adjustment | ফেরতের কারণ, typed by whoever prints the note |
| (e) | the effect on the amount of VAT | মূসকের পরিমাণ and মোট কর |
| (f) | buyer's name, address and BIN, **where the VAT on the supply is more than 5,000 taka** | ফেরত গ্রহণকারী ব্যক্তির নাম and বিআইএন. Over that figure the note is not offered at all until the sale names somebody, because 52(2) is what it would be refused under |
| (g) | anything else identifying the adjustment | The line table the form prescribes |

The form is **মূসক-৬.৭**, prescribed by rule 40(1)(ছ) of the VAT and SD Rules, 2016 and printed in
the gazette of 3 November 2016. It is not in the list of forms the Board publishes on its website,
which is where this project looked first and wrongly concluded that no form existed: that list is
short, not authoritative. The Rules are kept at `data/research/vat_rules_2016_bn.pdf` and the form
is on page 98; its text layer is the same legacy Bengali encoding as the 6.3, so the page was
rendered and read as an image.

Two things about the form are worth knowing before reading the paper it produces. Its একক মূল্য
column is the unit price **including** VAT and supplementary duty, by its own footnote, where the
6.3's column of the same name is the value **excluding** tax: two forms, two definitions, and the
figure is worked out by the crate that priced the sale rather than by a screen. And every figure on
it prints as a size: a refund's arithmetic runs below nothing, the form says ফেরত in four places,
and a page of minus signs would be saying it twice and contradicting itself once.

**Section 52(2)** is sharper than 51(2): a note without clause (f) "shall not be used in support of
a claim for any decreasing adjustment". The buyer has already taken the credit on the way out, and
this is the paper that gives it back.

Two figures, and they are figures about different things. Section 51(1)(c) counts the **value of
the supply** and draws its line at 25,000 taka; section 52(1)(f) counts the **VAT** and draws its
line at 5,000. A shop reading only the first asks for a BIN on the way out and not on the way back.
They live side by side in `core/src/domain/mod.rs` as `NAME_THE_BUYER_ABOVE` and
`NAME_THE_BUYER_ON_A_CREDIT_ABOVE`, each beside its citation, each tested at the figure itself and
one poisha either side, and the till reads whichever one the direction of the goods calls for.

### Section 33(1): when the tax is payable

At the **first** of three moments: the supply being made, the invoice being issued, or any of the
consideration being received. Over a counter all three land together and the earliest is the supply.

So the tax figures count what was **sold**, not what was collected, and a sale on account is counted
the day the goods went. Counting collections would declare it late. There was no cash-basis choice
to make, which this project had left open as an unasked question.

### Sections 32(1) and the definition of "consideration": discounts

"Consideration" is the money paid or payable for a supply **but does not include any discount in
price given at the time of a supply**, and section 32(1) makes the value of a taxable supply the
consideration less the tax fraction of it. So an ordinary discount at the counter reduces what the
tax is charged on, which is what `VatBase::Discounted` does.

### Sections 55, 56 and 57: supplementary duty

- **55(1)** imposes it on importing goods, on supplying goods *manufactured* in Bangladesh, and on
  supplying services rendered here.
- **56** names the person liable: the importer, the supplier of the goods manufactured, or the
  supplier of the services.
- **55(5)**: payable at **one stage only**.
- **55(3)**: none is imposed on a zero-rated supply.
- **57(b)** with **32(1)**: the value for imposing the duty is the value of the supply less the duty
  itself, which puts the duty **inside** what VAT is charged on. Duty first, VAT on the sum.

A shop that buys goods in from a distributor and sells them over a counter is none of the three
persons in section 56, and the duty was borne before the goods reached it. **So this product does
not model supplementary duty, and that is an answer rather than a gap.** The column on the invoice
is left empty rather than printed as a nought, because a nought is a claim that none was due and
this code is not the thing that knows. A shop that imports, manufactures, or supplies services
carrying the duty needs more than this till does.

### Section 107: keeping records

Five years (107(1)), which is the figure this project's notes already carried from a vendor blog and
can now carry from the Act. Sub-section (2) lists what that covers, and two of its clauses bear on
this product directly: (b) all statements of sale, and (c) all tax invoices, credit notes and debit
notes issued **and received**. What openpos keeps is the sale, from which both papers are re-rendered
on demand; see *What is not claimed*.

### Rules 40 and 41: the books a shop keeps

The Act's section 107 says five years and what must be kept; the Rules say on which form. Two
clauses decide which book a retail shop keeps, and they turn on one fact about it:

- **Rule 40(1)(ক)**, the ক্রয় হিসাব পুস্তক on form মূসক-৬.১, is for purchases, **except where the
  person sells the same goods they buy**.
- **Rule 40(1)(খ)**, the বিক্রয় হিসাব পুস্তক on form মূসক-৬.২, is for a person in that excepted
  case: all their sales, **with the purchase details of the goods in them**.
- **Rule 41(ক)** asks an enlisted person, paying turnover tax rather than VAT, to keep both.

A shop that buys goods in and sells the same goods is exactly the excepted case, so a registered
retailer keeps মূসক-৬.২ and nothing else. That form is one page per product: what the shelf held,
what came in with the supplier's own invoice number and BIN beside it, what went out, and what is
left. Every figure is a quantity; there is no money on it.

The back office prints it from the movements the shop already has, which are the same movements
every other stock screen reads. A book that disagreed with what the shop says is on its shelves
would be two sets of books, and the one an inspector holds would be the second.

## The form, and the two papers

The Mushak 6.3 prescribes, in order: the registered person's name, BIN and the address of issue; the
buyer's name and BIN against the invoice number, date of issue and time of issue; **সরবরাহের
গন্তব্যস্থল**, where the supply is going; a ten column table (serial, description with brand, unit,
quantity, unit price excluding tax, total excluding tax, supplementary duty, VAT rate or specific
tax, VAT in taka, value including all duties and taxes) over a **সর্বমোট** row; and the name,
designation, signature and seal of the person responsible for the establishment.

This product prints **two different papers**, and the split follows the technology:

- **The counter receipt**: 32 characters of thermal text in English, laid out by the core, because no
  ESC/POS code page carries Bengali and the layout pads by counting characters.
- **The Mushak 6.3**: A4, in Bengali, laid out by the screen, because a ten column table is not text.
  Shared by the till and the back office as one component, `apps/shared/tax_invoice.svelte`.

Its words are Bengali in **both** language columns of the dictionary, which is the only place in this
product where that is right: the form is prescribed in Bengali, and a page headed "Tax Invoice" where
the rule says কর চালানপত্র is a different document. Two tests hold that line.

## What will surprise you

**A sale can be refused the invoice.** An item whose tax is fixed to its listed price keeps its tax
when a discount comes off: 100.00 with ten percent off is charged at 90.00 and taxed 15.00, both
figures right by the shop's own reckoning. Section 15(2) says the payable VAT is the rate multiplied
by the value of the supply, which on 90.00 is 13.50, and the form gives a line the value excluding
tax, the rate and the tax with **no column for the amount the rate was charged on**. So anybody
multiplying the two columns the form does have gets a third figure. The invoice is refused for such
a sale on both screens, with the line named. A shopkeeper can settle that with their accountant;
they cannot unprint a page an inspector has disproved.

**No tax invoice is offered for goods coming back.** The form is the paper for a supply and a return
is not one: it is a decreasing adjustment, which section 52 gives a credit note for. The button used
to be there and laid one out, with a quantity of −1 and a total below nothing, headed কর চালানপত্র.
Both screens now say what the shop is not being handed instead, because a shopkeeper who is told
nothing hands over the receipt and believes the paper side is done.

**The signature block prints empty.** Three of its four lines are made by a hand holding a pen, and
printing a cashier's name into the first would be this till claiming somebody signed.

**সরবরাহের গন্তব্যস্থল is not the buyer's address.** The form asks where the supply is going. This
product holds where the buyer is, prints that, and leaves the line correctable by hand, which is what
a form is for.

## What is not claimed

This is not a certified Mushak 6.3 and no compliance is claimed. Known distance from the form:

- The destination of supply is filled with the buyer's address, which is a different question when
  goods are delivered.
- The supplementary duty column is empty by the reasoning above, which is right for a retailer and
  wrong for anybody else.
- No fiscal number from an EFD or SDC appears anywhere. That is a separate regime and untouched.
- Which figure a monthly return wants for a listed-price line is still an open question in
  `todo.md`, and section 15(2) sharpens rather than settles it: the payable VAT is the rate times
  the value of the supply, which on a discounted listed-price line is less than the shop charged the
  customer. What becomes of the difference is not answered anywhere this project has read, and it is
  a question for an accountant rather than for this code.
- The shop keeps the sale, not the document as issued. Section 107(2)(c) asks a taxpayer to keep all
  tax invoices and credit notes issued and received, for five years (107(1)). Every paper here is
  re-rendered from the sale on demand, so what is kept is everything the paper was made from rather
  than an image of the paper, and a second printing is marked as a copy.
- The Bengali on the form is quoted from the form. The Bengali around it, on the screens, has still
  not been read by a native speaker.
- The credit note carries no address for either party, because the form has no line for one, while
  section 52(1)(b) and (f) ask for both. The receipt beside it carries the shop's.
- The sales book is printed one product at a time, on the screen, by somebody who asks for it. A
  shop of eight hundred lines has eight hundred pages and no way to ask for all of them at once.
- The book shows what the shop's own movements say, including a shelf that runs below nothing. A
  shop that sells goods it never booked in has a book that says so, which is the truth about its
  records rather than a fault in the page.
- মূসক-৬.১, the purchase book, is not printed. Rule 40(1)(ক) excepts a shop that sells the goods it
  buys, which is the shop this product is for; a shop that is not one needs a book this does not
  keep.
- Nothing stores the reason for a return. The form's ফেরতের কারণ is typed by whoever prints the note
  and a reprint asks again, so two printings of one note can carry different words. The person
  printing it is the person who knows, which is why it is asked there; it is a gap all the same.
- The note's supplementary duty line is empty for the reason the invoice's column is, and its
  বাদ কর্তন line is empty because this till hands back what the goods came to and keeps nothing.
