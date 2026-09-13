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

### Section 52: goods coming back, and the note that gives the tax back

A credit note is "a document issued by a taxpayer in support of a decreasing adjustment" (section
2(39)), and section 52(1) lists what one carries:

| Clause | What it asks for | Where it is in this product |
|---|---|---|
| (a) | the note's own serial number, and the date and time of issue | The refund's own receipt number and its date and time, on the paper |
| (b) | supplier's name, address and BIN | The shop's own record, at the head of the paper |
| (c) | the serial number, **date and time** of the original tax invoice | The number only, printed as "against T1-000100", and only when the cashier was given it |
| (d) | the nature of the adjustment | REFUND at the head, and the lines. Not a phrase naming why |
| (e) | the effect on the amount of VAT | The VAT line, below nothing, on the refund's own totals |
| (f) | buyer's name, address and BIN, **where the VAT on the supply is more than 5,000 taka** | The till says so at the counter, by the same route as the invoice rule |
| (g) | anything else identifying the adjustment | Not attempted |

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

### Section 107(1): keeping records

Five years, which is the figure this project's notes already carried from a vendor blog and can now
carry from the Act.

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
figures right. The form gives a line the value excluding tax, the rate and the tax, and **no column
for the amount the rate was charged on**, so anybody multiplying the two columns it does have gets
13.50. The invoice is refused for such a sale on both screens, with the line named. A shopkeeper can
settle that with their accountant; they cannot unprint a page an inspector has disproved.

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
- Which figure a monthly return wants for a listed-price line, where the tax is not the rate times
  the taxable amount, is still an open question in `todo.md`.
- The Bengali on the form is quoted from the form. The Bengali around it, on the screens, has still
  not been read by a native speaker.
- The refund paper is not a prescribed credit note. It carries most of what section 52(1) lists and
  is headed REFUND rather than by the form's own name, it names the original invoice by number
  without its date and time, and it states no nature of adjustment beyond the lines themselves. What
  the Board prescribes for a credit note has not been read from a primary source, and nothing here
  will print a form's name until it has been.
