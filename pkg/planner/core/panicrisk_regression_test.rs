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

// 防 panic 回归测试：畸形分区名切片与 schema/table 成对排序。
//
// 锁定历史上因反转切片或排序 less 函数内变异导致的运行时 panic，
// 确保编解码辅助与 SchemaTableSorter 在边界输入下保持安全。

use crate::TiDBCodecFuncHelper;
use crate::ast;
use crate::memtable_infoschema_extractor::SchemaTableSorter;
use model_dependency::TableInfo;

// TestExtractTablePartitionMalformed guards against an out-of-range slice when
// the input contains ')' before '(' (e.g. a crafted table-name argument to
// tidb_encode_record_key). Previously str[start+1:end] panicked with a reversed
// slice; now it returns the whole string with an empty partition.
// TestExtractTablePartitionMalformed 保留畸形分区名输入不会触发切片越界 panic 的回归用例。
#[test]
/// 畸形分区名（`)` 先于 `(`）不得触发切片反转 panic。
fn test_extract_table_partition_malformed() {
    let helper = TiDBCodecFuncHelper;
    let cases = [
        ("t)(", "t)(", None), // ')' before '(' — the panic trigger
        (")(", ")(", None),   // leading ')'
        ("t(p)", "t", Some("p")),
        ("t", "t", None),
        ("t(p", "t(p", None),
        ("tp)", "tp)", None),
    ];

    for (input, expected_table, expected_partition) in cases {
        let (table, partition) = helper.extractTablePartition(input);
        assert_eq!(table, expected_table, "input={input:?}");
        assert_eq!(partition.as_deref(), expected_partition, "input={input:?}");
    }
}

// TestSchemaTableSorterKeepsPairsAligned verifies that sorting schema/table
// pairs keeps schemas[i] paired with tables[i]. The previous hand-rolled
// sort.Slice mutated the table slice inside the less func, scrambling the
// pairing.
// TestSchemaTableSorterKeepsPairsAligned 保留 schema/table 成对排序不能错位的回归用例。
#[test]
/// schema/table 成对排序后 schemas[i] 仍与 tables[i] 对应。
fn test_schema_table_sorter_keeps_pairs_aligned() {
    let mut schemas = vec![
        ast::NewCIStr("db_b"),
        ast::NewCIStr("db_a"),
        ast::NewCIStr("db_a"),
    ];
    let mut tables = vec![
        TableInfo {
            Name: ast::NewCIStr("t_in_b"),
            ..TableInfo::default()
        },
        TableInfo {
            Name: ast::NewCIStr("t2_in_a"),
            ..TableInfo::default()
        },
        TableInfo {
            Name: ast::NewCIStr("t1_in_a"),
            ..TableInfo::default()
        },
    ];

    SchemaTableSorter::new(&mut schemas, &mut tables)
        .expect("parallel schema/table slices have equal length")
        .sort();

    let pairs = schemas
        .iter()
        .zip(&tables)
        .map(|(schema, table)| (schema.L.as_str(), table.Name.L.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        pairs,
        vec![("db_a", "t1_in_a"), ("db_a", "t2_in_a"), ("db_b", "t_in_b"),]
    );
}

#[test]
/// 长度不一致的并行切片应返回错误而非静默错位。
fn test_schema_table_sorter_rejects_misaligned_slices() {
    let mut schemas = vec![ast::NewCIStr("db")];
    let mut tables = Vec::new();
    assert!(SchemaTableSorter::new(&mut schemas, &mut tables).is_err());
}
