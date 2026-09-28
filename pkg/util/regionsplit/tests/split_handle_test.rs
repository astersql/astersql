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

// Region 拆分键与 Go 实现的编码兼容性测试。
//
// 覆盖整数主键、Common Handle 和索引键三条路径，确保 Rust 生成的字节布局
// 与 TiDB codec 约定一致，避免拆分点因编码差异落入错误的 Region。

use astersql_util_regionsplit::{
    Datum, GetSplitIndexKeys, GetSplitTableKeys, Handle, IndexInfo, SplitHandleCols,
    StatementContext, TableInfo, commonHandleCols, intHandleCols,
};

/// 将有符号整数转换为保持数值顺序的大端编码。
fn encode_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

/// 构造表记录键前缀 `t{table_id}_r`。
fn record_prefix(table_id: i64) -> Vec<u8> {
    let mut prefix = vec![b't'];
    prefix.extend_from_slice(&encode_i64(table_id));
    prefix.extend_from_slice(b"_r");
    prefix
}

/// 构造表索引键前缀 `t{table_id}_i{index_id}`。
fn index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut prefix = vec![b't'];
    prefix.extend_from_slice(&encode_i64(table_id));
    prefix.extend_from_slice(b"_i");
    prefix.extend_from_slice(&encode_i64(index_id));
    prefix
}

/// 验证整数主键的等分点沿用 Go 的 IntHandle 编码。
#[test]
fn integer_record_keys_match_go_handle_encoding() {
    let table = TableInfo {
        pk_is_handle: true,
        ..TableInfo::default()
    };
    let keys = GetSplitTableKeys(
        &StatementContext::default(),
        &table,
        &intHandleCols,
        7,
        &[Datum::Int(0)],
        &[Datum::Int(2000)],
        2,
        Vec::new(),
    )
    .expect("valid integer split bounds");

    let mut expected = record_prefix(7);
    expected.extend_from_slice(&encode_i64(1000));
    assert_eq!(keys, vec![expected]);
}

/// 验证复合主键 Datum 按 Go 的 memcomparable 规则编码为 Common Handle。
#[test]
fn common_handle_datums_match_go_memcomparable_encoding() {
    let handle = commonHandleCols
        .BuildHandleByDatums(
            &StatementContext::default(),
            &[Datum::Int(7), Datum::Bytes(vec![1, 2, 3])],
        )
        .expect("valid common handle datums");

    let mut expected = vec![3]; // intFlag，标识整数 Datum。
    expected.extend_from_slice(&encode_i64(7));
    expected.push(1); // bytesFlag，标识后续为补齐后的字节串 Datum。
    expected.extend_from_slice(&[1, 2, 3, 0, 0, 0, 0, 0, 0xfa]);
    assert_eq!(handle, Handle::Common(expected));
}

/// 验证索引切分键同时保留索引值与整数 Handle 的 Go 兼容布局。
#[test]
fn index_keys_match_go_integer_value_and_handle_encoding() {
    let table = TableInfo {
        indices: vec![IndexInfo {
            id: 1,
            name: "first".to_owned(),
        }],
        ..TableInfo::default()
    };
    let index = IndexInfo {
        id: 1,
        name: "idx".to_owned(),
    };
    let keys = GetSplitIndexKeys(
        &StatementContext::default(),
        &table,
        &index,
        7,
        &[Datum::Int(0)],
        &[Datum::Int(100)],
        2,
        Vec::new(),
    )
    .expect("valid index split bounds");

    assert_eq!(keys.len(), 2);
    let mut expected_prefix = index_prefix(7, 1);
    expected_prefix.push(3); // 索引值的 intFlag。
    assert!(keys[1].starts_with(&expected_prefix));
    // Go 插值算法保留公共前缀，只对剩余后缀的前八个字节插值；Handle 标志属于
    // 该后缀的一部分，并非在结果末尾另行追加的字段。
    assert_eq!(keys[1].len(), expected_prefix.len() + 7 + 8);
    assert_eq!(keys[1][expected_prefix.len() + 8], 3); // codec.IntHandleFlag。
}
