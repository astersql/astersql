// Copyright 2026 AsterSQL.

use std::time::Duration;

use crate::pool_task_details::PoolTaskDetails;

#[test]
fn go_merge_48_pool_task_details_merge_samples_and_minima() {
    let mut first = kvproto::kvrpcpb::PoolTaskDetails::new();
    first.set_poll_count(4);
    first.set_dispatch_count(2);
    first.set_total_wall_nanos(100);
    first.set_total_queue_wait_nanos(40);
    first.set_max_queue_wait_nanos(30);
    first.set_min_queue_wait_nanos(10);
    first.set_fair_queue_enabled(true);
    first.set_total_fair_queue_waited_task_slices(7);
    first.set_max_fair_queue_waited_task_slices(5);
    first.set_min_fair_queue_waited_task_slices(2);
    let mut second = kvproto::kvrpcpb::PoolTaskDetails::new();
    second.set_poll_count(2);
    second.set_dispatch_count(3);
    second.set_total_wall_nanos(50);
    second.set_total_queue_wait_nanos(20);
    second.set_max_queue_wait_nanos(15);
    second.set_min_queue_wait_nanos(5);
    second.set_fair_queue_enabled(true);
    second.set_total_fair_queue_waited_task_slices(4);
    second.set_max_fair_queue_waited_task_slices(3);
    second.set_min_fair_queue_waited_task_slices(1);
    let mut details = PoolTaskDetails::default();
    details.merge_from_pb(&first);
    details.merge_from_pb(&second);
    assert_eq!(details.task_count, 2);
    assert_eq!(
        (
            details.poll_count,
            details.max_poll_count,
            details.min_poll_count
        ),
        (6, 4, 2)
    );
    assert_eq!(
        (
            details.dispatch_count,
            details.max_dispatch_count,
            details.min_dispatch_count
        ),
        (5, 3, 2)
    );
    assert_eq!(details.total_wall_time, Duration::from_nanos(150));
    assert_eq!(details.min_task_wall_time, Duration::from_nanos(50));
    assert_eq!(details.total_queue_wait_time, Duration::from_nanos(60));
    assert_eq!(details.min_queue_wait_time, Duration::from_nanos(5));
    assert_eq!(details.fair_queue_sample_count, 5);
    assert_eq!(details.total_fair_queue_waited_task_slices, 11);
    assert_eq!(details.min_fair_queue_waited_task_slices, 1);
}
