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

// 任务类型工厂注册表测试。
//
// 验证 `RegisterTaskType`/`GetTaskExecutorFactory`/`ClearTaskExecutors`
// 在进程级注册表上的注册、覆盖与清理行为。

// Ported from pkg/dxf/framework/taskexecutor/register_test.go.

// 工厂返回 Arc<dyn TaskExecutor>。
use std::sync::Arc;

use crate::{
    ClearTaskExecutors, Context, GetTaskExecutorFactory, Param, RegisterTaskType,
    RegistryLockForTest, Task, TaskExecutor,
};

/// 空操作执行器，仅用于验证工厂可被调用。
struct NoopTaskExecutor;
impl TaskExecutor for NoopTaskExecutor {
    fn Init(&self, _: &Context) -> crate::Result<()> {
        Ok(())
    }
    fn Run(&self) {}
    fn GetTaskBase(&self) -> crate::TaskBase {
        crate::TaskBase::default()
    }
    fn CancelRunningSubtask(&self) {}
    fn Cancel(&self) {}
    fn Close(&self) {}
    fn IsRetryableError(&self, _: &crate::ExecutorError) -> bool {
        false
    }
}

/// 构造返回 NoopTaskExecutor 的工厂。
fn factory() -> crate::FactoryFn {
    Arc::new(|_ctx: Context, _task: Task, _param: Param| {
        Arc::new(NoopTaskExecutor) as Arc<dyn TaskExecutor>
    })
}

#[test]
/// 覆盖注册、覆盖同名类型、清空注册表。
fn test_register_task_type() {
    let _guard = RegistryLockForTest();
    // other case might add task types, so we need to clear it first.
    ClearTaskExecutors();

    RegisterTaskType("test1".to_string(), factory());
    assert!(GetTaskExecutorFactory("test1").is_some());
    assert!(GetTaskExecutorFactory("test2").is_none());

    RegisterTaskType("test2".to_string(), factory());
    assert!(GetTaskExecutorFactory("test1").is_some());
    assert!(GetTaskExecutorFactory("test2").is_some());

    // register again with the same type must not error and must keep the
    // registry at the same two entries.
    RegisterTaskType("test2".to_string(), factory());
    assert!(GetTaskExecutorFactory("test1").is_some());
    assert!(GetTaskExecutorFactory("test2").is_some());

    ClearTaskExecutors();
    assert!(GetTaskExecutorFactory("test1").is_none());
    assert!(GetTaskExecutorFactory("test2").is_none());
}

#[test]
/// 取出的工厂应能创建并 Init/Run 执行器。
fn test_get_task_executor_factory_invokes_registered_factory() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    RegisterTaskType("invokable".to_string(), factory());

    let found = GetTaskExecutorFactory("invokable").expect("factory should be registered");
    let executor = found(Context::Background(), Task::default(), test_param());
    executor
        .Init(&Context::Background())
        .expect("Init should succeed");
    executor.Run();

    ClearTaskExecutors();
}

/// 构造最小可用 Param（空表 + 空 Extension）。
fn test_param() -> Param {
    /// GetTaskByID 恒失败的空表。
    struct EmptyTaskTable;
    impl crate::TaskTable for EmptyTaskTable {
        fn GetTaskByID(&self, _: &Context, _: i64) -> crate::Result<Task> {
            Err(crate::ExecutorError("not found".into()))
        }
    }
    /// 默认幂等的空 Extension。
    struct EmptyExtension;
    impl crate::Extension for EmptyExtension {
        fn IsIdempotent(&self, _: &crate::Subtask) -> bool {
            true
        }
        fn GetStepExecutor(&self, _: &Task) -> crate::Result<Arc<dyn crate::StepExecutor>> {
            Ok(Arc::new(crate::BaseStepExecutor))
        }
        fn IsRetryableError(&self, _: &crate::ExecutorError) -> bool {
            false
        }
    }
    crate::NewParamForTest(
        Arc::new(EmptyTaskTable),
        Arc::new(crate::newSlotManager(1)),
        crate::NodeResource::default(),
        "test-exec",
        Arc::new(EmptyExtension),
    )
}
