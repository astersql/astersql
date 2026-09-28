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

// 逻辑计划与物理计划核心类型。
//
// 执行计划（Plan）在 DXF 中分两层：LogicalPlan 描述任务级语义与 meta 编解码；
// PhysicalPlan 是由 ProcessorSpec 构成的处理器 DAG，按 step（任务阶段）切分
// 并生成各子任务（subtask）的 meta。PlanCtx 贯穿规划各阶段，携带会话与存储句柄。

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// 启用 kv-runtime 时，规划上下文可持有真实 KV Storage。
#[cfg(feature = "kv-runtime")]
pub type PlanStore = Arc<dyn astersql_kv::Storage>;
/// 未启用 kv-runtime 时，用类型擦除句柄占位，避免强依赖存储实现。
#[cfg(not(feature = "kv-runtime"))]
pub type PlanStore = Arc<dyn Any + Send + Sync>;

/// 规划阶段错误类型，与 storage 边界错误对齐。
pub type PlannerError = storage::Error;

/// 贯穿逻辑计划、物理计划与任务创建各阶段的上下文。
/// 类似 Go 中的接口字段，session 与 storage 为共享句柄。
/// All values passed through the logical-plan, physical-plan and task creation
/// stages. Like Go's interface fields, session and storage are shared handles.
#[derive(Clone)]
pub struct PlanCtx {
    /// 请求上下文（超时、取消等）。
    pub ctx: storage::Context,
    /// 会话上下文，用于带 session 的任务创建。
    pub session_ctx: storage::sessionctx::Context,
    /// 任务 ID；规划早期可能尚未分配。
    pub task_id: i64,
    /// 任务业务键，创建任务时写入存储。
    pub task_key: String,
    /// 任务类型（如 Backfill、ImportInto）。
    pub task_type: proto::TaskType,
    /// 所需 slot/线程数，映射到 required_slots。
    pub thread_count: i32,
    /// 任务可用的最大节点数上限。
    pub max_node_count: i32,
    /// Keyspace（多租户键空间标识）。
    pub keyspace: String,
    /// 上一阶段各 step 的子任务 meta，供后续 step 规划消费。
    pub previous_subtask_metas: HashMap<proto::Step, Vec<Vec<u8>>>,
    /// 是否走全局排序（global sort）路径。
    pub global_sort: bool,
    /// 下一任务 step（阶段编号）。
    pub next_task_step: proto::Step,
    /// 可参与执行的节点数量。
    pub execute_nodes_count: i32,
    /// 可选的底层存储句柄。
    pub store: Option<PlanStore>,
}

/// 提供零值默认上下文，便于测试与占位构造。
impl Default for PlanCtx {
    fn default() -> Self {
        Self {
            ctx: (),
            session_ctx: storage::sessionctx::Context::default(),
            task_id: 0,
            task_key: String::new(),
            task_type: "",
            thread_count: 0,
            max_node_count: 0,
            keyspace: String::new(),
            previous_subtask_metas: HashMap::new(),
            global_sort: false,
            next_task_step: 0,
            execute_nodes_count: 0,
            store: None,
        }
    }
}

/// 逻辑计划：保留 Go 侧四类生命周期操作。
/// 额外任务参数、任务 meta 编解码，以及构造物理计划。
/// Logical plans preserve the four Go lifecycle operations: extra task
/// parameters, task-meta encoding/decoding and physical-plan construction.
pub trait LogicalPlan {
    /// 返回任务额外参数（手动恢复、prepare 模式等）。
    fn get_task_extra_params(&self) -> proto::ExtraParams;
    /// 将逻辑计划编码为任务级 meta 字节。
    fn to_task_meta(&self) -> Result<Vec<u8>, PlannerError>;
    /// 从任务 meta 反序列化并填充自身。
    fn from_task_meta(&mut self, meta: &[u8]) -> Result<(), PlannerError>;
    /// 基于 PlanCtx 构造物理计划（处理器 DAG）。
    fn to_physical_plan(&self, context: PlanCtx) -> Result<PhysicalPlan, PlannerError>;
}

/// 物理计划：处理器 DAG。
/// 处理器按插入顺序保存，该顺序也是按 step 导出 subtask meta 的顺序。
/// A processor DAG. Processors remain in insertion order, which is also the
/// subtask-meta order returned for a requested step.
#[derive(Default)]
pub struct PhysicalPlan {
    /// 按插入顺序排列的处理器规格列表。
    pub processors: Vec<ProcessorSpec>,
}

impl PhysicalPlan {
    /// 追加一个处理器到 DAG 末尾。
    pub fn add_processor(&mut self, processor: ProcessorSpec) {
        self.processors.push(processor);
    }

    /// 只读访问全部处理器规格。
    pub fn processors(&self) -> &[ProcessorSpec] {
        &self.processors
    }

    /// 按指定 step 过滤处理器，并依次调用 pipeline 生成 subtask meta。
    /// 未匹配 step 的处理器会被跳过，顺序保持插入序。
    pub fn to_subtask_metas(
        &self,
        context: PlanCtx,
        step: proto::Step,
    ) -> Result<Vec<Vec<u8>>, PlannerError> {
        let mut subtask_metas = Vec::with_capacity(self.processors.len());
        for processor in &self.processors {
            if processor.step != step {
                continue;
            }
            // PlanCtx is a Go value parameter. Clone its shared handles so each
            // matching pipeline observes the same planning snapshot.
            subtask_metas.push(processor.pipeline.to_subtask_meta(context.clone())?);
        }
        Ok(subtask_metas)
    }
}

/// 单个处理器节点：输入/输出链接、所属 step 与流水线规格。
pub struct ProcessorSpec {
    /// 处理器 ID，用于 LinkSpec 互连。
    pub id: i32,
    /// 输入列类型与上游链接。
    pub input: InputSpec,
    /// 流水线规格，负责生成该处理器的 subtask meta。
    pub pipeline: Box<dyn PipelineSpec>,
    /// 输出下游链接。
    pub output: OutputSpec,
    /// 该处理器所属的任务 step。
    pub step: proto::Step,
}

impl ProcessorSpec {
    /// 构造处理器；输入/输出默认为空规格。
    pub fn new(id: i32, step: proto::Step, pipeline: Box<dyn PipelineSpec>) -> Self {
        Self {
            id,
            input: InputSpec::default(),
            pipeline,
            output: OutputSpec::default(),
            step,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 处理器输入描述：列类型编码与上游链接。
pub struct InputSpec {
    /// 指向上游处理器的链接。
    /// 列类型序列化字节（对齐 Go 侧编码）。
    pub column_types: Vec<u8>,
    pub links: Vec<LinkSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 处理器输出描述：下游链接列表。
pub struct OutputSpec {
    pub links: Vec<LinkSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DAG 边：指向另一个处理器的 ID。
pub struct LinkSpec {
    /// 对端处理器 ID。
    pub processor_id: i32,
}

/// 流水线规格：把 PlanCtx 编码为单个 subtask 的 meta。
pub trait PipelineSpec {
    /// 生成该流水线对应的子任务 meta。
    fn to_subtask_meta(&self, context: PlanCtx) -> Result<Vec<u8>, PlannerError>;
}
