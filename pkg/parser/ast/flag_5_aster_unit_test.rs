// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// 表达式标志位（flag）自底向上传播与函数还原的单元测试。
//
// “标志位”用于标记表达式是否含常量、参数占位符、聚合/窗口函数、子查询等，
// 供后续语义分析与优化器剪枝使用。本文件同时覆盖部分函数特殊形态还原。

use crate::functions::flag::*;
use crate::functions::*;

/// 构造带指定初始 flag 的叶子表达式。
fn raw(sql: &str, flag: u64) -> FlagExpr {
    FlagExpr::Leaf {
        sql: sql.into(),
        flag,
    }
}

/// CASE/聚合/窗口节点经 set_flag 后，标志位应按子节点按位或并叠加自身类型位。
#[test]
fn flag_propagation_matches_go_cases() {
    let mut expr = FlagExpr::Case {
        value: Some(Box::new(raw("1", FLAG_CONSTANT))),
        when_clauses: vec![(
            FlagExpr::Binary(
                Box::new(raw("a", FLAG_HAS_REFERENCE)),
                Box::new(raw("1", FLAG_CONSTANT)),
            ),
            raw("1", FLAG_CONSTANT),
        )],
        else_clause: Some(Box::new(raw("0", FLAG_CONSTANT))),
        flag: 0,
    };
    set_flag(&mut expr);
    assert_eq!(expr.get_flag(), FLAG_CONSTANT | FLAG_HAS_REFERENCE);

    let mut aggregate = FlagExpr::Aggregate {
        args: vec![raw("a", FLAG_HAS_REFERENCE)],
        flag: 0,
    };
    set_flag(&mut aggregate);
    assert!(has_agg_flag(&aggregate));
    assert_eq!(
        aggregate.get_flag(),
        FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_REFERENCE
    );

    let mut window = FlagExpr::Window {
        args: vec![aggregate],
        flag: 0,
    };
    set_flag(&mut window);
    assert!(has_window_flag(&window));
    assert!(has_agg_flag(&window));
}

/// IN 列表、参数占位符、子查询与用户变量标志应正确合并。
#[test]
fn variable_param_and_container_flags_match_go() {
    let mut expr = FlagExpr::PatternIn {
        expr: Box::new(raw("a", FLAG_HAS_REFERENCE)),
        list: vec![FlagExpr::ParamMarker { flag: 0 }, raw("1", FLAG_CONSTANT)],
        sel: Some(Box::new(FlagExpr::Subquery { flag: 0 })),
        flag: 0,
    };
    set_flag(&mut expr);
    assert_eq!(
        expr.get_flag(),
        FLAG_HAS_REFERENCE | FLAG_HAS_PARAM_MARKER | FLAG_CONSTANT | FLAG_HAS_SUBQUERY
    );

    let mut variable = FlagExpr::Variable {
        value: Some(Box::new(expr)),
        flag: 0,
    };
    set_flag(&mut variable);
    assert_eq!(variable.get_flag() & FLAG_HAS_VARIABLE, FLAG_HAS_VARIABLE);
}

/// DATE_ADD/EXTRACT/CONVERT/MEMBER OF 等特殊函数形态还原。
#[test]
fn function_restore_special_forms_match_go() {
    let date_add = FuncCallExpr::keyword("date_add", vec![expr("d"), expr("1"), expr("DAY")]);
    assert_eq!(date_add.restore().unwrap(), "DATE_ADD(d, INTERVAL 1 DAY)");
    assert_eq!(
        FuncCallExpr::keyword("extract", vec![expr("YEAR"), expr("d")])
            .restore()
            .unwrap(),
        "EXTRACT(YEAR FROM d)"
    );
    assert_eq!(
        FuncCallExpr::keyword("convert", vec![expr("a"), expr("utf8mb4")])
            .restore()
            .unwrap(),
        "CONVERT(a USING utf8mb4)"
    );
    assert_eq!(
        FuncCallExpr::keyword(JSON_MEMBER_OF, vec![expr("1"), expr("j")])
            .restore()
            .unwrap(),
        "1 MEMBER OF (j)"
    );
    assert!(
        FuncCallExpr::keyword(JSON_MEMBER_OF, vec![expr("1")])
            .restore()
            .is_err()
    );
}

/// GROUP_CONCAT、窗口函数、CAST 与时间单位/TRIM 方向等辅助类型。
#[test]
fn aggregate_window_cast_and_units_match_go() {
    let aggregate = AggregateFuncExpr {
        name: "group_concat".into(),
        args: vec![expr("a"), expr("','")],
        distinct: true,
        order_by: Some("ORDER BY a".into()),
    };
    assert_eq!(
        aggregate.restore().unwrap(),
        "GROUP_CONCAT(DISTINCT a ORDER BY a SEPARATOR ',')"
    );

    let window = WindowFuncExpr {
        name: "nth_value".into(),
        args: vec![expr("a"), expr("2")],
        distinct: true,
        ignore_null: true,
        from_last: true,
        spec: "(PARTITION BY b)".into(),
    };
    assert_eq!(
        window.restore().unwrap(),
        "NTH_VALUE(DISTINCT a, 2) FROM LAST IGNORE NULLS OVER (PARTITION BY b)"
    );

    assert_eq!(
        FuncCastExpr::new(expr("a"), "SIGNED", CastFunctionType::Cast)
            .restore()
            .unwrap(),
        "CAST(a AS SIGNED)"
    );
    assert_eq!(
        TimeUnitType::Day.duration().unwrap(),
        std::time::Duration::from_secs(86_400)
    );
    assert!(TimeUnitType::Month.duration().is_err());
    assert!(TimeUnitType::MinuteSecond.duration().is_err());
    assert_eq!(TrimDirectionType::Leading.as_str(), "LEADING");
    assert_eq!(GetFormatSelectorType::Datetime.as_str(), "DATETIME");
}
