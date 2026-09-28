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

// ExchangeReceiver：MPP（大规模并行处理）片段边界上的网络接收端，
// 持有下游发送任务列表；拓扑生成时孩子仍是 Sender，但 protobuf 下推在此截断。

use base::{ContextRef, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::CorrelatedColumn;
use protobuf::Message;
use std::sync::{Arc, RwLock};

use crate::{BasePhysicalPlan, PhysicalExchangeSender, PhysicalSchemaProducer};

/// Network leaf of one fragment and owner of the sending tasks on the lower
/// fragment. Its child remains the connected exchange sender for topology
/// generation, while protobuf conversion stops at this boundary.
/// 片段网络叶子与下游发送任务的持有者；孩子保留 Connected Sender 供拓扑用，PB 转换止于此。
pub struct PhysicalExchangeReceiver {
    /// Schema 生产者与基类物理计划。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 线程安全的 MPP 任务列表（发送端元数据）。
    tasks: Arc<RwLock<Vec<kv::MPPTask>>>,
}

impl PhysicalExchangeReceiver {
    /// 创建 TypeExchangeReceiver 空接收端。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeExchangeReceiver,
                0,
            )),
            tasks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 克隆基类与 schema；与 Go 一致，不复制运行时生成的任务列表。
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
            tasks: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// 取唯一孩子并断言为 PhysicalExchangeSender。
    pub fn GetExchangeSender(&self) -> Result<&PhysicalExchangeSender, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .and_then(|child| child.as_any().downcast_ref::<PhysicalExchangeSender>())
            .ok_or_else(|| expression::errors::New("exchange receiver child must be a sender"))
    }

    /// 覆盖写入下游发送任务列表。
    pub fn SetTasks(&self, tasks: Vec<kv::MPPTask>) {
        *self.tasks.write().expect("exchange receiver task lock") = tasks;
    }

    /// 返回当前任务列表快照。
    pub fn Tasks(&self) -> Vec<kv::MPPTask> {
        self.tasks
            .read()
            .expect("exchange receiver task lock")
            .clone()
    }

    /// EXPLAIN：有细粒度 shuffle 流时输出 stream_count。
    pub fn ExplainInfo(&self) -> String {
        let streams = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if streams == 0 {
            String::new()
        } else {
            format!("stream_count: {streams}")
        }
    }

    /// 规范化 EXPLAIN 与 ExplainInfo 相同。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.ExplainInfo()
    }

    /// 委托 schema producer 解析列索引。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()
    }

    /// 接收端本身不含表达式，无相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }

    /// 挂接到物理任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }

    /// 代价 v1：委托基类。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 代价 v2：委托基类。
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

    /// 编码 tipb.ExchangeReceiver：任务元数据、字段类型与细粒度 shuffle 参数。
    pub fn ToPB(
        &self,
        context: &mut base::BuildPBContext,
        _store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let tasks = self.Tasks();
        // 将每个 MPPTask 序列化为 encoded_task_meta。
        let encoded_tasks = tasks
            .iter()
            .map(|task| {
                task.ToPB()
                    .write_to_bytes()
                    .map_err(|error| expression::errors::New(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // 输出列类型必须可映射到 TiFlash FieldType。
        let field_types = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .schema()
            .Columns
            .iter()
            .map(|column| {
                let field_type = column.RetType.as_ref().ok_or_else(|| {
                    expression::errors::New("exchange receiver column has no return type")
                })?;
                expression::ToPBFieldTypeWithCheck(field_type, kv::StoreType::TiFlash)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut receiver = tipb::ExchangeReceiver::new();
        receiver.set_encoded_task_meta(encoded_tasks.into());
        receiver.set_field_types(field_types.into());
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeExchangeReceiver);
        executor.set_exchange_receiver(receiver);
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

    /// 内存：schema producer + 任务切片容量。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + (self
                .tasks
                .read()
                .expect("exchange receiver task lock")
                .capacity()
                * std::mem::size_of::<kv::MPPTask>()) as i64
    }
}
