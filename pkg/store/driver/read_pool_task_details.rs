// Copyright 2026 AsterSQL.

use astersql_kv::kv::PoolTaskDetails;

/// Preserve every client aggregate field at the canonical KV boundary.
pub fn read_pool_task_details(details: &tikv_client::PoolTaskDetails) -> Option<PoolTaskDetails> {
    if details.task_count == 0 {
        return None;
    }
    Some(PoolTaskDetails {
        TaskCount: details.task_count,
        PollCount: details.poll_count,
        MaxPollCount: details.max_poll_count,
        MinPollCount: details.min_poll_count,
        DispatchCount: details.dispatch_count,
        MaxDispatchCount: details.max_dispatch_count,
        MinDispatchCount: details.min_dispatch_count,
        TotalWallTime: details.total_wall_time,
        TaskWallTimeSampleCount: details.task_wall_time_sample_count,
        MaxTaskWallTime: details.max_task_wall_time,
        MinTaskWallTime: details.min_task_wall_time,
        TotalQueueWaitTime: details.total_queue_wait_time,
        MaxQueueWaitTime: details.max_queue_wait_time,
        MinQueueWaitTime: details.min_queue_wait_time,
        TotalWakeWaitTime: details.total_wake_wait_time,
        MaxWakeWaitTime: details.max_wake_wait_time,
        MinWakeWaitTime: details.min_wake_wait_time,
        FairQueueSampleCount: details.fair_queue_sample_count,
        TotalFairQueueWaitedTaskSlices: details.total_fair_queue_waited_task_slices,
        MaxFairQueueWaitedTaskSlices: details.max_fair_queue_waited_task_slices,
        MinFairQueueWaitedTaskSlices: details.min_fair_queue_waited_task_slices,
        PollCPUTime: details.poll_cpu_time,
        MaxPollCPUTime: details.max_poll_cpu_time,
        MinPollCPUTime: details.min_poll_cpu_time,
        PollWallTime: details.poll_wall_time,
        MaxPollWallTime: details.max_poll_wall_time,
        MinPollWallTime: details.min_poll_wall_time,
    })
}

/// Preserve every client aggregate field at the canonical KV boundary.
pub fn cop_read_pool_task_details(
    details: &astersql_store_copr::pool_task_details::PoolTaskDetails,
) -> Option<PoolTaskDetails> {
    if details.task_count == 0 {
        return None;
    }
    Some(PoolTaskDetails {
        TaskCount: details.task_count,
        PollCount: details.poll_count,
        MaxPollCount: details.max_poll_count,
        MinPollCount: details.min_poll_count,
        DispatchCount: details.dispatch_count,
        MaxDispatchCount: details.max_dispatch_count,
        MinDispatchCount: details.min_dispatch_count,
        TotalWallTime: details.total_wall_time,
        TaskWallTimeSampleCount: details.task_wall_time_sample_count,
        MaxTaskWallTime: details.max_task_wall_time,
        MinTaskWallTime: details.min_task_wall_time,
        TotalQueueWaitTime: details.total_queue_wait_time,
        MaxQueueWaitTime: details.max_queue_wait_time,
        MinQueueWaitTime: details.min_queue_wait_time,
        TotalWakeWaitTime: details.total_wake_wait_time,
        MaxWakeWaitTime: details.max_wake_wait_time,
        MinWakeWaitTime: details.min_wake_wait_time,
        FairQueueSampleCount: details.fair_queue_sample_count,
        TotalFairQueueWaitedTaskSlices: details.total_fair_queue_waited_task_slices,
        MaxFairQueueWaitedTaskSlices: details.max_fair_queue_waited_task_slices,
        MinFairQueueWaitedTaskSlices: details.min_fair_queue_waited_task_slices,
        PollCPUTime: details.poll_cpu_time,
        MaxPollCPUTime: details.max_poll_cpu_time,
        MinPollCPUTime: details.min_poll_cpu_time,
        PollWallTime: details.poll_wall_time,
        MaxPollWallTime: details.max_poll_wall_time,
        MinPollWallTime: details.min_poll_wall_time,
    })
}
