// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 自动 ANALYZE 刷新器的隔离单元测试。
//
// 使用内存队列模拟初始化扫描与 DML 增量，并记录实际执行的表 ID，重点验证
// 时间窗口、失败重试、优先级与动态并发度，以及队列和 Worker 的生命周期边界。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 记录已执行表 ID 的测试执行器，用于避免依赖真实 ANALYZE 环境。
struct RecordingExecutor {
    calls: Mutex<Vec<i64>>,
}

impl crate::AnalysisExecutor for RecordingExecutor {
    fn analyze(&self, job: &crate::AnalysisJob) -> Result<(), String> {
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push(job.table_id);
        Ok(())
    }
}

/// 可一次性取走初始化作业和 DML 作业的测试数据源。
///
/// 原子字段允许测试在共享引用下调整期望并发度，并注入一次初始化失败。
struct QueueSource {
    initial: Mutex<Vec<crate::AnalysisJob>>,
    dml: Mutex<Vec<crate::AnalysisJob>>,
    desired_concurrency: AtomicUsize,
    initialize_calls: AtomicUsize,
    fail_initialize_once: AtomicBool,
}

impl QueueSource {
    fn new(initial: Vec<crate::AnalysisJob>, dml: Vec<crate::AnalysisJob>) -> Self {
        Self {
            initial: Mutex::new(initial),
            dml: Mutex::new(dml),
            desired_concurrency: AtomicUsize::new(1),
            initialize_calls: AtomicUsize::new(0),
            fail_initialize_once: AtomicBool::new(false),
        }
    }
}

impl crate::JobSource for QueueSource {
    fn initialize(&self) -> Result<Vec<crate::AnalysisJob>, String> {
        self.initialize_calls.fetch_add(1, Ordering::AcqRel);
        if self
            .fail_initialize_once
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Err("initialization failed".into());
        }
        Ok(std::mem::take(
            &mut *self.initial.lock().expect("initial mutex poisoned"),
        ))
    }

    fn process_dml_changes(&self) -> Result<Vec<crate::AnalysisJob>, String> {
        Ok(std::mem::take(
            &mut *self.dml.lock().expect("dml mutex poisoned"),
        ))
    }

    fn desired_concurrency(&self) -> usize {
        self.desired_concurrency.load(Ordering::Acquire)
    }
}

fn job(table_id: i64, priority: i64) -> crate::AnalysisJob {
    crate::AnalysisJob {
        table_id,
        priority,
        must_retry: false,
    }
}

fn refresher(
    source: Arc<QueueSource>,
    executor: Arc<RecordingExecutor>,
    concurrency: usize,
) -> crate::Refresher {
    crate::Refresher::new(crate::Worker::new(executor, concurrency), source)
}

#[test]
/// 即使当前不在执行窗口内，也应先初始化队列，但不得提交作业。
fn test_queue_initializes_outside_time_window() {
    let source = Arc::new(QueueSource::new(vec![job(1, 10)], Vec::new()));
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source.clone(), executor.clone(), 1);
    refresher.set_auto_analysis_time_window(100, 200).unwrap();

    assert!(!refresher.analyze_highest_priority_tables(50).unwrap());
    assert!(refresher.is_queue_initialized());
    assert_eq!(refresher.len(), 1);
    assert!(executor.calls.lock().unwrap().is_empty());
    assert_eq!(source.initialize_calls.load(Ordering::Acquire), 1);
    refresher.close();
}

#[test]
/// 与 Go 的 ProcessDMLChangesForTest 一致：队列初始化前不得消费 DML 增量。
fn test_process_dml_changes_before_initialization_is_ignored() {
    let source = Arc::new(QueueSource::new(Vec::new(), vec![job(9, 4)]));
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source, executor.clone(), 1);

    refresher.process_dml_changes().unwrap();
    assert_eq!(refresher.len(), 0);
    assert!(refresher.analyze_highest_priority_tables(0).unwrap());
    refresher.wait_finished();
    assert_eq!(executor.calls.lock().unwrap().as_slice(), &[9]);
    refresher.close();
}

#[test]
/// 初始化失败不能将队列标记为就绪，下一轮调用应重新初始化并执行作业。
fn test_initialization_error_can_be_retried() {
    let source = Arc::new(QueueSource::new(vec![job(1, 1)], Vec::new()));
    source.fail_initialize_once.store(true, Ordering::Release);
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source.clone(), executor.clone(), 1);

    assert_eq!(
        refresher.analyze_highest_priority_tables(0).unwrap_err(),
        "initialization failed"
    );
    assert!(!refresher.is_queue_initialized());
    assert!(refresher.analyze_highest_priority_tables(0).unwrap());
    refresher.wait_finished();
    assert_eq!(executor.calls.lock().unwrap().as_slice(), &[1]);
    assert_eq!(source.initialize_calls.load(Ordering::Acquire), 2);
    refresher.close();
}

#[test]
/// 动态并发度应填满可用槽位，并优先提交优先级最高的作业。
fn test_analyze_submits_all_available_concurrency_slots_in_priority_order() {
    let source = Arc::new(QueueSource::new(
        vec![job(1, 1), job(2, 30), job(3, 20)],
        Vec::new(),
    ));
    source.desired_concurrency.store(2, Ordering::Release);
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source, executor.clone(), 1);

    assert!(refresher.analyze_highest_priority_tables(0).unwrap());
    refresher.wait_finished();
    // 并发执行完成顺序不稳定，排序后只校验被选中的两个最高优先级作业。
    let mut calls = executor.calls.lock().unwrap().clone();
    calls.sort_unstable();
    assert_eq!(calls, vec![2, 3]);
    assert_eq!(refresher.len(), 1);
    refresher.close();
}

#[test]
/// 首次初始化完成后，同一轮调度还应吸收 DML 增量产生的作业。
fn test_processes_dml_changes_after_initialization() {
    let source = Arc::new(QueueSource::new(Vec::new(), vec![job(9, 4)]));
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source, executor.clone(), 1);

    assert!(refresher.analyze_highest_priority_tables(0).unwrap());
    refresher.wait_finished();
    assert_eq!(executor.calls.lock().unwrap().as_slice(), &[9]);
    refresher.close();
}

#[test]
/// 跨午夜窗口使用“晚于起点或早于终点”语义，并拒绝超出一天的边界值。
fn test_cross_midnight_time_window_and_invalid_bounds() {
    let source = Arc::new(QueueSource::new(Vec::new(), Vec::new()));
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source, executor, 1);
    refresher
        .set_auto_analysis_time_window(23 * 60, 2 * 60)
        .unwrap();
    assert!(refresher.is_within_time_window(23 * 60 + 30));
    assert!(refresher.is_within_time_window(60));
    assert!(!refresher.is_within_time_window(12 * 60));
    assert!(refresher.set_auto_analysis_time_window(24 * 60, 0).is_err());
    assert!(
        refresher
            .set_auto_analysis_time_window(0, 24 * 60 + 1)
            .is_err()
    );
    refresher.close();
}

#[test]
/// 仅关闭优先队列应重置初始化状态，但不得取消已经交给 Worker 的作业。
fn test_close_priority_queue_resets_queue_but_not_worker() {
    let source = Arc::new(QueueSource::new(vec![job(1, 1)], Vec::new()));
    let executor = Arc::new(RecordingExecutor {
        calls: Mutex::new(Vec::new()),
    });
    let refresher = refresher(source, executor.clone(), 1);
    assert!(refresher.analyze_highest_priority_tables(0).unwrap());
    refresher.close_priority_queue();
    assert!(!refresher.is_queue_initialized());
    assert_eq!(refresher.len(), 0);
    refresher.wait_finished();
    assert_eq!(executor.calls.lock().unwrap().as_slice(), &[1]);
    refresher.close();
}
