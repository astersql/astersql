// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 代价模型 Ver2：分资源维度（CPU/内存/磁盘/网络/请求）的代价估算。
//
// 相对 Ver1，Ver2 将代价拆分为可追踪分量（CostTrace），并按 TiDB/TiKV/TiFlash
// 使用不同因子。含 canonical 物理计划入口与基于 PlanNode 的分算子实现。

use base_dependency as base;
use physicalop_dependency as physicalop;

struct CardinalityContextAdapter<'a>(&'a dyn base::PlanContext);

impl cardinality_dependency::CardinalityContext for CardinalityContextAdapter<'_> {
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

fn canonical_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    costusage_dependency::format_go_float(value)
}

/// 估算行宽：无直方图时按当前输出 Schema 的类型宽度回退，与 Go
/// `getAvgRowSize` 一致；不能按整张表或声明 flen 计费。
fn canonical_row_size(plan: &dyn base::PhysicalPlan) -> f64 {
    let columns = plan.schema().Columns.iter().collect::<Vec<_>>();
    plan.stats_info()
        .HistColl
        .as_deref()
        .and_then(|histograms| histograms.downcast_ref::<statistics_dependency::HistColl>())
        .map(|histograms| {
            cardinality_dependency::GetAvgRowSizeDataInDiskByRows(histograms, &columns)
        })
        .unwrap_or_else(|| {
            columns
                .iter()
                .filter_map(|column| column.RetType.as_ref())
                .map(|field_type| chunk_dependency::EstimateTypeWidth(field_type) as f64)
                .sum::<f64>()
        })
        .max(1.0)
}

fn canonical_avg_row_size(plan: &dyn base::PhysicalPlan, index: bool) -> f64 {
    let histograms = physicalop::GetTblStats(Some(plan));
    let fallback = statistics_dependency::NewHistColl(0, 0, 0, 0, 0);
    let histograms = histograms
        .as_deref()
        .and_then(|value| value.downcast_ref::<statistics_dependency::HistColl>())
        .or_else(|| {
            plan.stats_info()
                .HistColl
                .as_deref()
                .and_then(|value| value.downcast_ref::<statistics_dependency::HistColl>())
        })
        .unwrap_or(&fallback);
    let columns = plan.schema().Columns.iter().collect::<Vec<_>>();
    if !histograms.Pseudo && histograms.ColNum() > 0 && histograms.RealtimeCount > 0 {
        let size = columns
            .iter()
            .map(|column| {
                histograms.GetCol(column.UniqueID).map_or(8.0, |column| {
                    cardinality_dependency::AvgColSizeChunkFormat(column, histograms.RealtimeCount)
                })
            })
            .sum::<f64>();
        return (size + columns.len() as f64 / 8.0).max(1.0);
    }
    cardinality_dependency::GetAvgRowSize(
        &CardinalityContextAdapter(plan.s_ctx().as_ref()),
        histograms,
        &columns,
        index,
        false,
    )
    .max(1.0)
}

fn canonical_trace_row_size(plan: &dyn base::PhysicalPlan) -> f64 {
    if plan.as_any().is::<physicalop::PhysicalExchangeReceiver>()
        || plan.as_any().is::<physicalop::PhysicalExchangeSender>()
    {
        if let Some(child) = plan.children().into_iter().next() {
            return canonical_trace_row_size(child);
        }
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
    {
        let eval = plan.s_ctx().GetExprCtx().GetEvalCtx();
        return projection
            .Exprs
            .iter()
            .map(|expression| chunk_dependency::EstimateTypeWidth(expression.GetType(eval)) as f64)
            .sum::<f64>()
            .max(1.0);
    }
    canonical_row_size(plan)
}

fn canonical_schema_width(plan: &dyn base::PhysicalPlan) -> f64 {
    plan.schema()
        .Columns
        .iter()
        .filter_map(|column| column.RetType.as_ref())
        .map(|field_type| chunk_dependency::EstimateTypeWidth(field_type) as f64)
        .sum::<f64>()
        .max(1.0)
}

fn canonical_num_functions(expressions: &[expression_dependency::ExprBox]) -> f64 {
    expressions
        .iter()
        .map(|expression| {
            if expression
                .as_any()
                .is::<expression_dependency::ScalarFunction>()
            {
                1.0
            } else {
                0.01
            }
        })
        .sum()
}

fn canonical_factor(name: &str, value: f64) -> costusage_dependency::CostVer2Factor {
    costusage_dependency::CostVer2Factor {
        name: name.to_owned(),
        value,
    }
}

fn canonical_net_cost(
    option: &costusage_dependency::PlanCostOption,
    rows: f64,
    width: f64,
    name: &str,
    factor: f64,
) -> costusage_dependency::CostVer2 {
    costusage_dependency::new_cost_ver2(
        Some(option),
        canonical_factor(name, factor),
        rows * width * factor,
        || {
            format!(
                "net({}*rowsize({width})*{name}({factor}))",
                canonical_number(rows)
            )
        },
    )
}

fn canonical_concurrency(plan: &dyn base::PhysicalPlan, name: &str, fallback: f64) -> f64 {
    plan.s_ctx()
        .GetSessionVars()
        .GetSystemVar(name)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(fallback)
}

fn canonical_term(
    option: &costusage_dependency::PlanCostOption,
    name: &str,
    factor_name: &str,
    factor: f64,
    units: f64,
    formula: String,
) -> costusage_dependency::CostVer2 {
    costusage_dependency::new_cost_ver2(
        Some(option),
        canonical_factor(factor_name, factor),
        units * factor,
        || format!("{name}({formula}*{factor_name}({factor}))"),
    )
}

pub fn canonical_index_lookup_cost(
    plan: &dyn base::PhysicalPlan,
    outer_rows: f64,
    option: &costusage_dependency::PlanCostOption,
) -> Result<costusage_dependency::CostVer2, expression_dependency::Error> {
    let rows = plan.stats_count().max(0.0) / outer_rows.max(1.0);
    let width = if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
    {
        // IndexJoin performs a dynamic table lookup. Go charges the storage
        // scan using the full table-row layout even when the lookup reader
        // projects fewer result columns; the reader network term below still
        // uses its projected schema width.
        scan.Table
            .as_ref()
            .map(|table| table.Columns.as_slice())
            .unwrap_or(scan.Columns.as_slice())
            .iter()
            .map(|column| chunk_dependency::EstimateTypeWidth(&column.FieldType) as f64)
            .sum::<f64>()
            .max(2.0)
    } else if plan.as_any().is::<physicalop::PhysicalTableReader>() {
        canonical_schema_width(plan).max(2.0)
    } else {
        canonical_row_size(plan).max(2.0)
    };
    if plan.as_any().is::<physicalop::PhysicalTableScan>() {
        let cost = canonical_term(
            option,
            "scan",
            "tikv_scan_factor",
            40.7,
            rows.max(1.0) * width.log2(),
            format!("{}*logrowsize({width})", canonical_number(rows.max(1.0))),
        );
        return Ok(costusage_dependency::mul_cost_ver2(&cost, 1.0));
    }
    if let Some(selection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSelection>()
    {
        let child = plan
            .children()
            .first()
            .map(|child| canonical_index_lookup_cost(*child, outer_rows, option))
            .transpose()?
            .unwrap_or_else(|| costusage_dependency::new_zero_cost_ver2(true));
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count() / outer_rows.max(1.0));
        let filters = canonical_num_functions(&selection.Conditions);
        return Ok(costusage_dependency::sum_cost_ver2(&[
            canonical_term(
                option,
                "cpu",
                "tikv_cpu_factor",
                49.9,
                input_rows * filters,
                format!("{}*filters({filters})", canonical_number(input_rows)),
            ),
            child,
        ]));
    }
    if plan.as_any().is::<physicalop::PhysicalTableReader>() {
        let child = plan
            .children()
            .first()
            .map(|child| canonical_index_lookup_cost(*child, outer_rows, option))
            .transpose()?
            .unwrap_or_else(|| costusage_dependency::new_zero_cost_ver2(true));
        let net = canonical_net_cost(option, rows, width, "tidb_kv_net_factor", 3.96);
        return Ok(costusage_dependency::mul_cost_ver2(
            &costusage_dependency::div_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[child, net]),
                15.0,
            ),
            1.0,
        ));
    }
    Ok(costusage_dependency::new_zero_cost_ver2(true))
}

/// Go IndexJoin batches inner lookups and amortizes the probe cost by this
/// empirical ratio after multiplying it by the outer cardinality exactly once.
pub(crate) fn canonical_index_join_batch_ratio() -> f64 {
    6.0
}

fn canonical_child_can_order(mut plan: &dyn base::PhysicalPlan) -> bool {
    loop {
        let any = plan.as_any();
        if any.is::<physicalop::PhysicalTableReader>()
            || any.is::<physicalop::PhysicalIndexReader>()
            || any.is::<physicalop::PhysicalIndexLookUpReader>()
            || any.is::<physicalop::PhysicalIndexMergeReader>()
        {
            return true;
        }
        if any.is::<physicalop::PhysicalProjection>()
            || any.is::<physicalop::PhysicalSelection>()
            || any.is::<physicalop::PhysicalUnionScan>()
        {
            let children = plan.children();
            if children.len() != 1 {
                return false;
            }
            plan = children[0];
            continue;
        }
        return false;
    }
}

/// Go-equivalent v2 cost entry for the real physical-plan hierarchy.
/// 与 Go 对齐的 v2 代价入口，面向真实物理计划层次结构。
#[derive(Clone, Hash, Eq, PartialEq)]
struct CostRequestKey {
    plan: usize,
    task: i32,
    inl: Vec<bool>,
}

#[derive(Clone)]
struct CostRequest<'a> {
    plan: &'a dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    inl: Vec<bool>,
}

impl CostRequest<'_> {
    fn key(&self) -> CostRequestKey {
        CostRequestKey {
            plan: self.plan as *const dyn base::PhysicalPlan as *const () as usize,
            task: self.task.0,
            inl: self.inl.clone(),
        }
    }
}

struct CostWorklist<'a> {
    values: std::collections::HashMap<CostRequestKey, costusage_dependency::CostVer2>,
    pending: Option<CostRequest<'a>>,
}

fn cost_of_child<'a>(
    plan: &'a dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    _option: &costusage_dependency::PlanCostOption,
    inl: &[bool],
    context: &mut CostWorklist<'a>,
) -> Result<costusage_dependency::CostVer2, expression_dependency::Error> {
    let request = CostRequest {
        plan,
        task,
        inl: inl.to_vec(),
    };
    if let Some(cost) = context.values.get(&request.key()) {
        return Ok(cost.clone());
    }
    context.pending = Some(request);
    Err(expression_dependency::errors::New(
        "physical child cost pending",
    ))
}

/// Evaluate the same operator formulas as Go, with an explicit request stack.
/// Q64's MPP plan is deep enough that recursive cost calls exhaust Rust's
/// fixed-size test thread stack even though the physical tree is acyclic.
pub fn GetCanonicalPlanCostVer2(
    plan: &dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    option: &costusage_dependency::PlanCostOption,
    inl: &[bool],
) -> Result<costusage_dependency::CostVer2, expression_dependency::Error> {
    let root = CostRequest {
        plan,
        task,
        inl: inl.to_vec(),
    };
    let root_key = root.key();
    let mut work = vec![root];
    let mut context = CostWorklist {
        values: std::collections::HashMap::new(),
        pending: None,
    };
    while let Some(request) = work.last() {
        let key = request.key();
        if context.values.contains_key(&key) {
            work.pop();
            continue;
        }
        context.pending = None;
        match get_canonical_plan_cost_ver2_inner(
            request.plan,
            request.task,
            option,
            &request.inl,
            &mut context,
        ) {
            Ok(cost) => {
                context.values.insert(key, cost);
                work.pop();
            }
            Err(error) => {
                let Some(child) = context.pending.take() else {
                    return Err(error);
                };
                if work.iter().any(|request| request.key() == child.key()) {
                    return Err(expression_dependency::errors::New(
                        "cyclic physical plan during cost calculation",
                    ));
                }
                work.push(child);
            }
        }
    }
    context
        .values
        .remove(&root_key)
        .ok_or_else(|| expression_dependency::errors::New("physical plan cost was not calculated"))
}

fn get_canonical_plan_cost_ver2_inner<'a>(
    plan: &'a dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    option: &costusage_dependency::PlanCostOption,
    inl: &[bool],
    context: &mut CostWorklist<'a>,
) -> Result<costusage_dependency::CostVer2, expression_dependency::Error> {
    let rows = plan.stats_count().max(0.0);
    let width = if costusage_dependency::trace_cost(Some(option)) {
        canonical_trace_row_size(plan)
    } else {
        canonical_row_size(plan)
    };
    let any = plan.as_any();

    if let Some(reader) = any.downcast_ref::<physicalop::PhysicalIndexLookUpReader>() {
        let Some(index_plan) = reader.IndexPlan.as_deref() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(
                costusage_dependency::trace_cost(Some(option)),
            ));
        };
        let Some(table_plan) = reader.TablePlan.as_deref() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(
                costusage_dependency::trace_cost(Some(option)),
            ));
        };
        let limit = reader.PushedLimit.map(|limit| limit.Count as f64);
        let index_rows = limit.map_or_else(
            || index_plan.stats_count(),
            |count| index_plan.stats_count().min(count),
        );
        let table_rows = limit.map_or_else(
            || table_plan.stats_count(),
            |count| table_plan.stats_count().min(count),
        );
        let index_width = canonical_avg_row_size(index_plan, true);
        // Double-read transports the row handle in addition to the projected
        // table columns. In chunk format that is one 8-byte fixed value plus
        // one null-bitmap bit.
        let table_width = canonical_avg_row_size(table_plan, false) + 8.125;
        let index_child = cost_of_child(
            index_plan,
            property_dependency::CopMultiReadTaskType,
            option,
            inl,
            context,
        )?;
        let table_child = cost_of_child(
            table_plan,
            property_dependency::CopMultiReadTaskType,
            option,
            inl,
            context,
        )?;
        let index_side = costusage_dependency::div_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[
                canonical_net_cost(option, index_rows, index_width, "tidb_kv_net_factor", 3.96),
                index_child,
            ]),
            15.0,
        );
        let table_side = costusage_dependency::div_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[
                canonical_net_cost(option, table_rows, table_width, "tidb_kv_net_factor", 3.96),
                table_child,
            ]),
            15.0,
        );
        let double_read_cpu = costusage_dependency::new_cost_ver2(
            Some(option),
            canonical_factor("tidb_cpu_factor", 49.9),
            index_rows * 49.9,
            || {
                format!(
                    "double-read-cpu({}*tidb_cpu_factor(49.9))",
                    canonical_number(index_rows)
                )
            },
        );
        let tasks = index_rows / 20_000.0 * 32.0;
        let double_read_request = costusage_dependency::new_cost_ver2(
            Some(option),
            canonical_factor("tidb_request_factor", 6_000_000.0),
            tasks * 6_000_000.0,
            || format!("doubleRead(tasks({tasks})*tidb_request_factor(6e+06))"),
        );
        let table_and_double_read = costusage_dependency::div_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[
                table_side,
                costusage_dependency::sum_cost_ver2(&[double_read_cpu, double_read_request]),
            ]),
            5.0,
        );
        let lookup = costusage_dependency::mul_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[index_side, table_and_double_read]),
            1.0,
        );
        return Ok(costusage_dependency::mul_cost_ver2(&lookup, 1.0));
    }

    // Readers cross the TiDB/storage boundary. Their storage subtree uses the
    // storage task factors, then network and scan work are amortized by the
    // DistSQL concurrency. This task transition is also essential for plan
    // selection; recursively charging the subtree as `root` strongly biases
    // the optimizer toward MPP plans.
    if any.is::<physicalop::PhysicalTableReader>() || any.is::<physicalop::PhysicalIndexReader>() {
        let Some(child) = plan.children().into_iter().next() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(
                costusage_dependency::trace_cost(Some(option)),
            ));
        };
        fn contains_reader(plan: &dyn base::PhysicalPlan) -> bool {
            plan.as_any().is::<physicalop::PhysicalTableReader>()
                || plan.as_any().is::<physicalop::PhysicalIndexReader>()
                || plan
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalTableReader>()
                    .and_then(|reader| reader.TablePlan.as_deref())
                    .is_some_and(contains_reader)
                || plan.children().into_iter().any(contains_reader)
        }
        if contains_reader(child) {
            return Ok(costusage_dependency::new_cost_ver2(
                Some(option),
                canonical_factor("invalid_nested_reader", 1.0),
                1.0e100,
                || "invalid_nested_reader".to_owned(),
            ));
        }
        let child_task = if any
            .downcast_ref::<physicalop::PhysicalTableReader>()
            .is_some_and(|reader| reader.StoreType == kv_dependency::StoreType::TiFlash)
        {
            property_dependency::MppTaskType
        } else {
            property_dependency::CopSingleReadTaskType
        };
        let child_cost = cost_of_child(child, child_task, option, inl, context)?;
        let child_rows = child.stats_count().max(0.0);
        let net_factor = if child_task == property_dependency::MppTaskType {
            ("tidb_flash_net_factor", 2.2)
        } else {
            ("tidb_kv_net_factor", 3.96)
        };
        let net_cost = canonical_net_cost(
            option,
            child_rows,
            width.max(2.0),
            net_factor.0,
            net_factor.1,
        );
        let total = costusage_dependency::sum_cost_ver2(&[child_cost, net_cost]);
        return Ok(costusage_dependency::mul_cost_ver2(
            &costusage_dependency::div_cost_ver2(
                &total,
                canonical_concurrency(
                    plan,
                    vardef_dependency::TiDBDistSQLScanConcurrency,
                    vardef_dependency::DefDistSQLScanConcurrency as f64,
                ),
            ),
            1.0,
        ));
    }

    if any.is::<physicalop::PhysicalExchangeReceiver>() {
        let child_cost = plan
            .children()
            .into_iter()
            .next()
            .map(|child| cost_of_child(child, task, option, inl, context))
            .transpose()?
            .unwrap_or_else(|| {
                costusage_dependency::new_zero_cost_ver2(costusage_dependency::trace_cost(Some(
                    option,
                )))
            });
        let net_cost = canonical_net_cost(option, rows, width, "tiflash_mpp_net_factor", 1.0);
        let net_cost = if plan
            .children()
            .first()
            .and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalExchangeSender>()
            })
            .is_some_and(|sender| sender.ExplainInfo().contains("Broadcast"))
        {
            costusage_dependency::mul_cost_ver2(&net_cost, 3.0)
        } else {
            net_cost
        };
        return Ok(costusage_dependency::sum_cost_ver2(&[child_cost, net_cost]));
    }
    if any.is::<physicalop::PhysicalExchangeSender>() {
        return plan
            .children()
            .into_iter()
            .next()
            .map(|child| cost_of_child(child, task, option, inl, context))
            .transpose()
            .map(|cost| {
                let child_cost = cost.unwrap_or_else(|| {
                    costusage_dependency::new_zero_cost_ver2(costusage_dependency::trace_cost(
                        Some(option),
                    ))
                });
                // Go routes ExchangeSender through SumCostVer2 even though it
                // currently has one child. Keep that single-node sum because
                // it contributes one observable pair of cost_trace brackets.
                costusage_dependency::sum_cost_ver2(&[child_cost])
            });
    }

    if let Some(join) = any.downcast_ref::<physicalop::PhysicalIndexJoin>() {
        let children = plan.children();
        if children.len() == 2 {
            let inner_index = join.BasePhysicalJoin.InnerChildIdx.min(1);
            let outer_index = 1 - inner_index;
            let outer = children[outer_index];
            let inner = children[inner_index];
            let outer_rows = outer.stats_count().max(1.0);
            let inner_rows = inner.stats_count().max(0.0);
            let outer_width = canonical_row_size(outer).max(2.0);
            let inner_width = canonical_schema_width(inner).max(2.0);
            let outer_cost = cost_of_child(
                outer,
                property_dependency::RootTaskType,
                option,
                inl,
                context,
            )?;
            let inner_cost = canonical_index_lookup_cost(inner, outer_rows, option)?;
            let probe_rows_one = (inner_rows / outer_rows).max(1.0);
            let is_semi_join = matches!(
                join.BasePhysicalJoin.JoinType,
                base_dependency::JoinType::SemiJoin
                    | base_dependency::JoinType::AntiSemiJoin
                    | base_dependency::JoinType::LeftOuterSemiJoin
                    | base_dependency::JoinType::AntiLeftOuterSemiJoin
            );
            let semi_overread = if is_semi_join && costusage_dependency::trace_cost(Some(option)) {
                probe_rows_one
            } else {
                1.0
            };
            let lookup_trace = costusage_dependency::div_cost_ver2(
                &costusage_dependency::mul_cost_ver2(&inner_cost, outer_rows),
                canonical_index_join_batch_ratio(),
            );
            let lookup_trace = if is_semi_join {
                costusage_dependency::mul_cost_ver2(&lookup_trace, semi_overread)
            } else {
                lookup_trace
            };
            let cpu = 49.9;
            let is_index_hash_join = plan.tp(&[]) == "IndexHashJoin";
            let hash_rows = if is_semi_join || is_index_hash_join {
                outer_rows
            } else {
                inner_rows
            };
            let hash_width = if is_semi_join || is_index_hash_join {
                outer_width
            } else {
                inner_width
            };
            let build = costusage_dependency::sum_cost_ver2(&[
                canonical_term(
                    option,
                    "hashkey",
                    "tidb_cpu_factor",
                    cpu,
                    0.0,
                    format!("{}*0", canonical_number(hash_rows)),
                ),
                canonical_term(
                    option,
                    "hashmem",
                    "tidb_mem_factor",
                    0.2,
                    hash_rows * hash_width,
                    format!("{}*{hash_width}", canonical_number(hash_rows)),
                ),
                canonical_term(
                    option,
                    "hashbuild",
                    "tidb_cpu_factor",
                    cpu,
                    hash_rows,
                    canonical_number(hash_rows),
                ),
            ]);
            let probe_filter = canonical_term(
                option,
                "cpu",
                "tidb_cpu_factor",
                cpu,
                inner_rows * 0.0,
                format!("{}*filters(0)", canonical_number(inner_rows)),
            );
            let parallel = costusage_dependency::div_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[lookup_trace, probe_filter, build]),
                5.0,
            );
            let startup = canonical_term(
                option,
                "cpu",
                "tidb_cpu_factor",
                cpu,
                30.0,
                "10*3".to_owned(),
            );
            let mut outer_parts = vec![outer_cost];
            if join.BasePhysicalJoin.JoinType == base_dependency::JoinType::SemiJoin
                && !join.BasePhysicalJoin.OtherConditions.is_empty()
            {
                let selectivity = 0.03;
                outer_parts.push(costusage_dependency::div_cost_ver2(
                    &canonical_term(
                        option,
                        "cpu",
                        "tidb_cpu_factor",
                        cpu,
                        outer_rows * selectivity,
                        format!("{}*filters({selectivity})", canonical_number(outer_rows)),
                    ),
                    5.0,
                ));
            }
            let outer = if outer_parts.len() == 1 {
                outer_parts.remove(0)
            } else {
                costusage_dependency::sum_cost_ver2(&outer_parts)
            };
            let mut parts = vec![startup, outer];
            parts.extend([
                canonical_term(
                    option,
                    "cpu",
                    "tidb_cpu_factor",
                    cpu,
                    outer_rows * 0.0,
                    format!("{}*filters(0)", canonical_number(outer_rows)),
                ),
                canonical_term(
                    option,
                    "cpu",
                    "tidb_cpu_factor",
                    cpu,
                    outer_rows * 10.0,
                    format!("{}*10", canonical_number(outer_rows)),
                ),
                parallel,
            ]);
            let cost = costusage_dependency::sum_cost_ver2(&parts);
            return Ok(costusage_dependency::mul_cost_ver2(&cost, 1.0));
        }
    }
    if let Some(join) = any.downcast_ref::<physicalop::PhysicalHashJoin>() {
        let children = plan.children();
        if children.len() == 2 {
            let swap = (join.BasePhysicalJoin.InnerChildIdx == 1 && !join.UseOuterToBuild)
                || (join.BasePhysicalJoin.InnerChildIdx == 0 && join.UseOuterToBuild);
            let (build_index, probe_index) = if swap { (1, 0) } else { (0, 1) };
            let build = children[build_index];
            let probe = children[probe_index];
            let (build_filters, probe_filters, build_keys, probe_keys) = if swap {
                (
                    &join.BasePhysicalJoin.RightConditions,
                    &join.BasePhysicalJoin.LeftConditions,
                    &join.BasePhysicalJoin.RightJoinKeys,
                    &join.BasePhysicalJoin.LeftJoinKeys,
                )
            } else {
                (
                    &join.BasePhysicalJoin.LeftConditions,
                    &join.BasePhysicalJoin.RightConditions,
                    &join.BasePhysicalJoin.LeftJoinKeys,
                    &join.BasePhysicalJoin.RightJoinKeys,
                )
            };
            let build_rows = build.stats_count().max(1.0);
            let probe_rows = probe.stats_count().max(0.0);
            let build_width = if costusage_dependency::trace_cost(Some(option)) {
                canonical_trace_row_size(build).max(2.0)
            } else {
                canonical_row_size(build).max(2.0)
            };
            let cpu = if task == property_dependency::MppTaskType {
                2.4
            } else {
                49.9
            };
            let mem = if task == property_dependency::MppTaskType {
                0.05
            } else {
                0.2
            };
            let build_child = cost_of_child(build, task, option, inl, context)?;
            let probe_child = cost_of_child(probe, task, option, inl, context)?;
            let build_filter_count = canonical_num_functions(build_filters);
            let probe_filter_count = canonical_num_functions(probe_filters);
            let build_filter_count = if build_filter_count == 0.0 {
                0.0
            } else {
                build_filter_count
            };
            let probe_filter_count = if probe_filter_count == 0.0 {
                0.0
            } else {
                probe_filter_count
            };
            let build_filter = canonical_term(
                option,
                "cpu",
                if task == property_dependency::MppTaskType {
                    "tiflash_cpu_factor"
                } else {
                    "tidb_cpu_factor"
                },
                cpu,
                build_rows * build_filter_count,
                format!(
                    "{}*filters({build_filter_count})",
                    canonical_number(build_rows)
                ),
            );
            let probe_filter = canonical_term(
                option,
                "cpu",
                if task == property_dependency::MppTaskType {
                    "tiflash_cpu_factor"
                } else {
                    "tidb_cpu_factor"
                },
                cpu,
                probe_rows * probe_filter_count,
                format!(
                    "{}*filters({probe_filter_count})",
                    canonical_number(probe_rows)
                ),
            );
            let build_key_count = build_keys.len() as f64;
            let probe_key_count = probe_keys.len() as f64;
            let build_hash = costusage_dependency::sum_cost_ver2(&[
                canonical_term(
                    option,
                    "hashkey",
                    if task == property_dependency::MppTaskType {
                        "tiflash_cpu_factor"
                    } else {
                        "tidb_cpu_factor"
                    },
                    cpu,
                    build_rows * build_key_count,
                    format!("{}*{build_key_count}", canonical_number(build_rows)),
                ),
                canonical_term(
                    option,
                    "hashmem",
                    if task == property_dependency::MppTaskType {
                        "tiflash_mem_factor"
                    } else {
                        "tidb_mem_factor"
                    },
                    mem,
                    build_rows * build_width,
                    format!("{}*{build_width}", canonical_number(build_rows)),
                ),
                canonical_term(
                    option,
                    "hashbuild",
                    if task == property_dependency::MppTaskType {
                        "tiflash_cpu_factor"
                    } else {
                        "tidb_cpu_factor"
                    },
                    cpu,
                    build_rows,
                    canonical_number(build_rows),
                ),
            ]);
            let probe_hash = costusage_dependency::sum_cost_ver2(&[
                canonical_term(
                    option,
                    "hashkey",
                    if task == property_dependency::MppTaskType {
                        "tiflash_cpu_factor"
                    } else {
                        "tidb_cpu_factor"
                    },
                    cpu,
                    probe_rows * probe_key_count,
                    format!("{}*{probe_key_count}", canonical_number(probe_rows)),
                ),
                canonical_term(
                    option,
                    "hashprobe",
                    if task == property_dependency::MppTaskType {
                        "tiflash_cpu_factor"
                    } else {
                        "tidb_cpu_factor"
                    },
                    cpu,
                    probe_rows,
                    canonical_number(probe_rows),
                ),
            ]);
            let scan_build_unmatched =
                if join.BasePhysicalJoin.JoinType == base_dependency::JoinType::FullOuterJoin {
                    canonical_term(
                        option,
                        "cpu",
                        if task == property_dependency::MppTaskType {
                            "tiflash_cpu_factor"
                        } else {
                            "tidb_cpu_factor"
                        },
                        cpu,
                        build_rows,
                        format!("scanBuildUnmatched({})", canonical_number(build_rows)),
                    )
                } else {
                    costusage_dependency::new_zero_cost_ver2(costusage_dependency::trace_cost(
                        Some(option),
                    ))
                };
            if task != property_dependency::MppTaskType {
                let probe_parallel = costusage_dependency::div_cost_ver2(
                    &costusage_dependency::sum_cost_ver2(&[
                        probe_filter,
                        probe_hash,
                        scan_build_unmatched,
                    ]),
                    join.Concurrency.max(1) as f64,
                );
                return Ok(costusage_dependency::mul_cost_ver2(
                    &costusage_dependency::sum_cost_ver2(&[
                        canonical_term(
                            option,
                            "cpu",
                            "tidb_cpu_factor",
                            cpu,
                            30.0,
                            "10*3".to_owned(),
                        ),
                        build_child,
                        probe_child,
                        build_hash,
                        build_filter,
                        probe_parallel,
                    ]),
                    1.0,
                ));
            }
            let parallel = costusage_dependency::div_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[
                    build_hash,
                    build_filter,
                    probe_hash,
                    probe_filter,
                    scan_build_unmatched,
                ]),
                3.0,
            );
            return Ok(costusage_dependency::mul_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[build_child, probe_child, parallel]),
                1.0,
            ));
        }
    }
    if any.is::<physicalop::PhysicalIndexScan>() {
        let width = canonical_row_size(plan);
        let scan = canonical_term(
            option,
            "scan",
            "tikv_scan_factor",
            40.7,
            rows.max(1.0) * width.max(2.0).log2(),
            format!(
                "{}*logrowsize({})",
                canonical_number(rows.max(1.0)),
                width.max(2.0)
            ),
        );
        return Ok(costusage_dependency::mul_cost_ver2(&scan, 1.0));
    }
    if let Some(scan) = any.downcast_ref::<physicalop::PhysicalTableScan>() {
        let width = canonical_row_size(plan)
            + if base::Plan::tp(&scan.PhysicalSchemaProducer.BasePhysicalPlan, &[])
                == "TableRowIDScan"
            {
                16.0
            } else {
                0.0
            };
        let tiflash = scan.StoreType == kv_dependency::StoreType::TiFlash;
        let (factor_name, factor) = if tiflash {
            ("tiflash_scan_factor", 11.6)
        } else {
            ("tikv_scan_factor", 40.7)
        };
        let one = |charged_rows: f64, charged_width: f64| {
            canonical_term(
                option,
                "scan",
                factor_name,
                factor,
                charged_rows * charged_width.max(2.0).log2(),
                format!(
                    "{}*logrowsize({})",
                    canonical_number(charged_rows),
                    charged_width.max(2.0)
                ),
            )
        };
        let equality_conditions = scan
            .FilterCondition
            .iter()
            .filter(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast_dependency::EQ | parser_ast_dependency::NullEQ
                    )
                })
            })
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        let late_conditions = if !scan.LateMaterializationFilterCondition.is_empty() {
            &scan.LateMaterializationFilterCondition
        } else if !equality_conditions.is_empty() {
            &equality_conditions
        } else if scan.FilterCondition.is_empty() {
            &scan.AccessCondition
        } else {
            &scan.FilterCondition
        };
        let late_materialization_filter = {
            let columns =
                expression_dependency::ExtractColumnsFromExpressions(late_conditions, None);
            columns
                .iter()
                .map(|column| column.UniqueID)
                .collect::<std::collections::HashSet<_>>()
                .len()
                <= 1
        };
        if tiflash
            && !late_conditions.is_empty()
            && late_materialization_filter
            && plan.stats_info().StatsVersion != statistics_dependency::PseudoVersion
            && let Some(histograms) = scan
                .TblColHists
                .as_deref()
                .and_then(|histograms| histograms.downcast_ref::<statistics_dependency::HistColl>())
                .or_else(|| {
                    plan.stats_info()
                        .HistColl
                        .as_deref()
                        .and_then(|histograms| {
                            histograms.downcast_ref::<statistics_dependency::HistColl>()
                        })
                })
            && histograms.RealtimeCount > 100
        {
            let filter_columns =
                expression_dependency::ExtractColumnsFromExpressions(late_conditions, None);
            let filter_width =
                cardinality_dependency::GetAvgRowSizeDataInDiskByRows(histograms, &filter_columns);
            let total_rows = histograms.RealtimeCount.max(0) as f64 + 10_000.0;
            let first = canonical_term(
                option,
                "lm_col_scan",
                factor_name,
                factor,
                total_rows * filter_width.max(2.0).log2(),
                format!(
                    "{}*logrowsize({filter_width})",
                    canonical_number(total_rows)
                ),
            );
            let rest_width = (width - filter_width).max(2.0);
            let rest = costusage_dependency::new_cost_ver2(
                Some(option),
                canonical_factor(factor_name, factor),
                rows * rest_width.log2() * factor * 1.5,
                || {
                    format!(
                        "lm_rest_col_scan({}*logrowsize({rest_width})*{factor_name}({factor})*lm_scan_factor(1.5))",
                        canonical_number(rows)
                    )
                },
            );
            return Ok(costusage_dependency::mul_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[first, rest]),
                1.0,
            ));
        }
        let scan_rows = rows.max(1.0);
        let scan_cost = one(scan_rows, width);
        if tiflash {
            return Ok(costusage_dependency::mul_cost_ver2(
                &costusage_dependency::sum_cost_ver2(&[scan_cost, one(10_000.0, width)]),
                1.0,
            ));
        }
        return Ok(costusage_dependency::mul_cost_ver2(&scan_cost, 1.0));
    }
    if let Some(projection) = any.downcast_ref::<physicalop::PhysicalProjection>() {
        let child = plan.children().into_iter().next();
        let input_rows = child.map_or(rows, |child| child.stats_count().max(1.0));
        let functions = canonical_num_functions(&projection.Exprs);
        let cpu = if task == property_dependency::MppTaskType {
            2.4
        } else {
            49.9
        };
        let local = canonical_term(
            option,
            "cpu",
            if task == property_dependency::MppTaskType {
                "tiflash_cpu_factor"
            } else {
                "tidb_cpu_factor"
            },
            cpu,
            input_rows * functions,
            format!("{}*filters({functions})", canonical_number(input_rows)),
        );
        let local = costusage_dependency::div_cost_ver2(
            &local,
            canonical_concurrency(
                plan,
                vardef_dependency::TiDBProjectionConcurrency,
                vardef_dependency::DefExecutorConcurrency as f64,
            ),
        );
        let child_cost = child
            .map(|child| cost_of_child(child, task, option, inl, context))
            .transpose()?
            .unwrap_or_else(|| costusage_dependency::new_zero_cost_ver2(false));
        return Ok(costusage_dependency::sum_cost_ver2(&[child_cost, local]));
    }
    if let Some(selection) = any.downcast_ref::<physicalop::PhysicalSelection>() {
        let child = plan.children().into_iter().next();
        let input_rows = child.map_or(rows, |child| child.stats_count().max(1.0));
        let functions = canonical_num_functions(&selection.Conditions);
        let cpu = if task == property_dependency::MppTaskType {
            2.4
        } else {
            49.9
        };
        let local = canonical_term(
            option,
            "cpu",
            if task == property_dependency::MppTaskType {
                "tiflash_cpu_factor"
            } else {
                "tidb_cpu_factor"
            },
            cpu,
            input_rows * functions,
            format!("{}*filters({functions})", canonical_number(input_rows)),
        );
        let child_cost = child
            .map(|child| cost_of_child(child, task, option, inl, context))
            .transpose()?
            .unwrap_or_else(|| costusage_dependency::new_zero_cost_ver2(false));
        return Ok(costusage_dependency::sum_cost_ver2(&[local, child_cost]));
    }
    if any.is::<physicalop::PhysicalMaxOneRow>() {
        let child_cost = plan
            .children()
            .first()
            .map(|child| cost_of_child(*child, task, option, inl, context))
            .transpose()?
            .unwrap_or_else(|| costusage_dependency::new_zero_cost_ver2(false));
        let local = canonical_term(
            option,
            "cpu",
            "tidb_cpu_factor",
            49.9,
            rows.max(1.0) * 0.01,
            format!("{}*filters(0.01)", canonical_number(rows.max(1.0))),
        );
        let total = costusage_dependency::sum_cost_ver2(&[
            child_cost,
            costusage_dependency::div_cost_ver2(&local, 5.0),
        ]);
        return Ok(costusage_dependency::sum_cost_ver2(&[total]));
    }
    if let Some(hash) = any.downcast_ref::<physicalop::PhysicalHashAgg>() {
        let Some(child) = plan.children().into_iter().next() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(false));
        };
        let grouped_index_join_projection = child.as_any().is::<physicalop::PhysicalProjection>()
            && child.children().first().is_some_and(|inner| {
                inner.as_any().is::<physicalop::PhysicalProjection>()
                    && inner.children().first().is_some_and(|join| {
                        matches!(join.tp(&[]).as_str(), "IndexHashJoin" | "IndexMergeJoin")
                    })
            });
        let input_rows = child.stats_count().max(1.0);
        let output_rows = rows.max(1.0);
        let output_width = width.max(2.0);
        let keys = hash.BasePhysicalAgg.GroupByItems.len() as f64;
        let aggs = hash.BasePhysicalAgg.AggFuncs.len() as f64;
        let groups: f64 = hash
            .BasePhysicalAgg
            .GroupByItems
            .iter()
            .map(|item| {
                item.as_any()
                    .downcast_ref::<expression_dependency::Column>()
                    .is_some_and(|column| {
                        column.String().starts_with("Column#") && !grouped_index_join_projection
                    })
                    .then_some(1.0)
                    .unwrap_or_else(|| {
                        if item.as_any().is::<expression_dependency::ScalarFunction>() {
                            1.0
                        } else {
                            0.01
                        }
                    })
            })
            .sum();
        let mpp = task == property_dependency::MppTaskType;
        let cpu = if mpp { 2.4 } else { 49.9 };
        let mem = if mpp { 0.05 } else { 0.2 };
        let cpu_name = if mpp {
            "tiflash_cpu_factor"
        } else {
            "tidb_cpu_factor"
        };
        let mem_name = if mpp {
            "tiflash_mem_factor"
        } else {
            "tidb_mem_factor"
        };
        let concurrency = canonical_concurrency(
            plan,
            vardef_dependency::TiDBHashAggFinalConcurrency,
            vardef_dependency::DefExecutorConcurrency as f64,
        );
        // Projection nodes inserted after physical task selection expose join
        // columns needed by the aggregate, but Go keeps the aggregate's cached
        // child cost from before that post-processing step.  Render the
        // projection's own cost on its row while keeping it out of ancestor
        // aggregate costs.
        let aggregate_cost_child = child
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .and_then(|_| child.children().first().copied())
            .filter(|inner| {
                grouped_index_join_projection
                    || inner.as_any().is::<physicalop::PhysicalHashJoin>()
                    || inner.as_any().is::<physicalop::PhysicalIndexJoin>()
                    || matches!(inner.tp(&[]).as_str(), "IndexHashJoin" | "IndexMergeJoin")
            })
            .unwrap_or(child);
        let child_cost = cost_of_child(aggregate_cost_child, task, option, inl, context)?;
        let start = canonical_term(option, "cpu", cpu_name, cpu, 30.0, "10*3".to_owned());
        let aggregate = canonical_term(
            option,
            "agg",
            cpu_name,
            cpu,
            input_rows * aggs,
            format!("{}*aggs({aggs})", canonical_number(input_rows)),
        );
        let grouping = canonical_term(
            option,
            "group",
            cpu_name,
            cpu,
            input_rows * groups,
            format!(
                "{}*cols({})",
                canonical_number(input_rows),
                canonical_number(groups)
            ),
        );
        // Go's root HashAgg deliberately records the CPU-only hash build as
        // one CostVer2 term (rather than SumCostVer2(hash-key, hash-build)).
        // Besides keeping the factor total identical, this preserves the
        // cost_trace AST shape: `hashkey(...)+hashbuild(...)` is wrapped once
        // by the aggregate's outer SumCostVer2.
        let build = if task == property_dependency::RootTaskType {
            costusage_dependency::new_cost_ver2(
                Some(option),
                canonical_factor(cpu_name, cpu),
                (output_rows * keys + output_rows) * cpu,
                || {
                    format!(
                        "hashkey({}*{keys}*{}({cpu}))+hashbuild({}*{}({cpu}))",
                        canonical_number(output_rows),
                        cpu_name,
                        canonical_number(output_rows),
                        cpu_name,
                    )
                },
            )
        } else {
            costusage_dependency::sum_cost_ver2(&[
                canonical_term(
                    option,
                    "hashkey",
                    cpu_name,
                    cpu,
                    output_rows * keys,
                    format!("{}*{keys}", canonical_number(output_rows)),
                ),
                canonical_term(
                    option,
                    "hashmem",
                    mem_name,
                    mem,
                    output_rows * output_width,
                    format!("{}*{output_width}", canonical_number(output_rows)),
                ),
                canonical_term(
                    option,
                    "hashbuild",
                    cpu_name,
                    cpu,
                    output_rows,
                    canonical_number(output_rows),
                ),
            ])
        };
        let probe = costusage_dependency::sum_cost_ver2(&[
            canonical_term(
                option,
                "hashkey",
                cpu_name,
                cpu,
                input_rows * keys,
                format!("{}*{keys}", canonical_number(input_rows)),
            ),
            canonical_term(
                option,
                "hashprobe",
                cpu_name,
                cpu,
                input_rows,
                canonical_number(input_rows),
            ),
        ]);
        let parallel_parts = vec![aggregate, grouping, build, probe];
        let parallel = costusage_dependency::div_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&parallel_parts),
            concurrency,
        );
        let mut parts = vec![start, child_cost];
        if task == property_dependency::RootTaskType && canonical_child_can_order(child) {
            parts.push(canonical_term(
                option,
                "hashmem",
                mem_name,
                mem,
                concurrency * output_rows * output_width,
                format!("{concurrency}*{output_rows}*{output_width}"),
            ));
        }
        parts.push(parallel);
        return Ok(costusage_dependency::mul_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&parts),
            1.0,
        ));
    }
    if let Some(stream) = any.downcast_ref::<physicalop::PhysicalStreamAgg>() {
        let Some(child) = plan.children().into_iter().next() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(false));
        };
        let input_rows = child.stats_count().max(1.0);
        let aggs = stream.BasePhysicalAgg.AggFuncs.len() as f64;
        let groups = canonical_num_functions(&stream.BasePhysicalAgg.GroupByItems);
        let mpp = task == property_dependency::MppTaskType;
        let cpu = if mpp { 2.4 } else { 49.9 };
        let cpu_name = if mpp {
            "tiflash_cpu_factor"
        } else {
            "tidb_cpu_factor"
        };
        let child_cost = cost_of_child(child, task, option, inl, context)?;
        let aggregate = canonical_term(
            option,
            "agg",
            cpu_name,
            cpu,
            input_rows * aggs,
            format!("{input_rows}*aggs({aggs})"),
        );
        let grouping = canonical_term(
            option,
            "group",
            cpu_name,
            cpu,
            input_rows * groups,
            format!("{input_rows}*cols({groups})"),
        );
        return Ok(costusage_dependency::sum_cost_ver2(&[
            child_cost, aggregate, grouping,
        ]));
    }
    if let Some(sort) = any.downcast_ref::<physicalop::PhysicalSort>() {
        let Some(child) = plan.children().into_iter().next() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(false));
        };
        let input_rows = child.stats_count().max(1.0);
        let mpp = task == property_dependency::MppTaskType;
        let cpu = if mpp { 2.4 } else { 49.9 };
        let mem = if mpp { 0.05 } else { 0.2 };
        let cpu_name = if mpp {
            "tiflash_cpu_factor"
        } else {
            "tidb_cpu_factor"
        };
        let mem_name = if mpp {
            "tiflash_mem_factor"
        } else {
            "tidb_mem_factor"
        };
        let functions = sort
            .ByItems
            .iter()
            .filter(|item| {
                item.Expr
                    .as_any()
                    .is::<expression_dependency::ScalarFunction>()
            })
            .count() as f64;
        let child_cost = cost_of_child(child, task, option, inl, context)?;
        let expr = canonical_term(
            option,
            "exprCPU",
            cpu_name,
            cpu,
            input_rows * functions,
            format!("{}*{functions}", canonical_number(input_rows)),
        );
        let order = canonical_term(
            option,
            "orderCPU",
            cpu_name,
            cpu,
            (input_rows * input_rows.log2()).max(0.0),
            format!("{input_rows}*log({input_rows})"),
        );
        let memory = canonical_term(
            option,
            "sortMem",
            mem_name,
            mem,
            input_rows * width.max(2.0),
            format!("{input_rows}*{}", width.max(2.0)),
        );
        let ordering = costusage_dependency::sum_cost_ver2(&[expr, order]);
        return Ok(costusage_dependency::mul_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[child_cost, ordering, memory]),
            1.0,
        ));
    }
    if let Some(topn) = any.downcast_ref::<physicalop::PhysicalTopN>() {
        let Some(child) = plan.children().into_iter().next() else {
            return Ok(costusage_dependency::new_zero_cost_ver2(false));
        };
        let input_rows = child.stats_count().max(1.0);
        let mut n = (topn.Count + topn.Offset).max(1) as f64;
        if n > 100.0 {
            n = if input_rows < topn.Offset as f64 {
                input_rows + topn.Offset as f64
            } else {
                n.min(input_rows).max(100.0)
            };
        }
        let mpp = task == property_dependency::MppTaskType;
        let cpu = if mpp { 2.4 } else { 49.9 };
        let mem = if mpp { 0.05 } else { 0.2 };
        let cpu_name = if mpp {
            "tiflash_cpu_factor"
        } else {
            "tidb_cpu_factor"
        };
        let mem_name = if mpp {
            "tiflash_mem_factor"
        } else {
            "tidb_mem_factor"
        };
        let functions = topn
            .ByItems
            .iter()
            .filter(|item| {
                item.Expr
                    .as_any()
                    .is::<expression_dependency::ScalarFunction>()
            })
            .count() as f64;
        let child_cost = cost_of_child(child, task, option, inl, context)?;
        let expr = canonical_term(
            option,
            "exprCPU",
            cpu_name,
            cpu,
            input_rows * functions,
            format!("{}*{functions}", canonical_number(input_rows)),
        );
        let order = canonical_term(
            option,
            "orderCPU",
            cpu_name,
            cpu,
            (input_rows * n.log2()).max(0.0),
            format!("{}*log({n})", canonical_number(input_rows)),
        );
        let memory = canonical_term(
            option,
            "topMem",
            mem_name,
            mem,
            n * width.max(2.0),
            format!("{n}*{}", width.max(2.0)),
        );
        let ordering = costusage_dependency::sum_cost_ver2(&[expr, order]);
        return Ok(costusage_dependency::mul_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&[child_cost, ordering, memory]),
            1.0,
        ));
    }
    let cpu_name = if task == property_dependency::MppTaskType {
        "tiflash_cpu_factor"
    } else {
        "tidb_cpu_factor"
    };
    let cpu_value = if task == property_dependency::MppTaskType {
        2.4
    } else {
        49.9
    };
    let mut child_costs = || {
        plan.children()
            .into_iter()
            .map(|child| cost_of_child(child, task, option, inl, context))
            .collect::<Result<Vec<_>, _>>()
            .map(|costs| costusage_dependency::sum_cost_ver2(&costs))
    };
    if let Some(scan) = any.downcast_ref::<physicalop::PhysicalTableScan>() {
        let scan_name = if scan.StoreType == kv_dependency::StoreType::TiFlash {
            "tiflash_scan_factor"
        } else {
            "tikv_scan_factor"
        };
        let scan_factor = if scan.StoreType == kv_dependency::StoreType::TiFlash {
            11.6
        } else {
            40.7
        };
        let one = |charged_rows: f64| {
            canonical_term(
                option,
                "scan",
                scan_name,
                scan_factor,
                charged_rows * width.max(2.0).log2(),
                format!("{charged_rows}*logrowsize({width})"),
            )
        };
        let late_materialization_filter = {
            let equality_residual = scan.FilterCondition.iter().any(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast_dependency::EQ | parser_ast_dependency::NullEQ
                    )
                })
            });
            let columns =
                expression_dependency::ExtractColumnsFromExpressions(&scan.FilterCondition, None);
            !equality_residual
                && columns
                    .iter()
                    .map(|column| column.UniqueID)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    <= 1
        };
        let cost = if scan.StoreType == kv_dependency::StoreType::TiFlash
            && !scan.FilterCondition.is_empty()
            && late_materialization_filter
            && let Some(histograms) = scan
                .TblColHists
                .as_deref()
                .and_then(|histograms| histograms.downcast_ref::<statistics_dependency::HistColl>())
                .or_else(|| {
                    plan.stats_info()
                        .HistColl
                        .as_deref()
                        .and_then(|histograms| {
                            histograms.downcast_ref::<statistics_dependency::HistColl>()
                        })
                }) {
            let filter_columns =
                expression_dependency::ExtractColumnsFromExpressions(&scan.FilterCondition, None);
            let filter_width =
                cardinality_dependency::GetAvgRowSizeDataInDiskByRows(histograms, &filter_columns);
            let total_rows = histograms.RealtimeCount.max(0) as f64 + 10_000.0;
            costusage_dependency::sum_cost_ver2(&[
                canonical_term(
                    option,
                    "lm_col_scan",
                    scan_name,
                    scan_factor,
                    total_rows * filter_width.max(2.0).log2(),
                    format!("{total_rows}*logrowsize({filter_width})"),
                ),
                costusage_dependency::new_cost_ver2(
                    Some(option),
                    canonical_factor(scan_name, scan_factor),
                    rows * (width - filter_width).max(2.0).log2() * scan_factor * 1.5,
                    || {
                        format!(
                            "lm_rest_col_scan({rows}*logrowsize({})*{scan_name}({scan_factor})*lm_scan_factor(1.5))",
                            width - filter_width
                        )
                    },
                ),
            ])
        } else if scan.StoreType == kv_dependency::StoreType::TiFlash {
            costusage_dependency::sum_cost_ver2(&[one(rows), one(10_000.0)])
        } else {
            one(if inl.first().copied().unwrap_or(false) {
                1.0
            } else {
                rows.max(1.0)
            })
        };
        return Ok(costusage_dependency::mul_cost_ver2(&cost, 1.0));
    }
    if let Some(selection) = any.downcast_ref::<physicalop::PhysicalSelection>() {
        let child = child_costs()?;
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count());
        let filters = canonical_num_functions(&selection.Conditions);
        let local = canonical_term(
            option,
            "cpu",
            cpu_name,
            cpu_value,
            input_rows * filters,
            format!("{}*filters({filters})", canonical_number(input_rows)),
        );
        return Ok(costusage_dependency::sum_cost_ver2(&[child, local]));
    }
    if let Some(projection) = any.downcast_ref::<physicalop::PhysicalProjection>() {
        let child = child_costs()?;
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count());
        let filters = canonical_num_functions(&projection.Exprs);
        let local = canonical_term(
            option,
            "cpu",
            cpu_name,
            cpu_value,
            input_rows * filters,
            format!("{}*filters({filters})", canonical_number(input_rows)),
        );
        return Ok(costusage_dependency::sum_cost_ver2(&[
            child,
            costusage_dependency::div_cost_ver2(
                &local,
                canonical_concurrency(
                    plan,
                    vardef_dependency::TiDBProjectionConcurrency,
                    vardef_dependency::DefExecutorConcurrency as f64,
                ),
            ),
        ]));
    }
    if let Some(hash) = any.downcast_ref::<physicalop::PhysicalHashAgg>() {
        let child = child_costs()?;
        let input_rows = plan
            .children()
            .first()
            .map_or(rows.max(1.0), |child| child.stats_count());
        let output_rows = rows.max(1.0);
        let agg_count = hash.BasePhysicalAgg.AggFuncs.len() as f64;
        let group_count = canonical_num_functions(&hash.BasePhysicalAgg.GroupByItems);
        let key_count = hash.BasePhysicalAgg.GroupByItems.len() as f64;
        let mut local = vec![
            canonical_term(
                option,
                "agg",
                cpu_name,
                cpu_value,
                input_rows * agg_count,
                format!("{input_rows}*aggs({agg_count})"),
            ),
            canonical_term(
                option,
                "group",
                cpu_name,
                cpu_value,
                input_rows * group_count,
                format!("{input_rows}*cols({group_count})"),
            ),
            canonical_term(
                option,
                "hashkey",
                cpu_name,
                cpu_value,
                output_rows * key_count,
                format!("{output_rows}*{key_count}"),
            ),
            canonical_term(
                option,
                "hashbuild",
                cpu_name,
                cpu_value,
                output_rows,
                output_rows.to_string(),
            ),
            canonical_term(
                option,
                "hashkey",
                cpu_name,
                cpu_value,
                input_rows * key_count,
                format!("{input_rows}*{key_count}"),
            ),
            canonical_term(
                option,
                "hashprobe",
                cpu_name,
                cpu_value,
                input_rows,
                input_rows.to_string(),
            ),
        ];
        if task == property_dependency::MppTaskType {
            local.push(canonical_term(
                option,
                "hashmem",
                "tiflash_mem_factor",
                0.05,
                output_rows * width,
                format!("{output_rows}*{width}"),
            ));
        }
        let local = costusage_dependency::div_cost_ver2(
            &costusage_dependency::sum_cost_ver2(&local),
            canonical_concurrency(
                plan,
                vardef_dependency::TiDBHashAggFinalConcurrency,
                vardef_dependency::DefExecutorConcurrency as f64,
            ),
        );
        return Ok(costusage_dependency::sum_cost_ver2(&[
            child,
            costusage_dependency::mul_cost_ver2(&local, 1.0),
        ]));
    }
    if let Some(sort) = any.downcast_ref::<physicalop::PhysicalSort>() {
        let child = child_costs()?;
        let input_rows = plan
            .children()
            .first()
            .map_or(rows.max(1.0), |child| child.stats_count().max(1.0));
        let expr_count = sort
            .ByItems
            .iter()
            .filter(|item| {
                item.Expr
                    .as_any()
                    .is::<expression_dependency::ScalarFunction>()
            })
            .count() as f64;
        let order = costusage_dependency::sum_cost_ver2(&[
            canonical_term(
                option,
                "exprCPU",
                cpu_name,
                cpu_value,
                input_rows * expr_count,
                format!("{input_rows}*{expr_count}"),
            ),
            canonical_term(
                option,
                "orderCPU",
                cpu_name,
                cpu_value,
                input_rows * input_rows.log2().max(0.0),
                format!("{input_rows}*log({input_rows})"),
            ),
            canonical_term(
                option,
                "sortMem",
                if task == property_dependency::MppTaskType {
                    "tiflash_mem_factor"
                } else {
                    "tidb_mem_factor"
                },
                if task == property_dependency::MppTaskType {
                    0.05
                } else {
                    0.2
                },
                input_rows * width,
                format!("{input_rows}*{width}"),
            ),
        ]);
        return Ok(costusage_dependency::sum_cost_ver2(&[
            child,
            costusage_dependency::mul_cost_ver2(&order, 1.0),
        ]));
    }
    let cpu_factor = if task == property_dependency::MppTaskType {
        2.40
    } else {
        49.90
    };
    let memory_factor = if task == property_dependency::MppTaskType {
        0.05
    } else {
        0.20
    };
    let (factor_name, factor_value, units) = if let Some(scan) =
        any.downcast_ref::<physicalop::PhysicalTableScan>()
    {
        let factor = if scan.IsMPPOrBatchCop { 11.60 } else { 40.70 };
        let charged_rows = if scan.StoreType == kv_dependency::StoreType::TiFlash {
            rows + 10_000.0
        } else {
            rows.max(1.0)
        };
        (
            "scan",
            factor,
            charged_rows * width.max(2.0).log2() * factor,
        )
    } else if let Some(scan) = any.downcast_ref::<physicalop::PhysicalIndexScan>() {
        ("scan", 40.70, rows * width.max(1.0).log2().max(1.0) * 40.70)
    } else if any.is::<physicalop::PhysicalTableReader>()
        || any.is::<physicalop::PhysicalIndexReader>()
    {
        ("network", 1.0, rows * width)
    } else if any.is::<physicalop::PhysicalExchangeSender>() {
        ("network", 1.0, 0.0)
    } else if any.is::<physicalop::PhysicalExchangeReceiver>() {
        ("network", 1.0, rows * width)
    } else if let Some(reader) = any.downcast_ref::<physicalop::PhysicalIndexLookUpReader>() {
        let order = if reader.KeepOrder {
            rows * rows.max(2.0).log2()
        } else {
            0.0
        };
        ("cpu", 1.0, rows * (323.0 + 1.675 * width) + order + 15.0)
    } else if let Some(projection) = any.downcast_ref::<physicalop::PhysicalProjection>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(1.0));
        let concurrency = plan
            .s_ctx()
            .GetSessionVars()
            .GetSystemVar(vardef_dependency::TiDBProjectionConcurrency)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(vardef_dependency::DefExecutorConcurrency as f64);
        (
            "cpu",
            cpu_factor,
            input_rows * canonical_num_functions(&projection.Exprs) * cpu_factor / concurrency,
        )
    } else if let Some(selection) = any.downcast_ref::<physicalop::PhysicalSelection>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(1.0));
        (
            "cpu",
            cpu_factor,
            input_rows * canonical_num_functions(&selection.Conditions) * cpu_factor,
        )
    } else if any.is::<physicalop::PhysicalSort>() {
        ("cpu", 1.0, rows * rows.max(2.0).log2() + rows * width * 0.2)
    } else if any.is::<physicalop::PhysicalTopN>() {
        ("cpu", 1.0, rows * rows.max(2.0).log2())
    } else if let Some(hash) = any.downcast_ref::<physicalop::PhysicalHashAgg>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows.max(1.0), |child| child.stats_count().max(1.0));
        let output_rows = rows.max(1.0);
        let key_count = hash.BasePhysicalAgg.GroupByItems.len() as f64;
        let concurrency = plan
            .s_ctx()
            .GetSessionVars()
            .GetSystemVar(vardef_dependency::TiDBHashAggFinalConcurrency)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(vardef_dependency::DefExecutorConcurrency as f64);
        let aggregate = input_rows * hash.BasePhysicalAgg.AggFuncs.len() as f64 * cpu_factor;
        let grouping =
            input_rows * canonical_num_functions(&hash.BasePhysicalAgg.GroupByItems) * cpu_factor;
        let hash_build_cpu = output_rows * (key_count + 1.0) * cpu_factor;
        let hash_probe = input_rows * (key_count + 1.0) * cpu_factor;
        let hash_memory = if task == property_dependency::RootTaskType {
            0.0
        } else {
            output_rows * width * memory_factor
        };
        (
            "cpu",
            cpu_factor,
            30.0 * cpu_factor
                + (aggregate + grouping + hash_build_cpu + hash_probe + hash_memory) / concurrency,
        )
    } else if let Some(stream) = any.downcast_ref::<physicalop::PhysicalStreamAgg>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows.max(1.0), |child| child.stats_count().max(1.0));
        let aggregate = input_rows * stream.BasePhysicalAgg.AggFuncs.len() as f64;
        let grouping = input_rows * canonical_num_functions(&stream.BasePhysicalAgg.GroupByItems);
        ("cpu", cpu_factor, (aggregate + grouping) * cpu_factor)
    } else if plan.children().is_empty() {
        ("cpu", 1.0, rows.max(1.0))
    } else {
        ("cpu", 1.0, 0.0)
    };

    let mut costs = plan
        .children()
        .into_iter()
        .map(|child| cost_of_child(child, task, option, inl, context))
        .collect::<Result<Vec<_>, _>>()?;
    if units != 0.0 {
        costs.push(costusage_dependency::new_cost_ver2(
            Some(option),
            costusage_dependency::CostVer2Factor {
                name: factor_name.to_owned(),
                value: factor_value,
            },
            units,
            || format!("canonical({} rows * {} bytes)", rows, width),
        ));
    }
    Ok(costusage_dependency::sum_cost_ver2(&costs))
}

use crate::plan_cost_ver1::{GetPlanCostVer1, PlanCostOption, getCardinality};
use crate::task::{Expression, PlanKind, PlanNode, StoreType, TaskType};
use std::ops::{Add, AddAssign};
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone, Debug, Default)]
/// 单条代价追踪记录：所用因子名、数值与公式字符串。
pub struct CostTrace {
    pub factor: &'static str,
    pub value: f64,
    pub formula: String,
}

#[derive(Clone, Debug, Default)]
/// Ver2 代价：按资源维度拆分，并可附带 trace 列表。
pub struct CostVer2 {
    pub cpu: f64,
    pub memory: f64,
    pub disk: f64,
    pub network: f64,
    pub request: f64,
    pub traces: Vec<CostTrace>,
}
impl CostVer2 {
    /// 按因子名称将 units×Value 记入对应维度，可选记录 trace。
    pub fn new(
        factor: CostVer2Factor,
        units: f64,
        formula: impl Into<String>,
        trace: bool,
    ) -> Self {
        let value = units.max(0.0) * factor.Value;
        let mut c = Self::default();
        match factor.Name {
            "memory" => c.memory = value,
            "disk" => c.disk = value,
            "network" => c.network = value,
            "request" => c.request = value,
            _ => c.cpu = value,
        }
        if trace {
            c.traces.push(CostTrace {
                factor: factor.Name,
                value,
                formula: formula.into(),
            });
        }
        c
    }
    /// 各资源维度代价之和。
    pub fn GetCost(&self) -> f64 {
        self.cpu + self.memory + self.disk + self.network + self.request
    }
    /// 各维度代价同除以 n（至少为 1），用于并发分摊。
    pub fn div(mut self, n: f64) -> Self {
        let n = n.max(1.0);
        self.cpu /= n;
        self.memory /= n;
        self.disk /= n;
        self.network /= n;
        self.request /= n;
        self
    }
}
impl Add for CostVer2 {
    type Output = Self;
    fn add(mut self, rhs: Self) -> Self {
        self += rhs;
        self
    }
}
impl AddAssign for CostVer2 {
    fn add_assign(&mut self, rhs: Self) {
        self.cpu += rhs.cpu;
        self.memory += rhs.memory;
        self.disk += rhs.disk;
        self.network += rhs.network;
        self.request += rhs.request;
        self.traces.extend(rhs.traces);
    }
}

#[derive(Clone, Copy, Debug)]
/// 命名代价因子：Name 决定归入哪一维度，Value 为权重。
pub struct CostVer2Factor {
    pub Name: &'static str,
    pub Value: f64,
}

#[derive(Clone, Debug)]
/// Ver2 默认因子集合：分引擎 CPU/内存/扫描与网络/请求/磁盘。
pub struct costVer2Factors {
    pub tidb_cpu: CostVer2Factor,
    pub tikv_cpu: CostVer2Factor,
    pub tiflash_cpu: CostVer2Factor,
    pub tidb_mem: CostVer2Factor,
    pub tikv_mem: CostVer2Factor,
    pub tiflash_mem: CostVer2Factor,
    pub tikv_scan: CostVer2Factor,
    pub tiflash_scan: CostVer2Factor,
    pub tiflash_desc_scan: CostVer2Factor,
    pub network: CostVer2Factor,
    pub request: CostVer2Factor,
    pub disk: CostVer2Factor,
}
impl Default for costVer2Factors {
    fn default() -> Self {
        Self {
            tidb_cpu: CostVer2Factor {
                Name: "cpu",
                Value: 1.0,
            },
            tikv_cpu: CostVer2Factor {
                Name: "cpu",
                Value: 0.8,
            },
            tiflash_cpu: CostVer2Factor {
                Name: "cpu",
                Value: 0.3,
            },
            tidb_mem: CostVer2Factor {
                Name: "memory",
                Value: 0.2,
            },
            tikv_mem: CostVer2Factor {
                Name: "memory",
                Value: 0.15,
            },
            tiflash_mem: CostVer2Factor {
                Name: "memory",
                Value: 0.1,
            },
            tikv_scan: CostVer2Factor {
                Name: "cpu",
                Value: 1.0,
            },
            tiflash_scan: CostVer2Factor {
                Name: "cpu",
                Value: 0.4,
            },
            tiflash_desc_scan: CostVer2Factor {
                Name: "cpu",
                Value: 0.6,
            },
            network: CostVer2Factor {
                Name: "network",
                Value: 1.0,
            },
            request: CostVer2Factor {
                Name: "request",
                Value: 8.0,
            },
            disk: CostVer2Factor {
                Name: "disk",
                Value: 1.5,
            },
        }
    }
}
/// 返回默认 Ver2 因子表。
pub fn defaultVer2Factors() -> costVer2Factors {
    costVer2Factors::default()
}

type TraceHook = Arc<dyn Fn(&PlanNode, &CostVer2, TaskType, &PlanCostOption) + Send + Sync>;
static GEN_PLAN_COST_TRACE: OnceLock<RwLock<Option<TraceHook>>> = OnceLock::new();
/// 注册/清除全局代价计算追踪钩子。
pub fn SetGenPlanCostTrace(hook: Option<TraceHook>) {
    *GEN_PLAN_COST_TRACE
        .get_or_init(|| RwLock::new(None))
        .write()
        .expect("cost trace hook lock poisoned") = hook;
}

/// 按任务类型选择 TiDB/TiFlash/TiKV CPU 因子。
fn cpu_factor(task: TaskType, f: &costVer2Factors) -> CostVer2Factor {
    match task {
        TaskType::Root => f.tidb_cpu,
        TaskType::Mpp => f.tiflash_cpu,
        _ => f.tikv_cpu,
    }
}
/// 按任务类型选择内存因子。
fn mem_factor(task: TaskType, f: &costVer2Factors) -> CostVer2Factor {
    match task {
        TaskType::Root => f.tidb_mem,
        TaskType::Mpp => f.tiflash_mem,
        _ => f.tikv_mem,
    }
}
/// 按临时表/TiFlash/降序选择扫描因子。
fn scan_factor(plan: &PlanNode, task: TaskType, f: &costVer2Factors) -> CostVer2Factor {
    if plan.flags.temporary_table {
        CostVer2Factor {
            Name: "cpu",
            Value: 0.0,
        }
    } else if plan.store == StoreType::TiFlash || task == TaskType::Mpp {
        if plan.flags.desc {
            f.tiflash_desc_scan
        } else {
            f.tiflash_scan
        }
    } else {
        f.tikv_scan
    }
}
/// 取第 i 个子节点基数。
fn child_rows(p: &PlanNode, i: usize, o: &PlanCostOption) -> f64 {
    p.children
        .get(i)
        .map(|c| getCardinality(c, o.CostFlag))
        .unwrap_or_else(|| getCardinality(p, o.CostFlag))
}
/// 递归计算第 i 个子节点 Ver2 代价。
fn child_cost(p: &mut PlanNode, i: usize, task: TaskType, o: &PlanCostOption) -> CostVer2 {
    p.children
        .get_mut(i)
        .map(|c| GetPlanCostVer2(c, task, o))
        .unwrap_or_default()
}
/// 所有子节点 Ver2 代价之和。
fn all_children(p: &mut PlanNode, task: TaskType, o: &PlanCostOption) -> CostVer2 {
    p.children
        .iter_mut()
        .fold(CostVer2::default(), |a, c| a + GetPlanCostVer2(c, task, o))
}
/// 与 Go `numFunctions` 一致：标量函数计 1，列和常量计经验值 0.01。
fn count_functions(exprs: &[Expression]) -> f64 {
    exprs
        .iter()
        .map(|e| if e.function_count > 0 { 1.0 } else { 0.01 })
        .sum::<f64>()
}

/// 扫描代价：rows × log2(row_size) × factor。
pub fn scanCostVer2(
    option: &PlanCostOption,
    rows: f64,
    size: f64,
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(
        factor,
        rows * size.max(1.0).log2().max(0.0),
        format!("scan({rows}*log2({size}))"),
        option.trace,
    )
}
/// 网络代价：rows × size × factor。
pub fn netCostVer2(
    option: &PlanCostOption,
    rows: f64,
    size: f64,
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(
        factor,
        rows * size,
        format!("net({rows}*{size})"),
        option.trace,
    )
}
/// 过滤代价：rows × 函数复杂度 × factor。
pub fn filterCostVer2(
    option: &PlanCostOption,
    rows: f64,
    filters: &[Expression],
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(
        factor,
        rows * count_functions(filters),
        format!("filter({rows}*{})", count_functions(filters)),
        option.trace,
    )
}
/// 聚合函数 CPU 代价。
pub fn aggCostVer2(
    option: &PlanCostOption,
    rows: f64,
    funcs: &[Expression],
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(
        factor,
        rows * funcs.len() as f64,
        format!("agg({rows}*{})", funcs.len()),
        option.trace,
    )
}
/// 分组表达式 CPU 代价。
pub fn groupCostVer2(
    option: &PlanCostOption,
    rows: f64,
    groups: &[Expression],
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(
        factor,
        rows * count_functions(groups),
        format!("group({rows}*{})", count_functions(groups)),
        option.trace,
    )
}
/// 公开包装：统计表达式函数复杂度。
pub fn numFunctions(exprs: &[Expression]) -> f64 {
    count_functions(exprs)
}
/// 排序代价：rows × log2(n) × 排序键复杂度。
pub fn orderCostVer2(
    option: &PlanCostOption,
    rows: f64,
    n: f64,
    items: &[Expression],
    factor: CostVer2Factor,
) -> CostVer2 {
    let scalar_functions = items
        .iter()
        .filter(|expression| expression.function_count > 0)
        .count() as f64;
    CostVer2::new(
        factor,
        rows * scalar_functions,
        format!("exprCPU({rows}*{scalar_functions})"),
        option.trace,
    ) + CostVer2::new(
        factor,
        rows * n.log2().max(0.0),
        format!("orderCPU({rows}*log2({n}))"),
        option.trace,
    )
}
/// Hash Build：建表 CPU + 行宽内存。
pub fn hashBuildCostVer2(
    option: &PlanCostOption,
    rows: f64,
    size: f64,
    keys: f64,
    cpu: CostVer2Factor,
    mem: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(cpu, rows * keys, "hash-key", option.trace)
        + CostVer2::new(cpu, rows, "hash-build-cpu", option.trace)
        + CostVer2::new(mem, rows * size.max(1.0), "hash-build-memory", option.trace)
}
/// Hash Probe：探测侧 CPU。
pub fn hashProbeCostVer2(
    option: &PlanCostOption,
    rows: f64,
    keys: f64,
    factor: CostVer2Factor,
) -> CostVer2 {
    CostVer2::new(factor, rows * keys, "hash-key", option.trace)
        + CostVer2::new(factor, rows, "hash-probe", option.trace)
}
/// 回表/双读请求代价。
pub fn doubleReadCostVer2(option: &PlanCostOption, tasks: f64, factor: CostVer2Factor) -> CostVer2 {
    CostVer2::new(factor, tasks, "double-read", option.trace)
}
/// IndexJoin seek：外层行 × ranges 次请求。
pub fn indexJoinSeekingCostVer2(
    option: &PlanCostOption,
    build_rows: f64,
    ranges: f64,
    factor: CostVer2Factor,
) -> CostVer2 {
    if build_rows <= 1.0 || ranges <= 1.0 {
        return CostVer2::default();
    }
    CostVer2::new(
        factor,
        build_rows * 10.0 * 8.0_f64.log2() * ranges,
        format!("seeking({build_rows}*{ranges}*10*log2(8))"),
        option.trace,
    )
}

/// TiFlash 表扫额外惩罚：小扫、宽列、keep_order；TiKV/临时表为 0。
pub fn getTableScanPenalty(plan: &PlanNode, rows: f64) -> f64 {
    if plan.store != StoreType::TiFlash || plan.flags.temporary_table {
        return 0.0;
    }
    let small_scan = (10_000.0 - rows).max(0.0) / 10_000.0 * 500.0;
    let column_penalty = plan.schema.len().saturating_sub(20) as f64 * rows.sqrt();
    let order_penalty = if plan.flags.keep_order {
        rows * 0.1
    } else {
        0.0
    };
    small_scan + column_penalty + order_penalty
}

/// IndexJoin 族共用逻辑；merge/hash 标志分别叠加排序或哈希建表代价。
fn index_join_cost(
    plan: &mut PlanNode,
    task: TaskType,
    option: &PlanCostOption,
    f: &costVer2Factors,
    merge: bool,
    hash: bool,
) -> CostVer2 {
    let outer = child_rows(plan, 1usize.saturating_sub(plan.inner_child), option);
    let inner = child_rows(plan, plan.inner_child.min(1), option);
    let child = all_children(plan, task, option);
    let ranges = plan.ranges.max(1) as f64;
    let mut total = child
        + indexJoinSeekingCostVer2(option, outer, ranges, f.request)
        + netCostVer2(option, outer * inner, plan.row_size(), f.network);
    if merge {
        total += orderCostVer2(option, outer, outer, &plan.by_items, cpu_factor(task, f));
    }
    if hash {
        total += hashBuildCostVer2(
            option,
            outer,
            plan.row_size(),
            plan.join_keys as f64,
            cpu_factor(task, f),
            mem_factor(task, f),
        );
    }
    total
        + hashProbeCostVer2(
            option,
            outer * inner,
            plan.join_keys as f64,
            cpu_factor(task, f),
        )
}

/// 递归累加计划树 ranges 数量。
pub fn getNumberOfRanges(plan: &PlanNode) -> usize {
    plan.ranges + plan.children.iter().map(getNumberOfRanges).sum::<usize>()
}

/// Ver2 主入口：按算子分派，可选调用全局 trace 钩子。
pub fn GetPlanCostVer2(plan: &mut PlanNode, task: TaskType, option: &PlanCostOption) -> CostVer2 {
    let f = defaultVer2Factors();
    let rows = getCardinality(plan, option.CostFlag);
    let cpu = cpu_factor(task, &f);
    let mem = mem_factor(task, &f);
    let cost = match plan.kind {
        PlanKind::Selection => {
            let input = child_rows(plan, 0, option);
            let child = child_cost(plan, 0, task, option);
            child
                + if plan.flags.from_data_source {
                    CostVer2::default()
                } else {
                    filterCostVer2(option, input, &plan.conditions, cpu)
                }
        }
        PlanKind::Projection => {
            let input = child_rows(plan, 0, option);
            child_cost(plan, 0, task, option)
                + CostVer2::new(
                    cpu,
                    input * count_functions(&plan.expressions),
                    "projection",
                    option.trace,
                )
        }
        PlanKind::IndexScan | PlanKind::TableScan => {
            let mut c = scanCostVer2(option, rows, plan.row_size(), scan_factor(plan, task, &f));
            if plan.kind == PlanKind::TableScan {
                c += CostVer2::new(
                    cpu,
                    getTableScanPenalty(plan, rows),
                    "tiflash-table-penalty",
                    option.trace,
                );
            }
            c + filterCostVer2(option, rows, &plan.conditions, cpu)
        }
        PlanKind::IndexReader | PlanKind::TableReader => {
            let child_task = if plan.store == StoreType::TiFlash {
                TaskType::Mpp
            } else {
                TaskType::CopSingleRead
            };
            let child = child_cost(plan, 0, child_task, option);
            let source = plan.children.first().unwrap_or(plan);
            (child
                + netCostVer2(option, source.rows(), source.row_size(), f.network)
                + CostVer2::new(
                    f.request,
                    source.ranges.max(1) as f64,
                    "reader-requests",
                    option.trace,
                ))
            .div(option.factors.dist_sql_concurrency as f64)
        }
        PlanKind::IndexLookupReader => {
            let child = all_children(plan, TaskType::CopMultiRead, option);
            let network = plan.children.iter().fold(CostVer2::default(), |a, c| {
                a + netCostVer2(option, c.rows(), c.row_size(), f.network)
            });
            let index_rows = plan.children.first().map(PlanNode::rows).unwrap_or(rows);
            child.div(option.factors.dist_sql_concurrency as f64)
                + network
                + doubleReadCostVer2(
                    option,
                    index_rows / option.factors.index_lookup_size.max(1) as f64,
                    f.request,
                )
                + orderCostVer2(
                    option,
                    index_rows,
                    option.factors.index_lookup_size as f64,
                    &plan.by_items,
                    cpu,
                )
        }
        PlanKind::IndexMergeReader => {
            let child = all_children(plan, TaskType::CopSingleRead, option);
            let network = plan.children.iter().fold(CostVer2::default(), |a, c| {
                a + netCostVer2(option, c.rows(), c.row_size(), f.network)
            });
            (child + network + doubleReadCostVer2(option, plan.children.len() as f64, f.request))
                .div(option.factors.dist_sql_concurrency as f64)
        }
        PlanKind::Sort => {
            let input = child_rows(plan, 0, option);
            child_cost(plan, 0, task, option)
                + orderCostVer2(option, input, input, &plan.by_items, cpu)
                + CostVer2::new(mem, input * plan.row_size(), "sort-memory", option.trace)
        }
        PlanKind::TopN => {
            let input = child_rows(plan, 0, option);
            let n = (plan.offset + plan.count).max(2) as f64;
            child_cost(plan, 0, task, option)
                + orderCostVer2(option, input, n, &plan.by_items, cpu)
                + CostVer2::new(mem, n * plan.row_size(), "topn-memory", option.trace)
        }
        PlanKind::StreamAgg => {
            let input = child_rows(plan, 0, option);
            child_cost(plan, 0, task, option)
                + aggCostVer2(option, input, &plan.agg_funcs, cpu)
                + groupCostVer2(option, input, &plan.group_items, cpu)
        }
        PlanKind::HashAgg => {
            let input = child_rows(plan, 0, option);
            child_cost(plan, 0, task, option)
                // CPU/分组可按并发分摊；内存代价单独累加，不整除 concurrency。
                + (aggCostVer2(option, input, &plan.agg_funcs, cpu)
                    + groupCostVer2(option, input, &plan.group_items, cpu))
                .div(plan.concurrency as f64)
                + CostVer2::new(
                    mem,
                    plan.rows() * plan.row_size(),
                    "hash-agg-memory",
                    option.trace,
                )
        }
        PlanKind::MergeJoin => {
            let left = child_rows(plan, 0, option);
            let right = child_rows(plan, 1, option);
            all_children(plan, task, option)
                + CostVer2::new(
                    cpu,
                    (left + right) * plan.join_keys.max(1) as f64,
                    "merge-join",
                    option.trace,
                )
        }
        PlanKind::HashJoin => {
            let build = child_rows(plan, plan.inner_child.min(1), option);
            let probe = child_rows(plan, 1usize.saturating_sub(plan.inner_child), option);
            all_children(plan, task, option)
                + hashBuildCostVer2(
                    option,
                    build,
                    plan.row_size(),
                    plan.join_keys as f64,
                    cpu,
                    mem,
                )
                + hashProbeCostVer2(option, probe, plan.join_keys as f64, cpu)
                    .div(plan.concurrency as f64)
        }
        PlanKind::IndexJoin => index_join_cost(plan, task, option, &f, false, false),
        PlanKind::IndexHashJoin => index_join_cost(plan, task, option, &f, false, true),
        PlanKind::IndexMergeJoin => index_join_cost(plan, task, option, &f, true, false),
        PlanKind::Apply => {
            let outer = child_rows(plan, 0, option);
            let inner = child_rows(plan, 1, option);
            let left = child_cost(plan, 0, task, option);
            let right = child_cost(plan, 1, task, option);
            left + CostVer2 {
                cpu: right.cpu * outer,
                memory: right.memory * outer,
                disk: right.disk * outer,
                network: right.network * outer,
                request: right.request * outer,
                traces: right.traces,
            } + CostVer2::new(cpu, outer * inner, "apply", option.trace)
        }
        PlanKind::UnionAll => {
            all_children(plan, task, option)
                + CostVer2::new(cpu, rows, "union-all", option.trace).div(plan.concurrency as f64)
        }
        PlanKind::PointGet => {
            netCostVer2(option, 1.0, plan.row_size(), f.network)
                + CostVer2::new(f.request, 1.0, "point-get", option.trace)
        }
        PlanKind::BatchPointGet => {
            netCostVer2(option, rows, plan.row_size(), f.network)
                + CostVer2::new(
                    f.request,
                    plan.ranges.max(1) as f64,
                    "batch-point-get",
                    option.trace,
                )
        }
        PlanKind::ExchangeReceiver => {
            all_children(plan, TaskType::Mpp, option)
                + netCostVer2(option, rows, plan.row_size(), f.network)
        }
        PlanKind::Cte => {
            let child = all_children(plan, task, option);
            let io = if plan.flags.use_cache { mem } else { f.disk };
            child
                + CostVer2::new(
                    io,
                    rows * plan.row_size(),
                    "cte-materialization",
                    option.trace,
                )
        }
        _ => all_children(plan, task, option),
    };
    if let Some(lock) = GEN_PLAN_COST_TRACE.get() {
        if let Some(hook) = lock.read().expect("cost trace hook lock poisoned").as_ref() {
            hook(plan, &cost, task, option);
        }
    }
    cost
}

/// 按 model_version 选择 Ver1 或 Ver2，返回标量总代价。
pub fn GetPlanCost(
    plan: &mut PlanNode,
    task: TaskType,
    option: &PlanCostOption,
    model_version: i32,
) -> f64 {
    if model_version == 2 {
        GetPlanCostVer2(plan, task, option).GetCost()
    } else {
        GetPlanCostVer1(plan, task, option)
    }
}
