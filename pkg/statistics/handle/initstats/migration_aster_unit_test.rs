// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 统计初始化（initstats）并发度与区间 worker 的迁移单元测试。
//
// 对齐 Go 侧 `GetConcurrency` 边界、全局配置读取、`RangeWorker` 进度累加与
// 关闭 channel 后再投递任务的失败语义。`InitStatsPercentage` 为启动加载进度百分比。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use anyhow::anyhow;
use astersql_statistics_handle_initstats::config::{
    get_global_config, restore_func, store_global_config,
};
use astersql_statistics_handle_initstats::{
    GetConcurrency, InitStatsPercentage, NewRangeWorker, Task, get_concurrency_for,
};
use serial_test::serial;

/// 校验强制初始化与后台模式下并发度相对 CPU 核数的上下界与 Go 一致。
#[test]
fn concurrency_bounds_match_go_for_force_and_background_modes() {
    let cases = [
        (true, 1, 2),
        (true, 4, 2),
        (true, 8, 6),
        (true, 64, 16),
        (false, 1, 2),
        (false, 4, 2),
        (false, 10, 5),
        (false, 64, 16),
    ];
    for (force_init_stats, processors, expected) in cases {
        assert_eq!(
            get_concurrency_for(force_init_stats, processors),
            expected,
            "force_init_stats={force_init_stats}, processors={processors}",
        );
    }
}

/// 串行测试：切换全局 `force_init_stats` 后，`GetConcurrency` 应读到迁移后的配置。
#[test]
#[serial]
fn get_concurrency_reads_the_migrated_global_config() {
    let restore = restore_func();
    let processors = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);

    let mut config = get_global_config().as_ref().clone();
    config.performance.force_init_stats = true;
    store_global_config(config.clone());
    assert_eq!(GetConcurrency(), get_concurrency_for(true, processors));

    config.performance.force_init_stats = false;
    store_global_config(config);
    assert_eq!(GetConcurrency(), get_concurrency_for(false, processors));
    restore();
}

/// 区间 worker 应处理全部任务；单任务失败仍计入完成数并按权重推进进度百分比。
#[test]
#[serial]
fn range_worker_processes_every_range_and_updates_progress_after_errors() {
    InitStatsPercentage.Store(0.25);
    let processed = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&processed);
    // total_task=3、权重 0.6：每完成一任务进度 +0.2，最终应为 0.25+0.6=0.85。
    let worker = NewRangeWorker(
        "buckets".to_owned(),
        move |task| {
            captured.lock().unwrap().push(task);
            if task.StartTid == 11 {
                Err(anyhow!("injected load failure"))
            } else {
                Ok(())
            }
        },
        1,
        3,
        0.6,
    );

    worker.LoadStats();
    worker.SendTask(Task {
        StartTid: 1,
        EndTid: 10,
    });
    worker.SendTask(Task {
        StartTid: 11,
        EndTid: 20,
    });
    worker.SendTask(Task {
        StartTid: 21,
        EndTid: 30,
    });
    worker.Wait();

    let mut tasks = processed.lock().unwrap().clone();
    tasks.sort_by_key(|task| task.StartTid);
    assert_eq!(
        tasks,
        vec![
            Task {
                StartTid: 1,
                EndTid: 10
            },
            Task {
                StartTid: 11,
                EndTid: 20
            },
            Task {
                StartTid: 21,
                EndTid: 30
            },
        ]
    );
    assert_eq!(worker.completed_task_count(), 3);
    assert!((InitStatsPercentage.Load() - 0.85).abs() < f64::EPSILON);
}

/// 用屏障卡住全部消费者，确认同时活跃数等于请求的并发度。
#[test]
#[serial]
fn range_worker_runs_the_requested_number_of_consumers() {
    InitStatsPercentage.Store(0.0);
    let barrier = Arc::new(Barrier::new(5));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let worker_barrier = Arc::clone(&barrier);
    let worker_active = Arc::clone(&active);
    let worker_maximum = Arc::clone(&maximum);
    let worker = NewRangeWorker(
        "topn".to_owned(),
        move |_| {
            let now = worker_active.fetch_add(1, Ordering::SeqCst) + 1;
            worker_maximum.fetch_max(now, Ordering::SeqCst);
            worker_barrier.wait();
            worker_active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        },
        4,
        4,
        1.0,
    );

    worker.LoadStats();
    for table_id in 1..=4 {
        worker.SendTask(Task {
            StartTid: table_id,
            EndTid: table_id,
        });
    }
    barrier.wait();
    worker.Wait();

    assert_eq!(maximum.load(Ordering::SeqCst), 4);
    assert_eq!(worker.completed_task_count(), 4);
}

/// Wait 关闭任务通道后再次 SendTask，应与 Go 关闭 channel 的 panic 行为一致。
#[test]
#[should_panic(expected = "task channel is closed")]
fn sending_after_wait_matches_go_closed_channel_failure() {
    let worker = NewRangeWorker("histograms".to_owned(), |_| Ok(()), 1, 0, 1.0);
    worker.LoadStats();
    worker.Wait();
    worker.SendTask(Task {
        StartTid: 1,
        EndTid: 1,
    });
}
