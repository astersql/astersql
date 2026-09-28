// Copyright 2026 AsterSQL.
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

// 表达式工具函数集合：列抽取、列替换、NOT 下推、DNF/CNF 过滤提取、
// 常量/副作用判定、计划缓存相关裁剪，以及 digest 回填与二进制协议参数解析等。
//
// 本模块对应 Go `pkg/expression/util.go`，为优化器与执行器提供共享的表达式树操作原语。

use std::collections::{HashMap, HashSet};

use crate::*;

type Expr = Box<dyn Expression>;
type ColumnFilter = fn(&Column) -> bool;

/// ColumnSubstitute 使用的写时复制参数切片：直到首个子项变化才克隆整组参数。
pub struct cowExprRef<'a> {
    reference: &'a [Expr],
    changed: Option<Vec<Expr>>,
}

impl<'a> cowExprRef<'a> {
    /// 若参数实际变化则克隆切片后再写入；未变化时保持借用。
    pub fn Set(&mut self, index: usize, changed: bool, value: Expr) {
        if let Some(values) = &mut self.changed {
            values[index] = value;
            return;
        }
        if changed {
            let mut values = self
                .reference
                .iter()
                .map(|expr| expr.clone())
                .collect::<Vec<_>>();
            values[index] = value;
            self.changed = Some(values);
        }
    }
    /// 返回最终参数列表：有克隆则用克隆，否则从引用拷贝。
    pub fn Result(&self) -> Vec<Expr> {
        self.changed.as_ref().map_or_else(
            || self.reference.iter().map(|e| e.clone()).collect(),
            |v| v.iter().map(|e| e.clone()).collect(),
        )
    }
}

/// 将 `input` 中满足谓词的表达式追加到 `result`，保持相对顺序。
pub fn Filter(
    mut result: Vec<Expr>,
    input: &[Expr],
    filter: fn(&dyn Expression) -> bool,
) -> Vec<Expr> {
    result.extend(
        input
            .iter()
            .filter(|expr| filter(expr.as_ref()))
            .map(|expr| expr.clone()),
    );
    result
}

/// 从后向前原地移除命中项，filtered_out 的顺序因此与 Go 一样是逆序发现顺序。
pub fn FilterOutInPlace(
    mut input: Vec<Expr>,
    filter: fn(&dyn Expression) -> bool,
) -> (Vec<Expr>, Vec<Expr>) {
    let mut filtered = Vec::new();
    for index in (0..input.len()).rev() {
        if filter(input[index].as_ref()) {
            filtered.push(input.remove(index));
        }
    }
    (input, filtered)
}

/// 抽取表达式依赖的物理列，并递归展开虚拟生成列的 `VirtualExpr`。
pub fn ExtractDependentColumns(expr: &dyn Expression) -> Vec<&Column> {
    extractDependentColumns(Vec::with_capacity(8), expr)
}
/// `ExtractDependentColumns` 的递归实现，向 `result` 累积列引用。
pub fn extractDependentColumns<'a>(
    mut result: Vec<&'a Column>,
    expr: &'a dyn Expression,
) -> Vec<&'a Column> {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        result.push(column);
        // 虚拟生成列：继续展开其定义表达式上的依赖列。
        if let Some(virtual_expr) = &column.VirtualExpr {
            result = extractDependentColumns(result, virtual_expr.as_ref());
        }
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            result = extractDependentColumns(result, arg.as_ref());
        }
    }
    result
}

/// 按 UniqueID 去重并排序，消除 Go map 遍历的不稳定性。
pub fn ExtractColumns(expr: &dyn Expression) -> Vec<&Column> {
    let mut map = HashMap::with_capacity(8);
    extractColumns(&mut map, expr, None);
    let mut result = map.into_values().collect::<Vec<_>>();
    result.sort_by_key(|column| column.UniqueID);
    result
}
/// 抽取表达式中的关联列（CorrelatedColumn，外层查询列在子查询中的引用）。
pub fn ExtractCorColumns(expr: &dyn Expression) -> Vec<&CorrelatedColumn> {
    if let Some(column) = expr.as_any().downcast_ref::<CorrelatedColumn>() {
        return vec![column];
    }
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .map_or_else(Vec::new, |function| {
            function
                .GetArgs()
                .iter()
                .flat_map(|arg| ExtractCorColumns(arg.as_ref()))
                .collect()
        })
}
/// 从多表达式抽取列，按 UniqueID 去重后排序；可选 `filter` 过滤列。
pub fn ExtractColumnsFromExpressions(exprs: &[Expr], filter: Option<ColumnFilter>) -> Vec<&Column> {
    let mut map = HashMap::with_capacity(exprs.len());
    for expr in exprs {
        extractColumns(&mut map, expr.as_ref(), filter);
    }
    let mut result = map.into_values().collect::<Vec<_>>();
    result.sort_by_key(|column| column.UniqueID);
    result
}
/// 从多表达式抽取列为 UniqueID→Column 映射（不去重排序，保留 map）。
pub fn ExtractColumnsMapFromExpressions(
    exprs: &[Expr],
    filter: Option<ColumnFilter>,
) -> HashMap<i64, &Column> {
    let mut map = HashMap::with_capacity(exprs.len());
    for expr in exprs {
        extractColumns(&mut map, expr.as_ref(), filter);
    }
    map
}

// Column 包含非 Send 的表达式节点；使用线程局部池保持复用，同时避免把节点跨线程搬运。
std::thread_local! {
    pub static uniqueIDToColumnMapPool: std::cell::RefCell<Vec<HashMap<i64, Column>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}
/// 从线程局部池取出可复用的 UniqueID→Column 映射，避免频繁分配。
pub fn GetUniqueIDToColumnMap() -> HashMap<i64, Column> {
    uniqueIDToColumnMapPool.with(|pool| pool.borrow_mut().pop().unwrap_or_default())
}
/// 清空后归还映射到线程局部池。
pub fn PutUniqueIDToColumnMap(mut map: HashMap<i64, Column>) {
    map.clear();
    uniqueIDToColumnMapPool.with(|pool| pool.borrow_mut().push(map));
}

/// 复用调用方提供的 map，向其中写入抽取到的列引用。
pub fn ExtractColumnsMapFromExpressionsWithReusedMap<'a>(
    map: &mut HashMap<i64, &'a Column>,
    filter: Option<ColumnFilter>,
    exprs: &'a [Expr],
) {
    for expr in exprs {
        extractColumns(map, expr.as_ref(), filter);
    }
}
/// 用切片累积列引用，结束后按 UniqueID 排序并去重。
pub fn ExtractAllColumnsFromExpressionsInUsedSlices<'a>(
    mut reuse: Vec<&'a Column>,
    filter: Option<ColumnFilter>,
    exprs: &'a [Expr],
) -> Vec<&'a Column> {
    for expr in exprs {
        reuse = extractColumnsSlices(reuse, expr.as_ref(), filter);
    }
    reuse.sort_by_key(|column| column.UniqueID);
    reuse.dedup_by_key(|column| column.UniqueID);
    reuse
}
/// 抽取全部列引用到新 Vec（可能含重复，调用方可再处理）。
pub fn ExtractAllColumnsFromExpressions<'a>(
    exprs: &'a [Expr],
    filter: Option<ColumnFilter>,
) -> Vec<&'a Column> {
    exprs.iter().fold(Vec::with_capacity(8), |result, expr| {
        extractColumnsSlices(result, expr.as_ref(), filter)
    })
}
/// 将表达式中的列 UniqueID 写入 `FastIntSet`。
pub fn ExtractColumnsSetFromExpressions(
    set: &mut intset::FastIntSet,
    filter: Option<ColumnFilter>,
    exprs: &[Expr],
) {
    for expr in exprs {
        extractColumnsSet(set, expr.as_ref(), filter);
    }
}

/// 递归把列写入 HashMap；遇到标量函数则遍历参数。
pub fn extractColumns<'a>(
    result: &mut HashMap<i64, &'a Column>,
    expr: &'a dyn Expression,
    filter: Option<ColumnFilter>,
) {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if filter.is_none_or(|f| f(column)) {
            result.insert(column.UniqueID, column);
        }
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            extractColumns(result, arg.as_ref(), filter);
        }
    }
}
/// 递归把列追加到 Vec（允许重复）。
pub fn extractColumnsSlices<'a>(
    mut result: Vec<&'a Column>,
    expr: &'a dyn Expression,
    filter: Option<ColumnFilter>,
) -> Vec<&'a Column> {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if filter.is_none_or(|f| f(column)) {
            result.push(column);
        }
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            result = extractColumnsSlices(result, arg.as_ref(), filter);
        }
    }
    result
}
/// 递归把列 UniqueID 插入整数集合。
pub fn extractColumnsSet(
    result: &mut intset::FastIntSet,
    expr: &dyn Expression,
    filter: Option<ColumnFilter>,
) {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if filter.is_none_or(|f| f(column)) {
            result.Insert(column.UniqueID as i32);
        }
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            extractColumnsSet(result, arg.as_ref(), filter);
        }
    }
}

/// 从等式条件中收集彼此等价的列/表达式分组。
pub fn ExtractEquivalenceColumns(mut result: Vec<Vec<Expr>>, exprs: &[Expr]) -> Vec<Vec<Expr>> {
    for expr in exprs {
        result = extractEquivalenceColumns(result, expr.as_ref());
    }
    result
}

/// 只接受 column < int64 或 column <= int64；严格小于时把上界减一。
pub fn FindUpperBound(expr: &dyn Expression) -> (Option<&Column>, i64) {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return (None, 0);
    };
    let args = function.GetArgs();
    if args.len() != 2 || !matches!(function.FuncName.L.as_str(), ast::LT | ast::LE) {
        return (None, 0);
    }
    let Some(column) = args[0].as_any().downcast_ref::<Column>() else {
        return (None, 0);
    };
    let Some(constant) = args[1].as_any().downcast_ref::<Constant>() else {
        return (None, 0);
    };
    if constant.Value.Kind() != types::KindInt64 {
        return (None, 0);
    }
    let value = constant.Value.GetInt64();
    (
        Some(column),
        if function.FuncName.L == ast::LT {
            value.wrapping_sub(1)
        } else {
            value
        },
    )
}

/// 处理单个表达式，识别等式并把两端加入等价组。
pub fn extractEquivalenceColumns(
    mut result: Vec<Vec<Expr>>,
    expr: &dyn Expression,
) -> Vec<Vec<Expr>> {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return result;
    };
    let args = function.GetArgs();
    let eligible = matches!(function.FuncName.L.as_str(), ast::EQ | ast::NullEQ)
        || (function.FuncName.L == ast::In && args.len() == 2);
    if eligible && args.len() == 2 {
        let left_ok = args[0].as_any().is::<Column>() || args[0].as_any().is::<ScalarFunction>();
        let right_ok = args[1].as_any().is::<Column>() || args[1].as_any().is::<ScalarFunction>();
        if left_ok
            && right_ok
            && (args[0].as_any().is::<Column>() || args[1].as_any().is::<Column>())
        {
            result.push(vec![args[0].clone(), args[1].clone()]);
        }
    }
    result
}

/// 同时抽取普通列与关联列。
pub fn extractColumnsAndCorColumns<'a>(
    mut result: Vec<&'a Column>,
    expr: &'a dyn Expression,
) -> Vec<&'a Column> {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        result.push(column);
    } else if let Some(column) = expr.as_any().downcast_ref::<CorrelatedColumn>() {
        result.push(&column.column);
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            result = extractColumnsAndCorColumns(result, arg.as_ref());
        }
    }
    result
}

/// 抽取形如 `col = const/scalar` 的等式一侧列或标量。
pub fn ExtractConstantEqColumnsOrScalar(
    ctx: &dyn BuildContext,
    mut result: Vec<Expr>,
    exprs: &[Expr],
) -> Vec<Expr> {
    for expr in exprs {
        result = extractConstantEqColumnsOrScalar(ctx, result, expr.as_ref());
    }
    result
}
/// `ExtractConstantEqColumnsOrScalar` 的递归实现。
pub fn extractConstantEqColumnsOrScalar(
    ctx: &dyn BuildContext,
    mut result: Vec<Expr>,
    expr: &dyn Expression,
) -> Vec<Expr> {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return result;
    };
    let args = function.GetArgs();
    if matches!(function.FuncName.L.as_str(), ast::EQ | ast::NullEQ) && args.len() == 2 {
        for (candidate, other) in [(&args[0], &args[1]), (&args[1], &args[0])] {
            let lhs =
                candidate.as_any().is::<Column>() || candidate.as_any().is::<ScalarFunction>();
            let rhs = other.as_any().is::<Constant>() || other.as_any().is::<CorrelatedColumn>();
            if lhs && rhs {
                result.push((*candidate).clone());
            }
        }
    } else if function.FuncName.L == ast::In && args.len() > 1 {
        let guard = &args[1];
        let all_same = args[1..].iter().all(|arg| {
            arg.as_any().is::<Constant>() && guard.Equal(ctx.GetEvalCtx(), arg.as_ref())
        });
        if all_same && (args[0].as_any().is::<Column>() || args[0].as_any().is::<ScalarFunction>())
        {
            result.push(args[0].clone());
        }
    }
    result
}
/// 对表达式切片批量抽取列与关联列。
pub fn ExtractColumnsAndCorColumnsFromExpressions<'a>(
    mut result: Vec<&'a Column>,
    list: &'a [Expr],
) -> Vec<&'a Column> {
    for expr in list {
        result = extractColumnsAndCorColumns(result, expr.as_ref());
    }
    result
}
/// 构造包含全部相关列 UniqueID 的 `FastIntSet`。
pub fn ExtractColumnSet(exprs: &[Expr]) -> intset::FastIntSet {
    let mut set = intset::NewFastIntSet(Vec::new());
    for expr in exprs {
        extractColumnSet(expr.as_ref(), &mut set);
    }
    set
}
/// 递归向集合插入列 UniqueID。
pub fn extractColumnSet(expr: &dyn Expression, set: &mut intset::FastIntSet) {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        set.Insert(column.UniqueID as i32);
    } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        for arg in function.GetArgs() {
            extractColumnSet(arg.as_ref(), set);
        }
    }
}

/// 克隆列后设置 InOperand；标量函数递归改写参数并清理两类哈希缓存。
pub fn SetExprColumnInOperand(expr: Expr) -> Expr {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        let mut cloned = column.CloneColumn();
        cloned.InOperand = true;
        return Box::new(cloned);
    }
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let mut function = function.clone_scalar();
        for arg in function.GetArgsMut() {
            *arg = SetExprColumnInOperand(arg.clone());
        }
        function.CleanHashCode();
        return Box::new(function);
    }
    expr
}

/// 按模式列映射替换表达式中的列，默认不强制全部命中。
pub fn ColumnSubstitute(
    ctx: &dyn BuildContext,
    expr: Expr,
    schema: &Schema,
    replacements: &[Expr],
) -> Expr {
    ColumnSubstituteImpl(ctx, expr, schema, replacements, false).2
}
/// 强制替换全部匹配列；失败时回退语义与 Go 一致。
pub fn ColumnSubstituteAll(
    ctx: &dyn BuildContext,
    expr: Expr,
    schema: &Schema,
    replacements: &[Expr],
) -> (bool, Expr) {
    let (_, failed, result) = ColumnSubstituteImpl(ctx, expr, schema, replacements, true);
    (failed, result)
}

/// 递归替换模式列，并保持 Go 实现的排序规则、强制性和失败回退语义。
pub fn ColumnSubstituteImpl(
    ctx: &dyn BuildContext,
    expr: Expr,
    schema: &Schema,
    replacements: &[Expr],
    fail_fast: bool,
) -> (bool, bool, Expr) {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        let Some(index) = schema.ColumnIndex(column) else {
            return (false, false, expr);
        };
        let replacement = replacements[index].clone();
        return (
            true,
            false,
            if column.InOperand {
                SetExprColumnInOperand(replacement)
            } else {
                replacement
            },
        );
    }
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return (false, false, expr);
    };
    if matches!(function.FuncName.L.as_str(), ast::Cast | ast::Grouping) {
        let (changed, failed, new_arg) = ColumnSubstituteImpl(
            ctx,
            function.GetArgs()[0].clone(),
            schema,
            replacements,
            fail_fast,
        );
        if failed && fail_fast {
            return (changed, failed, expr);
        }
        if !changed {
            return (false, false, expr);
        }
        let old_flag = function.GetStaticType().GetFlag();
        let old_coercibility = function.Coercibility();
        let mut rebuilt = if function.FuncName.L == ast::Cast {
            match BuildCastFunctionWithCheck(
                ctx,
                new_arg,
                function.GetStaticType().clone(),
                false,
                function.IsExplicitCharset(),
            ) {
                Ok(value) => value,
                Err(error) => {
                    terror::Log(error);
                    return (true, true, expr);
                }
            }
        } else {
            let mut cloned = function.clone_scalar();
            cloned.GetArgsMut()[0] = new_arg;
            cloned.CleanHashCode();
            Box::new(cloned) as Expr
        };
        rebuilt.SetCoercibility(old_coercibility);
        rebuilt.GetTypeMut().SetFlag(old_flag);
        return (true, false, rebuilt);
    }
    if ctx.IsConstantPropagateCheck() && function.FuncName.L == ast::Length {
        if let Some(column) = function.GetArgs()[0].as_any().downcast_ref::<Column>() {
            if let Some(index) = schema.ColumnIndex(column) {
                if replacements[index].as_any().is::<Constant>()
                    && matches!(
                        schema.Columns[index].GetStaticType().GetCollate(),
                        charset::CollationUTF8MB4 | charset::CollationUTF8
                    )
                {
                    return (false, false, expr);
                }
            }
        }
    }
    let function_args = function
        .GetArgs()
        .iter()
        .map(|arg| arg.as_ref())
        .collect::<Vec<_>>();
    let old_collation = match CheckAndDeriveCollationFromExprs(
        ctx,
        &function.FuncName.L,
        function.GetStaticType().EvalType(),
        &function_args,
    ) {
        Ok(collation) => collation,
        Err(error) => {
            logutil::BgLogger().warn(format!(
                "Unexpected error happened during ColumnSubstitution: {error}"
            ));
            return (false, false, expr);
        }
    };
    let mut args = cowExprRef {
        reference: function.GetArgs(),
        changed: None,
    };
    let mut substituted = false;
    let mut has_fail = false;
    for index in 0..function.GetArgs().len() {
        let original = function.GetArgs()[index].clone();
        let (mut changed, failed, replacement) =
            ColumnSubstituteImpl(ctx, original.clone(), schema, replacements, fail_fast);
        if fail_fast && failed {
            return (changed, true, expr);
        }
        let old_changed = changed;
        if changed && collate::NewCollationEnabled() {
            let mut trial = args.Result();
            trial[index] = replacement.clone();
            let trial_args = trial.iter().map(|arg| arg.as_ref()).collect::<Vec<_>>();
            let new_collation = match CheckAndDeriveCollationFromExprs(
                ctx,
                &function.FuncName.L,
                function.GetStaticType().EvalType(),
                &trial_args,
            ) {
                Ok(collation) => collation,
                Err(_) => return (false, true, expr),
            };
            changed = false;
            if old_collation.Collation == new_collation.Collation {
                let same_argument_collation = replacement.GetType(ctx.GetEvalCtx()).GetCollate()
                    == original.GetType(ctx.GetEvalCtx()).GetCollate()
                    && replacement.Coercibility() == original.Coercibility();
                changed = same_argument_collation
                    || checkCollationStrictness(
                        &old_collation.Collation,
                        replacement.GetType(ctx.GetEvalCtx()).GetCollate(),
                    );
            }
        }
        has_fail |= failed || old_changed != changed;
        if fail_fast && old_changed != changed {
            return (changed, true, expr);
        }
        args.Set(index, changed, replacement);
        substituted |= changed;
    }
    if !substituted {
        return (false, false, expr);
    }
    let mut values = args.Result();
    if function.FuncName.L == ast::EQ && values[0].as_any().is::<Constant>() {
        values.swap(0, 1);
    }
    match NewFunction(
        ctx,
        &function.FuncName.L,
        function.GetStaticType().clone(),
        values,
    ) {
        Ok(rebuilt) => (true, has_fail, rebuilt),
        Err(_) => (true, true, expr),
    }
}

/// 判断新校对规则是否不弱于原校对（严格性检查）。
pub fn checkCollationStrictness(collation: &str, new_collation: &str) -> bool {
    let Some(&old_group) = CollationStrictnessGroup.get(collation) else {
        return false;
    };
    let Some(&new_group) = CollationStrictnessGroup.get(new_collation) else {
        return false;
    };
    old_group == new_group || CollationStrictness[&old_group].contains(&new_group)
}

/// 返回可按 2..=36 进制解析的最长前缀，并移除合法的前导加号。
pub fn getValidPrefix(text: &str, base: i64) -> &str {
    if !(2..=36).contains(&base) {
        return "";
    }
    let mut end = 0;
    for (index, byte) in text.bytes().enumerate() {
        if index == 0 && matches!(byte, b'+' | b'-') {
            continue;
        }
        let Some(digit) = (byte as char).to_digit(base as u32) else {
            break;
        };
        let _ = digit;
        end = index + 1;
    }
    if end > 1 && text.starts_with('+') {
        &text[1..end]
    } else {
        &text[..end]
    }
}

/// 把关联列替换为其 Datum；函数全部参数成为常量时直接求值折叠，否则按原函数类型重建。
pub fn SubstituteCorCol2Constant(ctx: &mut dyn BuildContext, expr: Expr) -> Result<Expr, Error> {
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let mut all_constant = true;
        let mut args = Vec::with_capacity(function.GetArgs().len());
        for arg in function.GetArgs() {
            let next = SubstituteCorCol2Constant(ctx, arg.clone())?;
            all_constant &= next.as_any().is::<Constant>();
            args.push(next);
        }
        if all_constant {
            return Ok(Box::new(Constant::with_type(
                function.Eval(ctx.GetEvalCtx(), chunk::Row::default())?,
                function.GetStaticType().clone(),
            )));
        }
        if function.FuncName.L == ast::Cast {
            let argument = args.remove(0);
            return Ok(BuildCastFunction(ctx, &argument, function.GetStaticType()));
        }
        if function.FuncName.L == ast::Grouping {
            let mut cloned = function.clone_scalar();
            cloned.GetArgsMut()[0] = args.remove(0);
            cloned.CleanHashCode();
            return Ok(Box::new(cloned));
        }
        return NewFunction(
            ctx,
            &function.FuncName.L,
            function.GetStaticType().clone(),
            args,
        );
    }
    if let Some(column) = expr.as_any().downcast_ref::<CorrelatedColumn>() {
        return Ok(Box::new(Constant::with_type(
            column
                .data
                .as_ref()
                .map(|data| data.read().expect("correlated datum lock poisoned").clone())
                .unwrap_or_default(),
            column.column.GetStaticType().clone(),
        )));
    }
    if let Some(constant) = expr.as_any().downcast_ref::<Constant>() {
        if constant.DeferredExpr.is_some() {
            let return_type = <Constant as Expression>::GetType(constant, ctx.GetEvalCtx()).clone();
            let folded = FoldConstant(ctx, expr);
            let value = folded
                .as_any()
                .downcast_ref::<Constant>()
                .expect("FoldConstant must return Constant")
                .Value
                .clone();
            return Ok(Box::new(Constant::with_type(value, return_type)));
        }
    }
    Ok(expr)
}

/// 按指定校对在 `text` 中定位 `substring`，返回 1-based 位置；空 needle 返回 1。
pub fn locateStringWithCollation(text: &str, substring: &str, collation: &str) -> i64 {
    let collator = collate::GetCollator(collation);
    let text_key = collator.KeyWithoutTrimRightSpace(text);
    let needle = collator.KeyWithoutTrimRightSpace(substring);
    if needle.is_empty() {
        return 1;
    }
    let Some(mut byte_index) = text_key
        .windows(needle.len())
        .position(|window| window == needle)
    else {
        return 0;
    };
    if byte_index == 0 {
        return 1;
    }
    let mut chars = 0;
    for ch in text.chars() {
        chars += 1;
        byte_index =
            byte_index.saturating_sub(collator.KeyWithoutTrimRightSpace(&ch.to_string()).len());
        if byte_index == 0 {
            return chars + 1;
        }
    }
    0
}
/// 将 `±HH:MM` 时区字符串转为秒偏移。
pub fn timeZone2int(timezone: &str) -> i32 {
    let sign = if timezone.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = timezone[1..].split_once(':').unwrap();
    sign * (hours.parse::<i32>().unwrap() * 3600 + minutes.parse::<i32>().unwrap() * 60)
}

/// 逻辑运算符函数名集合（AND/OR/XOR 等）。
pub static logicalOps: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        HashSet::from([
            ast::LT,
            ast::GE,
            ast::GT,
            ast::LE,
            ast::EQ,
            ast::NE,
            ast::UnaryNot,
            ast::Like,
            ast::LogicAnd,
            ast::LogicOr,
            ast::LogicXor,
            ast::In,
            ast::IsNull,
            ast::IsFalsity,
            ast::IsTruthWithoutNull,
            ast::IsTruthWithNull,
            ast::NullEQ,
            ast::Regexp,
        ])
    });
/// 比较/逻辑运算符到其相反运算符的映射。
pub static oppositeOp: std::sync::LazyLock<HashMap<&'static str, &'static str>> =
    std::sync::LazyLock::new(|| {
        HashMap::from([
            (ast::LT, ast::GE),
            (ast::GE, ast::LT),
            (ast::GT, ast::LE),
            (ast::LE, ast::GT),
            (ast::EQ, ast::NE),
            (ast::NE, ast::EQ),
            (ast::LogicOr, ast::LogicAnd),
            (ast::LogicAnd, ast::LogicOr),
        ])
    });
/// 对称运算符映射（交换左右参数后的等价算子）。
pub static symmetricOp: std::sync::LazyLock<HashMap<opcode::Op, opcode::Op>> =
    std::sync::LazyLock::new(|| {
        HashMap::from([
            (opcode::Op::LT, opcode::Op::GT),
            (opcode::Op::GE, opcode::Op::LE),
            (opcode::Op::GT, opcode::Op::LT),
            (opcode::Op::LE, opcode::Op::GE),
            (opcode::Op::EQ, opcode::Op::EQ),
            (opcode::Op::NE, opcode::Op::NE),
            (opcode::Op::NullEQ, opcode::Op::NullEQ),
        ])
    });
/// 比较类运算符名集合。
pub static CompareOpMap: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        HashSet::from([
            ast::LT,
            ast::GE,
            ast::GT,
            ast::LE,
            ast::EQ,
            ast::NE,
            ast::NullEQ,
            ast::In,
        ])
    });

/// 把外层 NOT 下推到函数各参数（按运算符语义改写）。
pub fn pushNotAcrossArgs(
    ctx: &mut dyn BuildContext,
    exprs: &[Expr],
    not: bool,
) -> (Vec<Expr>, bool) {
    let mut changed = false;
    let values = exprs
        .iter()
        .map(|expr| {
            let (value, current) = pushNotAcrossExpr(ctx, expr.clone(), not);
            changed |= current;
            value
        })
        .collect();
    (values, changed)
}
/// 判断 CAST 是否无精度损失，可被安全消除。
pub fn noPrecisionLossCastCompatible(cast: &types::FieldType, source: &types::FieldType) -> bool {
    if types_dependency::field::IsTypeVarchar(cast.GetType())
        && types_dependency::field::IsTypeVarchar(source.GetType())
    {
        return cast.GetFlen() >= source.GetFlen()
            && collate::CompatibleCollate(cast.GetCollate(), source.GetCollate());
    }
    if mysql::IsIntegerType(cast.GetType()) && mysql::IsIntegerType(source.GetType()) {
        let (cast_len, _) = mysql::GetDefaultFieldLengthAndDecimal(cast.GetType());
        let (source_len, _) = mysql::GetDefaultFieldLengthAndDecimal(source.GetType());
        return cast_len >= source_len
            && mysql::HasUnsignedFlag(cast.GetFlag()) == mysql::HasUnsignedFlag(source.GetFlag());
    }
    false
}
/// 在满足条件时剥掉外层 CAST，返回内层表达式。
pub fn unwrapCast(
    ctx: &mut dyn BuildContext,
    parent: &ScalarFunction,
    offset: usize,
) -> (Expr, bool) {
    let Some(cast) = parent.GetArgs()[offset]
        .as_any()
        .downcast_ref::<ScalarFunction>()
    else {
        return (Box::new(parent.clone_scalar()), false);
    };
    if cast.FuncName.L != ast::Cast {
        return (Box::new(parent.clone_scalar()), false);
    }
    let (_, parent_collation) = parent.CharsetAndCollation();
    if cast.GetStaticType().EvalType() == types::ETString
        && !collate::CompatibleCollate(cast.GetStaticType().GetCollate(), &parent_collation)
    {
        return (Box::new(parent.clone_scalar()), false);
    }
    if !parent.GetArgs()[1 - offset].as_any().is::<Constant>() {
        return (Box::new(parent.clone_scalar()), false);
    }
    let Some(column) = cast.GetArgs()[0].as_any().downcast_ref::<Column>() else {
        return (Box::new(parent.clone_scalar()), false);
    };
    if !noPrecisionLossCastCompatible(cast.GetStaticType(), column.GetStaticType()) {
        return (Box::new(parent.clone_scalar()), false);
    }
    let mut args = parent
        .GetArgs()
        .iter()
        .map(|arg| arg.clone())
        .collect::<Vec<_>>();
    args[offset] = Box::new(column.CloneColumn());
    (
        NewFunctionInternal(
            ctx,
            &parent.FuncName.L,
            parent.GetStaticType().clone(),
            args,
        )
        .unwrap(),
        true,
    )
}
/// 尝试消除无精度损失的 CAST 函数节点。
pub fn eliminateCastFunction(ctx: &mut dyn BuildContext, expr: Expr) -> (Expr, bool) {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return (expr, false);
    };
    if function.FuncName.L == ast::LogicOr || function.FuncName.L == ast::LogicAnd {
        let items = if function.FuncName.L == ast::LogicOr {
            FlattenDNFConditions(function)
        } else {
            FlattenCNFConditions(function)
        };
        let mut changed = false;
        let rewritten = items
            .into_iter()
            .map(|item| {
                let (next, current) = eliminateCastFunction(ctx, item);
                changed |= current;
                next
            })
            .collect::<Vec<_>>();
        if changed {
            return (
                if function.FuncName.L == ast::LogicOr {
                    ComposeDNFCondition(ctx, &rewritten)
                        .expect("a flattened DNF contains at least one condition")
                } else {
                    ComposeCNFCondition(ctx, &rewritten)
                        .expect("a flattened CNF contains at least one condition")
                },
                true,
            );
        }
        return (expr, false);
    }
    if matches!(
        function.FuncName.L.as_str(),
        ast::EQ | ast::NullEQ | ast::LE | ast::GE | ast::LT | ast::GT
    ) {
        let first = unwrapCast(ctx, function, 0);
        if first.1 {
            return first;
        }
        return unwrapCast(ctx, function, 1);
    }
    if function.FuncName.L == ast::In
        && function.GetArgs()[1..]
            .iter()
            .all(|arg| arg.as_any().is::<Constant>())
    {
        let Some(cast) = function.GetArgs()[0]
            .as_any()
            .downcast_ref::<ScalarFunction>()
        else {
            return (expr, false);
        };
        let (_, parent_collation) = function.CharsetAndCollation();
        if cast.FuncName.L != ast::Cast
            || cast.GetStaticType().EvalType() == types::ETString
                && !collate::CompatibleCollate(cast.GetStaticType().GetCollate(), &parent_collation)
        {
            return (expr, false);
        }
        return unwrapCast(ctx, function, 0);
    }
    (expr, false)
}

/// 为 NOT 下推给参数包裹 IS TRUE WITH NULL，保留 SQL 三值逻辑。
fn wrapWithIsTrueForPushNot(ctx: &mut dyn BuildContext, arg: Expr) -> Result<Expr, Error> {
    if arg.GetType(ctx.GetEvalCtx()).EvalType() == types::ETInt {
        if let Some(function) = arg.as_any().downcast_ref::<ScalarFunction>() {
            if logicalOps.contains(function.FuncName.L.as_str()) {
                return Ok(arg);
            }
        }
    }
    let class = isTrueOrFalseFunctionClass::new(ast::IsTruthWithNull, opcode::IsTruth, true);
    let function = class.getFunction(ctx, vec![arg])?;
    Ok(FoldConstant(
        ctx,
        Box::new(ScalarFunction {
            FuncName: ast::NewCIStr(ast::IsTruthWithNull),
            RetType: Some(function.getRetTp().clone()),
            Function: function,
            hashcode: Vec::new(),
            canonicalhashcode: Vec::new(),
        }),
    ))
}

/// 依据德摩根律下推 NOT，并用 IS TRUE WITH NULL 保留 Go 的三值逻辑。
pub fn pushNotAcrossExpr(ctx: &mut dyn BuildContext, expr: Expr, not: bool) -> (Expr, bool) {
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        if function.FuncName.L == ast::UnaryNot {
            let Ok(child) = wrapWithIsTrueForPushNot(ctx, function.GetArgs()[0].clone()) else {
                return (expr, false);
            };
            let (child, changed) = pushNotAcrossExpr(ctx, child, !not);
            if !changed && !not {
                return (expr, false);
            }
            return (child, true);
        }
        if matches!(
            function.FuncName.L.as_str(),
            ast::LT | ast::GE | ast::GT | ast::LE | ast::EQ | ast::NE
        ) {
            if not {
                return (
                    NewFunctionInternal(
                        ctx,
                        oppositeOp[function.FuncName.L.as_str()],
                        function.GetStaticType().clone(),
                        function.GetArgs().iter().map(|arg| arg.clone()).collect(),
                    )
                    .unwrap(),
                    true,
                );
            }
            let (args, child_changed) = pushNotAcrossArgs(ctx, function.GetArgs(), false);
            if child_changed {
                return (
                    NewFunctionInternal(
                        ctx,
                        &function.FuncName.L,
                        function.GetStaticType().clone(),
                        args,
                    )
                    .unwrap(),
                    true,
                );
            }
        }
        if matches!(function.FuncName.L.as_str(), ast::LogicAnd | ast::LogicOr) {
            let (args, child_changed) = pushNotAcrossArgs(ctx, function.GetArgs(), not);
            if not || child_changed {
                let name = if not {
                    oppositeOp[function.FuncName.L.as_str()]
                } else {
                    function.FuncName.L.as_str()
                };
                return (
                    NewFunctionInternal(ctx, name, function.GetStaticType().clone(), args).unwrap(),
                    true,
                );
            }
        }
    }
    if not {
        let tp = *types::NewFieldType(mysql::TypeTiny);
        return (
            NewFunctionInternal(ctx, ast::UnaryNot, tp, vec![expr]).unwrap(),
            true,
        );
    }
    (expr, false)
}
/// 取出 IS TRUE / IS TRUE WITH NULL 包装内的表达式。
pub fn GetExprInsideIsTruth(expr: Expr) -> Expr {
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        if matches!(
            function.FuncName.L.as_str(),
            ast::IsTruthWithNull | ast::IsTruthWithoutNull
        ) {
            return GetExprInsideIsTruth(function.GetArgs()[0].clone());
        }
    }
    expr
}
/// 对表达式执行 NOT 下推改写的对外入口。
pub fn PushDownNot(ctx: &mut dyn BuildContext, expr: Expr) -> Expr {
    pushNotAcrossExpr(ctx, expr, false).0
}
/// 消除整棵表达式中无精度损失的 CAST。
pub fn EliminateNoPrecisionLossCast(ctx: &mut dyn BuildContext, expr: Expr) -> Expr {
    eliminateCastFunction(ctx, expr).0
}
/// 判断表达式是否包含尚未下推的外层 NOT。
pub fn ContainOuterNot(expr: &dyn Expression) -> bool {
    containOuterNot(expr, false)
}
/// `ContainOuterNot` 的递归实现，`not` 表示当前是否处于 NOT 上下文。
pub fn containOuterNot(expr: &dyn Expression, not: bool) -> bool {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    match function.FuncName.L.as_str() {
        ast::UnaryNot => containOuterNot(function.GetArgs()[0].as_ref(), true),
        ast::IsTruthWithNull | ast::IsNull => containOuterNot(function.GetArgs()[0].as_ref(), not),
        _ if not => true,
        _ => function
            .GetArgs()
            .iter()
            .any(|arg| containOuterNot(arg.as_ref(), false)),
    }
}
/// 判断表达式切片中是否包含与 `target` 语义相等的项。
pub fn Contains(ctx: &dyn EvalContext, exprs: &[Expr], target: &dyn Expression) -> bool {
    exprs
        .iter()
        .any(|expr| std::ptr::eq(expr.as_ref(), target) || expr.Equal(ctx, target))
}

/// 从 DNF（析取范式）条件中提取各分支共有的过滤条件。
pub fn ExtractFiltersFromDNFs(ctx: &mut dyn BuildContext, mut conditions: Vec<Expr>) -> Vec<Expr> {
    let mut extracted = Vec::new();
    for index in (0..conditions.len()).rev() {
        if conditions[index]
            .as_any()
            .downcast_ref::<ScalarFunction>()
            .is_some_and(|f| f.FuncName.L == ast::LogicOr)
        {
            let function = conditions[index]
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .unwrap();
            let (items, remainder) = extractFiltersFromDNF(ctx, function);
            extracted.extend(items);
            if let Some(value) = remainder {
                conditions[index] = value;
            } else {
                conditions.remove(index);
            }
        }
    }
    conditions.extend(extracted);
    conditions
}
/// 统计每个 CNF 叶哈希在多少个 DNF 分支出现，提取每个分支共有的条件，并稳定排序输出。
pub fn extractFiltersFromDNF(
    ctx: &mut dyn BuildContext,
    dnf: &ScalarFunction,
) -> (Vec<Expr>, Option<Expr>) {
    let leaves = FlattenDNFConditions(dnf);
    let mut counts: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut values: HashMap<Vec<u8>, Expr> = HashMap::new();
    for (branch, leaf) in leaves.iter().enumerate() {
        let mut seen = HashSet::new();
        for item in SplitCNFItems(leaf.as_ref()) {
            let hash = item.HashCode().to_vec();
            if branch == 0 {
                counts.insert(hash.clone(), 1);
                values.insert(hash, item.clone());
            } else if counts.contains_key(&hash) && seen.insert(hash.clone()) {
                *counts.get_mut(&hash).unwrap() += 1;
            }
        }
    }
    values.retain(|hash, _| counts[hash] == leaves.len());
    if values.is_empty() {
        return (Vec::new(), Some(Box::new(dnf.clone_scalar())));
    }
    let hashes = values.keys().cloned().collect::<HashSet<_>>();
    let mut remainder = Vec::new();
    let mut only_extracted = false;
    for leaf in leaves {
        let items = SplitCNFItems(leaf.as_ref())
            .into_iter()
            .filter(|item| !hashes.contains(&item.HashCode()))
            .collect::<Vec<_>>();
        if items.is_empty() {
            only_extracted = true;
            break;
        }
        remainder.push(
            ComposeCNFCondition(ctx, &items)
                .expect("a non-empty CNF item list must compose to an expression"),
        );
    }
    let mut extracted = values.into_values().collect::<Vec<_>>();
    extracted.sort_by_key(|expr| expr.HashCode().to_vec());
    (
        extracted,
        if only_extracted {
            None
        } else {
            ComposeDNFCondition(ctx, &remainder)
        },
    )
}
/// 从 DNF 推导可下推的宽松过滤（可能弱于原条件但仍正确）。
pub fn DeriveRelaxedFiltersFromDNF(
    ctx: &mut dyn BuildContext,
    expr: &dyn Expression,
    schema: &Schema,
) -> Option<Expr> {
    let function = expr.as_any().downcast_ref::<ScalarFunction>()?;
    if function.FuncName.L != ast::LogicOr {
        return None;
    }
    let mut relaxed = Vec::new();
    for leaf in FlattenDNFConditions(function) {
        let mut items = Vec::new();
        for item in SplitCNFItems(leaf.as_ref()) {
            if let Some(nested) = item
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .filter(|f| f.FuncName.L == ast::LogicOr)
            {
                if let Some(value) = DeriveRelaxedFiltersFromDNF(ctx, nested, schema) {
                    items.push(value);
                }
            } else if ExprFromSchema(item.as_ref(), schema) {
                items.push(item);
            }
        }
        if items.is_empty() {
            return None;
        }
        relaxed.push(
            ComposeCNFCondition(ctx, &items)
                .expect("a non-empty relaxed CNF must compose to an expression"),
        );
    }
    ComposeDNFCondition(ctx, &relaxed)
}

/// 若表达式是 Row 构造则返回元素个数，否则为 1。
pub fn GetRowLen(expr: &dyn Expression) -> usize {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .filter(|f| f.FuncName.L == ast::RowFunc)
        .map_or(1, |f| f.GetArgs().len())
}
/// 断言参数不是多列 Row，否则返回错误。
pub fn CheckArgsNotMultiColumnRow(args: &[Expr]) -> Result<(), Error> {
    if args.iter().any(|arg| GetRowLen(arg.as_ref()) != 1) {
        Err(ErrOperandColumns.GenWithStackByArgs(1))
    } else {
        Ok(())
    }
}
/// 取标量函数第 `index` 个参数的克隆。
pub fn GetFuncArg(expr: &dyn Expression, index: usize) -> Option<Expr> {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .map(|f| f.GetArgs()[index].clone())
}
/// 弹出 Row 的第一个元素，剩余重建为 Row 或单表达式。
pub fn PopRowFirstArg(
    ctx: &mut dyn BuildContext,
    expr: &dyn Expression,
) -> Result<Option<Expr>, Error> {
    let Some(row) = expr
        .as_any()
        .downcast_ref::<ScalarFunction>()
        .filter(|f| f.FuncName.L == ast::RowFunc)
    else {
        return Ok(None);
    };
    if row.GetArgs().len() == 2 {
        return Ok(Some(row.GetArgs()[1].clone()));
    }
    Ok(Some(NewFunction(
        ctx,
        ast::RowFunc,
        row.GetStaticType().clone(),
        row.GetArgs()[1..].iter().map(|e| e.clone()).collect(),
    )?))
}
/// 由 Datum 与字段类型构造常量表达式。
pub fn DatumToConstant(datum: types::Datum, field_type: u8, flag: u64) -> Constant {
    let mut tp = types::NewFieldType(field_type);
    tp.AddFlag(flag as usize);
    Constant::with_type(datum, *tp)
}
/// 由预处理语句参数标记构造常量/参数表达式。
pub fn ParamMarkerExpression(
    ctx: &dyn BuildContext,
    marker: &driver::ParamMarkerExpr,
    need_param: bool,
) -> Result<Constant, Error> {
    let mut tp = types::NewFieldType(mysql::TypeUnspecified);
    types::InferParamTypeFromDatum(&marker.Datum, &mut tp);
    let mut constant = Constant::with_type(marker.Datum.clone(), *tp);
    constant.ParamMarker =
        (ctx.IsUseCache() || need_param).then(|| ParamMarker::new(marker.Order as usize));
    Ok(constant)
}

/// AST 访问器：检查 Prepare 中的参数标记合法性。
pub struct ParamMarkerInPrepareChecker {
    pub InPrepareStmt: bool,
}
impl ParamMarkerInPrepareChecker {
    pub fn Enter(&mut self, node: &mut ast::expressions::Expr) -> bool {
        ast::expressions::Visitor::enter(self, node)
    }
    pub fn Leave(&mut self, node: &mut ast::expressions::Expr) -> bool {
        ast::expressions::Visitor::leave(self, node)
    }
}
impl ast::expressions::Visitor for ParamMarkerInPrepareChecker {
    fn enter(&mut self, _node: &mut ast::expressions::Expr) -> bool {
        false
    }
    fn leave(&mut self, _node: &mut ast::expressions::Expr) -> bool {
        true
    }
    fn enter_param_marker(&mut self, node: &mut dyn ast::expressions::ParamMarkerExpr) -> bool {
        self.InPrepareStmt = !node.in_execute();
        true
    }
}
/// Column/CorrelatedColumn 的 FieldType 可能来自共享 InfoSchema，因此跳过；其余表达式清 ParseToJSONFlag。
pub fn DisableParseJSONFlag4Expr(ctx: &dyn EvalContext, expr: &mut dyn Expression) {
    if expr.as_any().is::<Column>() || expr.as_any().is::<CorrelatedColumn>() {
        return;
    }
    let flag = expr.GetType(ctx).GetFlag() & !mysql::ParseToJSONFlag;
    expr.GetTypeMut().SetFlag(flag);
}
/// 由 ParamMarker 构造位置表达式（ORDER BY / GROUP BY 位置引用）。
pub fn ConstructPositionExpr(marker: driver::ParamMarkerExpr) -> ast::PositionExpr {
    ast::PositionExpr {
        parameter: Some(Box::new(marker)),
        position: 0,
    }
}
/// 从位置表达式解析出整数位置。
pub fn PosFromPositionExpr(
    ctx: &dyn BuildContext,
    position: &ast::PositionExpr,
) -> Result<(i32, bool), Error> {
    let Some(marker) = &position.parameter else {
        return Ok((position.position, false));
    };
    let marker = marker
        .as_any()
        .downcast_ref::<driver::ParamMarkerExpr>()
        .ok_or_else(|| {
            errors::Errorf("PositionExpr parameter is not parser_driver::ParamMarkerExpr")
        })?;
    let constant = ParamMarkerExpression(ctx, marker, false)?;
    let (value, null) = GetIntFromConstant(ctx.GetEvalCtx(), &constant)?;
    Ok((value, null))
}
/// 从常量表达式取出字符串值。
pub fn GetStringFromConstant(
    ctx: &dyn EvalContext,
    value: &dyn Expression,
) -> Result<(String, bool), Error> {
    let constant = value
        .as_any()
        .downcast_ref::<Constant>()
        .ok_or_else(|| errors::Errorf("Not a Constant expression"))?;
    constant.EvalString(ctx, chunk::Row::default())
}
/// 从常量表达式取出整型值。
pub fn GetIntFromConstant(
    ctx: &dyn EvalContext,
    value: &dyn Expression,
) -> Result<(i32, bool), Error> {
    let (text, null) = GetStringFromConstant(ctx, value)?;
    if null {
        return Ok((0, true));
    }
    Ok(text
        .parse::<i32>()
        .map_or((0, true), |number| (number, false)))
}
/// 构造“表达式 IS NOT NULL”形式的判断。
pub fn BuildNotNullExpr(ctx: &mut dyn BuildContext, expr: Expr) -> Expr {
    let tiny = *types::NewFieldType(mysql::TypeTiny);
    let is_null = NewFunctionInternal(ctx, ast::IsNull, tiny.clone(), vec![expr]).unwrap();
    NewFunctionInternal(ctx, ast::UnaryNot, tiny, vec![is_null]).unwrap()
}

/// 判断表达式在运行期是否可视为常量（允许确定性内建）。
pub fn IsRuntimeConstExpr(expr: &dyn Expression) -> bool {
    if expr.as_any().is::<Column>() {
        return false;
    }
    if expr.as_any().is::<Constant>() || expr.as_any().is::<CorrelatedColumn>() {
        return true;
    }
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|f| {
            !unFoldableFunctions.contains_key(f.FuncName.L.as_str())
                && f.GetArgs()
                    .iter()
                    .all(|arg| IsRuntimeConstExpr(arg.as_ref()))
        })
}
/// 判断表达式是否包含非确定性函数（如 RAND、UUID）。
pub fn CheckNonDeterministic(expr: &dyn Expression) -> bool {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|f| {
            unFoldableFunctions.contains_key(f.FuncName.L.as_str())
                || f.GetArgs()
                    .iter()
                    .any(|arg| CheckNonDeterministic(arg.as_ref()))
        })
}
/// 判断表达式树中是否出现指定函数名。
pub fn CheckFuncInExpr(expr: &dyn Expression, name: &str) -> bool {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|f| {
            f.FuncName.L == name
                || f.GetArgs()
                    .iter()
                    .any(|arg| CheckFuncInExpr(arg.as_ref(), name))
        })
}
/// 判断表达式是否有可变副作用（SET_VAR、SLEEP 等）。
pub fn IsMutableEffectsExpr(expr: &dyn Expression) -> bool {
    if let Some(f) = expr.as_any().downcast_ref::<ScalarFunction>() {
        return crate::function_traits_kernel::has_mutable_effect(&f.FuncName.L)
            || f.GetArgs()
                .iter()
                .any(|arg| IsMutableEffectsExpr(arg.as_ref()));
    }
    expr.as_any()
        .downcast_ref::<Constant>()
        .and_then(|c| c.DeferredExpr.as_ref())
        .is_some_and(|e| IsMutableEffectsExpr(e.as_ref()))
}
/// 判断表达式是否仅由不可变函数构成。
pub fn IsImmutableFunc(expr: &dyn Expression) -> bool {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .map_or(true, |f| {
            !unFoldableFunctions.contains_key(f.FuncName.L.as_str())
                && !crate::function_traits_kernel::has_mutable_effect(&f.FuncName.L)
                && f.GetArgs().iter().all(|arg| IsImmutableFunc(arg.as_ref()))
        })
}
/// 按语义哈希去除重复表达式，保持首次出现顺序。
pub fn RemoveDupExprs(exprs: Vec<Expr>) -> Vec<Expr> {
    let mut seen = HashSet::new();
    exprs
        .into_iter()
        .filter(|expr| {
            let hash = expr.HashCode().to_vec();
            IsMutableEffectsExpr(expr.as_ref()) || seen.insert(hash)
        })
        .collect()
}

/// 常量可来自字面 Datum、参数标记或 deferred expression；返回值依次为数值、是否 NULL、是否有效。
pub fn GetUint64FromConstant(ctx: &dyn EvalContext, expr: &dyn Expression) -> (u64, bool, bool) {
    let Some(constant) = expr.as_any().downcast_ref::<Constant>() else {
        logutil::BgLogger().warn("not a constant expression");
        return (0, false, false);
    };
    let datum = if let Some(marker) = &constant.ParamMarker {
        match marker.GetUserVar(ctx) {
            Ok(value) => value,
            Err(_) => return (0, false, false),
        }
    } else if let Some(deferred) = &constant.DeferredExpr {
        match deferred.Eval(ctx, chunk::Row::default()) {
            Ok(value) => value,
            Err(_) => return (0, false, false),
        }
    } else {
        constant.Value.clone()
    };
    match datum.Kind() {
        types::KindNull => (0, true, true),
        types::KindInt64 if datum.GetInt64() >= 0 => (datum.GetInt64() as u64, false, true),
        types::KindUint64 => (datum.GetUint64(), false, true),
        _ => (0, false, false),
    }
}

/// 判断表达式列表是否包含虚拟生成列。
pub fn ContainVirtualColumn(exprs: &[Expr]) -> bool {
    exprs.iter().any(|expr| {
        expr.as_any()
            .downcast_ref::<Column>()
            .is_some_and(|c| c.VirtualExpr.is_some())
            || expr
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .is_some_and(|f| ContainVirtualColumn(f.GetArgs()))
    })
}
/// 判断表达式列表是否包含关联列。
pub fn ContainCorrelatedColumn(exprs: &[Expr]) -> bool {
    exprs.iter().any(|expr| {
        expr.as_any().is::<CorrelatedColumn>()
            || expr
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .is_some_and(|f| ContainCorrelatedColumn(f.GetArgs()))
    })
}
/// 判断 JSON_UNQUOTE 是否属于可下推获益的形状（如 ->>）。
pub fn jsonUnquoteFunctionBenefitsFromPushedDown(function: &ScalarFunction) -> bool {
    function.GetArgs()[0]
        .as_any()
        .downcast_ref::<ScalarFunction>()
        .filter(|f| f.FuncName.L == ast::Cast)
        .and_then(|cast| cast.GetArgs()[0].as_any().downcast_ref::<ScalarFunction>())
        .is_some_and(|f| f.FuncName.L == ast::JSONExtract)
}

/// TiKV 投影仅接受列裁剪或明确收益的 JSON 函数；普通函数和 JSON_UNQUOTE 非 ->> 形状均拒绝。
pub fn ProjectionBenefitsFromPushedDown(exprs: &[Expr], input_schema_len: usize) -> bool {
    let mut all_columns = true;
    let mut column_count = 0;
    for expr in exprs {
        if expr.as_any().is::<Column>() {
            column_count += 1;
            continue;
        }
        all_columns = false;
        let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
            return false;
        };
        let json_benefit = matches!(
            function.FuncName.L.as_str(),
            ast::JSONDepth
                | ast::JSONLength
                | ast::JSONType
                | ast::JSONValid
                | ast::JSONContains
                | ast::JSONContainsPath
                | ast::JSONExtract
                | ast::JSONKeys
                | ast::JSONSearch
                | ast::JSONMemberOf
                | ast::JSONOverlaps
        ) || function.FuncName.L == ast::JSONUnquote
            && jsonUnquoteFunctionBenefitsFromPushedDown(function);
        if !json_benefit {
            return false;
        }
    }
    if all_columns {
        column_count < input_schema_len
    } else {
        true
    }
}

/// 计划缓存场景下，判断表达式是否可能被过度优化而不安全。
pub fn MaybeOverOptimized4PlanCache(ctx: &dyn BuildContext, exprs: &[Expr]) -> bool {
    ctx.IsUseCache() && containMutableConst(ctx.GetEvalCtx(), exprs)
}
/// 判断是否包含参数标记或 deferred 等可变常量。
pub fn containMutableConst(ctx: &dyn EvalContext, exprs: &[Expr]) -> bool {
    let _ = ctx;
    exprs.iter().any(|expr| {
        expr.as_any()
            .downcast_ref::<Constant>()
            .is_some_and(|c| c.ParamMarker.is_some() || c.DeferredExpr.is_some())
            || expr
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .is_some_and(|f| containMutableConst(ctx, f.GetArgs()))
    })
}

/// 去掉单个表达式中的可变常量（参数标记/deferred）。
fn removeMutableConstExpr(ctx: &dyn BuildContext, expr: Expr) -> Result<Expr, Error> {
    if let Some(constant) = expr.as_any().downcast_ref::<Constant>() {
        let mut constant = constant.Clone();
        constant.ParamMarker = None;
        if let Some(deferred) = constant.DeferredExpr.take() {
            constant.Value = deferred.Eval(ctx.GetEvalCtx(), chunk::Row::default())?;
        }
        return Ok(Box::new(constant));
    }
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let mut function = function.clone_scalar();
        for arg in function.GetArgsMut() {
            *arg = removeMutableConstExpr(ctx, arg.clone())?;
        }
        function.CleanHashCode();
        return Ok(Box::new(function));
    }
    Ok(expr)
}

/// 清除参数标记；deferred expression 先求值写回 Value，成功后再移除延迟节点。
pub fn RemoveMutableConst(ctx: &dyn BuildContext, exprs: &mut [Expr]) -> Result<(), Error> {
    for expr in exprs {
        *expr = removeMutableConstExpr(ctx, expr.clone())?;
    }
    Ok(())
}

/// KiB（2^10 字节）。
pub const kib: f64 = (1_u64 << 10) as f64;
/// MiB（2^20 字节）。
pub const mib: f64 = (1_u64 << 20) as f64;
/// GiB（2^30 字节）。
pub const gib: f64 = (1_u64 << 30) as f64;
/// TiB（2^40 字节）。
pub const tib: f64 = (1_u64 << 40) as f64;
/// PiB（2^50 字节）。
pub const pib: f64 = (1_u64 << 50) as f64;
/// EiB（2^60 字节）。
pub const eib: f64 = (1_u64 << 60) as f64;
/// 纳秒基准单位。
pub const nano: f64 = 1.0;
/// 微秒（1000 纳秒）。
pub const micro: f64 = 1_000.0 * nano;
/// 毫秒。
pub const milli: f64 = 1_000.0 * micro;
/// 秒。
pub const sec: f64 = 1_000.0 * milli;
/// 分钟。
pub const minute: f64 = 60.0 * sec;
/// 小时。
pub const hour: f64 = 60.0 * minute;
/// 一天的时长。
pub const dayTime: f64 = 24.0 * hour;

/// Match Go `strconv.FormatFloat` spellings used by the original formatting helpers.
fn formatFloatLikeGo(value: f64, scientific: bool, precision: usize) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "+Inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Inf".to_owned();
    }
    if !scientific {
        return format!("{value:.precision$}");
    }
    let formatted = format!("{value:.precision$e}");
    let (mantissa, exponent) = formatted
        .split_once('e')
        .expect("Rust scientific formatting always contains an exponent");
    let exponent = exponent
        .parse::<i32>()
        .expect("Rust scientific formatting emits a numeric exponent");
    format!("{mantissa}e{exponent:+03}")
}

/// 按绝对值选择 IEC 字节单位；大于十万的换算结果使用科学计数法。
pub fn GetFormatBytes(bytes: f64) -> String {
    let (divisor, unit) = match bytes.abs() {
        value if value >= eib => (eib, "EiB"),
        value if value >= pib => (pib, "PiB"),
        value if value >= tib => (tib, "TiB"),
        value if value >= gib => (gib, "GiB"),
        value if value >= mib => (mib, "MiB"),
        value if value >= kib => (kib, "KiB"),
        _ => (1.0, "bytes"),
    };
    if divisor == 1.0 {
        return format!("{} {}", formatFloatLikeGo(bytes, false, 0), unit);
    }
    let value = bytes / divisor;
    if value.abs() >= 100_000.0 {
        format!("{} {}", formatFloatLikeGo(value, true, 2), unit)
    } else {
        format!("{} {}", formatFloatLikeGo(value, false, 2), unit)
    }
}
/// 按绝对值选择纳秒/微秒/毫秒/秒/分/时/天单位格式化时长。
pub fn GetFormatNanoTime(nanoseconds: f64) -> String {
    let (divisor, unit) = match nanoseconds.abs() {
        value if value >= dayTime => (dayTime, "d"),
        value if value >= hour => (hour, "h"),
        value if value >= minute => (minute, "min"),
        value if value >= sec => (sec, "s"),
        value if value >= milli => (milli, "ms"),
        value if value >= micro => (micro, "us"),
        _ => (1.0, "ns"),
    };
    if divisor == 1.0 {
        return format!("{} {}", formatFloatLikeGo(nanoseconds, false, 0), unit);
    }
    let value = nanoseconds / divisor;
    if value.abs() >= 100_000.0 {
        format!("{} {}", formatFloatLikeGo(value, true, 2), unit)
    } else {
        format!("{} {}", formatFloatLikeGo(value, false, 2), unit)
    }
}

/// 按 digest 从 statements_summary 回填规范化 SQL 文本。
pub struct SQLDigestTextRetriever {
    pub SQLDigestsMap: HashMap<String, String>,
    mockLocalData: Option<HashMap<String, String>>,
    mockGlobalData: Option<HashMap<String, String>>,
    fetchAllLimit: usize,
}
/// 创建空的 digest→SQL 文本检索器。
pub fn NewSQLDigestTextRetriever() -> SQLDigestTextRetriever {
    SQLDigestTextRetriever {
        SQLDigestsMap: HashMap::new(),
        mockLocalData: None,
        mockGlobalData: None,
        fetchAllLimit: 512,
    }
}

impl SQLDigestTextRetriever {
    /// 测试用：按 digest 列表从 mock 数据取文本；空列表返回全量。
    pub fn runMockQuery(
        &self,
        data: &HashMap<String, String>,
        values: &[String],
    ) -> HashMap<String, String> {
        if values.is_empty() {
            return data.clone();
        }
        values
            .iter()
            .filter_map(|digest| data.get(digest).map(|text| (digest.clone(), text.clone())))
            .collect()
    }

    /// 根据 query_global 选择 statements_summary 或 cluster_statements_summary，并合并 history 防止轮转丢失。
    pub fn runFetchDigestQuery<E>(
        &self,
        ctx: kv::Context,
        exec: &E,
        query_global: bool,
        values: &[String],
    ) -> Result<HashMap<String, String>, Error>
    where
        E: expropt::SQLExecutor<Context = kv::Context, Row = chunk::Row>,
    {
        if !query_global {
            if let Some(data) = &self.mockLocalData {
                return Ok(self.runMockQuery(data, values));
            }
        } else if let Some(data) = &self.mockGlobalData {
            return Ok(self.runMockQuery(data, values));
        }
        let mut statement = if query_global {
            "select digest, digest_text from information_schema.cluster_statements_summary union distinct select digest, digest_text from information_schema.cluster_statements_summary_history".to_owned()
        } else {
            "select digest, digest_text from information_schema.statements_summary union distinct select digest, digest_text from information_schema.statements_summary_history".to_owned()
        };
        if !values.is_empty() {
            statement.push_str(&format!(
                " where digest in ({})",
                std::iter::repeat("%?")
                    .take(values.len())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        let args = values
            .iter()
            .cloned()
            .map(|value| Box::new(value) as Box<dyn std::any::Any>)
            .collect::<Vec<_>>();
        let internal_ctx = kv::WithInternalSourceType(ctx, kv::InternalTxnOthers);
        let (rows, _) = exec
            .exec_restricted_sql(&internal_ctx, &[], &statement, &args)
            .map_err(|error| errors::Errorf(error.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| (row.GetString(0), row.GetString(1)))
            .collect())
    }
    /// 仅回填 SQLDigestsMap 中仍为空的 digest 文本。
    pub fn updateDigestInfo(&mut self, query_result: &HashMap<String, String>) {
        for (digest, text) in &mut self.SQLDigestsMap {
            if text.is_empty() {
                if let Some(found) = query_result.get(digest) {
                    *text = found.clone();
                }
            }
        }
    }

    /// 从本机 statements_summary 检索并回填 digest 文本。
    pub fn RetrieveLocal<E>(&mut self, ctx: kv::Context, exec: &E) -> Result<(), Error>
    where
        E: expropt::SQLExecutor<Context = kv::Context, Row = chunk::Row>,
    {
        if self.SQLDigestsMap.is_empty() {
            return Ok(());
        }
        let values = if self.SQLDigestsMap.len() <= self.fetchAllLimit {
            self.SQLDigestsMap.keys().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let result = self.runFetchDigestQuery(ctx, exec, false, &values)?;
        if !values.is_empty() && result.len() == self.SQLDigestsMap.len() {
            self.SQLDigestsMap = result;
        } else {
            self.updateDigestInfo(&result);
        }
        Ok(())
    }
    /// 先尝试本地，不足时再查集群级 statements_summary。
    pub fn RetrieveGlobal<E>(&mut self, ctx: kv::Context, exec: &E) -> Result<(), Error>
    where
        E: expropt::SQLExecutor<Context = kv::Context, Row = chunk::Row>,
    {
        self.RetrieveLocal(ctx.clone(), exec)?;
        let unknown = self
            .SQLDigestsMap
            .iter()
            .filter(|(_, text)| text.is_empty())
            .map(|(digest, _)| digest.clone())
            .collect::<Vec<_>>();
        if unknown.is_empty() {
            return Ok(());
        }
        let values = if self.SQLDigestsMap.len() <= self.fetchAllLimit {
            unknown
        } else {
            Vec::new()
        };
        let result = self.runFetchDigestQuery(ctx, exec, true, &values)?;
        self.updateDigestInfo(&result);
        Ok(())
    }
}

/// 用 Rust debug 转义生成便于日志展示的文本，不保留首尾引号。
struct EvalContextParamValues<'a>(&'a dyn EvalContext);

impl exprctx::ParamValues for EvalContextParamValues<'_> {
    fn GetParamValue(&self, index: usize) -> Result<types::Datum, exprctx::ParamError> {
        self.0.GetParamValue(index)
    }
}

/// 将表达式列表格式化为便于日志展示的字符串。
pub fn ExprsToStringsForDisplay(ctx: &dyn EvalContext, exprs: &[Expr]) -> Vec<String> {
    let param_values = EvalContextParamValues(ctx);
    exprs
        .iter()
        .map(|expr| {
            let text = expr.StringWithCtx(Some(&param_values), errors::RedactLogDisable);
            let quoted = format!("{:?}", text);
            quoted[1..quoted.len() - 1].to_owned()
        })
        .collect()
}
/// 判断表达式树中是否存在满足 `condition` 的列。
pub fn HasColumnWithCondition(expr: &dyn Expression, condition: ColumnFilter) -> bool {
    hasColumnWithCondition(expr, condition)
}
/// `HasColumnWithCondition` 的递归实现。
pub fn hasColumnWithCondition(expr: &dyn Expression, condition: ColumnFilter) -> bool {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        return condition(column);
    }
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|f| {
            f.GetArgs()
                .iter()
                .any(|arg| hasColumnWithCondition(arg.as_ref(), condition))
        })
}
/// 在计划缓存开关下判断表达式是否仍可视为常量。
pub fn ConstExprConsiderPlanCache(expr: &dyn Expression, in_plan_cache: bool) -> bool {
    match expr.ConstLevel() {
        ConstStrict => true,
        ConstOnlyInContext => !in_plan_cache,
        _ => false,
    }
}
/// 判断表达式列表是否含副作用。
pub fn ExprsHasSideEffects(exprs: &[Expr]) -> bool {
    exprs.iter().any(|expr| ExprHasSetVarOrSleep(expr.as_ref()))
}
/// 判断表达式是否包含 SET_VAR 或 SLEEP。
pub fn ExprHasSetVarOrSleep(expr: &dyn Expression) -> bool {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|f| {
            matches!(f.FuncName.L.as_str(), ast::SetVar | ast::Sleep)
                || f.GetArgs()
                    .iter()
                    .any(|arg| ExprHasSetVarOrSleep(arg.as_ref()))
        })
}

/// 解析 MySQL COM_STMT_EXECUTE 二进制参数；整数按小端读取，时间/时长先组装文本再交给 types 解析。
pub fn ExecBinaryParam(
    type_context: types::Context,
    binary_params: &[param::BinaryParam],
) -> Result<Vec<Expr>, Error> {
    let mut datums = Vec::with_capacity(binary_params.len());
    for parameter in binary_params {
        let value = &parameter.Val;
        let unsigned = parameter.IsUnsigned;
        let datum = match parameter.Tp {
            mysql::TypeNull => types::Datum::default(),
            mysql::TypeTiny => {
                if unsigned {
                    types::NewUintDatum(value[0] as u64)
                } else {
                    types::NewIntDatum(value[0] as i8 as i64)
                }
            }
            mysql::TypeShort | mysql::TypeYear => {
                let raw = u16::from_le_bytes(value[..2].try_into().unwrap());
                if unsigned {
                    types::NewUintDatum(raw as u64)
                } else {
                    types::NewIntDatum(raw as i16 as i64)
                }
            }
            mysql::TypeInt24 | mysql::TypeLong => {
                let raw = u32::from_le_bytes(value[..4].try_into().unwrap());
                if unsigned {
                    types::NewUintDatum(raw as u64)
                } else {
                    types::NewIntDatum(raw as i32 as i64)
                }
            }
            mysql::TypeLonglong => {
                let raw = u64::from_le_bytes(value[..8].try_into().unwrap());
                if unsigned {
                    types::NewUintDatum(raw)
                } else {
                    types::NewIntDatum(raw as i64)
                }
            }
            mysql::TypeFloat => types::NewFloat32Datum(f32::from_bits(u32::from_le_bytes(
                value[..4].try_into().unwrap(),
            ))),
            mysql::TypeDouble => types::NewFloat64Datum(f64::from_bits(u64::from_le_bytes(
                value[..8].try_into().unwrap(),
            ))),
            mysql::TypeDate | mysql::TypeTimestamp | mysql::TypeDatetime => {
                let text = match value.len() {
                    0 => types_time::ZeroDatetimeStr.to_owned(),
                    4 => binaryDate(0, value).1,
                    7 => binaryDateTime(0, value).1,
                    11 => binaryTimestamp(0, value).1,
                    13 => binaryTimestampWithTZ(0, value).1,
                    _ => return Err(errors::New(mysql::error::ErrMalformPacket().to_string())),
                };
                // TIMESTAMP 参数为了兼容 MySQL 也解析成 Datetime。
                let time = if parameter.Tp == mysql::TypeDate {
                    types_time::ParseDate(&type_context, &text)
                } else {
                    types::ParseDatetime(&type_context, &text)
                }
                .map_err(|error| errors::New(error.to_string()))?;
                types::NewTimeDatum(time)
            }
            mysql::TypeDuration => {
                let (text, fsp) = match value.len() {
                    0 => ("0".to_owned(), 0),
                    8 if value[0] <= 1 => (binaryDuration(1, value, value[0]).1, 0),
                    12 if value[0] <= 1 => {
                        (binaryDurationWithMS(1, value, value[0]).1, types::MaxFsp)
                    }
                    _ => return Err(errors::New(mysql::error::ErrMalformPacket().to_string())),
                };
                let duration = types::ParseDuration(&type_context, &text, fsp)
                    .map_err(|error| errors::New(error.to_string()))?
                    .0;
                types::NewDurationDatum(duration)
            }
            mysql::TypeNewDecimal => {
                if parameter.IsNull {
                    types::NewDecimalDatum(types::MyDecimal::default())
                } else {
                    let mut decimal = types::MyDecimal::default();
                    match decimal.FromString(value) {
                        Ok(()) | Err(types_decimal::mydecimal::DecimalError::Truncated) => {}
                        Err(error) => return Err(errors::New(error.to_string())),
                    };
                    types::NewDecimalDatum(decimal)
                }
            }
            mysql::TypeBlob | mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeLongBlob => {
                types::NewBytesDatum(if parameter.IsNull {
                    Vec::new()
                } else {
                    value.clone()
                })
            }
            mysql::TypeUnspecified
            | mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeString
            | mysql::TypeEnum
            | mysql::TypeSet
            | mysql::TypeGeometry
            | mysql::TypeBit => {
                if parameter.IsNull {
                    types::Datum::default()
                } else {
                    types::NewStringDatum(String::from_utf8_lossy(value).into_owned())
                }
            }
            other => {
                let error = param::ERR_UNKNOWN_FIELD_TYPE.GenWithStack(
                    "stmt unknown field type %d",
                    &[param::terror::errors::ErrorArg::from(other)],
                );
                return Err(errors::New(error.to_string()));
            }
        };
        datums.push(datum);
    }
    Ok(datums
        .into_iter()
        .map(|datum| {
            let mut field_type = types::FieldType::default();
            types::InferParamTypeFromDatum(&datum, &mut field_type);
            Box::new(Constant::with_type(datum, field_type)) as Expr
        })
        .collect())
}

/// 从 MySQL 二进制协议缓冲解析 DATE，返回新偏移与文本。
pub fn binaryDate(mut position: usize, values: &[u8]) -> (usize, String) {
    let year = u16::from_le_bytes(values[position..position + 2].try_into().unwrap());
    position += 2;
    let month = values[position];
    let day = values[position + 1];
    position += 2;
    (position, format!("{:04}-{:02}-{:02}", year, month, day))
}
/// 从二进制协议解析 DATETIME。
pub fn binaryDateTime(position: usize, values: &[u8]) -> (usize, String) {
    let (mut position, date) = binaryDate(position, values);
    let hours = values[position];
    let minutes = values[position + 1];
    let seconds = values[position + 2];
    position += 3;
    (
        position,
        format!("{} {:02}:{:02}:{:02}", date, hours, minutes, seconds),
    )
}
/// 从二进制协议解析 TIMESTAMP（无时区）。
pub fn binaryTimestamp(position: usize, values: &[u8]) -> (usize, String) {
    let (mut position, datetime) = binaryDateTime(position, values);
    let microseconds = u32::from_le_bytes(values[position..position + 4].try_into().unwrap());
    position += 4;
    (position, format!("{}.{:06}", datetime, microseconds))
}
/// 从二进制协议解析带时区偏移的 TIMESTAMP。
pub fn binaryTimestampWithTZ(position: usize, values: &[u8]) -> (usize, String) {
    let (mut position, timestamp) = binaryTimestamp(position, values);
    let shift = i16::from_le_bytes(values[position..position + 2].try_into().unwrap());
    position += 2;
    let hours = shift / 60;
    (
        position,
        format!("{}{:+}:{:02}", timestamp, hours, (shift % 60).abs()),
    )
}
/// 从二进制协议解析 DURATION（天/时/分/秒）。
pub fn binaryDuration(mut position: usize, values: &[u8], negative: u8) -> (usize, String) {
    let sign = if negative == 1 { "-" } else { "" };
    let days = u32::from_le_bytes(values[position..position + 4].try_into().unwrap());
    position += 4;
    let hours = values[position];
    let minutes = values[position + 1];
    let seconds = values[position + 2];
    position += 3;
    (
        position,
        format!(
            "{}{} {:02}:{:02}:{:02}",
            sign, days, hours, minutes, seconds
        ),
    )
}
/// 从二进制协议解析带微秒的 DURATION。
pub fn binaryDurationWithMS(position: usize, values: &[u8], negative: u8) -> (usize, String) {
    let (mut position, duration) = binaryDuration(position, values, negative);
    let microseconds = u32::from_le_bytes(values[position..position + 4].try_into().unwrap());
    position += 4;
    (position, format!("{}.{:06}", duration, microseconds))
}

/// 判断表达式是否为常量 NULL。
pub fn IsConstNull(expr: &dyn Expression) -> bool {
    expr.as_any()
        .downcast_ref::<ScalarFunction>()
        .filter(|f| {
            matches!(
                f.FuncName.L.as_str(),
                ast::LT | ast::LE | ast::GT | ast::GE | ast::EQ | ast::NE
            )
        })
        .and_then(|f| f.GetArgs().get(1))
        .and_then(|arg| arg.as_any().downcast_ref::<Constant>())
        .is_some_and(|c| c.Value.IsNull() && c.DeferredExpr.is_none())
}
/// 判断是否为“列 op 列”形态的标量函数，并返回两侧列。
pub fn IsColOpCol(function: &ScalarFunction) -> (Option<&Column>, Option<&Column>, bool) {
    if function.GetArgs().len() != 2 {
        return (None, None, false);
    }
    let left = function.GetArgs()[0].as_any().downcast_ref::<Column>();
    let right = function.GetArgs()[1].as_any().downcast_ref::<Column>();
    (left, right, left.is_some() && right.is_some())
}
/// 从“列 op 列”函数中抽出左右列引用。
pub fn ExtractColumnsFromColOpCol(function: &ScalarFunction) -> (Option<&Column>, Option<&Column>) {
    if function.GetArgs().len() != 2 {
        return (None, None);
    }
    (
        function.GetArgs()[0].as_any().downcast_ref::<Column>(),
        function.GetArgs()[1].as_any().downcast_ref::<Column>(),
    )
}
