// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// parser_driver 迁移期扩展单元测试，对齐 Go ValueExpr 行为细节。
//
// 除 Restore/Format 表外，还覆盖布尔/字符集/无符号分支、引号辅助函数、
// 构造钩子、PositionExpr 访问者以及 Format 的无错误返回契约。

use super::*;
use std::any::Any;
use std::io::Write;
use types_decimal::mydecimal::NewDecFromInt;

/// 用默认 Restore 标志还原 Datum。
fn restore(datum: types::Datum) -> String {
    let expression = ValueExpr {
        Datum: datum,
        ..ValueExpr::default()
    };
    expression
        .RestoreToString(format::DefaultRestoreFlags)
        .unwrap()
}

/// Format 输出到 Vec 再转字符串。
fn formatted(datum: types::Datum) -> String {
    let expression = ValueExpr {
        Datum: datum,
        ..ValueExpr::default()
    };
    let mut output = Vec::new();
    expression.Format(&mut output);
    String::from_utf8(output).unwrap()
}

#[test]
/// Restore 输出与 Go 表驱动用例一致。
fn value_expr_restore_matches_go_table() {
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewDurationDatum(types::ZeroDuration), "'00:00:00'"),
        (
            types::NewTimeDatum(types::Time::default()),
            "'0000-00-00 00:00:00'",
        ),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
    ];

    for (datum, expected) in cases {
        assert_eq!(restore(datum), expected);
    }
}

#[test]
/// Format 输出与 Go 表一致，并覆盖 NaN/Inf。
fn value_expr_format_matches_go_table() {
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
        (types::NewStringDatum("''".to_owned()), "''''''"),
        (
            types::NewStringDatum("\\''\t\n".to_owned()),
            "'\\\\''''\t\n'",
        ),
    ];

    for (datum, expected) in cases {
        assert_eq!(formatted(datum), expected);
    }

    assert_eq!(formatted(types::NewFloat64Datum(f64::NAN)), "NaN");
    assert_eq!(formatted(types::NewFloat64Datum(f64::INFINITY)), "+Inf");
    assert_eq!(formatted(types::NewFloat64Datum(f64::NEG_INFINITY)), "-Inf");
}

#[test]
/// 布尔、字符集前缀、无符号 HEX 与未实现 Kind 的错误分支。
fn restore_preserves_boolean_charset_unsigned_and_error_branches() {
    let mut boolean = ValueExpr {
        Datum: types::NewIntDatum(1),
        ..ValueExpr::default()
    };
    boolean.Type.AddFlag(types::mysql::IsBooleanFlag);
    assert_eq!(
        boolean
            .RestoreToString(format::DefaultRestoreFlags)
            .unwrap(),
        "TRUE"
    );
    boolean.Datum.SetInt64(0);
    assert_eq!(
        boolean
            .RestoreToString(format::DefaultRestoreFlags)
            .unwrap(),
        "FALSE"
    );

    let mut string = ValueExpr {
        Datum: types::NewStringDatum("text".to_owned()),
        ..ValueExpr::default()
    };
    string.Type.SetCharset("latin1".to_owned());
    assert_eq!(
        string.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "_LATIN1'text'"
    );
    assert_eq!(
        string
            .RestoreToString(format::DefaultRestoreFlags | format::RestoreStringWithoutCharset)
            .unwrap(),
        "'text'"
    );

    let mut binary = ValueExpr {
        Datum: types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![0x0a, 0xff])),
        ..ValueExpr::default()
    };
    binary.Type.AddFlag(types::mysql::UnsignedFlag);
    assert_eq!(
        binary.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "x'0aff'"
    );

    let mut raw = types::Datum::default();
    raw.SetRaw(vec![1]);
    let expression = ValueExpr {
        Datum: raw,
        ..ValueExpr::default()
    };
    assert_eq!(
        expression
            .RestoreToString(format::DefaultRestoreFlags)
            .unwrap_err()
            .to_string(),
        "Not implemented"
    );
}

#[test]
/// Wrap/Unwrap 单引号往返与未引号串原样返回。
fn quote_helpers_are_go_compatible() {
    let cases = ["plain", "'", "\\", "\\''\t\n", "中文"];
    for input in cases {
        assert_eq!(UnwrapFromSingleQuotes(&WrapInSingleQuotes(input)), input);
    }
    assert_eq!(UnwrapFromSingleQuotes("not quoted"), "not quoted");
    assert_eq!(UnwrapFromSingleQuotes("'unterminated"), "'unterminated");
}

#[test]
/// newValueExpr/newParamMarkerExpr/init 钩子的类型与偏移语义。
fn constructors_set_type_collation_offsets_and_hooks() {
    let null_expression = newValueExpr(
        Box::new(()),
        types::mysql::DefaultCharset,
        types::mysql::DefaultCollationName,
    );
    assert_eq!(null_expression.Datum.Kind(), types::KindNull);
    assert_eq!(null_expression.Type.GetType(), types::mysql::TypeNull);
    assert_eq!(
        null_expression.Type.GetFlag() & types::mysql::NotNullFlag,
        0
    );

    let expression = newValueExpr(
        Box::new("hello".to_owned()),
        types::mysql::DefaultCharset,
        types::mysql::DefaultCollationName,
    );
    assert_eq!(expression.Datum.GetString(), "hello");
    assert_eq!(expression.Type.GetCollate(), expression.Datum.Collation());
    assert_eq!(expression.GetProjectionOffset(), -1);

    let mut original = ValueExpr::default();
    original.SetProjectionOffset(41);
    let reused = newValueExpr(
        Box::new(original),
        types::mysql::DefaultCharset,
        types::mysql::DefaultCollationName,
    );
    assert_eq!(reused.GetProjectionOffset(), 41);

    let mut marker = newParamMarkerExpr(17);
    assert_eq!(marker.Offset, 17);
    assert_eq!(marker.GetProjectionOffset(), 0);
    marker.SetOrder(3);
    assert_eq!(marker.Order, 3);
    assert_eq!(
        marker.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "?"
    );

    let hooks = init();
    assert_eq!((hooks.new_decimal)("12.50").unwrap().String(), "12.50");
    assert_eq!((hooks.new_hex_literal)("x'0f'").unwrap().0.0, vec![0x0f]);
    assert_eq!((hooks.new_bit_literal)("b'101'").unwrap().0.0, vec![0x05]);
}

#[test]
/// PositionExpr 持有 ParamMarker 并接受 AST Visitor 遍历。
fn position_expr_keeps_runtime_marker_and_visits_it() {
    use parser_ast::expressions::{Expr, PositionExpr};

    #[derive(Default)]
    struct PositionVisitor {
        expressions: usize,
        markers: usize,
    }

    impl parser_ast::expressions::Visitor for PositionVisitor {
        fn enter(&mut self, _node: &mut Expr) -> bool {
            self.expressions += 1;
            false
        }

        fn leave(&mut self, _node: &mut Expr) -> bool {
            true
        }

        fn enter_param_marker(
            &mut self,
            marker: &mut dyn parser_ast::expressions::ParamMarkerExpr,
        ) -> bool {
            self.markers += 1;
            marker.set_order(marker.order() + 1);
            false
        }
    }

    let mut marker = newParamMarkerExpr(17);
    marker.Order = 3;
    marker.InExecute = true;
    marker.Datum = types::NewIntDatum(9);
    let mut expression = Expr::Position(PositionExpr {
        position: 0,
        parameter: Some(marker),
    });
    let cloned = expression.clone();

    let mut visitor = PositionVisitor::default();
    assert!(expression.accept(&mut visitor));
    assert_eq!((visitor.expressions, visitor.markers), (1, 1));

    let Expr::Position(position) = expression else {
        panic!("position expression changed variant");
    };
    let marker = position.parameter.unwrap();
    assert_eq!(
        (marker.offset(), marker.order(), marker.in_execute()),
        (17, 4, true)
    );
    assert_eq!(
        marker
            .get_value()
            .downcast_ref::<types::Datum>()
            .unwrap()
            .GetInt64(),
        9
    );
    let Expr::Position(cloned_position) = cloned else {
        panic!("cloned position expression changed variant");
    };
    let cloned_marker = cloned_position.parameter.unwrap();
    assert_eq!(
        (
            cloned_marker.offset(),
            cloned_marker.order(),
            cloned_marker.in_execute()
        ),
        (17, 3, true)
    );
}

/// Enter 时用带投影偏移的 ValueExpr 替换原节点。
struct ReplacingVisitor {
    enter_count: usize,
    leave_count: usize,
    skip: bool,
}

impl Visitor for ReplacingVisitor {
    fn Enter(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool) {
        self.enter_count += 1;
        if node.is::<ValueExpr>() {
            let mut replacement = ValueExpr::default();
            replacement.SetProjectionOffset(99);
            (Box::new(replacement), self.skip)
        } else {
            (node, self.skip)
        }
    }

    fn Leave(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool) {
        self.leave_count += 1;
        (node, true)
    }
}

#[test]
/// Accept 在 skip=true/false 下均执行 Enter 替换与 Leave。
fn accept_preserves_go_enter_replace_skip_and_leave_flow() {
    for skip in [false, true] {
        let mut visitor = ReplacingVisitor {
            enter_count: 0,
            leave_count: 0,
            skip,
        };
        let (node, ok) = Box::new(ValueExpr::default()).Accept(&mut visitor);
        let node = node.downcast::<ValueExpr>().unwrap();
        assert_eq!(node.GetProjectionOffset(), 99);
        assert!(ok);
        assert_eq!(visitor.enter_count, 1);
        assert_eq!(visitor.leave_count, 1);
    }
}

#[derive(Default)]
/// 始终失败的 Write，用于验证 Format 吞掉写错误。
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("injected"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
/// Format 对齐 Go：写失败不向外返回错误。
fn format_keeps_go_no_error_return_contract() {
    let expression = ValueExpr {
        Datum: types::NewIntDatum(1),
        ..ValueExpr::default()
    };
    expression.Format(&mut FailingWriter);
}

#[test]
/// Go RestoreCtx 的 WritePlain 不返回写错误，支持的 Kind 仍应恢复成功。
fn restore_keeps_go_writer_error_contract() {
    let expression = ValueExpr {
        Datum: types::NewIntDatum(1),
        ..ValueExpr::default()
    };
    let mut writer = FailingWriter;
    let mut context = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut writer);
    assert!(expression.Restore(&mut context).is_ok());

    let marker = newParamMarkerExpr(0);
    let mut writer = FailingWriter;
    let mut context = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut writer);
    assert!(marker.Restore(&mut context).is_ok());
}
