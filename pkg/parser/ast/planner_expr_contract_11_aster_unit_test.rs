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

// 规划器表达式契约测试：函数名常量、ValueExpr 构造、Default 表达式与 Accept 遍历。
//
// 对齐 Go 侧调度名字符串、Datum/FieldType 深拷贝语义，以及 CASE 子节点访问顺序与替换保留。

use crate::*;

/// 比较/逻辑/聚合等函数名常量必须与 Go 调度表一致。
#[test]
fn planner_function_names_match_go_dispatch_contract() {
    assert_eq!(
        [EQ, NE, NullEQ, GE, LE, GT, LT],
        ["eq", "ne", "nulleq", "ge", "le", "gt", "lt"]
    );
    assert_eq!(
        [
            RowFunc,
            UnaryNot,
            UnaryMinus,
            Minus,
            BitNeg,
            LogicAnd,
            Like,
            Ilike,
            Regexp,
            In,
            Case,
            If,
            Ifnull,
            Nullif,
            Grouping,
            GetVar,
            SetVar,
            FTSMysqlMatchAgainst,
            CurrentTimestamp,
            UnixTimestamp,
        ],
        [
            "row",
            "not",
            "unaryminus",
            "minus",
            "bitneg",
            "and",
            "like",
            "ilike",
            "regexp",
            "in",
            "case",
            "if",
            "ifnull",
            "nullif",
            "grouping",
            "getvar",
            "setvar",
            "match_against",
            "current_timestamp",
            "unix_timestamp",
        ]
    );
    assert_eq!(
        [AggFuncMin, AggFuncMax, AggFuncSum, AggFuncCount],
        ["min", "max", "sum", "count"]
    );
}

/// NewValueExpr 应保留类型化 Datum，且 FieldType 可 DeepCopy。
#[test]
fn new_value_expr_keeps_typed_datum_and_deep_copyable_type() {
    let integer = NewValueExpr(42_i64, "", "");
    let ExprKind::Value(integer) = integer.Kind else {
        panic!("integer should be a value expression")
    };
    assert_eq!(integer.Datum, ValueDatum::Int64(42));
    let copied_type = parser_types::types::FieldType::DeepCopy(Some(&integer.Type)).unwrap();
    assert_eq!(copied_type, integer.Type);

    // 字符串值同时带上 charset/collation。
    let text = NewValueExpr("hello", "utf8mb4", "utf8mb4_bin");
    let ExprKind::Value(text) = text.Kind else {
        panic!("text should be a value expression")
    };
    assert_eq!(text.Datum, ValueDatum::String("hello".to_owned()));
    assert_eq!(text.Type.GetCharset(), "utf8mb4");
    assert_eq!(text.Type.GetCollate(), "utf8mb4_bin");

    assert!(matches!(
        NewValueExpr(None::<i64>, "", "").Kind,
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Null,
            ..
        })
    ));
}

/// DEFAULT 表达式：无列名与带列名两种形态均识别为 DefaultExpr。
#[test]
fn default_expression_preserves_optional_name_discriminator() {
    let unnamed = ExprNode {
        node_text: Default::default(),
        Kind: ExprKind::DefaultValue,
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    assert!(unnamed.IsDefaultExpr());
    assert!(unnamed.DefaultName().is_none());

    let named = ExprNode {
        node_text: Default::default(),
        Kind: ExprKind::NamedDefault(ColumnName {
            Name: NewCIStr("a"),
            ..Default::default()
        }),
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    assert!(named.IsDefaultExpr());
    assert_eq!(named.DefaultName().unwrap().Name.L, "a");
}

/// 访问器：Enter 可将 Int64(1) 替换为 2，并记录 enter/leave 事件。
#[derive(Default)]
struct ReplacingVisitor {
    events: Vec<String>,
}

fn expression_event(kind: &ExprKind) -> String {
    match kind {
        ExprKind::Case { .. } => "case".to_owned(),
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Int64(value),
            ..
        }) => format!("int:{value}"),
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Bool(value),
            ..
        }) => format!("bool:{value}"),
        other => format!("{other:?}"),
    }
}

impl ExprNodeVisitor for ReplacingVisitor {
    fn Enter(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        self.events
            .push(format!("enter:{}", expression_event(&input.Kind)));
        let mut replacement = input.clone();
        // 将字面量 1 改写为 2，验证替换在子树中保留。
        if matches!(
            input.Kind,
            ExprKind::Value(ValueExpr {
                Datum: ValueDatum::Int64(1),
                ..
            })
        ) {
            replacement = NewValueExpr(2_i64, "", "");
        }
        (replacement, false)
    }

    fn Leave(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        self.events
            .push(format!("leave:{}", expression_event(&input.Kind)));
        (input.clone(), true)
    }
}

/// CASE Accept：按 Go 顺序遍历 Value/When/Else，并保留 Enter 替换结果。
#[test]
fn expr_accept_walks_case_children_in_go_order_and_keeps_replacements() {
    let expression = ExprNode {
        node_text: Default::default(),
        Kind: ExprKind::Case {
            Value: Some(Box::new(NewValueExpr(1_i64, "", ""))),
            WhenClauses: vec![WhenClause {
                Expr: NewValueExpr(true, "", ""),
                Result: NewValueExpr(1_i64, "", ""),
            }],
            ElseClause: Some(Box::new(NewValueExpr(0_i64, "", ""))),
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    let mut visitor = ReplacingVisitor::default();
    let (rewritten, ok) = expression.Accept(&mut visitor);
    assert!(ok);
    let ExprKind::Case {
        Value,
        WhenClauses,
        ElseClause,
    } = rewritten.Kind
    else {
        panic!("case expression should remain a case expression")
    };
    // Value 与 When.Result 中的 1 均被替换为 2。
    assert!(matches!(
        Value.unwrap().Kind,
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Int64(2),
            ..
        })
    ));
    assert!(matches!(
        WhenClauses[0].Result.Kind,
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Int64(2),
            ..
        })
    ));
    assert!(ElseClause.is_some());
    assert_eq!(
        visitor.events,
        [
            "enter:case",
            "enter:int:1",
            "leave:int:2",
            "enter:bool:true",
            "leave:bool:true",
            "enter:int:1",
            "leave:int:2",
            "enter:int:0",
            "leave:int:0",
            "leave:case",
        ]
    );
}
