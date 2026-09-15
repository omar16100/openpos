//! Crash-recovery properties.
//!
//! The storage review asked for fault injection from day one, on the grounds
//! that a test culture for crash recovery is never retrofitted. These tests
//! break the device at every operation boundary of a realistic workload and
//! assert the three invariants a shopkeeper is entitled to:
//!
//! 1. **No committed sale is ever lost.** If `commit` returned `Ok`, the flush
//!    succeeded, a receipt may have printed, and the customer has walked away.
//!    That sale must survive any subsequent crash.
//! 2. **No uncommitted sale is ever surfaced.** A half-written frame must never
//!    be read back as a real sale. Inventing a sale is worse than losing one,
//!    because it silently corrupts the day's cash reconciliation.
//! 3. **A snapshot that was completed is always loadable.** Cold start is the
//!    product's one promise; a checkpoint that dies halfway must never leave the
//!    till unable to boot.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::storage::backend::{Backend, Fault, FaultyBackend, MemoryBackend};
use openpos_core::storage::frame::{PayloadKind, Store};
use openpos_core::storage::journal::Journal;
use proptest::prelude::*;

const TENANT: u128 = 42;
const TERMINAL: u128 = 7;

/// A step in a shop's day.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// A sale, committed durably before its receipt would print.
    Sale(Vec<u8>),
    /// Catalogue changes pulled from the server.
    Delta(Vec<u8>),
    /// Fold the deltas into a new snapshot.
    Checkpoint(Vec<u8>),
}

/// What survived, and what the journal promised while running.
struct Outcome {
    /// Payloads of sales whose commit returned `Ok`. These are the promises.
    acknowledged_sales: Vec<Vec<u8>>,
    /// Payload of the last checkpoint that completed, if any.
    last_snapshot: Option<Vec<u8>>,
    /// The device image as it would be found after a power cut.
    durable: MemoryBackend,
}

/// Run a day's work against a device that may break, and report what was
/// promised and what physically survived.
fn run(ops: &[Op], fault: Option<(usize, Fault)>) -> Outcome {
    let mut backend = FaultyBackend::new();
    if let Some((at, kind)) = fault {
        backend = backend.with_fault(at, kind);
    }

    let Ok((mut journal, _)) = Journal::open(backend, TENANT, TERMINAL, 1) else {
        return Outcome {
            acknowledged_sales: Vec::new(),
            last_snapshot: None,
            durable: MemoryBackend::new(),
        };
    };

    let mut acknowledged_sales = Vec::new();
    let mut last_snapshot = None;

    for op in ops {
        match op {
            Op::Sale(payload) => {
                if journal
                    .commit(Store::Critical, PayloadKind::SaleCommit, 1, payload)
                    .is_ok()
                {
                    acknowledged_sales.push(payload.clone());
                }
            }
            Op::Delta(payload) => {
                let _ = journal.commit(Store::ReplicaCache, PayloadKind::ItemDeltas, 1, payload);
            }
            Op::Checkpoint(payload) => {
                if journal.checkpoint(payload).is_ok() {
                    last_snapshot = Some(payload.clone());
                }
            }
        }
    }

    Outcome {
        acknowledged_sales,
        last_snapshot,
        durable: journal.backend().durable(),
    }
}

/// Reopen the surviving image, the way a till does after a power cut.
fn reboot(durable: MemoryBackend) -> (Vec<Vec<u8>>, Option<Vec<u8>>) {
    let (journal, _recovery) =
        Journal::open(durable, TENANT, TERMINAL, 1).expect("a till must always reopen");
    let sales = journal
        .read(Store::Critical)
        .expect("the critical log must be readable after recovery")
        .into_iter()
        .map(|record| record.payload)
        .collect();
    let snapshot = journal
        .load_snapshot()
        .expect("snapshot read must not fail")
        .map(|(_schema, bytes)| bytes);
    (sales, snapshot)
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 1..40).prop_map(Op::Sale),
        prop::collection::vec(any::<u8>(), 1..40).prop_map(Op::Delta),
        prop::collection::vec(any::<u8>(), 1..60).prop_map(Op::Checkpoint),
    ]
}

fn fault_strategy() -> impl Strategy<Value = Fault> {
    prop_oneof![
        Just(Fault::Fail),
        Just(Fault::PowerCut),
        (1_usize..50).prop_map(|bytes| Fault::Tear { bytes }),
    ]
}

proptest! {
    /// Invariants 1 and 2 together, with the device breaking at an arbitrary
    /// operation boundary.
    #[test]
    fn acknowledged_sales_survive_and_nothing_else_appears(
        ops in prop::collection::vec(op_strategy(), 1..10),
        fault_at in 0_usize..40,
        fault in fault_strategy(),
    ) {
        let outcome = run(&ops, Some((fault_at, fault)));
        let (recovered, _) = reboot(outcome.durable);

        // 1. Every sale the journal acknowledged is still there, in order.
        //    A receipt may already be in a customer's hand for each of these.
        let acknowledged = &outcome.acknowledged_sales;
        prop_assert!(
            recovered.len() >= acknowledged.len(),
            "lost an acknowledged sale: promised {}, found {}",
            acknowledged.len(),
            recovered.len()
        );
        for (position, promised) in acknowledged.iter().enumerate() {
            prop_assert_eq!(
                &recovered[position],
                promised,
                "acknowledged sale {} came back changed or missing",
                position
            );
        }

        // 2. Nothing else appears. A commit that returned an error must leave no
        //    trace at all: the cashier saw it fail and will re-ring the basket
        //    under a new id, so a surviving frame would become a second sale for
        //    the same goods.
        prop_assert_eq!(
            &recovered,
            acknowledged,
            "recovery surfaced a sale that was never acknowledged"
        );
    }

    /// Invariant 3: a completed checkpoint is always loadable afterwards, no
    /// matter what broke later. This is the cold-start promise.
    #[test]
    fn a_completed_snapshot_always_loads(
        ops in prop::collection::vec(op_strategy(), 1..10),
        fault_at in 0_usize..40,
        fault in fault_strategy(),
    ) {
        let outcome = run(&ops, Some((fault_at, fault)));
        let had_snapshot = outcome.last_snapshot.is_some();
        let (_, snapshot) = reboot(outcome.durable);

        if had_snapshot {
            prop_assert!(
                snapshot.is_some(),
                "a checkpoint completed but the till cannot cold start"
            );
        }
    }

    /// The journal always reopens. Recovery may discard a torn tail, but it must
    /// never refuse to start: a till that will not boot is a shop that cannot
    /// trade.
    #[test]
    fn recovery_never_refuses_to_open(
        ops in prop::collection::vec(op_strategy(), 1..10),
        fault_at in 0_usize..40,
        fault in fault_strategy(),
    ) {
        let outcome = run(&ops, Some((fault_at, fault)));
        let opened = Journal::open(outcome.durable, TENANT, TERMINAL, 1);
        prop_assert!(opened.is_ok());
    }

    /// Without any fault, everything committed is present and in order. The
    /// happy path is worth pinning too, so a regression cannot hide behind the
    /// fault-injection cases.
    #[test]
    fn a_clean_run_keeps_everything(ops in prop::collection::vec(op_strategy(), 1..10)) {
        let outcome = run(&ops, None);
        let (recovered, snapshot) = reboot(outcome.durable);

        prop_assert_eq!(&recovered, &outcome.acknowledged_sales);
        prop_assert_eq!(snapshot, outcome.last_snapshot);
    }
}

/// Every byte-level truncation of a real log, which is what a torn write looks
/// like from the outside. Deterministic rather than random, so the whole space is
/// covered rather than sampled.
#[test]
fn every_truncation_point_recovers_to_a_valid_prefix() {
    let mut source = MemoryBackend::new();
    {
        let (mut journal, _) = Journal::open(source.clone(), TENANT, TERMINAL, 1).unwrap();
        for index in 0..6_u8 {
            journal
                .commit(Store::Critical, PayloadKind::SaleCommit, 1, &[index; 12])
                .unwrap();
        }
        source = journal.backend().clone();
    }
    let whole = source.read_log(Store::Critical).unwrap();

    for cut in 0..=whole.len() {
        let mut damaged = source.clone();
        damaged.truncate_log(Store::Critical, cut).unwrap();

        let (journal, recovery) = Journal::open(damaged, TENANT, TERMINAL, 1).unwrap();
        let records = journal.read(Store::Critical).unwrap();

        // Whatever survived is a prefix of what was written, never a mixture.
        for (position, record) in records.iter().enumerate() {
            let expected = [u8::try_from(position).unwrap(); 12];
            assert_eq!(
                record.payload, expected,
                "truncating at {cut} produced a sale that was never written"
            );
        }
        // And the journal reports honestly whether it had to discard anything.
        // Cutting exactly on a frame boundary leaves a shorter but perfectly
        // valid log, so only a cut inside a frame counts as a discard.
        let frame_size = whole.len() / 6;
        let on_a_boundary = cut % frame_size == 0;
        assert_eq!(
            recovery.critical_discarded > 0,
            !on_a_boundary,
            "cut at {cut} of {} misreported its discard",
            whole.len()
        );
    }
}
