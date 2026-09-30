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

// 物理算子：哈希连接（PhysicalHashJoin）。
//
// 哈希连接先把 build 侧建哈希表，再以 probe 侧逐行探测匹配。
// HashJoin V2 对等值键与连接类型有 GA（正式可用）范围限制；
// 还可挂接 Runtime Filter（运行时过滤）以减少探测侧无效扫描。

use base::{ContextRef, JoinType, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, ExprBox, ScalarFunction};

use crate::{BasePhysicalJoin, RuntimeFilter, RuntimeFilterBuildNode, RuntimeFilterType};

/// 判断给定连接形态是否落在 HashJoin V2 的 GA（正式可用）范围内。
pub fn IsGAForHashJoinV2(
    join_type: JoinType,
    left_keys: &[expression::Column],
    null_eq: &[bool],
    na_keys: &[expression::Column],
) -> bool {
    // 要求：无 NA 等值键、有左连接键、无 NULL-EQ，且连接类型在 GA 白名单内。
    na_keys.is_empty()
        && !left_keys.is_empty()
        && !null_eq.iter().any(|v| *v)
        && matches!(
            join_type,
            JoinType::LeftOuterJoin
                | JoinType::RightOuterJoin
                | JoinType::InnerJoin
                | JoinType::SemiJoin
                | JoinType::AntiSemiJoin
        )
}

/// 判断能否启用 HashJoin V2（比 GA 集合更宽，含部分 Outer Semi）。
pub fn CanUseHashJoinV2(
    join_type: JoinType,
    left_keys: &[expression::Column],
    null_eq: &[bool],
    na_keys: &[expression::Column],
) -> bool {
    can_use_hash_join_v2_with_non_ga(join_type, left_keys, null_eq, na_keys, true)
}

pub(crate) fn can_use_hash_join_v2_with_non_ga(
    join_type: JoinType,
    left_keys: &[expression::Column],
    null_eq: &[bool],
    na_keys: &[expression::Column],
    allow_non_ga: bool,
) -> bool {
    if !IsGAForHashJoinV2(join_type, left_keys, null_eq, na_keys) && !allow_non_ga {
        return false;
    }
    na_keys.is_empty()
        && !left_keys.is_empty()
        && !null_eq.iter().any(|v| *v)
        && matches!(
            join_type,
            JoinType::LeftOuterJoin
                | JoinType::RightOuterJoin
                | JoinType::InnerJoin
                | JoinType::LeftOuterSemiJoin
                | JoinType::SemiJoin
                | JoinType::AntiSemiJoin
                | JoinType::AntiLeftOuterSemiJoin
        )
}

pub(crate) fn can_tiflash_use_hash_join_v2(
    version: &str,
    max_bytes_before_external_join: i64,
    max_query_memory_per_node: i64,
    query_spill_ratio: f64,
    join_type: JoinType,
    has_join_keys: bool,
    has_na_join_keys: bool,
    has_null_eq: bool,
) -> bool {
    version.eq_ignore_ascii_case("optimized")
        && max_bytes_before_external_join <= 0
        && !(max_query_memory_per_node > 0 && query_spill_ratio > 0.0)
        && join_type == JoinType::InnerJoin
        && has_join_keys
        && !has_na_join_keys
        && !has_null_eq
}

/// 物理哈希连接：等值/NA 等值条件、并发度、MPP shuffle 与运行时过滤。
pub struct PhysicalHashJoin {
    /// 连接公共基座：连接类型、键与两侧条件。
    pub BasePhysicalJoin: BasePhysicalJoin,
    /// 探测侧并行 worker 数。
    pub Concurrency: u64,
    /// 普通等值连接条件。
    pub EqualConditions: Vec<ScalarFunction>,
    /// Null-Aware（空值感知）等值条件。
    pub NAEqualConditions: Vec<ScalarFunction>,
    /// 为 true 时用外表（outer）建哈希表。
    pub UseOuterToBuild: bool,
    /// 目标存储类型（TiKV / TiFlash 等）。
    pub StoreTp: kv::StoreType,
    /// 是否为 MPP（大规模并行处理）shuffle 连接。
    pub MppShuffleJoin: bool,
    /// 该候选是否由当前 Join 可见名匹配的 HASH_JOIN hint 指定。
    pub FromHashJoinHint: bool,
    /// 当前逻辑 Join 子树是否使用 SQL 表别名。
    pub HasTableAlias: bool,
    /// 已注册的运行时过滤实例。
    pub RuntimeFilterList: Vec<RuntimeFilter>,
    /// 本节点支持的运行时过滤类型列表。
    pub RuntimeFilterTypes: Vec<RuntimeFilterType>,
}

/// 构造哈希连接，默认 TiKV、无 MPP shuffle、空等值条件。
pub fn NewPhysicalHashJoin(
    base: BasePhysicalJoin,
    concurrency: u64,
    use_outer_to_build: bool,
) -> PhysicalHashJoin {
    PhysicalHashJoin {
        BasePhysicalJoin: base,
        Concurrency: concurrency,
        EqualConditions: Vec::new(),
        NAEqualConditions: Vec::new(),
        UseOuterToBuild: use_outer_to_build,
        StoreTp: kv::StoreType::TiKV,
        MppShuffleJoin: false,
        FromHashJoinHint: false,
        HasTableAlias: false,
        RuntimeFilterList: Vec::new(),
        RuntimeFilterTypes: Vec::new(),
    }
}

impl PhysicalHashJoin {
    /// Remove repeated logical output columns before resolving child slots.
    /// Hash-join schemas carry each source column once; projections above the
    /// join represent repeated SQL references as separate expressions.
    pub(super) fn DeduplicateOutputColumns(
        output: &mut expression::Schema,
        columns_to_resolve: usize,
    ) -> usize {
        let mut seen = std::collections::HashSet::new();
        let mut columns = output
            .Columns
            .iter()
            .take(columns_to_resolve)
            .filter(|column| seen.insert(column.UniqueID))
            .map(expression::Column::Clone)
            .collect::<Vec<_>>();
        columns.extend(
            output
                .Columns
                .iter()
                .skip(columns_to_resolve)
                .map(expression::Column::Clone),
        );
        let resolved_columns = columns
            .len()
            .saturating_sub(output.Columns.len().saturating_sub(columns_to_resolve));
        output.Columns = columns;
        resolved_columns
    }

    /// Resolve output columns to their unique child slots, matching Go's
    /// `ResolveIndicesItself` behavior.
    pub(super) fn ResolveOutputColumns(
        output: &mut expression::Schema,
        children: &expression::Schema,
        columns_to_resolve: usize,
    ) -> usize {
        let mut marked = vec![false; children.Columns.len()];
        let mut resolved = 0;
        for column in output.Columns.iter_mut().take(columns_to_resolve) {
            let child_index = children
                .Columns
                .iter()
                .enumerate()
                .find(|(index, candidate)| !marked[*index] && column.EqualColumn(*candidate))
                .map(|(index, _)| index);
            if let Some(index) = child_index {
                let mut replacement = column.Clone();
                replacement.Index = index as isize;
                *column = replacement;
                marked[index] = true;
                resolved += 1;
            }
        }
        resolved
    }

    /// 初始化基座物理计划：类型 HashJoin、统计信息与子节点所需物理属性。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        let plan = &mut self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP("HashJoin");
        plan.Plan.SetQueryBlockOffset(offset);
        plan.set_stats(stats);
        plan.SetChildrenReqProps(props);
        self
    }
    /// 挂接子 Task。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }
    /// 基于本节点字段判断能否使用 HashJoin V2。
    pub fn CanUseHashJoinV2(&self) -> bool {
        CanUseHashJoinV2(
            self.BasePhysicalJoin.JoinType,
            &self.BasePhysicalJoin.LeftJoinKeys,
            &self.BasePhysicalJoin.IsNullEQ,
            &self.BasePhysicalJoin.LeftNAJoinKeys,
        )
    }
    /// TiFlash 路径下是否允许 HashJoin V2（当前要求 InnerJoin）。
    pub fn CanTiFlashUseHashJoinV2(&self, sctx: &dyn base::PlanContext) -> bool {
        let vars = sctx.GetSessionVars();
        let system_i64 = |name: &str, default: i64| {
            vars.GetSystemVar(name)
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let system_f64 = |name: &str, default: f64| {
            vars.GetSystemVar(name)
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        can_tiflash_use_hash_join_v2(
            vars.GetSystemVar(vardef::TiFlashHashJoinVersion)
                .as_deref()
                .unwrap_or(vardef::DefTiFlashHashJoinVersion),
            system_i64(
                vardef::TiDBMaxBytesBeforeTiFlashExternalJoin,
                vardef::DefTiFlashMaxBytesBeforeExternalJoin,
            ),
            system_i64(vardef::TiFlashMemQuotaQueryPerNode, -1),
            system_f64(vardef::TiFlashQuerySpillRatio, 0.0),
            self.BasePhysicalJoin.JoinType,
            !self.BasePhysicalJoin.LeftJoinKeys.is_empty(),
            !self.BasePhysicalJoin.LeftNAJoinKeys.is_empty(),
            self.BasePhysicalJoin.IsNullEQ.iter().any(|value| *value),
        )
    }
    /// 深拷贝并切换计划上下文。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut base = self.BasePhysicalJoin.CloneWithSelf(new_ctx)?;
        // The optimizer clones physical candidates before attachment. Keep the
        // NULL-safe equality bitmap aligned with the cloned hash join keys.
        base.IsNullEQ = self.BasePhysicalJoin.IsNullEQ.clone();
        Ok(Self {
            BasePhysicalJoin: base,
            Concurrency: self.Concurrency,
            EqualConditions: self
                .EqualConditions
                .iter()
                .map(ScalarFunction::clone_scalar)
                .collect(),
            NAEqualConditions: self
                .NAEqualConditions
                .iter()
                .map(ScalarFunction::clone_scalar)
                .collect(),
            UseOuterToBuild: self.UseOuterToBuild,
            StoreTp: self.StoreTp,
            MppShuffleJoin: self.MppShuffleJoin,
            FromHashJoinHint: self.FromHashJoinHint,
            HasTableAlias: self.HasTableAlias,
            RuntimeFilterList: self
                .RuntimeFilterList
                .iter()
                .map(|rf| *rf.Clone())
                .collect(),
            RuntimeFilterTypes: self.RuntimeFilterTypes.clone(),
        })
    }
    /// EXPLAIN 文本（含表达式细节）。
    pub fn ExplainInfo(&self) -> String {
        self.explain(false)
    }
    /// 归一化 EXPLAIN（便于计划缓存比对）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.explain(true)
    }
    /// 拼连接类型与左右/其它条件；无等值条件时标记 CARTESIAN。
    fn explain(&self, normalized: bool) -> String {
        let mut out = if self.EqualConditions.is_empty() && self.NAEqualConditions.is_empty() {
            "CARTESIAN ".to_owned()
        } else {
            String::new()
        };
        if self.EqualConditions.is_empty() && !self.NAEqualConditions.is_empty() {
            out.push_str("Null-aware ");
        }
        out.push_str(&self.BasePhysicalJoin.JoinType.to_string());
        if self.BasePhysicalJoin.JoinType != JoinType::InnerJoin
            && let Some(left) = self.children().first()
        {
            let left_type = left
                .as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .map_or_else(|| left.tp(&[]), crate::PhysicalTableScan::TP);
            out.push_str(", left side:");
            out.push_str(&left_type);
        }
        let eval_ctx = self.s_ctx().GetExprCtx().GetEvalCtx();
        let render_conditions = |conditions: &[ExprBox]| {
            if normalized {
                String::from_utf8_lossy(&expression::SortedExplainNormalizedExpressionList(
                    conditions,
                ))
                .into_owned()
            } else {
                String::from_utf8_lossy(&expression::SortedExplainExpressionList(
                    eval_ctx, conditions,
                ))
                .into_owned()
            }
        };
        let render_equal_conditions = |conditions: &[ScalarFunction], null_aware: bool| {
            conditions
                .iter()
                .enumerate()
                .map(|(index, condition)| {
                    let mut condition = condition.clone_scalar();
                    let (left_keys, right_keys) = if null_aware {
                        (
                            &self.BasePhysicalJoin.LeftNAJoinKeys,
                            &self.BasePhysicalJoin.RightNAJoinKeys,
                        )
                    } else {
                        (
                            &self.BasePhysicalJoin.LeftJoinKeys,
                            &self.BasePhysicalJoin.RightJoinKeys,
                        )
                    };
                    if self.BasePhysicalJoin.JoinType == JoinType::LeftOuterJoin
                        && let (Some(left), Some(right)) =
                            (left_keys.get(index), right_keys.get(index))
                        && condition.GetArgs().len() == 2
                    {
                        condition.GetArgsMut()[0] = Box::new(left.Clone());
                        condition.GetArgsMut()[1] = Box::new(right.Clone());
                    }
                    if normalized {
                        condition.ExplainNormalizedInfo()
                    } else {
                        condition.ExplainInfo(eval_ctx)
                    }
                })
                .collect::<Vec<_>>()
        };
        for (conditions, null_aware) in [
            (&self.EqualConditions[..], false),
            (&self.NAEqualConditions[..], true),
        ] {
            let equal_conditions = render_equal_conditions(conditions, null_aware);
            if !equal_conditions.is_empty() {
                out.push_str(", equal:");
                if !normalized {
                    out.push('[');
                }
                out.push_str(&equal_conditions.join(" "));
                if !normalized {
                    out.push(']');
                }
            }
        }
        for (label, conditions) in [
            ("left cond", &self.BasePhysicalJoin.LeftConditions),
            ("right cond", &self.BasePhysicalJoin.RightConditions),
            ("other cond", &self.BasePhysicalJoin.OtherConditions),
        ] {
            if !conditions.is_empty() {
                if normalized || label != "left cond" {
                    out.push_str(&format!(", {label}:{}", render_conditions(conditions)));
                } else {
                    out.push_str(&format!(", {label}:[{}]", render_conditions(conditions)));
                }
            }
        }
        let stream_count = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            out.push_str(&format!(", stream_count: {stream_count}"));
        }
        if !self.RuntimeFilterList.is_empty() {
            out.push_str(", runtime filter:");
            out.push_str(
                &self
                    .RuntimeFilterList
                    .iter()
                    .map(|filter| filter.ExplainInfo(true, eval_ctx))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        out
    }
    /// 抽取相关子查询列（CorrelatedColumn）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let mut result = self.BasePhysicalJoin.ExtractCorrelatedCols();
        for function in self.EqualConditions.iter().chain(&self.NAEqualConditions) {
            result.extend(
                expression::ExtractCorColumns(function)
                    .into_iter()
                    .map(CorrelatedColumn::Clone),
            );
        }
        result
    }
    /// 旧版代价：两侧行数之和 / 并发度。
    pub fn GetCost(&self, left: f64, right: f64, _root: bool, _flag: u64) -> f64 {
        (left.max(0.0) + right.max(0.0)) / self.Concurrency.max(1) as f64
    }
    /// 计划代价 V1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
    /// 计划代价 V2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
    /// 估算内存：基座 + 等值/NA 条件。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalJoin.MemoryUsage()
            + self
                .EqualConditions
                .iter()
                .chain(&self.NAEqualConditions)
                .map(ScalarFunction::MemoryUsage)
                .sum::<i64>()
    }
    /// 判断右孩子是否为 build 侧（建哈希表一侧）。
    pub fn RightIsBuildSide(&self) -> bool {
        if self.UseOuterToBuild {
            self.BasePhysicalJoin.InnerChildIdx == 0
        } else {
            self.BasePhysicalJoin.InnerChildIdx != 0
        }
    }
    /// 按左右 Schema 解析连接键与条件列下标。
    pub fn ResolveIndicesItself(&mut self) -> Result<(), expression::Error> {
        let children = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children();
        if children.len() < 2 {
            return Ok(());
        }
        // 左键/左条件对左 Schema；右键/右条件对右 Schema；其它条件对合并 Schema。
        let left = children[0].schema().Clone();
        let right = children[1].schema().Clone();
        for (index, function) in self.EqualConditions.iter_mut().enumerate() {
            if function.GetArgs().len() != 2 {
                return Err(expression::errors::New(
                    "hash join equal condition must have two arguments",
                ));
            }
            let direct = function.GetArgs()[0]
                .ResolveIndices(&left)
                .and_then(|left_argument| {
                    function.GetArgs()[1]
                        .ResolveIndices(&right)
                        .map(|right_argument| (left_argument, right_argument, false))
                });
            let (left_argument, right_argument, reversed) = direct.or_else(|_| {
                function.GetArgs()[1]
                    .ResolveIndices(&left)
                    .and_then(|left_argument| {
                        function.GetArgs()[0]
                            .ResolveIndices(&right)
                            .map(|right_argument| (left_argument, right_argument, true))
                    })
            })?;
            let left_column = left_argument
                .as_any()
                .downcast_ref::<expression::Column>()
                .ok_or_else(|| expression::errors::New("left hash join key is not a column"))?
                .Clone();
            let right_column = right_argument
                .as_any()
                .downcast_ref::<expression::Column>()
                .ok_or_else(|| expression::errors::New("right hash join key is not a column"))?
                .Clone();
            if index >= self.BasePhysicalJoin.LeftJoinKeys.len()
                || index >= self.BasePhysicalJoin.RightJoinKeys.len()
            {
                return Err(expression::errors::New(
                    "hash join key count does not match equal conditions",
                ));
            }
            self.BasePhysicalJoin.LeftJoinKeys[index] = left_column;
            self.BasePhysicalJoin.RightJoinKeys[index] = right_column;
            if reversed {
                function.GetArgsMut()[0] = right_argument;
                function.GetArgsMut()[1] = left_argument;
            } else {
                function.GetArgsMut()[0] = left_argument;
                function.GetArgsMut()[1] = right_argument;
            }
            function.CleanHashCode();
        }
        for (index, function) in self.NAEqualConditions.iter_mut().enumerate() {
            if function.GetArgs().len() != 2 {
                return Err(expression::errors::New(
                    "null-aware hash join equal condition must have two arguments",
                ));
            }
            let left_argument = function.GetArgs()[0].ResolveIndices(&left)?;
            let right_argument = function.GetArgs()[1].ResolveIndices(&right)?;
            let left_column = left_argument
                .as_any()
                .downcast_ref::<expression::Column>()
                .ok_or_else(|| expression::errors::New("left NA hash join key is not a column"))?
                .Clone();
            let right_column = right_argument
                .as_any()
                .downcast_ref::<expression::Column>()
                .ok_or_else(|| expression::errors::New("right NA hash join key is not a column"))?
                .Clone();
            if index >= self.BasePhysicalJoin.LeftNAJoinKeys.len()
                || index >= self.BasePhysicalJoin.RightNAJoinKeys.len()
            {
                return Err(expression::errors::New(
                    "NA hash join key count does not match equal conditions",
                ));
            }
            self.BasePhysicalJoin.LeftNAJoinKeys[index] = left_column;
            self.BasePhysicalJoin.RightNAJoinKeys[index] = right_column;
            function.GetArgsMut()[0] = left_argument;
            function.GetArgsMut()[1] = right_argument;
            function.CleanHashCode();
        }
        let resolve_join_key = |column: &expression::Column, schema: &expression::Schema| {
            if let Ok(resolved) = column.ResolveIndices(schema) {
                return Ok(resolved);
            }
            let mut matching = schema
                .Columns
                .iter()
                .filter(|candidate| candidate.String() == column.String());
            let first = matching.next();
            if let Some(candidate) = first
                && matching.next().is_none()
            {
                return candidate.ResolveIndices(schema);
            }
            column.ResolveIndices(schema)
        };
        for column in &mut self.BasePhysicalJoin.LeftJoinKeys {
            let unresolved = format!("{}#{}", column.String(), column.UniqueID);
            *column = resolve_join_key(column, &left).map_err(|error| {
                expression::errors::New(format!(
                    "resolve left hash join key {unresolved} against schema [{}]: {error}",
                    left.Columns
                        .iter()
                        .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        }
        for column in &mut self.BasePhysicalJoin.RightJoinKeys {
            let unresolved = format!("{}#{}", column.String(), column.UniqueID);
            *column = resolve_join_key(column, &right).map_err(|error| {
                expression::errors::New(format!(
                    "resolve right hash join key {unresolved} against schema [{}]: {error}",
                    right
                        .Columns
                        .iter()
                        .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        }
        for expr in &mut self.BasePhysicalJoin.LeftConditions {
            *expr = expr.ResolveIndices(&left).map_err(|error| {
                expression::errors::New(format!("resolve left hash join condition: {error}"))
            })?;
        }
        for expr in &mut self.BasePhysicalJoin.RightConditions {
            *expr = expr.ResolveIndices(&right).map_err(|error| {
                expression::errors::New(format!("resolve right hash join condition: {error}"))
            })?;
        }
        let merged = expression::MergeSchema(Some(&left), Some(&right)).expect("two schemas");
        let context = self.s_ctx().clone();
        for expr in &mut self.BasePhysicalJoin.OtherConditions {
            let unresolved = expr.CloneExpr();
            *expr = expr
                .ResolveIndices(&merged)
                .or_else(|original_error| {
                    // Join reorder and aggregate-key projection may preserve a
                    // semantic column while assigning it a fresh UniqueID. Go's
                    // expression rewriter substitutes that column before physical
                    // ResolveIndices. Recover the same unambiguous name mapping at
                    // this boundary instead of leaving a stale logical ID behind.
                    let columns = expression::ExtractColumns(unresolved.as_ref())
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>();
                    let replacements = columns
                        .iter()
                        .map(|column| {
                            let mut matches = merged
                                .Columns
                                .iter()
                                .filter(|candidate| candidate.String() == column.String());
                            let first = matches.next();
                            if first.is_some() && matches.next().is_none() {
                                first.expect("checked unique join column").Clone()
                            } else {
                                column.Clone()
                            }
                        })
                        .collect::<Vec<_>>();
                    let remapped = expression::ColumnSubstitute(
                        context.GetExprCtx(),
                        unresolved.CloneExpr(),
                        &expression::NewSchema(columns),
                        &expression::Column2Exprs(&replacements),
                    );
                    remapped.ResolveIndices(&merged).map_err(|_| original_error)
                })
                .map_err(|error| {
                    expression::errors::New(format!("resolve other hash join condition: {error}"))
                })?;
        }
        let mut output = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .Schema()
            .Clone();
        let mut columns_to_resolve = output.Columns.len();
        if matches!(
            self.BasePhysicalJoin.JoinType,
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
        ) {
            columns_to_resolve = columns_to_resolve.saturating_sub(1);
        }
        columns_to_resolve = Self::DeduplicateOutputColumns(&mut output, columns_to_resolve);
        let resolved = Self::ResolveOutputColumns(&mut output, &merged, columns_to_resolve);
        if resolved < columns_to_resolve {
            return Err(expression::errors::New(format!(
                "some hash join output columns cannot find references from children: resolved {resolved} of {columns_to_resolve}"
            )));
        }
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(output);
        Ok(())
    }
    /// 先解析 Schema 生产器，再解析本节点连接表达式。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .ResolveIndices()?;
        self.ResolveIndicesItself()
    }
    /// 编码为 tipb::Join Executor，含键、条件与孩子。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let client = ctx
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let build = ctx.GetExprCtx();
        let eval = build.GetEvalCtx();
        if !self.BasePhysicalJoin.LeftJoinKeys.is_empty()
            && !self.BasePhysicalJoin.LeftNAJoinKeys.is_empty()
        {
            return Err(expression::errors::New(
                "join key and na join key can not both exist",
            ));
        }
        let null_aware = !self.BasePhysicalJoin.LeftNAJoinKeys.is_empty();
        let (left_join_keys, right_join_keys, equal_conditions) = if null_aware {
            (
                &self.BasePhysicalJoin.LeftNAJoinKeys,
                &self.BasePhysicalJoin.RightNAJoinKeys,
                &self.NAEqualConditions,
            )
        } else {
            (
                &self.BasePhysicalJoin.LeftJoinKeys,
                &self.BasePhysicalJoin.RightJoinKeys,
                &self.EqualConditions,
            )
        };
        let cols = |values: &[expression::Column]| {
            values
                .iter()
                .map(|column| Box::new(column.Clone()) as ExprBox)
                .collect::<Vec<_>>()
        };
        let left_keys =
            expression::ExpressionsToPBList(eval, &cols(left_join_keys), client.as_ref())?;
        let right_keys =
            expression::ExpressionsToPBList(eval, &cols(right_join_keys), client.as_ref())?;
        let left_conditions = expression::ExpressionsToPBList(
            eval,
            &self.BasePhysicalJoin.LeftConditions,
            client.as_ref(),
        )?;
        let right_conditions = expression::ExpressionsToPBList(
            eval,
            &self.BasePhysicalJoin.RightConditions,
            client.as_ref(),
        )?;
        let split_other_from_in = matches!(
            self.BasePhysicalJoin.JoinType,
            JoinType::AntiSemiJoin | JoinType::AntiLeftOuterSemiJoin | JoinType::LeftOuterSemiJoin
        );
        let (other_conditions, other_eq_conditions): (Vec<&ExprBox>, Vec<&ExprBox>) = self
            .BasePhysicalJoin
            .OtherConditions
            .iter()
            .partition(|condition| {
                !split_other_from_in || !expression::IsEQCondFromIn(condition.as_ref())
            });
        let clone_conditions = |conditions: Vec<&ExprBox>| {
            conditions
                .into_iter()
                .map(|condition| condition.CloneExpr())
                .collect::<Vec<_>>()
        };
        let other_conditions = expression::ExpressionsToPBList(
            eval,
            &clone_conditions(other_conditions),
            client.as_ref(),
        )?;
        let other_eq_conditions = expression::ExpressionsToPBList(
            eval,
            &clone_conditions(other_eq_conditions),
            client.as_ref(),
        )?;
        let mut probe_types = Vec::with_capacity(equal_conditions.len());
        let mut build_types = Vec::with_capacity(equal_conditions.len());
        for condition in equal_conditions {
            let mut field_type = condition.GetStaticType().clone();
            let (charset, collation) = condition.CharsetAndCollation();
            field_type.SetCharset(charset);
            field_type.SetCollate(collation);
            let encoded = expression::ToPBFieldTypeWithCheck(&field_type, store)?;
            probe_types.push(encoded.clone());
            build_types.push(encoded);
        }
        let runtime_filters =
            crate::RuntimeFilterListToPB(ctx, &self.RuntimeFilterList, client.as_ref())?;
        let mut join = tipb::Join::new();
        join.set_join_type(match self.BasePhysicalJoin.JoinType {
            JoinType::LeftOuterJoin => tipb::JoinType::TypeLeftOuterJoin,
            JoinType::RightOuterJoin => tipb::JoinType::TypeRightOuterJoin,
            JoinType::FullOuterJoin => tipb::JoinType::TypeFullOuterJoin,
            JoinType::SemiJoin => tipb::JoinType::TypeSemiJoin,
            JoinType::AntiSemiJoin => tipb::JoinType::TypeAntiSemiJoin,
            JoinType::LeftOuterSemiJoin => tipb::JoinType::TypeLeftOuterSemiJoin,
            JoinType::AntiLeftOuterSemiJoin => tipb::JoinType::TypeAntiLeftOuterSemiJoin,
            _ => tipb::JoinType::TypeInnerJoin,
        });
        join.set_inner_idx(self.BasePhysicalJoin.InnerChildIdx as i64);
        join.set_left_join_keys(left_keys.into());
        join.set_right_join_keys(right_keys.into());
        join.set_left_conditions(left_conditions.into());
        join.set_right_conditions(right_conditions.into());
        join.set_other_conditions(other_conditions.into());
        join.set_other_eq_conditions_from_in(other_eq_conditions.into());
        join.set_probe_types(probe_types.into());
        join.set_build_types(build_types.into());
        join.set_is_null_aware_semi_join(null_aware);
        join.set_is_null_eq(self.BasePhysicalJoin.IsNullEQ.clone().into());
        join.set_runtime_filter_list(runtime_filters.into());
        let children = self
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children();
        let mut encoded = Vec::with_capacity(children.len());
        for child in children {
            encoded.push(*child.to_pb(ctx, store)?);
        }
        join.set_children(encoded.into());
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeJoin);
        executor.set_join(join);
        executor.set_executor_id(
            self.BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .explain_id(&[])
                .to_string(),
        );
        executor.set_fine_grained_shuffle_stream_count(
            self.BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .TiFlashFineGrainedShuffleStreamCount,
        );
        executor.set_fine_grained_shuffle_batch_size(ctx.TiFlashFineGrainedShuffleBatchSize);
        Ok(Box::new(executor))
    }
}

/// 作为运行时过滤的 build 节点：提供 ID、build 侧与过滤类型。
impl RuntimeFilterBuildNode for PhysicalHashJoin {
    fn id(&self) -> i32 {
        self.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .id()
    }
    fn right_is_build_side(&self) -> bool {
        self.RightIsBuildSide()
    }
    fn runtime_filter_types(&self) -> &[RuntimeFilterType] {
        &self.RuntimeFilterTypes
    }
    fn register_runtime_filter(&mut self, id: i32) {
        if !self.RuntimeFilterList.iter().any(|rf| rf.ID() == id) { /* assigned by owner */ }
    }
}
