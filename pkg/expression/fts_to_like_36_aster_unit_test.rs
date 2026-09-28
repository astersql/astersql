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

// FTS→ILIKE 及相关模块的聚合单元测试（task 700 / aster unit）。
//
// 覆盖：搜索串校验与转义、布尔/自然语言表达式语义、builtin 守卫、
// 函数分类特征、Grouping Sets 合并、时间辅助函数、表达式下推规则。

use std::sync::{Arc, Mutex};

use crate::Schema;
use crate::expression_files_36::function_traits as traits;
use crate::expression_files_36::grouping_sets::*;
use crate::expression_files_36::helper::*;
use crate::expression_files_36::infer_pushdown::*;
use crate::expression_files_36::*;
use chrono::{TimeZone, Utc};

/// 构造 VARCHAR 列表达式，便于拼装 MATCH 列列表。
fn col(id: i64, index: usize) -> Expression {
    Expression::column(id, index, FieldType::varchar())
}

/// 构造带 NotNull 的 BIGINT 列，供 Grouping Sets 相关测试使用。
fn grouping_column(id: i64) -> crate::Column {
    let mut field_type = *crate::types::NewFieldType(crate::mysql::TypeLonglong);
    field_type.SetFlag(crate::mysql::NotNullFlag);
    crate::Column {
        RetType: Some(field_type),
        UniqueID: id,
        Index: id as isize,
        ..Default::default()
    }
}

/// 将 grouping_column 装箱为 ExprBox。
fn grouping_expression(id: i64) -> crate::ExprBox {
    Box::new(grouping_column(id))
}

/// 校验/解析/转义与 Go 边界用例对齐：合法词、非法符号、布尔前缀、中文。
#[test]
fn fts_validator_parser_and_escape_match_go_edges() {
    for valid in ["", "MySQL tutorial", "abc123 mysql8", "你好"] {
        validate_fts_search_string_for_like_fallback(
            valid,
            FulltextSearchModifier::NaturalLanguage,
        )
        .unwrap();
    }
    for invalid in ["x-x", "MySQL,", "+word", "word*", "100%", "test_file"] {
        assert!(
            validate_fts_search_string_for_like_fallback(
                invalid,
                FulltextSearchModifier::NaturalLanguage
            )
            .is_err()
        );
    }
    for valid in ["+MySQL", "-MySQL", "+apple -cherry pie", "+你好"] {
        validate_fts_search_string_for_like_fallback(valid, FulltextSearchModifier::Boolean)
            .unwrap();
    }
    for invalid in ["+", "-", "xx-yy", "word*", ">word", "(word)"] {
        assert!(
            validate_fts_search_string_for_like_fallback(invalid, FulltextSearchModifier::Boolean)
                .is_err()
        );
    }

    assert_eq!(
        parse_fts_boolean_search_string("+apple -cherry pie"),
        vec![
            FtsSearchTerm::required("apple"),
            FtsSearchTerm::excluded("cherry"),
            FtsSearchTerm::optional("pie"),
        ]
    );
    assert_eq!(escape_fts_like_pattern(r"mix_%_\\all"), r"mix\_\%\_\\\\all");
}

/// 布尔模式与自然语言模式生成的谓词树，求值语义对齐 MySQL LIKE 回退。
#[test]
fn fts_boolean_and_natural_trees_preserve_mysql_fallback_semantics() {
    let columns = vec![col(1, 0), col(2, 1)];
    let required = build_fts_to_ilike_expression(
        &columns,
        "+apple -cherry pie",
        FulltextSearchModifier::Boolean,
    )
    .unwrap();
    assert_eq!(
        required.eval_bool(&[Some("APPLE tart"), None]).unwrap(),
        Some(true)
    );
    assert_eq!(
        required
            .eval_bool(&[Some("apple"), Some("cherry")])
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        required.eval_bool(&[Some("pie"), None]).unwrap(),
        Some(false)
    );

    let optional = build_fts_to_ilike_expression(
        &columns,
        "apple pie -cherry",
        FulltextSearchModifier::Boolean,
    )
    .unwrap();
    assert_eq!(
        optional.eval_bool(&[None, Some("PIE")]).unwrap(),
        Some(true)
    );
    assert_eq!(
        optional.eval_bool(&[None, Some("pie cherry")]).unwrap(),
        Some(false)
    );

    let only_excluded =
        build_fts_to_ilike_expression(&columns, "-apple -pie", FulltextSearchModifier::Boolean)
            .unwrap();
    assert_eq!(
        only_excluded.eval_bool(&[Some("other"), None]).unwrap(),
        Some(false)
    );

    let natural = build_fts_to_ilike_expression(
        &columns,
        "mysql tutorial",
        FulltextSearchModifier::NaturalLanguage,
    )
    .unwrap();
    assert_eq!(
        natural.eval_bool(&[None, Some("A MySQL guide")]).unwrap(),
        Some(true)
    );
}

/// builtin 入口：NULL 搜索串直通；多列与查询扩展被拒绝。
#[test]
fn fts_builtin_validation_keeps_null_and_modifier_guards() {
    let column = col(1, 0);
    let null_builtin = ScalarFunction::fts(
        Expression::constant(Datum::Null),
        vec![column.clone()],
        FulltextSearchModifier::NaturalLanguage,
    );
    assert_eq!(
        build_fts_to_ilike_expression_from_builtin(&null_builtin).unwrap(),
        Expression::constant(Datum::Null)
    );

    let multi = ScalarFunction::fts(
        Expression::constant(Datum::String("mysql".into())),
        vec![column.clone(), col(2, 1)],
        FulltextSearchModifier::NaturalLanguage,
    );
    assert!(
        build_fts_to_ilike_expression_from_builtin(&multi)
            .unwrap_err()
            .to_string()
            .contains("multi-column")
    );
    assert!(
        build_fts_to_ilike_expression(
            &[column],
            "mysql",
            FulltextSearchModifier::NaturalLanguageWithQueryExpansion,
        )
        .is_err()
    );
}

/// 函数分类表与优化器约定一致：不可缓存、不可折叠、延迟求值、分区函数等。
#[test]
fn function_traits_match_optimizer_classification() {
    assert!(traits::is_uncacheable("database"));
    assert!(traits::is_unfoldable("rand"));
    assert!(traits::is_illegal_generated_column_function("uuid"));
    assert!(traits::is_deferred("now", false));
    assert!(!traits::is_deferred("sysdate", false));
    assert!(traits::is_deferred("sysdate", true));
    assert!(traits::is_allowed_partition_function("to_days"));
    assert!(traits::is_allowed_partition_binary_op(
        traits::PartitionOp::IntDiv
    ));
    assert!(!traits::is_allowed_partition_unary_op(
        traits::PartitionOp::Mul
    ));
    assert!(traits::has_mutable_effect("setvar"));
    assert!(traits::is_boolean_function("regexp_like"));
    assert!(!traits::is_noop_function("anything"));
}

/// Grouping Sets：merge、target_one、去重/还原、rollup 与 distinct_size 与 Go 对齐。
#[test]
fn grouping_sets_merge_rollup_target_and_distinct_match_go() {
    let a = grouping_expression(1);
    let b = grouping_expression(2);
    let c = grouping_expression(3);
    let d = grouping_expression(4);

    let raw = GroupingSets(vec![
        GroupingSet(vec![GroupingExprs(vec![
            c.clone(),
            d.clone(),
            a.clone(),
            b.clone(),
        ])]),
        GroupingSet(vec![GroupingExprs(vec![b.clone()])]),
        GroupingSet(vec![GroupingExprs(vec![a.clone(), b.clone()])]),
    ]);
    let merged = raw.merge();
    assert_eq!(merged.0.len(), 1);
    assert_eq!(
        merged.0[0].0.iter().map(|e| e.0.len()).collect::<Vec<_>>(),
        vec![1, 2, 4]
    );

    let separate = GroupingSets(vec![
        GroupingSet(vec![GroupingExprs(vec![a.clone(), b.clone()])]),
        GroupingSet(vec![GroupingExprs(vec![c.clone()])]),
    ]);
    assert_eq!(separate.target_one(std::slice::from_ref(&d)), 0);
    assert_eq!(separate.target_one(std::slice::from_ref(&c)), 1);
    assert_eq!(separate.target_one(&[b.clone(), c.clone()]), -1);

    let (dedup, positions) =
        deduplicate_gby_expression(&[a.clone(), b.clone(), b.clone(), c.clone()]);
    assert_eq!(positions, vec![0, 1, 1, 2]);
    let restored = restore_gby_expression(
        &dedup
            .iter()
            .map(|expression| expression.as_column().unwrap().clone())
            .collect::<Vec<_>>(),
        &positions,
    );
    let rollup = rollup_grouping_sets(&restored);
    assert_eq!(rollup.0.len(), 5);
    let (size, gids, id_to_gids) = rollup.distinct_size_with_threshold(3);
    assert_eq!(size, 4);
    assert_eq!(gids.unwrap(), vec![0, 1, 2, 2, 3]);
    assert_eq!(id_to_gids.unwrap().get(&2).unwrap().len(), 2);
}

/// 仅对出现在 expand/rollup 中的列清除 NotNull；无关列保持原可空性。
#[test]
fn grouping_sets_adjust_nullability_only_for_expand_columns() {
    let gss = rollup_grouping_sets(&[grouping_expression(1), grouping_expression(2)]);
    let mut schema: Schema = crate::NewSchema(vec![
        grouping_column(1),
        grouping_column(2),
        grouping_column(3),
    ]);
    adjust_nullability_from_grouping_sets(&gss, &mut schema);
    assert_eq!(
        schema
            .Columns
            .iter()
            .map(|column| {
                column.RetType.as_ref().unwrap().GetFlag() & crate::mysql::NotNullFlag != 0
            })
            .collect::<Vec<_>>(),
        vec![false, false, true]
    );
}

/// CURRENT_TIMESTAMP 精度校验、时区换算与非法时间解析与 Go helper 一致。
#[test]
fn helper_timestamp_validation_parsing_and_timezone_match_go() {
    let field = TimeFieldType { decimal: 3 };
    assert!(is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![3]),
        Some(&field)
    ));
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![1]),
        Some(&field)
    ));
    assert!(!is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![]),
        Some(&field)
    ));
    assert!(is_valid_current_timestamp_expr(
        &TimeExpr::current_timestamp(vec![]),
        None
    ));

    let instant = Utc.timestamp_opt(1_234, 987_654_321).unwrap();
    let utc = TimeContext::new(instant, chrono_tz::UTC);
    let current = get_time_current_timestamp(&utc, TimeType::Timestamp, 3).unwrap();
    assert_eq!(current.to_string(), "1970-01-01 00:20:34.987");

    let shanghai = TimeContext::new(instant, chrono_tz::Asia::Shanghai);
    let current = get_time_value(
        &shanghai,
        TimeInput::Text("CURRENT_TIMESTAMP".into()),
        TimeType::Timestamp,
        0,
        None,
    )
    .unwrap();
    assert_eq!(current.to_string(), "1970-01-01 08:20:34");
    assert_eq!(
        get_time_value(&utc, TimeInput::Integer(0), TimeType::Timestamp, 0, None)
            .unwrap()
            .to_string(),
        "0000-00-00 00:00:00"
    );
    assert!(
        get_time_value(
            &utc,
            TimeInput::Text("2012-13-12 00:00:00".into()),
            TimeType::Timestamp,
            0,
            None
        )
        .is_err()
    );
    assert_eq!(
        get_time_value(&utc, TimeInput::Null, TimeType::Timestamp, 0, None).unwrap(),
        TimeValue::Null
    );
}

/// 下推：黑名单、警告收集、FTS 修饰符与 ENUM 列对 TiFlash 的限制。
#[test]
fn pushdown_store_rules_blacklist_warnings_and_order_match_go() {
    clear_pushdown_blacklist();
    let normal = Arc::new(Mutex::new(Vec::new()));
    let extra = Arc::new(Mutex::new(Vec::new()));
    let ctx = PushDownContext::new(false, Some(normal.clone()), Some(extra.clone()), 1024);

    let plus = Expression::scalar(
        "plus",
        Signature::Generic("PlusInt".into()),
        vec![
            Expression::constant(Datum::Int(1)),
            Expression::constant(Datum::Int(2)),
        ],
        FieldType::integer(),
    );
    let unsupported = Expression::scalar(
        "made_up",
        Signature::Generic("MadeUp".into()),
        vec![],
        FieldType::integer(),
    );
    let (pushed, remained) =
        push_down_exprs(&ctx, vec![plus.clone(), unsupported], StoreType::TiKV);
    assert_eq!(pushed, vec![plus.clone()]);
    assert_eq!(remained.len(), 1);
    assert_eq!(extra.lock().unwrap().len(), 1);
    assert!(normal.lock().unwrap().is_empty());

    replace_pushdown_blacklist([("plus".to_string(), StoreType::TiKV.mask())]);
    assert!(!can_expr_push_down(&ctx, &plus, StoreType::TiKV, false));
    assert!(can_expr_push_down(&ctx, &plus, StoreType::TiFlash, false));

    let fts_natural = Expression::ScalarFunction(ScalarFunction::fts(
        Expression::constant(Datum::String("mysql".into())),
        vec![col(1, 0)],
        FulltextSearchModifier::NaturalLanguage,
    ));
    let fts_boolean = Expression::ScalarFunction(ScalarFunction::fts(
        Expression::constant(Datum::String("mysql".into())),
        vec![col(1, 0)],
        FulltextSearchModifier::Boolean,
    ));
    assert!(can_expr_push_down(
        &ctx,
        &fts_natural,
        StoreType::TiFlash,
        false
    ));
    assert!(!can_expr_push_down(
        &ctx,
        &fts_boolean,
        StoreType::TiFlash,
        false
    ));

    let enum_col = Expression::column(9, 0, FieldType::enum_type());
    assert!(!can_expr_push_down(
        &ctx,
        &enum_col,
        StoreType::TiFlash,
        false
    ));
    assert!(can_expr_push_down(
        &ctx,
        &enum_col,
        StoreType::TiFlash,
        true
    ));
    clear_pushdown_blacklist();
}
