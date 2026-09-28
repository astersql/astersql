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

// ExchangeSender：MPP 片段边界上的推送端（Push-mode Sink），
// 按 PassThrough/Broadcast/Hash 将本片段结果分发到目标任务。

use base::{ContextRef, MPPSink, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, ExprBox};
use protobuf::Message;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// Push-mode MPP boundary that dispatches one fragment to its target tasks.
/// 推模式 MPP 边界：把当前片段输出分发到 TargetTasks。
pub struct PhysicalExchangeSender {
    /// Schema 生产者与基类物理计划。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 目标接收端 MPP 任务列表。
    pub TargetTasks: Vec<kv::MPPTask>,
    /// 交换类型：透传 / 广播 / Hash 分区。
    pub ExchangeType: tipb::ExchangeType,
    /// Hash 分区列（仅 Hash 交换使用）。
    pub HashCols: Vec<property::MPPPartitionColumn>,
    /// 本端（Sender 所在）MPP 任务。
    pub Tasks: Vec<kv::MPPTask>,
    /// 交换数据压缩模式。
    pub CompressionMode: vardef::ExchangeCompressionMode,
}

impl PhysicalExchangeSender {
    /// 创建默认 PassThrough、无压缩的 Sender。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeExchangeSender,
                0,
            )),
            TargetTasks: Vec::new(),
            ExchangeType: tipb::ExchangeType::PassThrough,
            HashCols: Vec::new(),
            Tasks: Vec::new(),
            CompressionMode: vardef::ExchangeCompressionModeNONE,
        }
    }

    /// 重新安装 TypeExchangeSender 基类与统计信息。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo) -> Self {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP(plancodec::TypeExchangeSender);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(0);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self
    }

    /// Go intentionally does not copy scheduled tasks while cloning a plan.
    /// 克隆时故意不复制已调度任务（与 Go 一致）。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            TargetTasks: Vec::new(),
            ExchangeType: self.ExchangeType,
            HashCols: self.HashCols.iter().map(|column| column.Clone()).collect(),
            Tasks: Vec::new(),
            CompressionMode: self.CompressionMode,
        })
    }

    /// 读取压缩模式。
    pub fn GetCompressionMode(&self) -> vardef::ExchangeCompressionMode {
        self.CompressionMode
    }

    /// 是否为 Hash 分区交换。
    pub fn IsHashExchange(&self) -> bool {
        self.ExchangeType == tipb::ExchangeType::Hash
    }

    /// 本端任务切片。
    pub fn GetSelfTasks(&self) -> &[kv::MPPTask] {
        &self.Tasks
    }

    /// 覆盖本端任务。
    pub fn SetSelfTasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.Tasks = tasks;
    }

    /// 覆盖目标任务。
    pub fn SetTargetTasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.TargetTasks = tasks;
    }

    /// 追加目标任务，保留输入顺序和重复项。
    pub fn AppendTargetTasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.TargetTasks.extend(tasks);
    }

    /// EXPLAIN：交换类型、压缩、Hash 列、任务 ID、stream_count。
    pub fn ExplainInfo(&self) -> String {
        // 将 tipb 交换类型映射为 EXPLAIN 可读名称。
        let exchange = match self.ExchangeType {
            tipb::ExchangeType::PassThrough => "PassThrough",
            tipb::ExchangeType::Broadcast => "Broadcast",
            tipb::ExchangeType::Hash => "HashPartition",
        };
        let mut output = format!("ExchangeType: {exchange}");
        if self.CompressionMode != vardef::ExchangeCompressionModeNONE {
            output.push_str(&format!(", Compression: {}", self.CompressionMode.Name()));
        }
        if self.ExchangeType == tipb::ExchangeType::Hash {
            let columns = property::ExplainColumnList(
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .s_ctx()
                    .GetExprCtx()
                    .GetEvalCtx(),
                &self.HashCols,
            );
            output.push_str(&format!(
                ", Hash Cols: {}",
                String::from_utf8_lossy(&columns)
            ));
        }
        if !self.Tasks.is_empty() {
            output.push_str(", tasks: [");
            output.push_str(
                &self
                    .Tasks
                    .iter()
                    .map(|task| task.ID.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(']');
        }
        let stream_count = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            output.push_str(&format!(", stream_count: {stream_count}"));
        }
        output
    }

    /// 规范化 EXPLAIN，与 ExplainInfo 相同。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.ExplainInfo()
    }

    /// 用唯一孩子 Schema 解析 HashCols 索引。
    pub fn ResolveIndicesItself(&mut self) -> Result<(), expression::Error> {
        let child = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .copied()
            .ok_or_else(|| expression::errors::New("exchange sender requires one child"))?;
        let schema = child.schema().Clone();
        self.ResolveIndicesItselfWithSchema(&schema)
    }

    /// 相对给定 Schema 解析分区列索引。
    pub fn ResolveIndicesItselfWithSchema(
        &mut self,
        schema: &expression::Schema,
    ) -> Result<(), expression::Error> {
        for column in &mut self.HashCols {
            *column = column.ResolveIndices(schema)?;
        }
        Ok(())
    }

    /// 先解析基类，再解析自身 HashCols。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        self.ResolveIndicesItself()
    }

    /// Sender 无相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }

    /// 代价模型 v1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 代价模型 v2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }

    /// 挂接到物理任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }

    /// 编码 tipb.ExchangeSender（含 child、分区键、类型与压缩）。
    pub fn ToPB(
        &self,
        context: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let child = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .ok_or_else(|| expression::errors::New("exchange sender requires one child"))?
            .to_pb(context, kv::StoreType::TiFlash)?;

        let encoded_tasks = self
            .TargetTasks
            .iter()
            .map(|task| {
                task.ToPB()
                    .write_to_bytes()
                    .map_err(|error| expression::errors::New(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Hash 分区：收集分区表达式与带 collation 的字段类型。
        let mut hash_expressions: Vec<ExprBox> = Vec::with_capacity(self.HashCols.len());
        let mut hash_types = Vec::with_capacity(self.HashCols.len());
        for column in &self.HashCols {
            hash_expressions.push(Box::new(column.Col.Clone()));
            let return_type = column.Col.RetType.as_ref().ok_or_else(|| {
                expression::errors::New("exchange partition column has no return type")
            })?;
            let mut field_type = expression::ToPBFieldTypeWithCheck(return_type, store)?;
            field_type.set_collate(column.CollateID);
            hash_types.push(field_type);
        }
        let all_types = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .schema()
            .Columns
            .iter()
            .map(|column| {
                let field_type = column.RetType.as_ref().ok_or_else(|| {
                    expression::errors::New("exchange sender column has no return type")
                })?;
                expression::ToPBFieldTypeWithCheck(field_type, store)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let client = context
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let partition_keys = expression::ExpressionsToPBList(
            context.GetExprCtx().GetEvalCtx(),
            &hash_expressions,
            client.as_ref(),
        )?;

        // 会话压缩模式映射到 tipb.CompressionMode。
        let compression = match self.CompressionMode {
            vardef::ExchangeCompressionModeFast => tipb::CompressionMode::Fast,
            vardef::ExchangeCompressionModeHC => tipb::CompressionMode::HighCompression,
            _ => tipb::CompressionMode::None,
        };
        let mut sender = tipb::ExchangeSender::new();
        sender.set_tp(self.ExchangeType);
        sender.set_encoded_task_meta(encoded_tasks.into());
        sender.set_partition_keys(partition_keys.into());
        sender.set_child(*child);
        sender.set_types(hash_types.into());
        sender.set_all_field_types(all_types.into());
        sender.set_compression(compression);

        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeExchangeSender);
        executor.set_exchange_sender(sender);
        executor.set_executor_id(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .explain_id(&[])
                .to_string(),
        );
        executor.set_fine_grained_shuffle_stream_count(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .TiFlashFineGrainedShuffleStreamCount,
        );
        executor.set_fine_grained_shuffle_batch_size(context.TiFlashFineGrainedShuffleBatchSize);
        Ok(Box::new(executor))
    }

    /// 估算内存：producer + HashCols + 任务容量。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + (std::mem::size_of::<Vec<kv::MPPTask>>() * 3 + std::mem::size_of::<i32>()) as i64
            + self
                .HashCols
                .iter()
                .map(property::MPPPartitionColumn::MemoryUsage)
                .sum::<i64>()
            + ((self.TargetTasks.capacity() + self.HashCols.capacity() + self.Tasks.capacity())
                * std::mem::size_of::<*const ()>()) as i64
    }
}

/// 实现 MPPSink：调度层通过统一接口配置本端/目标任务与压缩。
impl MPPSink for PhysicalExchangeSender {
    /// 转发 GetCompressionMode。
    fn get_compression_mode(&self) -> vardef::ExchangeCompressionMode {
        self.GetCompressionMode()
    }

    /// 转发 GetSelfTasks。
    fn get_self_tasks(&self) -> &[kv::MPPTask] {
        self.GetSelfTasks()
    }

    /// 转发 SetSelfTasks。
    fn set_self_tasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.SetSelfTasks(tasks)
    }

    /// 转发 SetTargetTasks。
    fn set_target_tasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.SetTargetTasks(tasks)
    }

    /// 转发 AppendTargetTasks。
    fn append_target_tasks(&mut self, tasks: Vec<kv::MPPTask>) {
        self.AppendTargetTasks(tasks)
    }
}
