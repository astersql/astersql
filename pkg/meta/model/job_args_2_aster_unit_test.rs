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

// Job 参数 V1/V2 编解码与完成态布局的单元测试。
//
// 覆盖建表/批量建表、重命名、Flashback ON/OFF 字符串、索引参数布局、
// V2 缓存命中以及列式索引类型向后兼容。

use super::serde_json;
use super::*;

/// 构造已序列化 V1 RawArgs 的 Job，便于 Get*Args 往返断言。
fn v1_job<T: JobArgs>(args: &T, action: ActionType) -> Job {
    let probe = Job {
        version: JobVersion1,
        tp: action,
        ..Default::default()
    };
    Job {
        version: JobVersion1,
        tp: action,
        raw_args: serde_json::to_vec(&args.getArgsV1(&probe)).unwrap(),
        ..Default::default()
    }
}

#[test]
/// 建表/视图/序列在 V1 下各自布局往返一致。
fn v1_create_table_round_trips_each_action_layout() {
    let cases = [
        (
            ActionCreateTable,
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 100,
                    ..Default::default()
                })),
                FKCheck: true,
                ..Default::default()
            },
        ),
        (
            ActionCreateView,
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 101,
                    ..Default::default()
                })),
                OnExistReplace: true,
                OldViewTblID: 88,
                ..Default::default()
            },
        ),
        (
            ActionCreateSequence,
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 102,
                    ..Default::default()
                })),
                ..Default::default()
            },
        ),
    ];

    for (action, expected) in cases {
        let mut job = v1_job(&expected, action);
        let actual = GetCreateTableArgs(&mut job).unwrap();
        assert_eq!(expected.TableInfo, actual.TableInfo);
        assert_eq!(expected.FKCheck, actual.FKCheck);
        assert_eq!(expected.OnExistReplace, actual.OnExistReplace);
        assert_eq!(expected.OldViewTblID, actual.OldViewTblID);
    }
}

#[test]
/// 批量建表解码后统一应用共享外键检查标志。
fn v1_batch_create_applies_the_shared_fk_check() {
    let expected = BatchCreateTableArgs {
        Tables: vec![
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 1,
                    ..Default::default()
                })),
                FKCheck: true,
                ..Default::default()
            },
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 2,
                    ..Default::default()
                })),
                FKCheck: false,
                ..Default::default()
            },
        ],
    };
    let mut job = v1_job(&expected, ActionCreateTables);
    let actual = GetBatchCreateTableArgs(&mut job).unwrap();
    assert_eq!(2, actual.Tables.len());
    assert!(actual.Tables.iter().all(|table| table.FKCheck));
    assert_eq!(2, actual.Tables[1].TableInfo.as_ref().unwrap().ID);
}

#[test]
/// V1 并行数组组装 RenameTables 时保留全部字段对齐。
fn rename_tables_v1_preserves_all_parallel_fields() {
    let renamed = GetRenameTablesArgsFromV1(
        vec![1, 2],
        vec![ast::NewCIStr("old-a"), ast::NewCIStr("old-b")],
        vec![ast::NewCIStr("ta"), ast::NewCIStr("tb")],
        vec![3, 4],
        vec![ast::NewCIStr("na"), ast::NewCIStr("nb")],
        vec![5, 6],
    );
    assert_eq!(2, renamed.len());
    assert_eq!("old-b", renamed[1].OldSchemaName.O);
    assert_eq!("tb", renamed[1].OldTableName.O);
    assert_eq!(4, renamed[1].NewSchemaID);
    assert_eq!("nb", renamed[1].NewTableName.O);
    assert_eq!(6, renamed[1].TableID);
}

#[test]
/// Flashback V1 使用 ON/OFF 字符串并正确解码布尔开关。
fn flashback_v1_keeps_go_on_off_strings_and_decodes_them() {
    let expected = FlashbackClusterArgs {
        FlashbackTS: 42,
        EnableGC: true,
        EnableAutoAnalyze: true,
        EnableTTLJob: false,
        SuperReadOnly: true,
        LockedRegionCnt: 3,
        StartTS: 40,
        CommitTS: 44,
        ..Default::default()
    };
    let probe = Job::default();
    let raw = expected.getArgsV1(&probe);
    assert_eq!(serde_json::json!("ON"), raw[3]);
    assert_eq!(serde_json::json!("ON"), raw[4]);
    assert_eq!(serde_json::json!("OFF"), raw[8]);

    let mut job = v1_job(&expected, 0);
    let actual = GetFlashbackClusterArgs(&mut job).unwrap();
    assert!(actual.EnableAutoAnalyze);
    assert!(actual.SuperReadOnly);
    assert!(!actual.EnableTTLJob);
    assert_eq!(42, actual.FlashbackTS);
}

#[test]
/// 单索引与多索引加索引的 V1 布局均可解码。
fn modify_index_v1_supports_single_and_multi_add_layouts() {
    let single = ModifyIndexArgs {
        IndexArgs: vec![IndexArg {
            Unique: true,
            IndexName: ast::NewCIStr("idx"),
            Global: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut job = v1_job(&single, ActionAddIndex);
    let decoded = GetModifyIndexArgs(&mut job).unwrap();
    assert_eq!(1, decoded.IndexArgs.len());
    assert!(decoded.IndexArgs[0].Unique);
    assert!(decoded.IndexArgs[0].Global);

    let multi = ModifyIndexArgs {
        IndexArgs: vec![
            IndexArg {
                IndexName: ast::NewCIStr("a"),
                ..Default::default()
            },
            IndexArg {
                IndexName: ast::NewCIStr("b"),
                Unique: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut job = v1_job(&multi, ActionAddIndex);
    let decoded = GetModifyIndexArgs(&mut job).unwrap();
    assert_eq!(2, decoded.IndexArgs.len());
    assert_eq!("b", decoded.IndexArgs[1].IndexName.O);
    assert!(decoded.IndexArgs[1].Unique);
}

#[test]
/// 完成态索引参数长度区分加/回滚加/删三种协议。
fn finished_index_layouts_match_add_drop_and_rollback_protocols() {
    let args = ModifyIndexArgs {
        IndexArgs: vec![IndexArg {
            IndexName: ast::NewCIStr("idx"),
            IndexID: 9,
            IfExist: true,
            IsGlobal: true,
            Global: true,
            ..Default::default()
        }],
        PartitionIDs: vec![7, 8],
        OpType: OpAddIndex,
    };
    let add_job = Job {
        tp: ActionAddIndex,
        ..Default::default()
    };
    assert_eq!(4, args.getFinishedArgsV1(&add_job).len());

    let rollback = ModifyIndexArgs {
        OpType: OpRollbackAddIndex,
        ..args
    };
    assert_eq!(3, rollback.getFinishedArgsV1(&add_job).len());

    let drop_args = ModifyIndexArgs {
        OpType: OpDropIndex,
        ..rollback
    };
    let drop_job = Job {
        tp: ActionDropIndex,
        ..Default::default()
    };
    assert_eq!(5, drop_args.getFinishedArgsV1(&drop_job).len());
}

#[test]
/// V2 首次解码后命中类型缓存，即使 RawArgs 损坏仍返回缓存。
fn v2_decodes_json_object_then_reuses_typed_cache() {
    let expected = ModifySchemaArgs {
        ToCharset: "utf8mb4".to_owned(),
        ToCollate: "utf8mb4_bin".to_owned(),
        ..Default::default()
    };
    let mut job = Job {
        version: JobVersion2,
        raw_args: serde_json::to_vec(&expected).unwrap(),
        ..Default::default()
    };

    let first = GetModifySchemaArgs(&mut job).unwrap();
    assert_eq!("utf8mb4", first.ToCharset);
    assert_eq!(1, job.args.len());
    job.raw_args = b"not-json".to_vec();
    let cached = GetModifySchemaArgs(&mut job).unwrap();
    assert_eq!("utf8mb4_bin", cached.ToCollate);
}

#[test]
/// 旧版仅设 IsColumnar 时 GetColumnarIndexType 回退为 Vector。
fn columnar_index_type_keeps_backward_compatibility() {
    let legacy = IndexArg {
        IsColumnar: true,
        ColumnarIndexType: ColumnarIndexTypeNA,
        ..Default::default()
    };
    assert_eq!(ColumnarIndexTypeVector, legacy.GetColumnarIndexType());
    let general = IndexArg::default();
    assert_eq!(ColumnarIndexTypeNA, general.GetColumnarIndexType());
}
