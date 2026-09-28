// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");

// 点查（Point Get）快路径计划构建。
//
// 对满足条件的单表等值/IN/OR 查询，绕过通用优化器直接生成
// PointGet / BatchPointGet 以及基于它们的点更新、点删除计划，
// 以降低简单主键/唯一索引查找的规划开销。

use crate::planbuilder::{
    BuilderError, ColumnInfo, IndexMeta, Result, Schema, SchemaColumn, TableInfo, Value,
};
use crate::task::{Expression, FieldType, PlanKind, PlanNode, TypeCode};
use std::collections::HashSet;

/// 全局索引无列位置标记（-1 表示不按列定位）。
pub const GlobalWithoutColumnPos: i32 = -1;
/// 会话/缓存中存放点查计划的键名。
pub const PointPlanKey: &str = "pointPlanKey";

#[derive(Clone, Debug)]
/// 等值谓词抽出的列名与常量值对，可带预处理参数标记。
pub struct nameValuePair {
    /// 列名。
    pub column_name: String,
    /// 常量值。
    pub value: Value,
    /// 预处理参数下标。
    pub param_marker: Option<usize>,
    /// 字段类型。
    pub field_type: FieldType,
}
#[derive(Clone, Debug)]
/// 快速点查计划及其输出列名。
pub struct PointPlanVal {
    /// 关联计划节点。
    pub plan: FastPlan,
    /// 输出列名。
    pub names: Vec<String>,
}
#[derive(Clone, Debug)]
/// 快速路径计划：单点/批量点查、点更新、点删除。
pub enum FastPlan {
    Point(PointGetPlan),
    Batch(BatchPointGetPlan),
    Update(PointUpdatePlan),
    Delete(PointDeletePlan),
}

#[derive(Clone, Debug)]
/// 可尝试走点查快路径的简化查询形态（单表、简单谓词等）。
pub struct FastQuery {
    /// 表名。
    pub table: TableInfo,
    /// 别名。
    pub alias: Option<String>,
    /// 投影字段列表。
    pub fields: Vec<FastField>,
    /// 谓词列表。
    pub predicates: Vec<Predicate>,
    /// 行锁相关信息或是否加锁。
    pub lock: Option<LockInfo>,
    /// ORDER BY 是否降序。
    pub order_desc: bool,
    /// LIMIT 值。
    pub limit: Option<u64>,
    /// USE INDEX 提示。
    pub index_hints: Vec<String>,
    /// IGNORE INDEX 提示。
    pub ignore_index_hints: Vec<String>,
}
#[derive(Clone, Debug)]
/// SELECT 投影字段：列名、别名或 row_checksum。
pub struct FastField {
    /// 列名。
    pub column: String,
    /// 别名。
    pub alias: Option<String>,
    /// 是否为 row_checksum 伪列。
    pub row_checksum: bool,
}
#[derive(Clone, Debug)]
/// 快路径可识别的谓词：等值、IN、AND/OR、其它表达式等。
pub enum Predicate {
    Eq(String, Value),
    NullSafeEq(String, Value),
    In(String, Vec<Value>),
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Other(Expression),
    False,
}
#[derive(Clone, Debug)]
/// FOR UPDATE 等行锁信息：是否等待及等待秒数。
pub struct LockInfo {
    /// 是否 FOR UPDATE。
    pub for_update: bool,
    /// 是否 NOWAIT。
    pub nowait: bool,
    /// 等待秒数。
    pub wait_seconds: Option<i64>,
}

#[derive(Clone, Debug)]
/// 单点获取计划：按主键 handle 或唯一索引精确定位一行。
pub struct PointGetPlan {
    /// 表名。
    pub table: TableInfo,
    /// 选用的索引元数据。
    pub index: Option<IndexMeta>,
    /// 行 handle 值（整型主键路径）。
    pub handle: Option<Value>,
    /// 索引查找键值。
    pub index_values: Vec<Value>,
    /// 访问条件表达式。
    pub access_conditions: Vec<Expression>,
    /// 输出 Schema。
    pub schema: Schema,
    /// 行锁相关信息或是否加锁。
    pub lock: bool,
    /// 锁等待时间（毫秒）。
    pub lock_wait_time: i64,
    /// 目标分区 ID。
    pub partition_id: Option<i64>,
    /// 列列表。
    pub columns: Vec<ColumnInfo>,
}
#[derive(Clone, Debug)]
/// 批量点获取计划：一次按多组 handle/索引值取多行。
pub struct BatchPointGetPlan {
    /// 表名。
    pub table: TableInfo,
    /// 选用的索引元数据。
    pub index: Option<IndexMeta>,
    /// 批量 handle 值列表。
    pub handles: Vec<Value>,
    /// 索引查找键值。
    pub index_values: Vec<Vec<Value>>,
    /// 访问条件表达式。
    pub access_conditions: Vec<Expression>,
    /// 输出 Schema。
    pub schema: Schema,
    /// 行锁相关信息或是否加锁。
    pub lock: bool,
    /// 锁等待时间（毫秒）。
    pub lock_wait_time: i64,
    /// 是否保持顺序。
    pub keep_order: bool,
    /// 是否降序。
    pub desc: bool,
    /// 批量目标分区 ID。
    pub partition_ids: Vec<i64>,
}
#[derive(Clone, Debug)]
/// UPDATE 赋值：目标列与新值。
pub struct Assignment {
    /// 列名。
    pub column: String,
    /// 常量值。
    pub value: Value,
}
#[derive(Clone, Debug)]
/// 基于点查源的更新计划。
pub struct PointUpdatePlan {
    /// 点查源计划。
    pub source: Box<FastPlan>,
    /// 表名。
    pub table: TableInfo,
    /// UPDATE 赋值列表。
    pub assignments: Vec<Assignment>,
    /// 按列序整理后的赋值。
    pub ordered: Vec<(usize, Value)>,
    /// 是否忽略错误（INSERT IGNORE 等）。
    pub ignore_error: bool,
}
#[derive(Clone, Debug)]
/// 基于点查源的删除计划。
pub struct PointDeletePlan {
    /// 点查源计划。
    pub source: Box<FastPlan>,
    /// 表名。
    pub table: TableInfo,
    /// 是否忽略错误（INSERT IGNORE 等）。
    pub ignore_error: bool,
}

/// 根据锁信息与会话默认值计算是否加锁及等待毫秒数。
pub fn getLockWaitTime(lock: Option<&LockInfo>, session_wait_ms: i64) -> (bool, i64) {
    match lock {
        Some(lock) if lock.for_update => (
            true,
            if lock.nowait {
                0
            } else {
                lock.wait_seconds
                    .map(|s| s.saturating_mul(1000))
                    .unwrap_or(session_wait_ms)
            },
        ),
        _ => (false, 0),
    }
}

/// 尝试将简化查询编译为点查/批量点查快路径计划。
pub fn TryFastPlan(
    query: &FastQuery,
    privilege_ok: bool,
    session_wait_ms: i64,
) -> Option<FastPlan> {
    // 权限不足或无投影字段时不能走快路径。
    if !privilege_ok || query.fields.is_empty() {
        return None;
    }
    // 优先尝试 OR 分支合并为批量点查。
    if let Some(batch) = tryOr2BatchPointGet(query, session_wait_ms) {
        return Some(FastPlan::Batch(batch));
    }
    let pairs = getNameValuePairs(&query.table, query.predicates.as_slice()).ok()?;
    if let Some(batch) = tryWhereIn2BatchPointGet(query, &pairs, session_wait_ms) {
        return Some(FastPlan::Batch(batch));
    }
    tryPointGetPlan(query, &pairs, true, session_wait_ms).map(FastPlan::Point)
}

/// 将 AND/OR 谓词展开为可独立点查的析取分支集合。
fn predicateBranches(predicates: &[Predicate]) -> Option<Vec<Vec<&Predicate>>> {
    /// expand：计划构建相关符号（对齐 Go 同名定义）。
    fn expand(predicate: &Predicate) -> Option<Vec<Vec<&Predicate>>> {
        match predicate {
            Predicate::Or(items) if !items.is_empty() => {
                let mut branches = Vec::new();
                for item in items {
                    branches.extend(expand(item)?);
                }
                Some(branches)
            }
            Predicate::And(items) => combine(items),
            Predicate::Eq(_, _) => Some(vec![vec![predicate]]),
            Predicate::False => Some(Vec::new()),
            Predicate::NullSafeEq(_, _)
            | Predicate::In(_, _)
            | Predicate::Other(_)
            | Predicate::Or(_) => None,
        }
    }

    /// combine：计划构建相关符号（对齐 Go 同名定义）。
    fn combine(predicates: &[Predicate]) -> Option<Vec<Vec<&Predicate>>> {
        let mut branches = vec![Vec::new()];
        for predicate in predicates {
            let expanded = expand(predicate)?;
            if expanded.is_empty() {
                return Some(Vec::new());
            }
            let mut next = Vec::new();
            for prefix in &branches {
                for suffix in &expanded {
                    let mut branch = prefix.clone();
                    branch.extend(suffix.iter().copied());
                    next.push(branch);
                }
            }
            branches = next;
        }
        Some(branches)
    }

    combine(predicates)
}

/// 把单个谓词分支转为列名-值对列表。
fn pairsForBranch(table: &TableInfo, branch: &[&Predicate]) -> Option<Vec<nameValuePair>> {
    let owned = branch
        .iter()
        .map(|predicate| (*predicate).clone())
        .collect::<Vec<_>>();
    getNameValuePairs(table, &owned).ok()
}

/// 若 WHERE 含 OR 等值分支，尝试合并为批量点查。
pub fn tryOr2BatchPointGet(query: &FastQuery, session_wait_ms: i64) -> Option<BatchPointGetPlan> {
    /// containsOr：计划构建相关符号（对齐 Go 同名定义）。
    fn containsOr(predicate: &Predicate) -> bool {
        match predicate {
            Predicate::Or(_) => true,
            Predicate::And(items) => items.iter().any(containsOr),
            _ => false,
        }
    }

    if !query.predicates.iter().any(containsOr) || query.limit == Some(0) {
        return None;
    }
    let branches = predicateBranches(&query.predicates)?;
    if branches.len() < 2 {
        return None;
    }
    let (schema, _) = buildSchemaFromFields(&query.table, &query.fields).ok()?;
    let mut plan = newBatchPointGetPlan(&query.table, schema);
    let (lock, wait) = getLockWaitTime(query.lock.as_ref(), session_wait_ms);
    plan.lock = lock;
    plan.lock_wait_time = wait;
    plan.keep_order = query.limit.is_some();
    plan.desc = query.order_desc;

    let branch_pairs = branches
        .iter()
        .map(|branch| pairsForBranch(&query.table, branch))
        .collect::<Option<Vec<_>>>()?;
    if query.table.pk_is_handle {
        let primary = query
            .table
            .columns
            .iter()
            .find(|column| column.primary_key)?;
        for pairs in &branch_pairs {
            let pair = pairs
                .iter()
                .find(|pair| pair.column_name.eq_ignore_ascii_case(&primary.name))?;
            plan.handles.push(getPointGetValue(primary, &pair.value)?);
        }
    } else {
        let first = branch_pairs.first()?;
        let index = choose_index(
            &query.table,
            first,
            None,
            &query.index_hints,
            &query.ignore_index_hints,
        )?;
        for pairs in &branch_pairs {
            if checkTblIndexForPointPlan(&query.table, Some(&index), pairs).is_err() {
                return None;
            }
            let (values, _, _) = getIndexValues(&index, &query.table, pairs);
            if values.len() != index.columns.len() {
                return None;
            }
            plan.index_values.push(values);
        }
        plan.index = Some(index);
    }
    plan.access_conditions = query.predicates.iter().map(predicate_expr).collect();
    Some(plan)
}

/// 构造空的单点获取计划骨架。
pub fn newPointGetPlan(table: &TableInfo, schema: Schema) -> PointGetPlan {
    PointGetPlan {
        table: table.clone(),
        index: None,
        handle: None,
        index_values: Vec::new(),
        access_conditions: Vec::new(),
        schema,
        lock: false,
        lock_wait_time: 0,
        partition_id: None,
        columns: table.columns.clone(),
    }
}
/// 构造空的批量点获取计划骨架。
pub fn newBatchPointGetPlan(table: &TableInfo, schema: Schema) -> BatchPointGetPlan {
    BatchPointGetPlan {
        table: table.clone(),
        index: None,
        handles: Vec::new(),
        index_values: Vec::new(),
        access_conditions: Vec::new(),
        schema,
        lock: false,
        lock_wait_time: 0,
        keep_order: false,
        desc: false,
        partition_ids: Vec::new(),
    }
}

/// 将 WHERE col IN (...) 转为批量点查。
pub fn tryWhereIn2BatchPointGet(
    query: &FastQuery,
    pairs: &[nameValuePair],
    session_wait_ms: i64,
) -> Option<BatchPointGetPlan> {
    let in_pred = query.predicates.iter().find_map(|p| match p {
        Predicate::In(name, values) => Some((name, values)),
        _ => None,
    })?;
    if in_pred.1.is_empty() || query.limit == Some(0) {
        return None;
    }
    let (schema, _) = buildSchemaFromFields(&query.table, &query.fields).ok()?;
    let mut plan = newBatchPointGetPlan(&query.table, schema);
    let (lock, wait) = getLockWaitTime(query.lock.as_ref(), session_wait_ms);
    plan.lock = lock;
    plan.lock_wait_time = wait;
    plan.keep_order = query.limit.is_some();
    plan.desc = query.order_desc;
    let column = findCol(&query.table, in_pred.0)?;
    if column.primary_key && query.table.pk_is_handle {
        plan.handles = in_pred
            .1
            .iter()
            .map(|value| getPointGetValue(column, value))
            .collect::<Option<Vec<_>>>()?;
    } else {
        let index = choose_index(
            &query.table,
            pairs,
            Some(in_pred.0),
            &query.index_hints,
            &query.ignore_index_hints,
        )?;
        let mut values = Vec::new();
        for in_value in in_pred.1 {
            let mut row = Vec::new();
            for offset in &index.columns {
                let col = &query.table.columns[*offset];
                if col.name.eq_ignore_ascii_case(in_pred.0) {
                    row.push(getPointGetValue(col, in_value)?);
                } else {
                    let pair = pairs
                        .iter()
                        .find(|p| p.column_name.eq_ignore_ascii_case(&col.name))?;
                    row.push(getPointGetValue(col, &pair.value)?);
                }
            }
            values.push(row);
        }
        plan.index = Some(index);
        plan.index_values = values;
    }
    plan.access_conditions = query.predicates.iter().map(predicate_expr).collect();
    Some(plan)
}

/// 尝试由等值谓词构造单点 PointGet 计划。
pub fn tryPointGetPlan(
    query: &FastQuery,
    pairs: &[nameValuePair],
    check: bool,
    session_wait_ms: i64,
) -> Option<PointGetPlan> {
    if query
        .predicates
        .iter()
        .any(|p| matches!(p, Predicate::In(_, _) | Predicate::Other(_)))
        || query.limit == Some(0)
    {
        return None;
    }
    let (schema, _) = buildSchemaFromFields(&query.table, &query.fields).ok()?;
    let mut plan = newPointGetPlan(&query.table, schema);
    let (lock, wait) = getLockWaitTime(query.lock.as_ref(), session_wait_ms);
    plan.lock = lock;
    plan.lock_wait_time = wait;
    if let Some((handle, _)) = findPKHandle(&query.table, pairs) {
        plan.handle = Some(getPointGetValue(
            query.table.columns.iter().find(|c| c.primary_key)?,
            &handle.value,
        )?);
    } else {
        let index = choose_index(
            &query.table,
            pairs,
            None,
            &query.index_hints,
            &query.ignore_index_hints,
        )?;
        let (values, _, _) = getIndexValues(&index, &query.table, pairs);
        if values.len() != index.columns.len() {
            return None;
        }
        plan.index = Some(index);
        plan.index_values = values;
    }
    if check && checkTblIndexForPointPlan(&query.table, plan.index.as_ref(), pairs).is_err() {
        return None;
    }
    plan.access_conditions = query.predicates.iter().map(predicate_expr).collect();
    Some(plan)
}

/// 在可用唯一索引中为点查选择匹配索引。
fn choose_index(
    table: &TableInfo,
    pairs: &[nameValuePair],
    in_column: Option<&str>,
    use_hints: &[String],
    ignore_hints: &[String],
) -> Option<IndexMeta> {
    table
        .indices
        .iter()
        .filter(|i| {
            i.unique
                && !i.invisible
                && !i.prefix_lengths.iter().any(Option::is_some)
                && indexIsAvailableByHints(i, use_hints, ignore_hints)
        })
        .find(|i| {
            i.columns.iter().all(|o| {
                pairs
                    .iter()
                    .any(|p| p.column_name.eq_ignore_ascii_case(&table.columns[*o].name))
                    || in_column.is_some_and(|n| n.eq_ignore_ascii_case(&table.columns[*o].name))
            })
        })
        .cloned()
}
/// 检查表索引是否适合作为点查访问路径。
pub fn checkTblIndexForPointPlan(
    table: &TableInfo,
    index: Option<&IndexMeta>,
    pairs: &[nameValuePair],
) -> Result<()> {
    if let Some(index) = index {
        if !index.unique
            || index.columns.iter().any(|offset| {
                !pairs.iter().any(|p| {
                    p.column_name
                        .eq_ignore_ascii_case(&table.columns[*offset].name)
                })
            })
        {
            return Err(BuilderError(
                "index is not a complete unique point path".into(),
            ));
        }
    } else if !table.pk_is_handle
        || !pairs.iter().any(|p| {
            table
                .columns
                .iter()
                .any(|c| c.primary_key && c.name.eq_ignore_ascii_case(&p.column_name))
        })
    {
        return Err(BuilderError("primary key handle is missing".into()));
    }
    Ok(())
}
/// 结合 USE/IGNORE INDEX 提示判断索引是否可用。
pub fn indexIsAvailableByHints(
    index: &IndexMeta,
    use_hints: &[String],
    ignore_hints: &[String],
) -> bool {
    !ignore_hints
        .iter()
        .any(|n| index.name.eq_ignore_ascii_case(n))
        && (use_hints.is_empty() || use_hints.iter().any(|n| index.name.eq_ignore_ascii_case(n)))
}
/// 校验快路径计划所需的表级权限。
pub fn checkFastPlanPrivilege(
    privileges: &HashSet<(String, String, String)>,
    db: &str,
    table: &str,
    required: &[&str],
) -> Result<()> {
    if required
        .iter()
        .all(|p| privileges.contains(&(db.into(), table.into(), (*p).into())))
    {
        Ok(())
    } else {
        Err(BuilderError(format!("access denied for {db}.{table}")))
    }
}
/// 由 SELECT 字段列表构建输出 Schema。
pub fn buildSchemaFromFields(
    table: &TableInfo,
    fields: &[FastField],
) -> Result<(Schema, Vec<String>)> {
    let mut schema = Vec::new();
    let mut names = Vec::new();
    for (idx, field) in fields.iter().enumerate() {
        if field.row_checksum {
            let (name, column, ok) = tryExtractRowChecksumColumn(field, idx);
            if ok {
                schema.push(column);
                names.push(name);
                continue;
            }
        }
        let column = findCol(table, &field.column)
            .ok_or_else(|| BuilderError(format!("unknown column {}", field.column)))?;
        schema.push(SchemaColumn {
            name: field.alias.clone().unwrap_or_else(|| column.name.clone()),
            field_type: column.field_type.clone(),
            flag: 0,
        });
        names.push(field.alias.clone().unwrap_or_else(|| column.name.clone()));
    }
    Ok((schema, names))
}
/// 尝试将字段识别为 row_checksum 伪列。
pub fn tryExtractRowChecksumColumn(field: &FastField, _idx: usize) -> (String, SchemaColumn, bool) {
    let ok = field.row_checksum || field.column.eq_ignore_ascii_case("tidb_row_checksum");
    let name = field
        .alias
        .clone()
        .unwrap_or_else(|| "tidb_row_checksum".into());
    (
        name.clone(),
        SchemaColumn {
            name,
            field_type: FieldType {
                code: TypeCode::UInt,
                flen: 20,
                decimal: 0,
                unsigned: true,
            },
            flag: 0,
        },
        ok,
    )
}
/// 取出快路径查询的单表名与别名。
pub fn getSingleTableNameAndAlias(query: &FastQuery) -> (&str, &str) {
    (
        &query.table.name,
        query.alias.as_deref().unwrap_or(&query.table.name),
    )
}

/// 从谓词中提取可用于点查的列名-值对。
pub fn getNameValuePairs(
    table: &TableInfo,
    predicates: &[Predicate],
) -> Result<Vec<nameValuePair>> {
    let mut out = Vec::new();
    /// walk：计划构建相关符号（对齐 Go 同名定义）。
    fn walk(table: &TableInfo, p: &Predicate, out: &mut Vec<nameValuePair>) -> Result<()> {
        match p {
            Predicate::Eq(name, value) => {
                let col = findCol(table, name)
                    .ok_or_else(|| BuilderError(format!("unknown column {name}")))?;
                if out
                    .iter()
                    .any(|pair| pair.column_name.eq_ignore_ascii_case(name))
                {
                    return Err(BuilderError(format!(
                        "column {name} constrained more than once"
                    )));
                }
                out.push(nameValuePair {
                    column_name: name.clone(),
                    value: value.clone(),
                    param_marker: None,
                    field_type: col.field_type.clone(),
                });
            }
            Predicate::And(items) => {
                for item in items {
                    walk(table, item, out)?;
                }
            }
            Predicate::False => {}
            Predicate::NullSafeEq(_, _) => {
                return Err(BuilderError("predicate is not a point equality".into()));
            }
            Predicate::In(_, _) | Predicate::Or(_) | Predicate::Other(_) => {}
        }
        Ok(())
    }
    for p in predicates {
        walk(table, p, &mut out)?;
    }
    Ok(out)
}
/// 将字面量转为点查可用的 typed Value。
pub fn getPointGetValue(column: &ColumnInfo, value: &Value) -> Option<Value> {
    if checkCanConvertInPointGet(column, value) {
        crate::planbuilder::convertValue(value, column).ok()
    } else {
        None
    }
}
/// 判断常量类型是否可转换为目标列类型用于点查。
pub fn checkCanConvertInPointGet(column: &ColumnInfo, value: &Value) -> bool {
    match (&column.field_type.code, value) {
        (_, Value::Null) => false,
        (TypeCode::Int, Value::Int(_) | Value::String(_))
        | (TypeCode::UInt, Value::UInt(_) | Value::String(_))
        | (TypeCode::String, Value::String(_))
        | (TypeCode::Bytes, Value::Bytes(_)) => true,
        _ => false,
    }
}
/// 在列名-值对中查找主键 handle 列。
pub fn findPKHandle<'a>(
    table: &TableInfo,
    pairs: &'a [nameValuePair],
) -> Option<(&'a nameValuePair, FieldType)> {
    let pk = table
        .columns
        .iter()
        .find(|c| c.primary_key && table.pk_is_handle)?;
    pairs
        .iter()
        .find(|p| p.column_name.eq_ignore_ascii_case(&pk.name))
        .map(|p| (p, pk.field_type.clone()))
}
/// 按索引列顺序从列名-值对组装索引查找键。
pub fn getIndexValues(
    index: &IndexMeta,
    table: &TableInfo,
    pairs: &[nameValuePair],
) -> (Vec<Value>, Vec<Expression>, Vec<FieldType>) {
    let mut values = Vec::new();
    let mut constants = Vec::new();
    let mut types = Vec::new();
    for offset in &index.columns {
        let column = &table.columns[*offset];
        let Some(pair) = pairs
            .iter()
            .find(|p| p.column_name.eq_ignore_ascii_case(&column.name))
        else {
            break;
        };
        if let Some(value) = getPointGetValue(column, &pair.value) {
            constants.push(Expression {
                name: format!("{value:?}"),
                return_type: Some(column.field_type.clone()),
                ..Expression::default()
            });
            values.push(value);
            types.push(column.field_type.clone());
        }
    }
    (values, constants, types)
}
/// 在列名-值对列表中按列名查找下标。
pub fn findInPairs(name: &str, pairs: &[nameValuePair]) -> Option<usize> {
    pairs
        .iter()
        .position(|p| p.column_name.eq_ignore_ascii_case(name))
}

#[derive(Default)]
/// 检测表达式树中是否含有子查询。
pub struct subQueryChecker {
    found: bool,
}
impl subQueryChecker {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&mut self, expr: &Expression) -> bool {
        if expr.name.starts_with("subquery:") {
            self.found = true;
            false
        } else {
            true
        }
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&self, _expr: &Expression) -> bool {
        true
    }
}
/// 判断表达式是否包含子查询（点更新等场景需排除）。
pub fn isExprHasSubQuery(expr: &Expression) -> bool {
    expr.name.starts_with("subquery:")
}
/// 检查 UPDATE 赋值列表是否含有子查询。
pub fn checkIfAssignmentListHasSubQuery(assignments: &[Assignment]) -> bool {
    assignments
        .iter()
        .any(|a| matches!(&a.value, Value::String(s) if s.starts_with("subquery:")))
}
/// 尝试将 UPDATE 编译为点更新快路径。
pub fn tryUpdatePointPlan(
    query: &FastQuery,
    assignments: &[Assignment],
    ignore_error: bool,
    privilege_ok: bool,
) -> Option<FastPlan> {
    if checkIfAssignmentListHasSubQuery(assignments) {
        return None;
    }
    let point = TryFastPlan(query, privilege_ok, 50_000)?;
    Some(buildPointUpdatePlan(point, &query.table, assignments, ignore_error).ok()?)
}
/// 组装 PointUpdatePlan。
pub fn buildPointUpdatePlan(
    point: FastPlan,
    table: &TableInfo,
    assignments: &[Assignment],
    ignore_error: bool,
) -> Result<FastPlan> {
    let ordered = buildOrderedList(table, assignments)?;
    Ok(FastPlan::Update(PointUpdatePlan {
        source: Box::new(point),
        table: table.clone(),
        assignments: assignments.to_vec(),
        ordered,
        ignore_error,
    }))
}
/// 按表列顺序整理赋值列表。
pub fn buildOrderedList(
    table: &TableInfo,
    assignments: &[Assignment],
) -> Result<Vec<(usize, Value)>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for assignment in assignments {
        let column = findCol(table, &assignment.column)
            .ok_or_else(|| BuilderError(format!("unknown column {}", assignment.column)))?;
        if column.generated {
            return Err(BuilderError(format!(
                "generated column {} cannot be assigned",
                column.name
            )));
        }
        if !seen.insert(column.offset) {
            return Err(BuilderError(format!(
                "column {} assigned twice",
                column.name
            )));
        }
        out.push((
            column.offset,
            getPointGetValue(column, &assignment.value)
                .ok_or_else(|| BuilderError("invalid assignment value".into()))?,
        ));
    }
    Ok(out)
}
/// 尝试将 DELETE 编译为点删除快路径。
pub fn tryDeletePointPlan(
    query: &FastQuery,
    ignore_error: bool,
    privilege_ok: bool,
) -> Option<FastPlan> {
    let plan = TryFastPlan(query, privilege_ok, 50_000)?;
    Some(buildPointDeletePlan(plan, &query.table, ignore_error))
}
/// 组装 PointDeletePlan。
pub fn buildPointDeletePlan(point: FastPlan, table: &TableInfo, ignore_error: bool) -> FastPlan {
    FastPlan::Delete(PointDeletePlan {
        source: Box::new(point),
        table: table.clone(),
        ignore_error,
    })
}
/// 按名在表中查找列元数据。
pub fn findCol<'a>(table: &'a TableInfo, name: &str) -> Option<&'a ColumnInfo> {
    table
        .columns
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
}
/// 将列元数据转为表达式列引用。
pub fn colInfoToColumn(column: &ColumnInfo, idx: usize) -> Expression {
    Expression {
        name: column.name.clone(),
        column: Some(idx),
        return_type: Some(column.field_type.clone()),
        ..Expression::default()
    }
}
/// 收集点查/DML 所需的 handle 列下标。
pub fn buildHandleCols(_db: &str, table: &TableInfo, _point: &FastPlan) -> Vec<usize> {
    table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.primary_key)
        .map(|(i, _)| i)
        .collect()
}
/// 返回哈希/键分区表的分区列名（若有）。
pub fn getHashOrKeyPartitionColumnName(table: &TableInfo) -> Option<String> {
    table
        .columns
        .iter()
        .find(|c| c.primary_key)
        .map(|c| c.name.clone())
}
/// 将 Predicate 转为通用 Expression 形式。
fn predicate_expr(predicate: &Predicate) -> Expression {
    Expression {
        name: match predicate {
            Predicate::Eq(_, _) => "eq",
            Predicate::NullSafeEq(_, _) => "null_eq",
            Predicate::In(_, _) => "in",
            Predicate::And(_) => "and",
            Predicate::Or(_) => "or",
            Predicate::Other(e) => &e.name,
            Predicate::False => "false",
        }
        .into(),
        ..Expression::default()
    }
}

/// 将 FastPlan 转为通用 PlanNode，便于嵌入计划树。
pub fn pointPlanToNode(plan: &FastPlan) -> PlanNode {
    let mut node = PlanNode::new(match plan {
        FastPlan::Point(_) => PlanKind::PointGet,
        FastPlan::Batch(_) => PlanKind::BatchPointGet,
        FastPlan::Update(_) => PlanKind::Other("Update".into()),
        FastPlan::Delete(_) => PlanKind::Other("Delete".into()),
    });
    node.stats.row_count = match plan {
        FastPlan::Batch(p) => p.handles.len().max(p.index_values.len()) as f64,
        _ => 1.0,
    };
    node
}
