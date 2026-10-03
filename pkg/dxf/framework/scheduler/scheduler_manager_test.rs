// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 验证调度管理器对终态任务执行类型专属清理，并将清理后的任务迁入历史记录。
// 测试通过可观察的清理例程同时记录调用次数和改写任务元数据，覆盖清理工厂的派发契约。

use crate::test_support::{TestTaskManager, task};
use crate::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 记录调用次数，并模拟清理过程对任务元数据的合法改写。
struct CountingCleanup(Arc<AtomicUsize>);

impl CleanUpRoutine for CountingCleanup {
    fn clean_up(&self, task: &mut Task) -> Result<()> {
        self.0.fetch_add(1, Ordering::AcqRel);
        task.meta.extend_from_slice(b"-clean");
        Ok(())
    }
}

#[test]
fn test_clean_up_routine() {
    let task_manager = Arc::new(TestTaskManager::default());
    let mut finished = task(1, TASK_STATE_SUCCEED);
    finished.base.task_type = "cleanup-test".to_owned();
    finished.meta = b"meta".to_vec();
    task_manager.insert_task(finished);

    let calls = Arc::new(AtomicUsize::new(0));
    // 工厂按任务类型派发；每次构造的新例程共享计数器，以便验证实际调用次数。
    RegisterSchedulerCleanUpFactory(
        "cleanup-test",
        Arc::new({
            let calls = Arc::clone(&calls);
            move || Arc::new(CountingCleanup(Arc::clone(&calls)))
        }),
    );
    let manager = Manager::new(task_manager.clone(), "server", None);
    assert_eq!(manager.cleanup_finished_tasks().unwrap(), 1);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    let transferred = task_manager.transferred_tasks.lock().unwrap();
    // 迁入历史记录的必须是清理后的任务，而不是清理前的快照。
    assert_eq!(transferred.len(), 1);
    assert_eq!(transferred[0].meta, b"meta-clean");
}

#[test]
fn cleanup_starts_immediately_and_drains_all_bounded_batches() {
    let restore = crate::proto::SetTaskCleanupBatchSizeForTest(2);
    let task_manager = Arc::new(TestTaskManager::default());
    for id in 1..=5 {
        task_manager.insert_task(task(id, TASK_STATE_SUCCEED));
    }
    let manager = Manager::new(task_manager.clone(), "server", None);
    manager.start().unwrap();
    restore();
    assert_eq!(task_manager.transferred_tasks.lock().unwrap().len(), 5);
}

struct PartialCleanup(Arc<AtomicUsize>);
impl CleanUpRoutine for PartialCleanup {
    fn clean_up(&self, task: &mut Task) -> Result<()> {
        self.0.fetch_add(1, Ordering::AcqRel);
        if task.base.id == 2 {
            Err(SchedulerError::new("cleanup failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn cleanup_drain_stops_on_empty_query_failure_transfer_failure_and_partial_progress() {
    for failure in ["empty", "query", "transfer", "partial", "zero"] {
        let tasks = Arc::new(TestTaskManager::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let kind = format!("drain-stop-{failure}");
        if failure == "partial" || failure == "zero" {
            let calls = calls.clone();
            RegisterSchedulerCleanUpFactory(
                &kind,
                Arc::new(move || Arc::new(PartialCleanup(calls.clone()))),
            );
        }
        if failure != "empty" && failure != "query" {
            for id in if failure == "zero" {
                vec![2]
            } else {
                vec![1, 2]
            } {
                let mut task = task(id, TASK_STATE_SUCCEED);
                task.base.task_type = kind.clone();
                tasks.insert_task(task);
            }
        }
        if failure == "query" {
            *tasks.cleanup_error.lock().unwrap() = Some(SchedulerError::new("query failed"));
        }
        if failure == "transfer" {
            *tasks.transfer_error.lock().unwrap() = Some(SchedulerError::new("transfer failed"));
        }
        let manager = Manager::new(tasks.clone(), "server", None);
        manager.drain_cleanup_task_batches();
        assert_eq!(tasks.cleanup_reads.load(Ordering::Acquire), 1, "{failure}");
        assert_eq!(
            tasks.transferred_tasks.lock().unwrap().len(),
            usize::from(failure == "partial"),
            "{failure}"
        );
        if failure == "partial" || failure == "zero" {
            assert!(tasks.task_by_id(2).is_ok());
        }
    }
}

#[test]
fn cleanup_batch_reports_full_progress_and_tick_drains_new_terminal_tasks() {
    let tasks = Arc::new(TestTaskManager::default());
    tasks.insert_task(task(1, TASK_STATE_SUCCEED));
    let manager = Manager::new(tasks.clone(), "server", None);
    assert!(manager.process_cleanup_task_batch());
    assert!(!manager.process_cleanup_task_batch());
    manager.start().unwrap();
    tasks.insert_task(task(2, TASK_STATE_REVERTED));
    manager.tick().unwrap();
    assert_eq!(tasks.transferred_tasks.lock().unwrap().len(), 2);
}
