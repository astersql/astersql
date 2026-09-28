// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// 聚合下推（Aggregation Push Down）逻辑优化规则。
//
// 把可分解的聚合穿越 Join / UNION ALL 推到子计划一侧，形成 Partial/Final
// 两阶段聚合，减少 Join 前需处理的数据量。

use crate::rule_aggregation_elimination::{
    AggFuncDesc, AggFuncName, AggMode, LogicalAggregation, LogicalPlan, Result,
};
use crate::task::{Expression, FieldType, JoinType};

/// 聚合下推求解器；`next_column_id` 用于生成 Partial 输出列标识。
#[derive(Default)]
pub struct AggregationPushDownSolver {
    next_column_id: usize,
}
impl AggregationPushDownSolver {
    /// 该聚合是否可相对 Join 分解（无 ORDER BY，且符合 Go 的函数/DISTINCT 白名单）。
    pub fn isDecomposableWithJoin(&self, function: &AggFuncDesc) -> bool {
        function.order_by.is_empty()
            && match function.name {
                AggFuncName::Max | AggFuncName::Min | AggFuncName::FirstRow => true,
                AggFuncName::Sum | AggFuncName::Count => !function.distinct,
                _ => false,
            }
    }
    /// 该聚合是否可穿越 UNION ALL 分解（排除 GroupConcat/JSON 等）。
    pub fn isDecomposableWithUnion(&self, function: &AggFuncDesc) -> bool {
        function.order_by.is_empty()
            && matches!(
                function.name,
                AggFuncName::Max
                    | AggFuncName::Min
                    | AggFuncName::FirstRow
                    | AggFuncName::Sum
                    | AggFuncName::Count
                    | AggFuncName::Avg
                    | AggFuncName::ApproxCountDistinct
            )
    }
    /// 聚合参数列落在左(0)/右(1)/两侧(-1)/皆无(2) 子 schema。
    pub fn getAggFuncChildIdx(
        &self,
        function: &AggFuncDesc,
        leftSchema: &[FieldType],
        rightSchema: &[FieldType],
    ) -> i32 {
        let left = function
            .args
            .iter()
            .any(|e| e.column.is_some_and(|c| c < leftSchema.len()));
        let right = function.args.iter().any(|e| {
            e.column
                .is_some_and(|c| c >= leftSchema.len() && c < leftSchema.len() + rightSchema.len())
        });
        match (left, right) {
            (true, false) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, true) => -1,
        }
    }
    /// 按子侧收集可分解聚合；任一不可分解则整体失败。
    pub fn collectAggFuncs(
        &self,
        agg: &LogicalAggregation,
        joinType: JoinType,
        leftSchema: &[FieldType],
        rightSchema: &[FieldType],
    ) -> (bool, Vec<AggFuncDesc>, Vec<AggFuncDesc>) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        for function in &agg.agg_funcs {
            if !self.isDecomposableWithJoin(function) {
                return (false, Vec::new(), Vec::new());
            }
            match self.getAggFuncChildIdx(function, leftSchema, rightSchema) {
                0 => {
                    if joinType == JoinType::RightOuter && !self.checkAllArgsColumn(function) {
                        return (false, Vec::new(), Vec::new());
                    }
                    left.push(function.clone());
                }
                1 => {
                    if joinType == JoinType::LeftOuter && !self.checkAllArgsColumn(function) {
                        return (false, Vec::new(), Vec::new());
                    }
                    right.push(function.clone());
                }
                2 if joinType == JoinType::LeftOuter => left.push(function.clone()),
                2 => right.push(function.clone()),
                _ => return (false, Vec::new(), Vec::new()),
            }
        }
        (true, left, right)
    }
    /// 按列下标把分组列分到左右两侧。
    pub fn collectGbyCols(
        &self,
        agg: &LogicalAggregation,
        leftLen: usize,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        for column in agg.group_by_items.iter().filter_map(|e| e.column) {
            if column < leftLen {
                add_unique(&mut left, column);
            } else {
                add_unique(&mut right, column);
            }
        }
        (left, right)
    }
    /// 同时拆分聚合函数与分组列到左右孩子。
    pub fn splitAggFuncsAndGbyCols(
        &self,
        agg: &LogicalAggregation,
        joinType: JoinType,
        leftSchema: &[FieldType],
        rightSchema: &[FieldType],
    ) -> (
        bool,
        Vec<AggFuncDesc>,
        Vec<AggFuncDesc>,
        Vec<usize>,
        Vec<usize>,
    ) {
        let (valid, left_funcs, right_funcs) =
            self.collectAggFuncs(agg, joinType, leftSchema, rightSchema);
        let (left_group, right_group) = self.collectGbyCols(agg, leftSchema.len());
        (valid, left_funcs, right_funcs, left_group, right_group)
    }
    /// 向分组列列表去重追加列。
    pub fn addGbyCol(
        &self,
        groupColumns: &mut Vec<usize>,
        columns: impl IntoIterator<Item = usize>,
    ) {
        for column in columns {
            add_unique(groupColumns, column);
        }
    }
    /// 仅 Inner / LeftOuter / RightOuter 允许聚合下推。
    pub fn checkValidJoin(&self, joinType: JoinType) -> bool {
        matches!(
            joinType,
            JoinType::Inner | JoinType::LeftOuter | JoinType::RightOuter
        )
    }
    /// 拆成 Partial1 与 Final；COUNT 的 Final 变为 SUM。
    pub fn decompose(&self, function: &AggFuncDesc, output: usize) -> (AggFuncDesc, AggFuncDesc) {
        let mut partial = function.clone();
        partial.mode = AggMode::Partial1;
        partial.distinct = false;
        let mut final_function = function.clone();
        final_function.mode = AggMode::Final;
        final_function.distinct = false;
        final_function.args = vec![Expression {
            name: format!("partial_{output}"),
            column: Some(output),
            return_type: Some(function.return_type.clone()),
            ..Expression::default()
        }];
        if function.name == AggFuncName::Count {
            final_function.name = AggFuncName::Sum;
        }
        (partial, final_function)
    }
    /// 在子计划上建 Partial 聚合，并返回对应 Final 函数列表。
    pub fn tryToPushDownAgg(
        &mut self,
        functions: &[AggFuncDesc],
        groupColumns: &[usize],
        child: LogicalPlan,
        schema: Vec<FieldType>,
    ) -> Option<(LogicalPlan, Vec<AggFuncDesc>)> {
        if functions.is_empty() {
            return None;
        }
        if functions
            .iter()
            .all(|function| function.name == AggFuncName::FirstRow)
            || matches!(child, LogicalPlan::Join { .. })
        {
            return None;
        }
        if child
            .unique_keys()
            .iter()
            .any(|key| !key.is_empty() && key.iter().all(|column| groupColumns.contains(column)))
        {
            return None;
        }
        let mut partial = Vec::new();
        let mut final_functions = Vec::new();
        for function in functions {
            let output = self.next_column_id;
            self.next_column_id += 1;
            let (p, f) = self.decompose(function, output);
            partial.push(p);
            final_functions.push(f);
        }
        for column in groupColumns {
            partial.push(AggFuncDesc {
                name: AggFuncName::FirstRow,
                args: vec![Expression {
                    name: format!("col_{column}"),
                    column: Some(*column),
                    return_type: schema.get(*column).cloned(),
                    ..Expression::default()
                }],
                distinct: false,
                mode: AggMode::Complete,
                return_type: schema
                    .get(*column)
                    .cloned()
                    .expect("group-by column must belong to the child schema"),
                order_by: Vec::new(),
            });
        }
        let mut group_by_items: Vec<_> = groupColumns
            .iter()
            .map(|c| Expression {
                name: format!("col_{c}"),
                column: Some(*c),
                ..Expression::default()
            })
            .collect();
        if group_by_items.is_empty() {
            group_by_items.push(Expression {
                name: "0".into(),
                ..Expression::default()
            });
        }
        let mut output_schema: Vec<_> = functions
            .iter()
            .map(|function| function.return_type.clone())
            .collect();
        output_schema.extend(
            groupColumns
                .iter()
                .filter_map(|column| schema.get(*column).cloned()),
        );
        let output_len = output_schema.len();
        Some((
            LogicalPlan::Aggregation(LogicalAggregation {
                agg_funcs: partial,
                group_by_items,
                child: Box::new(child),
                schema: output_schema,
                output_columns: (0..output_len).collect(),
                no_eliminate: true,
            }),
            final_functions,
        ))
    }
    /// 外连接空匹配时各聚合的默认填充值。
    pub fn getDefaultValues(&self, agg: &LogicalAggregation) -> (Vec<String>, bool) {
        let mut values = Vec::new();
        for function in &agg.agg_funcs {
            match function.name {
                AggFuncName::Count | AggFuncName::BitOr | AggFuncName::BitXor => {
                    values.push("0".into())
                }
                AggFuncName::BitAnd => values.push("18446744073709551615".into()),
                AggFuncName::Sum
                | AggFuncName::Avg
                | AggFuncName::Max
                | AggFuncName::Min
                | AggFuncName::FirstRow => values.push("null".into()),
                _ => return (Vec::new(), false),
            }
        }
        (values, true)
    }
    /// 是否含 COUNT 或 SUM（外连接下推需特殊处理）。
    pub fn checkAnyCountAndSum(&self, functions: &[AggFuncDesc]) -> bool {
        functions
            .iter()
            .any(|f| matches!(f.name, AggFuncName::Count | AggFuncName::Sum))
    }
    /// 聚合参数是否全为列引用。
    pub fn checkAllArgsColumn(&self, function: &AggFuncDesc) -> bool {
        function.args.iter().all(|arg| arg.column.is_some())
    }
    /// 用给定函数与分组列在孩子上构造新的 Partial 聚合计划。
    pub fn makeNewAgg(
        &mut self,
        functions: &[AggFuncDesc],
        groupColumns: &[usize],
        child: LogicalPlan,
    ) -> Option<LogicalPlan> {
        let schema = child.schema().to_vec();
        self.tryToPushDownAgg(functions, groupColumns, child, schema)
            .map(|v| v.0)
    }
    /// 克隆聚合并把所有函数模式设为 Partial1。
    pub fn splitPartialAgg(&mut self, agg: &LogicalAggregation) -> LogicalAggregation {
        let mut pushed = agg.clone();
        pushed.no_eliminate = true;
        for function in &mut pushed.agg_funcs {
            function.mode = AggMode::Partial1;
            function.distinct = false;
        }
        pushed
    }
    /// 把 Partial 聚合推到 UNION ALL 的单个孩子上。
    pub fn pushAggCrossUnion(
        &mut self,
        agg: &LogicalAggregation,
        unionSchema: &[FieldType],
        child: LogicalPlan,
    ) -> Result<LogicalPlan> {
        if agg
            .agg_funcs
            .iter()
            .any(|f| !self.isDecomposableWithUnion(f))
        {
            return Err("aggregate cannot be decomposed across UNION ALL".into());
        }
        let mut partial = self.splitPartialAgg(agg);
        partial.child = Box::new(child);
        partial.schema = unionSchema.to_vec();
        Ok(LogicalPlan::Aggregation(partial))
    }
    /// 规则入口：执行聚合下推遍历。
    pub fn Optimize(&mut self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        let plan = self.aggPushDown(plan)?;
        Ok(plan)
    }
    /// 尝试把聚合推过 UNION ALL：各分支 Partial，父层改 Final。
    pub fn tryAggPushDownForUnion(
        &mut self,
        children: Vec<LogicalPlan>,
        agg: &mut LogicalAggregation,
        schema: Vec<FieldType>,
    ) -> Result<bool> {
        if agg
            .agg_funcs
            .iter()
            .any(|f| !self.isDecomposableWithUnion(f))
        {
            return Ok(false);
        }
        let mut rewritten = Vec::new();
        for child in children {
            rewritten.push(self.pushAggCrossUnion(agg, &schema, child)?);
        }
        agg.child = Box::new(LogicalPlan::UnionAll {
            children: rewritten,
            schema,
        });
        for function in &mut agg.agg_funcs {
            function.mode = AggMode::Final;
            if function.name == AggFuncName::Count {
                function.name = AggFuncName::Sum;
            }
        }
        Ok(true)
    }
    /// 对聚合节点尝试穿越 Union/Join 下推，否则递归子树。
    pub fn aggPushDown(&mut self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        let LogicalPlan::Aggregation(mut agg) = plan else {
            return self.recurse(plan);
        };
        let temporary_leaf = LogicalPlan::Node {
            node: Default::default(),
            unique_keys: Vec::new(),
            max_one_row: false,
        };
        let child = *std::mem::replace(&mut agg.child, Box::new(temporary_leaf));
        match child {
            LogicalPlan::UnionAll { children, schema } => {
                let changed = self.tryAggPushDownForUnion(children, &mut agg, schema)?;
                Ok((LogicalPlan::Aggregation(agg), changed))
            }
            LogicalPlan::Join {
                join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                schema,
            } if self.checkValidJoin(join_type) => {
                let left_schema = left.schema().to_vec();
                let right_schema = right.schema().to_vec();
                let (valid, left_funcs, right_funcs, mut left_group, mut right_group) =
                    self.splitAggFuncsAndGbyCols(&agg, join_type, &left_schema, &right_schema);
                if !valid {
                    agg.child = Box::new(LogicalPlan::Join {
                        join_type,
                        left,
                        right,
                        equal_conditions,
                        other_conditions,
                        schema,
                    });
                    return Ok((LogicalPlan::Aggregation(agg), false));
                }
                // Join 等值列必须进入两侧分组，否则 Partial 分组粒度错误。
                self.addGbyCol(&mut left_group, equal_conditions.iter().map(|(l, _)| *l));
                self.addGbyCol(&mut right_group, equal_conditions.iter().map(|(_, r)| *r));
                // 一侧 COUNT/SUM 会受另一侧连接基数影响，因此禁止把另一侧聚合下推。
                let right_invalid = self.checkAnyCountAndSum(&left_funcs);
                let left_invalid = self.checkAnyCountAndSum(&right_funcs);
                let left_child = *left;
                let right_child = *right;
                let (left, left_final) = if left_invalid {
                    (left_child, Vec::new())
                } else {
                    let child_schema = left_child.schema().to_vec();
                    match self.tryToPushDownAgg(
                        &left_funcs,
                        &left_group,
                        left_child.clone(),
                        child_schema,
                    ) {
                        Some((plan, finals)) => (plan, finals),
                        None => (left_child, Vec::new()),
                    }
                };
                let (right, mut right_final) = if right_invalid {
                    (right_child, Vec::new())
                } else {
                    let child_schema = right_child.schema().to_vec();
                    match self.tryToPushDownAgg(
                        &right_funcs,
                        &right_group,
                        right_child.clone(),
                        child_schema,
                    ) {
                        Some((plan, finals)) => (plan, finals),
                        None => (right_child, Vec::new()),
                    }
                };
                let right_offset = left.schema().len();
                for function in &mut right_final {
                    for argument in &mut function.args {
                        if let Some(column) = &mut argument.column {
                            *column += right_offset;
                            argument.name = format!("partial_{column}");
                        }
                    }
                }
                let changed = !left_final.is_empty() || !right_final.is_empty();
                let mut left_final = left_final.into_iter();
                let mut right_final = right_final.into_iter();
                for function in &mut agg.agg_funcs {
                    match self.getAggFuncChildIdx(function, &left_schema, &right_schema) {
                        0 => {
                            if let Some(final_function) = left_final.next() {
                                *function = final_function;
                            }
                        }
                        1 => {
                            if let Some(final_function) = right_final.next() {
                                *function = final_function;
                            }
                        }
                        2 if join_type == JoinType::LeftOuter => {
                            if let Some(final_function) = left_final.next() {
                                *function = final_function;
                            }
                        }
                        2 => {
                            if let Some(final_function) = right_final.next() {
                                *function = final_function;
                            }
                        }
                        _ => {}
                    }
                }
                let mut rewritten_schema = left.schema().to_vec();
                rewritten_schema.extend_from_slice(right.schema());
                agg.child = Box::new(LogicalPlan::Join {
                    join_type,
                    left: Box::new(left),
                    right: Box::new(right),
                    equal_conditions,
                    other_conditions,
                    schema: rewritten_schema,
                });
                Ok((LogicalPlan::Aggregation(agg), changed))
            }
            other => {
                let (child, changed) = self.recurse(other)?;
                agg.child = Box::new(child);
                Ok((LogicalPlan::Aggregation(agg), changed))
            }
        }
    }
    /// 规则注册名。
    pub fn Name(&self) -> &'static str {
        "aggregation_push_down"
    }
    /// 非聚合根：递归下推到投影/连接/并集/Expand 孩子。
    fn recurse(&mut self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)> {
        Ok(match plan {
            LogicalPlan::Projection {
                expressions,
                child,
                schema,
            } => {
                let (child, changed) = self.aggPushDown(*child)?;
                (
                    LogicalPlan::Projection {
                        expressions,
                        child: Box::new(child),
                        schema,
                    },
                    changed,
                )
            }
            LogicalPlan::Join {
                join_type,
                left,
                right,
                equal_conditions,
                other_conditions,
                schema,
            } => {
                let (left, l) = self.aggPushDown(*left)?;
                let (right, r) = self.aggPushDown(*right)?;
                (
                    LogicalPlan::Join {
                        join_type,
                        left: Box::new(left),
                        right: Box::new(right),
                        equal_conditions,
                        other_conditions,
                        schema,
                    },
                    l || r,
                )
            }
            LogicalPlan::UnionAll { children, schema } => {
                let mut out = Vec::new();
                let mut changed = false;
                for child in children {
                    let (child, c) = self.aggPushDown(child)?;
                    changed |= c;
                    out.push(child);
                }
                (
                    LogicalPlan::UnionAll {
                        children: out,
                        schema,
                    },
                    changed,
                )
            }
            LogicalPlan::Expand {
                child,
                grouping_sets,
                level_projections,
                schema,
            } => {
                let (child, changed) = self.aggPushDown(*child)?;
                (
                    LogicalPlan::Expand {
                        child: Box::new(child),
                        grouping_sets,
                        level_projections,
                        schema,
                    },
                    changed,
                )
            }
            other => (other, false),
        })
    }
}

/// 向列表去重追加列下标。
fn add_unique(columns: &mut Vec<usize>, column: usize) {
    if !columns.contains(&column) {
        columns.push(column);
    }
}
