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

// MPP Fragment 拓扑生成：把以 ExchangeSender 为根的物理计划切成可调度片段。
//
// Fragment 是 MPP 执行图中的调度单元，挂载一个 `MPPSink`（通常为 ExchangeSender）。
// 本模块按 ExchangeReceiver 边界递归构建子片段，并向协调器返回 KV ranges 与节点地址。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base::{PhysicalPlan, Plan};

use crate::{PhysicalExchangeReceiver, PhysicalExchangeSender, PhysicalTableScan};

fn allocate_mpp_task_id(counter: &AtomicI64) -> i64 {
    counter.fetch_add(1, Ordering::SeqCst) + 1
}

/// Allocates a statement-scoped MPP task ID, starting at one.
///
/// Go receives the whole session context and reaches this counter through
/// `SessionVars.StmtCtx.MPPQueryInfo`. Rust accepts the statement context
/// directly to preserve the same query-scoped lifetime without coupling this
/// physical-operator crate to a concrete session implementation.
pub fn AllocMPPTaskID(statement_context: &stmtctx::StatementContext) -> i64 {
    allocate_mpp_task_id(&statement_context.MPPQueryInfo.AllocatedMPPTaskID)
}

// Keep the Go initial value and `atomic.AddUint64` behavior: the first allocated
// local MPP query ID is 2, and every subsequent call advances it by one.
static MPP_QUERY_ID: AtomicU64 = AtomicU64::new(1);

/// Allocates a process-local MPP query ID, matching Go `AllocMPPQueryID`.
pub fn AllocMPPQueryID() -> u64 {
    MPP_QUERY_ID.fetch_add(1, Ordering::SeqCst) + 1
}

/// A scheduled MPP fragment ready for coordinator request construction.
/// 已调度的 MPP 片段，可直接用于构造协调器请求。
#[derive(Clone)]
pub struct Fragment {
    /// 片段出口 Sink；多读者通过 Arc 共享同一生产者。
    pub Sink: Arc<Mutex<Box<dyn base::MPPSink>>>,
    /// 是否为查询根片段（结果回传 TiDB）。
    pub IsRoot: bool,
    /// 是否必须收敛到单个 TiFlash 任务；PassThrough 接收端会置位。
    pub singleton: bool,
}

impl Fragment {
    /// 包装 Sink 并标记是否为根片段。
    pub fn New(sink: Box<dyn base::MPPSink>, is_root: bool) -> Self {
        Self {
            Sink: Arc::new(Mutex::new(sink)),
            IsRoot: is_root,
            singleton: false,
        }
    }

    /// 初始化片段元数据，并保留 Go 侧 PassThrough 的 singleton 语义。
    ///
    /// `singleton` 使用 OR 语义：只要片段内任一 ExchangeReceiver 的下游
    /// Sender 是 PassThrough，该片段的任务就必须收敛到单节点。
    pub fn init(&mut self, plan: &dyn PhysicalPlan) -> Result<(), expression::Error> {
        if let Some(receiver) = plan.as_any().downcast_ref::<PhysicalExchangeReceiver>() {
            let sender = receiver.GetExchangeSender()?;
            self.singleton |= sender.ExchangeType == tipb::ExchangeType::PassThrough;
            return Ok(());
        }
        for child in plan.children() {
            self.init(child)?;
        }
        Ok(())
    }

    /// 结构体浅层大小加上内部 Sink 的动态内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64 + self.Sink.lock().expect("MPP sink lock").memory_usage()
    }
}

/// `GenerateRootMPPTasks` 的返回值：片段列表、UnionScan 所需 KV 范围与节点地址集。
pub struct GeneratedRootMppTasks {
    pub fragments: Vec<Fragment>,
    pub kv_ranges: Vec<kv::KeyRange>,
    pub node_addresses: HashSet<String>,
}

/// 从物理计划生成根 MPP 任务拓扑的抽象接口。
pub trait RootMppTaskGenerator: Send + Sync {
    fn GenerateRootMPPTasks(
        &self,
        original_plan: &dyn base::PhysicalPlan,
        start_ts: u64,
        gather_id: u64,
        query_id: kv::MPPQueryID,
    ) -> Result<GeneratedRootMppTasks, expression::Error>;
}

/// InfoSchema/range-codec boundary used by the topology generator. It converts
/// one physical table scan into the exact MPP build request and the KV ranges
/// that must be returned to UnionScan.
/// InfoSchema/range-codec 边界：把一次表扫描编码为 MPP 构建请求与 KV ranges。
pub trait MppScanRangeEncoder: Send + Sync {
    fn Encode(&self, scan: &PhysicalTableScan) -> Result<EncodedMppScan, expression::Error>;
}

/// 单次表扫描编码结果：构建请求、KV 范围、表 ID 与 TiFlash 静态裁剪标记。
pub struct EncodedMppScan {
    pub request: kv::MPPBuildTasksRequest,
    pub kv_ranges: Vec<kv::KeyRange>,
    pub table_id: i64,
    pub tiflash_static_prune: bool,
}

/// Concrete session-backed implementation of Go `GenerateRootMPPTasks` for
/// the registered owned physical-plan topology.
/// 会话侧 `GenerateRootMPPTasks` 实现，依赖 MPPClient 与 range encoder。
pub struct SessionRootMppTaskGenerator {
    pub client: Arc<dyn kv::MPPClient>,
    pub context: kv::Context,
    pub range_encoder: Arc<dyn MppScanRangeEncoder>,
    pub timeout: Duration,
    pub dispatch_policy: kv::tiflashcompute::DispatchPolicy,
    pub replica_read: kv::tiflash::ReplicaRead,
    pub chosen_version: kv::MppVersion,
    pub session_id: u64,
    pub session_alias: String,
    next_task_id: AtomicI64,
}

impl SessionRootMppTaskGenerator {
    #[allow(clippy::too_many_arguments)]
    /// 绑定会话与存储侧参数；任务 ID 从 0 起自增分配。
    pub fn New(
        client: Arc<dyn kv::MPPClient>,
        context: kv::Context,
        range_encoder: Arc<dyn MppScanRangeEncoder>,
        timeout: Duration,
        dispatch_policy: kv::tiflashcompute::DispatchPolicy,
        replica_read: kv::tiflash::ReplicaRead,
        chosen_version: kv::MppVersion,
        session_id: u64,
        session_alias: String,
    ) -> Self {
        Self {
            client,
            context,
            range_encoder,
            timeout,
            dispatch_policy,
            replica_read,
            chosen_version,
            session_id,
            session_alias,
            next_task_id: AtomicI64::new(0),
        }
    }

    /// 分配全局唯一的 MPP 任务 ID（从 1 开始）。
    fn allocate_task_id(&self) -> i64 {
        allocate_mpp_task_id(&self.next_task_id)
    }

    /// 通过 range encoder 与 MPPClient 为表扫描构造各 TiFlash 上的 MPPTask。
    fn construct_scan_tasks(
        &self,
        scan: &PhysicalTableScan,
        start_ts: u64,
        gather_id: u64,
        query_id: kv::MPPQueryID,
    ) -> Result<(Vec<kv::MPPTask>, Vec<kv::KeyRange>), expression::Error> {
        let encoded = self.range_encoder.Encode(scan)?;
        let request = encoded.request;
        let partition_table_ids: Vec<_> = request
            .PartitionIDAndRanges
            .iter()
            .map(|partition| partition.ID)
            .collect();
        let mut warning = |_error: kv::Error| {};
        let metas = self
            .client
            .ConstructMPPTasks(
                &self.context,
                &request,
                self.timeout,
                self.dispatch_policy,
                self.replica_read,
                &mut warning,
            )
            .map_err(|error| expression::errors::New(error.to_string()))?;
        // MPP 模式下表扫描必须至少有一个任务，否则无法下发。
        if metas.is_empty() {
            return Err(expression::errors::New(
                "In mpp mode, the number of tasks for table scan should not be zero",
            ));
        }
        Ok((
            metas
                .into_iter()
                .map(|meta| kv::MPPTask {
                    Meta: Some(meta),
                    ID: self.allocate_task_id(),
                    StartTs: start_ts,
                    GatherID: gather_id,
                    MppQueryID: query_id,
                    TableID: encoded.table_id,
                    MppVersion: self.chosen_version,
                    SessionID: self.session_id,
                    SessionAlias: self.session_alias.clone(),
                    PartitionTableIDs: partition_table_ids.clone(),
                    TiFlashStaticPrune: encoded.tiflash_static_prune,
                    ..Default::default()
                })
                .collect(),
            encoded.kv_ranges,
        ))
    }

    /// 无本地表扫描时，按子片段任务地址去重并派生本层 MPPTask。
    fn derive_tasks_from_children(
        &self,
        child_tasks: &[Vec<kv::MPPTask>],
        start_ts: u64,
        gather_id: u64,
        query_id: kv::MPPQueryID,
    ) -> Result<Vec<kv::MPPTask>, expression::Error> {
        let mut by_address: HashMap<String, Box<dyn kv::MPPTaskMeta>> = HashMap::new();
        for tasks in child_tasks {
            for task in tasks {
                if let Some(meta) = task.Meta.as_ref() {
                    by_address
                        .entry(meta.GetAddress())
                        .or_insert_with(|| meta.clone());
                }
            }
        }
        if by_address.is_empty() {
            return Err(expression::errors::New(
                "MPP fragment has neither table scan nor child tasks",
            ));
        }
        // 按地址排序保证任务列表确定性。
        let mut addresses: Vec<_> = by_address.into_iter().collect();
        addresses.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(addresses
            .into_iter()
            .map(|(_, meta)| kv::MPPTask {
                Meta: Some(meta),
                ID: self.allocate_task_id(),
                StartTs: start_ts,
                GatherID: gather_id,
                MppQueryID: query_id,
                TableID: -1,
                MppVersion: self.chosen_version,
                SessionID: self.session_id,
                SessionAlias: self.session_alias.clone(),
                ..Default::default()
            })
            .collect())
    }

    /// 递归构建以 ExchangeSender 为出口的片段树，并用 cache 去重共享 CTE 生产者。
    fn build_sender(
        &self,
        sender: &PhysicalExchangeSender,
        is_root: bool,
        start_ts: u64,
        gather_id: u64,
        query_id: kv::MPPQueryID,
        cache: &mut HashMap<i32, BuiltFragments>,
    ) -> Result<BuiltFragments, expression::Error> {
        // Shared CTE readers carry cloned receiver->sender paths with the same
        // producer plan ID. Reusing the owned sink makes all readers append to
        // one producer fragment and prevents duplicate scan/range scheduling.
        // 共享 CTE 读者复用同一生产者片段，清空 kv_ranges 避免重复上报扫描范围。
        if let Some(cached) = cache.get(&sender.id()) {
            let mut reused = cached.clone();
            reused.kv_ranges.clear();
            return Ok(reused);
        }
        let receivers = collect_receivers(sender);
        let mut child_groups = Vec::with_capacity(receivers.len());
        for receiver in &receivers {
            child_groups.push(self.build_sender(
                receiver.GetExchangeSender()?,
                false,
                start_ts,
                gather_id,
                query_id,
                cache,
            )?);
        }

        let scans = collect_table_scans(sender);
        // 单个 Fragment 内不允许出现多个表扫描。
        if scans.len() > 1 {
            return Err(expression::errors::New(
                "one MPP fragment contains more than one table scan",
            ));
        }
        let (mut tasks, mut kv_ranges) = match scans.first() {
            Some(scan) => self.construct_scan_tasks(scan, start_ts, gather_id, query_id)?,
            None => (
                self.derive_tasks_from_children(
                    &child_groups
                        .iter()
                        .map(|group| group.root_tasks.clone())
                        .collect::<Vec<_>>(),
                    start_ts,
                    gather_id,
                    query_id,
                )?,
                Vec::new(),
            ),
        };
        // PassThrough Exchange 表示结果汇聚到单任务，截断为 1。
        if receivers.iter().any(|receiver| {
            receiver
                .GetExchangeSender()
                .is_ok_and(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
        }) {
            tasks.truncate(1);
        }

        for (receiver, child) in receivers.iter().zip(&child_groups) {
            receiver.SetTasks(child.root_tasks.clone());
        }
        let mut owned_sender = sender.Clone(sender.s_ctx().clone())?;
        owned_sender.SetSelfTasks(tasks.clone());
        if is_root {
            // 根片段目标任务 ID=-1，表示结果回传协调器。
            owned_sender.SetTargetTasks(vec![kv::MPPTask {
                ID: -1,
                StartTs: start_ts,
                GatherID: gather_id,
                MppQueryID: query_id,
                MppVersion: self.chosen_version,
                SessionID: self.session_id,
                SessionAlias: self.session_alias.clone(),
                ..Default::default()
            }]);
        }

        let mut fragments = vec![Fragment::New(Box::new(owned_sender), is_root)];
        for mut child in child_groups {
            // 子片段 sink 追加本层任务为 target，以便数据流向父片段。
            child.fragments[0]
                .Sink
                .lock()
                .expect("MPP sink lock")
                .append_target_tasks(tasks.clone());
            kv_ranges.append(&mut child.kv_ranges);
            fragments.append(&mut child.fragments);
        }
        let built = BuiltFragments {
            fragments,
            root_tasks: tasks,
            kv_ranges,
        };
        cache.insert(sender.id(), built.clone());
        Ok(built)
    }
}

/// 一次 `build_sender` 的中间结果：片段、本层任务与累积 KV 范围。
#[derive(Clone)]
struct BuiltFragments {
    fragments: Vec<Fragment>,
    root_tasks: Vec<kv::MPPTask>,
    kv_ranges: Vec<kv::KeyRange>,
}

impl RootMppTaskGenerator for SessionRootMppTaskGenerator {
    /// 要求根计划为 ExchangeSender，生成去重后的片段拓扑与非空 KV ranges。
    fn GenerateRootMPPTasks(
        &self,
        original_plan: &dyn PhysicalPlan,
        start_ts: u64,
        gather_id: u64,
        query_id: kv::MPPQueryID,
    ) -> Result<GeneratedRootMppTasks, expression::Error> {
        let sender = original_plan
            .as_any()
            .downcast_ref::<PhysicalExchangeSender>()
            .ok_or_else(|| expression::errors::New("MPP root plan must be an exchange sender"))?;
        let mut cache = HashMap::new();
        let mut built =
            self.build_sender(sender, true, start_ts, gather_id, query_id, &mut cache)?;
        if built.kv_ranges.is_empty() {
            return Err(expression::errors::New(
                "kvRanges for MPPTask should not be empty",
            ));
        }
        let node_addresses: HashSet<_> = built
            .fragments
            .iter()
            .flat_map(|fragment| {
                fragment
                    .Sink
                    .lock()
                    .expect("MPP sink lock")
                    .get_self_tasks()
                    .to_vec()
            })
            .filter_map(|task| task.Meta.as_ref().map(|meta| meta.GetAddress()))
            .collect();
        // 按 Sink Arc 指针去重，共享 CTE 生产者只保留一份片段。
        let mut seen = HashSet::new();
        built
            .fragments
            .retain(|fragment| seen.insert(Arc::as_ptr(&fragment.Sink) as usize));
        Ok(GeneratedRootMppTasks {
            fragments: built.fragments,
            kv_ranges: built.kv_ranges,
            node_addresses,
        })
    }
}

/// 收集计划树中直接或间接挂接的 ExchangeReceiver（不跨 Receiver 边界）。
fn collect_receivers(plan: &dyn PhysicalPlan) -> Vec<&PhysicalExchangeReceiver> {
    let mut receivers = Vec::new();
    collect_receivers_into(plan, &mut receivers);
    receivers
}

fn collect_receivers_into<'a>(
    plan: &'a dyn PhysicalPlan,
    receivers: &mut Vec<&'a PhysicalExchangeReceiver>,
) {
    for child in plan.children() {
        if let Some(receiver) = child.as_any().downcast_ref::<PhysicalExchangeReceiver>() {
            receivers.push(receiver);
        } else {
            collect_receivers_into(child, receivers);
        }
    }
}

/// 在当前片段内收集 TableScan；遇到 ExchangeReceiver 则停止下钻（属子片段）。
fn collect_table_scans(plan: &dyn PhysicalPlan) -> Vec<&PhysicalTableScan> {
    let mut scans = Vec::new();
    collect_table_scans_into(plan, &mut scans);
    scans
}

fn collect_table_scans_into<'a>(
    plan: &'a dyn PhysicalPlan,
    scans: &mut Vec<&'a PhysicalTableScan>,
) {
    for child in plan.children() {
        if child.as_any().is::<PhysicalExchangeReceiver>() {
            continue;
        }
        if let Some(scan) = child.as_any().downcast_ref::<PhysicalTableScan>() {
            scans.push(scan);
        } else {
            collect_table_scans_into(child, scans);
        }
    }
}
