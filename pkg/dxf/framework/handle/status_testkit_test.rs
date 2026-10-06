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

// 调度状态接口的轻量测试夹具与端到端行为测试。
//
// 这里用内存中的 `Runtime` 替代数据库和节点发现组件，集中验证状态汇总、
// Owner 节点合并、TTL 配置过期回退，以及任务变更通知的合并语义。

use super::*;
use std::fs::{File, create_dir, remove_dir, remove_file};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

/// 只实现状态查询所需能力的内存运行时，其余接口显式返回未使用错误。
struct StatusRuntime {
    tasks: Mutex<Vec<(proto::TaskState, i32, i32, proto::TaskType, proto::Step)>>,
    nodes: Mutex<Vec<proto::ManagedNode>>,
    busy_nodes: Mutex<Vec<schstatus::Node>>,
    owner: String,
    local_cpu: i32,
    flag: Mutex<Option<schstatus::TTLFlag>>,
    tune_factors: Mutex<Option<schstatus::TTLTuneFactors>>,
}

impl Default for StatusRuntime {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
            nodes: Mutex::new(Vec::new()),
            busy_nodes: Mutex::new(Vec::new()),
            owner: ":4000".to_owned(),
            local_cpu: 16,
            flag: Mutex::new(None),
            tune_factors: Mutex::new(None),
        }
    }
}

impl StatusRuntime {
    /// 安装进程级运行时，并由返回的守卫负责恢复为空状态。
    fn install(self: Arc<Self>) -> RuntimeGuard {
        ClearRuntime();
        InstallRuntime(self);
        RuntimeGuard
    }

    /// 将紧凑的测试输入展开成状态计算所需的完整任务记录。
    fn task(
        state: proto::TaskState,
        required_slots: i32,
        max_node_count: i32,
        task_type: proto::TaskType,
        step: proto::Step,
    ) -> proto::TaskBase {
        proto::TaskBase {
            ID: 0,
            Key: String::new(),
            Type: task_type,
            State: state,
            Step: step,
            Priority: proto::NormalPriority,
            RequiredSlots: required_slots,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: max_node_count,
            ExtraParams: proto::ExtraParams::default(),
            Keyspace: String::new(),
        }
    }
}

/// 防止测试结束后遗留全局运行时，污染同一进程中的后续用例。
struct RuntimeGuard;

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        ClearRuntime();
    }
}

/// 串行化会替换全局运行时的测试，避免并发测试互相覆盖夹具。
struct RuntimeTestLock {
    path: PathBuf,
    _file: File,
}

fn runtime_test_lock() -> RuntimeTestLock {
    let path = std::env::temp_dir().join("astersql-dxf-framework-handle-runtime.lock");
    // 创建目录具有原子性；目录已存在时等待当前持有者的守卫完成清理。
    loop {
        match create_dir(&path) {
            Ok(()) => {
                let file = File::create(path.join("owner")).expect("create runtime lock owner");
                return RuntimeTestLock { path, _file: file };
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
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

// 状态接口只读取任务、节点、Owner 与 TTL 配置；其他方法保留失败实现，
// 以便意外扩大的依赖能立即暴露，而不是被无意义的默认值掩盖。
impl Runtime for StatusRuntime {
    fn get_cpu_count_of_node(&self, _ctx: &Context) -> Result<i32> {
        Ok(self.local_cpu)
    }

    fn get_task_by_key_with_history(
        &self,
        _ctx: &Context,
        _key: &str,
    ) -> Result<Option<proto::Task>> {
        Err(Error::new("unused in status testkit"))
    }

    fn create_task(
        &self,
        _ctx: &Context,
        _key: &str,
        _task_type: proto::TaskType,
        _keyspace: &str,
        _required_slots: i32,
        _target_scope: &str,
        _max_node_count: i32,
        _extra_params: proto::ExtraParams,
        _meta: Vec<u8>,
    ) -> Result<i64> {
        Err(Error::new("unused in status testkit"))
    }

    fn get_task_by_id(&self, _ctx: &Context, _id: i64) -> Result<proto::Task> {
        Err(Error::new("unused in status testkit"))
    }

    fn get_task_by_id_with_history(&self, _ctx: &Context, _id: i64) -> Result<proto::Task> {
        Err(Error::new("unused in status testkit"))
    }

    fn get_task_base_by_id_with_history(
        &self,
        _ctx: &Context,
        _id: i64,
    ) -> Result<proto::TaskBase> {
        Err(Error::new("unused in status testkit"))
    }

    fn get_task_by_key(&self, _ctx: &Context, _key: &str) -> Result<Option<proto::Task>> {
        Err(Error::new("unused in status testkit"))
    }

    fn cancel_task(&self, _ctx: &Context, _id: i64) -> Result<()> {
        Err(Error::new("unused in status testkit"))
    }

    fn pause_task(&self, _ctx: &Context, _key: &str) -> Result<bool> {
        Err(Error::new("unused in status testkit"))
    }

    fn resume_task(&self, _ctx: &Context, _key: &str) -> Result<bool> {
        Err(Error::new("unused in status testkit"))
    }

    fn get_task_bases_in_states(
        &self,
        _ctx: &Context,
        states: &[proto::TaskState],
    ) -> Result<Vec<proto::TaskBase>> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|(state, ..)| states.contains(state))
            .map(|&(state, slots, max_nodes, task_type, step)| {
                Self::task(state, slots, max_nodes, task_type, step)
            })
            .collect())
    }

    fn get_all_nodes(&self, _ctx: &Context) -> Result<Vec<proto::ManagedNode>> {
        Ok(self
            .nodes
            .lock()
            .unwrap()
            .iter()
            .map(|node| proto::ManagedNode {
                ID: node.ID.clone(),
                Role: node.Role.clone(),
                CPUCount: node.CPUCount,
            })
            .collect())
    }

    fn get_busy_nodes(&self, _ctx: &Context) -> Result<Vec<schstatus::Node>> {
        Ok(self.busy_nodes.lock().unwrap().clone())
    }

    fn owner_exec_id(&self, _ctx: &Context) -> Result<String> {
        Ok(self.owner.clone())
    }

    fn get_active_task_summary(&self, _ctx: &Context) -> Result<storage::ActiveTaskSummary> {
        Err(Error::new("unused in status testkit"))
    }

    fn list_history_tasks(
        &self,
        _ctx: &Context,
        _page_size: i32,
        _page_token: i64,
        _keyspace: &str,
    ) -> Result<storage::HistoryTaskPage> {
        Err(Error::new("unused in status testkit"))
    }

    fn local_cpu_count(&self) -> i32 {
        self.local_cpu
    }

    fn update_pause_scale_in_flag(&self, _ctx: &Context, flag: &schstatus::TTLFlag) -> Result<()> {
        *self.flag.lock().unwrap() = Some(flag.clone());
        Ok(())
    }

    fn get_pause_scale_in_flag(&self, _ctx: &Context) -> Result<Option<schstatus::TTLFlag>> {
        Ok(self.flag.lock().unwrap().clone())
    }

    fn get_schedule_tune_factors(
        &self,
        _ctx: &Context,
        _keyspace: &str,
    ) -> Result<Option<schstatus::TTLTuneFactors>> {
        Ok(self.tune_factors.lock().unwrap().clone())
    }

    fn is_next_gen(&self) -> bool {
        false
    }

    fn service_scope(&self) -> String {
        String::new()
    }

    fn cloud_storage_uri(&self) -> String {
        String::new()
    }

    fn sem_enabled(&self) -> bool {
        true
    }

    fn cluster_id(&self, _ctx: &Context) -> Option<u64> {
        None
    }

    fn new_object_store(
        &self,
        _ctx: &Context,
        _uri: &str,
        _recording: Option<Arc<AccessStats>>,
    ) -> Result<Arc<dyn ObjectStorage>> {
        Err(Error::new("unused in status testkit"))
    }

    fn write_meter_data(
        &self,
        _ctx: &Context,
        _timestamp: i64,
        _key: &str,
        _item: &MeterItem,
    ) -> Result<()> {
        Err(Error::new("unused in status testkit"))
    }
}

#[test]
/// 验证空任务队列仍至少需要一个 Worker，并把当前 Owner 计入忙碌节点。
fn test_get_schedule_status() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(StatusRuntime {
        nodes: Mutex::new(vec![proto::ManagedNode {
            ID: ":4000".to_owned(),
            Role: String::new(),
            CPUCount: 16,
        }]),
        ..Default::default()
    });
    let _guard = runtime.install();
    let status = GetScheduleStatus(&Context::background()).unwrap();
    assert_eq!(status.Version, schstatus::Version1);
    assert_eq!(status.TaskQueue.ScheduledCount, 0);
    assert_eq!(status.TiDBWorker.CPUCount, 16);
    assert_eq!(status.TiDBWorker.RequiredCount, 1);
    assert_eq!(status.TiDBWorker.CurrentCount, 1);
    assert!(
        status.TiDBWorker.BusyNodes
            == vec![schstatus::Node {
                ID: ":4000".to_owned(),
                IsOwner: true
            }]
    );
    assert_eq!(status.TiKVWorker.RequiredCount, 1);
    assert!(status.Flags.is_empty());
}

#[test]
/// 验证无节点时 CPU 回退到本机值，以及 Owner 与执行子任务节点的合并规则。
fn test_node_info_and_busy_nodes() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(StatusRuntime::default());
    let _guard = runtime.clone().install();
    let ctx = Context::background();
    assert_eq!(GetNodesInfo(&ctx).unwrap(), (0, 16));
    runtime.nodes.lock().unwrap().push(proto::ManagedNode {
        ID: "node1".to_owned(),
        Role: String::new(),
        CPUCount: 16,
    });
    assert_eq!(GetNodesInfo(&ctx).unwrap(), (1, 16));
    assert!(
        GetBusyNodes(&ctx).unwrap()
            == vec![schstatus::Node {
                ID: ":4000".to_owned(),
                IsOwner: true
            }]
    );
    runtime.busy_nodes.lock().unwrap().push(schstatus::Node {
        ID: ":4001".to_owned(),
        IsOwner: false,
    });
    let mut busy = GetBusyNodes(&ctx).unwrap();
    busy.sort_by(|left, right| left.ID.cmp(&right.ID));
    assert!(
        busy == vec![
            schstatus::Node {
                ID: ":4000".to_owned(),
                IsOwner: true
            },
            schstatus::Node {
                ID: ":4001".to_owned(),
                IsOwner: false
            },
        ]
    );
}

#[test]
/// 节点列表 API 原样返回按存储顺序读取的 host、role 与 CPU，并保留空列表。
fn test_list_managed_nodes() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(StatusRuntime::default());
    let _guard = runtime.clone().install();
    let ctx = Context::background();

    assert!(ListManagedNodes(&ctx).unwrap().is_empty());
    runtime.nodes.lock().unwrap().extend([
        proto::ManagedNode {
            ID: ":4001".to_owned(),
            Role: String::new(),
            CPUCount: 4,
        },
        proto::ManagedNode {
            ID: ":4002".to_owned(),
            Role: "background".to_owned(),
            CPUCount: 8,
        },
    ]);

    let nodes = ListManagedNodes(&ctx).unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].ID, ":4001");
    assert_eq!(nodes[0].Role, "");
    assert_eq!(nodes[0].CPUCount, 4);
    assert_eq!(nodes[1].ID, ":4002");
    assert_eq!(nodes[1].Role, "background");
    assert_eq!(nodes[1].CPUCount, 8);
}

#[test]
/// 暂停缩容标志仅在启用且 TTL 未过期时对外可见。
fn test_schedule_flag() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(StatusRuntime::default());
    let _guard = runtime.clone().install();
    let ctx = Context::background();
    assert!(GetScheduleFlags(&ctx).unwrap().is_empty());
    let flag = schstatus::TTLFlag {
        Enabled: true,
        TTLInfo: schstatus::TTLInfo {
            TTL: Duration::from_secs(3600),
            ExpireTime: SystemTime::now() + Duration::from_secs(3600),
        },
    };
    UpdatePauseScaleInFlag(&ctx, &flag).unwrap();
    let flags = GetScheduleFlags(&ctx).unwrap();
    assert!(flags.get(schstatus::PauseScaleInFlag) == Some(&flag));
    let expired = schstatus::TTLFlag {
        TTLInfo: schstatus::TTLInfo {
            ExpireTime: SystemTime::now() - Duration::from_secs(3600),
            ..flag.TTLInfo.clone()
        },
        ..flag.clone()
    };
    UpdatePauseScaleInFlag(&ctx, &expired).unwrap();
    assert!(GetScheduleFlags(&ctx).unwrap().is_empty());
    UpdatePauseScaleInFlag(&ctx, &schstatus::TTLFlag::default()).unwrap();
    assert!(GetScheduleFlags(&ctx).unwrap().is_empty());
}

#[test]
/// 调优因子缺失或过期时回退默认值，仍在有效期内时返回持久化配置。
fn test_get_schedule_tune_factors() {
    let _lock = runtime_test_lock();
    let runtime = Arc::new(StatusRuntime::default());
    let _guard = runtime.clone().install();
    let ctx = Context::background();
    assert!(GetScheduleTuneFactors(&ctx, "test").unwrap() == schstatus::GetDefaultTuneFactors());
    runtime
        .tune_factors
        .lock()
        .unwrap()
        .replace(schstatus::TTLTuneFactors {
            TTLInfo: schstatus::TTLInfo {
                TTL: Duration::from_secs(3600),
                ExpireTime: SystemTime::now() + Duration::from_secs(3600),
            },
            TuneFactors: schstatus::TuneFactors { AmplifyFactor: 1.5 },
        });
    assert_eq!(
        GetScheduleTuneFactors(&ctx, "test").unwrap().AmplifyFactor,
        1.5
    );
    runtime
        .tune_factors
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .TTLInfo
        .ExpireTime = SystemTime::now() - Duration::from_secs(3600);
    assert!(GetScheduleTuneFactors(&ctx, "test").unwrap() == schstatus::GetDefaultTuneFactors());
}

#[test]
/// 连续通知被容量为一的信号合并，同时锁定采样日志器的 Go 兼容默认值。
fn task_change_notification_is_coalesced_and_logger_defaults_match_go() {
    while TryRecvTaskChange() {}
    NotifyTaskChange();
    NotifyTaskChange();
    assert!(TryRecvTaskChange());
    assert!(!TryRecvTaskChange());
    let logger = NewSampleErrVerboseLogger(vec![LogField {
        key: "task".to_owned(),
        value: "42".to_owned(),
    }]);
    assert_eq!(logger.category, DXF_LOG_CATEGORY);
    assert_eq!(logger.tick, SAMPLE_LOG_TICK);
    assert_eq!(logger.first, SAMPLE_LOG_FIRST);
    assert_eq!(logger.fields[0].value, "42");
}
