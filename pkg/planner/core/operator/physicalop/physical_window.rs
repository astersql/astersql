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

// 物理窗口算子（Window）与并行 Shuffle 边界。
//
// 窗口函数在 PARTITION BY / ORDER BY / Frame 上计算；可下推 TiFlash（MPP），
// 或在 TiDB Root 执行。PhysicalShuffle 是 Go `optimizeByShuffle4Window` 插入的并行边界。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use logicalop::LogicalPlan as _;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 物理窗口：对输入流按分区/排序/窗框计算窗口函数。
pub struct PhysicalWindow {
    /// Schema/统计等公共字段。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 窗口函数描述列表（ROW_NUMBER、SUM OVER 等）。
    pub WindowFuncDescs: Vec<logicalop::WindowFuncDesc>,
    /// PARTITION BY 键。
    pub PartitionBy: Vec<property::SortItem>,
    /// 窗口内 ORDER BY 键。
    pub OrderBy: Vec<property::SortItem>,
    /// 窗框（ROWS/RANGE/GROUPS + 起止边界）。
    pub Frame: Option<logicalop::WindowFrame>,
    /// 执行存储：TiDB Root 或 TiFlash。
    pub StoreTp: kv::StoreType,
}

/// 并行 Shuffle 边界：主线程与 worker 尾之间的分发点（见下英文）。
/// PhysicalShuffle is the main-thread boundary inserted by Go's
/// `optimizeByShuffle4Window`. The worker tail remains its physical child,
/// while `DataSourceExplainIDs` identifies the plans feeding the splitter.
pub struct PhysicalShuffle {
    /// Schema/统计等公共字段。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// Shuffle 并发度。
    pub Concurrency: usize,
    /// 喂入 splitter 的数据源 ExplainID 列表。
    pub DataSourceExplainIDs: Vec<String>,
    /// Typed sources feeding the splitter; they are not additional flat children.
    pub DataSources: Vec<Box<dyn PhysicalPlan>>,
    /// Go's per-source partition keys, used for shuffle work and index resolution.
    pub ByItemArrays: Vec<Vec<expression::ExprBox>>,
    pub SplitterType: crate::physical_shuffle::PartitionSplitterType,
}

/// Worker-side receiver; Go keeps its data source outside Children().
pub struct PhysicalShuffleReceiverStub {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub DataSource: Option<Box<dyn PhysicalPlan>>,
}

impl PhysicalShuffleReceiverStub {
    pub fn New(ctx: ContextRef, data_source: Option<Box<dyn PhysicalPlan>>) -> Self {
        let mut producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            ctx,
            plancodec::TypeShuffleReceiver,
            0,
        ));
        if let Some(source) = &data_source {
            producer.SetSchema(source.schema().Clone());
        }
        Self {
            PhysicalSchemaProducer: producer,
            DataSource: data_source,
        }
    }

    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx.clone())?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            DataSource: self
                .DataSource
                .as_ref()
                .map(|source| source.clone_physical(new_ctx))
                .transpose()?,
        })
    }

    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
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
}

impl PhysicalShuffle {
    /// 构造 Shuffle 节点骨架。
    pub fn New(ctx: ContextRef, concurrency: usize, data_source_explain_ids: Vec<String>) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeShuffle,
                0,
            )),
            Concurrency: concurrency,
            DataSourceExplainIDs: data_source_explain_ids,
            DataSources: Vec::new(),
            ByItemArrays: Vec::new(),
            SplitterType: Default::default(),
        }
    }

    /// 继承子节点 Schema/输出名，挂接单一 child。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        child: Box<dyn PhysicalPlan>,
    ) -> Self {
        let schema = child.schema().Clone();
        let output_names = child.output_names();
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(offset);
        self.PhysicalSchemaProducer.SetSchema(schema);
        base::Plan::set_output_names(
            &mut self.PhysicalSchemaProducer.BasePhysicalPlan,
            output_names,
        );
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(vec![Box::new(property::PhysicalProperty::default())]);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildren(vec![child]);
        self
    }

    /// 克隆 Shuffle 元数据。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx.clone())?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            Concurrency: self.Concurrency,
            DataSourceExplainIDs: self.DataSourceExplainIDs.clone(),
            DataSources: self
                .DataSources
                .iter()
                .map(|source| source.clone_physical(new_ctx.clone()))
                .collect::<Result<_, _>>()?,
            ByItemArrays: self
                .ByItemArrays
                .iter()
                .map(|items| items.iter().map(|item| item.CloneExpr()).collect())
                .collect(),
            SplitterType: self.SplitterType,
        })
    }

    /// Explain：并发度与数据源 ID。
    pub fn ExplainInfo(&self) -> String {
        let source_ids = if self.DataSources.is_empty() {
            self.DataSourceExplainIDs.clone()
        } else {
            self.DataSources
                .iter()
                .map(|source| source.explain_id(&[]).to_string())
                .collect()
        };
        format!(
            "execution info: concurrency:{}, data sources:[{}]",
            self.Concurrency,
            source_ids.join(",")
        )
    }

    /// 解析子树列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        if self.DataSources.len() != self.ByItemArrays.len() {
            return Err(expression::errors::New(
                "shuffle data sources and partition keys differ",
            ));
        }
        for (source, items) in self.DataSources.iter().zip(&mut self.ByItemArrays) {
            for item in items {
                *item = item.ResolveIndices(source.schema())?;
            }
        }
        Ok(())
    }

    /// 估算 Shuffle 内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .DataSourceExplainIDs
                .iter()
                .map(String::capacity)
                .sum::<usize>() as i64
            + self
                .DataSources
                .iter()
                .map(|source| source.memory_usage())
                .sum::<i64>()
            + self
                .ByItemArrays
                .iter()
                .flatten()
                .map(|item| item.MemoryUsage())
                .sum::<i64>()
    }

    /// Shuffle 本身不含相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<expression::CorrelatedColumn> {
        Vec::new()
    }

    /// 挂接到执行任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
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
}

/// PhysicalWindow 方法实现。
impl PhysicalWindow {
    /// 构造默认在 TiDB 侧执行的窗口算子。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeWindow,
                0,
            )),
            WindowFuncDescs: Vec::new(),
            PartitionBy: Vec::new(),
            OrderBy: Vec::new(),
            Frame: None,
            StoreTp: kv::StoreType::TiDB,
        }
    }

    /// 写入统计与子节点有序属性需求。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        child: property::PhysicalProperty,
    ) -> Self {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(offset);
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(vec![Box::new(child)]);
        self
    }

    /// 深拷贝窗口描述与排序项。
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
            WindowFuncDescs: self.WindowFuncDescs.clone(),
            PartitionBy: self
                .PartitionBy
                .iter()
                .map(property::SortItem::Clone)
                .collect(),
            OrderBy: self.OrderBy.iter().map(property::SortItem::Clone).collect(),
            Frame: self.Frame.clone(),
            StoreTp: self.StoreTp,
        })
    }

    /// 从窗口函数参数与窗框计算表达式抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<expression::CorrelatedColumn> {
        let frame = self.Frame.iter().flat_map(|frame| {
            [&frame.Start, &frame.End]
                .into_iter()
                .flatten()
                .flat_map(|bound| bound.CalcFuncs.iter())
        });
        self.WindowFuncDescs
            .iter()
            .flat_map(|desc| desc.Args.iter())
            .chain(frame)
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(expression::CorrelatedColumn::Clone)
            .collect()
    }

    /// 估算窗口描述与排序项内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .WindowFuncDescs
                .iter()
                .map(|desc| {
                    desc.Name.capacity() as i64
                        + desc.Args.iter().map(|arg| arg.MemoryUsage()).sum::<i64>()
                })
                .sum::<i64>()
            + self
                .PartitionBy
                .iter()
                .map(property::SortItem::MemoryUsage)
                .sum::<i64>()
            + self
                .OrderBy
                .iter()
                .map(property::SortItem::MemoryUsage)
                .sum::<i64>()
    }

    /// 格式化窗框边界（current row / unbounded / N preceding|following）。
    fn format_bound(&self, bound: &logicalop::FrameBound) -> String {
        if bound.Type == logicalop::BoundType::CurrentRow {
            return "current row".to_owned();
        }
        let mut result = if bound.UnBounded {
            "unbounded".to_owned()
        } else if let Some(function) = bound
            .CalcFuncs
            .first()
            .and_then(|expr| expr.as_any().downcast_ref::<expression::ScalarFunction>())
        {
            let eval = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .s_ctx()
                .GetExprCtx()
                .GetEvalCtx();
            function.GetArgs().get(1).map_or_else(
                || bound.Num.to_string(),
                |expr| {
                    expr.as_any()
                        .downcast_ref::<expression::Constant>()
                        .filter(|constant| {
                            constant.Value.Kind() == expression::types::KindMysqlDecimal
                        })
                        .map_or_else(
                            || expr.ExplainInfo(eval),
                            |constant| constant.Value.GetMysqlDecimal().String(),
                        )
                },
            )
        } else {
            let redact = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .s_ctx()
                .GetExprCtx()
                .GetEvalCtx()
                .GetTiDBRedactLog();
            match redact.as_str() {
                expression::errors::RedactLogEnable => "?".to_owned(),
                expression::errors::RedactLogMarker => format!("‹{}›", bound.Num),
                _ => bound.Num.to_string(),
            }
        };
        result.push_str(if bound.Type == logicalop::BoundType::Preceding {
            " preceding"
        } else {
            " following"
        });
        result
    }

    /// Explain：函数列表 + OVER(partition/order/frame)。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let schema = self.PhysicalSchemaProducer.SchemaRef();
        let output_start = schema.map_or(0, |schema| {
            schema
                .Columns
                .len()
                .saturating_sub(self.WindowFuncDescs.len())
        });
        let mut result = self
            .WindowFuncDescs
            .iter()
            .enumerate()
            .map(|(index, desc)| {
                let args = desc
                    .Args
                    .iter()
                    .map(|arg| {
                        arg.as_any()
                            .downcast_ref::<expression::Constant>()
                            .filter(|constant| constant.Value.Kind() == expression::types::KindNull)
                            .map_or_else(|| arg.ExplainInfo(eval), |_| "<nil>".to_owned())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let output = schema
                    .and_then(|schema| schema.Columns.get(output_start + index))
                    .map_or_else(String::new, |column| column.ExplainInfo(eval));
                format!("{}({})->{}", desc.Name, args, output)
            })
            .collect::<Vec<_>>()
            .join(", ");
        result.push_str(" over(");
        property::ExplainPartitionBy(eval, &mut result, &self.PartitionBy, false);
        if !self.OrderBy.is_empty() {
            if !self.PartitionBy.is_empty() {
                result.push(' ');
            }
            result.push_str("order by ");
            result.push_str(
                &self
                    .OrderBy
                    .iter()
                    .map(|item| {
                        format!(
                            "{}{}",
                            item.Col.ExplainInfo(eval),
                            if item.Desc { " desc" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if let Some(frame) = &self.Frame {
            if !self.PartitionBy.is_empty() || !self.OrderBy.is_empty() {
                result.push(' ');
            }
            result.push_str(if frame.Type == logicalop::FrameType::Rows {
                "rows between "
            } else {
                "range between "
            });
            if let Some(start) = &frame.Start {
                result.push_str(&self.format_bound(start));
            }
            result.push_str(" and ");
            if let Some(end) = &frame.End {
                result.push_str(&self.format_bound(end));
            }
        }
        result.push(')');
        let stream_count = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            result.push_str(&format!(", stream_count: {stream_count}"));
        }
        result
    }

    /// 按子 Schema 解析分区/排序/函数参数/窗框表达式下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(schema) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .map(|child| child.schema().Clone())
        else {
            return Ok(());
        };
        if let Some(output_schema) = self.PhysicalSchemaProducer.SchemaRef() {
            let mut resolved = output_schema.Clone();
            let passthrough_count = resolved
                .Columns
                .len()
                .saturating_sub(self.WindowFuncDescs.len());
            for column in resolved.Columns.iter_mut().take(passthrough_count) {
                *column = column.ResolveIndices(&schema)?;
            }
            self.PhysicalSchemaProducer.SetSchema(resolved);
        }
        for item in self.PartitionBy.iter_mut().chain(&mut self.OrderBy) {
            item.Col = item.Col.ResolveIndices(&schema)?;
        }
        for desc in &mut self.WindowFuncDescs {
            for arg in &mut desc.Args {
                *arg = arg.ResolveIndices(&schema)?;
            }
        }
        if let Some(frame) = &mut self.Frame {
            for bound in [&mut frame.Start, &mut frame.End].into_iter().flatten() {
                for expr in &mut bound.CalcFuncs {
                    *expr = expr.ResolveIndices(&schema)?;
                }
            }
        }
        Ok(())
    }

    /// 挂接到执行任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
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

    /// 序列化为 tipb::Window，并设置细粒度 Shuffle 参数。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let client = ctx
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let expression_context = ctx.GetExprCtx();
        let eval = expression_context.GetEvalCtx();
        let mut window = tipb::Window::new();
        let mut functions = Vec::with_capacity(self.WindowFuncDescs.len());
        for descriptor in &self.WindowFuncDescs {
            let descriptor = aggregation::NewWindowFuncDesc(
                expression_context.as_ref(),
                &descriptor.Name,
                descriptor.Args.clone(),
                true,
            )
            .map_err(|error| expression::errors::New(error.to_string()))?
            .ok_or_else(|| expression::errors::New("invalid window function descriptor"))?;
            functions.push(
                aggregation::WindowFuncToPBExpr(eval, client.as_ref(), &descriptor).ok_or_else(
                    || expression::errors::New("window function cannot be pushed down"),
                )?,
            );
        }
        window.set_func_desc(functions.into());
        window.set_partition_by(
            self.PartitionBy
                .iter()
                .map(|item| {
                    expression::SortByItemToPB(eval, client.as_ref(), &item.Col, item.Desc)
                        .ok_or_else(|| {
                            expression::errors::New("window partition item cannot be pushed down")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        );
        window.set_order_by(
            self.OrderBy
                .iter()
                .map(|item| {
                    expression::SortByItemToPB(eval, client.as_ref(), &item.Col, item.Desc)
                        .ok_or_else(|| {
                            expression::errors::New("window order item cannot be pushed down")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        );
        if let Some(frame) = &self.Frame {
            let mut pb_frame = tipb::WindowFrame::new();
            pb_frame.set_type(match frame.Type {
                logicalop::FrameType::Rows => tipb::WindowFrameType::Rows,
                logicalop::FrameType::Range => tipb::WindowFrameType::Ranges,
                logicalop::FrameType::Groups => tipb::WindowFrameType::Groups,
            });
            if let Some(bound) = &frame.Start {
                pb_frame.set_start(frame_bound_to_pb(bound, eval, client.as_ref())?);
            }
            if let Some(bound) = &frame.End {
                pb_frame.set_end(frame_bound_to_pb(bound, eval, client.as_ref())?);
            }
            window.set_frame(pb_frame);
        }
        let child = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .ok_or_else(|| expression::errors::New("window requires one child"))?
            .to_pb(ctx, store)?;
        window.set_child(*child);
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeWindow);
        executor.set_window(window);
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
        executor.set_fine_grained_shuffle_batch_size(ctx.TiFlashFineGrainedShuffleBatchSize);
        Ok(Box::new(executor))
    }
}

/// 将逻辑 FrameBound 转为 tipb::WindowFrameBound。
fn frame_bound_to_pb(
    bound: &logicalop::FrameBound,
    eval: &dyn expression::exprctx::EvalContext,
    client: &dyn kv::Client,
) -> Result<tipb::WindowFrameBound, expression::Error> {
    let mut result = tipb::WindowFrameBound::new();
    result.set_type(match bound.Type {
        logicalop::BoundType::Following => tipb::WindowBoundType::Following,
        logicalop::BoundType::Preceding => tipb::WindowBoundType::Preceding,
        logicalop::BoundType::CurrentRow => tipb::WindowBoundType::CurrentRow,
    });
    result.set_unbounded(bound.UnBounded);
    result.set_offset(bound.Num);
    if bound.IsExplicitRange {
        let expressions = expression::ExpressionsToPBList(eval, &bound.CalcFuncs, client)?;
        if let Some(expression) = expressions.into_iter().next() {
            result.set_frame_range(expression);
        }
    }
    Ok(result)
}

/// 检测窗口表达式是否含虚拟列或相关列（阻碍 TiFlash 下推）。
fn contains_virtual_window_expression(logical: &logicalop::LogicalWindow) -> bool {
    logical.WindowFuncDescs.iter().any(|descriptor| {
        expression::ContainVirtualColumn(&descriptor.Args)
            || expression::ContainCorrelatedColumn(&descriptor.Args)
    }) || logical
        .PartitionBy
        .iter()
        .chain(&logical.OrderBy)
        .any(|item| item.Col.VirtualExpr.is_some())
        || logical.Frame.iter().any(|frame| {
            [&frame.Start, &frame.End]
                .into_iter()
                .flatten()
                .any(|bound| {
                    expression::ContainVirtualColumn(&bound.CalcFuncs)
                        || expression::ContainCorrelatedColumn(&bound.CalcFuncs)
                        || expression::ContainVirtualColumn(&bound.CompareCols)
                        || expression::ContainCorrelatedColumn(&bound.CompareCols)
                })
        })
}

/// 枚举窗口物理实现：优先 TiFlash MPP，否则 Root + 有序子属性。
pub fn ExhaustPhysicalPlans4LogicalWindow(
    logical: &logicalop::LogicalWindow,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let mut plans = Vec::new();
    if ctx.GetSessionVars().IsMPPAllowed()
        && planner_util::ShouldCheckTiFlashPushDown(ctx.as_ref(), logicalop::GetHasTiFlash(Some(logical)))
        // TiFlash 窗口：需满足比较函数可下推、无虚拟/相关列、分区有序前缀等。
        && logical.checkComparisonForTiFlash()
        && !contains_virtual_window_expression(logical)
        && required.IsSortItemAllForPartition()
        && matches!(
            required.TaskTp,
            property::RootTaskType | property::MppTaskType
        )
        && required.MPPPartitionTp != property::BroadcastType
    {
        let mut mpp_child = property::PhysicalProperty::default();
        mpp_child.ExpectedCnt = f64::MAX;
        mpp_child.CanAddEnforcer = true;
        mpp_child.SortItems = logical
            .PartitionBy
            .iter()
            .chain(&logical.OrderBy)
            .map(property::SortItem::Clone)
            .collect();
        mpp_child.SortItemsForPartition = mpp_child
            .SortItems
            .iter()
            .map(property::SortItem::Clone)
            .collect();
        mpp_child.TaskTp = property::MppTaskType;
        mpp_child.CTEProducerStatus = required.CTEProducerStatus;
        if required.IsPrefix(&mpp_child) {
            let mut partition_columns = logical
                .GetPartitionKeys()
                .into_iter()
                .map(|column| {
                    let collate = column.RetType.as_ref().map_or_else(
                        || "binary".to_owned(),
                        |field| field.GetCollate().to_owned(),
                    );
                    property::MPPPartitionColumn {
                        Col: column,
                        CollateID: property::GetCollateIDByNameForPartition(&collate),
                    }
                })
                .collect::<Vec<_>>();
            if required.MPPPartitionTp == property::HashType {
                if let Some(matches) = required.IsSubsetOf(&partition_columns) {
                    partition_columns = property::ChoosePartitionKeys(&partition_columns, &matches);
                } else {
                    partition_columns.clear();
                }
            }
            if !logical.PartitionBy.is_empty() && !partition_columns.is_empty() {
                mpp_child.MPPPartitionTp = property::HashType;
                mpp_child.MPPPartitionCols = partition_columns;
            } else if logical.PartitionBy.is_empty() {
                mpp_child.MPPPartitionTp = property::SinglePartitionType;
            }
            if !(required.MPPPartitionTp == property::SinglePartitionType
                && mpp_child.MPPPartitionTp != property::SinglePartitionType)
                && (logical.PartitionBy.is_empty() || !mpp_child.MPPPartitionCols.is_empty())
            {
                let stats = logical
                    .StatsInfo()
                    .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
                    .unwrap_or_default();
                let mut window = PhysicalWindow::New(ctx.clone());
                window.WindowFuncDescs = logical.WindowFuncDescs.clone();
                window.PartitionBy = logical
                    .PartitionBy
                    .iter()
                    .map(property::SortItem::Clone)
                    .collect();
                window.OrderBy = logical
                    .OrderBy
                    .iter()
                    .map(property::SortItem::Clone)
                    .collect();
                window.Frame = logical.Frame.clone();
                window.StoreTp = kv::StoreType::TiFlash;
                window
                    .PhysicalSchemaProducer
                    .SetSchema(logical.Schema().Clone());
                plans.push(Box::new(window.Init(
                    ctx.clone(),
                    stats,
                    logical.QueryBlockOffset(),
                    mpp_child,
                )) as Box<dyn PhysicalPlan>);
            }
        }
    }
    if required.TaskTp == property::MppTaskType
        || (ctx.GetSessionVars().IsMPPEnforced() && !plans.is_empty())
    {
        // 仅要 MPP 任务，或强制 MPP 且已有合法 TiFlash 实现时，不再枚举
        // Root/Cop 窗口，避免代价比较退回非 MPP 输入。
        return plans;
    }
    let mut child = property::PhysicalProperty::default();
    child.ExpectedCnt = f64::MAX;
    child.SortItems = logical
        .PartitionBy
        .iter()
        .chain(&logical.OrderBy)
        .map(property::SortItem::Clone)
        .collect();
    child.CanAddEnforcer = true;
    child.CTEProducerStatus = required.CTEProducerStatus;
    child.NoCopPushDown = required.NoCopPushDown;
    if !required.IsPrefix(&child) {
        // 父要求的有序属性不是子属性前缀时无法直接挂接。
        return Vec::new();
    }
    let stats = logical
        .StatsInfo()
        .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
        .unwrap_or_default();
    let mut window = PhysicalWindow::New(ctx.clone());
    window.WindowFuncDescs = logical.WindowFuncDescs.clone();
    window.PartitionBy = logical
        .PartitionBy
        .iter()
        .map(property::SortItem::Clone)
        .collect();
    window.OrderBy = logical
        .OrderBy
        .iter()
        .map(property::SortItem::Clone)
        .collect();
    window.Frame = logical.Frame.clone();
    window
        .PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    plans.push(Box::new(window.Init(
        ctx,
        stats,
        logical.QueryBlockOffset(),
        child,
    )));
    plans
}
