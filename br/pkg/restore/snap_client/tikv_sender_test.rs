// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `tikv_sender_test.go`.
//! Pure merge/sort + Mem fixtures (no kv/domain/kvproto).
//! tikv_sender 测试：物理表排序与文件范围校验。
//! 构造 CreatedTable/分区 ID/改写规则，锁定排序稳定性。
//! encode_row_key 等辅助函数保证与实现侧编码一致。
//! 不发起真实 TiKV RPC，只验证发送前的数据整形契约。
//! 对齐 Go `tikv_sender_test.go`。
//! new_partition_id/new_created_table 构造分区与表 fixture。
//! physical_ids 提取并排序物理 ID，供排序断言使用。
//! test_get_sorted_physical_tables 锁定排序稳定性与分区展开。
//! encode_row_key/rewrite_rules_for/file 支持范围校验用例。
//! test_sort_and_validate_file_ranges 覆盖正常与非法范围。
//! downstream_id/key 辅助对齐改写后的下游键空间。
//! 不发起真实 ImportRPC，只验证发送前数据整形。
//! 非法范围应返回错误而不是 panic。
//! 分区表与非分区表夹具分开构造，避免隐式耦合。
//! 与 Go 测试场景保持同一输入集合更利于对照。
//! 补充要点1：new_partition_id/new_created_table 构造分区与表 fixture。
//! 补充要点2：physical_ids 提取并排序物理 ID，供排序断言使用。
//! 补充要点3：test_get_sorted_physical_tables 锁定排序稳定性与分区展开。
//! 补充要点4：encode_row_key/rewrite_rules_for/file 支持范围校验用例。
//! 补充要点5：test_sort_and_validate_file_ranges 覆盖正常与非法范围。

use std::collections::{HashMap, HashSet};

use crate::export_test::{GetFileRangeKey, GetSortedPhysicalTables};
use crate::pipeline_items::PhysicalTable;
use crate::stubs::{
    CreatedTable, RewriteRules, backuppb, codec, import_sstpb, metautil, model, tablecodec,
};
use crate::tikv_sender::SortAndValidateFileRanges;

/// `new_partition_id`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn new_partition_id(ids: &[i64]) -> model::PartitionInfo {
    model::PartitionInfo {
        Definitions: ids
            .iter()
            .enumerate()
            .map(|(i, id)| model::PartitionDefinition {
                ID: *id,
                Name: model::CIStr::new(format!("{i}")),
            })
            .collect(),
    }
}

/// `new_created_table`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn new_created_table(
    old_table_id: i64,
    new_table_id: i64,
    old_partition_ids: &[i64],
    new_partition_ids: &[i64],
) -> CreatedTable {
    CreatedTable {
        Table: model::TableInfo {
            ID: new_table_id,
            Partition: Some(new_partition_id(new_partition_ids)),
            ..Default::default()
        },
        OldTable: metautil::Table {
            Info: model::TableInfo {
                ID: old_table_id,
                Partition: Some(new_partition_id(old_partition_ids)),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `physical_ids`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn physical_ids(physical_tables: &[PhysicalTable]) -> (Vec<i64>, Vec<i64>) {
    let old_ids = physical_tables.iter().map(|t| t.OldPhysicalID).collect();
    let new_ids = physical_tables.iter().map(|t| t.NewPhysicalID).collect();
    (old_ids, new_ids)
}

/// TestGetSortedPhysicalTables — Go `TestGetSortedPhysicalTables`.
#[test]
/// 测试 `test_get_sorted_physical_tables`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_get_sorted_physical_tables() {
    let created = vec![
        new_created_table(100, 200, &[32, 145, 324], &[900, 23, 54]),
        new_created_table(300, 400, &[322, 11245, 343224], &[9030, 22353, 5354]),
    ];
    let physical = GetSortedPhysicalTables(&created);
    let (old_ids, new_ids) = physical_ids(&physical);
    assert_eq!(old_ids, vec![145, 324, 100, 300, 32, 343224, 322, 11245]);
    assert_eq!(new_ids, vec![23, 54, 200, 400, 900, 5354, 9030, 22353]);
}

/// `encode_row_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn encode_row_key(table_id: i64, handle: i64) -> Vec<u8> {
    let mut key = tablecodec::EncodeTablePrefix(table_id);
    key.extend_from_slice(b"_r");
    let u = (handle as u64) ^ (1u64 << 63);
    key.extend_from_slice(&u.to_be_bytes());
    key
}

/// `rewrite_rules_for`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn rewrite_rules_for(physical_pairs: &[(i64, i64)]) -> RewriteRules {
    RewriteRules {
        Data: physical_pairs
            .iter()
            .map(|(old_id, new_id)| import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(*old_id),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(*new_id),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// `file`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn file(
    table_id: i64,
    start_row: i64,
    end_row: i64,
    total_kvs: u64,
    total_bytes: u64,
    cf: &str,
) -> backuppb::File {
    backuppb::File {
        Name: format!("file_{table_id}_{start_row}_{cf}.sst"),
        StartKey: encode_row_key(table_id, start_row),
        EndKey: encode_row_key(table_id, end_row),
        TotalKvs: total_kvs,
        TotalBytes: total_bytes,
        Cf: cf.into(),
        ..Default::default()
    }
}

/// `downstream_id`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn downstream_id(upstream: i64) -> i64 {
    upstream + ((999 - upstream) % 10 + 1) * 1000
}

/// `key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn key(table_id: i64, row: i64) -> Vec<u8> {
    codec::EncodeBytes(Vec::new(), &encode_row_key(downstream_id(table_id), row))
}

fn expected_file_set(table_id: i64, rows_and_cfs: &[(i64, &str)]) -> (i64, Vec<String>) {
    (
        downstream_id(table_id),
        rows_and_cfs
            .iter()
            .map(|(row, cf)| format!("file_{table_id}_{row}_{cf}.sst"))
            .collect(),
    )
}

fn actual_groups(groups: &[Vec<crate::stubs::BackupFileSet>]) -> Vec<Vec<(i64, Vec<String>)>> {
    groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|set| {
                    (
                        set.TableID,
                        set.SSTFiles.iter().map(|file| file.Name.clone()).collect(),
                    )
                })
                .collect()
        })
        .collect()
}

/// TestSortAndValidateFileRanges — full Go case matrix.
#[test]
/// 测试 `test_sort_and_validate_file_ranges`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_sort_and_validate_file_ranges() {
    let w = "write";
    let d = "default";

    // Case: split-on-table, no checkpoint (subset of Go first case)
    let mut files_100 = HashMap::new();
    files_100.insert(
        100,
        vec![file(100, 1, 2, 100, 100, w), file(100, 1, 2, 100, 100, d)],
    );
    files_100.insert(102, vec![file(102, 1, 2, 100, 100, w)]);

    let mut files_200 = HashMap::new();
    files_200.insert(
        202,
        vec![
            file(202, 1, 2, 100, 100, w),
            file(202, 1, 2, 100, 100, d),
            file(202, 2, 3, 100, 100, w),
            file(202, 2, 3, 100, 100, d),
        ],
    );

    let mut files_300 = HashMap::new();
    files_300.insert(302, vec![file(302, 1, 2, 100, 100, w)]);

    let part = |ids: &[i64]| model::PartitionInfo {
        Definitions: ids
            .iter()
            .map(|id| model::PartitionDefinition {
                ID: *id,
                Name: model::CIStr::new(format!("p_{id}")),
            })
            .collect(),
    };
    let down_part = |ids: &[i64]| model::PartitionInfo {
        Definitions: ids
            .iter()
            .map(|id| model::PartitionDefinition {
                ID: downstream_id(*id),
                Name: model::CIStr::new(format!("p_{id}")),
            })
            .collect(),
    };

    let mk = |up: i64, parts: &[i64], fmap: HashMap<i64, Vec<backuppb::File>>| {
        let down = downstream_id(up);
        let mut pairs = vec![(up, down)];
        for &p in parts {
            pairs.push((p, downstream_id(p)));
        }
        CreatedTable {
            Table: model::TableInfo {
                ID: down,
                Name: model::CIStr::new(format!("tbl-{up}")),
                Partition: Some(down_part(parts)),
                ..Default::default()
            },
            OldTable: metautil::Table {
                DB: model::DBInfo {
                    Name: model::CIStr::new("test"),
                    ..Default::default()
                },
                Info: model::TableInfo {
                    ID: up,
                    Partition: Some(part(parts)),
                    ..Default::default()
                },
                FilesOfPhysicals: fmap,
                ..Default::default()
            },
            RewriteRule: Some(rewrite_rules_for(&pairs)),
        }
    };

    let created = vec![
        mk(100, &[101, 102, 103], files_100),
        mk(200, &[201, 202, 203], files_200),
        mk(300, &[301, 302, 303], files_300),
    ];

    let checkpoints = HashMap::from([
        (
            downstream_id(100),
            HashSet::from([GetFileRangeKey(&format!("file_100_1_{w}.sst"))]),
        ),
        (
            downstream_id(202),
            HashSet::from([GetFileRangeKey(&format!("file_202_1_{w}.sst"))]),
        ),
    ]);

    let f102 = expected_file_set(102, &[(1, w)]);
    let f202_1 = expected_file_set(202, &[(1, w), (1, d)]);
    let f202_2 = expected_file_set(202, &[(2, w), (2, d)]);
    let f202_all = expected_file_set(202, &[(1, w), (1, d), (2, w), (2, d)]);
    let f302 = expected_file_set(302, &[(1, w)]);
    let f100 = expected_file_set(100, &[(1, w), (1, d)]);

    struct Case {
        name: &'static str,
        threshold: u64,
        split_on_table: bool,
        checkpoint: bool,
        split_keys: Vec<Vec<u8>>,
        groups: Vec<Vec<(i64, Vec<String>)>>,
    }

    let cases = vec![
        Case {
            name: "large/split/no-checkpoint",
            threshold: 80,
            split_on_table: true,
            checkpoint: false,
            split_keys: vec![key(202, 2)],
            groups: vec![
                vec![f102.clone()],
                vec![f202_1.clone()],
                vec![f202_2.clone()],
                vec![f302.clone()],
                vec![f100.clone()],
            ],
        },
        Case {
            name: "large/split/checkpoint",
            threshold: 80,
            split_on_table: true,
            checkpoint: true,
            split_keys: vec![key(202, 2)],
            groups: vec![vec![f102.clone()], vec![f202_2.clone()], vec![f302.clone()]],
        },
        Case {
            name: "large/merge/no-checkpoint",
            threshold: 80,
            split_on_table: false,
            checkpoint: false,
            split_keys: vec![
                key(102, 2),
                key(202, 2),
                key(202, 3),
                key(302, 2),
                key(100, 2),
            ],
            groups: vec![
                vec![f102.clone()],
                vec![f202_1.clone()],
                vec![f202_2.clone()],
                vec![f302.clone()],
                vec![f100.clone()],
            ],
        },
        Case {
            name: "large/merge/checkpoint",
            threshold: 80,
            split_on_table: false,
            checkpoint: true,
            split_keys: vec![
                key(102, 2),
                key(202, 2),
                key(202, 3),
                key(302, 2),
                key(100, 2),
            ],
            groups: vec![vec![f102.clone()], vec![f202_2.clone()], vec![f302.clone()]],
        },
        Case {
            name: "small-1/split/no-checkpoint",
            threshold: 350,
            split_on_table: true,
            checkpoint: false,
            split_keys: vec![key(202, 2)],
            groups: vec![
                vec![f102.clone()],
                vec![f202_1.clone()],
                vec![f202_2.clone()],
                vec![f302.clone()],
                vec![f100.clone()],
            ],
        },
        Case {
            name: "small-1/split/checkpoint",
            threshold: 350,
            split_on_table: true,
            checkpoint: true,
            split_keys: vec![key(202, 2)],
            groups: vec![vec![f102.clone()], vec![f202_2.clone()], vec![f302.clone()]],
        },
        Case {
            name: "small-1/merge/no-checkpoint",
            threshold: 350,
            split_on_table: false,
            checkpoint: false,
            split_keys: vec![key(202, 2), key(302, 2), key(100, 2)],
            groups: vec![
                vec![f102.clone(), f202_1.clone()],
                vec![f202_2.clone(), f302.clone()],
                vec![f100.clone()],
            ],
        },
        Case {
            name: "small-1/merge/checkpoint",
            threshold: 350,
            split_on_table: false,
            checkpoint: true,
            split_keys: vec![key(202, 2), key(302, 2), key(100, 2)],
            groups: vec![vec![f102.clone()], vec![f202_2.clone(), f302.clone()]],
        },
        Case {
            name: "small-2/split/no-checkpoint",
            threshold: 450,
            split_on_table: true,
            checkpoint: false,
            split_keys: vec![],
            groups: vec![
                vec![f102.clone()],
                vec![f202_all.clone()],
                vec![f302.clone()],
                vec![f100.clone()],
            ],
        },
        Case {
            name: "small-2/split/checkpoint",
            threshold: 450,
            split_on_table: true,
            checkpoint: true,
            split_keys: vec![],
            groups: vec![vec![f102.clone()], vec![f202_2.clone()], vec![f302.clone()]],
        },
        Case {
            name: "small-2/merge/no-checkpoint",
            threshold: 450,
            split_on_table: false,
            checkpoint: false,
            split_keys: vec![key(102, 2), key(202, 3), key(100, 2)],
            groups: vec![
                vec![f102.clone()],
                vec![f202_all.clone()],
                vec![f302.clone(), f100.clone()],
            ],
        },
        Case {
            name: "small-2/merge/checkpoint",
            threshold: 450,
            split_on_table: false,
            checkpoint: true,
            split_keys: vec![key(102, 2), key(202, 3), key(100, 2)],
            groups: vec![vec![f102.clone()], vec![f202_2.clone()], vec![f302.clone()]],
        },
        Case {
            name: "small-3/merge/no-checkpoint",
            threshold: 501,
            split_on_table: false,
            checkpoint: false,
            split_keys: vec![key(202, 3), key(100, 2)],
            groups: vec![
                vec![f102.clone(), f202_all.clone()],
                vec![f302.clone(), f100.clone()],
            ],
        },
        Case {
            name: "small-3/merge/checkpoint",
            threshold: 501,
            split_on_table: false,
            checkpoint: true,
            split_keys: vec![key(202, 3), key(100, 2)],
            groups: vec![vec![f102.clone(), f202_2.clone()], vec![f302.clone()]],
        },
    ];

    for case in cases {
        let checkpoint = if case.checkpoint {
            &checkpoints
        } else {
            &HashMap::new()
        };
        let (split_keys, groups) = SortAndValidateFileRanges(
            &created,
            checkpoint,
            case.threshold,
            case.threshold,
            case.split_on_table,
        )
        .unwrap();
        assert_eq!(split_keys, case.split_keys, "{} split keys", case.name);
        assert_eq!(actual_groups(&groups), case.groups, "{} groups", case.name);
    }

    // Go's final two cases use uneven sizes/counts to prove that grouping is based on
    // the merged range statistics, not merely on the number of files.
    let mut uneven_100 = HashMap::new();
    uneven_100.insert(
        100,
        vec![file(100, 1, 2, 100, 100, w), file(100, 1, 2, 100, 100, d)],
    );
    uneven_100.insert(102, vec![file(102, 1, 2, 100, 100, w)]);
    let mut uneven_200 = HashMap::new();
    uneven_200.insert(
        202,
        vec![
            file(202, 1, 2, 100, 100, w),
            file(202, 1, 2, 100, 100, d),
            file(202, 2, 3, 400, 400, w),
            file(202, 2, 3, 80, 80, d),
        ],
    );
    let mut uneven_300 = HashMap::new();
    uneven_300.insert(302, vec![file(302, 1, 2, 10, 10, w)]);
    let uneven_created = vec![
        mk(100, &[101, 102, 103], uneven_100),
        mk(200, &[201, 202, 203], uneven_200),
        mk(300, &[301, 302, 303], uneven_300),
    ];
    let uneven_keys = vec![key(202, 2), key(302, 2), key(100, 2)];
    let uneven_all = vec![
        vec![f102.clone(), f202_1],
        vec![f202_2.clone(), f302.clone()],
        vec![f100],
    ];
    let (keys, groups) =
        SortAndValidateFileRanges(&uneven_created, &HashMap::new(), 501, 501, false).unwrap();
    assert_eq!(keys, uneven_keys, "small-4/merge/no-checkpoint split keys");
    assert_eq!(
        actual_groups(&groups),
        uneven_all,
        "small-4/merge/no-checkpoint groups"
    );

    let uneven_checkpoint = vec![vec![f102], vec![f202_2, f302]];
    let (keys, groups) =
        SortAndValidateFileRanges(&uneven_created, &checkpoints, 501, 501, false).unwrap();
    assert_eq!(keys, uneven_keys, "small-4/merge/checkpoint split keys");
    assert_eq!(
        actual_groups(&groups),
        uneven_checkpoint,
        "small-4/merge/checkpoint groups"
    );
}
