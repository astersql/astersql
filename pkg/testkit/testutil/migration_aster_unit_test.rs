// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `testutil` 迁移回归单测：覆盖 common handle、掩码排序、断言与日志钩子契约。

use std::any::Any;

use super::*;

#[test]
fn go_int_parameters_use_pointer_width() {
    let shard_bits: isize = 5;
    assert_eq!(
        vec![1],
        MaskSortHandles(vec![(1_i64 << 59) | 1], shard_bits, mysql::TypeLonglong)
    );
    let length: isize = 4;
    assert_eq!(RandStringRunes(length).len(), length as usize);
}

/// 验证 `MustNewCommonHandle` 对整型与字符串列的编码与列数。
#[test]
fn common_handle_encodes_every_input_datum() {
    let id = 100_i64;
    let name = "abc".to_owned();
    let values: Vec<&dyn Any> = vec![&id, &name];

    let handle = MustNewCommonHandle(values);

    assert!(!handle.IsInt());
    assert_eq!(2, handle.NumCols());
    let (_, first) = codec::DecodeOne(&handle.EncodedCol(0)).expect("decode integer column");
    let (_, second) = codec::DecodeOne(&handle.EncodedCol(1)).expect("decode string column");
    assert_eq!(100, first.GetInt64());
    assert_eq!("abc", second.GetString());
    assert_eq!("{100, abc}", handle.String());
}

/// 验证分片高位被掩掉后仅保留低位并按数值排序。
#[test]
fn mask_sort_handles_keeps_only_unsharded_low_bits() {
    let handles = vec![(7_i64 << 59) | 3, (1_i64 << 59) | 1, (3_i64 << 59) | 2];
    assert_eq!(
        vec![1, 2, 3],
        MaskSortHandles(handles, 5, mysql::TypeLonglong)
    );

    let handles = vec![(7_i64 << 27) | 3, (1_i64 << 27) | 1, (3_i64 << 27) | 2];
    assert_eq!(vec![1, 2, 3], MaskSortHandles(handles, 5, mysql::TypeLong));
}

/// 验证 Datum / Handle 断言遵循 Go 侧二进制校对规则（大小写敏感）。
#[test]
fn datum_and_handle_assertions_follow_go_comparison_rules() {
    DatumEqual(
        types::NewStringDatum("A".to_owned()),
        types::NewStringDatum("A".to_owned()),
    );

    let expected = kv::IntHandle(42);
    let actual = kv::IntHandle(42);
    HandleEqual(&expected, &actual);

    let mismatch = std::panic::catch_unwind(|| {
        DatumEqual(
            types::NewStringDatum("A".to_owned()),
            types::NewStringDatum("a".to_owned()),
        );
    });
    assert!(
        mismatch.is_err(),
        "binary collation must remain case-sensitive"
    );
}

/// 验证无序字符串多重集比较：重排相等、nil 与空切片区分、重复计数敏感。
#[test]
fn unordered_string_comparison_preserves_nil_and_duplicate_semantics() {
    let a = ["1".to_owned(), "1".to_owned(), "2".to_owned()];
    let reordered = ["1".to_owned(), "2".to_owned(), "1".to_owned()];
    let different = ["1".to_owned(), "2".to_owned(), "2".to_owned()];
    let empty: [String; 0] = [];

    assert!(CompareUnorderedStringSlice(Some(&a), Some(&reordered)));
    assert!(!CompareUnorderedStringSlice(Some(&a), Some(&different)));
    assert!(CompareUnorderedStringSlice(None, None));
    assert!(!CompareUnorderedStringSlice(Some(&empty), None));
    assert!(!CompareUnorderedStringSlice(None, Some(&empty)));
}

/// 验证会话连接属性固定夹具与随机字母串生成契约。
#[test]
fn shared_fixture_and_random_string_match_go_contract() {
    let json = DefaultSessionConnectAttrsJSON();
    assert_eq!(
        r#"{"_client_name":"Go-MySQL-Driver","_os":"linux","app_name":"test_app"}"#,
        json
    );
    assert_eq!(
        format!("# Session_connect_attrs: {json}"),
        DefaultSessionConnectAttrsSlowLogLine()
    );
    RequireContainsDefaultSessionConnectAttrs(&json);

    let generated = RandStringRunes(256);
    assert_eq!(256, generated.chars().count());
    assert!(
        generated
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    );
}

/// 验证日志钩子按消息子串过滤并保留结构化字段。
#[test]
fn log_hook_filters_messages_and_retains_fields() {
    let (dispatcher, hook) = WithLogHook("needle");
    tracing::dispatcher::with_default(&dispatcher, || {
        tracing::info!(answer = 1, "ignored event");
        tracing::warn!(answer = 42, detail = "present", "needle event");
    });

    hook.CheckLogCount(1);
    let logs = hook.Logs();
    logs[0].CheckMsg("needle event");
    logs[0].CheckField(&[LogField::i64("answer", 42)]);
    logs[0].CheckFieldNotEmpty("detail");
}
