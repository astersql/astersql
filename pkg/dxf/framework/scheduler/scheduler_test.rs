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

// 调度器状态机与管理器生命周期的回归测试。
//
// 重点覆盖任务失败信息持久化、规划失败后的重试/回滚选择、子任务在执行节点间的分配，
// 以及暂停、取消和管理器启停时调度器数量的变化。

use crate::test_support::{TestExtension, TestTaskManager, scheduler, task};
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

// 管理器标记任务失败时必须同时保存失败状态和原始错误，供后续诊断读取。
#[test]
fn test_task_fail_in_manager() {
    let manager = TestTaskManager::default();
    manager.insert_task(task(1, TASK_STATE_PENDING));
    manager
        .fail_task(1, TASK_STATE_PENDING, SchedulerError::new("factory failed"))
        .unwrap();
    assert_eq!(manager.task(1).base.state, TASK_STATE_FAILED);
    assert_eq!(
        manager.task(1).error,
        Some(SchedulerError::new("factory failed"))
    );
}

// 没有后续步骤时，单次调度应直接完成任务，并且只调用一次完成回调。
#[test]
fn test_simple() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(STEP_DONE, Ordering::Release);
    let scheduler = scheduler(
        task(1, TASK_STATE_PENDING),
        manager,
        extension.clone(),
        true,
    );
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_SUCCEED);
    assert_eq!(extension.done_calls.load(Ordering::Acquire), 1);
}

// 不可重试的规划错误会写入任务，并驱动任务进入回滚态，而不是向调度循环返回错误。
#[test]
fn test_simple_err_stage() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    *extension.plan_error.lock().unwrap() = Some(SchedulerError::new("plan failed"));
    let scheduler = scheduler(task(1, TASK_STATE_PENDING), manager, extension, true);
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
    assert_eq!(
        scheduler.task().error,
        Some(SchedulerError::new("plan failed"))
    );
}

// 已收到取消请求的任务无需再规划子任务，应携带统一的取消错误进入回滚态。
#[test]
fn test_simple_cancel() {
    let manager = Arc::new(TestTaskManager::default());
    let scheduler = scheduler(
        task(1, TASK_STATE_CANCELLING),
        manager,
        Arc::new(TestExtension::default()),
        true,
    );
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
    assert!(IsCancelledErr(scheduler.task().error.as_ref().unwrap()));
}

// 任一子任务被取消也会使其所属任务进入回滚态，避免继续推进当前步骤。
#[test]
fn test_simple_subtask_cancel() {
    let manager = Arc::new(TestTaskManager::default());
    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 1;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_CANCELED, 1)]));
    manager
        .task_errors
        .lock()
        .unwrap()
        .insert(1, vec![SchedulerError::new("subtask cancelled")]);
    let scheduler = scheduler(running, manager, Arc::new(TestExtension::default()), true);
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
}

// ManualRecovery 开启时，不可重试的 subtask 错误必须停在等待人工处理态；
// 人工将失败 subtask 重置并把 task 恢复为 running 后，调度可继续到成功。
#[test]
fn test_manual_recovery_resumes_awaiting_resolution_task() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(STEP_DONE, Ordering::Release);

    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 1;
    running.base.extra_params.manual_recovery = true;
    manager
        .state_counts
        .lock()
        .unwrap()
        .insert((1, 1), HashMap::from([(SUBTASK_STATE_FAILED, 1)]));
    manager
        .task_errors
        .lock()
        .unwrap()
        .insert(1, vec![SchedulerError::new("non retryable subtask error")]);

    let awaiting = scheduler(running, manager.clone(), extension.clone(), true);
    awaiting.schedule_once().unwrap();
    assert_eq!(awaiting.task().base.state, TASK_STATE_AWAITING_RESOLUTION);
    assert_eq!(
        awaiting.task().error,
        Some(SchedulerError::new("non retryable subtask error"))
    );

    manager.state_counts.lock().unwrap().remove(&(1, 1));
    manager.task_errors.lock().unwrap().remove(&1);
    let mut recovered_task = manager.task(1);
    recovered_task.base.state = TASK_STATE_RUNNING;
    manager.insert_task(recovered_task);
    let recovered = scheduler(manager.task(1), manager, extension, true);
    recovered.schedule_once().unwrap();
    assert_eq!(recovered.task().base.state, TASK_STATE_SUCCEED);
}

// 等待人工处理的 task 被取消后，应沿 cancelling -> reverting -> reverted 收敛。
#[test]
fn test_manual_recovery_task_can_be_cancelled() {
    let manager = Arc::new(TestTaskManager::default());
    let mut cancelling = task(1, TASK_STATE_CANCELLING);
    cancelling.base.step = 1;
    cancelling.base.extra_params.manual_recovery = true;

    let scheduler = scheduler(
        cancelling,
        manager,
        Arc::new(TestExtension::default()),
        true,
    );
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
    assert!(IsCancelledErr(scheduler.task().error.as_ref().unwrap()));
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTED);
}

// 子任务元数据按可用执行节点轮询分配，保证多个节点都能获得工作。
#[test]
fn test_parallel() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    *extension.eligible.lock().unwrap() = vec!["n1".to_owned(), "n2".to_owned()];
    *extension.metas.lock().unwrap() =
        vec![b"1".to_vec(), b"2".to_vec(), b"3".to_vec(), b"4".to_vec()];
    let task = task(1, TASK_STATE_PENDING);
    manager.insert_task(task.clone());
    let node_manager = Arc::new(NodeManager::new());
    node_manager.set_nodes(vec![
        ManagedNode {
            id: "n1".to_owned(),
            role: String::new(),
            cpu_count: 8,
        },
        ManagedNode {
            id: "n2".to_owned(),
            role: String::new(),
            cpu_count: 8,
        },
    ]);
    let slot_manager = Arc::new(SlotManager::new());
    slot_manager.update_capacity(8);
    let scheduler = BaseScheduler::new(
        task,
        Param {
            task_manager: manager.clone(),
            node_manager,
            slot_manager,
            server_id: "test".to_owned(),
            allocated_slots: true,
            node_resource: None,
        },
        extension,
    );
    scheduler.schedule_once().unwrap();
    let exec_ids = manager
        .persisted_subtasks
        .lock()
        .unwrap()
        .iter()
        .map(|subtask| subtask.base.exec_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(exec_ids, vec!["n1", "n2", "n1", "n2"]);
}

// 可重试的规划错误由调度器返回给上层重试，同时保持任务仍处于待调度态。
#[test]
fn test_parallel_err_stage() {
    let manager = Arc::new(TestTaskManager::default());
    let extension = Arc::new(TestExtension::default());
    extension.next_step.store(1, Ordering::Release);
    extension.retryable.store(true, Ordering::Release);
    *extension.plan_error.lock().unwrap() = Some(SchedulerError::new("retry plan"));
    let scheduler = scheduler(task(1, TASK_STATE_PENDING), manager, extension, true);
    assert_eq!(
        scheduler.schedule_once().unwrap_err(),
        SchedulerError::new("retry plan")
    );
    assert_eq!(scheduler.task().base.state, TASK_STATE_PENDING);
}

// 并行任务收到取消请求后同样统一转入回滚态。
#[test]
fn test_parallel_cancel() {
    let manager = Arc::new(TestTaskManager::default());
    let scheduler = scheduler(
        task(1, TASK_STATE_CANCELLING),
        manager,
        Arc::new(TestExtension::default()),
        true,
    );
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
}

// 并行步骤中即使其他子任务已成功，只要存在取消项，主任务仍必须回滚。
#[test]
fn test_parallel_subtask_cancel() {
    let manager = Arc::new(TestTaskManager::default());
    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 2;
    manager.state_counts.lock().unwrap().insert(
        (1, 2),
        HashMap::from([(SUBTASK_STATE_SUCCEED, 3), (SUBTASK_STATE_CANCELED, 1)]),
    );
    manager
        .task_errors
        .lock()
        .unwrap()
        .insert(1, vec![SchedulerError::new("one worker cancelled")]);
    let scheduler = scheduler(running, manager, Arc::new(TestExtension::default()), true);
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_REVERTING);
}

// 当前步骤没有活跃子任务时，暂停请求可以在一次调度后完成。
#[test]
fn test_pause() {
    let manager = Arc::new(TestTaskManager::default());
    let mut pausing = task(1, TASK_STATE_PAUSING);
    pausing.base.step = 1;
    let scheduler = scheduler(pausing, manager, Arc::new(TestExtension::default()), true);
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSED);
}

// 仍有运行中或待执行子任务时，主任务必须保持暂停中，等待子任务全部停稳。
#[test]
fn test_parallel_pause() {
    let manager = Arc::new(TestTaskManager::default());
    let mut pausing = task(1, TASK_STATE_PAUSING);
    pausing.base.step = 1;
    manager.state_counts.lock().unwrap().insert(
        (1, 1),
        HashMap::from([(SUBTASK_STATE_RUNNING, 2), (SUBTASK_STATE_PENDING, 1)]),
    );
    let scheduler = scheduler(pausing, manager, Arc::new(TestExtension::default()), true);
    scheduler.schedule_once().unwrap();
    assert_eq!(scheduler.task().base.state, TASK_STATE_PAUSING);
}

// Go 的 PauseTaskOnError 是持久化事务：任务转为 pausing，同时把当前步骤的
// failed subtask 转为 paused。不能只更新调度器本地快照，否则 owner 切换后会丢失暂停。
#[test]
fn test_disk_full_auto_pause_is_persisted() {
    let manager = Arc::new(TestTaskManager::default());
    let mut running = task(1, TASK_STATE_RUNNING);
    running.base.step = 1;
    running.base.extra_params.pause_on_kv_disk_full = true;
    manager.insert_task(running.clone());
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
    manager.active_subtasks.lock().unwrap().insert(
        1,
        vec![SubtaskBase {
            id: 10,
            task_id: 1,
            step: 1,
            state: SUBTASK_STATE_FAILED,
            exec_id: "n1".to_owned(),
            concurrency: 1,
            ordinal: 1,
        }],
    );

    let scheduler = scheduler(
        running,
        manager.clone(),
        Arc::new(TestExtension::default()),
        true,
    );
    scheduler.schedule_once().unwrap();

    assert_eq!(manager.task(1).base.state, TASK_STATE_PAUSING);
    assert_eq!(
        manager.task(1).error,
        Some(SchedulerError::new("TiKV disk full"))
    );
    assert_eq!(
        manager.active_subtasks.lock().unwrap()[&1][0].state,
        SUBTASK_STATE_PAUSED
    );
}

// 状态转换表既允许合法推进和幂等的运行态刷新，也拒绝跨阶段跳转。
#[test]
fn test_verify_task_state_transform() {
    let allowed = [
        (TASK_STATE_PENDING, TASK_STATE_RUNNING),
        (TASK_STATE_PENDING, TASK_STATE_CANCELLING),
        (TASK_STATE_PENDING, TASK_STATE_PAUSING),
        (TASK_STATE_RUNNING, TASK_STATE_SUCCEED),
        (TASK_STATE_RUNNING, TASK_STATE_REVERTING),
        (TASK_STATE_CANCELLING, TASK_STATE_REVERTING),
        (TASK_STATE_REVERTING, TASK_STATE_REVERTED),
        (TASK_STATE_PAUSING, TASK_STATE_PAUSED),
        (TASK_STATE_PAUSED, TASK_STATE_RESUMING),
        (TASK_STATE_RESUMING, TASK_STATE_RUNNING),
    ];
    for (from, to) in allowed {
        assert!(VerifyTaskStateTransform(from, to), "{from} -> {to}");
    }
    assert!(VerifyTaskStateTransform(
        TASK_STATE_RUNNING,
        TASK_STATE_RUNNING
    ));
    assert!(!VerifyTaskStateTransform(
        TASK_STATE_SUCCEED,
        TASK_STATE_RUNNING
    ));
    assert!(!VerifyTaskStateTransform(
        TASK_STATE_PENDING,
        TASK_STATE_REVERTED
    ));
}

// 取消识别依赖框架约定的消息标记，并允许该标记被外层错误文本包裹。
#[test]
fn test_is_cancelled_err() {
    assert!(IsCancelledErr(&SchedulerError::new(TASK_CANCEL_MESSAGE)));
    assert!(IsCancelledErr(&SchedulerError::new(format!(
        "wrapped {TASK_CANCEL_MESSAGE}"
    ))));
    assert!(!IsCancelledErr(&SchedulerError::new(
        "cancelled by timeout"
    )));
}

// 管理器启动后由一次 tick 为待处理任务创建调度器，停止时必须清空调度器集合。
#[test]
fn test_manager_schedule_loop() {
    let task_type = "manager-loop";
    RegisterSchedulerFactory(
        task_type,
        Arc::new(|task, param| {
            Arc::new(BaseScheduler::new(
                task,
                param,
                Arc::new(TestExtension::default()),
            ))
        }),
    );
    let task_manager = Arc::new(TestTaskManager::default());
    let mut waiting = task(1, "waiting");
    waiting.base.task_type = task_type.to_owned();
    task_manager.insert_task(waiting.clone());
    *task_manager.top_unfinished.lock().unwrap() = vec![waiting.base];
    let manager = Manager::new(task_manager, "server", None);
    assert!(!manager.initialized());
    manager.start().unwrap();
    manager.tick().unwrap();
    assert_eq!(manager.scheduler_count(), 1);
    manager.stop();
    assert_eq!(manager.scheduler_count(), 0);
}
