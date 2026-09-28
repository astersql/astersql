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

// There is no working SQL parser wired into this crate yet, so unlike the Go
// `flag_test.go` (which parses real SQL and calls `ast.SetFlag`), the cases below build
// `FlagExpr` trees by hand that are structurally equivalent to each Go test expression and
// then run the same `crate::flag::SetFlag`/`HasAggFlag` propagation the production code uses.

// 表达式标志位（flag）自底向上传播的单元测试。
//
// 因本 crate 尚未接入可用 SQL 解析器，用例手工构造与 Go `flag_test.go`
// 结构等价的 `FlagExpr` 树，再调用 `SetFlag`/`HasAggFlag` 验证标志合并。
// 标志位用于标记常量、参数占位符、函数、列引用、聚合、子查询、变量等。

use crate::flag::{
    FLAG_CONSTANT, FLAG_HAS_AGGREGATE_FUNC, FLAG_HAS_DEFAULT, FLAG_HAS_FUNC, FLAG_HAS_PARAM_MARKER,
    FLAG_HAS_REFERENCE, FLAG_HAS_SUBQUERY, FLAG_HAS_VARIABLE, FLAG_HAS_WINDOW_FUNC, FlagExpr,
    HasAggFlag, HasWindowFlag, SetFlag,
};

/// 构造带初始 flag 的叶子 FlagExpr。
fn leaf(flag: u64) -> FlagExpr {
    FlagExpr::Leaf {
        sql: String::new(),
        flag,
    }
}

/// 验证 HasAggFlag：仅当子树含聚合标志时为真。
#[test]
// Go 通过改写原始 flag 测聚合位；此处用子节点形态驱动 SetFlag 传播。
fn TestHasAggFlag() {
    // Go builds a single `ast.BetweenExpr` and overwrites its raw flag for each case.
    // `FlagExpr::Between`'s flag is always derived from its children by `SetFlag`, so each
    // case below shapes the children so the propagated flag matches the Go raw flag under
    // test: FlagHasAggregateFunc, FlagHasAggregateFunc|FlagHasVariable, FlagHasVariable.
    let mut aggregate_only = FlagExpr::Between {
        expr: Box::new(FlagExpr::Aggregate {
            args: vec![],
            flag: 0,
        }),
        left: Box::new(leaf(FLAG_CONSTANT)),
        right: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut aggregate_only);
    assert!(HasAggFlag(&aggregate_only));

    let mut aggregate_and_variable = FlagExpr::Between {
        expr: Box::new(FlagExpr::Aggregate {
            args: vec![],
            flag: 0,
        }),
        left: Box::new(FlagExpr::Variable {
            value: None,
            flag: 0,
        }),
        right: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut aggregate_and_variable);
    assert!(HasAggFlag(&aggregate_and_variable));

    let mut variable_only = FlagExpr::Between {
        expr: Box::new(FlagExpr::Variable {
            value: None,
            flag: 0,
        }),
        left: Box::new(leaf(FLAG_CONSTANT)),
        right: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut variable_only);
    assert!(!HasAggFlag(&variable_only));
}

/// Covers the Go `HasWindowFlag` helper and the `flagSetter.Leave` branches that
/// are not represented by the parser-driven cases in `flag_test.go`.
#[test]
fn test_window_and_remaining_flag_setter_branches() {
    let mut window = FlagExpr::Window {
        args: vec![FlagExpr::ColumnName { flag: 0 }],
        flag: 0,
    };
    SetFlag(&mut window);
    assert_eq!(window.get_flag(), FLAG_HAS_WINDOW_FUNC | FLAG_HAS_REFERENCE);
    assert!(HasWindowFlag(&window));
    assert!(!HasAggFlag(&window));

    for mut reference in [
        FlagExpr::ColumnName { flag: 0 },
        FlagExpr::Position { flag: 0 },
        FlagExpr::Values { flag: 0 },
    ] {
        SetFlag(&mut reference);
        assert_eq!(reference.get_flag(), FLAG_HAS_REFERENCE);
    }

    let mut variable = FlagExpr::Variable {
        value: Some(Box::new(FlagExpr::ParamMarker { flag: 0 })),
        flag: 0,
    };
    SetFlag(&mut variable);
    assert_eq!(
        variable.get_flag(),
        FLAG_HAS_VARIABLE | FLAG_HAS_PARAM_MARKER
    );

    let mut pattern_in = FlagExpr::PatternIn {
        expr: Box::new(leaf(FLAG_CONSTANT)),
        list: vec![],
        sel: Some(Box::new(FlagExpr::Subquery { flag: 0 })),
        flag: 0,
    };
    SetFlag(&mut pattern_in);
    assert_eq!(pattern_in.get_flag(), FLAG_HAS_SUBQUERY);

    for mut pattern in [
        FlagExpr::PatternLike {
            expr: None,
            pattern: Box::new(FlagExpr::ParamMarker { flag: 0 }),
            flag: 0,
        },
        FlagExpr::PatternRegexp {
            expr: None,
            pattern: Box::new(FlagExpr::ParamMarker { flag: 0 }),
            flag: 0,
        },
    ] {
        SetFlag(&mut pattern);
        assert_eq!(pattern.get_flag(), FLAG_HAS_PARAM_MARKER);
    }
}

/// 对照 Go flag_test：手建 FlagExpr 树并断言 SetFlag 传播结果。
#[test]
fn TestFlag() {
    // "1 between 0 and 2" -> FlagConstant
    let mut e = FlagExpr::Between {
        expr: Box::new(leaf(FLAG_CONSTANT)),
        left: Box::new(leaf(FLAG_CONSTANT)),
        right: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "case 1 when 1 then 1 else 0 end" -> FlagConstant
    let mut e = FlagExpr::Case {
        value: Some(Box::new(leaf(FLAG_CONSTANT))),
        when_clauses: vec![(leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT))],
        else_clause: Some(Box::new(leaf(FLAG_CONSTANT))),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "case 1 when 1 then 1 else 0 end" (duplicated in Go's table) -> FlagConstant
    let mut e = FlagExpr::Case {
        value: Some(Box::new(leaf(FLAG_CONSTANT))),
        when_clauses: vec![(leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT))],
        else_clause: Some(Box::new(leaf(FLAG_CONSTANT))),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "case 1 when a > 1 then 1 else 0 end" -> FlagConstant | FlagHasReference
    let mut e = FlagExpr::Case {
        value: Some(Box::new(leaf(FLAG_CONSTANT))),
        when_clauses: vec![(
            FlagExpr::Binary(
                Box::new(leaf(FLAG_HAS_REFERENCE)),
                Box::new(leaf(FLAG_CONSTANT)),
            ),
            leaf(FLAG_CONSTANT),
        )],
        else_clause: Some(Box::new(leaf(FLAG_CONSTANT))),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT | FLAG_HAS_REFERENCE);

    // "1 = ANY (select 1) OR exists (select 1)" -> FlagHasSubquery
    let mut e = FlagExpr::Binary(
        Box::new(FlagExpr::CompareSubquery {
            left: Box::new(leaf(FLAG_CONSTANT)),
            right: Box::new(FlagExpr::Subquery { flag: 0 }),
            flag: 0,
        }),
        Box::new(FlagExpr::ExistsSubquery {
            select: Box::new(FlagExpr::Subquery { flag: 0 }),
            flag: 0,
        }),
    );
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_SUBQUERY);

    // "1 in (1) or 1 is true or null is null or 'abc' like 'abc' or 'abc' rlike 'abc'"
    // -> FlagConstant
    let mut e = FlagExpr::Binary(
        Box::new(FlagExpr::Binary(
            Box::new(FlagExpr::Binary(
                Box::new(FlagExpr::Binary(
                    Box::new(FlagExpr::PatternIn {
                        expr: Box::new(leaf(FLAG_CONSTANT)),
                        list: vec![leaf(FLAG_CONSTANT)],
                        sel: None,
                        flag: 0,
                    }),
                    Box::new(FlagExpr::IsTruth {
                        expr: Box::new(leaf(FLAG_CONSTANT)),
                        flag: 0,
                    }),
                )),
                Box::new(FlagExpr::IsNull {
                    expr: Box::new(leaf(FLAG_CONSTANT)),
                    flag: 0,
                }),
            )),
            Box::new(FlagExpr::PatternLike {
                expr: Some(Box::new(leaf(FLAG_CONSTANT))),
                pattern: Box::new(leaf(FLAG_CONSTANT)),
                flag: 0,
            }),
        )),
        Box::new(FlagExpr::PatternRegexp {
            expr: Some(Box::new(leaf(FLAG_CONSTANT))),
            pattern: Box::new(leaf(FLAG_CONSTANT)),
            flag: 0,
        }),
    );
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "row (1, 1) = row (1, 1)" -> FlagConstant
    let mut e = FlagExpr::Binary(
        Box::new(FlagExpr::Row {
            values: vec![leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT)],
            flag: 0,
        }),
        Box::new(FlagExpr::Row {
            values: vec![leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT)],
            flag: 0,
        }),
    );
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "(1 + a) > ?" -> FlagHasReference | FlagHasParamMarker
    let mut e = FlagExpr::Binary(
        Box::new(FlagExpr::Parentheses {
            expr: Box::new(FlagExpr::Binary(
                Box::new(leaf(FLAG_CONSTANT)),
                Box::new(leaf(FLAG_HAS_REFERENCE)),
            )),
            flag: 0,
        }),
        Box::new(FlagExpr::ParamMarker { flag: 0 }),
    );
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_REFERENCE | FLAG_HAS_PARAM_MARKER);

    // "trim('abc ')" -> FlagHasFunc
    let mut e = FlagExpr::FuncCall {
        args: vec![leaf(FLAG_CONSTANT)],
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_FUNC);

    // "now() + EXTRACT(YEAR FROM '2009-07-02') + CAST(1 AS UNSIGNED)" -> FlagHasFunc
    let mut e = FlagExpr::Binary(
        Box::new(FlagExpr::Binary(
            Box::new(FlagExpr::FuncCall {
                args: vec![],
                flag: 0,
            }),
            Box::new(FlagExpr::FuncCall {
                args: vec![leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT)],
                flag: 0,
            }),
        )),
        Box::new(FlagExpr::FuncCast {
            expr: Box::new(leaf(FLAG_CONSTANT)),
            flag: 0,
        }),
    );
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_FUNC);

    // "substring('abc', 1)" -> FlagHasFunc
    let mut e = FlagExpr::FuncCall {
        args: vec![leaf(FLAG_CONSTANT), leaf(FLAG_CONSTANT)],
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_FUNC);

    // "sum(a)" -> FlagHasAggregateFunc | FlagHasReference
    let mut e = FlagExpr::Aggregate {
        args: vec![leaf(FLAG_HAS_REFERENCE)],
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_REFERENCE);

    // "(select 1) as a" -> FlagHasSubquery
    let mut e = FlagExpr::Subquery { flag: 0 };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_SUBQUERY);

    // "@auto_commit" -> FlagHasVariable
    let mut e = FlagExpr::Variable {
        value: None,
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_VARIABLE);

    // "default(a)" -> FlagHasDefault
    let mut e = FlagExpr::Default { flag: 0 };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_DEFAULT);

    // "a is null" -> FlagHasReference
    let mut e = FlagExpr::IsNull {
        expr: Box::new(leaf(FLAG_HAS_REFERENCE)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_REFERENCE);

    // "1 is true" -> FlagConstant
    let mut e = FlagExpr::IsTruth {
        expr: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "a in (1, count(*), 3)" -> FlagConstant | FlagHasReference | FlagHasAggregateFunc
    let mut e = FlagExpr::PatternIn {
        expr: Box::new(leaf(FLAG_HAS_REFERENCE)),
        list: vec![
            leaf(FLAG_CONSTANT),
            FlagExpr::Aggregate {
                args: vec![],
                flag: 0,
            },
            leaf(FLAG_CONSTANT),
        ],
        sel: None,
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(
        e.get_flag(),
        FLAG_CONSTANT | FLAG_HAS_REFERENCE | FLAG_HAS_AGGREGATE_FUNC
    );

    // "'Michael!' REGEXP '.*'" -> FlagConstant
    let mut e = FlagExpr::PatternRegexp {
        expr: Some(Box::new(leaf(FLAG_CONSTANT))),
        pattern: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_CONSTANT);

    // "a REGEXP '.*'" -> FlagHasReference
    let mut e = FlagExpr::PatternRegexp {
        expr: Some(Box::new(leaf(FLAG_HAS_REFERENCE))),
        pattern: Box::new(leaf(FLAG_CONSTANT)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_REFERENCE);

    // "-a" -> FlagHasReference
    let mut e = FlagExpr::Unary {
        value: Box::new(leaf(FLAG_HAS_REFERENCE)),
        flag: 0,
    };
    SetFlag(&mut e);
    assert_eq!(e.get_flag(), FLAG_HAS_REFERENCE);
}
