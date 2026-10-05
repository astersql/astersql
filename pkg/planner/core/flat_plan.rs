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

// 物理计划扁平化（flat plan）与 EXPLAIN 行式输出。
//
// 将树形物理执行计划展开为深度优先的线性算子列表，记录父子下标、
// Join 的 Build/Probe 标签、CTE 的 Seed/Recursive 分区以及缩进层级，
// 供 `EXPLAIN` 以树状文本展示，并支持 analyze / verbose 附加列。

use crate::{PlanKind, PlanNode, StoreType};
use base_dependency as base;
use kv_dependency as kv;
use physicalop_dependency as physicalop;
use std::any::Any;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

/// Borrowed Go-compatible plan forest. Every tree has local, zero-based indexes.
pub struct TypedFlatPhysicalPlan<'a> {
    pub Main: Vec<TypedFlatOperator<'a>>,
    pub CTEs: Vec<Vec<TypedFlatOperator<'a>>>,
    pub ScalarSubQueries: Vec<Vec<TypedFlatOperator<'a>>>,
}

/// The caller supplies a borrowed scalar registry snapshot so no self-referential
/// references to temporary `Rc` values escape this function.
pub fn FlattenTypedPhysicalPlanForest<'a>(
    plan: &'a dyn base::Plan,
    scalar_subqueries: &'a [Rc<dyn Any>],
) -> Option<TypedFlatPhysicalPlan<'a>> {
    fn attach<'a>(
        tree: &mut Vec<TypedFlatOperator<'a>>,
        child: &'a dyn base::Plan,
        label: TypedOperatorLabel,
        last: bool,
    ) -> Option<()> {
        let offset = tree.len();
        let mut branch = FlattenTypedPhysicalPlan(child)?;
        branch[0].Label = label;
        branch[0].IsRoot = true;
        branch[0].IsLastChild = last;
        for op in &mut branch {
            op.ChildrenEndIdx += offset;
            for index in &mut op.ChildrenIdx {
                *index += offset;
            }
        }
        tree[0].ChildrenIdx.push(offset);
        tree.extend(branch);
        tree[0].ChildrenEndIdx = tree.len() - 1;
        Some(())
    }

    let main = FlattenTypedPhysicalPlan(plan)?;
    let mut pending = Vec::new();
    for op in &main {
        if op.IsRoot {
            if let Some(cte) = op.Origin.as_any().downcast_ref::<physicalop::PhysicalCTE>() {
                pending.push(cte.CTE.as_ref());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut ctes = Vec::new();
    let mut cursor = 0;
    while cursor < pending.len() {
        let definition = pending[cursor];
        cursor += 1;
        if !seen.insert(definition.IDForStorage) {
            continue;
        }
        let mut tree = FlattenTypedPhysicalPlan(definition)?;
        attach(
            &mut tree,
            definition.SeedPlan.as_ref(),
            TypedOperatorLabel::SeedPart,
            definition.RecurPlan.is_none(),
        )?;
        if let Some(recursive) = definition.RecurPlan.as_ref() {
            attach(
                &mut tree,
                recursive.as_ref(),
                TypedOperatorLabel::RecursivePart,
                true,
            )?;
        }
        for op in &tree {
            if op.IsRoot {
                if let Some(cte) = op.Origin.as_any().downcast_ref::<physicalop::PhysicalCTE>() {
                    pending.push(cte.CTE.as_ref());
                }
            }
        }
        ctes.push(tree);
    }
    let mut scalar = Vec::new();
    for registered in scalar_subqueries {
        let Some(ctx) = registered.downcast_ref::<crate::ScalarSubqueryEvalCtx>() else {
            continue;
        };
        let mut tree = vec![TypedFlatOperator {
            Origin: ctx,
            ChildrenIdx: Vec::new(),
            ChildrenEndIdx: 0,
            IsRoot: true,
            StoreType: kv::StoreType::TiDB,
            ReqType: physicalop::ReadReqType::Cop,
            Label: TypedOperatorLabel::Empty,
            IsINLProbeChild: false,
            NeedReverseDriverSide: false,
            IsLastChild: true,
        }];
        attach(
            &mut tree,
            ctx.scalar_sub_query.as_ref(),
            TypedOperatorLabel::Empty,
            true,
        )?;
        scalar.push(tree);
    }
    Some(TypedFlatPhysicalPlan {
        Main: main,
        CTEs: ctes,
        ScalarSubQueries: scalar,
    })
}

/// Borrowed physical operators retain their concrete Rust types and fields.
/// The preorder indexes follow the order returned by each physical operator.
pub struct TypedFlatOperator<'a> {
    pub Origin: &'a dyn base::Plan,
    pub ChildrenIdx: Vec<usize>,
    pub ChildrenEndIdx: usize,
    pub IsRoot: bool,
    pub StoreType: kv::StoreType,
    pub ReqType: physicalop::ReadReqType,
    pub Label: TypedOperatorLabel,
    pub IsINLProbeChild: bool,
    pub NeedReverseDriverSide: bool,
    pub IsLastChild: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedOperatorLabel {
    Empty,
    BuildSide,
    ProbeSide,
    SeedPart,
    RecursivePart,
}

/// Flatten a real physical plan without converting it to an EXPLAIN summary.
pub fn FlattenTypedPhysicalPlan(plan: &dyn base::Plan) -> Option<Vec<TypedFlatOperator<'_>>> {
    if let Some(execute) = plan.as_any().downcast_ref::<crate::RuntimeExecute>() {
        return FlattenTypedPhysicalPlan(execute.Plan.as_ref());
    }
    if let Some(explain) = plan.as_any().downcast_ref::<crate::RuntimeExplain>() {
        return FlattenTypedPhysicalPlan(explain.TargetPlan.as_ref());
    }
    fn append<'a>(
        plan: &'a dyn base::Plan,
        tree: &mut Vec<TypedFlatOperator<'a>>,
        is_root: bool,
        store_type: kv::StoreType,
        req_type: physicalop::ReadReqType,
        label: TypedOperatorLabel,
        inl_probe_child: bool,
        is_last_child: bool,
    ) -> Option<usize> {
        let physical = plan.as_physical_plan();
        let origin = plan.as_any();
        if physical.is_none()
            && !origin.is::<physicalop::Insert>()
            && !origin.is::<physicalop::Update>()
            && !origin.is::<physicalop::Delete>()
            && !origin.is::<physicalop::FKCheck>()
            && !origin.is::<physicalop::FKCascade>()
            && !origin.is::<crate::RuntimeAnalyze>()
            && !origin.is::<crate::RuntimeSimple>()
        {
            return None;
        }
        let index = tree.len();
        tree.push(TypedFlatOperator {
            Origin: plan,
            ChildrenIdx: Vec::new(),
            ChildrenEndIdx: index,
            IsRoot: is_root,
            StoreType: store_type,
            ReqType: req_type,
            Label: label,
            IsINLProbeChild: inl_probe_child,
            NeedReverseDriverSide: false,
            IsLastChild: is_last_child,
        });
        let reader_context =
            if let Some(reader) = origin.downcast_ref::<physicalop::PhysicalTableReader>() {
                Some((reader.StoreType, reader.ReadReqType))
            } else if origin.is::<physicalop::PhysicalIndexReader>()
                || origin.is::<physicalop::PhysicalIndexLookUpReader>()
                || origin.is::<physicalop::PhysicalIndexMergeReader>()
            {
                Some((kv::StoreType::TiKV, physicalop::ReadReqType::Cop))
            } else {
                None
            };
        let children = physical.map_or_else(Vec::new, |physical| physical.children());
        let mut child_labels = vec![TypedOperatorLabel::Empty; children.len()];
        if children.len() == 2 {
            let inner = if let Some(join) = origin.downcast_ref::<physicalop::PhysicalApply>() {
                Some((join.PhysicalHashJoin.BasePhysicalJoin.InnerChildIdx, true))
            } else if let Some(join) = origin.downcast_ref::<physicalop::PhysicalHashJoin>() {
                Some((join.BasePhysicalJoin.InnerChildIdx, join.UseOuterToBuild))
            } else if let Some(join) = physicalop::index_join_base(plan) {
                Some((join.BasePhysicalJoin.InnerChildIdx, true))
            } else {
                None
            };
            if let Some((inner, outer_build)) = inner.filter(|(inner, _)| *inner < 2) {
                child_labels[inner] = if outer_build {
                    TypedOperatorLabel::ProbeSide
                } else {
                    TypedOperatorLabel::BuildSide
                };
                child_labels[1 - inner] = if outer_build {
                    TypedOperatorLabel::BuildSide
                } else {
                    TypedOperatorLabel::ProbeSide
                };
            } else if let Some(join) = origin.downcast_ref::<physicalop::PhysicalMergeJoin>() {
                child_labels = if join.BasePhysicalJoin.JoinType == base::JoinType::RightOuterJoin {
                    vec![TypedOperatorLabel::BuildSide, TypedOperatorLabel::ProbeSide]
                } else {
                    vec![TypedOperatorLabel::ProbeSide, TypedOperatorLabel::BuildSide]
                };
            } else if origin.is::<physicalop::PhysicalIndexLookUpReader>() {
                child_labels = vec![TypedOperatorLabel::BuildSide, TypedOperatorLabel::ProbeSide];
            }
            tree[index].NeedReverseDriverSide = child_labels[0] == TypedOperatorLabel::ProbeSide
                && child_labels[1] == TypedOperatorLabel::BuildSide;
        }
        if let Some(reader) = origin.downcast_ref::<physicalop::PhysicalIndexMergeReader>() {
            child_labels = vec![TypedOperatorLabel::BuildSide; children.len()];
            if reader.TablePlan.is_some() && !child_labels.is_empty() {
                *child_labels.last_mut().unwrap() = TypedOperatorLabel::ProbeSide;
            }
        }
        let physical_child_count = children.len();
        for (child_position, child) in children.into_iter().enumerate() {
            let (child_root, child_store, child_req) = reader_context
                .map_or((is_root, store_type, req_type), |(store, req)| {
                    (false, store, req)
                });
            let child_inl_probe = inl_probe_child
                || (origin.is::<physicalop::PhysicalIndexLookUpReader>() && child_position == 1)
                || (origin.is::<physicalop::PhysicalIndexMergeReader>()
                    && child_labels[child_position] == TypedOperatorLabel::ProbeSide);
            let child_index = append(
                child,
                tree,
                child_root,
                child_store,
                child_req,
                child_labels[child_position],
                child_inl_probe,
                child_position + 1 == physical_child_count,
            )?;
            tree[index].ChildrenIdx.push(child_index);
        }
        // These are Go Plan children rather than PhysicalPlan.Children().
        // Keep the SELECT tree before foreign-key checks and cascades.
        let mut special_children: Vec<&dyn base::Plan> = Vec::new();
        if let Some(insert) = origin.downcast_ref::<physicalop::Insert>() {
            special_children.extend(
                insert
                    .SelectPlan
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
            special_children.extend(
                insert
                    .FKChecks
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
            special_children.extend(
                insert
                    .FKCascades
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
        } else if let Some(update) = origin.downcast_ref::<physicalop::Update>() {
            special_children.push(update.SelectPlan.as_ref());
            special_children.extend(
                update
                    .FKChecks
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
            special_children.extend(
                update
                    .FKCascades
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
        } else if let Some(delete) = origin.downcast_ref::<physicalop::Delete>() {
            special_children.push(delete.SelectPlan.as_ref());
            special_children.extend(
                delete
                    .FKChecks
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
            special_children.extend(
                delete
                    .FKCascades
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
        } else if let Some(cascade) = origin.downcast_ref::<physicalop::FKCascade>() {
            special_children.extend(cascade.CascadePlans.iter().map(|p| p.as_ref()));
        } else if let Some(receiver) =
            origin.downcast_ref::<physicalop::PhysicalShuffleReceiverStub>()
        {
            special_children.extend(
                receiver
                    .DataSource
                    .iter()
                    .map(|p| p.as_ref() as &dyn base::Plan),
            );
        }
        let child_count = special_children.len();
        for (position, child) in special_children.into_iter().enumerate() {
            let child_index = append(
                child,
                tree,
                true,
                store_type,
                req_type,
                TypedOperatorLabel::Empty,
                false,
                position + 1 == child_count,
            )?;
            tree[index].ChildrenIdx.push(child_index);
        }
        tree[index].ChildrenEndIdx = tree.len() - 1;
        Some(index)
    }

    let mut tree = Vec::new();
    append(
        plan,
        &mut tree,
        true,
        kv::StoreType::TiDB,
        physicalop::ReadReqType::Cop,
        TypedOperatorLabel::Empty,
        false,
        true,
    )?;
    Some(tree)
}

/// 扁平化后的算子序列（深度优先序）。
pub type FlatPlanTree = Vec<FlatOperator>;

/// 扁平化物理计划：主树、CTE、标量子查询及展示相关标志。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatPhysicalPlan {
    /// 主查询对应的扁平算子树。
    pub Main: FlatPlanTree,
    /// CTE（公用表表达式）相关的扁平算子树。
    pub CTE: FlatPlanTree,
    /// 标量子查询相关的扁平算子树。
    pub ScalarSubQ: FlatPlanTree,
    /// 是否处于 Execute 上下文（影响展示语义）。
    pub InExecute: bool,
    /// 是否尝试快速计划路径。
    pub TryFastPlan: bool,
    /// Join 展开时是否优先遍历 Build 侧。
    pub BuildSideFirst: bool,
}

/// Occurrence-aligned RU values for one operator in an EXPLAIN tree.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExplainRUOperatorResult {
    pub self_ru: f64,
    pub cum_ru: f64,
}

/// RU values produced by statement accounting for each flattened plan tree.
/// Keeping values by occurrence avoids collapsing repeated plan IDs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExplainRUResult {
    pub Main: Vec<ExplainRUOperatorResult>,
    pub CTE: Vec<ExplainRUOperatorResult>,
    pub ScalarSubQ: Vec<ExplainRUOperatorResult>,
    pub TotalRU: f64,
}

impl FlatPhysicalPlan {
    /// 取出 SELECT 侧计划：DML（Update/Delete/Insert）时跳过根算子，返回子树与偏移。
    pub fn GetSelectPlan(&self) -> (&[FlatOperator], usize) {
        if self.Main.is_empty() {
            return (&[], 0);
        }
        let mut has_dml = false;
        for (index, operator) in self.Main.iter().enumerate() {
            if matches!(
                operator.Origin.kind,
                PlanKind::Update | PlanKind::Delete | PlanKind::Insert
            ) {
                has_dml = true;
                continue;
            }
            if has_dml {
                let end = self.Main[index..]
                    .iter()
                    .position(|operator| {
                        matches!(
                            operator.Origin.kind,
                            PlanKind::FKCheck | PlanKind::FKCascade
                        )
                    })
                    .map_or(self.Main.len(), |suffix| index + suffix);
                return (&self.Main[index..end], index);
            }
            return (&self.Main[index..], index);
        }
        (&[], 0)
    }
}

/// 扁平算子上的角色标签：Join 的 Build/Probe，或 CTE 的种子/递归部分。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OperatorLabel {
    #[default]
    Empty,
    /// Hash/Index Join 的构建侧（通常较小的内表侧）。
    BuildSide,
    /// Join 的探测侧（外表侧）。
    ProbeSide,
    /// CTE 的种子（非递归）部分。
    SeedPart,
    /// CTE 的递归部分。
    RecursivePart,
}

impl fmt::Display for OperatorLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "",
            Self::BuildSide => "(Build)",
            Self::ProbeSide => "(Probe)",
            Self::SeedPart => "(Seed Part)",
            Self::RecursivePart => "(Recursive Part)",
        })
    }
}

/// 扁平化后的单个算子：保留原始 PlanNode，并附加树形展示所需元数据。
#[derive(Clone, Debug, PartialEq)]
pub struct FlatOperator {
    /// 原始物理计划节点。
    pub Origin: PlanNode,
    /// 子算子在扁平数组中的下标列表。
    pub ChildrenIdx: Vec<usize>,
    /// HashJoin 在 BuildSideFirst 时是否需交换驱动侧展示顺序。
    pub NeedReverseDriverSide: bool,
    /// Build/Probe/CTE 等角色标签。
    pub Label: OperatorLabel,
    /// 是否为当前子树的根。
    pub IsRoot: bool,
    /// 存储引擎类型（Root / TiKV / TiFlash 等）。
    pub StoreType: StoreType,
    /// 树深度，用于 EXPLAIN 缩进。
    pub Level: usize,
    /// 是否为同层最后一个孩子（决定 └─ / ├─）。
    pub IsLastChild: bool,
}

impl FlatOperator {
    /// 生成 EXPLAIN 中的算子 ID，形如 `TableScan_1`。
    pub fn ExplainID(&self) -> String {
        format!("{}_{}", self.Origin.kind.name(), self.Origin.id)
    }
}

/// 递归扁平化时向下传递的上下文：层级、标签与兄弟位置。
#[derive(Clone, Debug)]
struct OperatorContext {
    level: usize,
    label: OperatorLabel,
    is_root: bool,
    is_last_child: bool,
}

/// 将物理计划树扁平化为 `FlatPhysicalPlan`；`plan` 为 `None` 时返回 `None`。
pub fn FlattenPhysicalPlan(
    plan: Option<&PlanNode>,
    build_side_first: bool,
) -> Option<FlatPhysicalPlan> {
    let root = plan?;
    let mut flat = FlatPhysicalPlan {
        BuildSideFirst: build_side_first,
        ..Default::default()
    };
    let context = OperatorContext {
        level: 0,
        label: OperatorLabel::Empty,
        is_root: true,
        is_last_child: true,
    };
    flatten_recursively(root, &context, &mut flat.Main, build_side_first);
    Some(flat)
}

/// 按算子类型为各子节点分配 Build/Probe 或 Seed/Recursive 标签。
fn child_labels(plan: &PlanNode) -> Vec<OperatorLabel> {
    let mut labels = vec![OperatorLabel::Empty; plan.children.len()];
    match &plan.kind {
        PlanKind::HashJoin { inner_child, .. } => {
            if labels.len() == 2 {
                labels[*inner_child] = OperatorLabel::BuildSide;
                labels[1 - *inner_child] = OperatorLabel::ProbeSide;
            }
        }
        PlanKind::MergeJoin {
            join_type: crate::JoinType::RightOuterJoin,
            ..
        } => {
            if labels.len() == 2 {
                labels[0] = OperatorLabel::BuildSide;
                labels[1] = OperatorLabel::ProbeSide;
            }
        }
        PlanKind::MergeJoin { .. }
        | PlanKind::IndexJoin { .. }
        | PlanKind::IndexMergeJoin { .. }
        | PlanKind::IndexHashJoin { .. }
        | PlanKind::Apply => {
            // 这些 Join 约定：左为 Probe、右为 Build。
            if labels.len() == 2 {
                labels[0] = OperatorLabel::ProbeSide;
                labels[1] = OperatorLabel::BuildSide;
            }
        }
        PlanKind::CTE { .. } if labels.len() >= 2 => {
            labels[0] = OperatorLabel::SeedPart;
            labels[1] = OperatorLabel::RecursivePart;
        }
        _ => {}
    }
    labels
}

/// 深度优先写入扁平数组，返回当前算子在数组中的下标。
fn flatten_recursively(
    plan: &PlanNode,
    context: &OperatorContext,
    target: &mut FlatPlanTree,
    build_side_first: bool,
) -> usize {
    let index = target.len();
    target.push(FlatOperator {
        Origin: plan.clone(),
        ChildrenIdx: Vec::new(),
        NeedReverseDriverSide: false,
        Label: context.label,
        IsRoot: context.is_root,
        StoreType: plan.store_type,
        Level: context.level,
        IsLastChild: context.is_last_child,
    });

    let labels = child_labels(plan);
    target[index].NeedReverseDriverSide = !build_side_first
        && labels.len() == 2
        && labels[0] == OperatorLabel::ProbeSide
        && labels[1] == OperatorLabel::BuildSide;
    let mut order: Vec<usize> = (0..plan.children.len()).collect();
    // BuildSideFirst：先展开 Build 侧，使 EXPLAIN 与执行驱动顺序一致。
    if build_side_first {
        order.sort_by_key(|child| labels[*child] != OperatorLabel::BuildSide);
    }
    for (position, child_index) in order.iter().enumerate() {
        let child_context = OperatorContext {
            level: context.level + 1,
            label: labels[*child_index],
            is_root: if matches!(
                plan.kind,
                PlanKind::TableReader
                    | PlanKind::IndexReader
                    | PlanKind::IndexLookUpReader
                    | PlanKind::IndexMergeReader { .. }
            ) {
                false
            } else {
                context.is_root
            },
            is_last_child: position + 1 == order.len(),
        };
        let flat_child = flatten_recursively(
            &plan.children[*child_index],
            &child_context,
            target,
            build_side_first,
        );
        target[index].ChildrenIdx.push(flat_child);
    }
    index
}

/// 将扁平计划格式化为 EXPLAIN 的行列表；`analyze`/`verbose` 追加运行时与代价列。
pub fn ExplainFlatPlanInRowFormat(
    flat: &FlatPhysicalPlan,
    format: &str,
    analyze: bool,
) -> Vec<Vec<String>> {
    flat.Main
        .iter()
        .map(|operator| {
            // 按层级拼树状前缀：非末子用 ├─，末子用 └─。
            let prefix = if operator.Level == 0 {
                String::new()
            } else {
                format!(
                    "{}{}",
                    "  ".repeat(operator.Level.saturating_sub(1)),
                    if operator.IsLastChild {
                        "└─"
                    } else {
                        "├─"
                    }
                )
            };
            let label = operator.Label.to_string();
            let id = format!("{prefix}{}{label}", operator.ExplainID());
            let mut row = vec![
                id,
                format!("{:.2}", operator.Origin.estimated_rows),
                format!("{:?}", operator.StoreType).to_lowercase(),
                operator.Origin.access_object.clone(),
                operator.Origin.operator_info.clone(),
            ];
            if analyze {
                // EXPLAIN ANALYZE：插入实际行数，并追加执行信息与内存/磁盘。
                row.insert(
                    2,
                    operator
                        .Origin
                        .actual_rows
                        .map_or_else(|| "N/A".to_owned(), |rows| rows.to_string()),
                );
                row.push(operator.Origin.execution_info.clone());
                row.push(format_bytes(operator.Origin.memory_bytes));
                row.push(format_bytes(operator.Origin.disk_bytes));
            }
            if format.eq_ignore_ascii_case("verbose") {
                // verbose：插入估计代价与代价公式。
                row.insert(2, format!("{:.2}", operator.Origin.estimated_cost));
                row.insert(3, operator.Origin.cost_formula.clone());
            }
            row
        })
        .collect()
}

/// Render EXPLAIN ANALYZE FORMAT='ru' using the occurrence-aligned values
/// produced by statement RU accounting. When accounting is unavailable or a
/// tree is not fully aligned, the three RU columns stay empty as in Go.
pub fn ExplainFlatPlanInRUFormat(
    flat: &FlatPhysicalPlan,
    result: Option<&ExplainRUResult>,
) -> Vec<Vec<String>> {
    fn visit(
        tree: &[FlatOperator],
        values: Option<&[ExplainRUOperatorResult]>,
        total_ru: f64,
        rows: &mut Vec<Vec<String>>,
    ) {
        let values = values.filter(|values| values.len() == tree.len());
        for (index, operator) in tree.iter().enumerate() {
            let prefix = if operator.Level == 0 {
                String::new()
            } else {
                format!(
                    "{}{}",
                    "  ".repeat(operator.Level.saturating_sub(1)),
                    if operator.IsLastChild {
                        "└─"
                    } else {
                        "├─"
                    }
                )
            };
            let id = format!("{prefix}{}{}", operator.ExplainID(), operator.Label);
            let task = if operator.IsRoot {
                "root".to_owned()
            } else {
                format!(
                    "cop[{}]",
                    format!("{:?}", operator.StoreType).to_lowercase()
                )
            };
            let actual_rows = operator
                .Origin
                .actual_rows
                .map_or_else(|| "N/A".to_owned(), |rows| rows.to_string());
            let (self_ru, cum_ru, cum_ru_pct) =
                values.and_then(|values| values.get(index)).map_or_else(
                    || (String::new(), String::new(), String::new()),
                    |value| {
                        let percentage = if total_ru > 0.0 {
                            value.cum_ru / total_ru * 100.0
                        } else {
                            0.0
                        };
                        (
                            format!("{:.2}", value.self_ru),
                            format!("{:.2}", value.cum_ru),
                            format!("{percentage:.2}%"),
                        )
                    },
                );
            rows.push(vec![
                id,
                task,
                actual_rows,
                self_ru,
                cum_ru,
                cum_ru_pct,
                String::new(),
            ]);
        }
    }

    let mut rows = Vec::with_capacity(flat.Main.len() + flat.CTE.len() + flat.ScalarSubQ.len());
    let total_ru = result.map_or(0.0, |result| result.TotalRU);
    visit(
        &flat.Main,
        result.map(|result| result.Main.as_slice()),
        total_ru,
        &mut rows,
    );
    visit(
        &flat.CTE,
        result.map(|result| result.CTE.as_slice()),
        total_ru,
        &mut rows,
    );
    visit(
        &flat.ScalarSubQ,
        result.map(|result| result.ScalarSubQ.as_slice()),
        total_ru,
        &mut rows,
    );
    rows
}

/// 将字节数格式化为可读字符串；负数表示不可用（N/A）。
fn format_bytes(bytes: i64) -> String {
    if bytes < 0 {
        return "N/A".to_owned();
    }
    if bytes < 1024 {
        format!("{bytes} Bytes")
    } else {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    }
}
