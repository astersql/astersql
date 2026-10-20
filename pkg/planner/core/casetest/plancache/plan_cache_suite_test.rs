// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 语句特性相对会话级计划缓存开关的可缓存性用例。
//
// `PlanCacheStmt` 收集到的子查询、参数化 LIMIT 等特征，会对照
// `PlanCacheKeyContext` 中的会话开关（如 `enable_plan_cache_for_subquery`、
// `enable_plan_cache_for_parameterized_limit`）决定是否可缓存；关闭对应开关时
// `NewPlanCacheKey` 应返回 `cacheable=false` 并给出原因字符串。

/// 真实 prepared EXECUTE 的 warning 生命周期：执行产生 warning，下一条成功执行
/// 后 warning 集合按语句边界重置，不会污染后续 plan-cache 断言。
#[test]
fn prepared_execution_warnings_are_statement_scoped() {
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;
    use astersql_testkit::{Rows, TestKit};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "prepare warn_stmt from 'select tidb_decode_key(?)'",
        Vec::new(),
    );
    testkit.MustExec("set @key = 'not-a-key'", Vec::new());
    testkit.MustQuery("execute warn_stmt using @key", Vec::new());
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(vec![vec!["Warning", "1105", "invalid key: not-a-key"]]);

    testkit
        .MustQuery("select 1", Vec::new())
        .Check(Rows(&["1"]));
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(Rows(&[]));
}

/// 验证：子查询与参数化 LIMIT 开关逐级放开后，语句才变为可缓存。
#[test]
fn statement_features_obey_plan_cache_session_switches() {
    use astersql_planner_core::{
        NewPlanCacheKey, PlanCacheKeyContext, PlanCacheLimit, PlanCacheStmt, ast,
    };

    // 收集：带参数化 count 的 LIMIT，并标记含有子查询特征。
    let mut statement =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from t limit ?");
    statement.SchemaVersion = 9;
    statement.CollectPlanCacheStmtInfo(
        vec![PlanCacheLimit {
            count: 20,
            count_is_parameter: true,
            ..Default::default()
        }],
        true,
        Vec::new(),
    );

    // 默认上下文：子查询未启用 → 不可缓存。
    let disabled = NewPlanCacheKey(&PlanCacheKeyContext::default(), &statement).unwrap();
    assert!(!disabled.cacheable);
    assert!(disabled.reason.contains("subquery"));

    // 仅开启子查询缓存：仍因参数化 LIMIT 不可缓存。
    let subquery_enabled = PlanCacheKeyContext {
        enable_plan_cache_for_subquery: true,
        ..Default::default()
    };
    let limit_disabled = NewPlanCacheKey(&subquery_enabled, &statement).unwrap();
    assert!(!limit_disabled.cacheable);
    assert!(limit_disabled.reason.contains("param_limit"));

    // 两个开关都打开：语句可缓存。
    let all_enabled = PlanCacheKeyContext {
        enable_plan_cache_for_subquery: true,
        enable_plan_cache_for_parameterized_limit: true,
        ..Default::default()
    };
    assert!(NewPlanCacheKey(&all_enabled, &statement).unwrap().cacheable);
}

#[test]
fn coalesce_plan_cache_update() {
    use astersql_parser_ast::{Assignment, ColumnName, ExprNode, NewCIStr};
    use astersql_planner_core::{
        CoalesceColumnType, CoalesceParamMarkerOffsets, NewInstancePlanCache, NewLRUPlanCache,
        NewPlanCacheValueForTest, SupportedCoalesceAssignment,
    };
    use astersql_types::metadata::{FieldType, mysql};

    fn field_type(kind: u8, precision: isize, scale: isize, flags: usize) -> FieldType {
        let mut field_type = FieldType::default();
        field_type.SetType(kind);
        field_type.SetFlen(precision);
        field_type.SetDecimal(scale);
        field_type.SetFlag(flags);
        field_type
    }
    fn column(name: &str) -> ColumnName {
        ColumnName {
            Name: NewCIStr(name),
            ..Default::default()
        }
    }
    fn coalesce(first: ExprNode, second: ExprNode) -> ExprNode {
        ExprNode::Function(NewCIStr(""), NewCIStr("coalesce"), vec![first, second])
    }

    let decimal = field_type(mysql::TypeNewDecimal, 20, 8, 0);
    let decimal_other_precision = field_type(mysql::TypeNewDecimal, 19, 8, 0);
    let decimal_other_scale = field_type(mysql::TypeNewDecimal, 20, 7, 0);
    let signed_bigint = field_type(mysql::TypeLonglong, 20, 0, 0);
    let unsigned_bigint = field_type(mysql::TypeLonglong, 20, 0, mysql::UnsignedFlag);
    let nullable_bigint = field_type(mysql::TypeLonglong, 20, 0, 0);
    let not_null_bigint = field_type(mysql::TypeLonglong, 20, 0, mysql::NotNullFlag);
    let signed_int = field_type(mysql::TypeLong, 11, 0, 0);
    let string = field_type(mysql::TypeVarchar, 100, -1, 0);
    let columns = vec![
        CoalesceColumnType::new("d", decimal.clone()),
        CoalesceColumnType::new("other_precision", decimal_other_precision),
        CoalesceColumnType::new("other_scale", decimal_other_scale),
        CoalesceColumnType::new("signed_bigint", signed_bigint.clone()),
        CoalesceColumnType::new("unsigned_bigint", unsigned_bigint),
        CoalesceColumnType::new("nullable_bigint", nullable_bigint),
        CoalesceColumnType::new("not_null_bigint", not_null_bigint),
        CoalesceColumnType::new("signed_int", signed_int),
        CoalesceColumnType::new("s", string),
    ];
    let valid = Assignment {
        Column: column("d"),
        Expr: coalesce(
            ExprNode::DecimalValue("1.23456789".to_owned()),
            ExprNode::Column(column("d")),
        ),
    };
    assert!(SupportedCoalesceAssignment(&valid, &columns));
    assert!(SupportedCoalesceAssignment(
        &Assignment {
            Column: column("signed_bigint"),
            Expr: coalesce(
                ExprNode::IntValue(1),
                coalesce(
                    ExprNode::IntValue(2),
                    ExprNode::Column(column("signed_bigint")),
                ),
            ),
        },
        &columns,
    ));

    for invalid in [
        Assignment {
            Column: column("d"),
            Expr: coalesce(
                ExprNode::StringValue("abc".to_owned(), "utf8mb4", "utf8mb4_bin"),
                ExprNode::Column(column("d")),
            ),
        },
        Assignment {
            Column: column("d"),
            Expr: coalesce(
                ExprNode::DecimalValue("1.2".to_owned()),
                ExprNode::Column(column("other_precision")),
            ),
        },
        Assignment {
            Column: column("d"),
            Expr: coalesce(
                ExprNode::DecimalValue("1.2".to_owned()),
                ExprNode::Column(column("other_scale")),
            ),
        },
        Assignment {
            Column: column("signed_bigint"),
            Expr: coalesce(
                ExprNode::IntValue(1),
                ExprNode::Column(column("unsigned_bigint")),
            ),
        },
        Assignment {
            Column: column("signed_bigint"),
            Expr: coalesce(
                ExprNode::IntValue(1),
                ExprNode::Column(column("signed_int")),
            ),
        },
        Assignment {
            Column: column("not_null_bigint"),
            Expr: coalesce(
                ExprNode::IntValue(1),
                ExprNode::Column(column("nullable_bigint")),
            ),
        },
        Assignment {
            Column: column("s"),
            Expr: coalesce(
                ExprNode::DecimalValue("1.2".to_owned()),
                ExprNode::Column(column("s")),
            ),
        },
    ] {
        assert!(!SupportedCoalesceAssignment(&invalid, &columns));
    }
    assert!(!SupportedCoalesceAssignment(&valid, &[]));

    let parameterized = Assignment {
        Column: column("d"),
        Expr: coalesce(
            ExprNode::ParamMarker(30),
            coalesce(ExprNode::ParamMarker(10), ExprNode::Column(column("d"))),
        ),
    };
    assert_eq!(CoalesceParamMarkerOffsets(&parameterized), vec![10, 30]);

    let signature = |precision, scale| {
        vec![
            field_type(mysql::TypeNewDecimal, precision, scale, 0),
            field_type(mysql::TypeNewDecimal, 20, 8, 0),
        ]
    };
    let signatures = [signature(10, 8), signature(7, 5), signature(10, 5)];

    let lru = NewLRUPlanCache(10, 0.0, 0);
    for (index, types) in signatures.iter().enumerate() {
        assert!(lru.Get("same-key", types).is_none());
        let mut value = NewPlanCacheValueForTest(100);
        value.ParamTypes = types.clone();
        value.SetExactDecimalParamOffsets(vec![0]);
        lru.Put("same-key".to_owned(), value);
        assert_eq!(lru.Size(), index + 1);
    }
    for types in &signatures {
        assert!(lru.Get("same-key", types).is_some());
    }

    let instance = NewInstancePlanCache(1 << 20, 1 << 20);
    for (index, types) in signatures.iter().enumerate() {
        assert!(instance.Get("same-key", types).is_none());
        let mut value = NewPlanCacheValueForTest(100);
        value.ParamTypes = types.clone();
        value.SetExactDecimalParamOffsets(vec![0]);
        assert!(instance.Put("same-key".to_owned(), value));
        assert_eq!(instance.Size(), index + 1);
    }
    for types in &signatures {
        assert!(instance.Get("same-key", types).is_some());
    }

    let relaxed = NewLRUPlanCache(10, 0.0, 0);
    let mut value = NewPlanCacheValueForTest(100);
    value.ParamTypes = signatures[0].clone();
    relaxed.Put("relaxed".to_owned(), value);
    assert!(relaxed.Get("relaxed", &signatures[1]).is_some());
}
