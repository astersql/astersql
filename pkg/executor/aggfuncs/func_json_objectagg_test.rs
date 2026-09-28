// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// JSON_OBJECTAGG 聚合的单元测试。
//
// 可执行用例覆盖重复 key 覆盖写，以及 NULL key 被拒绝。


use crate::aggfuncs::{DEF_INT64_SIZE, DEF_INTERFACE_SIZE, SpillValue};
use crate::func_json_objectagg::JsonObjectAgg;

/// 校验同名 key 后写覆盖，以及 NULL key 触发包含 "NULL member names" 的错误。
#[test]
fn json_objectagg_overwrites_duplicate_keys_and_rejects_null_keys() {
    // 同一 key 连续写入时保留最后一次的 value。
    let mut object = JsonObjectAgg::default();
    object
        .update([
            (Some("k".into()), SpillValue::Int64(1)),
            (Some("k".into()), SpillValue::Int64(2)),
        ])
        .unwrap();
    assert_eq!(
        object.result().unwrap().get("k"),
        Some(&SpillValue::Int64(2))
    );
    // key 为 None 应对齐 MySQL：拒绝 NULL 成员名。
    let error = object
        .update([(None, SpillValue::String("invalid".into()))])
        .unwrap_err();
    assert!(error.0.contains("NULL member names"));
}

/// Go 仅为首次插入的 key/value 计入 update 增量；merge 则逐条计入源 partial。
#[test]
fn json_objectagg_reports_go_compatible_memory_deltas() {
    let mut destination = JsonObjectAgg::default();
    let inserted = destination
        .update([(Some("key".into()), SpillValue::Int64(1))])
        .unwrap();
    assert_eq!(
        inserted,
        "key".len() as i64 + DEF_INTERFACE_SIZE + DEF_INT64_SIZE
    );

    let replaced = destination
        .update([(Some("key".into()), SpillValue::Int64(2))])
        .unwrap();
    assert_eq!(replaced, 0);

    let mut source = JsonObjectAgg::default();
    source
        .update([(Some("key".into()), SpillValue::Int64(3))])
        .unwrap();
    assert_eq!(
        destination.merge(&source),
        "key".len() as i64 + DEF_INTERFACE_SIZE + DEF_INT64_SIZE
    );
    assert_eq!(
        destination.result().unwrap().get("key"),
        Some(&SpillValue::Int64(3))
    );
}
