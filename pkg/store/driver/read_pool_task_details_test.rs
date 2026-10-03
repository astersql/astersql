// Copyright 2026 AsterSQL.

use super::CopResultSubset;
use astersql_kv::ResultSubset;
use astersql_store_copr as copr;
use std::time::Duration;

#[test]
fn cop_response_keeps_read_pool_diagnostics_at_the_kv_boundary() {
    let details = sample();
    let response = copr::CopResponse {
        response: Some(copr::CopProtocolResponse {
            data: vec![10, 20, 30],
            read_pool_task_details: Some(details),
            ..Default::default()
        }),
        ..Default::default()
    };
    let subset = CopResultSubset::from(response);
    assert_eq!(subset.GetData(), &[10, 20, 30]);
    let pool = subset
        .ReadPoolTaskDetails()
        .expect("completed response retains diagnostics");
    assert_eq!(
        pool.String(),
        "{tasks:1, poll_count:{total:4, avg:4, max:4, min:4}, dispatch_count:{total:2, max:2, min:2}, task_wall_time:{total:20ms, avg:20ms, max:20ms, min:20ms}, queue_wait:{total:6ms, avg:3ms, max:4ms, min:2ms}, wake_wait:{total:4ms, avg:4ms, max:4ms, min:4ms}, fair_queue:{enabled:true, waited_task_slices:{total:6, avg:3, max:4, min:2}}, poll_cpu:{total:8ms, avg:2ms, max:3ms, min:1ms}, poll_wall:{total:12ms, avg:3ms, max:5ms, min:2ms}}"
    );
    assert!(
        CopResultSubset::from(copr::CopResponse::default())
            .ReadPoolTaskDetails()
            .is_none()
    );
}

pub(crate) fn sample() -> copr::pool_task_details::PoolTaskDetails {
    copr::pool_task_details::PoolTaskDetails {
        task_count: 1,
        poll_count: 4,
        max_poll_count: 4,
        min_poll_count: 4,
        dispatch_count: 2,
        max_dispatch_count: 2,
        min_dispatch_count: 2,
        total_wall_time: Duration::from_millis(20),
        task_wall_time_sample_count: 1,
        max_task_wall_time: Duration::from_millis(20),
        min_task_wall_time: Duration::from_millis(20),
        total_queue_wait_time: Duration::from_millis(6),
        max_queue_wait_time: Duration::from_millis(4),
        min_queue_wait_time: Duration::from_millis(2),
        total_wake_wait_time: Duration::from_millis(4),
        max_wake_wait_time: Duration::from_millis(4),
        min_wake_wait_time: Duration::from_millis(4),
        fair_queue_sample_count: 2,
        total_fair_queue_waited_task_slices: 6,
        max_fair_queue_waited_task_slices: 4,
        min_fair_queue_waited_task_slices: 2,
        poll_cpu_time: Duration::from_millis(8),
        max_poll_cpu_time: Duration::from_millis(3),
        min_poll_cpu_time: Duration::from_millis(1),
        poll_wall_time: Duration::from_millis(12),
        max_poll_wall_time: Duration::from_millis(5),
        min_poll_wall_time: Duration::from_millis(2),
    }
}
