// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Cascades 旧版实现规则：把逻辑 GroupExpr 转为带孩子物理属性的 Implementation 候选。
//
// 对应 Go `implementation_rules.go`。每条 `ImplementationRule` 先 `Match` 所需物理属性，
// 再 `OnImplement` 生成物理算子并设置孩子 `PhysicalProperty`；规则顺序与 Go 默认表一致。

// Go source: pkg/planner/cascades/old/implementation_rules.go.
// The rule order and physical-property propagation remain identical to the legacy cascades planner.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use astersql_expression::{self as expression, Schema};
use astersql_meta_model as model;
use astersql_planner_cardinality as cardinality;
use astersql_planner_cascades_pattern::*;
use astersql_planner_core_base as base;
use astersql_planner_core_base::{ContextRef, PhysicalPlan, Plan};
use astersql_planner_core_operator_logicalop::{self as logicalop, *};
use astersql_planner_core_operator_physicalop::*;
use astersql_planner_implementation::*;
use astersql_planner_memo::{self as memo, GroupExpr, GroupRef, ImplementationRef};
use astersql_planner_property::{PhysicalProperty, SortItem, StatsInfo};
use astersql_sessionctx_vardef as vardef;
use astersql_statistics::HistColl;

type RuleResult = logicalop::Result<Vec<ImplementationRef>>;

/// 把具体 Implementation 包成 Rc<RefCell<...>> 引用。
fn implementation_ref(value: impl memo::Implementation + 'static) -> ImplementationRef {
    Rc::new(RefCell::new(value))
}

/// 从 GroupExpr 升级得到所属 Group；已脱离则报错。
fn group_ref(expression: &GroupExpr) -> logicalop::Result<GroupRef> {
    expression
        .Group
        .upgrade()
        .ok_or_else(|| PlannerError("memo expression is detached from its group".to_owned()))
}

/// 把 ExprNode 向下转型为指定逻辑算子类型。
fn logical<T: 'static>(expression: &GroupExpr) -> logicalop::Result<&T> {
    expression
        .ExprNode
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| {
            PlannerError(format!(
                "unexpected logical operand {}",
                expression.ExprNode.TP()
            ))
        })
}

/// 取出逻辑计划上的 PlanContext。
fn context(plan: &dyn logicalop::LogicalPlan) -> logicalop::Result<ContextRef> {
    plan.SCtx()
        .cloned()
        .ok_or_else(|| PlannerError(format!("{} has no planner context", plan.TP())))
}

/// 把 PlanContext 适配为基数估计所需的 CardinalityContext。
struct CardinalityAdapter<'a>(&'a dyn base::PlanContext);

impl cardinality::CardinalityContext for CardinalityAdapter<'_> {
    fn GetSessionVars(&self) -> &astersql_planner_planctx::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &astersql_planner_planctx::rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

/// 读取表达式所属 Group 的统计、Schema 与执行引擎类型。
fn group_properties(expr: &GroupExpr) -> logicalop::Result<(StatsInfo, Schema, EngineType)> {
    let group = group_ref(expr)?;
    let group = group.borrow();
    let stats = group
        .Prop
        .Stats
        .as_deref()
        .cloned()
        .ok_or_else(|| PlannerError("memo group has no statistics".to_owned()))?;
    let schema = group
        .Prop
        .Schema
        .as_deref()
        .map(Schema::Clone)
        .ok_or_else(|| PlannerError("memo group has no schema".to_owned()))?;
    Ok((stats, schema, group.EngineType))
}

/// 读取第 index 个孩子 Group 的统计与 Schema。
fn child_properties(expr: &GroupExpr, index: usize) -> logicalop::Result<(StatsInfo, Schema)> {
    let child = expr
        .Children
        .get(index)
        .ok_or_else(|| PlannerError(format!("memo expression has no child {index}")))?
        .borrow();
    let stats = child
        .Prop
        .Stats
        .as_deref()
        .cloned()
        .ok_or_else(|| PlannerError(format!("memo child {index} has no statistics")))?;
    let schema = child
        .Prop
        .Schema
        .as_deref()
        .map(Schema::Clone)
        .ok_or_else(|| PlannerError(format!("memo child {index} has no schema")))?;
    Ok((stats, schema))
}

/// 构造 ExpectedCnt 为无穷大、无排序项的物理属性。
fn max_count_property() -> PhysicalProperty {
    let mut property = PhysicalProperty::default();
    property.ExpectedCnt = f64::MAX;
    property
}

/// 按父期望行数缩放统计（ScaleByExpectCnt）。
fn scaled_stats(stats: &StatsInfo, ctx: &ContextRef, expected_count: f64) -> StatsInfo {
    stats.ScaleByExpectCnt(ctx.GetSessionVars(), expected_count)
}

/// 从 StatsInfo 取出 HistColl（直方图集合）；缺失则报错。
fn histograms(stats: &StatsInfo) -> logicalop::Result<HistColl> {
    stats
        .HistColl
        .as_deref()
        .and_then(|histograms| histograms.downcast_ref::<HistColl>())
        .cloned()
        .ok_or_else(|| PlannerError("logical data source has no HistColl statistics".to_owned()))
}

/// 读取系统变量为 f64，解析失败时用默认值。
fn sys_f64(plan: &dyn PhysicalPlan, name: &str, default: f64) -> f64 {
    plan.s_ctx()
        .GetSessionVars()
        .GetSystemVar(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// 读取系统变量为 i64，解析失败时用默认值。
fn sys_i64(plan: &dyn PhysicalPlan, name: &str, default: i64) -> i64 {
    plan.s_ctx()
        .GetSessionVars()
        .GetSystemVar(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// 解析执行器并发度：若为 ConcurrencyUnset 则回退到 TiDBExecutorConcurrency。
fn executor_concurrency(plan: &dyn PhysicalPlan, name: &str, default: i64) -> usize {
    let configured = sys_i64(plan, name, default);
    let concurrency = if configured == vardef::ConcurrencyUnset {
        sys_i64(
            plan,
            vardef::TiDBExecutorConcurrency,
            vardef::DefExecutorConcurrency,
        )
    } else {
        configured
    };
    concurrency.max(1) as usize
}

/// 解析 HashJoin 并发度，逻辑同 executor_concurrency。
fn hash_join_concurrency(ctx: &ContextRef) -> u64 {
    let vars = ctx.GetSessionVars();
    let configured = vars
        .GetSystemVar(vardef::TiDBHashJoinConcurrency)
        .and_then(|value| value.parse().ok())
        .unwrap_or(vardef::DefTiDBHashJoinConcurrency);
    let concurrency = if configured == vardef::ConcurrencyUnset {
        vars.GetSystemVar(vardef::TiDBExecutorConcurrency)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefExecutorConcurrency)
    } else {
        configured
    };
    concurrency.max(1) as u64
}

/// 临时表网络/扫描因子置 0，普通表返回原值。
fn table_factor(table: &model::TableInfo, value: f64) -> f64 {
    if table.TempTableType == model::TempTableNone {
        value
    } else {
        0.0
    }
}

/// 把具体物理算子适配为 PlanAccess / 各代价 trait。
struct PlanAdapter<T: PhysicalPlan> {
    plan: T,
}

impl<T: PhysicalPlan> PlanAccess for PlanAdapter<T> {
    fn Plan(&self) -> &dyn PhysicalPlan {
        &self.plan
    }
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan {
        &mut self.plan
    }
}

impl ProjectionCostPlan for PlanAdapter<PhysicalProjection> {
    fn SelfCost(&self, input_rows: f64) -> f64 {
        let cpu = self.plan.s_ctx().GetSessionVars().GetCPUFactor();
        let concurrency = executor_concurrency(
            &self.plan,
            vardef::TiDBProjectionConcurrency,
            vardef::DefTiDBProjectionConcurrency,
        ) as f64;
        if concurrency <= 0.0 {
            input_rows * cpu
        } else {
            input_rows * cpu / concurrency
                + (1.0 + concurrency)
                    * sys_f64(
                        &self.plan,
                        vardef::TiDBOptConcurrencyFactor,
                        vardef::DefOptConcurrencyFactor,
                    )
        }
    }
}

impl SelectionCostPlan for PlanAdapter<PhysicalSelection> {
    fn CPUFactor(&self, coprocessor: bool) -> f64 {
        if coprocessor {
            self.plan.s_ctx().GetSessionVars().GetCopCPUFactor()
        } else {
            self.plan.s_ctx().GetSessionVars().GetCPUFactor()
        }
    }
}

impl HashAggCostPlan for PlanAdapter<PhysicalHashAgg> {
    fn SelfCost(&self, input_rows: f64, root: bool) -> f64 {
        self.plan.GetCost(input_rows, root, false, 0)
    }
}

impl TopNCostPlan for PlanAdapter<PhysicalTopN> {
    fn SelfCost(&self, input_rows: f64, root: bool) -> f64 {
        self.plan.GetCost(input_rows, root)
    }
}

impl UnionAllCostPlan for PlanAdapter<PhysicalUnionAll> {
    fn ConcurrencyFactor(&self) -> f64 {
        sys_f64(
            &self.plan,
            vardef::TiDBOptConcurrencyFactor,
            vardef::DefOptConcurrencyFactor,
        )
    }
}

impl ApplyCostPlan for PlanAdapter<PhysicalApply> {
    fn SelfCost(&self, left: f64, right: f64, left_cost: f64, right_cost: f64) -> f64 {
        self.plan.GetCost(left, right, left_cost, right_cost)
    }
    fn HasLeftConditions(&self) -> bool {
        !self
            .plan
            .PhysicalHashJoin
            .BasePhysicalJoin
            .LeftConditions
            .is_empty()
    }
}

impl BinaryJoinCostPlan for PlanAdapter<PhysicalHashJoin> {
    fn SelfCost(&self, left_rows: f64, right_rows: f64) -> f64 {
        self.plan.GetCost(left_rows, right_rows, false, 0)
    }
}

impl BinaryJoinCostPlan for PlanAdapter<PhysicalMergeJoin> {
    fn SelfCost(&self, left_rows: f64, right_rows: f64) -> f64 {
        self.plan.GetCost(left_rows, right_rows, 0)
    }
}

impl SortCostPlan for PlanAdapter<PhysicalSort> {
    fn ExpectedCount(&self) -> f64 {
        self.plan.get_child_req_props(0).ExpectedCnt
    }
    fn SelfCost(&self, input_rows: f64, schema: &Schema) -> f64 {
        self.plan.GetCost(input_rows, schema)
    }
    fn InjectProjectionBelowSort(&mut self, child: Box<dyn PhysicalPlan>) -> Box<dyn PhysicalPlan> {
        self.plan.set_children(vec![child]);
        self.plan
            .clone_physical(self.plan.s_ctx().clone())
            .expect("physical sort must be cloneable")
    }
}

impl ReaderCostPlan for PlanAdapter<PhysicalTableReader> {
    fn NetworkFactor(&self, table: &model::TableInfo) -> f64 {
        table_factor(
            table,
            sys_f64(
                &self.plan,
                vardef::TiDBOptNetworkFactor,
                vardef::DefOptNetworkFactor,
            ),
        )
    }
    fn AverageRowSize(&self, histograms: &HistColl, child: &dyn PhysicalPlan, index: bool) -> f64 {
        let columns = child.schema().Columns.iter().collect::<Vec<_>>();
        cardinality::GetAvgRowSize(
            &CardinalityAdapter(self.plan.s_ctx().as_ref()),
            histograms,
            &columns,
            index,
            false,
        )
    }
    fn CopIteratorWorkers(&self) -> usize {
        sys_i64(
            &self.plan,
            vardef::TiDBDistSQLScanConcurrency,
            vardef::DefDistSQLScanConcurrency,
        )
        .max(1) as usize
    }
}

impl ReaderCostPlan for PlanAdapter<PhysicalIndexReader> {
    fn NetworkFactor(&self, table: &model::TableInfo) -> f64 {
        table_factor(
            table,
            sys_f64(
                &self.plan,
                vardef::TiDBOptNetworkFactor,
                vardef::DefOptNetworkFactor,
            ),
        )
    }
    fn AverageRowSize(&self, histograms: &HistColl, child: &dyn PhysicalPlan, index: bool) -> f64 {
        let columns = child.schema().Columns.iter().collect::<Vec<_>>();
        cardinality::GetAvgRowSize(
            &CardinalityAdapter(self.plan.s_ctx().as_ref()),
            histograms,
            &columns,
            index,
            false,
        )
    }
    fn CopIteratorWorkers(&self) -> usize {
        sys_i64(
            &self.plan,
            vardef::TiDBDistSQLScanConcurrency,
            vardef::DefDistSQLScanConcurrency,
        )
        .max(1) as usize
    }
}

impl TableScanCostPlan for PlanAdapter<PhysicalTableScan> {
    fn AverageRowSize(&self, histograms: &HistColl, columns: &[expression::Column]) -> f64 {
        let columns = columns.iter().collect::<Vec<_>>();
        cardinality::GetTableAvgRowSize(
            &CardinalityAdapter(self.plan.s_ctx().as_ref()),
            histograms,
            &columns,
            astersql_kv::StoreType::TiKV,
            true,
        )
    }
    fn ScanFactor(&self) -> f64 {
        self.plan.Table.as_ref().map_or_else(
            || {
                sys_f64(
                    &self.plan,
                    vardef::TiDBOptScanFactor,
                    vardef::DefOptScanFactor,
                )
            },
            |table| {
                table_factor(
                    table,
                    sys_f64(
                        &self.plan,
                        vardef::TiDBOptScanFactor,
                        vardef::DefOptScanFactor,
                    ),
                )
            },
        )
    }
    fn DescScanFactor(&self) -> f64 {
        self.plan.Table.as_ref().map_or_else(
            || {
                sys_f64(
                    &self.plan,
                    vardef::TiDBOptDescScanFactor,
                    vardef::DefOptDescScanFactor,
                )
            },
            |table| {
                table_factor(
                    table,
                    sys_f64(
                        &self.plan,
                        vardef::TiDBOptDescScanFactor,
                        vardef::DefOptDescScanFactor,
                    ),
                )
            },
        )
    }
    fn Descending(&self) -> bool {
        self.plan.Desc
    }
}

impl IndexScanCostPlan for PlanAdapter<PhysicalIndexScan> {
    fn AverageRowSize(&self, histograms: &HistColl) -> f64 {
        let columns = self.plan.schema().Columns.iter().collect::<Vec<_>>();
        cardinality::GetIndexAvgRowSize(
            &CardinalityAdapter(self.plan.s_ctx().as_ref()),
            histograms,
            &columns,
            self.plan.Index.as_ref().is_some_and(|index| index.Unique),
        )
    }
    fn ScanFactor(&self) -> f64 {
        sys_f64(
            &self.plan,
            vardef::TiDBOptScanFactor,
            vardef::DefOptScanFactor,
        )
    }
    fn DescScanFactor(&self) -> f64 {
        sys_f64(
            &self.plan,
            vardef::TiDBOptDescScanFactor,
            vardef::DefOptDescScanFactor,
        )
    }
    fn SeekFactor(&self) -> f64 {
        sys_f64(
            &self.plan,
            vardef::TiDBOptSeekFactor,
            vardef::DefOptSeekFactor,
        )
    }
    fn Descending(&self) -> bool {
        self.plan.Desc
    }
    fn RangeCount(&self) -> usize {
        self.plan.Ranges.len()
    }
}

/// ImplementationRule 对应 Go 接口：先按所需物理属性匹配，再生成带孩子属性的物理实现候选。
pub trait ImplementationRule {
    #[allow(non_snake_case)]
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool;

    #[allow(non_snake_case)]
    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult;
}

/// defaultImplementationMap 对应 Go 包级规则表，并保持各 Operand 下的候选顺序。
#[allow(non_snake_case)]
pub fn defaultImplementationMap() -> HashMap<Operand, Vec<Box<dyn ImplementationRule>>> {
    HashMap::from([
        (
            Operand::TableDual,
            vec![Box::new(ImplTableDual) as Box<dyn ImplementationRule>],
        ),
        (Operand::MemTableScan, vec![Box::new(ImplMemTableScan)]),
        (Operand::Projection, vec![Box::new(ImplProjection)]),
        (Operand::TableScan, vec![Box::new(ImplTableScan)]),
        (Operand::IndexScan, vec![Box::new(ImplIndexScan)]),
        (
            Operand::TiKVSingleGather,
            vec![Box::new(ImplTiKVSingleReadGather)],
        ),
        (Operand::Show, vec![Box::new(ImplShow)]),
        (Operand::Selection, vec![Box::new(ImplSelection)]),
        (Operand::Sort, vec![Box::new(ImplSort)]),
        (Operand::Aggregation, vec![Box::new(ImplHashAgg)]),
        (Operand::Limit, vec![Box::new(ImplLimit)]),
        (
            Operand::TopN,
            vec![Box::new(ImplTopN), Box::new(ImplTopNAsLimit)],
        ),
        (
            Operand::Join,
            vec![
                Box::new(ImplHashJoinBuildLeft),
                Box::new(ImplHashJoinBuildRight),
                Box::new(ImplMergeJoin),
            ],
        ),
        (Operand::UnionAll, vec![Box::new(ImplUnionAll)]),
        (Operand::Apply, vec![Box::new(ImplApply)]),
        (Operand::MaxOneRow, vec![Box::new(ImplMaxOneRow)]),
        (Operand::Window, vec![Box::new(ImplWindow)]),
    ])
}

/// ImplTableDual 把 LogicalTableDual 转换成 PhysicalTableDual。
pub struct ImplTableDual;
impl ImplementationRule for ImplTableDual {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let logic = logical::<LogicalTableDual>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(logic)?;
        let mut physical = PhysicalTableDual::New(ctx.clone(), logic.RowCount).Init(
            ctx,
            stats,
            logic.QueryBlockOffset(),
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewTableDualImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplMemTableScan 把内存表逻辑扫描的元信息、列和抽取器复制到物理扫描。
pub struct ImplMemTableScan;
impl ImplementationRule for ImplMemTableScan {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let logic = logical::<LogicalMemTable>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(logic)?;
        let mut physical = PhysicalMemTable::New(ctx.clone());
        physical.DBName = logic.DBName.clone();
        physical.Table = logic.TableInfo.clone();
        physical.Columns = logic.Columns.clone();
        physical.Extractor = logic.Extractor.clone();
        let mut physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            logic.QueryBlockOffset(),
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewMemTableScanImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplProjection 通过 TryToGetChildProp 把父属性下推到投影孩子。
pub struct ImplProjection;
impl ImplementationRule for ImplProjection {
    fn Match(&self, _expr: &GroupExpr, _prop: &PhysicalProperty) -> bool {
        true
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let logic = logical::<LogicalProjection>(expr)?;
        let (child_prop, usable) = logic.TryToGetChildProp(req_prop);
        let (Some(child_prop), true) = (child_prop, usable) else {
            // 属性涉及无法穿过投影的表达式时，Go 返回 nil, nil 表示本规则无候选。
            return Ok(Vec::new());
        };
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(logic)?;
        let mut physical = PhysicalProjection::New(ctx.clone());
        physical.Exprs = logic.Exprs.iter().map(|item| item.CloneExpr()).collect();
        physical.CalculateNoDelay = logic.CalculateNoDelay;
        let mut physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            logic.QueryBlockOffset(),
            vec![Box::new(child_prop)],
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewProjectionImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplTiKVSingleReadGather 根据 IsIndexGather 选择 IndexReader 或 TableReader。
pub struct ImplTiKVSingleReadGather;
impl ImplementationRule for ImplTiKVSingleReadGather {
    fn Match(&self, _expr: &GroupExpr, _prop: &PhysicalProperty) -> bool {
        true
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let gather = logical::<TiKVSingleGather>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(gather)?;
        let source = gather
            .Source
            .as_ref()
            .ok_or_else(|| PlannerError("TiKVSingleGather has no data source".to_owned()))?
            .borrow();
        let table = source.TableInfo.clone();
        let histograms = histograms(&source.TableStats)?;
        let child_props = vec![Box::new(req_prop.CloneEssentialFields())];
        if gather.IsIndexGather {
            let reader = GetPhysicalIndexReader(
                ctx.clone(),
                schema,
                scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
                child_props,
            );
            return Ok(vec![implementation_ref(NewIndexReaderImpl(
                Box::new(PlanAdapter { plan: reader }),
                table,
                histograms,
            ))]);
        }
        let reader = GetPhysicalTableReader(
            ctx.clone(),
            schema,
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            child_props,
        );
        Ok(vec![implementation_ref(NewTableReaderImpl(
            Box::new(PlanAdapter { plan: reader }),
            table,
            histograms,
        ))])
    }
}

/// ImplTableScan 在无序要求或仅要求 Handle 列顺序时生成 PhysicalTableScan。
pub struct ImplTableScan;
impl ImplementationRule for ImplTableScan {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        let Ok(scan) = logical::<LogicalTableScan>(expr) else {
            return false;
        };
        prop.IsSortItemEmpty()
            || (prop.SortItems.len() == 1
                && scan.HandleCols.as_ref().is_some_and(|handle| {
                    handle
                        .GetCol(0)
                        .is_some_and(|column| prop.SortItems[0].Col.EqualColumn(column))
                }))
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let scan = logical::<LogicalTableScan>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(scan)?;
        let source = scan
            .Source
            .as_ref()
            .ok_or_else(|| PlannerError("LogicalTableScan has no data source".to_owned()))?
            .borrow();
        let columns = source.TblCols.clone();
        let histograms = histograms(&source.TableStats)?;
        let mut physical = GetPhysicalScan4LogicalTableScan(
            ctx.clone(),
            schema,
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
        );
        physical.Table = Some(source.TableInfo.clone());
        physical.Columns = source.Columns.clone();
        physical.Ranges = astersql_util_ranger::Ranges(scan.Ranges.clone());
        if !req_prop.IsSortItemEmpty() {
            physical.KeepOrder = true;
            physical.Desc = req_prop.SortItems[0].Desc;
        }
        Ok(vec![implementation_ref(NewTableScanImpl(
            Box::new(PlanAdapter { plan: physical }),
            columns,
            histograms,
        ))])
    }
}

/// ImplIndexScan 复用 LogicalIndexScan 的 MatchIndexProp 判定索引是否可提供所需顺序。
pub struct ImplIndexScan;
impl ImplementationRule for ImplIndexScan {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        logical::<LogicalIndexScan>(expr).is_ok_and(|scan| scan.MatchIndexProp(prop))
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let scan = logical::<LogicalIndexScan>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(scan)?;
        let source = scan
            .Source
            .as_ref()
            .ok_or_else(|| PlannerError("LogicalIndexScan has no data source".to_owned()))?
            .borrow();
        let histograms = histograms(&source.TableStats)?;
        let mut physical = GetPhysicalIndexScan4LogicalIndexScan(
            ctx.clone(),
            schema,
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
        );
        physical.Table = Some(source.TableInfo.clone());
        physical.Index = Some(scan.Index.clone());
        physical.Columns = scan.Columns.clone();
        physical.Ranges = astersql_util_ranger::Ranges(scan.Ranges.clone());
        if !req_prop.IsSortItemEmpty() {
            physical.KeepOrder = true;
            physical.Desc = req_prop.SortItems[0].Desc;
        }
        Ok(vec![implementation_ref(NewIndexScanImpl(
            Box::new(PlanAdapter { plan: physical }),
            histograms,
        ))])
    }
}

/// ImplShow 仅接受空排序属性，并复制 ShowContents 与 Extractor。
pub struct ImplShow;
impl ImplementationRule for ImplShow {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let show = logical::<LogicalShow>(expr)?;
        let (_, schema, _) = group_properties(expr)?;
        let ctx = context(show)?;
        // Go 注释指出未来可合并 LogicalShow/PhysicalShow，以减少运行时 GC 压力。
        let mut physical = PhysicalShow::New(ctx.clone());
        physical.ShowContents = show.ShowContents.clone();
        physical.Extractor = show.Extractor.clone();
        let mut physical = physical.Init(ctx);
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewShowImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplSelection 按 Group.EngineType 选择 TiDB 或 TiKV Selection 实现。
pub struct ImplSelection;
impl ImplementationRule for ImplSelection {
    fn Match(&self, _expr: &GroupExpr, _prop: &PhysicalProperty) -> bool {
        true
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let selection = logical::<LogicalSelection>(expr)?;
        let (stats, _, engine) = group_properties(expr)?;
        let ctx = context(selection)?;
        let mut physical = PhysicalSelection::New(ctx.clone());
        physical.Conditions = selection
            .Conditions
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect();
        let physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            selection.QueryBlockOffset(),
            vec![Box::new(req_prop.CloneEssentialFields())],
        );
        match engine {
            EngineTiDB => Ok(vec![implementation_ref(NewTiDBSelectionImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            EngineTiKV => Ok(vec![implementation_ref(NewTiKVSelectionImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            _ => Err(PlannerError(format!(
                "Unsupported EngineType '{}' for Selection.",
                engine
            ))),
        }
    }
}

/// ImplSort 在排序项全为列时生成 NominalSort，否则生成真正的 PhysicalSort。
pub struct ImplSort;
impl ImplementationRule for ImplSort {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        logical::<LogicalSort>(expr).is_ok_and(|sort| MatchItems(prop, &sort.ByItems))
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let sort = logical::<LogicalSort>(expr)?;
        let (stats, _, _) = group_properties(expr)?;
        let ctx = context(sort)?;
        if let Some(mut child_prop) = GetPropByOrderByItems(&sort.ByItems) {
            child_prop.ExpectedCnt = req_prop.ExpectedCnt;
            let nominal = NominalSort::New(ctx.clone()).Init(
                ctx.clone(),
                scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
                sort.QueryBlockOffset(),
                vec![Box::new(child_prop)],
            );
            return Ok(vec![implementation_ref(NewNominalSortImpl(Box::new(
                PlanAdapter { plan: nominal },
            )))]);
        }
        let mut physical = PhysicalSort::New(ctx.clone());
        physical.ByItems = sort.ByItems.iter().map(|item| item.Clone()).collect();
        let physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            sort.QueryBlockOffset(),
            vec![Box::new(max_count_property())],
        );
        Ok(vec![implementation_ref(NewSortImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplHashAgg 把 LogicalAggregation 转换成哈希聚合，并按执行引擎选择实现包装。
pub struct ImplHashAgg;
impl ImplementationRule for ImplHashAgg {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        // Go 尚未实现 StreamAgg，因此这里只接受空排序属性。
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let agg = logical::<LogicalAggregation>(expr)?;
        let (stats, schema, engine) = group_properties(expr)?;
        let ctx = context(agg)?;
        let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            ctx.clone(),
            "HashAgg",
            agg.QueryBlockOffset(),
        ));
        let physical =
            NewPhysicalHashAgg(agg, producer).map_err(|error| PlannerError(error.to_string()))?;
        let physical = physical.BasePhysicalAgg.InitForHash(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            agg.QueryBlockOffset(),
            schema,
            vec![Box::new(max_count_property())],
        );
        match engine {
            EngineTiDB => Ok(vec![implementation_ref(NewTiDBHashAggImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            EngineTiKV => Ok(vec![implementation_ref(NewTiKVHashAggImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            _ => Err(PlannerError(format!(
                "Unsupported EngineType '{}' for HashAggregation.",
                engine
            ))),
        }
    }
}

/// ImplLimit 让孩子最多产生 Count + Offset 行，再由 PhysicalLimit 截取结果。
pub struct ImplLimit;
impl ImplementationRule for ImplLimit {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let limit = logical::<LogicalLimit>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(limit)?;
        let mut child_prop = PhysicalProperty::default();
        child_prop.ExpectedCnt = limit.Count.wrapping_add(limit.Offset) as f64;
        let mut physical = PhysicalLimit::New(ctx.clone(), limit.Offset, limit.Count).Init(
            ctx,
            stats,
            limit.QueryBlockOffset(),
            vec![Box::new(child_prop)],
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewLimitImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplTopN 生成真正的 PhysicalTopN；非 TiDB 引擎只接受无序父属性。
pub struct ImplTopN;
impl ImplementationRule for ImplTopN {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        let Ok(topn) = logical::<LogicalTopN>(expr) else {
            return false;
        };
        let Ok((_, _, engine)) = group_properties(expr) else {
            return false;
        };
        if engine != EngineTiDB {
            return prop.IsSortItemEmpty();
        }
        MatchItems(prop, &topn.ByItems)
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let topn = logical::<LogicalTopN>(expr)?;
        let (stats, _, engine) = group_properties(expr)?;
        let ctx = context(topn)?;
        let mut physical = PhysicalTopN::New(ctx.clone(), topn.Offset, topn.Count);
        physical.ByItems = topn.ByItems.iter().map(|item| item.Clone()).collect();
        let physical = physical.Init(
            ctx,
            stats,
            topn.QueryBlockOffset(),
            vec![Box::new(max_count_property())],
        );
        match engine {
            EngineTiDB => Ok(vec![implementation_ref(NewTiDBTopNImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            EngineTiKV => Ok(vec![implementation_ref(NewTiKVTopNImpl(Box::new(
                PlanAdapter { plan: physical },
            )))]),
            _ => Err(PlannerError(format!(
                "Unsupported EngineType '{}' for TopN.",
                engine
            ))),
        }
    }
}

/// ImplTopNAsLimit 在排序可由孩子提供时，把 TopN 降为带顺序属性的 PhysicalLimit。
pub struct ImplTopNAsLimit;
impl ImplementationRule for ImplTopNAsLimit {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        let Ok(topn) = logical::<LogicalTopN>(expr) else {
            return false;
        };
        GetPropByOrderByItems(&topn.ByItems).is_some() && MatchItems(prop, &topn.ByItems)
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let topn = logical::<LogicalTopN>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(topn)?;
        let mut child_prop = PhysicalProperty::default();
        child_prop.ExpectedCnt = topn.Count.wrapping_add(topn.Offset) as f64;
        for item in &topn.ByItems {
            // Match 已保证排序表达式可作为列处理；Go 在此执行 *expression.Column 类型断言。
            let column = item
                .Expr
                .as_any()
                .downcast_ref::<expression::Column>()
                .ok_or_else(|| PlannerError("TopN order item is not a column".to_owned()))?;
            child_prop.SortItems.push(SortItem {
                Col: column.Clone(),
                Desc: item.Desc,
            });
        }
        let mut physical = PhysicalLimit::New(ctx.clone(), topn.Offset, topn.Count).Init(
            ctx,
            stats,
            topn.QueryBlockOffset(),
            vec![Box::new(child_prop)],
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewLimitImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// getImplForHashJoin 构造 HashJoin，并按父 ExpectedCnt 等比缩小外侧孩子的期望行数。
#[allow(non_snake_case)]
fn getImplForHashJoin(
    expr: &GroupExpr,
    prop: &PhysicalProperty,
    inner_idx: usize,
    use_outer_to_build: bool,
) -> logicalop::Result<ImplementationRef> {
    let join = logical::<LogicalJoin>(expr)?;
    let (stats, schema, _) = group_properties(expr)?;
    let ctx = context(join)?;
    let mut child_props = vec![max_count_property(), max_count_property()];
    if prop.ExpectedCnt < stats.RowCount {
        let scale = prop.ExpectedCnt / stats.RowCount;
        child_props[1 - inner_idx].ExpectedCnt =
            child_properties(expr, 1 - inner_idx)?.0.RowCount * scale;
    }
    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
        ctx.clone(),
        "HashJoin",
        join.QueryBlockOffset(),
    ));
    let merge = GetMergeJoin(join, producer);
    let mut physical = NewPhysicalHashJoin(
        merge.BasePhysicalJoin,
        hash_join_concurrency(&ctx),
        use_outer_to_build,
    );
    physical.BasePhysicalJoin.InnerChildIdx = inner_idx;
    physical.EqualConditions = join
        .EqualConditions
        .iter()
        .filter_map(|condition| {
            condition
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .map(expression::ScalarFunction::clone_scalar)
        })
        .collect();
    physical.NAEqualConditions = join
        .NAEQConditions
        .iter()
        .filter_map(|condition| {
            condition
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .map(expression::ScalarFunction::clone_scalar)
        })
        .collect();
    let mut physical = physical.Init(
        ctx.clone(),
        scaled_stats(&stats, &ctx, prop.ExpectedCnt),
        join.QueryBlockOffset(),
        child_props.into_iter().map(Box::new).collect(),
    );
    physical
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(schema);
    Ok(implementation_ref(NewHashJoinImpl(Box::new(PlanAdapter {
        plan: physical,
    }))))
}

/// 从 LogicalApply 构造底层 PhysicalHashJoin（供 ImplApply 包装）。
#[allow(non_snake_case)]
fn GetHashJoin(
    _group: Option<&GroupExpr>,
    apply: &LogicalApply,
    _required: &PhysicalProperty,
) -> logicalop::Result<PhysicalHashJoin> {
    let context = context(apply)?;
    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
        context.clone(),
        "HashJoin",
        apply.QueryBlockOffset(),
    ));
    let merge = GetMergeJoin(&apply.LogicalJoin, producer);
    Ok(NewPhysicalHashJoin(
        merge.BasePhysicalJoin,
        hash_join_concurrency(&context),
        false,
    ))
}

/// ImplHashJoinBuildLeft 使用左孩子建哈希表，并按 JoinType 保留 Go 对外连接的特殊 innerIdx。
pub struct ImplHashJoinBuildLeft;
impl ImplementationRule for ImplHashJoinBuildLeft {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        let Ok(join) = logical::<LogicalJoin>(expr) else {
            return false;
        };
        matches!(
            join.JoinType,
            JoinType::InnerJoin | JoinType::LeftOuterJoin | JoinType::RightOuterJoin
        ) && prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let implementation = match logical::<LogicalJoin>(expr)?.JoinType {
            JoinType::InnerJoin => getImplForHashJoin(expr, req_prop, 0, false)?,
            JoinType::LeftOuterJoin => getImplForHashJoin(expr, req_prop, 1, true)?,
            JoinType::RightOuterJoin => getImplForHashJoin(expr, req_prop, 0, false)?,
            _ => return Ok(Vec::new()),
        };
        Ok(vec![implementation])
    }
}

/// ImplHashJoinBuildRight 覆盖半连接以及右侧建表的普通/外连接候选。
pub struct ImplHashJoinBuildRight;
impl ImplementationRule for ImplHashJoinBuildRight {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let implementation = match logical::<LogicalJoin>(expr)?.JoinType {
            JoinType::SemiJoin
            | JoinType::AntiSemiJoin
            | JoinType::LeftOuterSemiJoin
            | JoinType::AntiLeftOuterSemiJoin => getImplForHashJoin(expr, req_prop, 1, false)?,
            JoinType::InnerJoin | JoinType::LeftOuterJoin => {
                getImplForHashJoin(expr, req_prop, 1, false)?
            }
            JoinType::RightOuterJoin => getImplForHashJoin(expr, req_prop, 0, true)?,
            JoinType::FullOuterJoin => return Ok(Vec::new()),
        };
        Ok(vec![implementation])
    }
}

/// ImplMergeJoin 枚举 GetMergeJoin 返回的全部物理 MergeJoin 方案。
pub struct ImplMergeJoin;
impl ImplementationRule for ImplMergeJoin {
    fn Match(&self, _expr: &GroupExpr, _prop: &PhysicalProperty) -> bool {
        true
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let join = logical::<LogicalJoin>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let (left_stats, left_schema) = child_properties(expr, 0)?;
        let (right_stats, right_schema) = child_properties(expr, 1)?;
        let ctx = context(join)?;
        let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            ctx.clone(),
            "MergeJoin",
            join.QueryBlockOffset(),
        ));
        let mut physical = GetMergeJoin(join, producer).Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            join.QueryBlockOffset(),
        );
        if physical.BasePhysicalJoin.LeftJoinKeys.is_empty() {
            return Ok(Vec::new());
        }
        let mut left_prop = max_count_property();
        let mut right_prop = max_count_property();
        let desc = req_prop.SortItems.first().is_some_and(|item| item.Desc);
        left_prop.SortItems = physical
            .BasePhysicalJoin
            .LeftJoinKeys
            .iter()
            .map(|column| SortItem {
                Col: column.Clone(),
                Desc: desc,
            })
            .collect();
        right_prop.SortItems = physical
            .BasePhysicalJoin
            .RightJoinKeys
            .iter()
            .map(|column| SortItem {
                Col: column.Clone(),
                Desc: desc,
            })
            .collect();
        if req_prop.ExpectedCnt < stats.RowCount {
            let scale = req_prop.ExpectedCnt / stats.RowCount;
            left_prop.ExpectedCnt = left_stats.RowCount * scale;
            right_prop.ExpectedCnt = right_stats.RowCount * scale;
        }
        if !req_prop.IsSortItemEmpty()
            && !req_prop.AllColsFromSchema(&left_schema)
            && !req_prop.AllColsFromSchema(&right_schema)
        {
            return Ok(Vec::new());
        }
        physical.Desc = desc;
        physical
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(vec![Box::new(left_prop), Box::new(right_prop)]);
        physical
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(schema);
        Ok(vec![implementation_ref(NewMergeJoinImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplUnionAll 为每个孩子复制父 ExpectedCnt，并保持输出 Schema。
pub struct ImplUnionAll;
impl ImplementationRule for ImplUnionAll {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let union = logical::<LogicalUnionAll>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(union)?;
        let child_props = expr
            .Children
            .iter()
            .map(|_| {
                let mut property = PhysicalProperty::default();
                property.ExpectedCnt = req_prop.ExpectedCnt;
                Box::new(property)
            })
            .collect();
        let mut physical = PhysicalUnionAll::New(ctx.clone()).Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            union.QueryBlockOffset(),
            child_props,
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewUnionAllImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplApply 要求父排序列全部来自外侧孩子，并把 LogicalApply 包装成 PhysicalApply。
pub struct ImplApply;
impl ImplementationRule for ImplApply {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        child_properties(expr, 0).is_ok_and(|(_, schema)| prop.AllColsFromSchema(&schema))
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let apply = logical::<LogicalApply>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(apply)?;
        let hash_join = GetHashJoin(None, apply, req_prop)?;
        let mut outer_prop = max_count_property();
        outer_prop.SortItems = req_prop.SortItems.clone();
        let inner_prop = max_count_property();
        let mut physical = PhysicalApply::New(hash_join);
        physical.OuterSchema = apply
            .CorCols
            .iter()
            .map(expression::CorrelatedColumn::Clone)
            .collect();
        let mut physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            apply.QueryBlockOffset(),
            vec![Box::new(outer_prop), Box::new(inner_prop)],
        );
        physical
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(schema);
        Ok(vec![implementation_ref(NewApplyImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplMaxOneRow 让孩子最多产生两行，以便物理算子检测违反单行约束的情况。
pub struct ImplMaxOneRow;
impl ImplementationRule for ImplMaxOneRow {
    fn Match(&self, _expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        prop.IsSortItemEmpty()
    }

    fn OnImplement(&self, expr: &GroupExpr, _req_prop: &PhysicalProperty) -> RuleResult {
        let max_one = logical::<LogicalMaxOneRow>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(max_one)?;
        let mut child_prop = PhysicalProperty::default();
        child_prop.ExpectedCnt = 2.0;
        let mut physical = PhysicalMaxOneRow::New(ctx.clone()).Init(
            ctx,
            stats,
            max_one.QueryBlockOffset(),
            child_prop,
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewMaxOneRowImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}

/// ImplWindow 要求父属性是 PartitionBy + OrderBy 的前缀，并把完整顺序下推给孩子。
pub struct ImplWindow;
impl ImplementationRule for ImplWindow {
    fn Match(&self, expr: &GroupExpr, prop: &PhysicalProperty) -> bool {
        let Ok(window) = logical::<LogicalWindow>(expr) else {
            return false;
        };
        let mut items = window.PartitionBy.clone();
        items.extend(window.OrderBy.clone());
        let mut child_prop = max_count_property();
        child_prop.SortItems = items;
        prop.IsPrefix(&child_prop)
    }

    fn OnImplement(&self, expr: &GroupExpr, req_prop: &PhysicalProperty) -> RuleResult {
        let window = logical::<LogicalWindow>(expr)?;
        let (stats, schema, _) = group_properties(expr)?;
        let ctx = context(window)?;
        let mut items = window.PartitionBy.clone();
        items.extend(window.OrderBy.clone());
        let mut child_prop = max_count_property();
        child_prop.SortItems = items;
        let mut physical = PhysicalWindow::New(ctx.clone());
        physical.WindowFuncDescs = window.WindowFuncDescs.clone();
        physical.PartitionBy = window.PartitionBy.clone();
        physical.OrderBy = window.OrderBy.clone();
        physical.Frame = window.Frame.clone();
        let mut physical = physical.Init(
            ctx.clone(),
            scaled_stats(&stats, &ctx, req_prop.ExpectedCnt),
            window.QueryBlockOffset(),
            child_prop,
        );
        physical.PhysicalSchemaProducer.SetSchema(schema);
        Ok(vec![implementation_ref(NewWindowImpl(Box::new(
            PlanAdapter { plan: physical },
        )))])
    }
}
