// Copyright 2026 AsterSQL.
// 常量传播（constant propagation）优化。
//
// 在 CNF（合取范式）谓词中根据「列 = 常量」与列等价类做常量替换，
// 并区分内连接/外连接：外连接仅从 preserved/outer 侧提取等式，
// 避免错误收紧 inner 侧可空性。对应 Go `constant_propagation.go`。

// Copyright 2016 PingCAP, Inc.
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

use crate::util_kernel::*;
use crate::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

/// 传播时视为「不可改写」的函数名集合（当前含 IsNull）。
static inequalFunctions: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from([ast::IsNull]));

// 本文件对应 pkg/expression/constant_propagation.go，保留 CNF/DNF、等价类及内外连接传播流程。
// Rust 通过拥有型上下文包装和局部对象生命周期实现 Go 接口包装与 sync.Pool 清理语义。

/// 并查集：维护列等价类，FindRoot 带路径压缩。
struct SimpleIntSet {
    parent: Vec<usize>,
}

impl SimpleIntSet {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn Clear(&mut self) {
        self.parent.clear();
    }

    fn GrowNewIntSet(&mut self, size: usize) {
        self.parent.clear();
        self.parent.extend(0..size);
    }

    fn FindRoot(&mut self, value: usize) -> usize {
        let mut root = value;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = value;
        while current != root {
            let parent = self.parent[current];
            self.parent[current] = root;
            current = parent;
        }
        root
    }

    fn Union(&mut self, left: usize, right: usize) {
        let left = self.FindRoot(left);
        let right = self.FindRoot(right);
        self.parent[left] = right;
    }
}

/// util 的替换 API 使用 `&mut BuildContext`，但上下文本身通过内部可变性记录告警和缓存决策。
/// 此适配器只提供独占的转发表，不复制或简化任何会话语义。
struct BuildContextProxy<'a> {
    inner: &'a dyn BuildContext,
}

impl BuildContext for BuildContextProxy<'_> {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.inner.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.inner.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.inner.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.inner.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.inner.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.inner.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.inner.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.inner.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.inner.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.inner.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.inner.IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        self.inner.IsConstantPropagateCheck()
    }
    fn ConnectionID(&self) -> u64 {
        self.inner.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.inner.IsReadonlyUserVar(name)
    }
}

/// 拥有型常量传播上下文，对应 Go `exprctx.WithConstantPropagateCheck`。
/// `Rc` 允许 DNF 分支复用同一会话快照，同时只覆盖常量传播检查标志。
struct ConstantPropagateContext<'a> {
    inner: Rc<dyn exprctx::ExprContext + 'a>,
}

/// 借用型 ExprContext 适配器，使只持有 planner 上下文借用的调用方也能复用完整求解器。
struct BorrowedExprContext<'a> {
    inner: &'a dyn exprctx::ExprContext,
}

impl BuildContext for BorrowedExprContext<'_> {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.inner.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.inner.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.inner.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.inner.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.inner.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.inner.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.inner.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.inner.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.inner.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.inner.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.inner.IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        true
    }
    fn ConnectionID(&self) -> u64 {
        self.inner.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.inner.IsReadonlyUserVar(name)
    }
}

impl exprctx::ExprContext for BorrowedExprContext<'_> {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        self.inner.GetWindowingUseHighPrecision()
    }

    fn GetGroupConcatMaxLen(&self) -> u64 {
        self.inner.GetGroupConcatMaxLen()
    }
}

impl BuildContext for ConstantPropagateContext<'_> {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        self.inner.GetEvalCtx()
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        self.inner.GetCharsetInfo()
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.inner.GetDefaultCollationForUTF8MB4()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        self.inner.GetBlockEncryptionMode()
    }
    fn GetSysdateIsNow(&self) -> bool {
        self.inner.GetSysdateIsNow()
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        self.inner.GetNoopFuncsMode()
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        self.inner.Rng()
    }
    fn IsUseCache(&self) -> bool {
        self.inner.IsUseCache()
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        self.inner.SetSkipPlanCache(reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        self.inner.AllocPlanColumnID()
    }
    fn IsInNullRejectCheck(&self) -> bool {
        self.inner.IsInNullRejectCheck()
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        true
    }
    fn ConnectionID(&self) -> u64 {
        self.inner.ConnectionID()
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        self.inner.IsReadonlyUserVar(name)
    }
}

impl exprctx::ExprContext for ConstantPropagateContext<'_> {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        self.inner.GetWindowingUseHighPrecision()
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        self.inner.GetGroupConcatMaxLen()
    }
}

/// 单个 CNF 中参与传播的列上限，防止等价类两层遍历造成优化时间爆炸。
pub static mut MaxPropagateColsCnt: usize = 100;
/// 判定派生谓词是否允许保留的回调类型（连接侧过滤常用）。
pub type VaildConstantPropagationExpressionFuncType = fn(&dyn Expression) -> bool;

/// 传播求解器共享状态：列映射、等式常量表、并查集与会话上下文。
struct BasePropConstSolver<'a> {
    col_mapper: HashMap<i64, usize>,
    eq_mapper: HashMap<usize, Constant>,
    union_set: SimpleIntSet,
    columns: Vec<Column>,
    ctx: Option<Rc<dyn exprctx::ExprContext + 'a>>,
}

fn newBasePropConstSolver<'a>() -> BasePropConstSolver<'a> {
    BasePropConstSolver {
        col_mapper: HashMap::with_capacity(4),
        eq_mapper: HashMap::with_capacity(4),
        union_set: SimpleIntSet::new(4),
        columns: Vec::with_capacity(4),
        ctx: None,
    }
}

impl BasePropConstSolver<'_> {
    /// Clear 对应归还对象池前的资源收尾，避免列、常量或会话上下文泄漏到下一次优化。
    fn Clear(&mut self) {
        self.col_mapper.clear();
        self.eq_mapper.clear();
        self.columns.clear();
        self.union_set.Clear();
        self.ctx = None;
    }
    fn getColID(&self, column: &Column) -> usize {
        self.col_mapper[&column.UniqueID]
    }
    fn insertCols(&mut self, columns: HashMap<i64, Column>) {
        for (unique_id, column) in columns {
            if !self.col_mapper.contains_key(&unique_id) {
                self.col_mapper.insert(unique_id, self.col_mapper.len());
                self.columns.push(column);
            }
        }
    }
    fn tryToUpdateEQList(&mut self, column: &Column, constant: &Constant) -> (bool, bool) {
        let ctx = self.ctx.as_ref().unwrap();
        if constant.Value.IsNull() && ConstExprConsiderPlanCache(constant, ctx.IsUseCache()) {
            return (false, true);
        }
        let id = self.getColID(column);
        if let Some(old) = self.eq_mapper.get(&id) {
            // 同一等价类出现不同常量（或比较出错）意味着整个 CNF 永假。
            let collator = collate::GetCollator(column.GetType(ctx.GetEvalCtx()).GetCollate());
            let conflict = old
                .Value
                .Compare(
                    ctx.GetEvalCtx().TypeCtx(),
                    &constant.Value,
                    collator.as_ref(),
                )
                .map_or(true, |order| order != 0);
            return (false, conflict);
        }
        self.eq_mapper.insert(id, constant.Clone());
        (true, false)
    }

    fn dealWithPossibleHybridType(&self, column: &Column, constant: &Constant) -> Option<Constant> {
        let ctx = self.ctx.as_ref().unwrap();
        if !column.GetType(ctx.GetEvalCtx()).Hybrid() {
            return Some(constant.Clone());
        }
        if column.GetType(ctx.GetEvalCtx()).GetType() != mysql::TypeEnum {
            return None;
        }
        let datum = constant
            .Eval(ctx.GetEvalCtx(), chunk::Row::default())
            .ok()?;
        if crate::core_support::MaybeOverOptimized4PlanCache(ctx.as_ref(), constant) {
            ctx.SetSkipPlanCache(
                "Skip plan cache since mutable constant is restored and propagated",
            );
        }
        // ENUM 只接受序号、名称或已经恢复的混合 Datum；其它 Kind 暂不安全传播。
        let value = match datum.Kind() {
            types::KindInt64 => types::ParseEnumValue(
                column.GetType(ctx.GetEvalCtx()).GetElems(),
                datum.GetInt64() as u64,
            )
            .ok()?,
            types::KindString => types::ParseEnumName(
                column.GetType(ctx.GetEvalCtx()).GetElems(),
                &datum.GetString(),
                &datum.Collation(),
            )
            .ok()?,
            types::KindMysqlEnum | types::KindMysqlSet => return Some(constant.Clone()),
            _ => return None,
        };
        Some(Constant::with_collation(
            types::NewMysqlEnumDatum(value),
            column.RetType.clone().unwrap(),
            column.collation_info.clone(),
        ))
    }

    fn extractColumnsInternal(
        &mut self,
        map: &mut HashMap<i64, Column>,
        target: &mut Vec<Box<dyn Expression>>,
        conds: Vec<Box<dyn Expression>>,
    ) {
        for condition in conds {
            target.extend(SplitCNFItems(condition.as_ref()));
            for column in ExtractColumns(condition.as_ref()) {
                map.entry(column.UniqueID).or_insert_with(|| column.clone());
            }
            self.insertCols(std::mem::take(map));
        }
    }
}

/// 检查 GT/GE/LT/LE/EQ 的一侧为列、另一侧为常量，且双方 collation 一致。
pub fn ValidCompareConstantPredicate(ctx: &dyn EvalContext, candidate: &dyn Expression) -> bool {
    let Some(function) = candidate.as_scalar_function() else {
        return false;
    };
    if !matches!(
        function.FuncName.L.as_str(),
        ast::GT | ast::GE | ast::LT | ast::LE | ast::EQ
    ) {
        return false;
    }
    ValidCompareConstantPredicateHelper(ctx, function, true).is_some()
        || ValidCompareConstantPredicateHelper(ctx, function, false).is_some()
}

/// 按列在左或右，抽取比较谓词中的 (Column, Constant) 对。
pub fn ValidCompareConstantPredicateHelper(
    ctx: &dyn EvalContext,
    function: &ScalarFunction,
    col_is_left: bool,
) -> Option<(Column, Constant)> {
    let (column_arg, constant_arg) = if col_is_left {
        (&function.GetArgs()[0], &function.GetArgs()[1])
    } else {
        (&function.GetArgs()[1], &function.GetArgs()[0])
    };
    let column = column_arg.as_column()?;
    let constant = constant_arg.as_constant()?;
    if column.GetStaticType().GetCollate() != constant.GetType(ctx)?.GetCollate() {
        return None;
    }
    Some((column.clone(), constant.Clone()))
}

fn validEqualCond(ctx: &dyn EvalContext, condition: &dyn Expression) -> Option<(Column, Constant)> {
    let function = condition.as_scalar_function()?;
    if function.FuncName.L != ast::EQ {
        return None;
    }
    ValidCompareConstantPredicateHelper(ctx, function, true)
        .or_else(|| ValidCompareConstantPredicateHelper(ctx, function, false))
}

fn true_constant() -> Box<dyn Expression> {
    Box::new(Constant::with_type(
        types::NewDatum(&true),
        *types::NewFieldType(mysql::TypeTiny),
    ))
}
fn false_constant() -> Box<dyn Expression> {
    Box::new(Constant::with_type(
        types::NewDatum(&false),
        *types::NewFieldType(mysql::TypeTiny),
    ))
}

fn replaceEqCondtionWithTrue(
    ctx: &dyn BuildContext,
    src: &Column,
    target: &Column,
    condition: Box<dyn Expression>,
) -> (Box<dyn Expression>, bool) {
    if src.GetStaticType().GetType() != target.GetStaticType().GetType() {
        return (condition, false);
    }
    let Some(function_ref) = condition.as_scalar_function() else {
        return (condition, false);
    };
    let mut function = function_ref.clone_scalar();
    match function.FuncName.L.as_str() {
        ast::In => {
            // 字符串 IN 的返回 collation 由全部参数共同推导，替换列可能改变类型，因此直接跳过。
            if src.GetType(ctx.GetEvalCtx()).EvalType() == types::ETString
                || target.GetType(ctx.GetEvalCtx()).EvalType() == types::ETString
            {
                return (condition, false);
            }
            let hit = (function.GetArgs()[0].Equal(ctx.GetEvalCtx(), src)
                && function.GetArgs()[1..]
                    .iter()
                    .any(|a| a.Equal(ctx.GetEvalCtx(), target)))
                || (function.GetArgs()[0].Equal(ctx.GetEvalCtx(), target)
                    && function.GetArgs()[1..]
                        .iter()
                        .any(|a| a.Equal(ctx.GetEvalCtx(), src)));
            if hit {
                return (true_constant(), true);
            }
        }
        ast::EQ => {
            let args = function.GetArgs();
            if (args[0].Equal(ctx.GetEvalCtx(), src) && args[1].Equal(ctx.GetEvalCtx(), target))
                || (args[1].Equal(ctx.GetEvalCtx(), src) && args[0].Equal(ctx.GetEvalCtx(), target))
            {
                return (true_constant(), true);
            }
        }
        ast::LogicOr | ast::LogicAnd => {
            let mut replaced = false;
            for argument in function.GetArgsMut() {
                let (new_arg, changed) =
                    replaceEqCondtionWithTrue(ctx, src, target, argument.clone());
                if changed {
                    *argument = new_arg;
                    replaced = true;
                }
            }
            if replaced {
                let rebuilt = NewFunctionInternal(
                    ctx,
                    &function.FuncName.L,
                    function.RetType.clone().unwrap(),
                    function.GetArgs().iter().cloned().collect(),
                )
                .unwrap_or(condition);
                return (rebuilt, true);
            }
        }
        _ => {}
    }
    (condition, false)
}

/// 返回 (是否替换、是否含不确定表达式、结果)。不确定或 NULL 敏感函数出现时整棵子树放弃替换。
fn tryToReplaceCond(
    ctx: &dyn BuildContext,
    src: &Column,
    target: &Column,
    condition: Box<dyn Expression>,
    null_aware: bool,
) -> (bool, bool, Box<dyn Expression>) {
    if src.GetStaticType().GetType() != target.GetStaticType().GetType() {
        return (false, false, condition);
    }
    let Some(function_ref) = condition.as_scalar_function() else {
        return (false, false, condition);
    };
    let mut function = function_ref.clone_scalar();
    if unFoldableFunctions.contains_key(function.FuncName.L.as_str())
        || inequalFunctions.contains(function.FuncName.L.as_str())
    {
        return (false, true, condition);
    }
    if null_aware
        && matches!(
            function.FuncName.L.as_str(),
            ast::Ifnull | ast::If | ast::Case | ast::NullEQ
        )
    {
        // 控制函数依赖外侧列原始可空性；替换参数可能改变外连接 NULL 扩展结果。
        return (false, true, condition);
    }
    let mut replaced = false;
    for argument in function.GetArgsMut() {
        if src.EqualColumn(argument.as_ref()) {
            let (_, collation) = condition.CharsetAndCollation();
            if target.GetType(ctx.GetEvalCtx()).GetCollate() == collation {
                *argument = Box::new(target.clone());
                replaced = true;
            }
        } else {
            let (sub_replaced, non_deterministic, sub_expr) =
                tryToReplaceCond(ctx, src, target, argument.clone(), null_aware);
            if non_deterministic {
                return (false, true, condition);
            }
            if sub_replaced {
                *argument = sub_expr;
                replaced = true;
            }
        }
    }
    if replaced {
        let name = function.FuncName.L.clone();
        let tp = function.RetType.clone().unwrap();
        let args = function.GetArgs().iter().cloned().collect();
        (
            true,
            false,
            NewFunctionInternal(ctx, &name, tp, args).unwrap_or(condition),
        )
    } else {
        (false, false, condition)
    }
}

struct PropConstSolver<'a> {
    base: BasePropConstSolver<'a>,
    conditions: Vec<Box<dyn Expression>>,
    vaild_expr_func: Option<VaildConstantPropagationExpressionFuncType>,
    schema1: Option<Schema>,
    schema2: Option<Schema>,
}

fn newPropConstSolver<'a>() -> PropConstSolver<'a> {
    PropConstSolver {
        base: newBasePropConstSolver(),
        conditions: Vec::with_capacity(4),
        vaild_expr_func: None,
        schema1: None,
        schema2: None,
    }
}

impl<'a> PropConstSolver<'a> {
    pub fn PropagateConstant(
        &mut self,
        ctx: Rc<dyn exprctx::ExprContext + 'a>,
        keep_join_key: bool,
        schema1: Option<Schema>,
        schema2: Option<Schema>,
        filter: Option<VaildConstantPropagationExpressionFuncType>,
        conditions: Vec<Box<dyn Expression>>,
    ) -> Vec<Box<dyn Expression>> {
        self.base.ctx = Some(ctx);
        self.schema1 = schema1;
        self.schema2 = schema2;
        self.vaild_expr_func = filter;
        self.solve(keep_join_key, conditions)
    }
    pub fn Clear(&mut self) {
        self.base.Clear();
        self.conditions.clear();
        self.vaild_expr_func = None;
        self.schema1 = None;
        self.schema2 = None;
    }
    fn propagateConstantEQ(&mut self) {
        let mut visited = vec![false; self.conditions.len()];
        for _ in 0..unsafe { MaxPropagateColsCnt } {
            let Some(mapper) = self.pickNewEQConds(&mut visited) else {
                return;
            };
            if mapper.is_empty() {
                return;
            }
            let columns: Vec<Column> = mapper
                .keys()
                .map(|id| self.base.columns[*id].clone())
                .collect();
            let constants: Vec<Box<dyn Expression>> = mapper
                .values()
                .map(|c| Box::new(c.Clone()) as Box<dyn Expression>)
                .collect();
            let schema = NewSchema(columns);
            for (index, condition) in self.conditions.iter_mut().enumerate() {
                if !visited[index] {
                    let mut proxy = BuildContextProxy {
                        inner: self.base.ctx.as_ref().unwrap().as_ref(),
                    };
                    *condition =
                        ColumnSubstitute(&mut proxy, condition.clone(), &schema, &constants);
                }
            }
        }
    }
    fn propagateColumnEQ(&mut self) {
        let ctx = self.base.ctx.as_ref().unwrap();
        self.base.union_set.GrowNewIntSet(self.base.columns.len());
        let mut visited = vec![false; self.conditions.len()];
        for (index, condition) in self.conditions.iter_mut().enumerate() {
            let Some(function) = condition.as_scalar_function() else {
                continue;
            };
            if function.FuncName.L != ast::EQ {
                continue;
            }
            let (left, right, ok) = IsColOpCol(function);
            if let (Some(left), Some(right), true) = (left, right, ok) {
                let compatible = left.GetType(ctx.GetEvalCtx()).GetCollate()
                    == right.GetType(ctx.GetEvalCtx()).GetCollate()
                    && !left.GetType(ctx.GetEvalCtx()).Hybrid()
                    && !right.GetType(ctx.GetEvalCtx()).Hybrid();
                if compatible {
                    let l = self.base.getColID(left);
                    let r = self.base.getColID(right);
                    visited[index] = true;
                    if self.base.union_set.FindRoot(l) != self.base.union_set.FindRoot(r) {
                        self.base.union_set.Union(l, r);
                    } else if l != r {
                        *condition = true_constant();
                    }
                }
            }
        }
        let original_len = self.conditions.len();
        for i in 0..self.base.columns.len() {
            for j in i + 1..self.base.columns.len() {
                if self.base.union_set.FindRoot(i) != self.base.union_set.FindRoot(j) {
                    continue;
                }
                let left = &self.base.columns[i];
                let right = &self.base.columns[j];
                for k in 0..original_len {
                    if visited[k] {
                        continue;
                    }
                    let (condition, _) = replaceEqCondtionWithTrue(
                        ctx.as_ref(),
                        left,
                        right,
                        self.conditions[k].clone(),
                    );
                    self.conditions[k] = condition;
                    // 两个方向都尝试，确保跨 join 两侧的等价列能生成可下推到任一子节点的条件。
                    for (src, target) in [(left, right), (right, left)] {
                        let (replaced, _, derived) = tryToReplaceCond(
                            ctx.as_ref(),
                            src,
                            target,
                            self.conditions[k].clone(),
                            false,
                        );
                        if replaced
                            && (isConstant(derived.as_ref())
                                || self.vaild_expr_func.map_or(true, |f| f(derived.as_ref())))
                        {
                            self.conditions.push(derived);
                        }
                    }
                }
            }
        }
    }
    fn setConds2ConstFalse(&mut self) {
        if crate::util_kernel::MaybeOverOptimized4PlanCache(
            self.base.ctx.as_ref().unwrap().as_ref(),
            &self.conditions,
        ) {
            self.base
                .ctx
                .as_ref()
                .unwrap()
                .SetSkipPlanCache("some parameters may be overwritten when constant propagation");
        }
        self.conditions.clear();
        self.conditions.push(false_constant());
    }
    fn pickNewEQConds(&mut self, visited: &mut [bool]) -> Option<HashMap<usize, Constant>> {
        let mut result = HashMap::new();
        for index in 0..self.conditions.len() {
            if visited[index] {
                continue;
            }
            let Some((column, constant)) = validEqualCond(
                self.base.ctx.as_ref().unwrap().GetEvalCtx(),
                self.conditions[index].as_ref(),
            ) else {
                if let Some(constant) = self.conditions[index].as_constant() {
                    visited[index] = true;
                    let single = CNFExprs(vec![Box::new(constant.Clone())]);
                    match EvalBool(
                        self.base.ctx.as_ref().unwrap().GetEvalCtx(),
                        &single,
                        chunk::Row::default(),
                    ) {
                        Ok((false, _)) => {
                            self.setConds2ConstFalse();
                            return None;
                        }
                        Err(error) => {
                            terror::Log(error);
                            return None;
                        }
                        _ => {}
                    }
                }
                continue;
            };
            if column
                .GetType(self.base.ctx.as_ref().unwrap().GetEvalCtx())
                .Hybrid()
            {
                continue;
            }
            visited[index] = true;
            let (updated, conflict) = self.base.tryToUpdateEQList(&column, &constant);
            if conflict {
                self.setConds2ConstFalse();
                return None;
            }
            if updated {
                let mut casted = constant;
                if column.GetStaticType()
                    != casted
                        .GetType(self.base.ctx.as_ref().unwrap().GetEvalCtx())
                        .as_ref()
                        .unwrap()
                {
                    // 构造 cast 时产生的告警属于优化期探测，随后截断，不能污染用户语句告警计数。
                    let count = self.base.ctx.as_ref().unwrap().GetEvalCtx().WarningCount();
                    let casted_expr: Box<dyn Expression> = Box::new(casted.Clone());
                    if let Some(new_constant) = BuildCastFunction(
                        self.base.ctx.as_ref().unwrap().as_ref(),
                        &casted_expr,
                        column.GetStaticType(),
                    )
                    .as_constant()
                    {
                        casted = new_constant.Clone();
                    }
                    self.base
                        .ctx
                        .as_ref()
                        .unwrap()
                        .GetEvalCtx()
                        .TruncateWarnings(count as isize);
                }
                result.insert(self.base.getColID(&column), casted);
            }
        }
        Some(result)
    }
    fn solve(
        &mut self,
        keep_join_key: bool,
        conditions: Vec<Box<dyn Expression>>,
    ) -> Vec<Box<dyn Expression>> {
        let original_conditions = conditions.clone();
        let join_keys = if keep_join_key {
            cloneJoinKeys(&conditions, self.schema1.as_ref(), self.schema2.as_ref())
        } else {
            Vec::new()
        };
        self.extractColumns(conditions);
        if self.base.columns.len() > unsafe { MaxPropagateColsCnt } {
            return original_conditions;
        }
        self.propagateConstantEQ();
        self.propagateColumnEQ();
        self.conditions = propagateConstantDNF(
            self.base.ctx.as_ref().unwrap().clone(),
            self.vaild_expr_func,
            std::mem::take(&mut self.conditions),
        );
        self.conditions.extend(join_keys);
        RemoveDupExprs(std::mem::take(&mut self.conditions))
    }
    fn extractColumns(&mut self, conditions: Vec<Box<dyn Expression>>) {
        let mut map = GetUniqueIDToColumnMap();
        self.base
            .extractColumnsInternal(&mut map, &mut self.conditions, conditions);
        PutUniqueIDToColumnMap(map);
    }
}

fn isConstant(condition: &dyn Expression) -> bool {
    condition.as_constant().is_some()
}

/// 连接场景常量传播：可保留 join key，并传入两侧 schema。
pub fn PropagateConstantForJoin(
    ctx: Box<dyn exprctx::ExprContext>,
    keep_join_key: bool,
    schema1: Schema,
    schema2: Schema,
    filter: Option<VaildConstantPropagationExpressionFuncType>,
    conditions: Vec<Box<dyn Expression>>,
) -> Vec<Box<dyn Expression>> {
    if conditions.is_empty() {
        return conditions;
    }
    let mut solver = newPropConstSolver();
    let inner: Rc<dyn exprctx::ExprContext> = Rc::from(ctx);
    let ctx: Rc<dyn exprctx::ExprContext> = Rc::new(ConstantPropagateContext { inner });
    let result = solver.PropagateConstant(
        ctx,
        keep_join_key,
        Some(schema1),
        Some(schema2),
        filter,
        conditions,
    );
    solver.Clear();
    result
}

/// 使用借用上下文传播连接常量，按两侧 Schema 保留连接键。
pub fn PropagateConstantForJoinRef<'a>(
    ctx: &'a dyn exprctx::ExprContext,
    keep_join_key: bool,
    schema1: Schema,
    schema2: Schema,
    filter: Option<VaildConstantPropagationExpressionFuncType>,
    conditions: Vec<Box<dyn Expression>>,
) -> Vec<Box<dyn Expression>> {
    if conditions.is_empty() {
        return conditions;
    }
    let mut solver = newPropConstSolver();
    let ctx: Rc<dyn exprctx::ExprContext + 'a> = Rc::new(BorrowedExprContext { inner: ctx });
    let result = solver.PropagateConstant(
        ctx,
        keep_join_key,
        Some(schema1),
        Some(schema2),
        filter,
        conditions,
    );
    solver.Clear();
    result
}

/// 普通选择/过滤条件的常量传播入口。
pub fn PropagateConstant(
    ctx: Box<dyn exprctx::ExprContext>,
    filter: Option<VaildConstantPropagationExpressionFuncType>,
    conditions: Vec<Box<dyn Expression>>,
) -> Vec<Box<dyn Expression>> {
    if conditions.is_empty() {
        return conditions;
    }
    let mut solver = newPropConstSolver();
    let inner: Rc<dyn exprctx::ExprContext> = Rc::from(ctx);
    let ctx: Rc<dyn exprctx::ExprContext> = Rc::new(ConstantPropagateContext { inner });
    let result = solver.PropagateConstant(ctx, false, None, None, filter, conditions);
    solver.Clear();
    result
}

/// 借用表达式上下文的常量传播入口；算法与拥有型 `PropagateConstant` 完全相同。
pub fn PropagateConstantRef<'a>(
    ctx: &'a dyn exprctx::ExprContext,
    filter: Option<VaildConstantPropagationExpressionFuncType>,
    conditions: Vec<Box<dyn Expression>>,
) -> Vec<Box<dyn Expression>> {
    if conditions.is_empty() {
        return conditions;
    }
    let mut solver = newPropConstSolver();
    let ctx: Rc<dyn exprctx::ExprContext + 'a> = Rc::new(BorrowedExprContext { inner: ctx });
    let result = solver.PropagateConstant(ctx, false, None, None, filter, conditions);
    solver.Clear();
    result
}

/// 外连接常量传播求解器：分别维护 join/filter 条件与 outer/inner schema。
struct PropOuterJoinConstSolver<'a> {
    base: BasePropConstSolver<'a>,
    join_conds: Vec<Box<dyn Expression>>,
    filter_conds: Vec<Box<dyn Expression>>,
    outer_schema: Schema,
    inner_schema: Schema,
    vaild_expr_func: Option<VaildConstantPropagationExpressionFuncType>,
    null_sensitive: bool,
}

fn newPropOuterJoinConstSolver<'a>() -> PropOuterJoinConstSolver<'a> {
    PropOuterJoinConstSolver {
        base: newBasePropConstSolver(),
        join_conds: Vec::with_capacity(4),
        filter_conds: Vec::with_capacity(4),
        outer_schema: NewSchema(Vec::new()),
        inner_schema: NewSchema(Vec::new()),
        vaild_expr_func: None,
        null_sensitive: false,
    }
}

impl PropOuterJoinConstSolver<'_> {
    fn Clear(&mut self) {
        self.base.Clear();
        self.join_conds.clear();
        self.filter_conds.clear();
        self.outer_schema = NewSchema(Vec::new());
        self.inner_schema = NewSchema(Vec::new());
        self.null_sensitive = false;
        self.vaild_expr_func = None;
    }
    fn setConds2ConstFalse(&mut self, filter_conditions: bool) {
        self.join_conds.clear();
        self.join_conds.push(false_constant());
        if filter_conditions {
            self.filter_conds.clear();
            self.filter_conds.push(false_constant());
        }
    }
    fn pickEQCondsOnOuterCol(
        &mut self,
        result: &mut HashMap<usize, Constant>,
        visited: &mut [bool],
        filter: bool,
    ) -> bool {
        let offset = if filter { 0 } else { self.filter_conds.len() };
        let length = if filter {
            self.filter_conds.len()
        } else {
            self.join_conds.len()
        };
        for index in 0..length {
            if visited[index + offset] {
                continue;
            }
            let condition = if filter {
                &self.filter_conds[index]
            } else {
                &self.join_conds[index]
            };
            let Some((column, constant)) = validEqualCond(
                self.base.ctx.as_ref().unwrap().GetEvalCtx(),
                condition.as_ref(),
            ) else {
                if let Some(value) = condition.as_constant() {
                    visited[index + offset] = true;
                    let single = CNFExprs(vec![Box::new(value.Clone())]);
                    let evaluated = match EvalBool(
                        self.base.ctx.as_ref().unwrap().GetEvalCtx(),
                        &single,
                        chunk::Row::default(),
                    ) {
                        Ok((value, _)) => value,
                        Err(error) => {
                            terror::Log(error);
                            return false;
                        }
                    };
                    if !evaluated {
                        self.setConds2ConstFalse(filter);
                        return false;
                    }
                }
                continue;
            };
            let Some(constant) = self.base.dealWithPossibleHybridType(&column, &constant) else {
                continue;
            };
            // 外连接只提取 preserved/outer 一侧的 column=constant，禁止反向固定 inner 列的可空性。
            if !self.outer_schema.Contains(&column) {
                continue;
            }
            visited[index + offset] = true;
            let (updated, conflict) = self.base.tryToUpdateEQList(&column, &constant);
            if conflict {
                self.setConds2ConstFalse(filter);
                return false;
            }
            if updated {
                result.insert(self.base.getColID(&column), constant);
            }
        }
        true
    }
    fn pickNewEQConds(&mut self, visited: &mut [bool]) -> Option<HashMap<usize, Constant>> {
        let mut result = HashMap::new();
        if !self.pickEQCondsOnOuterCol(&mut result, visited, true) {
            return None;
        }
        if !self.pickEQCondsOnOuterCol(&mut result, visited, false) {
            return None;
        }
        Some(result)
    }
    fn propagateConstantEQ(&mut self) {
        self.base.eq_mapper.clear();
        let filter_len = self.filter_conds.len();
        let mut visited = vec![false; filter_len + self.join_conds.len()];
        for _ in 0..unsafe { MaxPropagateColsCnt } {
            let Some(mapper) = self.pickNewEQConds(&mut visited) else {
                return;
            };
            if mapper.is_empty() {
                return;
            }
            let columns = mapper
                .keys()
                .map(|id| self.base.columns[*id].clone())
                .collect();
            let constants = mapper
                .values()
                .map(|c| Box::new(c.Clone()) as Box<dyn Expression>)
                .collect::<Vec<_>>();
            let schema = NewSchema(columns);
            for (index, condition) in self.join_conds.iter_mut().enumerate() {
                if !visited[index + filter_len] {
                    let mut proxy = BuildContextProxy {
                        inner: self.base.ctx.as_ref().unwrap().as_ref(),
                    };
                    *condition =
                        ColumnSubstitute(&mut proxy, condition.clone(), &schema, &constants);
                }
            }
        }
    }
    fn colsFromOuterAndInner(&self, first: &Column, second: &Column) -> Option<(Column, Column)> {
        if self.outer_schema.Contains(first) && self.inner_schema.Contains(second) {
            Some((first.clone(), second.clone()))
        } else if self.outer_schema.Contains(second) && self.inner_schema.Contains(first) {
            Some((second.clone(), first.clone()))
        } else {
            None
        }
    }
    fn validColEqualCond(&self, condition: &dyn Expression) -> Option<(Column, Column)> {
        let function = condition.as_scalar_function()?;
        if function.FuncName.L != ast::EQ {
            return None;
        }
        let left = function.GetArgs()[0].as_column()?;
        let right = function.GetArgs()[1].as_column()?;
        if left
            .GetType(self.base.ctx.as_ref().unwrap().GetEvalCtx())
            .GetCollate()
            != right
                .GetType(self.base.ctx.as_ref().unwrap().GetEvalCtx())
                .GetCollate()
        {
            return None;
        }
        self.colsFromOuterAndInner(left, right)
    }
    fn deriveConds(
        &mut self,
        outer: &Column,
        inner: &Column,
        schema: &Schema,
        filter_offset: usize,
        visited: &mut [bool],
        filter: bool,
    ) {
        let (offset, length) = if filter {
            (filter_offset, self.filter_conds.len())
        } else {
            (0, filter_offset)
        };
        for index in 0..length {
            if visited[index + offset] {
                continue;
            }
            let condition = if filter {
                &self.filter_conds[index]
            } else {
                &self.join_conds[index]
            };
            if !ExprFromSchema(condition.as_ref(), schema) {
                visited[index + offset] = true;
                continue;
            }
            // WHERE 条件只有完全来自 outer/preserved 侧时，才可派生到 inner 侧。
            if filter && !ExprFromSchema(condition.as_ref(), &self.outer_schema) {
                continue;
            }
            let (replaced, _, derived) = tryToReplaceCond(
                self.base.ctx.as_ref().unwrap().as_ref(),
                outer,
                inner,
                condition.clone(),
                true,
            );
            if replaced
                && (isConstant(derived.as_ref())
                    || self.vaild_expr_func.map_or(true, |f| f(derived.as_ref())))
            {
                self.join_conds.push(derived);
            }
        }
    }
    fn propagateColumnEQ(&mut self) {
        if self.null_sensitive {
            return;
        }
        self.base.union_set.GrowNewIntSet(self.base.columns.len());
        let mut visited = vec![false; self.join_conds.len() * 2 + self.filter_conds.len()];
        for index in 0..self.join_conds.len() {
            if let Some((outer, inner)) = self.validColEqualCond(self.join_conds[index].as_ref()) {
                self.base
                    .union_set
                    .Union(self.base.getColID(&outer), self.base.getColID(&inner));
                visited[index] = true;
                let Some(child) = self.inner_schema.RetrieveColumn(&inner).cloned() else {
                    continue;
                };
                if !mysql::HasNotNullFlag(child.GetStaticType().GetFlag()) {
                    // 普通 outer join 可由 outer=inner 推导 inner IS NOT NULL；null-sensitive join 已在入口整体禁用。
                    let mut proxy = BuildContextProxy {
                        inner: self.base.ctx.as_ref().unwrap().as_ref(),
                    };
                    self.join_conds
                        .push(BuildNotNullExpr(&mut proxy, Box::new(child)));
                }
            }
        }
        let join_len = self.join_conds.len();
        let merged = MergeSchema(Some(&self.outer_schema), Some(&self.inner_schema)).unwrap();
        for i in 0..self.base.columns.len() {
            for j in i + 1..self.base.columns.len() {
                if self.base.union_set.FindRoot(i) != self.base.union_set.FindRoot(j) {
                    continue;
                }
                if let Some((outer, inner)) =
                    self.colsFromOuterAndInner(&self.base.columns[i], &self.base.columns[j])
                {
                    self.deriveConds(&outer, &inner, &merged, join_len, &mut visited, false);
                    self.deriveConds(&outer, &inner, &merged, join_len, &mut visited, true);
                }
            }
        }
    }
    fn solve(
        &mut self,
        keep_join_key: bool,
        join: Vec<Box<dyn Expression>>,
        filter: Vec<Box<dyn Expression>>,
    ) -> (Vec<Box<dyn Expression>>, Vec<Box<dyn Expression>>) {
        let original_join = join.clone();
        let original_filter = filter.clone();
        let keys = if keep_join_key {
            cloneJoinKeys(&join, Some(&self.outer_schema), Some(&self.inner_schema))
        } else {
            Vec::new()
        };
        self.extractColumns(join, filter);
        if self.base.columns.len() > unsafe { MaxPropagateColsCnt } {
            return (original_join, original_filter);
        }
        self.propagateConstantEQ();
        self.propagateColumnEQ();
        self.join_conds = propagateConstantDNF(
            self.base.ctx.as_ref().unwrap().clone(),
            self.vaild_expr_func,
            std::mem::take(&mut self.join_conds),
        );
        self.join_conds.extend(keys);
        self.join_conds = RemoveDupExprs(std::mem::take(&mut self.join_conds));
        self.filter_conds = propagateConstantDNF(
            self.base.ctx.as_ref().unwrap().clone(),
            self.vaild_expr_func,
            std::mem::take(&mut self.filter_conds),
        );
        (self.join_conds.clone(), self.filter_conds.clone())
    }
    fn extractColumns(&mut self, join: Vec<Box<dyn Expression>>, filter: Vec<Box<dyn Expression>>) {
        let mut map = GetUniqueIDToColumnMap();
        self.base
            .extractColumnsInternal(&mut map, &mut self.join_conds, join);
        self.base
            .extractColumnsInternal(&mut map, &mut self.filter_conds, filter);
        PutUniqueIDToColumnMap(map);
    }
}

/// 对 OR（DNF）各分支分别做 CNF 传播后再重新组合。
fn propagateConstantDNF<'a>(
    ctx: Rc<dyn exprctx::ExprContext + 'a>,
    filter: Option<VaildConstantPropagationExpressionFuncType>,
    mut conditions: Vec<Box<dyn Expression>>,
) -> Vec<Box<dyn Expression>> {
    for condition in &mut conditions {
        if condition
            .as_scalar_function()
            .is_some_and(|f| f.FuncName.L == ast::LogicOr)
        {
            let items = SplitDNFItems(condition.as_ref())
                .into_iter()
                .map(|item| {
                    let mut solver = newPropConstSolver();
                    let nested_ctx: Rc<dyn exprctx::ExprContext + 'a> =
                        Rc::new(ConstantPropagateContext { inner: ctx.clone() });
                    let cnf =
                        solver.PropagateConstant(nested_ctx, false, None, None, filter, vec![item]);
                    ComposeCNFCondition(ctx.as_ref(), &cnf).unwrap_or_else(false_constant)
                })
                .collect::<Vec<_>>();
            *condition = ComposeDNFCondition(ctx.as_ref(), &items).unwrap_or_else(false_constant);
        }
    }
    conditions
}

/// 外连接公共入口：配置 outer/inner schema 与 null_sensitive 后求解。
pub fn PropConstForOuterJoin(
    ctx: Box<dyn exprctx::ExprContext>,
    join: Vec<Box<dyn Expression>>,
    filter: Vec<Box<dyn Expression>>,
    outer_schema: Schema,
    inner_schema: Schema,
    keep_join_key: bool,
    null_sensitive: bool,
    valid: Option<VaildConstantPropagationExpressionFuncType>,
) -> (Vec<Box<dyn Expression>>, Vec<Box<dyn Expression>>) {
    let mut solver = newPropOuterJoinConstSolver();
    solver.outer_schema = outer_schema;
    solver.inner_schema = inner_schema;
    solver.null_sensitive = null_sensitive;
    solver.base.ctx = Some(Rc::from(ctx));
    solver.vaild_expr_func = valid;
    let result = solver.solve(keep_join_key, join, filter);
    solver.Clear();
    result
}

/// 常量传播求解器接口，对齐 Go PropagateConstantSolver。
pub trait PropagateConstantSolver<'a> {
    fn PropagateConstant(
        &mut self,
        ctx: Rc<dyn exprctx::ExprContext + 'a>,
        keep_join_key: bool,
        schema1: Option<Schema>,
        schema2: Option<Schema>,
        filter: Option<VaildConstantPropagationExpressionFuncType>,
        conditions: Vec<Box<dyn Expression>>,
    ) -> Vec<Box<dyn Expression>>;
    fn Clear(&mut self);
}

/// 克隆仍应保留的跨 schema 等值连接键，避免传播后丢失 join 条件。
fn cloneJoinKeys(
    expressions: &[Box<dyn Expression>],
    schema1: Option<&Schema>,
    schema2: Option<&Schema>,
) -> Vec<Box<dyn Expression>> {
    let (Some(schema1), Some(schema2)) = (schema1, schema2) else {
        return Vec::new();
    };
    expressions
        .iter()
        .filter(|expr| isJoinKey(expr.as_ref(), schema1, schema2))
        .map(|expr| expr.clone())
        .collect()
}

/// 判断是否为两侧 schema 各取一列的等值连接键。
fn isJoinKey(expression: &dyn Expression, schema1: &Schema, schema2: &Schema) -> bool {
    let Some(function) = expression.as_scalar_function() else {
        return false;
    };
    if function.FuncName.L != ast::EQ {
        return false;
    }
    let (Some(first), Some(second)) = (
        function.GetArgs()[0].as_column(),
        function.GetArgs()[1].as_column(),
    ) else {
        return false;
    };
    (schema1.Contains(first) && schema2.Contains(second))
        || (schema1.Contains(second) && schema2.Contains(first))
}
