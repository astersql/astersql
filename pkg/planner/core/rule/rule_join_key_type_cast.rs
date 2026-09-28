// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Join 键类型转换（Join Key Type Cast）逻辑优化规则。
//
// 识别两侧 Projection 产生的 DOUBLE Join 键，将 signed INT/VARCHAR 比较改写为
// 整数比较，并在 VARCHAR 非保留侧插入与 Go 实现一致的转换安全 guard。
//
// JoinKeyTypeCastRewriter rewrites join conditions where implicit type
// conversion prevents index usage.
// By the time this rule runs (after column pruning and predicate pushdown),
// a condition like t_int.id = t_varchar.id has been transformed into:
// EqualConditions: proj_col_L = proj_col_R (both type DOUBLE)
// Left child Projection: [..., CAST(int_col AS DOUBLE) -> proj_col_L]
//	Right child Projection: [..., CAST(varchar_col AS DOUBLE) -> proj_col_R]
// This rule detects the pattern and rewrites:
// EqualConditions: new_col_L = new_col_R (both type INT)
// Left child Projection: [..., int_col -> new_col_L] (cast removed)
//	Right child Projection: [..., CAST(varchar_col AS SIGNED) -> new_col_R]
//	Guard Selection on VARCHAR side
// The guard predicate filters non-integer VARCHAR values:
//	CAST(CAST(varchar_col AS SIGNED) AS DOUBLE) = CAST(varchar_col AS DOUBLE)
// Limitation: TiDB's expression framework does not distinguish implicit CASTs
// (inserted by type coercion) from explicit user-written CASTs. If a user
// writes CAST(int_col AS DOUBLE) = CAST(varchar_col AS DOUBLE) in a join
// condition, updateEQCond materializes these into child projections that are
// indistinguishable from the implicit case, so this rule will rewrite them too.
// In practice the semantic difference is negligible: it only manifests for
// integers outside DOUBLE's exact range (>2^53), where the INT comparison is
// arguably more correct than DOUBLE (which would silently equate distinct
// values due to precision loss).
// type JoinKeyTypeCastRewriter struct{}
//
// Optimize implements base.LogicalOptRule.
// Optimize 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*JoinKeyTypeCastRewriter) Optimize(_ context.Context, p base.LogicalPlan) (base.LogicalPlan, bool, error) {
//     newP, changed := rewriteJoinTypeCasts(p)
//     return newP, changed, nil
// }
// */
use std::collections::BTreeMap;

use crate::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind};

/// Join 键类型转换规则：为不等类型的等值连接键插入 CAST。
pub struct JoinKeyTypeCast;

impl LogicalRule for JoinKeyTypeCast {
    fn name(&self) -> &'static str {
        "join_key_type_cast"
    }

    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        let changed = rewrite_join_keys(&mut plan)?;
        Ok((plan, changed))
    }
}

/// 后序遍历计划树，仅重写 Go 规则识别的投影 CAST 模式。
fn rewrite_join_keys(plan: &mut Plan) -> Result<bool, String> {
    let mut changed = false;
    for child in &mut plan.children {
        changed |= rewrite_join_keys(child)?;
    }
    let mut next_column_id = maximum_column_id(plan).saturating_add(1);
    let (kind, children) = (&mut plan.kind, &mut plan.children);
    let PlanKind::Join {
        join_type,
        equal_conditions,
        ..
    } = kind
    else {
        return Ok(changed);
    };
    if children.len() != 2
        || !matches!(children[0].kind, PlanKind::Projection { .. })
        || !matches!(children[1].kind, PlanKind::Projection { .. })
    {
        return Ok(changed);
    }

    let preserved_child = match join_type {
        JoinType::LeftOuter | JoinType::AntiSemi => Some(0),
        JoinType::RightOuter => Some(1),
        JoinType::Inner | JoinType::Semi => None,
    };
    let mut guards = [Vec::new(), Vec::new()];

    for condition in equal_conditions {
        let Expr::Scalar { function, args, .. } = condition else {
            continue;
        };
        // Null-safe equality has different NULL semantics and is explicitly
        // excluded by Go's cast-rewrite rule.
        if function != "eq" || args.len() != 2 {
            continue;
        }
        if !matches!(args[0].field_type(), Some(FieldType::Float))
            || !matches!(args[1].field_type(), Some(FieldType::Float))
        {
            continue;
        }
        let (Some(left_output), Some(right_output)) = (column_id(&args[0]), column_id(&args[1]))
        else {
            continue;
        };
        let (Some(left), Some(right)) = (
            projected_float_cast(&children[0], left_output, 0),
            projected_float_cast(&children[1], right_output, 1),
        ) else {
            continue;
        };
        let (int_info, text_info) = match (&left.original_type, &right.original_type) {
            (FieldType::SignedInt, FieldType::Text { .. }) => (&left, &right),
            (FieldType::Text { .. }, FieldType::SignedInt) => (&right, &left),
            _ => continue,
        };
        if preserved_child == Some(text_info.child_index) {
            continue;
        }

        let int_column = int_info.original.clone();
        let new_text_column = Expr::Column {
            id: next_column_id,
            field_type: FieldType::SignedInt,
        };
        next_column_id = next_column_id.saturating_add(1);

        append_projection_expression(
            children,
            int_info.child_index,
            int_column.clone(),
            column_id(&int_column).expect("classified column has an id"),
        );
        append_projection_expression(
            children,
            text_info.child_index,
            Expr::Cast {
                expr: Box::new(text_info.original.clone()),
                target: FieldType::SignedInt,
            },
            column_id(&new_text_column).expect("new column has an id"),
        );

        guards[text_info.child_index].push(Expr::Scalar {
            function: "eq".into(),
            args: vec![
                Expr::Cast {
                    expr: Box::new(Expr::Cast {
                        expr: Box::new(text_info.original.clone()),
                        target: FieldType::SignedInt,
                    }),
                    target: FieldType::Float,
                },
                Expr::Cast {
                    expr: Box::new(text_info.original.clone()),
                    target: FieldType::Float,
                },
            ],
            field_type: FieldType::Bool,
        });

        args[0] = if int_info.child_index == 0 {
            int_column.clone()
        } else {
            new_text_column.clone()
        };
        args[1] = if int_info.child_index == 1 {
            int_column
        } else {
            new_text_column
        };
        changed = true;
    }

    for (child_index, predicates) in guards.into_iter().enumerate() {
        if !predicates.is_empty() {
            insert_guard_selection(&mut children[child_index], predicates);
        }
    }
    Ok(changed)
}

#[derive(Clone)]
struct ProjectedCast {
    child_index: usize,
    original: Expr,
    original_type: FieldType,
}

fn column_id(expression: &Expr) -> Option<i64> {
    match expression {
        Expr::Column { id, .. } => Some(*id),
        _ => None,
    }
}

fn projected_float_cast(plan: &Plan, output_id: i64, child_index: usize) -> Option<ProjectedCast> {
    let PlanKind::Projection { expressions } = &plan.kind else {
        return None;
    };
    let position = plan.schema.iter().position(|id| *id == output_id)?;
    let Expr::Cast { expr, target } = expressions.get(position)? else {
        return None;
    };
    if *target != FieldType::Float || !matches!(expr.as_ref(), Expr::Column { .. }) {
        return None;
    }
    Some(ProjectedCast {
        child_index,
        original_type: expr.field_type()?.clone(),
        original: expr.as_ref().clone(),
    })
}

fn append_projection_expression(
    children: &mut [Plan],
    child_index: usize,
    expression: Expr,
    output_id: i64,
) {
    let projection = &mut children[child_index];
    let PlanKind::Projection { expressions } = &mut projection.kind else {
        return;
    };
    expressions.push(expression);
    projection.schema.push(output_id);
}

fn insert_guard_selection(projection: &mut Plan, predicates: Vec<Expr>) {
    let children = std::mem::take(&mut projection.children);
    let schema = children
        .first()
        .map(|child| child.schema.clone())
        .unwrap_or_default();
    projection.children = vec![Plan {
        kind: PlanKind::Selection,
        schema,
        children,
        predicates,
        keys: vec![],
        estimated_rows: projection.estimated_rows,
        used_stats: BTreeMap::new(),
    }];
}

fn maximum_column_id(plan: &Plan) -> i64 {
    plan.children.iter().map(maximum_column_id).fold(
        plan.schema.iter().copied().max().unwrap_or_default(),
        i64::max,
    )
}
/*
Name implements base.LogicalOptRule.
Name 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
pub fn (*JoinKeyTypeCastRewriter) Name() string {
    return "join_key_type_cast"
}

rewriteJoinTypeCasts recursively walks the plan tree.
rewriteJoinTypeCasts 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
pub fn rewriteJoinTypeCasts(p base.LogicalPlan) (base.LogicalPlan, bool) {
    anyChanged := false
    for i, child := range p.Children() {
        newChild, changed := rewriteJoinTypeCasts(child)
        if changed {
            p.SetChild(i, newChild)
            anyChanged = true
        }
    }
    join, ok := p.(*logicalop.LogicalJoin)
    if !ok {
        return p, anyChanged
    }
    changed := rewriteJoinEqConds(join)
    return p, anyChanged || changed
}

projCastInfo describes a CAST-to-DOUBLE expression in a child Projection.
projCastInfo 对应 Go 的同名 struct；字段、方法与声明顺序按来源保留。
pub struct projCastInfo {
    proj    *logicalop.LogicalProjection
    exprIdx int                // index in Projection.Exprs
    origCol *expression.Column // the underlying column inside CAST
}

rewriteJoinEqConds scans EqualConditions for DOUBLE-typed column pairs
that were produced by CAST-to-DOUBLE Projections, and rewrites INT-vs-VARCHAR
cases to use integer equality.
rewriteJoinEqConds 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
pub fn rewriteJoinEqConds(join *logicalop.LogicalJoin) bool {
    if len(join.EqualConditions) == 0 {
        return false
    }

    // Both children must be Projections (inserted by column pruning).
    leftProj, leftOk := join.Children()[0].(*logicalop.LogicalProjection)
    rightProj, rightOk := join.Children()[1].(*logicalop.LogicalProjection)
    if !leftOk || !rightOk {
        return false
    }

    ctx := join.SCtx()
    exprCtx := ctx.GetExprCtx()
    evalCtx := exprCtx.GetEvalCtx()
    anyChanged := false

    // Track which Projections need guard Selections added, keyed by child index.
    // guardEntry 对应 Go 的同名 struct；字段、方法与声明顺序按来源保留。
    pub struct guardEntry {
        proj  *logicalop.LogicalProjection
        conds []expression.Expression
    }
    guards := map[int]*guardEntry{}

    // Process each EqualCondition.
    // Determine which child index is the preserved (outer) side, if any.
    // The preserved side is the one whose rows appear in the output regardless
    // of whether a match exists. Pushing a guard filter there would incorrectly
    // remove rows that should survive with NULL-padded (or false-flagged) columns.
    // LEFT JOIN / LeftOuterSemiJoin / AntiLeftOuterSemiJoin / AntiSemiJoin:
    //   left (child 0) is preserved.
    // RIGHT JOIN: right (child 1) is preserved.
    // INNER JOIN / SemiJoin: no preserved side (unmatched rows are discarded).
    preservedChildIdx := -1 // -1 means no preserved side
    switch join.JoinType {
    case base.LeftOuterJoin,
        base.AntiSemiJoin,
        base.LeftOuterSemiJoin,
        base.AntiLeftOuterSemiJoin:
        preservedChildIdx = 0
    case base.RightOuterJoin:
        preservedChildIdx = 1
    }

    for eqIdx := 0; eqIdx < len(join.EqualConditions); eqIdx++ {
        eq := join.EqualConditions[eqIdx]

        // Skip null-safe equality (<=>): the guard uses = which filters
        // NULLs, breaking NULL<=>NULL semantics.
        if eq.FuncName.L == ast.NullEQ {
            continue
        }

        args := eq.GetArgs()
        if len(args) != 2 {
            continue
        }
        colL, okL := args[0].(*expression.Column)
        colR, okR := args[1].(*expression.Column)
        if !okL || !okR {
            continue
        }
        // Both must be DOUBLE BINARY (the telltale sign of implicit CAST).
        if colL.GetType(evalCtx).EvalType() != types.ETReal || colR.GetType(evalCtx).EvalType() != types.ETReal {
            continue
        }

        // Trace each column to its Projection expression.
        leftInfo := findCastInProj(leftProj, colL, evalCtx)
        rightInfo := findCastInProj(rightProj, colR, evalCtx)
        if leftInfo == nil || rightInfo == nil {
            continue
        }

        // Classify: one side must be signed INT, the other STRING.
        intInfo, strInfo := classifyCastPair(leftInfo, rightInfo)
        if intInfo == nil || strInfo == nil {
            continue
        }

        // Skip if the VARCHAR side is the preserved (outer) side of an
        // outer join. Pushing a guard filter there would incorrectly
        // remove rows that should be preserved with NULL-padded columns.
        strChildIdx := 1
        if strInfo.proj == leftProj {
            strChildIdx = 0
        }
        if strChildIdx == preservedChildIdx {
            continue
        }

        // INT side: add a pass-through of the bare int column, keeping
        // origCol.UniqueID. This preserves the identity so that the index
        // join builder can match it to the table's PK/index and the
        // selectivity estimator can look up the original column's NDV.
        newIntCol := &expression.Column{
            UniqueID: intInfo.origCol.UniqueID,
            RetType:  intInfo.origCol.RetType.Clone(),
        }
        intInfo.proj.Exprs = append(intInfo.proj.Exprs, intInfo.origCol.Clone())
        intInfo.proj.Schema().Append(newIntCol)

        // VARCHAR side: add CAST(varchar_col AS SIGNED). We allocate a new
        // UniqueID here because the data type changes (VARCHAR→INT), and
        // reusing the VARCHAR column's UniqueID would cause the executor to
        // read VARCHAR chunk data as INT.
        castIntExpr := expression.WrapWithCastAsInt(exprCtx, strInfo.origCol.Clone(), intInfo.origCol.RetType)
        newStrCol := &expression.Column{
            UniqueID: ctx.GetSessionVars().AllocPlanColumnID(),
            RetType:  castIntExpr.GetType(evalCtx).Clone(),
        }
        strInfo.proj.Exprs = append(strInfo.proj.Exprs, castIntExpr)
        strInfo.proj.Schema().Append(newStrCol)

        // Accumulate guard condition for the VARCHAR-side Projection.
        if guards[strChildIdx] == nil {
            guards[strChildIdx] = &guardEntry{proj: strInfo.proj}
        }
        guardLeft := expression.WrapWithCastAsReal(exprCtx, expression.WrapWithCastAsInt(exprCtx, strInfo.origCol.Clone(), intInfo.origCol.RetType))
        guardRight := expression.WrapWithCastAsReal(exprCtx, strInfo.origCol.Clone())
        guard := expression.NewFunctionInternal(
            exprCtx,
            ast.EQ,
            types.NewFieldType(mysql.TypeTiny),
            guardLeft,
            guardRight,
        )
        guards[strChildIdx].conds = append(guards[strChildIdx].conds, guard)

        // Replace the EQ condition with a new one using the NEW columns.
        // Preserve left/right ordering: args[0] must be from the left child,
        // args[1] from the right child (matching the original EQ condition).
        var newLeftCol, newRightCol *expression.Column
        if intInfo.proj == leftProj {
            newLeftCol, newRightCol = newIntCol, newStrCol
        } else {
            newLeftCol, newRightCol = newStrCol, newIntCol
        }
        newEq := expression.NewFunctionInternal(
            exprCtx,
            eq.FuncName.L,
            types.NewFieldType(mysql.TypeTiny),
            newLeftCol.Clone(),
            newRightCol.Clone(),
        )
        if sf, ok := newEq.(*expression.ScalarFunction); ok {
            join.EqualConditions[eqIdx] = sf
        }

        anyChanged = true
    }

    // Insert guard Selections below the affected Projections.
    for _, g := range guards {
        projChild := g.proj.Children()[0]
        sel := logicalop.LogicalSelection{Conditions: g.conds}.Init(ctx, join.QueryBlockOffset())
        sel.SetChildren(projChild)
        g.proj.SetChildren(sel)
    }

    // Note: we do NOT call MergeSchema() here. The new columns are only used
    // internally by EqualConditions as join keys, not by the join's output.
    // Column pruning (FlagPruneColumnsAgain) runs later and will rebuild
    // schemas correctly.
    return anyChanged
}

findCastInProj looks up a column in a Projection's schema and checks if the
corresponding expression is CAST(col AS DOUBLE). Returns nil if not matched.
findCastInProj 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
pub fn findCastInProj(proj *logicalop.LogicalProjection, col *expression.Column, evalCtx expression.EvalContext) *projCastInfo {
    schema := proj.Schema()
    for i, schemaCol := range schema.Columns {
        if schemaCol.UniqueID != col.UniqueID {
            continue
        }
        if i >= len(proj.Exprs) {
            return nil
        }
        sf, ok := proj.Exprs[i].(*expression.ScalarFunction)
        if !ok || sf.FuncName.L != ast.Cast {
            return nil
        }
        if sf.GetType(evalCtx).EvalType() != types.ETReal {
            return nil
        }
        args := sf.GetArgs()
        if len(args) != 1 {
            return nil
        }
        origCol, ok := args[0].(*expression.Column)
        if !ok {
            return nil
        }
        return &projCastInfo{
            proj:    proj,
            exprIdx: i,
            origCol: origCol,
        }
    }
    return nil
}

classifyCastPair determines which side is INT (signed) and which is STRING.
classifyCastPair 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
pub fn classifyCastPair(leftInfo, rightInfo *projCastInfo) (intInfo, strInfo *projCastInfo) {
    isSignedInt := func(info *projCastInfo) bool {
        tp := info.origCol.GetType(nil)
        if tp.EvalType() != types.ETInt || mysql.HasUnsignedFlag(tp.GetFlag()) {
            return false
        }
        // Keep BIGINT on the original DOUBLE-domain comparison path. Rewriting
        // VARCHAR = BIGINT to integer equality changes the observable result once
        // values cross DOUBLE's exact-integer boundary (for example 2^53 + 1).
        return tp.GetType() != mysql.TypeLonglong
    }
    isStr := func(info *projCastInfo) bool {
        return info.origCol.GetType(nil).EvalType().IsStringKind()
    }

    switch {
    case isSignedInt(leftInfo) && isStr(rightInfo):
        return leftInfo, rightInfo
    case isStr(leftInfo) && isSignedInt(rightInfo):
        return rightInfo, leftInfo
    default:
        return nil, nil
    }
}
*/
