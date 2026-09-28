// Copyright 2026 AsterSQL.
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

// Plan Cache 工具的 Aster 单元测试。
//
// 覆盖 Point Get 执行器缓存槽位、PlanCacheStmt 参数标记与快照 TS 求值、
// 缓存键确定性、运行时统计原子更新、参数类型兼容性、安全 Point Get 路径
// 四场景，以及按名解析 Prepared 语句后缓存 ID 的行为。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use super::*;
use expression_dependency::{
    Coercibility, CollationInfo, EvalContext, Expression, Repertoire, builtinFunc, collationInfo,
};

#[test]
/// 验证三个缓存槽（列信息/执行器/快路径计划）类型隔离，Reset 清空全部。
fn point_get_cache_keeps_each_payload_typed_and_resets_all_slots() {
    let cache = PointGetExecutorCache::<Arc<String>, u64, Vec<u8>>::default();
    let column = Arc::new("id".to_owned());

    cache.CacheColumnInfos(vec![Arc::clone(&column)]);
    cache.CacheExecutor(42);
    cache.CacheFastPlan(vec![1, 2, 3]);

    assert!(Arc::ptr_eq(&cache.CachedColumnInfos().unwrap()[0], &column));
    assert_eq!(cache.CloneFastPlan(), Some(vec![1, 2, 3]));
    assert_eq!(cache.TakeExecutor(), Some(42));

    cache.Reset();
    assert!(cache.CachedColumnInfos().is_none());
    assert!(cache.CloneFastPlan().is_none());
}

#[test]
/// 参数标记按 offset 排序并重置 in_execute；收集 Limit/子查询元数据；求值快照 TS。
fn plan_cache_stmt_owns_sorted_markers_metadata_and_snapshot_evaluator() {
    let mut statement = PlanCacheStmt::<String>::new(
        ast::misc::Prepared {
            stmt: "select * from t where id = ?".to_owned(),
            stmt_type: "Select".to_owned(),
        },
        "select * from t where id = ?",
    );
    statement.Params = ExtractAndSortParamMarkers(vec![
        PlanCacheParamMarker {
            offset: 30,
            in_execute: true,
            ..Default::default()
        },
        PlanCacheParamMarker {
            offset: 10,
            in_execute: true,
            ..Default::default()
        },
    ]);
    statement.CollectPlanCacheStmtInfo(
        vec![PlanCacheLimit {
            count: 10,
            ..Default::default()
        }],
        true,
        Vec::new(),
    );
    statement.SnapshotTSEvaluator = Some(Arc::new(|_| Ok(99)));

    assert_eq!(statement.Params[0].offset, 10);
    assert_eq!(statement.Params[1].order, 1);
    assert!(!statement.Params[0].in_execute);
    assert_eq!(statement.Limits()[0].count, 10);
    assert!(statement.HasSubquery());
    assert_eq!(statement.EvaluateSnapshotTS(&()).unwrap(), Some(99));
}

#[test]
/// 相同上下文与排序后的 RelateVersion 应生成相同缓存键；过大 Limit 不可缓存。
fn plan_cache_key_is_deterministic_and_includes_binding_limit_and_sorted_maps() {
    let mut first =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from t limit ?");
    first.SchemaVersion = 9;
    first.RelateVersion.insert(20, 2);
    first.RelateVersion.insert(10, 1);
    first.CollectPlanCacheStmtInfo(
        vec![PlanCacheLimit {
            count: 8,
            count_is_parameter: true,
            ..Default::default()
        }],
        false,
        Vec::new(),
    );
    let mut second =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from t limit ?");
    second.SchemaVersion = 9;
    second.RelateVersion.insert(10, 1);
    second.RelateVersion.insert(20, 2);
    second.CollectPlanCacheStmtInfo(first.Limits().to_vec(), false, Vec::new());

    let context = PlanCacheKeyContext {
        current_database: "test".to_owned(),
        statement_read_only: true,
        enable_plan_cache_for_parameterized_limit: true,
        enable_plan_cache_for_subquery: true,
        dirty_table_ids: vec![7, 3],
        ..Default::default()
    };
    let binding = MatchedPlanCacheBinding {
        bind_sql: "select /*+ use_index(t,primary) */ * from t limit ?".to_owned(),
    };
    let first_key = NewPlanCacheKeyWithMatchedBinding(&context, &first, Some(&binding)).unwrap();
    let second_key = NewPlanCacheKeyWithMatchedBinding(&context, &second, Some(&binding)).unwrap();

    assert!(first_key.cacheable);
    assert_eq!(first_key.binding, binding.bind_sql);
    assert_eq!(first_key.key, second_key.key);

    let mut oversized = first;
    oversized.CollectPlanCacheStmtInfo(
        vec![PlanCacheLimit {
            count: MaxCacheableLimitCount as u64 + 1,
            count_is_parameter: true,
            ..Default::default()
        }],
        false,
        Vec::new(),
    );
    let result = NewPlanCacheKey(&context, &oversized).unwrap();
    assert!(!result.cacheable);
    assert_eq!(result.reason, "limit count is too large");
}

#[test]
/// PlanCacheValue 内存占用缓存与 RuntimeInfo 原子累加。
fn plan_cache_value_runtime_and_memory_are_atomic_and_cached() {
    let value = NewPlanCacheValueForTest(128);
    let initial_memory = value.MemoryUsage();
    assert!(initial_memory >= 128);
    assert_eq!(value.MemoryUsage(), initial_memory);

    value.UpdateRuntimeInfo(3, 5, 7);
    value.UpdateRuntimeInfo(11, 13, 17);
    let (executions, processed, total, latency, last_used) = value.RuntimeInfo();
    assert_eq!((executions, processed, total, latency), (2, 14, 18, 24));
    assert!(last_used >= std::time::UNIX_EPOCH);
}

#[test]
/// 参数类型兼容：Varchar↔VarString、无符号位、DECIMAL 精度不可变宽。
fn parameter_type_compatibility_matches_varchar_unsigned_and_decimal_rules() {
    use types_dependency::metadata::{FieldType, ast_types, mysql};

    let mut varchar = FieldType::default();
    varchar.SetType(mysql::TypeVarchar);
    let mut var_string = varchar.clone();
    var_string.SetType(mysql::TypeVarString);
    assert!(CheckTypesCompatibility4PC(&[varchar], &[var_string]));

    let mut signed = FieldType::default();
    signed.SetType(mysql::TypeLonglong);
    let mut unsigned = signed.clone();
    unsigned.SetFlag(unsigned.GetFlag() | mysql::UnsignedFlag);
    assert_eq!(signed.EvalType(), ast_types::ETInt);
    assert!(!CheckTypesCompatibility4PC(&[signed], &[unsigned]));

    let mut cached_decimal = FieldType::default();
    cached_decimal.SetType(mysql::TypeNewDecimal);
    cached_decimal.SetFlen(12);
    cached_decimal.SetDecimal(4);
    let mut smaller_decimal = cached_decimal.clone();
    smaller_decimal.SetFlen(10);
    smaller_decimal.SetDecimal(2);
    assert!(CheckTypesCompatibility4PC(
        &[cached_decimal.clone()],
        &[smaller_decimal]
    ));
    let mut wider_decimal = cached_decimal.clone();
    wider_decimal.SetFlen(13);
    assert!(!CheckTypesCompatibility4PC(
        &[cached_decimal],
        &[wider_decimal]
    ));
}

#[test]
/// 安全 Point Get 四场景：等值、IN（需 fix）、OR 析取、等值+IN 组合。
fn safe_point_get_accepts_all_four_exact_scenarios_and_rejects_fix_gating() {
    let context = exprstatic_dependency::NewExprContext(Vec::new());

    let mut equality = access_path_with_ranges(1, 1);
    equality.AccessConds.push(scalar_function(
        &context,
        ast::EQ,
        vec![constant(1), constant(2)],
    ));
    assert!(IsSafePointGetPath4PlanCache(false, &equality));

    let mut in_list = access_path_with_ranges(1, 2);
    in_list.AccessConds.push(scalar_function(
        &context,
        ast::In,
        vec![constant(1), constant(2), constant(3)],
    ));
    assert!(!IsSafePointGetPath4PlanCache(false, &in_list));
    assert!(IsSafePointGetPath4PlanCache(true, &in_list));

    let first_eq = scalar_function(&context, ast::EQ, vec![constant(1), constant(2)]);
    let second_eq = scalar_function(&context, ast::EQ, vec![constant(1), constant(3)]);
    let mut disjunction = access_path_with_ranges(1, 2);
    disjunction.AccessConds.push(scalar_function(
        &context,
        ast::functions::LogicOr,
        vec![first_eq, second_eq],
    ));
    assert!(IsSafePointGetPath4PlanCacheScenario3(&disjunction));

    let mut equality_and_in = access_path_with_ranges(2, 2);
    equality_and_in.AccessConds.push(scalar_function(
        &context,
        ast::EQ,
        vec![constant(1), constant(2)],
    ));
    equality_and_in.AccessConds.push(scalar_function(
        &context,
        ast::In,
        vec![constant(1), constant(2), constant(3)],
    ));
    assert!(IsSafePointGetPath4PlanCacheScenario4(&equality_and_in));
}

/// 构造带指定函数名与参数的标量函数表达式，供访问条件测试使用。
fn scalar_function(
    context: &dyn expression_dependency::BuildContext,
    name: &str,
    arguments: Vec<expression_dependency::ExprBox>,
) -> expression_dependency::ExprBox {
    use types_dependency::metadata::{FieldType, mysql};

    let mut return_type = FieldType::default();
    return_type.SetType(mysql::TypeLonglong);
    let mut function = expression_dependency::NewValuesFunc(context, 0, return_type.clone());
    function.FuncName = ast::NewCIStr(name);
    function.RetType = Some(return_type.clone());
    function.Function = Box::new(PredicateBuiltin::new(arguments, return_type));
    Box::new(function)
}

/// 测试用谓词内建函数，实现 CollationInfo 与 builtinFunc。
struct PredicateBuiltin {
    collation: collationInfo,
    arguments: Vec<Box<dyn Expression>>,
    return_type: types_dependency::metadata::FieldType,
    pb_code: i32,
    collator: Box<dyn expression_dependency::collate::Collator>,
}

impl PredicateBuiltin {
    fn new(
        arguments: Vec<Box<dyn Expression>>,
        return_type: types_dependency::metadata::FieldType,
    ) -> Self {
        let collation = collationInfo::default();
        collation.SetCoercibility(expression_dependency::CoercibilityNumeric);
        Self {
            collation,
            arguments,
            return_type,
            pb_code: 0,
            collator: expression_dependency::collate::GetBinaryCollator(),
        }
    }
}

impl CollationInfo for PredicateBuiltin {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }

    fn Coercibility(&self) -> Coercibility {
        self.collation.Coercibility()
    }

    fn SetCoercibility(&self, value: Coercibility) {
        self.collation.SetCoercibility(value)
    }

    fn Repertoire(&self) -> Repertoire {
        self.collation.Repertoire()
    }

    fn SetRepertoire(&mut self, value: Repertoire) {
        self.collation.SetRepertoire(value)
    }

    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }

    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.collation.SetCharsetAndCollation(charset, collation)
    }

    fn IsExplicitCharset(&self) -> bool {
        self.collation.IsExplicitCharset()
    }

    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.collation.SetExplicitCharset(explicit)
    }
}

impl builtinFunc for PredicateBuiltin {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        self.arguments
            .iter()
            .all(|argument| argument.SafeToShareAcrossSession())
    }

    fn getArgs(&self) -> &[Box<dyn Expression>] {
        &self.arguments
    }

    fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
        &mut self.arguments
    }

    fn equal(&self, context: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|other| {
            self.return_type == other.return_type
                && self.arguments.len() == other.arguments.len()
                && self
                    .arguments
                    .iter()
                    .zip(&other.arguments)
                    .all(|(left, right)| left.Equal(context, right.as_ref()))
        })
    }

    fn getRetTp(&self) -> &types_dependency::metadata::FieldType {
        &self.return_type
    }

    fn setPbCode(&mut self, code: i32) {
        self.pb_code = code
    }

    fn PbCode(&self) -> i32 {
        self.pb_code
    }

    fn setCollator(&mut self, collator: Box<dyn expression_dependency::collate::Collator>) {
        self.collator = collator
    }

    fn collator(&self) -> &dyn expression_dependency::collate::Collator {
        self.collator.as_ref()
    }

    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(Self {
            collation: self.collation.clone(),
            arguments: self
                .arguments
                .iter()
                .map(|argument| argument.CloneExpr())
                .collect(),
            return_type: self.return_type.clone(),
            pb_code: self.pb_code,
            collator: self.collator.Clone(),
        })
    }

    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .arguments
                .iter()
                .map(|argument| argument.MemoryUsage())
                .sum::<i64>()
    }

    fn vectorized(&self) -> bool {
        true
    }
}

/// 构造 Int64 常量表达式。
fn constant(value: i64) -> expression_dependency::ExprBox {
    Box::new(expression_dependency::NewInt64Const(value))
}

/// 构造含指定宽度与条数 Ranges 的 AccessPath。
fn access_path_with_ranges(width: usize, count: usize) -> planner_util_dependency::AccessPath {
    let mut path = planner_util_dependency::AccessPath::default();
    for _ in 0..count {
        path.Ranges.push(Default::default());
        path.Ranges.last_mut().unwrap().LowVal = std::iter::repeat_with(Default::default)
            .take(width)
            .collect();
    }
    path
}

#[derive(Default)]
/// 内存版 Prepared 语句存储，实现 PreparedStatementStore。
struct PreparedStore {
    statements: HashMap<u32, Arc<PlanCacheStmt>>,
    names: HashMap<String, u32>,
}

impl PreparedStatementStore for PreparedStore {
    fn GetByID(&self, statement_id: u32) -> Option<Arc<PlanCacheStmt>> {
        self.statements.get(&statement_id).cloned()
    }

    fn GetIDByName(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }
}

#[test]
/// 首次按名解析写入 prepared_statement_id，之后即使清掉名字映射仍可按 ID 取回。
fn get_prepared_stmt_resolves_name_once_then_uses_cached_id() {
    let statement = Arc::new(PlanCacheStmt::new(
        ast::misc::Prepared::default(),
        "select 1",
    ));
    let mut store = PreparedStore::default();
    store.statements.insert(7, Arc::clone(&statement));
    store.names.insert("query".to_owned(), 7);
    let mut execute = ExecutePreparedStatement {
        name: "query".to_owned(),
        ..Default::default()
    };

    let resolved = GetPreparedStmt(&mut execute, &store).unwrap();
    assert!(Arc::ptr_eq(&resolved, &statement));
    assert_eq!(execute.prepared_statement_id, Some(7));
    store.names.clear();
    assert!(Arc::ptr_eq(
        &GetPreparedStmt(&mut execute, &store).unwrap(),
        &statement
    ));
}
