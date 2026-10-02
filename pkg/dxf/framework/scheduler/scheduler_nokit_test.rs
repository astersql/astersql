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

// 基础调度器的轻量级单元测试。
//
// 通过内存任务管理器和可配置扩展直接驱动单轮调度，覆盖初始化校验、阶段推进、
// 节点选择、失败暂停、任务刷新与参数修改等关键状态转换。

use crate::test_support::{TestExtension, TestTaskManager, scheduler, task};
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

// 初始化时必须拒绝无法作为有效运行时标识的任务 keyspace。
#[test]
fn test_base_scheduler_init_checks_task_runtime() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    let mut invalid = task(1, TASK_STATE_PENDING);
    invalid.base.keyspace = "key\0space".to_owned();
    let scheduler = scheduler(invalid, manager, extension, true);
    assert_eq!(
        scheduler.init().unwrap_err(),
        SchedulerError::new("invalid task keyspace")
    );
}

// 待执行任务进入下一阶段时，应持久化扩展生成的子任务并更新阶段状态。
#[test]
fn test_scheduler_on_next_stage() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    *extension.metas.lock().unwrap() = vec![b"a".to_vec(), b"b".to_vec()];
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager.clone(),
        extension,
        true,
    );

    assert!(!scheduler.schedule_once().unwrap());
    let current = scheduler.task();
    assert_eq!(current.base.state, TASK_STATE_RUNNING);
    assert_eq!(current.base.step, 1);
    let subtasks = manager.persisted_subtasks.lock().unwrap();
    assert_eq!(subtasks.len(), 2);
    assert_eq!(subtasks[0].base.exec_id, "n1");
    assert_eq!(subtasks[1].base.ordinal, 2);
}

// 与 Go 的 Switch2NextStep 表驱动场景一致：完成回调失败不得提前修改任务，
// 候选节点为空和可重试规划错误也必须原样返回。
#[test]
fn test_scheduler_on_next_stage_error_paths() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(STEP_DONE, Ordering::Release);
    *extension.done_error.lock().unwrap() = Some(SchedulerError::new("done err"));
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager.clone(),
        extension.clone(),
        true,
    );

    assert_eq!(
        scheduler.schedule_once().unwrap_err(),
        SchedulerError::new("done err")
    );
    assert_eq!(scheduler.task().base.state, TASK_STATE_PENDING);
    assert_eq!(scheduler.task().base.step, STEP_INIT);

    let empty_manager = Arc::new(TestTaskManager::default());
    let empty_extension = Arc::new(TestExtension::default());
    empty_extension.next_step.store(1, Ordering::Release);
    let mut scoped = task(2, TASK_STATE_PENDING);
    scoped.base.target_scope = "background".to_owned();
    let empty_scheduler =
        crate::test_support::scheduler(scoped, empty_manager, empty_extension, true);
    assert_eq!(
        empty_scheduler.schedule_once().unwrap_err(),
        SchedulerError::new("no available TiDB node to dispatch subtasks")
    );
    assert_eq!(empty_scheduler.task().base.state, TASK_STATE_PENDING);

    extension.next_step.store(1, Ordering::Release);
    extension.retryable.store(true, Ordering::Release);
    *extension.plan_error.lock().unwrap() = Some(SchedulerError::new("plan err"));
    assert_eq!(
        scheduler.schedule_once().unwrap_err(),
        SchedulerError::new("plan err")
    );
    assert_eq!(scheduler.task().base.state, TASK_STATE_PENDING);

    extension.retryable.store(false, Ordering::Release);
    *extension.plan_error.lock().unwrap() = Some(SchedulerError::new("fatal plan err"));
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
    assert_eq!(
        scheduler.task().error,
        Some(SchedulerError::new("fatal plan err"))
    );
}

// 扩展筛选出的候选节点必须用于实际的子任务分派。
#[test]
fn test_get_eligible_nodes() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    *extension.eligible.lock().unwrap() = vec!["chosen".to_owned()];
    *extension.metas.lock().unwrap() = vec![b"meta".to_vec()];
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager.clone(),
        extension,
        true,
    );

    scheduler.schedule_once().unwrap();
    assert_eq!(
        manager.persisted_subtasks.lock().unwrap()[0].base.exec_id,
        "chosen"
    );
}

// 当前阶段全部成功且没有后续阶段时，应结束任务并只调用一次完成回调。
#[test]
fn test_scheduler_is_step_succeed() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(STEP_DONE, Ordering::Release);
    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 1;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_SUCCEED, 2)]));
    let scheduler = scheduler(running, manager, extension.clone(), true);

    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_SUCCEED);
    assert_eq!(extension.done_calls.load(Ordering::Acquire), 1);
}

// 开启磁盘满自动暂停后，TiKV 磁盘满错误应转入暂停流程并保留原始原因。
#[test]
fn test_scheduler_auto_pause_on_kv_disk_full() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 1;
    running.base.extra_params.pause_on_kv_disk_full = true;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_FAILED, 1)]));
    manager
        .task_errors
        .lock()
        .unwrap()
        .insert(1, vec![SchedulerError::new("TiKV disk full")]);
    let scheduler = scheduler(running, manager, extension, true);

    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSING);
    assert_eq!(
        scheduler.task().error,
        Some(SchedulerError::new("TiKV disk full"))
    );
}

// Pausing / Resuming / Reverting 的等待与落库分支必须保持 Go 状态机语义。
#[test]
fn test_scheduler_pause_resume_and_revert_transitions() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    let mut pausing = task(1, TASK_STATE_PAUSING);
    pausing.base.step = 1;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_RUNNING, 1)]));
    let scheduler = scheduler(pausing, manager.clone(), extension.clone(), true);

    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSING);

    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::new());
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSED);

    let mut resuming = manager.task(1);
    resuming.base.state = TASK_STATE_RESUMING;
    manager.insert_task(resuming);
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_RUNNING);

    let mut reverting = manager.task(1);
    reverting.base.state = TASK_STATE_REVERTING;
    manager.insert_task(reverting);
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_PENDING, 1)]));
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(extension.tick_calls.load(Ordering::Acquire), 1);
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);

    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::new());
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTED);
    assert_eq!(extension.done_calls.load(Ordering::Acquire), 1);
}

// 非磁盘满失败按 Go 契约回滚；启用人工恢复时则进入 AwaitingResolution。
#[test]
fn test_scheduler_failure_revert_and_manual_recovery() {
    for (manual_recovery, expected_state) in [
        (false, TASK_STATE_REVERTING),
        (true, TASK_STATE_AWAITING_RESOLUTION),
    ] {
        let manager = Arc::new(TestTaskManager::default());
        let extension = Arc::new(TestExtension::default());
        let mut running = task(1, TASK_STATE_RUNNING);
        running.base.step = 1;
        running.base.extra_params.manual_recovery = manual_recovery;
        manager
            .state_counts
            .lock()
            .unwrap()
            .insert((1, 1), HashMap::from([(SUBTASK_STATE_FAILED, 1)]));
        manager
            .task_errors
            .lock()
            .unwrap()
            .insert(1, vec![SchedulerError::new("worker failed")]);
        let scheduler = scheduler(running, manager, extension, true);

        assert!(!scheduler.schedule_once().unwrap());
        assert_eq!(scheduler.task().base.state, expected_state);
        assert_eq!(
            scheduler.task().error,
            Some(SchedulerError::new("worker failed"))
        );
    }
}

// Prepare 错误分类、元数据传播和存储层是否推进步骤均与 Go 测试一致。
#[test]
fn test_scheduler_prepare_required_paths() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    let mut pending = task(1, TASK_STATE_PENDING);
    pending.base.extra_params.prepare_mode = PREPARE_MODE_REQUIRED;
    pending.meta = b"meta".to_vec();
    let scheduler = scheduler(pending, manager.clone(), extension.clone(), true);

    extension.retryable.store(true, Ordering::Release);
    *extension.prepare_error.lock().unwrap() = Some(SchedulerError::new("prepare err"));
    assert_eq!(
        scheduler.schedule_once().unwrap_err(),
        SchedulerError::new("prepare err")
    );
    assert_eq!(scheduler.task().base.state, TASK_STATE_PENDING);

    manager.switch_after_prepare.store(false, Ordering::Release);
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.step, STEP_INIT);
    assert_eq!(scheduler.task().meta, b"meta");

    manager.switch_after_prepare.store(true, Ordering::Release);
    assert!(!scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_RUNNING);
    assert_eq!(scheduler.task().base.step, 1);
    assert_eq!(scheduler.task().meta, b"meta-prepared");
    assert_eq!(extension.prepare_calls.load(Ordering::Acquire), 3);
}

// 尚未分配执行槽位时，本轮调度不得提前改变待执行状态。
#[test]
fn test_scheduler_not_allocate_slots() {
    let manager = Arc::new(TestTaskManager::default());
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager,
        Arc::new(TestExtension::default()),
        false,
    );
    assert!(scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_PENDING);
}

// 每轮决策前需刷新持久层中的任务，避免使用调度器持有的过期状态。
#[test]
fn test_scheduler_refresh_task() {
    let manager = Arc::new(TestTaskManager::default());
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager.clone(),
        Arc::new(TestExtension::default()),
        true,
    );
    let mut latest = manager.task(1);
    latest.base.state = TASK_STATE_PAUSED;
    latest.base.step = 9;
    manager.insert_task(latest);

    assert!(scheduler.schedule_once().unwrap());
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSED);
    assert_eq!(scheduler.task().base.step, 9);
}

// 修改流程同时处理内建并发参数和扩展自定义参数，完成后恢复原状态并清空修改项。
#[test]
fn test_scheduler_maintain_task_fields() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    let mut modifying = task(1, TASK_STATE_MODIFYING);
    modifying.previous_state = TASK_STATE_RUNNING;
    modifying.base.required_slots = 4;
    modifying.modifications = vec![
        Modification {
            kind: "modify_concurrency".to_owned(),
            to: 8,
        },
        Modification {
            kind: "modify_max_node_count".to_owned(),
            to: 3,
        },
        Modification {
            kind: "custom".to_owned(),
            to: 7,
        },
    ];
    let scheduler = scheduler(modifying, manager, extension, true);

    assert!(scheduler.schedule_once().unwrap());
    let current = scheduler.task();
    assert_eq!(current.base.state, TASK_STATE_RUNNING);
    assert_eq!(current.base.required_slots, 8);
    assert_eq!(current.base.max_node_count, 3);
    assert_eq!(current.meta, b"{}:custom=7");
    assert!(current.modifications.is_empty());
}

// 取消判定应识别被包装的取消标记，同时不能误判普通失败。
#[test]
fn test_on_task_finished() {
    assert!(IsCancelledErr(&SchedulerError::new(format!(
        "wrapped: {TASK_CANCEL_MESSAGE}"
    ))));
    assert!(!IsCancelledErr(&SchedulerError::new("ordinary failure")));
}

#[test]
fn finished_task_metric_classifies_errors() {
    use astersql_dxf_framework_dxfmetric::InitDistTaskMetrics;

    let counter = &InitDistTaskMetrics().FinishedTaskCounter;
    let value = |label| counter.with_label_values(&[label]).get();
    let before = ["all", "succeed", "failed", "cancelled", "data-error"].map(value);

    let cases = [
        (TASK_STATE_SUCCEED, None, "succeed"),
        (TASK_STATE_FAILED, Some("ordinary failure"), "failed"),
        (TASK_STATE_REVERTED, None, "failed"),
        (TASK_STATE_REVERTED, Some("ordinary failure"), "failed"),
        (
            TASK_STATE_REVERTED,
            Some("wrapped: cancelled by user"),
            "cancelled",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrEncodeKV Value conversion failed for column 'a'"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrEncodeKV Check constraint 'c' is violated"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrEncodeKV Table has no partition for value 1"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("[executor:8167]Duplicate key conflict found"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrFoundDataConflictRecords found data conflict records"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrFoundIndexConflictRecords found index conflict records"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("[kv:1062]Duplicate entry '1'"),
            "data-error",
        ),
        (
            TASK_STATE_REVERTED,
            Some("ErrEncodeKV column count mismatch"),
            "failed",
        ),
        (
            TASK_STATE_REVERTED,
            Some("Value conversion failed for column 'a'"),
            "failed",
        ),
        (TASK_STATE_RUNNING, None, ""),
    ];
    let mut expected = [0_u64; 5];
    for (state, message, label) in cases {
        let error = message.map(SchedulerError::new);
        super::scheduler::on_task_finished(state, error.as_ref());
        if !label.is_empty() {
            expected[0] += 1;
            let index = match label {
                "succeed" => 1,
                "failed" => 2,
                "cancelled" => 3,
                "data-error" => 4,
                _ => unreachable!(),
            };
            expected[index] += 1;
        }
    }
    for (index, label) in ["all", "succeed", "failed", "cancelled", "data-error"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            value(label) - before[index],
            expected[index] as f64,
            "{label}"
        );
    }
}

#[test]
fn terminal_transitions_update_metric() {
    use astersql_dxf_framework_dxfmetric::InitDistTaskMetrics;

    let counter = &InitDistTaskMetrics().FinishedTaskCounter;
    let all_before = counter.with_label_values(&["all"]).get();
    let success_before = counter.with_label_values(&["succeed"]).get();
    let data_before = counter.with_label_values(&["data-error"]).get();

    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(STEP_DONE, Ordering::Release);
    let mut running = task(801, TASK_STATE_RUNNING);
    running.base.step = 1;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((801, 1), HashMap::new());
    let success_scheduler = scheduler(running, manager, extension, true);
    success_scheduler.schedule_once().unwrap();
    assert_eq!(success_scheduler.task().base.state, TASK_STATE_SUCCEED);

    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    let mut reverting = task(802, TASK_STATE_REVERTING);
    reverting.error = Some(SchedulerError::new("[kv:1062]Duplicate entry '1'"));
    let reverting_scheduler = scheduler(reverting, manager, extension, false);
    reverting_scheduler.schedule_once().unwrap();
    assert_eq!(reverting_scheduler.task().base.state, TASK_STATE_REVERTED);

    assert_eq!(counter.with_label_values(&["all"]).get() - all_before, 2.0);
    assert_eq!(
        counter.with_label_values(&["succeed"]).get() - success_before,
        1.0
    );
    assert_eq!(
        counter.with_label_values(&["data-error"]).get() - data_before,
        1.0
    );
}
