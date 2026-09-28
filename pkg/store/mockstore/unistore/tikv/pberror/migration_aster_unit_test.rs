// Copyright 2021-present PingCAP, Inc.
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

// pberror 从 Go 迁移到 Rust 的字符串格式对齐单测。
//
// `PBError::Error()` 需与 Go protobuf compact text 输出一致，
// 包括普通 message、嵌套 Region 错误字段、空错误与 nil 指针。

use super::*;

/// 普通 message 字段的 compact text 应与 Go 一致。
#[test]
fn error_matches_go_protobuf_string() {
    let mut request_error = errorpb::Error::new();
    request_error.set_message("region unavailable".to_owned());

    let wrapped = PBError {
        RequestErr: Some(request_error),
    };

    assert_eq!(wrapped.Error(), "message:\"region unavailable\" ");
    assert_eq!(wrapped.to_string(), wrapped.Error());
}

/// ServerIsBusy 等嵌套 Region 错误字段需完整序列化。
#[test]
fn error_preserves_nested_region_error_fields() {
    let mut busy = errorpb::ServerIsBusy::new();
    busy.set_reason("raftstore is busy".to_owned());
    busy.set_backoff_ms(125);

    let mut request_error = errorpb::Error::new();
    request_error.set_server_is_busy(busy);

    let wrapped = PBError {
        RequestErr: Some(request_error),
    };

    assert_eq!(
        wrapped.Error(),
        "server_is_busy:<reason:\"raftstore is busy\" backoff_ms:125 > "
    );
}

/// 空的 errorpb::Error 对应空字符串（非 "<nil>"）。
#[test]
fn empty_request_error_matches_go_empty_protobuf_string() {
    let wrapped = PBError {
        RequestErr: Some(errorpb::Error::new()),
    };

    assert_eq!(wrapped.Error(), "");
}

/// RequestErr 为 None 时与 Go nil 指针一致，输出 "<nil>"。
#[test]
fn nil_request_error_matches_go_nil_protobuf_string() {
    let wrapped = PBError { RequestErr: None };

    assert_eq!(wrapped.Error(), "<nil>");
}

/// 确认 PBError 实现标准 Error trait，可向上抛出。
#[test]
fn pb_error_implements_standard_error() {
    fn assert_error<T: std::error::Error>() {}

    assert_error::<PBError>();
}
