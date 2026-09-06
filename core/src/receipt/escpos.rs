//! Receipt lines as bytes a thermal printer understands.
//!
//! ESC/POS is what every cheap 58mm and 80mm printer speaks, and the parts of it
//! a receipt needs are small: wake up, turn emphasis on and off, send text, feed
//! the paper, cut it. That is the whole of this file.
//!
//! # Bengali
//!
//! An ESC/POS printer renders text from a codepage burned into its firmware, and
//! no standard codepage contains Bengali. A printer handed Bengali bytes prints
//! whatever those byte values happen to mean in its current codepage, which is
//! Latin letters and box-drawing characters: mojibake, on paper, in a customer's
//! hand. Printing Bengali properly means rasterising it and sending an image,
//! which needs font data this crate has no business carrying.
//!
//! So text that cannot be represented is replaced with a visible mark and the
//! line is reported. A platform with a raster path can print those lines as
//! images; a platform without one at least knows which lines it is failing to
//! print, rather than finding out from a shopkeeper. Silently emitting the bytes
//! and hoping is the option this deliberately does not take.

use alloc::vec::Vec;

use super::Line;
use crate::receipt::Emphasis;

/// Wake the printer and clear whatever the last job left set.
const INIT: [u8; 2] = [0x1B, 0x40];
/// `ESC E n`: emphasis on and off.
const BOLD_ON: [u8; 3] = [0x1B, 0x45, 0x01];
const BOLD_OFF: [u8; 3] = [0x1B, 0x45, 0x00];
/// `ESC d n`: feed n lines, so the cut does not fall across the last one.
const FEED: [u8; 2] = [0x1B, 0x64];
/// `GS V 66 n`: feed and partial cut, leaving a tab the customer tears off.
const CUT: [u8; 4] = [0x1D, 0x56, 0x42, 0x00];

/// Stands in for a character the printer cannot render, chosen because it is
/// unmistakably not the text that was meant.
const UNPRINTABLE: u8 = b'?';

/// What the printer is and how the job should end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Printer {
    /// Lines of blank paper before the cut, so the cut does not land on the
    /// last line of text and so the customer has something to hold.
    pub feed_lines: u8,
    /// Whether to cut. A printer without a cutter ignores the command on some
    /// models and prints its bytes as text on others, so it is asked for
    /// rather than assumed.
    pub cut: bool,
}

impl Default for Printer {
    fn default() -> Self {
        Self {
            feed_lines: 4,
            cut: true,
        }
    }
}

/// A job, and what it could not say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub bytes: Vec<u8>,
    /// Indices of lines that held characters this encoding cannot carry.
    ///
    /// Not an error, because a receipt with one unprintable line is still worth
    /// printing, and not silence, because a shopkeeper should not be the one to
    /// discover it. A platform with a raster path prints these as images.
    pub unprintable: Vec<usize>,
}

impl Job {
    /// Whether every line printed as written.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unprintable.is_empty()
    }
}

/// Turn laid-out lines into bytes for a thermal printer.
#[must_use]
pub fn encode(lines: &[Line], printer: &Printer) -> Job {
    let mut bytes = Vec::with_capacity(lines.len().saturating_mul(40));
    let mut unprintable = Vec::new();
    bytes.extend_from_slice(&INIT);

    let mut emphasised = false;
    for (index, line) in lines.iter().enumerate() {
        let wants = matches!(line.emphasis, Emphasis::Strong);
        // Only when it changes: a printer told to turn bold on before every
        // line does redundant work, and on some firmware it drops a character.
        if wants != emphasised {
            bytes.extend_from_slice(if wants { &BOLD_ON } else { &BOLD_OFF });
            emphasised = wants;
        }

        let mut lost = false;
        for character in line.text.chars() {
            match u8::try_from(u32::from(character)) {
                // The printable ASCII range, which is what every codepage
                // agrees on. Anything above it means something different on
                // each printer, which is worse than a mark that means nothing.
                Ok(byte) if (0x20..=0x7E).contains(&byte) => bytes.push(byte),
                _ => {
                    bytes.push(UNPRINTABLE);
                    lost = true;
                }
            }
        }
        if lost {
            unprintable.push(index);
        }
        bytes.push(b'\n');
    }

    if emphasised {
        // Left on, the next job starts bold. A printer keeps this across jobs.
        bytes.extend_from_slice(&BOLD_OFF);
    }
    bytes.extend_from_slice(&FEED);
    bytes.push(printer.feed_lines);
    if printer.cut {
        bytes.extend_from_slice(&CUT);
    }

    Job { bytes, unprintable }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use alloc::string::String;
    use alloc::vec;

    use super::*;

    fn line(text: &str, emphasis: Emphasis) -> Line {
        Line {
            text: String::from(text),
            emphasis,
        }
    }

    #[test]
    fn a_job_wakes_the_printer_and_ends_by_cutting() {
        let job = encode(
            &[line("Total 100.00", Emphasis::Normal)],
            &Printer::default(),
        );

        assert!(
            job.bytes.starts_with(&INIT),
            "a printer keeps the last job's settings"
        );
        assert!(job.bytes.ends_with(&CUT));
        assert!(job.is_complete());
    }

    #[test]
    fn emphasis_is_switched_only_when_it_changes() {
        let job = encode(
            &[
                line("shop", Emphasis::Strong),
                line("also shop", Emphasis::Strong),
                line("plain", Emphasis::Normal),
            ],
            &Printer::default(),
        );

        // Once on, once off. A printer told to turn bold on before every line
        // does redundant work and on some firmware drops a character.
        let ons = job.bytes.windows(3).filter(|w| *w == BOLD_ON).count();
        let offs = job.bytes.windows(3).filter(|w| *w == BOLD_OFF).count();
        assert_eq!(ons, 1);
        assert_eq!(offs, 1);
    }

    #[test]
    fn emphasis_is_turned_off_before_the_job_ends() {
        let job = encode(&[line("TOTAL", Emphasis::Strong)], &Printer::default());

        // Left on, the next customer's receipt starts bold: the setting outlives
        // the job.
        let offs = job.bytes.windows(3).filter(|w| *w == BOLD_OFF).count();
        assert_eq!(offs, 1);
    }

    #[test]
    fn bengali_is_reported_rather_than_printed_as_nonsense() {
        let job = encode(
            &[
                line("Rice Miniket 5kg", Emphasis::Normal),
                line("মিনিকেট চাল ৫ কেজি", Emphasis::Normal),
            ],
            &Printer::default(),
        );

        // No standard ESC/POS codepage carries Bengali. Sending the bytes and
        // hoping prints Latin letters and box drawing in a customer's hand.
        assert_eq!(job.unprintable, vec![1]);
        assert!(!job.is_complete());
        // And the line that could be printed still was.
        assert!(job.bytes.windows(4).any(|w| w == b"Rice"));
    }

    #[test]
    fn every_byte_of_text_is_one_a_printer_agrees_about() {
        let job = encode(
            &[line("Total 1,234.56 - x/y (z)", Emphasis::Normal)],
            &Printer::default(),
        );

        // Anything above 0x7E means something different on each printer's
        // codepage, so nothing above it is ever emitted as text.
        let text: Vec<u8> = job
            .bytes
            .iter()
            .copied()
            .filter(|byte| *byte >= 0x20)
            .collect();
        assert!(text.iter().all(|byte| *byte <= 0x7E), "{text:?}");
        assert!(job.is_complete());
    }

    #[test]
    fn a_printer_with_no_cutter_is_not_sent_a_cut() {
        let job = encode(
            &[line("Thank you", Emphasis::Normal)],
            &Printer {
                feed_lines: 2,
                cut: false,
            },
        );

        // Some models print the command as text instead of ignoring it.
        assert!(!job.bytes.windows(4).any(|w| w == CUT));
        assert!(job.bytes.ends_with(&[0x1B, 0x64, 2]));
    }

    #[test]
    fn an_empty_receipt_still_produces_a_valid_job() {
        // A caller with nothing to print should get a harmless job rather than
        // a stream a printer will sit waiting on.
        let job = encode(&[], &Printer::default());
        assert!(job.bytes.starts_with(&INIT));
        assert!(job.bytes.ends_with(&CUT));
        assert!(job.is_complete());
    }
}
