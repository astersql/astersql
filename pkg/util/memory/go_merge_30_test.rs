// Copyright 2026 AsterSQL.

use crate::NewDigestIDBuilder;
#[cfg(feature = "mem-arbitrator")]
use crate::tracker::NewTracker;

#[test]
fn go_merge_30_digest_builder_distinguishes_component_boundaries() {
    let mut first = NewDigestIDBuilder();
    first.AddString("ab");
    first.AddString("c");
    let mut second = NewDigestIDBuilder();
    second.AddString("a");
    second.AddString("bc");
    assert_ne!(first.Sum64(), second.Sum64());
    assert_ne!(first.Sum64(), 0);
    assert_eq!(first.Sum64(), first.Sum64());
}

#[test]
#[cfg(feature = "mem-arbitrator")]
fn go_merge_30_reversal_offsets_root_usage_until_release() {
    use crate::arbitrator::{ArbitrationPriorityMedium, NewMemArbitrator};
    use std::sync::Arc;

    let core = Arc::new(NewMemArbitrator(10_000));
    let mut tracker = NewTracker(1, -1);
    assert!(tracker.InitMemArbitrator(
        Some(core),
        None,
        17,
        ArbitrationPriorityMedium,
        false,
        1_000,
        false,
    ));
    tracker.Consume(100);
    let reversal = tracker.AddReversal(60);
    let usage = tracker.MemArbitrator.as_ref().unwrap().MemUsage();
    assert_eq!(usage.HeapInuse, 100);
    assert_eq!(usage.RootPoolUsed, 40);
    reversal.Release();
    assert_eq!(
        tracker
            .MemArbitrator
            .as_ref()
            .unwrap()
            .MemUsage()
            .RootPoolUsed,
        100
    );
}

#[test]
#[cfg(feature = "mem-arbitrator")]
fn go_merge_30_detached_tracker_does_not_recharge_arbitrator() {
    use crate::arbitrator::{ArbitrationPriorityMedium, NewMemArbitrator};
    use std::sync::Arc;

    let core = Arc::new(NewMemArbitrator(10_000));
    let mut tracker = NewTracker(1, -1);
    assert!(tracker.InitMemArbitrator(
        Some(core),
        None,
        0,
        ArbitrationPriorityMedium,
        false,
        0,
        false,
    ));
    assert!(tracker.DetachMemArbitrator(false));
    tracker.Consume(100);
    assert_eq!(tracker.BytesConsumed(), 100);
    assert_eq!(
        tracker.MemArbitrator.as_ref().unwrap().MemUsage().HeapInuse,
        0
    );
}

#[test]
#[cfg(feature = "mem-arbitrator")]
fn go_merge_30_cached_promotion_records_large_memory_for_priority_buffer() {
    use crate::arbitrator::{ArbitrationPriorityMedium, ArbitratorModePriority, NewMemArbitrator};
    use std::sync::Arc;

    let core = Arc::new(NewMemArbitrator(10_000));
    core.SetWorkMode(ArbitratorModePriority);
    core.UpdateDigestProfileCache(17, 100, core.approxUnixTimeSec());
    let mut tracker = NewTracker(1, -1);
    assert!(tracker.InitMemArbitrator(
        Some(core.clone()),
        None,
        17,
        ArbitrationPriorityMedium,
        false,
        0,
        false,
    ));
    assert!(core.ReservedBufferForTest() >= 100);
}
