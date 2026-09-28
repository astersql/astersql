// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 类型化字面量与谓词表达式节点的单元测试。
//
// 校验 AST（抽象语法树）值节点在构造后同时保留 Datum（运行时数据）与
// MySQL 字段类型，并确认 EXISTS 子查询的取反标志可正确携带。

use super::*;

/// 校验 NULL / 布尔 / 有符号整数 / 无符号整数 / 浮点字面量各自绑定的 Datum 与类型。
#[test]
fn typed_literals_keep_datum_and_field_type() {
    // Go `newValueExpr` 会同时设置 Datum、完整默认 FieldType 与投影偏移。
    let cases = [
        (
            ExprNode::NullValue(),
            ValueDatum::Null,
            parser_mysql::r#type::TypeNull,
            0,
            parser_mysql::r#type::BinaryFlag,
        ),
        (
            ExprNode::BoolValue(true),
            ValueDatum::Bool(true),
            parser_mysql::r#type::TypeLonglong,
            1,
            parser_mysql::r#type::BinaryFlag | parser_mysql::r#type::IsBooleanFlag,
        ),
        (
            ExprNode::IntValue(-12),
            ValueDatum::Int64(-12),
            parser_mysql::r#type::TypeLonglong,
            3,
            parser_mysql::r#type::BinaryFlag,
        ),
        (
            ExprNode::UintValue(u64::MAX),
            ValueDatum::Uint64(u64::MAX),
            parser_mysql::r#type::TypeLonglong,
            20,
            parser_mysql::r#type::BinaryFlag | parser_mysql::r#type::UnsignedFlag,
        ),
        (
            ExprNode::FloatValue(1.25),
            // 浮点按 IEEE-754 比特模式存入 Datum，避免直接比较浮点相等性。
            ValueDatum::Float64(1.25_f64.to_bits()),
            parser_mysql::r#type::TypeDouble,
            4,
            parser_mysql::r#type::BinaryFlag,
        ),
    ];
    for (node, datum, mysql_type, flen, flags) in cases {
        // 类型化字面量必须落在 Value 变体上，否则测试本身构造有误。
        let ExprKind::Value(value) = node.Kind else {
            panic!("typed value expected")
        };
        assert_eq!(value.Datum, datum);
        assert_eq!(value.Type.GetType(), mysql_type);
        assert_eq!(value.Type.GetFlen(), flen);
        assert_eq!(
            value.Type.GetDecimal(),
            if mysql_type == parser_mysql::r#type::TypeDouble {
                parser_types::types::UnspecifiedLength
            } else {
                0
            }
        );
        assert_eq!(value.Type.GetCharset(), parser_charset::charset::CharsetBin);
        assert_eq!(
            value.Type.GetCollate(),
            parser_charset::charset::CollationBin
        );
        assert_eq!(value.Type.GetFlag(), flags);
        // ProjectionOffset 为 -1 表示尚未投影到结果列偏移。
        assert_eq!(value.ProjectionOffset, -1);
    }
}

/// 校验谓词默认字段类型为 TINY，以及 EXISTS 子查询可携带 Not 取反。
#[test]
fn predicate_nodes_carry_type_and_exists_negation() {
    let predicate_type = ExprNode::PredicateType();
    assert_eq!(predicate_type.GetType(), parser_mysql::r#type::TypeTiny);
    assert_eq!(predicate_type.GetFlen(), 1);
    assert_eq!(predicate_type.GetDecimal(), 0);
    // EXISTS 子查询：Sel 为被检测的子查询表达式，Not 表示 NOT EXISTS。
    let exists = ExprKind::ExistsSubquery {
        Sel: Box::new(ExprNode::NullValue()),
        Not: true,
    };
    let ExprKind::ExistsSubquery { Sel, Not } = exists else {
        unreachable!()
    };
    assert!(Not);
    assert!(matches!(
        Sel.Kind,
        ExprKind::Value(ValueExpr {
            Datum: ValueDatum::Null,
            ..
        })
    ));
}
