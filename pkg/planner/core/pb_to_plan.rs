// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Protobuf 执行器描述到物理执行计划的转换。
//
// 将远端/序列化侧的 `PBExecutor` 列表自底向上组装为树形 `PlanNode`
// （物理执行计划），并做谓词下推（predicate push-down）：把过滤条件
// 尽量推到 TableScan，减少上层算子处理的行数。当前仅支持集群表扫描等子集。

use crate::{PlanKind, PlanNode, PlannerContext};
use std::collections::BTreeMap;

/// Protobuf 侧的列元信息（列 ID、名称与字段类型）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PBColumnInfo {
    /// 列唯一 ID。
    pub id: i64,
    /// 列名。
    pub name: String,
    /// 字段类型的文本表示。
    pub field_type: String,
}
/// Protobuf 侧的表元信息，含库名、是否为集群表及列列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PBTableInfo {
    /// 表唯一 ID。
    pub id: i64,
    /// 所属数据库名。
    pub database: String,
    /// 表名。
    pub name: String,
    /// 是否为集群表（本转换路径仅允许扫描集群表）。
    pub cluster_table: bool,
    /// 表中全部列的元信息。
    pub columns: Vec<PBColumnInfo>,
}

/// Protobuf 序列化的执行器（算子）描述，对应一条物理算子配置。
#[derive(Clone, Debug, PartialEq)]
pub enum PBExecutor {
    /// 表扫描：按表 ID 与列 ID 列表读数据，`desc` 表示逆序扫描。
    TableScan {
        table_id: i64,
        columns: Vec<i64>,
        desc: bool,
    },
    /// 过滤（Selection）：保留满足条件的行。
    Selection { conditions: Vec<String> },
    /// 投影（Projection）：计算并输出指定表达式列。
    Projection { expressions: Vec<String> },
    /// TopN：按排序键取前 N 行，含 offset 与 limit。
    TopN {
        by_items: Vec<String>,
        offset: u64,
        limit: u64,
    },
    /// Limit：跳过 offset 后最多返回 limit 行。
    Limit { offset: u64, limit: u64 },
    /// 聚合：普通 HashAgg 或流式 StreamAgg（`stream` 为真时）。
    Aggregation {
        functions: Vec<String>,
        group_by: Vec<String>,
        stream: bool,
    },
    /// 终止连接或当前查询（Kill）。
    Kill { connection_id: u64, query: bool },
    /// 向各节点广播执行的查询文本。
    BroadcastQuery { query: String },
    /// 尚不支持的执行器类型，构建时返回错误。
    Unsupported(String),
}

/// 将 Protobuf 执行器列表构建为物理计划树的构建器。
pub struct PBPlanBuilder {
    /// 会话级规划上下文。
    sctx: PlannerContext,
    /// 最近一次表扫描转换得到的字段类型列表。
    field_types: Vec<String>,
    /// 按表 ID 索引的表元信息。
    tables: BTreeMap<i64, PBTableInfo>,
    /// 扫描范围列表，每项为 (start_key, end_key) 字节区间。
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
}

/// 用会话上下文、表元信息与扫描范围创建 `PBPlanBuilder`。
pub fn NewPBPlanBuilder(
    sctx: PlannerContext,
    tables: Vec<PBTableInfo>,
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
) -> PBPlanBuilder {
    PBPlanBuilder {
        sctx,
        field_types: Vec::new(),
        tables: tables.into_iter().map(|table| (table.id, table)).collect(),
        ranges,
    }
}

impl PBPlanBuilder {
    /// 按执行器列表自底向上构建物理计划，并做谓词下推。
    pub fn Build(&mut self, executors: &[PBExecutor]) -> Result<PlanNode, String> {
        let mut source = None;
        // 列表从前到后为叶子到根；每次将已构建子树作为当前算子的子节点。
        for executor in executors {
            source = Some(self.pbToPhysicalPlan(executor, source)?);
        }
        let source = source.ok_or_else(|| "executor list is empty".to_owned())?;
        let (_, source) = self.predicatePushDown(source, Vec::new());
        Ok(source)
    }

    /// 将单个 `PBExecutor` 转为 `PlanNode`，并挂上可选的子计划。
    fn pbToPhysicalPlan(
        &mut self,
        executor: &PBExecutor,
        sub_plan: Option<PlanNode>,
    ) -> Result<PlanNode, String> {
        let mut plan = match executor {
            PBExecutor::TableScan {
                table_id,
                columns,
                desc,
            } => self.pbToTableScan(*table_id, columns, *desc)?,
            PBExecutor::Selection { conditions } => PlanNode::New(
                0,
                PlanKind::Selection {
                    conditions: conditions.clone(),
                },
                Vec::new(),
            ),
            PBExecutor::Projection { expressions } => {
                let mut plan = PlanNode::New(0, PlanKind::Projection, Vec::new());
                plan.operator_info = expressions.join(", ");
                plan
            }
            PBExecutor::TopN {
                by_items,
                offset,
                limit,
            } => PlanNode::New(
                0,
                PlanKind::TopN {
                    by_items: by_items.clone(),
                    offset: *offset,
                    count: *limit,
                },
                Vec::new(),
            ),
            PBExecutor::Limit { offset, limit } => PlanNode::New(
                0,
                PlanKind::Limit {
                    offset: *offset,
                    count: *limit,
                },
                Vec::new(),
            ),
            PBExecutor::Aggregation {
                functions,
                group_by,
                stream,
            } => {
                let mut plan = PlanNode::New(
                    0,
                    if *stream {
                        PlanKind::StreamAgg
                    } else {
                        PlanKind::Aggregation {
                            functions: functions.clone(),
                        }
                    },
                    Vec::new(),
                );
                plan.operator_info = format!(
                    "functions:[{}], group-by:[{}]",
                    functions.join(","),
                    group_by.join(",")
                );
                plan
            }
            PBExecutor::Kill {
                connection_id,
                query,
            } => PlanNode::New(
                0,
                PlanKind::Generic(format!("Kill(connection={connection_id},query={query})")),
                Vec::new(),
            ),
            PBExecutor::BroadcastQuery { query } => {
                self.validateBroadcastQuery(query)?;
                PlanNode::New(
                    0,
                    PlanKind::Generic(format!("BroadcastQuery({query})")),
                    Vec::new(),
                )
            }
            PBExecutor::Unsupported(kind) => {
                return Err(format!("this exec type {kind} doesn't support yet"));
            }
        };
        // 将已构建的子计划挂到当前算子下，形成树。
        if let Some(child) = sub_plan {
            plan.children.push(child);
        }
        // Limit 算子展示信息沿用子节点，便于 EXPLAIN 阅读。
        if matches!(plan.kind, PlanKind::Limit { .. }) {
            if let Some(child) = plan.children.first() {
                plan.operator_info = child.operator_info.clone();
            }
        }
        Ok(plan)
    }

    /// 根据表 ID 与列 ID 构建 TableScan 物理计划节点。
    fn pbToTableScan(
        &mut self,
        table_id: i64,
        columns: &[i64],
        desc: bool,
    ) -> Result<PlanNode, String> {
        let table = self
            .tables
            .get(&table_id)
            .ok_or_else(|| format!("table which ID = {table_id} does not exist"))?;
        if !table.cluster_table {
            return Err(format!("table {} is not a cluster table", table.name));
        }
        let converted = self.convertColumnInfo(table, columns)?;
        self.field_types = converted
            .iter()
            .map(|column| column.field_type.clone())
            .collect();
        let mut plan = PlanNode::New(
            0,
            PlanKind::TableScan {
                table: table.name.clone(),
            },
            Vec::new(),
        );
        plan.operator_info = format!(
            "db:{}, columns:[{}], desc:{desc}, ranges:{}",
            table.database,
            converted
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
                .join(","),
            self.ranges.len()
        );
        Ok(plan)
    }

    /// 按列 ID 列表从表元信息中解析出对应的列描述。
    fn convertColumnInfo(
        &self,
        table: &PBTableInfo,
        ids: &[i64],
    ) -> Result<Vec<PBColumnInfo>, String> {
        ids.iter()
            .map(|id| {
                table
                    .columns
                    .iter()
                    .find(|column| column.id == *id)
                    .cloned()
                    .ok_or_else(|| format!("column id {id} does not exist in table {}", table.name))
            })
            .collect()
    }

    /// 按选中列在表定义中的顺序生成扫描 Schema（列名列表）。
    pub fn buildTableScanSchema(
        &self,
        table: &PBTableInfo,
        columns: &[PBColumnInfo],
    ) -> Vec<String> {
        table
            .columns
            .iter()
            .flat_map(|column| {
                columns
                    .iter()
                    .filter(move |selected| selected.id == column.id)
                    .map(|_| column.name.clone())
            })
            .collect()
    }

    /// 校验广播查询是否属于 Go 实现允许远端执行的管理语句集合。
    fn validateBroadcastQuery(&self, query: &str) -> Result<(), String> {
        let normalized = query
            .trim()
            .trim_end_matches(';')
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let allowed = matches!(
            normalized.as_slice(),
            [admin, reload, bindings]
                if admin == "admin" && reload == "reload" && bindings == "bindings"
        ) || matches!(
            normalized.as_slice(),
            [flush, stats_delta] if flush == "flush" && stats_delta == "stats_delta"
        ) || matches!(
            normalized.as_slice(),
            [refresh, stats] if refresh == "refresh" && stats == "stats"
        );
        if allowed {
            Ok(())
        } else {
            Err(format!("unexpected statement {query} in broadcast query"))
        }
    }

    /// 谓词下推：合并 Selection 条件并向子树传递，落在 TableScan 时写入 operator_info。
    ///
    /// 返回仍未消化的谓词与改写后的计划树。
    pub fn predicatePushDown(
        &self,
        mut plan: PlanNode,
        mut predicates: Vec<String>,
    ) -> (Vec<String>, PlanNode) {
        // Selection 本身被吸收：条件并入 predicates，继续对其子节点下推。
        if let PlanKind::Selection { conditions } = &plan.kind {
            let parent_predicates = predicates.clone();
            predicates.extend(conditions.clone());
            if let Some(child) = plan.children.pop() {
                let (remaining, child) = self.predicatePushDown(child, predicates);
                if remaining.is_empty() {
                    return (parent_predicates, child);
                }
                plan.kind = PlanKind::Selection {
                    conditions: remaining,
                };
                plan.children.push(child);
                return (parent_predicates, plan);
            }
        }
        // Go 版本仅允许配置了 extractor 的内存表消费谓词；其它集群表必须
        // 将谓词留给 Selection 执行，不能在扫描侧静默丢弃。
        if let PlanKind::TableScan { table } = &plan.kind {
            let has_extractor = matches!(
                table.to_ascii_uppercase().as_str(),
                "CLUSTER_SLOW_QUERY"
                    | "CLUSTER_STATEMENTS_SUMMARY"
                    | "CLUSTER_STATEMENTS_SUMMARY_HISTORY"
                    | "CLUSTER_TIDB_INDEX_USAGE"
            );
            if has_extractor {
                plan.operator_info
                    .push_str(&format!(", pushed:[{}]", predicates.join(",")));
                return (Vec::new(), plan);
            }
            return (predicates, plan);
        }
        if let Some(child) = plan.children.pop() {
            // 非 Selection 算子是下推边界。仍递归其子树，以处理子树内部自己的
            // Selection，但不得把当前上层谓词穿过 Projection/TopN/Limit/Agg。
            let (_, child) = self.predicatePushDown(child, Vec::new());
            plan.children.push(child);
            (predicates, plan)
        } else {
            (predicates, plan)
        }
    }

    /// 返回构建器持有的会话规划上下文。
    pub fn SessionContext(&self) -> &PlannerContext {
        &self.sctx
    }
}
