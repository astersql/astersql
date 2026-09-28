// Copyright 2026 AsterSQL.
//! DXF 测试工具的 Go/Rust 迁移契约测试。
//!
//! 本文件用可观测的内存桩覆盖测试上下文、调度器、执行器注册和任务表辅助函数，
//! 重点确认节点生命周期、错误脚本、清理时机及插入字段仍与 Go 版本一致。

use super::*;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// 记录测试上下文对 DXF 运行时的调用，并保存可被临时替换的资源和轮询间隔。
///
/// 各字段使用互斥锁，是为了让生命周期回调与测试断言共享同一份状态；它并不模拟
/// 生产运行时的调度实现。
struct Runtime {
    calls: Mutex<Vec<String>>,
    resource: Mutex<NodeResource>,
    intervals: Mutex<CheckIntervals>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            resource: Mutex::new(NodeResource {
                cpu_count: 1,
                memory_bytes: 1,
                disk_bytes: 1,
            }),
            intervals: Mutex::new(CheckIntervals {
                scheduler_running: std::time::Duration::from_secs(1),
                scheduler_finished: std::time::Duration::from_secs(1),
                cleanup: std::time::Duration::from_secs(1),
                task: std::time::Duration::from_secs(1),
                subtask: std::time::Duration::from_secs(1),
                max_subtask: std::time::Duration::from_secs(1),
                detect_modification: std::time::Duration::from_secs(1),
            }),
        }
    }
}

impl DxfRuntime for Runtime {
    fn set_node_resource(&self, resource: NodeResource) -> Result<NodeResource, DxfError> {
        let mut current = self.resource.lock().unwrap();
        let previous = *current;
        *current = resource;
        self.calls.lock().unwrap().push("resource".into());
        Ok(previous)
    }

    fn start_executor(&self, node_id: &str, _: NodeResource) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("start-executor:{node_id}"));
        Ok(())
    }

    fn stop_executor(&self, node_id: &str) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop-executor:{node_id}"));
        Ok(())
    }

    fn cancel_executor(&self, node_id: &str) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("cancel-executor:{node_id}"));
        Ok(())
    }

    fn start_scheduler(&self, node_id: &str, _: NodeResource) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("start-scheduler:{node_id}"));
        Ok(())
    }

    fn stop_scheduler(&self, node_id: &str) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop-scheduler:{node_id}"));
        Ok(())
    }

    fn cancel_scheduler(&self, node_id: &str) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("cancel-scheduler:{node_id}"));
        Ok(())
    }

    fn update_live_executor_ids(&self, node_ids: &[String]) -> Result<(), DxfError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("live:{node_ids:?}"));
        Ok(())
    }

    fn set_check_intervals(&self, intervals: CheckIntervals) -> Result<CheckIntervals, DxfError> {
        let mut current = self.intervals.lock().unwrap();
        let previous = *current;
        *current = intervals;
        self.calls.lock().unwrap().push("intervals".into());
        Ok(previous)
    }
}

#[test]
// 同时锁定步骤哨兵值、节点编号规则和上下文析构时触发的节点清理行为。
fn context_constants_and_lifecycle_match_go() {
    assert_eq!(STEP_INIT, Step(-1));
    assert_eq!(STEP_DONE, Step(-2));
    assert!(!TaskBase::default().is_done());
    assert!(
        TaskBase {
            state: TaskState::Reverted,
            ..Default::default()
        }
        .is_done()
    );

    let runtime = Arc::new(Runtime::default());
    {
        // 节点从固定端口号开始生成；缩容后，离开作用域还会清理剩余节点并恢复资源。
        let context = NewTestDXFContext(runtime.clone(), 2, 4, true).unwrap();
        assert_eq!(context.NodeCount(), 2);
        assert_eq!(context.GetNodeIDByIdx(0), ":4000");
        assert_eq!(context.GetRandNodeIDs(10).len(), 2);
        let subtask = Subtask {
            id: 7,
            task_id: 9,
            step: STEP_ONE,
            ..Default::default()
        };
        context.test_context().CollectSubtask(&subtask);
        assert_eq!(context.test_context().CollectedSubtaskCnt(9, STEP_ONE), 1);
        context.ScaleInBy(":4000").unwrap();
        assert_eq!(context.NodeCount(), 1);
    }
    let calls = runtime.calls.lock().unwrap();
    assert!(calls.iter().any(|call| call == "start-scheduler::4000"));
    assert!(calls.iter().any(|call| call == "stop-executor::4000"));
    assert!(calls.iter().any(|call| call == "stop-executor::4001"));
    assert!(calls.iter().any(|call| call == "stop-scheduler::4001"));
    assert_eq!(calls.iter().filter(|call| *call == "resource").count(), 2);
    assert_eq!(
        *runtime.resource.lock().unwrap(),
        NodeResource {
            cpu_count: 1,
            memory_bytes: 1,
            disk_bytes: 1,
        }
    );
}

#[test]
// 检查间隔 guard 必须恢复旧值，而对不存在节点的异步关闭应保持幂等空操作。
fn missing_async_shutdown_is_a_noop_and_intervals_restore() {
    let runtime = Arc::new(Runtime::default());
    let before = runtime.calls.lock().unwrap().len();
    {
        let guard = ReduceCheckInterval(runtime.clone()).unwrap();
        assert_eq!(
            runtime.intervals.lock().unwrap().scheduler_running,
            std::time::Duration::from_millis(100)
        );
        // Go 侧通过测试 Cleanup 恢复全局值；Rust 侧由 guard 的 Drop 承担同一职责。
        drop(guard);
    }
    let context = NewTestDXFContext(runtime.clone(), 0, 4, false).unwrap();
    context.AsyncShutdown(":missing".into()).unwrap();
    let calls = runtime.calls.lock().unwrap();
    assert!(calls.len() >= before + 2);
    assert!(!calls.iter().any(|call| call == "cancel-executor::missing"));
    assert_eq!(
        *runtime.intervals.lock().unwrap(),
        CheckIntervals {
            scheduler_running: std::time::Duration::from_secs(1),
            scheduler_finished: std::time::Duration::from_secs(1),
            cleanup: std::time::Duration::from_secs(1),
            task: std::time::Duration::from_secs(1),
            subtask: std::time::Duration::from_secs(1),
            max_subtask: std::time::Duration::from_secs(1),
            detect_modification: std::time::Duration::from_secs(1),
        }
    );
}

#[test]
// 固定调度器的步骤迁移、元数据编码以及“首次失败、随后成功”的错误脚本。
fn scheduler_scripts_match_go_transitions_and_errors() {
    let scheduler = GetMockBasicSchedulerExt();
    assert_eq!(scheduler.next_step(STEP_INIT), STEP_ONE);
    assert_eq!(scheduler.next_step(STEP_ONE), STEP_TWO);
    assert_eq!(scheduler.next_step(STEP_TWO), STEP_DONE);
    let task = Task {
        base: TaskBase {
            step: STEP_INIT,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        scheduler.next_subtasks_batch(&task, STEP_ONE).unwrap(),
        vec![
            b"subtask-0".to_vec(),
            b"subtask-1".to_vec(),
            b"subtask-2".to_vec()
        ]
    );
    assert_eq!(
        scheduler.next_subtasks_batch(&task, STEP_TWO).unwrap(),
        vec![b"subtask-0".to_vec()]
    );
    assert_eq!(
        scheduler.modify_meta(&[
            Modification {
                modification_type: "slots".into(),
                to: 3,
            },
            Modification {
                modification_type: "nodes".into(),
                to: 2,
            },
        ]),
        b"slots=3,nodes=2".to_vec()
    );

    let context = Arc::new(TestContext::default());
    let plan_error = GetPlanErrSchedulerExt(context);
    // 首次规划错误可重试，第二次必须产出与 Go mock 相同的三个子任务。
    assert!(plan_error.next_subtasks_batch(&task, STEP_ONE).is_err());
    assert_eq!(
        plan_error.next_subtasks_batch(&task, STEP_ONE).unwrap(),
        vec![b"task1".to_vec(), b"task2".to_vec(), b"task3".to_vec()]
    );
    let task = Task {
        base: TaskBase {
            step: STEP_ONE,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        plan_error.next_subtasks_batch(&task, STEP_TWO).unwrap(),
        vec![b"task4".to_vec()]
    );
    // 完成回调同样按调用顺序注入一次永久错误，后续调用恢复成功。
    assert!(plan_error.on_done().is_err());
    assert!(plan_error.on_done().is_ok());
}

#[test]
// 验证执行器注册由 RAII guard 清理，并确认通用扩展能把子任务送入注入的回调。
fn executor_helpers_run_and_cleanup_with_raii_registration() {
    #[derive(Default)]
    struct Registry {
        registered: Mutex<Vec<String>>,
        cleared: Mutex<usize>,
    }
    impl TaskExecutorRegistry for Registry {
        fn register_executor(
            &self,
            task_type: &str,
            _: TaskExecutorExtension,
        ) -> Result<(), DxfError> {
            self.registered.lock().unwrap().push(task_type.into());
            Ok(())
        }
        fn clear_executors(&self) {
            *self.cleared.lock().unwrap() += 1;
        }
    }

    let registry = Arc::new(Registry::default());
    let guard = InitTaskExecutor(registry.clone(), Arc::new(|_| Ok(()))).unwrap();
    assert_eq!(&*registry.registered.lock().unwrap(), &["Example"]);
    // guard 析构对应 Go 测试的 Cleanup，避免注册项泄漏到后续用例。
    drop(guard);
    assert_eq!(*registry.cleared.lock().unwrap(), 1);

    let ran = Arc::new(Mutex::new(Vec::new()));
    let ran_for_callback = ran.clone();
    let extension = GetCommonTaskExecutorExt(Arc::new(move |_| {
        let ran = ran_for_callback.clone();
        Ok(GetCommonStepExecutor(
            STEP_ONE,
            Arc::new(move |_| {
                ran.lock().unwrap().push("run");
                Ok(())
            }),
        ))
    }));
    let task = Task::default();
    let subtask = Subtask::default();
    run_registered_subtask(&extension, &task, &subtask).unwrap();
    assert_eq!(&*ran.lock().unwrap(), &["run"]);
}

#[derive(Default)]
/// 只记录插入请求的任务表桩；其余查询返回中性值，让断言聚焦于字段组装契约。
struct Table {
    inserted: Mutex<Vec<NewSubtask>>,
}

impl TaskTable for Table {
    fn insert_subtask(&self, subtask: NewSubtask) -> Result<i64, DxfError> {
        self.inserted.lock().unwrap().push(subtask);
        Ok(self.inserted.lock().unwrap().len() as i64)
    }
    fn get_one_pending_task(&self) -> Result<Option<Task>, DxfError> {
        Ok(None)
    }
    fn subtask_history_count(&self, _: Option<i64>) -> Result<usize, DxfError> {
        Ok(0)
    }
    fn subtasks_by_task_id(&self, _: i64) -> Result<Vec<Subtask>, DxfError> {
        Ok(Vec::new())
    }
    fn task_history_count(&self) -> Result<usize, DxfError> {
        Ok(0)
    }
    fn task_end_time(&self, _: i64) -> Result<Option<SystemTime>, DxfError> {
        Ok(None)
    }
    fn subtask_end_time(&self, _: i64) -> Result<Option<SystemTime>, DxfError> {
        Ok(None)
    }
    fn subtask_nodes(&self, _: i64) -> Result<Vec<String>, DxfError> {
        Ok(Vec::new())
    }
    fn update_subtask_exec_id(&self, _: &str, _: i64) -> Result<(), DxfError> {
        Ok(())
    }
    fn transfer_subtasks_to_history(&self, _: i64) -> Result<(), DxfError> {
        Ok(())
    }
    fn history_tasks_in_states(&self, _: &[TaskState]) -> Result<Vec<Task>, DxfError> {
        Ok(Vec::new())
    }
    fn delete_subtasks(&self, _: i64) -> Result<(), DxfError> {
        Ok(())
    }
    fn is_task_cancelling(&self, _: i64) -> Result<bool, DxfError> {
        Ok(false)
    }
    fn print_subtask_info(&self, _: i64) -> Result<(), DxfError> {
        Ok(())
    }
}

#[test]
// 对比普通子任务与带摘要子任务的默认状态、开始时间标记和任务 Keyspace 选择。
fn table_and_task_helpers_preserve_insert_fields_and_cleanup() {
    let table = Table::default();
    assert_eq!(
        CreateSubTask(
            &table,
            11,
            STEP_ONE,
            ":4000",
            b"meta".to_vec(),
            "Example",
            8
        )
        .unwrap(),
        1
    );
    assert_eq!(
        InsertSubtaskWithSummary(
            &table,
            11,
            STEP_TWO,
            ":4001",
            Vec::new(),
            b"{}".to_vec(),
            SubtaskState::Succeed,
            "Example",
            8,
        )
        .unwrap(),
        2
    );
    let inserted = table.inserted.lock().unwrap();
    // 普通创建保持待处理且未开始；显式摘要路径则保留摘要并标记已有开始时间。
    assert_eq!(inserted[0].state, SubtaskState::Pending);
    assert!(!inserted[0].has_start_time);
    assert_eq!(inserted[0].task_id, 11);
    assert_eq!(inserted[0].step, STEP_ONE);
    assert_eq!(inserted[0].exec_id, ":4000");
    assert_eq!(inserted[0].meta, b"meta");
    assert_eq!(inserted[0].task_type, "Example");
    assert_eq!(inserted[0].concurrency, 8);
    assert_eq!(inserted[0].summary_json, None);
    assert_eq!(inserted[1].task_id, 11);
    assert_eq!(inserted[1].step, STEP_TWO);
    assert_eq!(inserted[1].exec_id, ":4001");
    assert!(inserted[1].meta.is_empty());
    assert_eq!(inserted[1].state, SubtaskState::Succeed);
    assert_eq!(inserted[1].task_type, "Example");
    assert_eq!(inserted[1].concurrency, 8);
    assert_eq!(inserted[1].summary_json, Some(b"{}".to_vec()));
    assert!(inserted[1].has_start_time);
    assert_eq!(getTaskKS(false), "");
    assert_eq!(getTaskKS(true), "SYSTEM");
}
