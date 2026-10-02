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

// Handle 层的行为测试。
//
// 通过内存 Runtime 覆盖任务生命周期、重试策略、部署模式差异、对象存储与计量钩子；
// 涉及进程级全局 Runtime 的用例使用独占锁串行执行，避免并发测试相互替换依赖。

use super::*;
use std::collections::HashSet;
use std::fs::{File, create_dir, remove_dir, remove_file};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
/// Handle 测试 Runtime：生命周期用例委托真实 SQL 存储，其余边界用例记录内存调用。
struct MockRuntime {
    storage: Option<storage::TaskManager>,
    tasks: Mutex<Vec<proto::Task>>,
    history_keys: Mutex<HashSet<String>>,
    next_id: AtomicI64,
    next_gen: bool,
    service_scope: String,
    cloud_storage_uri: Mutex<String>,
    sem_enabled: bool,
    cluster_id: Option<u64>,
    meter_writes: Mutex<Vec<(i64, String, MeterItem)>>,
    history_lookup_not_found: bool,
    active_lookup_not_found: bool,
}

impl MockRuntime {
    // Storage currently owns a separate protocol representation. Copy every field
    // at the Runtime boundary so the test observes the persisted SQL result.
    fn stored_task(value: storage::proto::Task) -> proto::Task {
        let base = value.TaskBase;
        proto::Task {
            TaskBase: proto::TaskBase {
                ID: base.ID,
                Key: base.Key,
                Type: base.Type,
                State: base.State,
                Step: base.Step,
                Priority: base.Priority,
                RequiredSlots: base.RequiredSlots,
                TargetScope: base.TargetScope,
                CreateTime: base.CreateTime,
                MaxNodeCount: base.MaxNodeCount,
                Keyspace: base.Keyspace,
                ExtraParams: proto::ExtraParams {
                    ManualRecovery: base.ExtraParams.ManualRecovery,
                    PauseOnKVDiskFull: base.ExtraParams.PauseOnKVDiskFull,
                    MaxRuntimeSlots: base.ExtraParams.MaxRuntimeSlots,
                    TargetSteps: base.ExtraParams.TargetSteps,
                    PrepareMode: base.ExtraParams.PrepareMode,
                },
            },
            SchedulerID: value.SchedulerID,
            StartTime: value.StartTime,
            StateUpdateTime: value.StateUpdateTime,
            Meta: value.Meta,
            Error: value.Error,
            ModifyParam: proto::ModifyParam {
                PrevState: value.ModifyParam.PrevState,
                Modifications: value
                    .ModifyParam
                    .Modifications
                    .into_iter()
                    .map(|item| proto::Modification {
                        Type: item.Type,
                        To: item.To,
                    })
                    .collect(),
            },
        }
    }

    /// 深拷贝任务，确保调用方拿到的结果不会共享夹具内部的可变数据。
    fn clone_task(task: &proto::Task) -> proto::Task {
        proto::Task {
            TaskBase: proto::TaskBase {
                ID: task.ID,
                Key: task.Key.clone(),
                Type: task.Type,
                State: task.State,
                Step: task.Step,
                Priority: task.Priority,
                RequiredSlots: task.RequiredSlots,
                TargetScope: task.TargetScope.clone(),
                CreateTime: task.CreateTime,
                MaxNodeCount: task.MaxNodeCount,
                ExtraParams: proto::ExtraParams {
                    ManualRecovery: task.ExtraParams.ManualRecovery,
                    PauseOnKVDiskFull: task.ExtraParams.PauseOnKVDiskFull,
                    MaxRuntimeSlots: task.ExtraParams.MaxRuntimeSlots,
                    TargetSteps: task.ExtraParams.TargetSteps.clone(),
                    PrepareMode: task.ExtraParams.PrepareMode,
                },
                Keyspace: task.Keyspace.clone(),
            },
            SchedulerID: task.SchedulerID.clone(),
            StartTime: task.StartTime,
            StateUpdateTime: task.StateUpdateTime,
            Meta: task.Meta.clone(),
            Error: task.Error.clone(),
            ModifyParam: proto::ModifyParam {
                PrevState: task.ModifyParam.PrevState,
                Modifications: task
                    .ModifyParam
                    .Modifications
                    .iter()
                    .map(|modification| proto::Modification {
                        Type: modification.Type,
                        To: modification.To,
                    })
                    .collect(),
            },
        }
    }

    fn task(&self, key: &str, state: proto::TaskState, error: Option<&str>) -> proto::Task {
        let now = SystemTime::now();
        proto::Task {
            TaskBase: proto::TaskBase {
                ID: self.next_id.fetch_add(1, Ordering::SeqCst),
                Key: key.to_owned(),
                Type: proto::TaskTypeExample,
                State: state,
                Step: proto::StepInit,
                Priority: proto::NormalPriority,
                RequiredSlots: 2,
                TargetScope: String::new(),
                CreateTime: now,
                MaxNodeCount: 0,
                ExtraParams: proto::ExtraParams::default(),
                Keyspace: "test".to_owned(),
            },
            SchedulerID: String::new(),
            StartTime: now,
            StateUpdateTime: now,
            Error: error.map(str::to_owned),
            Meta: proto::EmptyMeta.to_vec(),
            ModifyParam: proto::ModifyParam {
                PrevState: proto::TaskStatePending,
                Modifications: Vec::new(),
            },
        }
    }

    fn install(self: Arc<Self>) -> RuntimeGuard {
        ClearRuntime();
        InstallRuntime(self);
        RuntimeGuard
    }

    fn add_task(&self, task: proto::Task) {
        self.tasks.lock().unwrap().push(task);
    }

    fn move_to_history(&self, key: &str) {
        self.history_keys.lock().unwrap().insert(key.to_owned());
    }
}

struct RuntimeGuard;

impl Drop for RuntimeGuard {
    // 无论测试正常结束还是 panic，都清除进程级 Runtime，避免污染后续用例。
    fn drop(&mut self) {
        ClearRuntime();
    }
}

/// 文件系统独占锁；用于串行化所有会替换全局 Runtime 的测试。
struct RuntimeTestLock {
    path: PathBuf,
    _file: File,
}

fn runtime_test_lock() -> RuntimeTestLock {
    let path = std::env::temp_dir().join("astersql-dxf-framework-handle-runtime.lock");
    loop {
        match create_dir(&path) {
            Ok(()) => {
                let file = File::create(path.join("owner")).expect("create runtime lock owner");
                return RuntimeTestLock { path, _file: file };
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                // 其他用例持锁时短暂让出，避免争用进程级 Runtime。
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("acquire runtime test lock: {error}"),
        }
    }
}

impl Drop for RuntimeTestLock {
    fn drop(&mut self) {
        let _ = remove_file(self.path.join("owner"));
        let _ = remove_dir(&self.path);
    }
}

struct MockObjectStorage {
    uri: String,
}

impl ObjectStorage for MockObjectStorage {
    fn uri(&self) -> &str {
        &self.uri
    }
}

impl Runtime for MockRuntime {
    // 以下实现只模拟 Handle 用例会触达的集成点；未使用的查询显式报错，防止误用被掩盖。
    fn get_cpu_count_of_node(&self, _ctx: &Context) -> Result<i32> {
        Ok(8)
    }

    fn get_task_by_key_with_history(
        &self,
        _ctx: &Context,
        key: &str,
    ) -> Result<Option<proto::Task>> {
        if let Some(manager) = &self.storage {
            return manager
                .GetTaskByKeyWithHistory((), key.to_owned())
                .map(Self::stored_task)
                .map(Some);
        }
        if self.history_lookup_not_found {
            return Err(storage::ErrTaskNotFound.into());
        }
        if self.history_keys.lock().unwrap().contains(key) {
            return Ok(Some(self.task(key, proto::TaskStateFailed, None)));
        }
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .iter()
            .find(|task| task.Key == key)
            .map(Self::clone_task))
    }

    fn create_task(
        &self,
        _ctx: &Context,
        key: &str,
        task_type: proto::TaskType,
        keyspace: &str,
        required_slots: i32,
        target_scope: &str,
        max_node_count: i32,
        extra_params: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64> {
        if let Some(manager) = &self.storage {
            return manager.CreateTask(
                (),
                key.to_owned(),
                task_type,
                keyspace.to_owned(),
                required_slots,
                target_scope.to_owned(),
                max_node_count,
                storage::proto::ExtraParams {
                    ManualRecovery: extra_params.ManualRecovery,
                    PauseOnKVDiskFull: extra_params.PauseOnKVDiskFull,
                    MaxRuntimeSlots: extra_params.MaxRuntimeSlots,
                    TargetSteps: extra_params.TargetSteps,
                    PrepareMode: extra_params.PrepareMode,
                },
                meta,
            );
        }
        let mut task = self.task(key, proto::TaskStateFailed, Some("unknown task type"));
        task.Type = task_type;
        task.Keyspace = keyspace.to_owned();
        task.RequiredSlots = required_slots;
        task.TargetScope = target_scope.to_owned();
        task.MaxNodeCount = max_node_count;
        task.ExtraParams = extra_params;
        task.Meta = meta;
        let id = task.ID;
        self.add_task(task);
        Ok(id)
    }

    fn get_task_by_id(&self, _ctx: &Context, id: i64) -> Result<proto::Task> {
        if let Some(manager) = &self.storage {
            return manager.GetTaskByID((), id).map(Self::stored_task);
        }
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|task| task.ID == id)
            .map(Self::clone_task)
            .ok_or_else(|| storage::Error::new("task not found"))
    }

    fn get_task_by_id_with_history(&self, ctx: &Context, id: i64) -> Result<proto::Task> {
        if let Some(manager) = &self.storage {
            return manager
                .GetTaskByIDWithHistory((), id)
                .map(Self::stored_task);
        }
        self.get_task_by_id(ctx, id)
    }

    fn get_task_base_by_id_with_history(&self, ctx: &Context, id: i64) -> Result<proto::TaskBase> {
        self.get_task_by_id(ctx, id).map(|task| task.TaskBase)
    }

    fn get_task_by_key(&self, _ctx: &Context, key: &str) -> Result<Option<proto::Task>> {
        if let Some(manager) = &self.storage {
            return manager
                .GetTaskByKey((), key.to_owned())
                .map(Self::stored_task)
                .map(Some);
        }
        if self.active_lookup_not_found {
            return Err(storage::ErrTaskNotFound.into());
        }
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .iter()
            .find(|task| task.Key == key)
            .map(Self::clone_task))
    }

    fn cancel_task(&self, _ctx: &Context, id: i64) -> Result<()> {
        if let Some(manager) = &self.storage {
            return manager.CancelTask((), id);
        }
        if let Some(task) = self
            .tasks
            .lock()
            .unwrap()
            .iter_mut()
            .find(|task| task.ID == id)
        {
            task.State = proto::TaskStateCancelling;
        }
        Ok(())
    }

    fn pause_task(&self, _ctx: &Context, _key: &str) -> Result<bool> {
        if let Some(manager) = &self.storage {
            return manager.PauseTask((), _key.to_owned());
        }
        Ok(true)
    }

    fn resume_task(&self, _ctx: &Context, _key: &str) -> Result<bool> {
        if let Some(manager) = &self.storage {
            return manager.ResumeTask((), _key.to_owned());
        }
        Ok(true)
    }

    fn get_task_bases_in_states(
        &self,
        _ctx: &Context,
        _states: &[proto::TaskState],
    ) -> Result<Vec<proto::TaskBase>> {
        Ok(Vec::new())
    }

    fn get_all_nodes(&self, _ctx: &Context) -> Result<Vec<proto::ManagedNode>> {
        Ok(vec![proto::ManagedNode {
            ID: ":4000".to_owned(),
            Role: String::new(),
            CPUCount: 8,
        }])
    }

    fn get_busy_nodes(&self, _ctx: &Context) -> Result<Vec<schstatus::Node>> {
        Ok(Vec::new())
    }

    fn owner_exec_id(&self, _ctx: &Context) -> Result<String> {
        Ok(":4000".to_owned())
    }

    fn get_active_task_summary(&self, _ctx: &Context) -> Result<storage::ActiveTaskSummary> {
        Err(storage::Error::new("unused in handle test"))
    }

    fn list_history_tasks(
        &self,
        _ctx: &Context,
        _page_size: i32,
        _page_token: i64,
        _keyspace: &str,
    ) -> Result<storage::HistoryTaskPage> {
        Err(storage::Error::new("unused in handle test"))
    }

    fn local_cpu_count(&self) -> i32 {
        8
    }

    fn update_pause_scale_in_flag(&self, _ctx: &Context, _flag: &schstatus::TTLFlag) -> Result<()> {
        Ok(())
    }

    fn get_pause_scale_in_flag(&self, _ctx: &Context) -> Result<Option<schstatus::TTLFlag>> {
        Ok(None)
    }

    fn get_schedule_tune_factors(
        &self,
        _ctx: &Context,
        _keyspace: &str,
    ) -> Result<Option<schstatus::TTLTuneFactors>> {
        Ok(None)
    }

    fn is_next_gen(&self) -> bool {
        self.next_gen
    }

    fn service_scope(&self) -> String {
        self.service_scope.clone()
    }

    fn cloud_storage_uri(&self) -> String {
        self.cloud_storage_uri.lock().unwrap().clone()
    }

    fn sem_enabled(&self) -> bool {
        self.sem_enabled
    }

    fn cluster_id(&self, _ctx: &Context) -> Option<u64> {
        self.cluster_id
    }

    fn new_object_store(
        &self,
        _ctx: &Context,
        uri: &str,
        _recording: Option<Arc<AccessStats>>,
    ) -> Result<Arc<dyn ObjectStorage>> {
        Ok(Arc::new(MockObjectStorage {
            uri: uri.to_owned(),
        }))
    }

    fn write_meter_data(
        &self,
        _ctx: &Context,
        timestamp: i64,
        key: &str,
        item: &MeterItem,
    ) -> Result<()> {
        self.meter_writes
            .lock()
            .unwrap()
            .push((timestamp, key.to_owned(), item.clone()));
        Ok(())
    }
}

#[test]
fn test_handle() {
    let _lock = runtime_test_lock();
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    storage::init();
    manager
        .InitMeta((), ":4000".into(), "test-scope".into())
        .unwrap();
    let runtime = Arc::new(MockRuntime {
        storage: Some(manager.clone()),
        service_scope: "test-scope".to_owned(),
        cluster_id: Some(1),
        ..Default::default()
    });
    let _guard = runtime.clone().install();
    let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctx = Context::from_cancellation_flag(cancellation.clone());
    let (done, timeout) = std::sync::mpsc::channel();
    let watchdog = thread::spawn(move || {
        if timeout.recv_timeout(Duration::from_secs(10)).is_err() {
            cancellation.store(true, Ordering::Release);
        }
    });

    // Drive the real schedule-loop body explicitly: the Domain owns no implicit
    // scheduler, and stopping this guard must precede the remaining Handle checks.
    struct SchedulerGuard(astersql_dxf_framework_scheduler::Manager);
    impl Drop for SchedulerGuard {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let scheduler = SchedulerGuard(astersql_dxf_framework_scheduler::Manager::new(
        Arc::new(astersql_dxf_framework_scheduler::StorageTaskManagerAdapter::new(manager.clone())),
        ":4000",
        None,
    ));
    scheduler.0.start().unwrap();
    // 未注册调度器时任务会进入失败终态，但提交参数仍应完整写入存储。
    let task = SubmitTask(
        &ctx,
        "1",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    )
    .unwrap();
    assert_eq!(
        manager.GetTaskByID((), task.ID).unwrap().State,
        proto::TaskStatePending
    );
    scheduler.0.tick().unwrap();
    let waited = WaitTask(&ctx, task.ID, proto::TaskBase::IsDone).unwrap();
    assert_eq!(waited.State, proto::TaskStateFailed);
    let history_lookup = runtime.get_task_by_id_with_history(&ctx, task.ID).unwrap();
    assert!(
        history_lookup
            .Error
            .as_deref()
            .unwrap()
            .contains("unknown task type")
    );
    let loaded = runtime.get_task_by_id(&ctx, task.ID).unwrap();
    assert_eq!(loaded.Key, "1");
    assert_eq!(loaded.Type, proto::TaskTypeExample);
    assert_eq!(loaded.Step, proto::StepInit);
    assert_eq!(loaded.RequiredSlots, 2);
    assert_eq!(loaded.Meta, proto::EmptyMeta);
    assert_eq!(loaded.State, proto::TaskStateFailed);
    assert!(
        loaded
            .Error
            .as_deref()
            .unwrap()
            .contains("unknown task type")
    );
    drop(scheduler);
    CancelTask(&ctx, "1").unwrap();

    // 活跃任务键必须唯一，重复提交不能绕过暂停、恢复等生命周期操作。
    let task = SubmitTask(
        &ctx,
        "2",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    )
    .unwrap();
    assert_eq!(task.Key, "2");
    assert_eq!(
        manager.GetTaskByID((), task.ID).unwrap().State,
        proto::TaskStatePending
    );
    let duplicate = SubmitTask(
        &ctx,
        "2",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    );
    assert!(duplicate.is_err());
    assert!(matches!(duplicate, Err(error) if error == storage::ErrTaskAlreadyExists));
    PauseTask(&ctx, "2").unwrap();
    assert_eq!(
        manager.GetTaskByID((), task.ID).unwrap().State,
        proto::TaskStatePausing
    );
    ResumeTask(&ctx, "2").unwrap();
    // Without a scheduler the pause remains in progress; ResumeTask is a no-op.
    assert_eq!(
        manager.GetTaskByID((), task.ID).unwrap().State,
        proto::TaskStatePausing
    );

    // 历史任务同样占用任务键，防止重放已经归档的任务。
    let history_task = SubmitTask(
        &ctx,
        "3",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    )
    .unwrap();
    manager
        .TransferTasks2History((), vec![manager.GetTaskByID((), history_task.ID).unwrap()])
        .unwrap();
    let duplicate = SubmitTask(
        &ctx,
        "3",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    );
    assert!(matches!(duplicate, Err(error) if error == storage::ErrTaskAlreadyExists));
    done.send(()).unwrap();
    watchdog.join().unwrap();
    domain.close();
}

#[test]
fn task_not_found_sentinels_match_go_submit_and_cancel_contracts() {
    let _lock = runtime_test_lock();
    let ctx = Context::background();

    // Go treats ErrTaskNotFound from the history lookup as proof that a new key is available.
    let runtime = Arc::new(MockRuntime {
        history_lookup_not_found: true,
        ..Default::default()
    });
    let _guard = runtime.install();
    let task = SubmitTask(
        &ctx,
        "new-task",
        proto::TaskTypeExample,
        "test",
        2,
        "",
        0,
        proto::EmptyMeta.to_vec(),
    )
    .expect("ErrTaskNotFound must not reject a new task");
    assert_eq!(task.Key, "new-task");

    // Go also makes cancellation idempotent when the active lookup returns the sentinel.
    let runtime = Arc::new(MockRuntime {
        active_lookup_not_found: true,
        ..Default::default()
    });
    let _guard = runtime.install();
    CancelTask(&ctx, "missing-task").expect("cancelling a missing task must succeed");
}

#[test]
fn test_run_with_retry() {
    let ctx = Context::background();

    // 可重试错误达到次数上限后，返回最后一次错误。
    let attempts = std::cell::Cell::new(0);
    let result = RunWithRetry(
        &ctx,
        3,
        &|_| Duration::ZERO,
        |_, _, _| {},
        |_| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            (true, Err(Error::new("mock error")))
        },
    );
    assert!(result.unwrap_err().to_string().contains("mock error"));
    assert_eq!(attempts.get(), 3);

    // 不可重试错误立即终止，即使允许的最大尝试次数很大。
    let result = RunWithRetry(
        &ctx,
        i32::MAX,
        &|_| Duration::ZERO,
        |_, _, _| {},
        |_| (false, Err(Error::new("mock error"))),
    );
    assert!(result.is_err());

    // 首次失败、第二次成功，验证重试后能返回成功。
    let attempts = std::cell::Cell::new(0);
    RunWithRetry(
        &ctx,
        i32::MAX,
        &|_| Duration::ZERO,
        |_, _, _| {},
        |_| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            if attempt == 1 {
                (true, Err(Error::new("mock error")))
            } else {
                (false, Ok(()))
            }
        },
    )
    .unwrap();
    assert_eq!(attempts.get(), 2);

    // 等待退避期间应响应上下文取消，而不是继续下一次尝试。
    let cancelled = Context::background();
    cancelled.cancel();
    let result = RunWithRetry(
        &cancelled,
        i32::MAX,
        &|_| Duration::from_secs(1),
        |_, _, _| {},
        |_| (true, Err(Error::new("mock error"))),
    );
    assert!(result.unwrap_err().to_string().contains("context canceled"));
}

#[test]
fn test_get_target_scope_and_default_region_split_config() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(MockRuntime {
        service_scope: "test-scope".to_owned(),
        ..Default::default()
    });
    let _guard = runtime.install();
    assert_eq!(GetTargetScope().unwrap(), "test-scope");
    assert_eq!(
        GetDefaultRegionSplitConfig().unwrap(),
        (DEF_REGION_SPLIT_SIZE, DEF_REGION_SPLIT_KEYS)
    );

    // Next-gen 使用固定服务域，并采用更大的默认 Region 切分阈值。
    let runtime = Arc::new(MockRuntime {
        next_gen: true,
        ..Default::default()
    });
    let _guard = runtime.install();
    assert_eq!(GetTargetScope().unwrap(), NEXT_GEN_TARGET_SCOPE);
    assert_eq!(
        GetDefaultRegionSplitConfig().unwrap(),
        (1 << 30, 102_400_000)
    );
}

#[test]
fn test_handles_preserve_go_cloud_storage_prefix_matrix() {
    let _lock = runtime_test_lock();
    // 同时覆盖 SEM 开关、根路径和带子路径 URI，确保前缀规则与 Go 实现一致。
    for sem_enabled in [true, false] {
        for (input, sem_out, no_sem_out) in [
            ("", "", ""),
            ("s3://bucket", "s3://bucket/dxf/", "s3://bucket/dxf/"),
            ("s3://bucket/", "s3://bucket/dxf/", "s3://bucket/dxf/"),
            (
                "s3://bucket/path",
                "s3://bucket/path/dxf/",
                "s3://bucket/path/dxf/1/",
            ),
            (
                "s3://bucket/path/",
                "s3://bucket/path/dxf/",
                "s3://bucket/path/dxf/1/",
            ),
        ] {
            let runtime = Arc::new(MockRuntime {
                cloud_storage_uri: Mutex::new(input.to_owned()),
                sem_enabled,
                cluster_id: Some(1),
                ..Default::default()
            });
            let _guard = runtime.install();
            let expected = if sem_enabled { sem_out } else { no_sem_out };
            assert_eq!(
                GetCloudStorageURI(&Context::background()).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn test_metering_and_object_store_hooks() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(MockRuntime::default());
    let _guard = runtime.clone().install();
    let (stats, store) =
        NewObjStoreWithRecording(&Context::background(), "s3://bucket/dxf/").unwrap();
    assert_eq!(store.uri(), "s3://bucket/dxf/");
    stats.record_request();
    assert_eq!(stats.requests(), 1);

    // 计量时长取任务状态更新时间与开始时间之差，写入键包含任务类型和 ID。
    let task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: 42,
            Key: String::new(),
            Type: proto::TaskTypeExample,
            State: proto::TaskStateSucceed,
            Step: proto::StepDone,
            Priority: proto::NormalPriority,
            RequiredSlots: 2,
            TargetScope: String::new(),
            CreateTime: UNIX_EPOCH + Duration::from_secs(60),
            MaxNodeCount: 3,
            ExtraParams: proto::ExtraParams::default(),
            Keyspace: "test".to_owned(),
        },
        SchedulerID: String::new(),
        StartTime: UNIX_EPOCH + Duration::from_secs(60),
        StateUpdateTime: UNIX_EPOCH + Duration::from_secs(180),
        Meta: proto::EmptyMeta.to_vec(),
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: proto::TaskStatePending,
            Modifications: Vec::new(),
        },
    };
    let item = SendRowAndSizeMeterData(&Context::background(), &task, 10, 20, 30).unwrap();
    assert_eq!(item.get("row_count"), Some(&MeterValue::Integer(10)));
    assert_eq!(item.get("data_kv_bytes"), Some(&MeterValue::Integer(20)));
    assert_eq!(
        item.get("duration_seconds"),
        Some(&MeterValue::Integer(120))
    );
    let writes = runtime.meter_writes.lock().unwrap();
    assert_eq!(writes[0].0, 180);
    assert_eq!(writes[0].1, "Example_42");
    drop(writes);

    // Go 的 time.Sub 会保留负持续时间；时钟异常时不能静默饱和为零。
    let mut reversed = MockRuntime::clone_task(&task);
    reversed.CreateTime = UNIX_EPOCH + Duration::from_secs(300);
    let item = SendRowAndSizeMeterData(&Context::background(), &reversed, 10, 20, 30).unwrap();
    assert_eq!(
        item.get("duration_seconds"),
        Some(&MeterValue::Integer(-120))
    );
}
#[test]
fn external_cancellation_flag_interrupts_handle_wait() {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let context = Context::from_cancellation_flag(flag.clone());
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        flag.store(true, std::sync::atomic::Ordering::Release);
    });
    let started = std::time::Instant::now();
    assert!(context.wait(std::time::Duration::from_secs(1)).is_err());
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    canceller.join().unwrap();
}
