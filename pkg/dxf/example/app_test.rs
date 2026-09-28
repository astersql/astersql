// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DXF example 端到端应用测试。
//
// 用本 crate 已移植的 scheduler / taskExecutor 驱动两步子任务，
// 并以内存 FakeTaskTable 代替尚未移植的 storage.TaskManager。
// DXF（Distributed eXecution Framework）是集群内分布式任务调度与执行框架。

// Ported from pkg/dxf/example/app_test.go. Go's `TestExampleApplication`
// registers the example scheduler/cleanup/task-executor factories with the
// real DXF `scheduler`/`handle` framework, submits a task through a
// mock-store-backed `storage.TaskManager`, and waits for the framework's
// own background loops to drive it to completion. Those framework crates
// (`scheduler`, `handle`, `storage`, `testkit`) are still unported
// (`[target.'cfg(any())'.dependencies]` placeholders in this crate's
// Cargo.toml) and are out of this task's scope, so this test instead
// exercises this crate's own real, already-ported production code
// end-to-end: the real `schedulerImpl` (`newScheduler`/`Init`/
// `OnNextSubtasksBatch`/`GetNextStep`/`OnDone`) drives subtask-meta
// generation for each step exactly like the real DXF scheduler loop would,
// and the real `taskExecutor`/`stepExecutor` (`newTaskExecutor`, registered
// via `RegisterTaskType` exactly as Go's `TestExampleApplication` does)
// consumes and runs those subtasks to completion, against an in-memory
// `TaskTable` fake standing in for the unported `storage.TaskManager`.

use crate::{
    StepDone, StepInit, StepOne, StepTwo, newScheduler, newTaskExecutor, postCleanupImpl,
    subtaskMeta, taskMeta,
};
use astersql_dxf_framework_taskexecutor::{
    ClearTaskExecutors, Context, ExecutorError, Extension, GetTaskExecutorFactory, NewParamForTest,
    NodeResource, RegisterTaskType, Result, SetSubtaskCheckIntervalForTest, StepExecutor, Subtask,
    SubtaskBase, SubtaskState, Task, TaskBase, TaskExecutor, TaskState, TaskTable, newSlotManager,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 测试用执行节点 ID（exec_id）。
const EXEC_ID: &str = "test";
/// 示例任务类型名，需与 RegisterTaskType 注册名一致。
const TASK_TYPE: &str = "example";

#[derive(Default)]
/// 内存版 TaskTable：保存任务与子任务状态，供 executor 轮询消费。
struct FakeTaskTable {
    tasks: Mutex<HashMap<i64, Task>>,
    subtasks: Mutex<Vec<Subtask>>,
    next_subtask_id: Mutex<i64>,
}
/// FakeTaskTable 的写入与查询辅助。
impl FakeTaskTable {
    /// 写入或覆盖指定任务快照。
    fn set_task(&self, task: Task) {
        self.tasks.lock().unwrap().insert(task.TaskBase.ID, task);
    }
    /// 推进任务当前 step（步骤序号）。
    fn set_step(&self, id: i64, step: i64) {
        if let Some(task) = self.tasks.lock().unwrap().get_mut(&id) {
            task.TaskBase.Step = step;
        }
    }
    /// 按 meta 批次追加 Pending 子任务。
    fn add_subtasks(&self, task_id: i64, step: i64, metas: Vec<Vec<u8>>) {
        let mut next_id = self.next_subtask_id.lock().unwrap();
        let mut subtasks = self.subtasks.lock().unwrap();
        for meta in metas {
            *next_id += 1;
            subtasks.push(Subtask {
                SubtaskBase: SubtaskBase {
                    ID: *next_id,
                    TaskID: task_id,
                    Step: step,
                    State: SubtaskState::Pending,
                    ExecID: EXEC_ID.to_string(),
                },
                Meta: meta,
            });
        }
    }
    /// 统计某任务某 step 下指定状态的子任务数量。
    fn count(&self, task_id: i64, step: i64, state: SubtaskState) -> usize {
        self.subtasks
            .lock()
            .unwrap()
            .iter()
            .filter(|s| {
                s.SubtaskBase.TaskID == task_id
                    && s.SubtaskBase.Step == step
                    && s.SubtaskBase.State == state
            })
            .count()
    }
}
/// 实现框架 TaskTable：供 BaseTaskExecutor 拉取/更新子任务。
impl TaskTable for FakeTaskTable {
    /// 按 ID 取任务；不存在则返回错误。
    fn GetTaskByID(&self, _: &Context, id: i64) -> Result<Task> {
        self.tasks
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| ExecutorError("task not found".into()))
    }
    /// 取首个匹配 exec_id/task/step/状态集合的子任务。
    fn GetFirstSubtaskInStates(
        &self,
        _: &Context,
        exec_id: &str,
        task_id: i64,
        step: i64,
        states: &[SubtaskState],
    ) -> Result<Option<Subtask>> {
        Ok(self
            .subtasks
            .lock()
            .unwrap()
            .iter()
            .find(|s| {
                s.SubtaskBase.TaskID == task_id
                    && s.SubtaskBase.Step == step
                    && s.SubtaskBase.ExecID == exec_id
                    && states.contains(&s.SubtaskBase.State)
            })
            .cloned())
    }
    /// 将子任务标为 Running 并绑定 exec_id。
    fn StartSubtask(&self, _: &Context, id: i64, exec_id: &str) -> Result<()> {
        for subtask in self.subtasks.lock().unwrap().iter_mut() {
            if subtask.SubtaskBase.ID == id {
                subtask.SubtaskBase.State = SubtaskState::Running;
                subtask.SubtaskBase.ExecID = exec_id.to_string();
            }
        }
        Ok(())
    }
    /// 更新子任务状态（错误参数在本假实现中忽略）。
    fn UpdateSubtaskStateAndError(
        &self,
        _: &Context,
        _: &str,
        id: i64,
        state: SubtaskState,
        _: Option<&ExecutorError>,
    ) -> Result<()> {
        for subtask in self.subtasks.lock().unwrap().iter_mut() {
            if subtask.SubtaskBase.ID == id {
                subtask.SubtaskBase.State = state;
            }
        }
        Ok(())
    }
    /// 将子任务标为 Succeed 并写回 meta。
    fn FinishSubtask(&self, _: &Context, _: &str, id: i64, meta: &[u8]) -> Result<()> {
        for subtask in self.subtasks.lock().unwrap().iter_mut() {
            if subtask.SubtaskBase.ID == id {
                subtask.SubtaskBase.State = SubtaskState::Succeed;
                subtask.Meta = meta.to_vec();
            }
        }
        Ok(())
    }
}

/// Never actually invoked: `newTaskExecutor` always overwrites
/// `param.Extension` with the crate's real `exampleExtension`, mirroring
/// Go's factory. `NewParamForTest` still needs *some* `Extension` to build
/// a `Param` with, hence this placeholder.
/// 占位 Extension：不会被真正调用，构造后即被 newTaskExecutor 覆盖。
struct UnusedExtension;
impl Extension for UnusedExtension {
    fn IsIdempotent(&self, _: &Subtask) -> bool {
        unreachable!("overwritten by newTaskExecutor before use")
    }
    fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        unreachable!("overwritten by newTaskExecutor before use")
    }
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        unreachable!("overwritten by newTaskExecutor before use")
    }
}

/// 串行化全局 RegisterTaskType / ClearTaskExecutors，避免并行测试互相污染。
fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// test_example_application mirrors Go's `TestExampleApplication`: register
/// the example task type, submit a task carrying a `taskMeta{SubtaskCount:
/// 3}`, and drive it through StepOne and StepTwo until `GetNextStep`
/// reports `StepDone`, checking every subtask's message round-trips through
/// `subtaskMeta::Marshal`/`Unmarshal` along the way.
/// 端到端跑通 example 任务：注册类型、两步子任务、校验 meta 往返。
#[test]
// 注册 example 工厂 → 调度生成 StepOne/Two 子任务 → executor.Run 跑完 → 清理。
fn test_example_application() {
    let _guard = registry_lock();
    // Go's `ReduceCheckInterval`: shrink the no-subtask poll backoff so
    // exhausting each step's subtasks doesn't stall the test for seconds.
    let backoff_guard =
        SetSubtaskCheckIntervalForTest(Duration::from_millis(1), Duration::from_millis(1));
    RegisterTaskType(
        TASK_TYPE.to_string(),
        Arc::new(|ctx, task, param| newTaskExecutor(ctx, task, param) as Arc<dyn TaskExecutor>),
    );

    // 每步生成 3 个子任务。
    let meta = taskMeta { SubtaskCount: 3 };
    let task_id = 1;
    let mut task_base = TaskBase {
        ID: task_id,
        Type: TASK_TYPE.to_string(),
        State: TaskState::Running,
        Step: StepInit,
        ..Default::default()
    };
    let task = Task {
        TaskBase: task_base.clone(),
        Meta: meta.Marshal(),
    };

    let table = Arc::new(FakeTaskTable::default());
    table.set_task(task.clone());

    let mut scheduler = newScheduler(Context::Background(), task.clone());
    scheduler
        .Init()
        .expect("scheduler init should unmarshal taskMeta");
    assert_eq!(3, scheduler.subtaskCount);

    let factory = GetTaskExecutorFactory(TASK_TYPE).expect("example task type must be registered");
    let param = NewParamForTest(
        table.clone(),
        Arc::new(newSlotManager(1)),
        NodeResource::default(),
        EXEC_ID,
        Arc::new(UnusedExtension),
    );
    let executor = factory(Context::Background(), task.clone(), param);

    // 逐步：取 next step → 生成 subtask meta → 入表 → Run 直至全部 Succeed。
    for &want_step in &[StepOne, StepTwo] {
        let next_step = scheduler.GetNextStep(&task_base);
        assert_eq!(want_step, next_step);
        task_base.Step = next_step;
        table.set_step(task_id, next_step);

        let metas = scheduler
            .OnNextSubtasksBatch(&Context::Background(), &task, &[], next_step)
            .expect("OnNextSubtasksBatch should build subtaskMeta for every subtask");
        assert_eq!(3, metas.len());
        for bytes in &metas {
            let decoded = subtaskMeta::Unmarshal(bytes).expect("subtaskMeta should round-trip");
            assert!(decoded.Message.contains(&format!(
                "step {}",
                if next_step == StepOne { "one" } else { "two" }
            )));
        }
        table.add_subtasks(task_id, next_step, metas);
        assert_eq!(3, table.count(task_id, next_step, SubtaskState::Pending));

        executor.Run();
        assert_eq!(3, table.count(task_id, next_step, SubtaskState::Succeed));
    }

    assert_eq!(StepDone, scheduler.GetNextStep(&task_base));
    scheduler
        .OnDone(&Context::Background(), &task)
        .expect("OnDone should succeed");
    postCleanupImpl
        .CleanUp(&Context::Background(), &task)
        .expect("CleanUp should succeed");

    SetSubtaskCheckIntervalForTest(backoff_guard.0, backoff_guard.1);
    ClearTaskExecutors();
}

#[test]
fn task_meta_unmarshal_matches_go_json_object_rules() {
    let decoded =
        taskMeta::Unmarshal(br#" { "ignored": {"nested": [1, true]}, "subtask_count" : 3 } "#)
            .expect("valid JSON object should decode");
    assert_eq!(3, decoded.SubtaskCount);

    assert_eq!(
        0,
        taskMeta::Unmarshal(br#"{"ignored": 1}"#)
            .expect("missing Go struct fields default to zero")
            .SubtaskCount
    );
}

#[test]
fn subtask_meta_json_matches_go_string_escaping() {
    let meta = subtaskMeta {
        Message: "line\n\t世 \" \\".to_string(),
    };
    assert_eq!(
        r#"{"message":"line\n\t世 \" \\"}"#.as_bytes(),
        meta.Marshal()
    );

    let decoded = subtaskMeta::Unmarshal(br#"{"message":"line\n\t\u4e16 \" \\"}"#)
        .expect("escaped JSON string should decode");
    assert_eq!(meta, decoded);
    assert_eq!(
        "",
        subtaskMeta::Unmarshal(br#"{"ignored": 1}"#)
            .expect("missing Go struct fields default to empty")
            .Message
    );
}

#[test]
#[should_panic(expected = "unknown step 99")]
fn scheduler_unknown_step_error_matches_go() {
    let task = Task {
        TaskBase: TaskBase {
            Type: TASK_TYPE.to_string(),
            ..Default::default()
        },
        Meta: taskMeta { SubtaskCount: 1 }.Marshal(),
    };
    let mut scheduler = newScheduler(Context::Background(), task.clone());
    scheduler.subtaskCount = 1;
    let _ = scheduler.OnNextSubtasksBatch(&Context::Background(), &task, &[], 99);
}

#[test]
#[should_panic]
fn scheduler_negative_subtask_count_matches_go_make() {
    let task = Task {
        TaskBase: TaskBase {
            Type: TASK_TYPE.to_string(),
            ..Default::default()
        },
        Meta: taskMeta { SubtaskCount: -1 }.Marshal(),
    };
    let mut scheduler = newScheduler(Context::Background(), task.clone());
    scheduler.subtaskCount = -1;
    let _ = scheduler.OnNextSubtasksBatch(&Context::Background(), &task, &[], StepOne);
}
