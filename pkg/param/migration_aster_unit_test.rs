// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `param` 包迁移核对单元测试。
//
// 验证 BinaryParam 字段形状与未知字段类型错误码/文案与 Go 侧一致。
use astersql_param::{BinaryParam, ERR_UNKNOWN_FIELD_TYPE, ErrUnknownFieldType};

#[test]
/// 确认 BinaryParam 完整保留协议解码后的类型、无符号、NULL 与原始字节。
fn binary_param_preserves_decoded_protocol_fields() {
    let param = BinaryParam {
        Tp: 0xf6,
        IsUnsigned: true,
        IsNull: false,
        Val: vec![0x00, 0x7f, 0x80, 0xff],
    };

    assert_eq!(param.Tp, 0xf6);
    assert!(param.IsUnsigned);
    assert!(!param.IsNull);
    assert_eq!(param.Val, [0x00, 0x7f, 0x80, 0xff]);
}

#[test]
/// Go 结构体始终可用零值；Rust 默认构造必须保持相同字段状态。
fn binary_param_default_matches_go_zero_value() {
    let param = BinaryParam::default();

    assert_eq!(param.Tp, 0);
    assert!(!param.IsUnsigned);
    assert!(!param.IsNull);
    assert!(param.Val.is_empty());
}

#[test]
/// 确认未知字段类型错误使用 server 标准错误码 8051 与固定文案。
fn unknown_field_type_uses_server_standard_error() {
    assert_eq!(ERR_UNKNOWN_FIELD_TYPE.Code(), 8051);
    assert_eq!(ERR_UNKNOWN_FIELD_TYPE.GetMsg(), "unknown field type");
    assert_eq!(ERR_UNKNOWN_FIELD_TYPE.RFCCode().to_string(), "server:8051");

    // Keep the Go package's exported identifier available to callers.
    assert_eq!(ErrUnknownFieldType.Code(), ERR_UNKNOWN_FIELD_TYPE.Code());
}
