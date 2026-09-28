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

// 外连接到半连接（Outer Join → Semi/Anti-Semi Join）逻辑优化规则。
//
// 识别 `LEFT/RIGHT JOIN` 后紧跟仅作用于内表列、且含 `IS NULL` 或拒空谓词的
// Selection，将其改写为 Semi/AntiSemi Join，语义上对应「找匹配/找不匹配」查询。

// 优化规则的数据结构、递归控制流和错误传播形状。
// 外部 TiDB 类型与函数调用保留为后续模块接线点；本任务不创建跨文件模块连线。
//
// Go imports（仅保留来源依赖，暂不接入 Rust crate）：
// 	"context"
// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/parser/ast"
// 	"github.com/pingcap/tidb/pkg/parser/mysql"
// 	"github.com/pingcap/tidb/pkg/planner/core/base"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/logicalop"
// 	"github.com/pingcap/tidb/pkg/util/intset"
//
// OuterJoinToSemiJoin is a logical optimization rule that rewrites a `LogicalJoin` (Outer Join)
// into an `AntiSemiJoin`. The transformation is triggered by a subsequent `LogicalSelection`
// that effectively filters for non-matched rows from the outer join.
// The core idea is to identify queries whose semantics are to find rows from one table that
// have NO match in another. This rule recognizes such patterns and changes the `LogicalJoin`'s
// `JoinType` to `AntiSemiJoin`.
// A key part of this transformation is the creation of a `LogicalProjection` on top of the
// new `AntiSemiJoin`. This projection is responsible for generating the `NULL` values for the
// columns of the inner table, which is the expected result for this type of query.
// The rule is triggered if the `WHERE` clause checks for `IS NULL` on a column from the
// inner table that is guaranteed to be non-null if a match had occurred. This guarantee
// comes from two main patterns identified in the `canConvertAntiJoin` function:
//  1. The `IS NULL` check is on a column that is part of the join condition.
//     SQL Example: `SELECT B.* FROM A RIGHT JOIN B ON A.id = B.a_id WHERE A.id IS NULL`
//  2. The `IS NULL` check is on a column that has a `NOT NULL` constraint in its table definition.
//     SQL Example: `SELECT A.* FROM A LEFT JOIN B ON A.id = B.a_id WHERE B.non_null_col IS NULL`
// In both cases, the original `LogicalSelection` is eliminated, and the plan is rewritten to
// `LogicalProjection -> LogicalJoin(AntiSemiJoin)`.
// type OuterJoinToSemiJoin struct{}
//
// Optimize implements base.LogicalOptRule.<0th> interface.
// Optimize 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (o *OuterJoinToSemiJoin) Optimize(_ context.Context, p base.LogicalPlan) (base.LogicalPlan, bool, error) {
//     result, isChanged := o.recursivePlan(p)
//     return result, isChanged, nil
// }
// recursivePlan 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (o *OuterJoinToSemiJoin) recursivePlan(p base.LogicalPlan) (base.LogicalPlan, bool) {
//     var isChanged bool
//     if sel, ok := p.(*logicalop.LogicalSelection); ok {
//         p, changed := o.dealWithSelection(nil, 0, sel)
//         isChanged = isChanged || changed
//         return p, isChanged
//     }
//     for idx, child := range p.Children() {
//         var changed bool
//         sel, ok := child.(*logicalop.LogicalSelection)
//         if !ok {
//             _, changed = o.recursivePlan(child)
//             isChanged = isChanged || changed
//             continue
//         }
//         _, changed = o.dealWithSelection(p, idx, sel)
//         isChanged = isChanged || changed
//     }
//     return p, isChanged
// }
//
// dealWithSelection 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (o *OuterJoinToSemiJoin) dealWithSelection(p base.LogicalPlan, childIdx int, sel *logicalop.LogicalSelection) (base.LogicalPlan, bool) {
//     selChild := sel.Children()[0]
//     switch cc := selChild.(type) {
//     case *logicalop.LogicalJoin:
//         newP, changed := o.startConvertOuterJoinToSemiJoin(p, childIdx, sel, cc)
//         return ensureSelectionRoot(newP, sel), changed
//     case *logicalop.LogicalProjection:
//         if validProjForConvertAntiJoin(cc) {
//             projectionChild := cc.Children()[0]
//             join, ok := projectionChild.(*logicalop.LogicalJoin)
//             if ok {
//                 newP, changed := o.startConvertOuterJoinToSemiJoin(p, childIdx, sel, join)
//                 return ensureSelectionRoot(newP, sel), changed
//             }
//             newChild, changed := o.recursivePlan(projectionChild)
//             resetChildIfChanged(cc, projectionChild, newChild)
//             return ensureSelectionRoot(p, sel), changed
//         }
//         newChild, changed := o.recursivePlan(cc)
//         resetChildIfChanged(sel, cc, newChild)
//         return ensureSelectionRoot(p, sel), changed
//     }
//     newChild, changed := o.recursivePlan(selChild)
//     resetChildIfChanged(sel, selChild, newChild)
//     return ensureSelectionRoot(p, sel), changed
// }
//
// startConvertOuterJoinToSemiJoin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (o *OuterJoinToSemiJoin) startConvertOuterJoinToSemiJoin(p base.LogicalPlan, childIdx int, sel *logicalop.LogicalSelection, join *logicalop.LogicalJoin) (base.LogicalPlan, bool) {
//     proj, ok := canConvertAntiJoin(join, sel.Conditions, sel.Schema())
//     if ok {
//         var pChild base.LogicalPlan
//         if proj != nil {
// projection <- anti semi join
//             proj.SetChildren(join)
//             pChild = proj
//         } else {
// anti semi join only
//             pChild = join
//         }
//         if p != nil {
//             p.SetChild(childIdx, pChild)
//         } else {
//             p = pChild
//         }
//     }
//     _, changed := o.recursivePlan(join)
//     return p, ok || changed
// }
//
// ensureSelectionRoot 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn ensureSelectionRoot(p base.LogicalPlan, sel *logicalop.LogicalSelection) base.LogicalPlan {
//     if p == nil {
//         return sel
//     }
//     return p
// }
//
// resetChildIfChanged 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn resetChildIfChanged(parent, oldChild, newChild base.LogicalPlan) {
//     if newChild != oldChild {
//         parent.SetChildren(newChild)
//     }
// }
//
// Name implements base.LogicalOptRule.<1st> interface.
// Name 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn (*OuterJoinToSemiJoin) Name() string {
//     return "outer_join_to_semi_join"
// }
//
// canConvertAntiJoin is used in outer-join-to-semi-join rule.
// /*
// #### Scenario 1: IS NULL on the Join Condition Column
//
// - In this scenario, the IS NULL filter is applied directly to the join key from the inner table.
//
// ##### Table Schema
// CREATE TABLE Table_A (id INT, name VARCHAR(50));
// CREATE TABLE Table_B (id INT, info VARCHAR(50));
//
// ##### Plan
// ```
// -- The optimizer rewrites the LEFT JOIN ... WHERE ... IS NULL pattern into an efficient
// -- Anti Semi Join to retrieve rows from Table A that have no match in Table B.
// SELECT Table_A.id, Table_A.name
// FROM
//     Table_A
// LEFT JOIN
//     Table_B ON Table_A.id = Table_B.id
// WHERE
//     Table_B.id IS NULL;
//
// =>
//
// SELECT Table_A.id, Table_A.name
// FROM
//     Table_A
// WHERE NOT EXISTS (
//     SELECT 1
//     FROM
//         Table_B
//     WHERE
//         Table_A.id = Table_B.id
// );
// ```
// #### Scenario 2: IS NULL on a Non-Join NOT NULL Column
//
// If a column in the inner table is defined as `NOT NULL` in the schema but is filtered as `IS NULL` after the join,
// and that column comes from the null-supplied side of the outer join, this indicates that the join did not find a match.
// This allows us to convert the join into an `ANTI SEMI JOIN`, even if the column is not part of the join keys.
//
// ##### Table Schema:
//
// CREATE TABLE Table_A (id INT,name VARCHAR(50));
// CREATE TABLE Table_B (
//     id INT,
//     status VARCHAR(20) NOT NULL -- Non-join column with NOT NULL constraint
// );
//
// ##### Plan
// ```
// -- Even though 'status' is not a join key, its NOT NULL constraint
// -- ensures that 'B.status IS NULL' only occurs when no match is found.
// SELECT A.* FROM Table_A A
// LEFT JOIN Table_B B ON A.id = B.id
// WHERE B.status IS NULL;
//
// =>
// SELECT A.* FROM Table_A A
// WHERE NOT EXISTS (
//     SELECT 1 FROM Table_B B WHERE A.id = B.id
// );
// ```
//
// Additionally, we found that there may be a projection between select and join.
//     Select[isnull(col1)] <- Projection[col1,col2,col3,col4] <- left outer join[outer side: col1 col2, inner side: col3 col4]
// Currently, we only support projections with column mapping relationships, not transformation relationships.
//     Projection[null->col1,null->col2,col3,col4] <- anti semi join[outer side: col1 col2, inner side: col3 col4]
// */
// canConvertAntiJoin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn canConvertAntiJoin(p *logicalop.LogicalJoin, selectCond []expression.Expression, selectSch *expression.Schema) (resultProj *logicalop.LogicalProjection, canConvertToAntiSemiJoin bool) {
//     if len(selectCond) != 1 || (len(p.EqualConditions) == 0 && len(p.OtherConditions) == 0) {
// This optimization only supports a single selection condition in selectCond.
// Any condition that refers only to the outer side of the join can be pushed below the join,
// so the remaining selection condition must be on the inner side.
// If multiple inner-side conditions were semantically equivalent, the optimizer should keep just one
// and eliminate the others, so we require exactly one selection condition here.
//         return nil, false
//     }
//     if _, ok := p.Self().(*logicalop.LogicalApply); ok {
// For now, we won't add complexity in the outer-join-to-semi-join rule.
// Additionally, existing user cases haven't been applied yet; they can be added later if needed.
//         return nil, false
//     }
//     var outerChildIdx int
//     switch p.JoinType {
//     case base.LeftOuterJoin:
//         outerChildIdx = 0
//     case base.RightOuterJoin:
//         outerChildIdx = 1
//     default:
//         return nil, false
//     }
//
//     sf, ok := selectCond[0].(*expression.ScalarFunction)
//     if !ok {
//         return nil, false
//     }
//     if sf.FuncName.L != ast.IsNull {
//         return nil, false
//     }
//     args := sf.GetArgs()
// Get the Column in the IsNull
//     isNullCol, ok := args[0].(*expression.Column)
//     if !ok {
//         return nil, false
//     }
//     outer := p.Children()[outerChildIdx]
//     outerSchema := outer.Schema()
//     innerSchemaSet := intset.NewFastIntSet()
// Identify all columns from the inner side of the join (columns that are not in the outer schema).
// If the column referenced in the IS NULL predicate is one of these inner-side columns used in
// equal/other join conditions, the outer join can be safely rewritten as an anti-semi join.
//     expression.ExtractColumnsSetFromExpressions(&innerSchemaSet, func(c *expression.Column) bool {
//         return !outerSchema.Contains(c)
//     }, expression.Column2Exprs(p.Schema().Columns)...)
// Scenario 1: column in IsNull expression is from the inner side columns in the eq/other condition.
//     joinCondNRInnerCol := joinCondNullRejectsInnerCol(&innerSchemaSet, isNullCol.UniqueID, p.EqualConditions, p.OtherConditions)
//
// Scenario 2:
//  column in IsNull expression is from the inner side columns.
//  but it is not in the equal/other condition.
//  We need to check whether inner column is not null in the origin table schema.
//  If it is not null column, it can be directly converted into an anti-semi join.
//  Use FD's not-null info to avoid treating columns that become nullable via outer joins as NOT NULL.
//     innerChild := p.Children()[1^outerChildIdx]
//     innerSch := innerChild.Schema()
// if isNullColInnerSchIdx < 0, it means the is null column is not in the inner schema.
//     isNullColInnerSchIdx := innerSch.ColumnIndex(isNullCol)
//     isInnerNonNullCol := false
//     if isNullColInnerSchIdx >= 0 {
//         fds := innerChild.ExtractFD()
//         if fds != nil {
//             isInnerNonNullCol = fds.NotNullCols.Has(int(isNullCol.UniqueID))
//         } else {
// Fallback to schema flag if FDs are unavailable.
//             isInnerNonNullCol = mysql.HasNotNullFlag(innerSch.Columns[isNullColInnerSchIdx].RetType.GetFlag())
//         }
//     }
//
// If either scenario is met, it can be converted to an anti-semi join.
//     canConvertToAntiSemiJoin = joinCondNRInnerCol || isInnerNonNullCol
//     if canConvertToAntiSemiJoin {
// resultProj is to generate the NULL values for the columns of the inner table, which is the
// expected result for this kind of anti-join query.
//         resultProj = generateProjectForConvertAntiJoin(p, &innerSchemaSet, outerSchema, selectSch)
// Anti-semi join's first child is outer, the second child is inner. join condition is outer op inner
// right outer join's first child is inner, the second child is outer, join condition is inner op outer
// left outer join's first child is outer, the second child is inner. join condition is outer op inner
// So we have to swap it here.
//         ctx := p.SCtx().GetExprCtx()
//         if p.JoinType == base.RightOuterJoin {
//             for idx, expr := range p.EqualConditions {
//                 args := expr.GetArgs()
//                 p.EqualConditions[idx] = expression.NewFunctionInternal(ctx, expr.FuncName.L, expr.GetType(ctx.GetEvalCtx()), args[1], args[0]).(*expression.ScalarFunction)
//             }
// it is not needed for the other conditions because the arguments order does not matter.
//             args := p.Children()
//             p.SetChildren(args[1], args[0])
//             tmp := p.LeftConditions
//             p.LeftConditions = p.RightConditions
//             p.RightConditions = tmp
//         }
//         p.JoinType = base.AntiSemiJoin
//         p.MergeSchema()
//     }
//     return resultProj, canConvertToAntiSemiJoin
// }
//
// validProjForConvertAntiJoin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn validProjForConvertAntiJoin(proj *logicalop.LogicalProjection) bool {
//     if proj == nil {
//         return true
//     }
// Sometimes there is a projection between select and join.
// We require that this projection does not make additional changes to any columns,
// then we can continue converting to a semi join.
//  Pass: Projection[col1,col2]
//  Fail: Projection[col2->col2_renamed, col2+1->col2_incremented]
//     for idx, c := range proj.Schema().Columns {
//         if !c.Equals(proj.Exprs[idx]) {
//             return false
//         }
//     }
//     return true
// }
//
// joinCondNullRejectsInnerCol checks whether the IS NULL column (above the join) is from the inner
// side and appears in the join predicate that null-rejects it (excluding NullEQ; only GT/GE/LE/LT/NE in others).
// joinCondNullRejectsInnerCol 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn joinCondNullRejectsInnerCol(innerSchSet *intset.FastIntSet, isNullColumnID int64, eq []*expression.ScalarFunction, other []expression.Expression) bool {
//     for _, s := range eq {
//         if s.FuncName.L == ast.NullEQ {
//             continue
//         }
//         col1, col2, ok := expression.IsColOpCol(s)
//         if ok {
//             if innerSchSet.Has(int(col1.UniqueID)) {
//                 if isNullColumnID == col1.UniqueID {
//                     return true
//                 }
//             } else if innerSchSet.Has(int(col2.UniqueID)) {
//                 if isNullColumnID == col2.UniqueID {
//                     return true
//                 }
//             }
//         }
//     }
//     for _, s := range other {
//         if sf, ok := s.(*expression.ScalarFunction); ok {
//             switch sf.FuncName.L {
//             case ast.GT, ast.GE, ast.LE, ast.LT, ast.NE:
//                 col1, col2, ok := expression.IsColOpCol(sf)
//                 if ok {
//                     if innerSchSet.Has(int(col1.UniqueID)) && isNullColumnID == col1.UniqueID {
//                         return true
//                     } else if innerSchSet.Has(int(col2.UniqueID)) && isNullColumnID == col2.UniqueID {
//                         return true
//                     }
//                 }
//             default:
//             }
//         }
//     }
//     return false
// }
//
// generateProjectForConvertAntiJoin is to generate projection and put it on the anti-semi join which is from outer join.
// outer column will not be changed. inner column will output the null.
// generateProjectForConvertAntiJoin 对应 Go 的同名函数或方法；保留原参数、分支和错误传播语义。
// pub fn generateProjectForConvertAntiJoin(p *logicalop.LogicalJoin, innerSchSet *intset.FastIntSet, outerSchema, parentNodeSchema *expression.Schema) (proj *logicalop.LogicalProjection) {
//     isAllFromOuterSide := true
//     for _, c := range parentNodeSchema.Columns {
//         if !outerSchema.Contains(c) {
//             isAllFromOuterSide = false
//             break
//         }
//     }
//     if isAllFromOuterSide {
//         return nil
//     }
//     projExprs := make([]expression.Expression, 0, len(parentNodeSchema.Columns))
//     for _, c := range parentNodeSchema.Columns {
//         if innerSchSet.Has(int(c.UniqueID)) {
// Keep null-extended inner columns aligned with the parent schema type.
// NewNull() defaults to TINYINT, while the original column can have
// another type and must become nullable after the outer join rewrite.
//             retType := c.RetType.Clone()
//             retType.DelFlag(mysql.NotNullFlag)
//             projExprs = append(projExprs, expression.NewNullWithFieldType(retType))
//         } else {
//             projExprs = append(projExprs, c.Clone())
//         }
//     }
//     proj = logicalop.LogicalProjection{Exprs: projExprs}.Init(p.SCtx(), p.QueryBlockOffset())
//     proj.SetSchema(parentNodeSchema.Clone())
//     return
// }
// */
use crate::rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind};

/// 外连接转半连接规则：把「外连接 + 内表 IS NULL/拒空过滤」改成 Semi/AntiSemi。
pub struct OuterJoinToSemiJoin;

impl LogicalRule for OuterJoinToSemiJoin {
    fn name(&self) -> &'static str {
        "outer_join_to_semi_join"
    }

    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        let changed = convert(&mut plan);
        Ok((plan, changed))
    }
}

/// 后序转换：仅当 Selection 紧贴 Left/Right Outer Join 且谓词只引用内表列时改写。
fn convert(plan: &mut Plan) -> bool {
    let mut changed = plan
        .children
        .iter_mut()
        .fold(false, |seen, child| convert(child) || seen);
    if !matches!(plan.kind, PlanKind::Selection)
        || plan.children.len() != 1
        || plan.predicates.len() != 1
    {
        return changed;
    }
    let join = &mut plan.children[0];
    let has_join_condition = matches!(
        &join.kind,
        PlanKind::Join {
            equal_conditions,
            other_conditions,
            ..
        } if !equal_conditions.is_empty() || !other_conditions.is_empty()
    );
    if !has_join_condition {
        return changed;
    }
    // 内表下标：LEFT JOIN 为右孩子，RIGHT JOIN 为左孩子。
    let join_type = match &join.kind {
        PlanKind::Join { join_type, .. } => *join_type,
        _ => return changed,
    };
    let inner_index = match join_type {
        JoinType::LeftOuter => 1,
        JoinType::RightOuter => 0,
        _ => return changed,
    };
    if join.children.len() != 2 {
        return changed;
    }
    let inner_columns = join.children[inner_index].all_columns();
    let outer_columns = join.children[1 - inner_index].all_columns();
    let Some(predicate) = plan.predicates.first() else {
        return changed;
    };
    let Expr::Scalar { function, args, .. } = predicate else {
        return changed;
    };
    let Some(Expr::Column { id, .. }) = args.first() else {
        return changed;
    };
    if function != "is_null"
        || args.len() != 1
        || !inner_columns.contains(id)
        || outer_columns.contains(id)
    {
        return changed;
    }
    // The IS NULL column must be null-rejected by a real join condition.
    let join_rejects_inner_column = match &join.kind {
        PlanKind::Join {
            equal_conditions,
            other_conditions,
            ..
        } => {
            equal_conditions
                .iter()
                .any(|condition| column_comparison_rejects(condition, id, None))
                || other_conditions.iter().any(|condition| {
                    column_comparison_rejects(condition, id, Some(&["gt", "ge", "lt", "le", "ne"]))
                })
        }
        _ => false,
    };
    if !join_rejects_inner_column {
        return changed;
    }
    // Go's outer-join conversion is the IS NULL anti-semi rewrite.
    let PlanKind::Join {
        join_type,
        equal_conditions,
        ..
    } = &mut join.kind
    else {
        return changed;
    };
    if *join_type == JoinType::RightOuter {
        for condition in equal_conditions {
            if let Expr::Scalar { args, .. } = condition
                && args.len() == 2
            {
                args.swap(0, 1);
            }
        }
        join.children.swap(0, 1);
    }
    *join_type = JoinType::AntiSemi;
    join.schema = join.children[0].schema.clone();
    plan.schema = join.schema.clone();
    changed = true;
    changed
}

/// Match Go's `expression.IsColOpCol`: only a binary scalar function whose
/// operands are both direct column references is a null-rejecting join key.
fn column_comparison_rejects(
    condition: &Expr,
    inner_column: &i64,
    allowed_functions: Option<&[&str]>,
) -> bool {
    let Expr::Scalar { function, args, .. } = condition else {
        return false;
    };
    if function == "null_eq"
        || allowed_functions.is_some_and(|allowed| !allowed.contains(&function.as_str()))
        || args.len() != 2
    {
        return false;
    }
    matches!((&args[0], &args[1]),
        (Expr::Column { id: left, .. }, Expr::Column { id: right, .. })
            if left == inner_column || right == inner_column)
}
