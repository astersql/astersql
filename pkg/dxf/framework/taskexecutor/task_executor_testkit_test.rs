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

// BaseTaskExecutor 的 testkit 风格集成测试（内存 TaskTable 假实现）。
//
// 对应 Go 的 `task_executor_testkit_test.go`：创建任务后经两步子任务，
// 验证每步 Pending → Succeed 迁移；因 storage 包未移植，用 FakeTaskTable 代替。

// Ported from pkg/dxf/framework/taskexecutor/task_executor_testkit_test.go.
// Go's `runOneTask`/`TestTaskExecutorBasic` drive a real `storage.TaskManager`
// backed by a `testkit` mock TiDB store; this crate has no SQL-backed
// `TaskTable` implementation (that lives in the still-unported `storage`
// package), so this test drives the same real `BaseTaskExecutor`/registry
// machinery against an in-memory `TaskTable` fake instead, exercising the
// exact scenario Go covers: create a task, run it through two steps'
// worth of subtasks via the registered executor factory, and check that
// every subtask transitions pending -> succeed at each step.

use crate::{
    Context, ExecutorError, Extension, GetTaskExecutorFactory, NewParamForTest, NodeResource,
    RegisterTaskType, RegistryLockForTest, Result, StepExecutor, Subtask, SubtaskBase,
    SubtaskState, Task, TaskBase, TaskExecutor, TaskState, TaskTable, newSlotManager,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 测试用执行节点 ID。
const EXEC_ID: &str = ":4000";
/// 注册到执行器工厂的任务类型名。
const TASK_TYPE: &str = "testkit-example";
/// 第一步步骤号。
const STEP_ONE: i64 = 1;
/// 第二步步骤号。
const STEP_TWO: i64 = 2;

/// 内存版任务表，替代 Go 中基于 session 的 `storage.TaskManager`。
/// FakeTaskTable stands in for Go's real, session-backed `storage.TaskManager`,
/// tracking tasks and their subtasks purely in memory.
#[derive(Default)]
struct FakeTaskTable {
    tasks: Mutex<HashMap<i64, Task>>,
    subtasks: Mutex<Vec<Subtask>>,
    next_subtask_id: Mutex<i64>,
}
impl FakeTaskTable {
    /// 对应 CreateTask + SwitchTaskStep 到 Running/StepOne。
    /// Mirrors `TaskManager.CreateTask` followed by `SwitchTaskStep` to
    /// `TaskStateRunning`/`StepOne`.
    fn create_task(&self, id: i64, task_type: &str) {
        self.tasks.lock().unwrap().insert(
            id,
            Task {
                TaskBase: TaskBase {
                    ID: id,
                    Type: task_type.to_string(),
                    State: TaskState::Running,
                    Step: STEP_ONE,
                    ..Default::default()
                },
                Meta: vec![],
            },
        );
    }
    /// 切换任务当前步骤。
    /// Mirrors `TaskManager.SwitchTaskStep`.
    fn switch_step(&self, id: i64, step: i64) {
        if let Some(task) = self.tasks.lock().unwrap().get_mut(&id) {
            task.TaskBase.Step = step;
        }
    }
    /// 创建 Pending 子任务。
    /// Mirrors `testutil.CreateSubTask`.
    fn create_subtask(&self, task_id: i64, step: i64) {
        let mut next_id = self.next_subtask_id.lock().unwrap();
        *next_id += 1;
        self.subtasks.lock().unwrap().push(Subtask {
            SubtaskBase: SubtaskBase {
                ID: *next_id,
                TaskID: task_id,
                Step: step,
                State: SubtaskState::Pending,
                ExecID: EXEC_ID.to_string(),
            },
            Meta: vec![],
        });
    }
    /// 统计指定任务/步骤/状态的子任务数。
    /// Mirrors `TaskManager.GetAllSubtasksByStepAndState`.
    fn count_subtasks(&self, task_id: i64, step: i64, state: SubtaskState) -> usize {
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
/// 实现 TaskTable：按队列语义供 BaseTaskExecutor 拉取。
impl TaskTable for FakeTaskTable {
    /// 按 ID 取任务。
    fn GetTaskByID(&self, _: &Context, id: i64) -> Result<Task> {
        self.tasks
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| ExecutorError("task not found".into()))
    }
    /// 取第一个匹配 exec_id/task/step/states 的子任务。
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
    /// 更新子任务状态（忽略错误载荷）。
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
    /// 标记 Succeed 并写入 meta。
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

/// 记录实际跑过的步骤，非法步骤直接 panic。
/// Records which steps actually ran, mirroring the Go test's callback that
/// logs `"run step one"`/`"run step two"` and panics on any other step.
#[derive(Default)]
struct RecordingStepExecutor {
    ran_steps: Mutex<Vec<i64>>,
}
impl StepExecutor for RecordingStepExecutor {
    fn RunSubtask(&self, _: &Context, subtask: &mut Subtask) -> Result<()> {
        match subtask.SubtaskBase.Step {
            STEP_ONE | STEP_TWO => {
                self.ran_steps
                    .lock()
                    .unwrap()
                    .push(subtask.SubtaskBase.Step);
                Ok(())
            }
            other => panic!("invalid step: {other}"),
        }
    }
}

#[derive(Default)]
/// 始终返回同一 RecordingStepExecutor 的扩展。
struct RecordingExtension {
    step: Arc<RecordingStepExecutor>,
    get_step_calls: AtomicUsize,
}
impl Extension for RecordingExtension {
    fn IsIdempotent(&self, _: &Subtask) -> bool {
        true
    }
    fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        self.get_step_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.step.clone())
    }
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        false
    }
}

/// 跑完 StepOne/StepTwo 各 `subtask_cnt` 个子任务并断言状态迁移。
/// run_one_task mirrors Go's `runOneTask`: create a task, run `subtaskCnt`
/// subtasks through StepOne then StepTwo via the registered executor
/// factory, checking pending -> succeed transitions at each step.
fn run_one_task(table: &Arc<FakeTaskTable>, task_id: i64, subtask_cnt: usize) {
    table.create_task(task_id, TASK_TYPE);

    // 第一步：创建 Pending 子任务 → Run → 断言全部 Succeed。
    // 1. StepOne.
    for _ in 0..subtask_cnt {
        table.create_subtask(task_id, STEP_ONE);
    }
    assert_eq!(
        subtask_cnt,
        table.count_subtasks(task_id, STEP_ONE, SubtaskState::Pending)
    );

    let task = table.GetTaskByID(&Context::Background(), task_id).unwrap();
    let factory = GetTaskExecutorFactory(&task.TaskBase.Type).expect("factory must be registered");
    let extension_impl = Arc::new(RecordingExtension::default());
    let extension: Arc<dyn Extension> = extension_impl.clone();
    let param = NewParamForTest(
        table.clone(),
        Arc::new(newSlotManager(1)),
        NodeResource::default(),
        EXEC_ID,
        extension,
    );
    let executor = factory(Context::Background(), task, param);
    executor.Run();
    assert_eq!(
        subtask_cnt,
        table.count_subtasks(task_id, STEP_ONE, SubtaskState::Succeed)
    );

    // 第二步：切换 step 后复用同一 executor 实例再跑一轮。
    // 2. StepTwo, reusing the same executor instance like Go does.
    table.switch_step(task_id, STEP_TWO);
    for _ in 0..subtask_cnt {
        table.create_subtask(task_id, STEP_TWO);
    }
    assert_eq!(
        subtask_cnt,
        table.count_subtasks(task_id, STEP_TWO, SubtaskState::Pending)
    );
    executor.Run();
    assert_eq!(
        subtask_cnt,
        table.count_subtasks(task_id, STEP_TWO, SubtaskState::Succeed)
    );
    if subtask_cnt > 0 {
        assert_eq!(2, extension_impl.get_step_calls.load(Ordering::SeqCst));
    }
}

#[test]
/// 连续 10 次以不同子任务数跑 run_one_task，覆盖注册与清理。
fn test_task_executor_basic() {
    let _guard = RegistryLockForTest();
    // 缩短无子任务轮询退避，避免每步耗尽后空等数秒。
    // Go's `taskexecutor.ReduceCheckInterval(t)`: shrink the no-subtask
    // poll backoff so exhausting each step's subtasks doesn't stall the
    // test for several seconds.
    let _interval_guard = crate::main_test::ReduceCheckInterval();
    let table = Arc::new(FakeTaskTable::default());
    // 注册工厂：始终构造 BaseTaskExecutor。
    RegisterTaskType(
        TASK_TYPE.to_string(),
        Arc::new(|ctx: Context, task: Task, param| {
            crate::NewBaseTaskExecutor(ctx, task, param) as Arc<dyn TaskExecutor>
        }),
    );

    // 子任务数从 0 递增到 9，覆盖空步与多子任务。
    for i in 0..10 {
        run_one_task(&table, (i + 1) as i64, i as usize);
    }

    crate::ClearTaskExecutors();
}
