// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Binary JSON 编解码单元测试：标量/容器布局、序列化与错误语义对齐 Go。
//
// 覆盖类型码选择、随机访问、Copy 独立性、opaque base64 文本、
// 哈希规范化、键过长与深度超限错误，以及 size/深度往返。

#[path = "json_constants.rs"]
pub mod json_constants;
pub use json_constants::*;

#[path = "json_binary.rs"]
pub mod json_binary;
pub use json_binary::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 解析 JSON 文本为 BinaryJSON（测试辅助，失败直接 panic）。
    fn parse(input: &str) -> BinaryJSON {
        ParseBinaryJSONFromString(input).unwrap()
    }

    /// 标量编码与访问器：int64/uint64/string 的类型码与文本表示。
    #[test]
    fn scalar_encoding_and_accessors_match_go() {
        let signed = CreateBinaryJSON(-7_i64);
        assert_eq!(signed.TypeCode, JSONTypeCodeInt64);
        assert_eq!(signed.GetInt64(), -7);
        assert_eq!(signed.String(), "-7");

        let unsigned = CreateBinaryJSON(u64::MAX);
        assert_eq!(unsigned.TypeCode, JSONTypeCodeUint64);
        assert_eq!(unsigned.GetUint64(), u64::MAX);
        assert_eq!(unsigned.String(), "18446744073709551615");

        let text = CreateBinaryJSON("hello");
        assert_eq!(text.TypeCode, JSONTypeCodeString);
        assert_eq!(text.GetString(), b"hello");
        assert_eq!(text.String(), r#""hello""#);
    }

    /// 解析保留整数种类；浮点格式化与 NaN 拒绝对齐 Go。
    #[test]
    fn parse_preserves_number_kinds_and_float_formatting() {
        assert_eq!(parse("3").TypeCode, JSONTypeCodeInt64);
        assert_eq!(parse("9223372036854775808").TypeCode, JSONTypeCodeUint64);
        assert_eq!(parse("3.0").String(), "3.0");
        assert_eq!(CreateBinaryJSON(1.0e15_f64).String(), "1e15");
        assert_eq!(CreateBinaryJSON(1.0e-16_f64).String(), "1e-16");
        assert!(CreateBinaryJSON(f64::NAN).MarshalJSON().is_err());
    }

    /// 对象/数组二进制布局支持按键序与下标随机访问。
    #[test]
    fn arrays_objects_and_random_access_use_go_binary_layout() {
        let value = parse(r#"{"name":"Tom","age":19,"ok":true,"items":[1,null]}"#);
        assert_eq!(value.TypeCode, JSONTypeCodeObject);
        assert_eq!(value.GetElemCount(), 4);
        assert_eq!(
            value.String(),
            r#"{"age": 19, "items": [1, null], "name": "Tom", "ok": true}"#
        );
        assert_eq!(
            value.GetKeys().String(),
            r#"["age", "items", "name", "ok"]"#
        );

        let array = parse("[1, true, null, \"x\"]");
        assert_eq!(array.GetElemCount(), 4);
        assert_eq!(array.ArrayGetElem(0).GetInt64(), 1);
        assert_eq!(array.ArrayGetElem(1).Value, vec![JSONLiteralTrue]);
        assert_eq!(array.ArrayGetElem(3).GetString(), b"x");
    }

    /// Copy 后修改不影响原值；转义序列与 Go 一致。
    #[test]
    fn copy_is_independent_and_json_strings_match_go_escaping() {
        let original = parse(r#"{"a":"line\n\u2028quote\"","b":[1]}"#);
        let mut copied = original.Copy();
        copied.Value[0] = 0;
        assert_eq!(original.GetElemCount(), 2);
        assert_eq!(
            original.String(),
            r#"{"a": "line\n\u2028quote\"", "b": [1]}"#
        );
        assert_ne!(original.Value[0], copied.Value[0]);
    }

    /// opaque 使用 `base64:typeN:...` 文本形式。
    #[test]
    fn opaque_values_use_the_go_base64_text_form() {
        let opaque = CreateBinaryJSON(Opaque {
            TypeCode: 253,
            Buf: b"abc".to_vec(),
        });
        assert_eq!(opaque.GetOpaqueFieldType(), 253);
        assert_eq!(opaque.GetOpaque().Buf, b"abc");
        assert_eq!(opaque.String(), r#""base64:type253:YWJj""#);
    }

    /// 可精确表示的整数与同值 float 哈希一致；过大整数保留 uint64 类型码。
    #[test]
    fn hash_normalizes_exact_numeric_values_but_not_large_integers() {
        let int_hash = CreateBinaryJSON(9_i64).HashValue(Vec::new());
        let float_hash = CreateBinaryJSON(9.0_f64).HashValue(Vec::new());
        assert_eq!(int_hash, float_hash);

        let large = CreateBinaryJSON((1_u64 << 63) + 1);
        assert_eq!(large.HashValue(Vec::new())[0], JSONTypeCodeUint64);
        assert_eq!(large.CalculateHashValueSize(), 9);
    }

    /// 空文档、过长键与超深嵌套分别映射到对应 JsonErrorKind。
    #[test]
    fn invalid_text_long_keys_and_deep_documents_keep_go_errors() {
        let empty = ParseBinaryJSONFromString("").unwrap_err();
        assert_eq!(empty.kind(), JsonErrorKind::InvalidJsonText);
        assert!(empty.to_string().contains("document is empty"));

        let mut object = BTreeMap::new();
        object.insert("a".repeat(65536), JsonValue::I64(1));
        let long_key = CreateBinaryJSONWithCheck(JsonValue::Object(object)).unwrap_err();
        assert_eq!(long_key.kind(), JsonErrorKind::ObjectKeyTooLong);

        let mut deep = JsonValue::Null;
        for _ in 0..101 {
            deep = JsonValue::Array(vec![deep]);
        }
        let deep_error = CreateBinaryJSONWithCheck(deep).unwrap_err();
        assert_eq!(deep_error.kind(), JsonErrorKind::DocumentTooDeep);
    }

    /// size 估算、GetValue 往返与深度计算覆盖对象/数组/科学计数法数字。
    #[test]
    fn size_calculation_and_value_round_trip_cover_all_json_shapes() {
        let input = JsonValue::Object(BTreeMap::from([
            (
                "array".to_owned(),
                JsonValue::Array(vec![JsonValue::Bool(false), JsonValue::Null]),
            ),
            ("number".to_owned(), JsonValue::Number("1.25e2".to_owned())),
        ]));
        let binary = CreateBinaryJSON(input.clone());
        let expected = JsonValue::Object(BTreeMap::from([
            (
                "array".to_owned(),
                JsonValue::Array(vec![JsonValue::Bool(false), JsonValue::Null]),
            ),
            ("number".to_owned(), JsonValue::F64(125.0)),
        ]));
        assert_eq!(binary.GetValue(), expected);
        assert!(CalculateBinaryJSONSize(input) >= binary.Value.len() as i64);
        assert_eq!(binary.GetElemDepth(), 3);
    }
}
