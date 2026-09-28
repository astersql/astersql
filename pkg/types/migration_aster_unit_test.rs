// Copyright 2026 AsterSQL.
// 迁移期门面冒烟测试：确认 types 包对外分组（scalar/time/metadata/json/decimal）可连通。
//
// 不验证完整业务正确性，只断言关键构造与常量暴露正常。
// Rust 分组再导出没有 Go 测试对应物；值契约分别来自 binary_literal.go、
// time.go、datum.go、json_binary.go、json_path_expr.go 和 mydecimal.go。

use super::*;

#[test]
/// 校验 binary literal、时间构造与 Kind 元数据常量可通过门面访问。
fn formal_facade_exposes_scalar_time_and_metadata_groups() {
    // 用无符号整型构造 BinaryLiteral，-1 表示按值自适应长度
    let literal = scalar::NewBinaryLiteralFromUint(0x1234, -1);
    assert_eq!(literal.0, vec![0x12, 0x34]);
    assert_eq!(scalar::NewBinaryLiteralFromUint(0, -1).0, vec![0]);

    // FromDate 组装年月日时分秒与微秒，用于校验时间核心字段
    let core = time::FromDate(2026, 7, 17, 12, 34, 56, 123_456);
    assert_eq!((core.Year(), core.Month(), core.Day()), (2026, 7, 17));
    assert_eq!(
        (
            core.Hour(),
            core.Minute(),
            core.Second(),
            core.Microsecond()
        ),
        (12, 34, 56, 123_456)
    );
    assert_eq!(metadata::KindNull, 0);
    assert_eq!(metadata::KindVectorFloat32, 19);
}

#[test]
/// 校验 JSON 二进制解析、路径表达式与 MyDecimal 默认值可通过门面访问。
fn formal_facade_exposes_json_and_decimal_groups() {
    // 解析简单 JSON 对象，确认二进制 JSON 入口可用
    let value = json_binary::ParseBinaryJSONFromString(r#"{"a":1}"#).unwrap();
    assert_eq!(value.GetElemCount(), 1);
    assert_eq!(value.String(), r#"{"a": 1}"#);
    assert!(json_binary::ParseBinaryJSONFromString("").is_err());
    assert!(json_binary::ParseBinaryJSONFromString("{} {}").is_err());

    // 解析 JSON Path（如 $.a[0]），确认路径表达式入口可用
    let path = json_path::ParseJSONPathExpr("$.a[0]").unwrap();
    assert_eq!(path.String(), "$.a[0]");
    assert!(json_path::ParseJSONPathExpr("$.a[").is_err());

    // MyDecimal 默认值为 0，确认 decimal 子模块可链接
    let decimal = decimal::mydecimal::MyDecimal::default();
    assert_eq!(decimal.ToString(), b"0".to_vec());
}
