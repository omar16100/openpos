//! How much mishearing the spoken lookup absorbs before it stops being useful,
//! and how often it is confidently wrong.
//!
//! The question this answers comes before the one in the plan doc. That gate is
//! about which recogniser to pick, and it needs a person reading product names
//! into a tablet. This is the question underneath it: whatever the recogniser
//! gets wrong, does anything downstream survive it? If the answer is no, then no
//! model is worth ninety-four megabytes and the feature should not be built at
//! all. Nobody had that number, and it needs no audio to get.
//!
//! Three figures come out, and the third is the one that decides whether this
//! can be put in a shop:
//!
//! - **found**: the right item was offered at all, anywhere in the list.
//! - **first**: the right item was the one at the top.
//! - **confidently wrong**: the till said it was sure, and it was sure of
//!   something else. This is the only outcome a cashier cannot see. A miss is a
//!   list they ignore; a wrong-but-hedged row is a row they check. A confident
//!   wrong row looks exactly like a confident right one.
//!
//! The corruptions model a transducer rather than random noise, because that is
//! what a transducer does. It cannot emit a word outside its lexicon, so when it
//! mishears it substitutes the nearest thing it does know: errors are biased and
//! repeatable, not scattered. So a substitution here draws from the shop's own
//! vocabulary rather than from nonsense, which is the harder and more honest
//! case. Dropping models a fan or a clipped start; splitting models a conjunct
//! mis-segmented, which Bangla gives ample opportunity for; a digit swap models
//! the single most expensive misread on a shelf full of sizes.
//!
//! Deterministic: the same seed gives the same figures, so a change to the
//! matching rules can be measured against this rather than argued about.
//!
//! Run with: `cargo run --release --example voice_accuracy`

// A harness builds synthetic data and divides counts by a total. Plain
// arithmetic is the right tool here; the workspace bans it in the code that
// handles real money.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use openpos_core::domain::{PriceMode, Supply, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::{Item, Replica};
use openpos_core::voice::{resolve, understand};

/// Error rates to report, as a percentage of words corrupted.
///
/// Up to forty because that is where a cheap microphone in a noisy shop
/// plausibly lands, well past the seventeen to thirty-three the model cards
/// report for read speech in a quiet room.
const RATES: &[u32] = &[0, 10, 20, 30, 40];

/// How many times each item is tried at each rate. Large enough that the third
/// figure, which is the rare one, is not a rounding artefact.
const TRIES: usize = 400;

/// What a shop here actually has on its shelves, in the words on the packet.
///
/// The demo catalogue, plus the crowd around it: a real shop has one rice a
/// cashier can name and forty things sharing the word for oil, and a harness
/// built only from distinctive names would flatter every rule being measured.
fn shop() -> (Replica, Vec<(Ulid, String)>) {
    let named: Vec<&str> = vec![
        "মিনিকেট চাল ৫ কেজি",
        "নাজিরশাইল চাল ৫ কেজি",
        "সয়াবিন তেল ১ লিটার",
        "সরিষার তেল ৫০০ মিলি",
        "মসুর ডাল ১ কেজি",
        "চিনি ১ কেজি",
        "চা ৪০০ গ্রাম",
        "গুঁড়া দুধ ৫০০ গ্রাম",
        "মিষ্টি দই ৫০০ গ্রাম",
        "মুড়ি ৫০০ গ্রাম",
        "বিস্কুট প্যাকেট",
        "সাবান বার",
    ];
    // The crowd. Every one of these carries a word the named items also carry,
    // which is what makes a single common word worth nothing on its own.
    let crowd: Vec<String> = (0..40)
        .map(|i| match i % 4 {
            0 => format!("রান্নার তেল {i}"),
            1 => format!("প্যাকেট চাল {i}"),
            2 => format!("গুঁড়া মশলা {i}"),
            _ => format!("দই {i}"),
        })
        .collect();

    let mut items = Vec::new();
    let mut wanted = Vec::new();
    for (at, name) in named.iter().map(|n| (*n).to_string()).chain(crowd).enumerate() {
        let id = Ulid::from_u128(at as u128 + 1);
        if at < named.len() {
            wanted.push((id, name.clone()));
        }
        items.push(Item {
            id,
            code: format!("SKU{at:04}").into(),
            name_en: format!("Item {at}").into(),
            name_bn: name.into(),
            unit: "Nos".into(),
            price: Minor::new(4_300),
            cost: Minor::new(3_800),
            vat_rate: Bp::new(1_500).unwrap_or(Bp::ZERO),
            price_mode: PriceMode::Exclusive,
            vat_base: VatBase::Discounted,
            supply: Supply::Standard,
            category: "".into(),
            barcodes: vec![format!("{}", 8_690_000_000_000_u64 + at as u64).into()],
            on_hand: Milli::new(40_000),
            active: true,
        });
    }
    (Replica::from_items(items), wanted)
}

/// A reproducible stream. No dependency, and the same figures every run.
struct Rolls(u64);

impl Rolls {
    fn next(&mut self) -> u64 {
        // xorshift64. Good enough to shuffle corruptions, and it is not
        // pretending to be anything else.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn upto(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next() % bound as u64) as usize
    }

    /// True `percent` times in a hundred.
    fn hits(&mut self, percent: u32) -> bool {
        (self.next() % 100) < u64::from(percent)
    }
}

/// Every word the shop's own names are made of, which is the lexicon a
/// transducer trained on this shop would substitute from.
fn vocabulary(replica: &Replica) -> Vec<String> {
    let mut words: Vec<String> = replica
        .items()
        .iter()
        .flat_map(|item| {
            item.name_bn
                .split_whitespace()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Mishear an utterance the way a transducer mishears one.
fn misheard(clean: &str, percent: u32, vocab: &[String], rolls: &mut Rolls) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in clean.split_whitespace() {
        if !rolls.hits(percent) {
            out.push(word.to_string());
            continue;
        }
        match rolls.upto(4) {
            // Lost to a fan, or clipped off the front.
            0 => {}
            // Substituted with the nearest thing the lexicon holds. Drawn from
            // this shop's own words, which is the hard case: the wrong word is
            // one that matches something.
            1 => out.push(vocab[rolls.upto(vocab.len())].clone()),
            // A conjunct mis-segmented, which Bangla offers on most words.
            2 => {
                let chars: Vec<char> = word.chars().collect();
                if chars.len() > 2 {
                    let at = 1 + rolls.upto(chars.len() - 2);
                    out.push(chars[..at].iter().collect());
                    out.push(chars[at..].iter().collect());
                } else {
                    out.push(word.to_string());
                }
            }
            // A digit read wrong, which on a shelf of sizes is the expensive one.
            _ => out.push(
                word.chars()
                    .map(|c| {
                        if c.is_ascii_digit() || ('\u{09E6}'..='\u{09EF}').contains(&c) {
                            char::from_u32(u32::from(c) ^ 1).unwrap_or(c)
                        } else {
                            c
                        }
                    })
                    .collect(),
            ),
        }
    }
    out.join(" ")
}

fn main() {
    let (replica, wanted) = shop();
    let vocab = vocabulary(&replica);
    println!(
        "{} items, {} of them asked for, {} tries each per rate\n",
        replica.len(),
        wanted.len(),
        TRIES
    );
    println!("  words   found   first   confidently wrong");
    println!("  wrong                   (the one nobody can see)");

    for &rate in RATES {
        let mut rolls = Rolls(0x5EED_1234_ABCD_0001);
        let (mut found, mut first, mut sure_wrong, mut total) = (0usize, 0usize, 0usize, 0usize);
        // The average hides a cliff. A transducer makes the same substitution
        // every time it meets the same sound, so a shop's worst-named item can
        // be reliably unfindable while the mean looks healthy, and it is the
        // shop's worst item that generates the complaint.
        let mut worst = (100.0_f64, String::new());

        for (id, clean) in &wanted {
            let mut mine = 0usize;
            for _ in 0..TRIES {
                let said = misheard(clean, rate, &vocab, &mut rolls);
                let heard = understand(&said);
                let out = resolve(&replica, &heard, 5);
                total += 1;
                if out.candidates.contains(id) {
                    found += 1;
                }
                let top = out.candidates.first().copied();
                if top == Some(*id) {
                    first += 1;
                    mine += 1;
                } else if out.sure && top.is_some() {
                    sure_wrong += 1;
                }
            }
            let rate_for_item = 100.0 * mine as f64 / TRIES as f64;
            if rate_for_item < worst.0 {
                worst = (rate_for_item, clean.clone());
            }
        }

        let pc = |n: usize| 100.0 * n as f64 / total as f64;
        println!(
            "  {rate:>4}%  {:>5.1}%  {:>5.1}%  {:>5.2}%     worst item {:>5.1}%  {}",
            pc(found),
            pc(first),
            pc(sure_wrong),
            worst.0,
            worst.1
        );
    }

    println!(
        "\nfound: the right item was offered. first: it was at the top.\n\
         confidently wrong: the till said it was sure, of something else. That last\n\
         one is the only outcome a cashier cannot tell from a correct one, which is\n\
         why the confidence rules are calibrated to refuse rather than to guess."
    );
}
