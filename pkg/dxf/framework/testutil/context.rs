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

// DXF 集成测试上下文：错误类型、任务/子任务模型与模拟集群。
//
// `TestDXFContext` 在内存中维护若干 TiDB 节点，可扩缩容、切换 owner
//（owner：负责调度的主节点），并缩短轮询间隔以加速测试。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const NODE_ID_POOL_CAPACITY: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
/// DXF 测试路径使用的简单错误包装。
pub struct DxfError(pub String);

impl fmt::Display for DxfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DxfError {}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
/// 任务步骤编号。
pub struct Step(pub i64);

/// 初始步骤。
pub const STEP_INIT: Step = Step(-1);
/// 第一步。
pub const STEP_ONE: Step = Step(1);
/// 第二步。
pub const STEP_TWO: Step = Step(2);
/// 终止步骤哨兵值。
pub const STEP_DONE: Step = Step(-2);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 任务生命周期状态。
pub enum TaskState {
    #[default]
    Pending,
    Running,
    Paused,
    Cancelling,
    Pausing,
    Reverting,
    Reverted,
    AwaitingResolution,
    Resuming,
    Modifying,
    Failed,
    Succeed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 子任务生命周期状态。
pub enum SubtaskState {
    #[default]
    Pending,
    Running,
    Paused,
    Canceled,
    Failed,
    Succeed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 任务基础字段（不含大块 meta）。
pub struct TaskBase {
    pub id: i64,
    pub key: String,
    pub state: TaskState,
    pub step: Step,
}

impl TaskBase {
    /// 是否已终态（Failed 或 Succeed）。
    pub fn is_done(&self) -> bool {
        matches!(
            self.state,
            TaskState::Failed | TaskState::Reverted | TaskState::Succeed
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 完整任务：基础字段 + 类型/并发/作用域/元数据。
pub struct Task {
    pub base: TaskBase,
    pub task_type: String,
    pub concurrency: usize,
    pub target_scope: String,
    pub meta: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 子任务记录。
pub struct Subtask {
    pub id: i64,
    pub task_id: i64,
    pub step: Step,
    pub exec_id: String,
    pub meta: Vec<u8>,
    pub state: SubtaskState,
    pub task_type: String,
    pub concurrency: usize,
    pub summary: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 节点可用资源配额（CPU/内存/磁盘）。
pub struct NodeResource {
    pub cpu_count: usize,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

impl NodeResource {
    /// 按 CPU 数构造默认内存 32GiB、磁盘 100GiB 的资源描述。
    pub fn for_cpu(cpu_count: usize) -> Self {
        Self {
            cpu_count,
            memory_bytes: 32 * 1024 * 1024 * 1024,
            disk_bytes: 100 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// DXF 各轮询回路的检查间隔。
pub struct CheckIntervals {
    pub scheduler_running: Duration,
    pub scheduler_finished: Duration,
    pub cleanup: Duration,
    pub task: Duration,
    pub subtask: Duration,
    pub max_subtask: Duration,
    pub detect_modification: Duration,
}

/// 真实 DXF 运行时在测试中的可替换接口。
pub trait DxfRuntime: Send + Sync {
    /// 设置节点资源，返回旧值。
    fn set_node_resource(&self, resource: NodeResource) -> Result<NodeResource, DxfError>;
    /// 在指定节点启动任务执行器。
    fn start_executor(&self, node_id: &str, resource: NodeResource) -> Result<(), DxfError>;
    /// 停止节点上的执行器。
    fn stop_executor(&self, node_id: &str) -> Result<(), DxfError>;
    /// 取消节点执行器（异步关闭前发信号）。
    fn cancel_executor(&self, node_id: &str) -> Result<(), DxfError>;
    /// 在节点上启动调度器（通常仅 owner）。
    fn start_scheduler(&self, node_id: &str, resource: NodeResource) -> Result<(), DxfError>;
    /// 停止调度器。
    fn stop_scheduler(&self, node_id: &str) -> Result<(), DxfError>;
    /// 取消调度器。
    fn cancel_scheduler(&self, node_id: &str) -> Result<(), DxfError>;
    /// 更新存活执行器节点 ID 列表。
    fn update_live_executor_ids(&self, node_ids: &[String]) -> Result<(), DxfError>;
    /// 设置轮询间隔，返回旧值。
    fn set_check_intervals(&self, intervals: CheckIntervals) -> Result<CheckIntervals, DxfError>;
}

/// 缩短检查间隔的 RAII 守卫，Drop 时恢复。
pub struct CheckIntervalGuard {
    runtime: Arc<dyn DxfRuntime>,
    previous: Option<CheckIntervals>,
}

impl Drop for CheckIntervalGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            let _ = self.runtime.set_check_intervals(previous);
        }
    }
}

#[allow(non_snake_case)]
/// 将各检查间隔压到毫秒级，加速测试收敛。
pub fn ReduceCheckInterval(runtime: Arc<dyn DxfRuntime>) -> Result<CheckIntervalGuard, DxfError> {
    let previous = runtime.set_check_intervals(CheckIntervals {
        scheduler_running: Duration::from_millis(100),
        scheduler_finished: Duration::from_millis(100),
        cleanup: Duration::from_millis(200),
        task: Duration::from_millis(10),
        subtask: Duration::from_millis(10),
        max_subtask: Duration::from_millis(10),
        detect_modification: Duration::from_millis(10),
    })?;
    Ok(CheckIntervalGuard {
        runtime,
        previous: Some(previous),
    })
}

#[derive(Default)]
/// 跨子任务执行的观测状态（已跑子任务集合、调用计数）。
pub struct TestContext {
    subtasks_has_run: RwLock<HashMap<(i64, Step), HashSet<i64>>>,
    call_time: AtomicU64,
}

impl TestContext {
    #[allow(non_snake_case)]
    /// 记录某个子任务已被执行。
    pub fn CollectSubtask(&self, subtask: &Subtask) {
        let mut collected = self.subtasks_has_run.write().unwrap();
        collected
            .entry((subtask.task_id, subtask.step))
            .or_default()
            .insert(subtask.id);
    }

    #[allow(non_snake_case)]
    /// 指定任务+步骤下已收集的子任务数。
    pub fn CollectedSubtaskCnt(&self, task_id: i64, step: Step) -> usize {
        self.subtasks_has_run
            .read()
            .unwrap()
            .get(&(task_id, step))
            .map_or(0, HashSet::len)
    }

    /// 单调递增的调用序号（用于脚本化首次失败等）。
    pub fn next_call_time(&self) -> u64 {
        self.call_time.fetch_add(1, Ordering::SeqCst)
    }
}

#[derive(Clone, Debug)]
/// 模拟集群中的一个 TiDB 节点。
struct TidbNode {
    id: String,
    owner: bool,
}

#[derive(Default)]
/// 集群节点列表、owner 集合与可回收 ID。
struct ClusterState {
    node_indices: HashMap<String, usize>,
    owner_ids: HashSet<String>,
    nodes: Vec<TidbNode>,
    recycled_ids: VecDeque<String>,
}

/// `TestDXFContext` 的内部共享状态。
struct TestDxfInner {
    runtime: Arc<dyn DxfRuntime>,
    state: Mutex<ClusterState>,
    joins: Mutex<Vec<JoinHandle<Result<(), DxfError>>>>,
    interval_guard: Mutex<Option<CheckIntervalGuard>>,
    original_resource: Mutex<Option<NodeResource>>,
    id_allocator: AtomicI32,
    election_counter: AtomicU64,
    mock_cpu_num: usize,
    /// 对外暴露的测试观测上下文。
    pub test_context: Arc<TestContext>,
}

#[derive(Clone)]
/// 可克隆的 DXF 测试集群句柄。
pub struct TestDXFContext {
    inner: Arc<TestDxfInner>,
}

impl TestDXFContext {
    /// 设置资源、可选缩短间隔，并按 node_num 扩容后选举 owner。
    fn new(
        runtime: Arc<dyn DxfRuntime>,
        node_num: usize,
        cpu_count: usize,
        reduce_check_interval: bool,
    ) -> Result<Self, DxfError> {
        let previous_resource = runtime.set_node_resource(NodeResource::for_cpu(cpu_count))?;
        let interval_guard = if reduce_check_interval {
            Some(ReduceCheckInterval(runtime.clone())?)
        } else {
            None
        };
        let context = Self {
            inner: Arc::new(TestDxfInner {
                runtime,
                state: Mutex::new(ClusterState::default()),
                joins: Mutex::new(Vec::new()),
                interval_guard: Mutex::new(interval_guard),
                original_resource: Mutex::new(Some(previous_resource)),
                id_allocator: AtomicI32::new(0),
                election_counter: AtomicU64::new(0),
                mock_cpu_num: cpu_count,
                test_context: Arc::new(TestContext::default()),
            }),
        };
        for _ in 0..node_num {
            let id = context.get_node_id();
            context.ScaleOutBy(id, false)?;
        }
        context.elect_if_needed()?;
        Ok(context)
    }

    /// 取得共享 TestContext。
    pub fn test_context(&self) -> Arc<TestContext> {
        self.inner.test_context.clone()
    }

    /// 分配节点 ID：优先复用回收队列，否则从 :4000 递增。
    fn get_node_id(&self) -> String {
        if let Some(id) = self.inner.state.lock().unwrap().recycled_ids.pop_front() {
            return id;
        }
        let number = self.inner.id_allocator.fetch_add(1, Ordering::SeqCst) + 4000;
        format!(":{number}")
    }

    /// Go 使用容量为 100 的非阻塞 channel；池满时丢弃归还的 ID。
    fn recycle_node_id(state: &mut ClusterState, id: String) {
        if state.recycled_ids.len() < NODE_ID_POOL_CAPACITY {
            state.recycled_ids.push_back(id);
        }
    }

    /// 把当前节点 ID 同步到运行时存活列表。
    fn update_live_ids(&self) -> Result<(), DxfError> {
        let ids = self
            .inner
            .state
            .lock()
            .unwrap()
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        self.inner.runtime.update_live_executor_ids(&ids)
    }

    #[allow(non_snake_case)]
    /// 扩容指定数量节点，并在无 owner 时选举。
    pub fn ScaleOut(&self, node_num: usize) -> Result<(), DxfError> {
        for _ in 0..node_num {
            self.ScaleOutBy(self.get_node_id(), false)?;
        }
        self.elect_if_needed()
    }

    #[allow(non_snake_case)]
    /// 按给定 ID 加入节点；`owner` 为真时同时启动调度器。
    pub fn ScaleOutBy(&self, id: String, owner: bool) -> Result<(), DxfError> {
        let resource = NodeResource::for_cpu(self.inner.mock_cpu_num);
        self.inner.runtime.start_executor(&id, resource)?;
        if owner {
            if let Err(error) = self.inner.runtime.start_scheduler(&id, resource) {
                let _ = self.inner.runtime.stop_executor(&id);
                return Err(error);
            }
        }
        {
            let mut state = self.inner.state.lock().unwrap();
            // 节点已存在：回滚刚启动的 runtime 组件并报错。
            if state.node_indices.contains_key(&id) {
                drop(state);
                if owner {
                    let _ = self.inner.runtime.stop_scheduler(&id);
                }
                let _ = self.inner.runtime.stop_executor(&id);
                return Err(DxfError(format!("node {id} already exists")));
            }
            let index = state.nodes.len();
            state.node_indices.insert(id.clone(), index);
            if owner {
                state.owner_ids.insert(id.clone());
            }
            state.nodes.push(TidbNode {
                id: id.clone(),
                owner,
            });
        }
        if let Err(error) = self.update_live_ids() {
            let removed = {
                let mut state = self.inner.state.lock().unwrap();
                let index = state.node_indices.remove(&id);
                index.map(|index| {
                    state.nodes.remove(index);
                    state.node_indices = state
                        .nodes
                        .iter()
                        .enumerate()
                        .map(|(index, node)| (node.id.clone(), index))
                        .collect();
                    state.owner_ids.remove(&id);
                    Self::recycle_node_id(&mut state, id.clone());
                })
            };
            if removed.is_some() {
                if owner {
                    let _ = self.inner.runtime.stop_scheduler(&id);
                }
                let _ = self.inner.runtime.stop_executor(&id);
            }
            return Err(error);
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 从尾部缩容指定数量节点。
    pub fn ScaleIn(&self, node_num: usize) -> Result<(), DxfError> {
        for _ in 0..node_num {
            let id = self
                .inner
                .state
                .lock()
                .unwrap()
                .nodes
                .last()
                .map(|node| node.id.clone());
            let Some(id) = id else { break };
            self.ScaleInBy(&id)?;
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 移除指定节点，停止其执行器/调度器并触发补选。
    pub fn ScaleInBy(&self, id: &str) -> Result<(), DxfError> {
        let removed = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(index) = state.node_indices.get(id).copied() else {
                return Ok(());
            };
            let node = state.nodes.remove(index);
            state.owner_ids.remove(id);
            state.node_indices = state
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id.clone(), index))
                .collect();
            Self::recycle_node_id(&mut state, id.to_owned());
            node
        };
        self.update_live_ids()?;
        let executor_result = self.inner.runtime.stop_executor(&removed.id);
        let scheduler_result = if removed.owner {
            self.inner.runtime.stop_scheduler(&removed.id)
        } else {
            Ok(())
        };
        executor_result.and(scheduler_result)?;
        self.elect_if_needed()
    }

    /// 若集群非空且无 owner，则轮询选举一个节点启动调度器。
    fn elect_if_needed(&self) -> Result<(), DxfError> {
        let selected = {
            let state = self.inner.state.lock().unwrap();
            if state.nodes.is_empty() || !state.owner_ids.is_empty() {
                return Ok(());
            }
            let next = self.inner.election_counter.fetch_add(1, Ordering::SeqCst) as usize;
            state.nodes[next % state.nodes.len()].id.clone()
        };
        self.inner
            .runtime
            .start_scheduler(&selected, NodeResource::for_cpu(self.inner.mock_cpu_num))?;
        let mut state = self.inner.state.lock().unwrap();
        if let Some(index) = state.node_indices.get(&selected).copied() {
            state.nodes[index].owner = true;
            state.owner_ids.insert(selected);
            Ok(())
        } else {
            drop(state);
            self.inner.runtime.stop_scheduler(&selected)
        }
    }

    #[allow(non_snake_case)]
    /// 停止全部现有 owner 调度器并重新选举。
    pub fn ChangeOwner(&self) -> Result<(), DxfError> {
        let owners = {
            let mut state = self.inner.state.lock().unwrap();
            let owners = state.owner_ids.drain().collect::<Vec<_>>();
            for node in &mut state.nodes {
                node.owner = false;
            }
            owners
        };
        for owner in owners {
            self.inner.runtime.stop_scheduler(&owner)?;
        }
        self.elect_if_needed()
    }

    #[allow(non_snake_case)]
    /// 在后台线程异步切换 owner。
    pub fn AsyncChangeOwner(&self) {
        let context = self.clone();
        self.inner
            .joins
            .lock()
            .unwrap()
            .push(thread::spawn(move || context.ChangeOwner()));
    }

    #[allow(non_snake_case)]
    /// 先发取消信号，再异步 ScaleInBy 真正下线节点。
    pub fn AsyncShutdown(&self, id: String) -> Result<(), DxfError> {
        let owner = {
            let state = self.inner.state.lock().unwrap();
            state
                .node_indices
                .get(&id)
                .map(|index| state.nodes[*index].owner)
        };
        // 取消 runtime 时不要持有集群锁，避免死锁。
        // Avoid holding the cluster lock while the runtime cancels workers.
        let Some(owner) = owner else {
            return Ok(());
        };
        self.inner.runtime.cancel_executor(&id)?;
        if owner {
            self.inner.runtime.cancel_scheduler(&id)?;
        }
        let context = self.clone();
        self.inner
            .joins
            .lock()
            .unwrap()
            .push(thread::spawn(move || context.ScaleInBy(&id)));
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 从环形偏移处取最多 `limit` 个节点 ID。
    pub fn GetRandNodeIDs(&self, limit: usize) -> HashSet<String> {
        let state = self.inner.state.lock().unwrap();
        let len = state.nodes.len();
        if len == 0 {
            return HashSet::new();
        }
        let start = self.inner.election_counter.fetch_add(1, Ordering::SeqCst) as usize % len;
        (0..limit.min(len))
            .map(|offset| state.nodes[(start + offset) % len].id.clone())
            .collect()
    }

    #[allow(non_snake_case)]
    /// 按下标取节点 ID。
    pub fn GetNodeIDByIdx(&self, index: usize) -> String {
        self.inner
            .state
            .lock()
            .unwrap()
            .nodes
            .get(index)
            .unwrap_or_else(|| panic!("node index {index} out of range"))
            .id
            .clone()
    }

    #[allow(non_snake_case)]
    /// 当前节点数。
    pub fn NodeCount(&self) -> usize {
        self.inner.state.lock().unwrap().nodes.len()
    }

    #[allow(non_snake_case)]
    /// 等待所有异步操作线程结束。
    pub fn WaitAsyncOperations(&self) -> Result<(), DxfError> {
        let joins = std::mem::take(&mut *self.inner.joins.lock().unwrap());
        for join in joins {
            join.join()
                .map_err(|_| DxfError("asynchronous DXF operation panicked".into()))??;
        }
        Ok(())
    }
}

/// 析构时汇合异步线程、停止节点并恢复资源/间隔。
impl Drop for TestDxfInner {
    fn drop(&mut self) {
        for join in std::mem::take(self.joins.get_mut().unwrap()) {
            let _ = join.join();
        }
        for node in std::mem::take(&mut self.state.get_mut().unwrap().nodes) {
            let _ = self.runtime.stop_executor(&node.id);
            if node.owner {
                let _ = self.runtime.stop_scheduler(&node.id);
            }
        }
        self.interval_guard.get_mut().unwrap().take();
        if let Some(resource) = self.original_resource.get_mut().unwrap().take() {
            let _ = self.runtime.set_node_resource(resource);
        }
    }
}

#[allow(non_snake_case)]
/// 构造指定节点数与 CPU 数的测试上下文。
pub fn NewTestDXFContext(
    runtime: Arc<dyn DxfRuntime>,
    node_num: usize,
    cpu_count: usize,
    reduce_check_interval: bool,
) -> Result<TestDXFContext, DxfError> {
    TestDXFContext::new(runtime, node_num, cpu_count, reduce_check_interval)
}

#[allow(non_snake_case)]
/// 在 [min_count, max_count] 间随机节点数构造上下文（16 CPU、缩短间隔）。
pub fn NewDXFContextWithRandomNodes(
    runtime: Arc<dyn DxfRuntime>,
    min_count: usize,
    max_count: usize,
) -> Result<TestDXFContext, DxfError> {
    // 非法区间直接报错。
    if min_count > max_count {
        return Err(DxfError("minimum node count exceeds maximum".into()));
    }
    let width = max_count - min_count + 1;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize;
    TestDXFContext::new(runtime, min_count + seed % width, 16, true)
}
