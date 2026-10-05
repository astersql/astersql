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

// Job 参数编解码与 Go 对齐的完整 V1/V2 往返测试。

use crate::group_2::*;
use crate::group_2::{serde, serde_json};
// 连接数据库、调用 PD 或执行外部业务动作。复杂的 Go require、泛型和 testify 语义保留为 Rust 断言。

// get_job_bytes 对应 Go 的 getJobBytes：构造指定版本与动作的 Job，填充普通参数后编码。
fn get_job_bytes<T>(in_args: &T, ver: JobVersion, tp: ActionType) -> Vec<u8>
where
    T: JobArgs + Clone + serde::Serialize + 'static,
{
    let mut j = Job {
        version: ver,
        tp,
        ..Default::default()
    };
    j.FillArgs(in_args.clone());
    j.Encode(true)
        .expect("Go require.NoError: Encode normal job args")
}

// get_finished_job_bytes 对应 Go 的 getFinishedJobBytes：使用 FillFinishedArgs 验证完成态参数协议。
fn get_finished_job_bytes<T>(in_args: &T, ver: JobVersion, tp: ActionType) -> Vec<u8>
where
    T: FinishedJobArgs + Clone + serde::Serialize + 'static,
{
    let mut j = Job {
        version: ver,
        tp,
        ..Default::default()
    };
    j.FillFinishedArgs(in_args.clone());
    j.Encode(true)
        .expect("Go require.NoError: Encode finished job args")
}

fn decode_job(bytes: Vec<u8>) -> Job {
    let mut j = Job::default();
    j.Decode(&bytes)
        .expect("Go require.NoError: Decode job bytes");
    j
}

#[test]
fn test_get_or_decode_args_v2() {
    let mut j = Job {
        version: JobVersion2,
        tp: ActionTruncateTable,
        ..Default::default()
    };
    j.FillArgs(TruncateTableArgs {
        FKCheck: true,
        ..Default::default()
    });
    let _ = j.Encode(true).expect("V2 encode should populate RawArgs");
    assert!(!j.raw_args.is_empty());

    // 第一次从 job.args 的 JSON 缓存返回，不重新读取 RawArgs。
    let args_v2 = getOrDecodeArgsV2::<TruncateTableArgs>(&mut j).expect("cached args");
    assert_eq!(j.args[0], serde_json::to_value(&args_v2).unwrap());

    // 清空缓存后应从 RawArgs JSON 重新解码，值相同但指针不同。
    j.args.clear();
    let decoded = getOrDecodeArgsV2::<TruncateTableArgs>(&mut j).expect("decode from raw json");
    assert_eq!(args_v2.FKCheck, decoded.FKCheck);
    assert!(decoded.FKCheck);
}

#[test]
fn v2_args_decode_go_json_field_names() {
    let mut job = Job {
        version: JobVersion2,
        tp: ActionTruncateTable,
        raw_args: br#"{"fk_check":true,"new_table_id":42,"new_partition_ids":[7]}"#.to_vec(),
        ..Default::default()
    };

    let args = GetTruncateTableArgs(&mut job).expect("decode Go V2 argument object");
    assert!(args.FKCheck);
    assert_eq!(42, args.NewTableID);
    assert_eq!(vec![7], args.NewPartitionIDs);

    let encoded = serde_json::to_value(TruncateTableArgs {
        FKCheck: true,
        ..Default::default()
    })
    .expect("encode Rust V2 argument object");
    assert_eq!(serde_json::json!({"fk_check": true}), encoded);
}

#[test]
fn test_create_schema_args() {
    let in_args = CreateSchemaArgs {
        DBInfo: Some(Box::new(DBInfo {
            ID: 100,
            ..Default::default()
        })),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionCreateSchema));
        let args = GetCreateSchemaArgs(&mut j2).expect("create schema args");
        assert_eq!(in_args.DBInfo, args.DBInfo);
    }
}

#[test]
fn test_drop_schema_args() {
    let in_args = DropSchemaArgs {
        FKCheck: true,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionDropSchema));
        let args = GetDropSchemaArgs(&mut j2).expect("drop schema args");
        assert_eq!(in_args.FKCheck, args.FKCheck);
    }

    let finished = DropSchemaArgs {
        AllDroppedTableIDs: vec![1, 2],
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_finished_job_bytes(&finished, v, ActionDropSchema));
        let args = GetFinishedDropSchemaArgs(&mut j2).expect("finished drop schema args");
        assert_eq!(vec![1, 2], args.AllDroppedTableIDs);
    }
}

#[test]
fn test_modify_schema_args() {
    let charset_args = ModifySchemaArgs {
        ToCharset: "aa".into(),
        ToCollate: "bb".into(),
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(
            &charset_args,
            v,
            ActionModifySchemaCharsetAndCollate,
        ));
        let args = GetModifySchemaArgs(&mut j2).expect("modify schema charset args");
        assert_eq!("aa", args.ToCharset);
        assert_eq!("bb", args.ToCollate);
    }

    // 默认 placement 的 V1 只编码 PolicyRef；nil 与非 nil 都要保持和 Go 兼容。
    for in_args in [
        ModifySchemaArgs {
            PolicyRef: Some(Box::new(PolicyRefInfo {
                ID: 123,
                ..Default::default()
            })),
            ..Default::default()
        },
        ModifySchemaArgs::default(),
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(
                &in_args,
                v,
                ActionModifySchemaDefaultPlacement,
            ));
            let args = GetModifySchemaArgs(&mut j2).expect("modify schema placement args");
            assert_eq!(in_args.PolicyRef, args.PolicyRef);
        }
    }
}

#[test]
fn test_create_table_args() {
    let create_table = CreateTableArgs {
        TableInfo: Some(Box::new(TableInfo {
            ID: 100,
            ..Default::default()
        })),
        FKCheck: true,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&create_table, v, ActionCreateTable));
        let args = GetCreateTableArgs(&mut j2).expect("create table args");
        assert_eq!(create_table.TableInfo, args.TableInfo);
        assert_eq!(create_table.FKCheck, args.FKCheck);
    }

    let create_view = CreateTableArgs {
        TableInfo: Some(Box::new(TableInfo {
            ID: 122,
            ..Default::default()
        })),
        OnExistReplace: true,
        OldViewTblID: 123,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&create_view, v, ActionCreateView));
        let args = GetCreateTableArgs(&mut j2).expect("create view args");
        assert_eq!(create_view.OnExistReplace, args.OnExistReplace);
        assert_eq!(create_view.OldViewTblID, args.OldViewTblID);
    }

    let create_sequence = CreateTableArgs {
        TableInfo: Some(Box::new(TableInfo {
            ID: 22,
            ..Default::default()
        })),
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&create_sequence, v, ActionCreateSequence));
        let args = GetCreateTableArgs(&mut j2).expect("create sequence args");
        assert_eq!(create_sequence.TableInfo, args.TableInfo);
    }
}

#[test]
fn test_batch_create_table_args() {
    let in_args = BatchCreateTableArgs {
        Tables: vec![
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 100,
                    ..Default::default()
                })),
                FKCheck: true,
                ..Default::default()
            },
            CreateTableArgs {
                TableInfo: Some(Box::new(TableInfo {
                    ID: 101,
                    ..Default::default()
                })),
                FKCheck: false,
                ..Default::default()
            },
        ],
    };

    // V1 兼容格式只保存一个 FKCheck，回读时会应用到所有 table args。
    let mut j2 = decode_job(get_job_bytes(&in_args, JobVersion1, ActionCreateTables));
    let args = GetBatchCreateTableArgs(&mut j2).expect("batch create table v1 args");
    for i in 0..in_args.Tables.len() {
        assert_eq!(in_args.Tables[i].TableInfo, args.Tables[i].TableInfo);
        assert_eq!(true, args.Tables[i].FKCheck);
    }

    let mut j2 = decode_job(get_job_bytes(&in_args, JobVersion2, ActionCreateTables));
    let args = GetBatchCreateTableArgs(&mut j2).expect("batch create table v2 args");
    assert_eq!(in_args.Tables, args.Tables);
}

#[test]
fn test_drop_table_args() {
    let in_args = DropTableArgs {
        Identifiers: vec![
            ast::Ident {
                Schema: ast::NewCIStr("db"),
                Name: ast::NewCIStr("tbl"),
            },
            ast::Ident {
                Schema: ast::NewCIStr("db2"),
                Name: ast::NewCIStr("tbl2"),
            },
        ],
        FKCheck: true,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionDropTable));
        let args = GetDropTableArgs(&mut j2).expect("drop table args");
        assert_eq!(in_args, args);
    }

    // DropView/DropSequence 的 V1 旧协议没有实际参数，RawArgs 应为 JSON null。
    for tp in [ActionDropView, ActionDropSequence] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            if v == JobVersion1 {
                assert_eq!(b"null", j2.raw_args.as_slice());
            } else {
                let args = GetDropTableArgs(&mut j2).expect("drop view/sequence args");
                assert_eq!(in_args, args);
            }
        }
    }
}

#[test]
fn test_finished_drop_table_args() {
    let in_args = DropTableArgs {
        StartKey: b"xxx".to_vec(),
        OldPartitionIDs: vec![1, 2],
        OldRuleIDs: vec!["schema/test/a/par1".into(), "schema/test/a/par2".into()],
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_finished_job_bytes(&in_args, v, ActionDropTable));
        let args = GetFinishedDropTableArgs(&mut j2).expect("finished drop table args");
        assert_eq!(in_args, args);
    }
}

#[test]
fn test_truncate_table_args() {
    let in_args = TruncateTableArgs {
        NewTableID: 1,
        FKCheck: true,
        OldPartitionIDs: vec![11, 2],
        NewPartitionIDs: vec![2, 3],
        ..Default::default()
    };
    for tp in [ActionTruncateTable, ActionTruncateTablePartition] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetTruncateTableArgs(&mut j2).expect("truncate args");
            if tp == ActionTruncateTable {
                assert_eq!(1, args.NewTableID);
                assert!(args.FKCheck);
            } else {
                assert_eq!(vec![11, 2], args.OldPartitionIDs);
            }
            assert_eq!(vec![2, 3], args.NewPartitionIDs);
        }
    }

    let finished = TruncateTableArgs {
        OldPartitionIDs: vec![5, 6],
        ..Default::default()
    };
    for tp in [ActionTruncateTable, ActionTruncateTablePartition] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_finished_job_bytes(&finished, v, tp));
            let args = GetFinishedTruncateTableArgs(&mut j2).expect("finished truncate args");
            assert_eq!(vec![5, 6], args.OldPartitionIDs);
        }
    }
}

#[test]
fn test_table_partition_args() {
    let in_args = TablePartitionArgs {
        PartNames: vec!["a".into(), "b".into()],
        PartInfo: Some(Box::new(PartitionInfo {
            Type: ast::PartitionTypeRange,
            Definitions: vec![
                PartitionDefinition {
                    ID: 1,
                    Name: ast::NewCIStr("a"),
                    LessThan: vec!["1".into()],
                    ..Default::default()
                },
                PartitionDefinition {
                    ID: 2,
                    Name: ast::NewCIStr("b"),
                    LessThan: vec!["2".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        })),
        ..Default::default()
    };

    for tp in [
        ActionAlterTablePartitioning,
        ActionRemovePartitioning,
        ActionReorganizePartition,
        ActionAddTablePartition,
        ActionDropTablePartition,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetTablePartitionArgs(&mut j2).expect("table partition args");
            if v == JobVersion2 {
                assert_eq!(in_args, args);
            } else {
                // V1 按动作裁剪字段：Add 不回读 PartNames，Drop 回读空 PartitionInfo。
                if j2.tp != ActionAddTablePartition {
                    assert_eq!(in_args.PartNames, args.PartNames);
                }
                if j2.tp != ActionDropTablePartition {
                    assert_eq!(in_args.PartInfo, args.PartInfo);
                } else {
                    assert_eq!(PartitionInfo::default(), *args.PartInfo.unwrap());
                }
            }
        }
    }

    // V2 DropTablePartition 缺 PartInfo 时也要补空对象，避免调用方解引用 nil。
    let mut j2 = decode_job(get_job_bytes(
        &TablePartitionArgs {
            PartNames: vec!["a".into(), "b".into()],
            ..Default::default()
        },
        JobVersion2,
        ActionDropTablePartition,
    ));
    let args = GetTablePartitionArgs(&mut j2).expect("drop partition should fill empty PartInfo");
    assert_eq!(PartitionInfo::default(), *args.PartInfo.unwrap());

    for ver in [JobVersion1, JobVersion2] {
        let mut j = Job {
            version: ver,
            tp: ActionAddTablePartition,
            ..Default::default()
        };
        j.FillArgs(in_args.clone());
        let _ = j.Encode(true).expect("encode add partition args");
        let part_names = vec!["aaaa".into(), "bbb".into()];
        FillRollbackArgsForAddPartition(
            &mut j,
            &TablePartitionArgs {
                PartNames: part_names.clone(),
                PartInfo: Some(Box::new(PartitionInfo {
                    Type: ast::PartitionTypeRange,
                    Definitions: vec![
                        PartitionDefinition {
                            ID: 1,
                            Name: ast::NewCIStr("aaaa"),
                            LessThan: vec!["1".into()],
                            ..Default::default()
                        },
                        PartitionDefinition {
                            ID: 2,
                            Name: ast::NewCIStr("bbb"),
                            LessThan: vec!["2".into()],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                })),
                ..Default::default()
            },
        );
        assert_eq!(1, j.args.len());
        if ver == JobVersion1 {
            assert_eq!(
                part_names,
                serde_json::from_value::<Vec<String>>(j.args[0].clone()).unwrap()
            );
        } else {
            let rollback_args =
                serde_json::from_value::<TablePartitionArgs>(j.args[0].clone()).unwrap();
            assert_eq!(part_names, rollback_args.PartNames);
            assert!(rollback_args.PartInfo.is_none());
            assert!(rollback_args.OldPhysicalTblIDs.is_empty());
        }

        j.state = JobStateRollingback;
        let bytes = j.Encode(true).expect("encode rollingback partition args");
        let mut j2 = decode_job(bytes);
        let args = GetTablePartitionArgs(&mut j2).expect("decode rollingback partition args");
        assert_eq!(part_names, args.PartNames);
        assert_eq!(PartitionInfo::default(), *args.PartInfo.unwrap());
    }
}

#[test]
fn test_finished_table_partition_args() {
    let in_args = TablePartitionArgs {
        OldPhysicalTblIDs: vec![1, 2],
        ..Default::default()
    };
    for tp in [
        ActionAlterTablePartitioning,
        ActionRemovePartitioning,
        ActionReorganizePartition,
        ActionDropTablePartition,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_finished_job_bytes(&in_args, v, tp));
            let args = GetFinishedTablePartitionArgs(&mut j2).expect("finished partition args");
            assert_eq!(in_args.OldPhysicalTblIDs, args.OldPhysicalTblIDs);
        }
    }

    // AddTablePartition 回滚完成时也允许走 FillFinishedArgs。
    for ver in [JobVersion1, JobVersion2] {
        let mut j = Job {
            version: ver,
            tp: ActionAddTablePartition,
            state: JobStateRollbackDone,
            ..Default::default()
        };
        j.FillFinishedArgs(in_args.clone());
        let mut j2 = decode_job(j.Encode(true).expect("encode rollback-done partition args"));
        let args =
            GetFinishedTablePartitionArgs(&mut j2).expect("finished rollback add partition args");
        assert_eq!(in_args.OldPhysicalTblIDs, args.OldPhysicalTblIDs);
    }
}

#[test]
fn test_exchange_table_partition_args() {
    let in_args = ExchangeTablePartitionArgs {
        PartitionID: 100,
        PTSchemaID: 123,
        PTTableID: 345,
        PartitionName: "c".into(),
        WithValidation: true,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionExchangeTablePartition));
        assert_eq!(
            in_args,
            GetExchangeTablePartitionArgs(&mut j2).expect("exchange partition args")
        );
    }
}

#[test]
fn test_alter_table_partition_args() {
    let in_args = AlterTablePartitionArgs {
        PartitionID: 123,
        LabelRule: Some(Box::new(pdhttp::LabelRule {
            ID: "ss".into(),
            ..Default::default()
        })),
        PolicyRefInfo: Some(Box::new(PolicyRefInfo {
            ID: 462,
            ..Default::default()
        })),
    };
    for tp in [
        ActionAlterTablePartitionAttributes,
        ActionAlterTablePartitionPlacement,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetAlterTablePartitionArgs(&mut j2).expect("alter table partition args");
            assert_eq!(in_args.PartitionID, args.PartitionID);
            if tp == ActionAlterTablePartitionAttributes {
                assert_eq!(in_args.LabelRule, args.LabelRule);
            } else {
                assert_eq!(in_args.PolicyRefInfo, args.PolicyRefInfo);
            }
        }
    }
}

#[test]
fn test_rename_table_args() {
    let in_args = RenameTableArgs {
        OldSchemaID: 9527,
        OldSchemaName: ast::NewCIStr("old_schema_name"),
        NewTableName: ast::NewCIStr("new_table_name"),
        ..Default::default()
    };
    for jobver in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, jobver, ActionRenameTable));
        assert_eq!(
            in_args,
            GetRenameTableArgs(&mut j2).expect("rename table args")
        );
    }
}

#[test]
fn test_rename_tables_args() {
    let in_args = RenameTablesArgs {
        RenameTableInfos: vec![
            RenameTableArgs {
                OldSchemaID: 1,
                OldSchemaName: ast::CIStr {
                    O: "db1".into(),
                    L: "db1".into(),
                },
                NewTableName: ast::CIStr {
                    O: "tb3".into(),
                    L: "tb3".into(),
                },
                OldTableName: ast::CIStr {
                    O: "tb1".into(),
                    L: "tb1".into(),
                },
                NewSchemaID: 3,
                TableID: 100,
                ..Default::default()
            },
            RenameTableArgs {
                OldSchemaID: 2,
                OldSchemaName: ast::CIStr {
                    O: "db2".into(),
                    L: "db2".into(),
                },
                NewTableName: ast::CIStr {
                    O: "tb2".into(),
                    L: "tb2".into(),
                },
                OldTableName: ast::CIStr {
                    O: "tb4".into(),
                    L: "tb4".into(),
                },
                NewSchemaID: 3,
                TableID: 101,
                ..Default::default()
            },
        ],
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionRenameTables));
        let args = GetRenameTablesArgs(&mut j2).expect("rename tables args");
        assert_eq!(in_args.RenameTableInfos[0], args.RenameTableInfos[0]);
        assert_eq!(in_args.RenameTableInfos[1], args.RenameTableInfos[1]);
        if v == JobVersion1 {
            // 老 TiDB 可能截断最后一个参数；Go 测试确认 Decode 后 GetRenameTablesArgs 仍兼容。
            let mut raw_args: Vec<serde_json::Value> =
                serde_json::from_slice(&j2.raw_args).unwrap();
            raw_args.pop();
            j2.raw_args = serde_json::to_vec(&raw_args).unwrap();
            let bytes = j2
                .Encode(true)
                .expect("encode truncated v1 rename tables args");
            j2.Decode(&bytes)
                .expect("decode truncated v1 rename tables args");
            let _ = GetRenameTablesArgs(&mut j2).expect("truncated v1 rename tables args");
        }
    }
}

#[test]
fn test_resource_group_args() {
    let in_args = ResourceGroupArgs {
        RGInfo: Some(Box::new(ResourceGroupInfo {
            ID: 100,
            Name: ast::NewCIStr("rg_name"),
            ..Default::default()
        })),
    };
    for tp in [
        ActionCreateResourceGroup,
        ActionAlterResourceGroup,
        ActionDropResourceGroup,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetResourceGroupArgs(&mut j2).expect("resource group args");
            if tp == ActionDropResourceGroup {
                assert_eq!(
                    in_args.RGInfo.as_ref().unwrap().Name,
                    args.RGInfo.as_ref().unwrap().Name
                );
            } else {
                assert_eq!(in_args, args);
            }
        }
    }
}

#[test]
fn test_get_alter_sequence_args() {
    let in_args = AlterSequenceArgs {
        Ident: ast::Ident {
            Schema: ast::NewCIStr("test_db"),
            Name: ast::NewCIStr("test_t"),
        },
        SeqOptions: vec![
            ast::SequenceOption {
                Tp: ast::SequenceOptionIncrementBy,
                IntValue: 7527,
                ..Default::default()
            },
            ast::SequenceOption {
                Tp: ast::SequenceCache,
                IntValue: 9528,
                ..Default::default()
            },
        ],
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAlterSequence));
        assert_eq!(
            in_args,
            GetAlterSequenceArgs(&mut j2).expect("alter sequence args")
        );
    }
}

#[test]
fn test_get_rebase_auto_id_args() {
    let in_args = RebaseAutoIDArgs {
        NewBase: 9527,
        Force: true,
    };
    for tp in [ActionRebaseAutoID, ActionRebaseAutoRandomBase] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            assert_eq!(
                in_args,
                GetRebaseAutoIDArgs(&mut j2).expect("rebase auto id args")
            );
        }
    }
}

#[test]
fn test_get_modify_table_comment_args() {
    let in_args = ModifyTableCommentArgs {
        Comment: "TiDB is great".into(),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionModifyTableComment));
        assert_eq!(
            in_args,
            GetModifyTableCommentArgs(&mut j2).expect("modify table comment args")
        );
    }
}

#[test]
fn test_get_alter_index_visibility_args() {
    let in_args = AlterIndexVisibilityArgs {
        IndexName: ast::NewCIStr("index-name"),
        Invisible: true,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAlterIndexVisibility));
        assert_eq!(
            in_args,
            GetAlterIndexVisibilityArgs(&mut j2).expect("alter index visibility args")
        );
    }
}

#[test]
fn test_get_add_foreign_key_args() {
    let in_args = AddForeignKeyArgs {
        FkInfo: Some(Box::new(FKInfo {
            ID: 7527,
            Name: ast::NewCIStr("fk-name"),
            ..Default::default()
        })),
        FkCheck: true,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddForeignKey));
        assert_eq!(
            in_args,
            GetAddForeignKeyArgs(&mut j2).expect("add foreign key args")
        );
    }
}

#[test]
fn test_get_modify_table_auto_id_cache_args() {
    let in_args = ModifyTableAutoIDCacheArgs { NewCache: 7527 };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionModifyTableAutoIDCache));
        assert_eq!(
            in_args,
            GetModifyTableAutoIDCacheArgs(&mut j2).expect("modify auto id cache args")
        );
    }
}

#[test]
fn test_get_shard_row_id_args() {
    let in_args = ShardRowIDArgs {
        ShardRowIDBits: 101,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionShardRowID));
        assert_eq!(
            in_args,
            GetShardRowIDArgs(&mut j2).expect("shard row id args")
        );
    }
}

#[test]
fn test_get_drop_foreign_key_args() {
    let in_args = DropForeignKeyArgs {
        FkName: ast::NewCIStr("fk-name"),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionDropForeignKey));
        assert_eq!(
            in_args,
            GetDropForeignKeyArgs(&mut j2).expect("drop foreign key args")
        );
    }
}

#[test]
fn test_get_alter_ttl_info_args() {
    let ttl_enable = true;
    let ttl_cron_job_schedule = "ttl-schedule".to_string();
    let in_args = AlterTTLInfoArgs {
        TTLInfo: Some(Box::new(TTLInfo {
            ColumnName: ast::NewCIStr("column_name"),
            IntervalExprStr: "1".into(),
            IntervalTimeUnit: 10010,
            ..Default::default()
        })),
        TTLEnable: Some(ttl_enable),
        TTLCronJobSchedule: Some(ttl_cron_job_schedule),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAlterTTLInfo));
        assert_eq!(
            in_args,
            GetAlterTTLInfoArgs(&mut j2).expect("alter ttl info args")
        );
    }
}

#[test]
fn test_add_check_constraint_args() {
    let constraint = ConstraintInfo {
        Name: ast::NewCIStr("t3_c1"),
        Table: ast::NewCIStr("t3"),
        ExprString: "id<10".into(),
        State: StateDeleteOnly,
        ..Default::default()
    };
    let in_args = AddCheckConstraintArgs {
        Constraint: Some(Box::new(constraint)),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddCheckConstraint));
        let args = GetAddCheckConstraintArgs(&mut j2).expect("add check constraint args");
        assert_eq!("t3_c1", args.Constraint.as_ref().unwrap().Name.O);
        assert_eq!("t3", args.Constraint.as_ref().unwrap().Table.O);
        assert_eq!("id<10", args.Constraint.as_ref().unwrap().ExprString);
        assert_eq!(StateDeleteOnly, args.Constraint.as_ref().unwrap().State);
    }
}

#[test]
fn test_check_constraint_args() {
    let in_args = CheckConstraintArgs {
        ConstraintName: ast::NewCIStr("c1"),
        Enforced: true,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionDropCheckConstraint));
        let args = GetCheckConstraintArgs(&mut j2).expect("check constraint args");
        assert_eq!("c1", args.ConstraintName.O);
        assert!(args.Enforced);
    }
}

#[test]
fn test_get_alter_table_placement_args() {
    for in_args in [
        AlterTablePlacementArgs {
            PlacementPolicyRef: Some(Box::new(PolicyRefInfo {
                ID: 7527,
                Name: ast::NewCIStr("placement-policy"),
                ..Default::default()
            })),
        },
        AlterTablePlacementArgs {
            PlacementPolicyRef: None,
        },
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAlterTablePlacement));
            assert_eq!(
                in_args,
                GetAlterTablePlacementArgs(&mut j2).expect("alter table placement args")
            );
        }
    }
}

#[test]
fn test_get_set_tiflash_replica_args() {
    let in_args = SetTiFlashReplicaArgs {
        TiflashReplica: ast::TiFlashReplicaSpec {
            Count: 3,
            Labels: vec!["TiFlash1".into(), "TiFlash2".into(), "TiFlash3".into()],
            Hypo: true,
            ..Default::default()
        },
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionSetTiFlashReplica));
        let args = GetSetTiFlashReplicaArgs(&mut j2).expect("set tiflash replica args");
        assert_eq!(in_args, args);
        assert_eq!(false, args.ResetAvailable);
    }

    let with_reset = SetTiFlashReplicaArgs {
        ResetAvailable: true,
        ..in_args.clone()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j3 = decode_job(get_job_bytes(&with_reset, v, ActionSetTiFlashReplica));
        let args = GetSetTiFlashReplicaArgs(&mut j3).expect("set tiflash replica reset args");
        assert_eq!(with_reset.TiflashReplica, args.TiflashReplica);
        if v == JobVersion2 {
            assert_eq!(true, args.ResetAvailable);
        }
    }

    let with_skip_gate = SetTiFlashReplicaArgs {
        ResetAvailable: true,
        SkipColumnarStorageGate: true,
        ..in_args.clone()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j4 = decode_job(get_job_bytes(&with_skip_gate, v, ActionSetTiFlashReplica));
        let args = GetSetTiFlashReplicaArgs(&mut j4).expect("set tiflash replica skip-gate args");
        assert_eq!(with_skip_gate.TiflashReplica, args.TiflashReplica);
        if v == JobVersion2 {
            assert!(args.ResetAvailable);
            assert!(args.SkipColumnarStorageGate);
        } else {
            assert!(!args.SkipColumnarStorageGate);
        }
    }
}

#[test]
fn test_get_update_tiflash_replica_status_args() {
    let in_args = UpdateTiFlashReplicaStatusArgs {
        Available: true,
        PhysicalID: 1001,
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionUpdateTiFlashReplicaStatus));
        assert_eq!(
            in_args,
            GetUpdateTiFlashReplicaStatusArgs(&mut j2).expect("update tiflash replica args")
        );
    }
}

#[test]
fn test_lock_table_args() {
    let in_args = LockTablesArgs {
        LockTables: vec![TableLockTpInfo(1, 1, ast::TableLockNone)],
        UnlockTables: vec![TableLockTpInfo(2, 2, ast::TableLockNone)],
        IndexOfLock: 13,
        IndexOfUnlock: 24,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        for tp in [ActionLockTable, ActionUnlockTable] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetLockTablesArgs(&mut j2).expect("lock tables args");
            assert_eq!(in_args.LockTables, args.LockTables);
            assert_eq!(in_args.UnlockTables, args.UnlockTables);
            assert_eq!(in_args.IndexOfLock, args.IndexOfLock);
            assert_eq!(in_args.IndexOfUnlock, args.IndexOfUnlock);
        }
    }
}

#[test]
fn test_repair_table_args() {
    let in_args = RepairTableArgs {
        TableInfo: Some(Box::new(TableInfo {
            ID: 1,
            Name: ast::NewCIStr("t"),
            ..Default::default()
        })),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionRepairTable));
        let args = GetRepairTableArgs(&mut j2).expect("repair table args");
        assert_eq!(in_args.TableInfo, args.TableInfo);
    }
}

#[test]
fn test_recover_args() {
    let recover_info = RecoverTableInfo {
        SchemaID: 1,
        DropJobID: 2,
        TableInfo: Some(Box::new(TableInfo {
            ID: 100,
            Name: ast::NewCIStr("table"),
            ..Default::default()
        })),
        OldSchemaName: "old".into(),
        OldTableName: "table".into(),
        ..Default::default()
    };
    let in_args = RecoverArgs {
        RecoverInfo: Some(Box::new(RecoverSchemaInfo {
            RecoverTableInfos: vec![recover_info],
            ..Default::default()
        })),
        CheckFlag: 2,
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        for tp in [ActionRecoverTable, ActionRecoverSchema] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            let args = GetRecoverArgs(&mut j2).expect("recover args");
            assert_eq!(in_args.CheckFlag, args.CheckFlag);
            assert_eq!(in_args.RecoverInfo, args.RecoverInfo);
        }
    }
}

#[test]
fn test_placement_policy_args() {
    let in_args = PlacementPolicyArgs {
        Policy: Some(Box::new(PolicyInfo {
            ID: 1,
            Name: ast::NewCIStr("policy"),
            State: StateDeleteOnly,
            ..Default::default()
        })),
        PolicyName: ast::NewCIStr("policy_name"),
        PolicyID: 123,
        ReplaceOnExist: false,
    };
    for tp in [
        ActionCreatePlacementPolicy,
        ActionAlterPlacementPolicy,
        ActionDropPlacementPolicy,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            j2.schema_id = in_args.PolicyID;
            let args = GetPlacementPolicyArgs(&mut j2).expect("placement policy args");
            if tp == ActionCreatePlacementPolicy {
                assert_eq!(in_args.Policy, args.Policy);
                assert_eq!(in_args.ReplaceOnExist, args.ReplaceOnExist);
            } else if tp == ActionAlterPlacementPolicy {
                assert_eq!(in_args.Policy, args.Policy);
                assert_eq!(in_args.PolicyID, args.PolicyID);
            } else {
                assert_eq!(in_args.PolicyName, args.PolicyName);
                assert_eq!(in_args.PolicyID, args.PolicyID);
            }
        }
    }
}

#[test]
fn test_masking_policy_args() {
    let in_args = MaskingPolicyArgs {
        Policy: Some(Box::new(MaskingPolicyInfo {
            ID: 1,
            Name: ast::NewCIStr("policy"),
            State: StateDeleteOnly,
            ..Default::default()
        })),
        PolicyName: ast::NewCIStr("policy_name"),
        PolicyID: 123,
        ReplaceOnExist: false,
    };
    for tp in [
        ActionCreateMaskingPolicy,
        ActionAlterMaskingPolicy,
        ActionDropMaskingPolicy,
    ] {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(&in_args, v, tp));
            j2.schema_id = in_args.PolicyID;
            let args = GetMaskingPolicyArgs(&mut j2).expect("masking policy args");
            if tp == ActionCreateMaskingPolicy {
                assert_eq!(in_args.Policy, args.Policy);
                assert_eq!(in_args.ReplaceOnExist, args.ReplaceOnExist);
            } else if tp == ActionAlterMaskingPolicy {
                assert_eq!(in_args.Policy, args.Policy);
                assert_eq!(in_args.PolicyID, args.PolicyID);
            } else {
                assert_eq!(in_args.PolicyName, args.PolicyName);
                assert_eq!(in_args.PolicyID, args.PolicyID);
            }
        }
    }
}

#[test]
fn test_get_set_default_value_args() {
    let in_args = SetDefaultValueArgs {
        Col: Some(Box::new(ColumnInfo {
            ID: 7527,
            Name: ast::NewCIStr("col_name"),
            ..Default::default()
        })),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionSetDefaultValue));
        assert_eq!(
            in_args,
            GetSetDefaultValueArgs(&mut j2).expect("set default value args")
        );
    }
}

#[test]
fn test_flashback_cluster_args() {
    let in_args = FlashbackClusterArgs {
        FlashbackTS: 111,
        StartTS: 222,
        CommitTS: 333,
        EnableGC: true,
        EnableAutoAnalyze: true,
        EnableTTLJob: true,
        SuperReadOnly: true,
        LockedRegionCnt: 444,
        PDScheduleValue: std::collections::HashMap::from([(
            "t1".to_string(),
            serde_json::Value::from(123.0),
        )]),
        FlashbackKeyRanges: vec![
            KeyRange {
                StartKey: b"db1".to_vec(),
                EndKey: b"db2".to_vec(),
            },
            KeyRange {
                StartKey: b"db2".to_vec(),
                EndKey: b"db3".to_vec(),
            },
        ],
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionFlashbackCluster));
        assert_eq!(
            in_args,
            GetFlashbackClusterArgs(&mut j2).expect("flashback cluster args")
        );
    }
}

#[test]
fn test_drop_column_args() {
    let in_args = TableColumnArgs {
        Col: Some(Box::new(ColumnInfo {
            Name: ast::NewCIStr("col_name"),
            ..Default::default()
        })),
        IgnoreExistenceErr: true,
        IndexIDs: vec![1, 2, 3],
        PartitionIDs: vec![4, 5, 6],
        Pos: Some(Box::new(ast::ColumnPosition::default())),
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionDropColumn));
        let args = GetTableColumnArgs(&mut j2).expect("drop column args");
        assert_eq!(in_args, args);
        if v == JobVersion1 {
            assert_eq!(4, j2.args.len());
        }
    }

    // V1 最小参数只应编码两段 RawArgs，保持旧协议兼容。
    let j2 = decode_job(get_job_bytes(
        &TableColumnArgs {
            Col: Some(Box::new(ColumnInfo::default())),
            ..Default::default()
        },
        JobVersion1,
        ActionDropColumn,
    ));
    let raw_args: Vec<serde_json::Value> =
        serde_json::from_slice(&j2.raw_args).expect("decode raw args");
    assert_eq!(2, raw_args.len());
}

#[test]
fn test_add_column_args() {
    let in_args = TableColumnArgs {
        Col: Some(Box::new(ColumnInfo {
            ID: 7527,
            Name: ast::NewCIStr("col_name"),
            ..Default::default()
        })),
        Pos: Some(Box::new(ast::ColumnPosition {
            Tp: ast::ColumnPositionFirst,
            ..Default::default()
        })),
        Offset: 1001,
        IgnoreExistenceErr: true,
        ..Default::default()
    };
    let drop_args = TableColumnArgs {
        Col: Some(Box::new(ColumnInfo {
            Name: ast::NewCIStr("drop_column"),
            ..Default::default()
        })),
        Pos: Some(Box::new(ast::ColumnPosition::default())),
        ..Default::default()
    };

    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddColumn));
        assert_eq!(
            in_args,
            GetTableColumnArgs(&mut j2).expect("add column args")
        );
        FillRollBackArgsForAddColumn(&mut j2, drop_args.clone());
        j2.state = JobStateRollingback;
        let mut j3 = decode_job(j2.Encode(true).expect("encode add-column rollback args"));
        assert_eq!(
            drop_args,
            GetTableColumnArgs(&mut j3).expect("decode add-column rollback args")
        );
    }
}

#[test]
fn test_alter_table_attributes_args() {
    let in_args = AlterTableAttributesArgs {
        LabelRule: Some(Box::new(pdhttp::LabelRule {
            ID: "id".into(),
            Index: 2,
            RuleType: "rule".into(),
            Labels: vec![pdhttp::RegionLabel {
                Key: "key".into(),
                Value: "value".into(),
            }],
            ..Default::default()
        })),
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAlterTableAttributes));
        let args = GetAlterTableAttributesArgs(&mut j2).expect("alter table attributes args");
        assert_eq!(
            in_args.LabelRule.as_ref().unwrap(),
            args.LabelRule.as_ref().unwrap()
        );
    }
}

#[test]
fn test_add_index_args() {
    let mut in_args = ModifyIndexArgs {
        IndexArgs: vec![IndexArg {
            Global: false,
            Unique: true,
            IndexName: ast::NewCIStr("idx1"),
            IndexPartSpecifications: vec![ast::IndexPartSpecification {
                Length: 2,
                ..Default::default()
            }],
            IndexOption: Some(Box::new(ast::IndexOption::default())),
            HiddenCols: vec![ColumnInfo::default(), ColumnInfo::default()],
            SQLMode: mysql::ModeANSI,
            IndexID: 1,
            IfExist: false,
            IsGlobal: false,
            FuncExpr: "test_string".into(),
            ..Default::default()
        }],
        PartitionIDs: vec![100, 101, 102],
        OpType: OpAddIndex,
    };

    for v in [JobVersion1, JobVersion2] {
        in_args.IndexArgs[0].IsColumnar = false;
        in_args.IndexArgs[0].IsPK = false;
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddIndex));
        let args = GetModifyIndexArgs(&mut j2).expect("add index args");
        let a = &args.IndexArgs[0];
        assert_eq!(in_args.IndexArgs[0].Global, a.Global);
        assert_eq!(in_args.IndexArgs[0].Unique, a.Unique);
        assert_eq!(in_args.IndexArgs[0].IndexName, a.IndexName);
        assert_eq!(
            in_args.IndexArgs[0].IndexPartSpecifications,
            a.IndexPartSpecifications
        );
        assert_eq!(in_args.IndexArgs[0].IndexOption, a.IndexOption);
        assert_eq!(in_args.IndexArgs[0].HiddenCols, a.HiddenCols);
    }

    for v in [JobVersion1, JobVersion2] {
        in_args.IndexArgs[0].IsColumnar = false;
        in_args.IndexArgs[0].IsPK = true;
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddPrimaryKey));
        let args = GetModifyIndexArgs(&mut j2).expect("add primary key args");
        let a = &args.IndexArgs[0];
        assert_eq!(in_args.IndexArgs[0].Global, a.Global);
        assert_eq!(in_args.IndexArgs[0].Unique, a.Unique);
        assert_eq!(in_args.IndexArgs[0].IndexName, a.IndexName);
        assert_eq!(
            in_args.IndexArgs[0].IndexPartSpecifications,
            a.IndexPartSpecifications
        );
        assert_eq!(in_args.IndexArgs[0].SQLMode, a.SQLMode);
        assert_eq!(in_args.IndexArgs[0].IndexOption, a.IndexOption);
    }

    for v in [JobVersion1, JobVersion2] {
        in_args.IndexArgs[0].IsColumnar = true;
        in_args.IndexArgs[0].IsPK = false;
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddColumnarIndex));
        let args = GetModifyIndexArgs(&mut j2).expect("add columnar index args");
        let a = &args.IndexArgs[0];
        assert_eq!(in_args.IndexArgs[0].IsColumnar, a.IsColumnar);
        assert_eq!(in_args.IndexArgs[0].IndexName, a.IndexName);
        assert_eq!(
            in_args.IndexArgs[0].IndexPartSpecifications,
            a.IndexPartSpecifications
        );
        assert_eq!(in_args.IndexArgs[0].IndexOption, a.IndexOption);
        assert_eq!(in_args.IndexArgs[0].FuncExpr, a.FuncExpr);
    }

    for v in [JobVersion1, JobVersion2] {
        in_args.IndexArgs[0].ColumnarIndexType = ColumnarIndexTypeInverted;
        in_args.IndexArgs[0].IsColumnar = true;
        in_args.IndexArgs[0].IsPK = false;
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionAddColumnarIndex));
        let args = GetModifyIndexArgs(&mut j2).expect("add inverted columnar index args");
        let a = &args.IndexArgs[0];
        assert_eq!(in_args.IndexArgs[0].ColumnarIndexType, a.ColumnarIndexType);
        assert_eq!(in_args.IndexArgs[0].IsColumnar, a.IsColumnar);
        assert_eq!(in_args.IndexArgs[0].IndexName, a.IndexName);
        assert_eq!(
            in_args.IndexArgs[0].IndexPartSpecifications,
            a.IndexPartSpecifications
        );
        assert_eq!(in_args.IndexArgs[0].IndexOption, a.IndexOption);
        assert_eq!(in_args.IndexArgs[0].FuncExpr, a.FuncExpr);
    }

    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_finished_job_bytes(&in_args, v, ActionAddIndex));
        let args = GetFinishedModifyIndexArgs(&mut j2).expect("finished add index args");
        let a = &args.IndexArgs[0];
        assert_eq!(in_args.IndexArgs[0].IndexID, a.IndexID);
        assert_eq!(in_args.IndexArgs[0].IfExist, a.IfExist);
        assert_eq!(in_args.IndexArgs[0].IsGlobal, a.IsGlobal);
        assert_eq!(in_args.PartitionIDs, args.PartitionIDs);
    }
}

#[test]
fn test_drop_index_arguements() {
    // 保留 Go 源文件中的拼写 Arguements；check_func 对普通参数和完成态参数各跑一遍。
    let check_func = |in_args: &ModifyIndexArgs| {
        for v in [JobVersion1, JobVersion2] {
            let mut j2 = decode_job(get_job_bytes(in_args, v, ActionDropIndex));
            let args = GetDropIndexArgs(&mut j2).expect("drop index args");
            for (i, expect) in in_args.IndexArgs.iter().enumerate() {
                assert_eq!(expect.IndexName, args.IndexArgs[i].IndexName);
                assert_eq!(expect.IfExist, args.IndexArgs[i].IfExist);
            }

            let mut j2 = decode_job(get_finished_job_bytes(in_args, v, ActionDropIndex));
            let args2 = GetFinishedModifyIndexArgs(&mut j2).expect("finished drop index args");
            assert_eq!(in_args.IndexArgs, args2.IndexArgs);
            assert_eq!(in_args.PartitionIDs, args2.PartitionIDs);
        }
    };

    let in_args = ModifyIndexArgs {
        IndexArgs: vec![IndexArg {
            IndexName: ast::NewCIStr("i2"),
            IfExist: true,
            IsColumnar: true,
            IndexID: 1,
            ..Default::default()
        }],
        PartitionIDs: vec![100, 101, 102, 103],
        OpType: OpDropIndex,
    };
    check_func(&in_args);
}

#[test]
fn test_get_rename_index_args() {
    let in_args = ModifyIndexArgs {
        IndexArgs: vec![
            IndexArg {
                IndexName: ast::NewCIStr("old"),
                ..Default::default()
            },
            IndexArg {
                IndexName: ast::NewCIStr("new"),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionRenameIndex));
        assert_eq!(
            in_args,
            GetModifyIndexArgs(&mut j2).expect("rename index args")
        );
    }
}

#[test]
fn test_modify_columns_args() {
    let in_args = ModifyColumnArgs {
        Column: Some(Box::new(ColumnInfo {
            ID: 111,
            Name: ast::NewCIStr("col1"),
            ..Default::default()
        })),
        OldColumnName: ast::NewCIStr("aa"),
        Position: Some(Box::new(ast::ColumnPosition {
            Tp: ast::ColumnPositionFirst,
            ..Default::default()
        })),
        ModifyColumnType: 1,
        NewShardBits: 123,
        ChangingColumn: Some(Box::new(ColumnInfo {
            ID: 222,
            Name: ast::NewCIStr("col2"),
            ..Default::default()
        })),
        RedundantIdxs: vec![1, 2],
        IndexIDs: vec![3, 4],
        PartitionIDs: vec![5, 6],
        ..Default::default()
    };

    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_job_bytes(&in_args, v, ActionModifyColumn));
        let args = GetModifyColumnArgs(&mut j2).expect("modify column args");
        assert_eq!(
            *in_args.Column.as_ref().unwrap(),
            *args.Column.as_ref().unwrap()
        );
        assert_eq!(in_args.OldColumnName, args.OldColumnName);
        assert_eq!(in_args.Position, args.Position);
        assert_eq!(in_args.ModifyColumnType, args.ModifyColumnType);
        assert_eq!(in_args.NewShardBits, args.NewShardBits);
        assert_eq!(
            *in_args.ChangingColumn.as_ref().unwrap(),
            *args.ChangingColumn.as_ref().unwrap()
        );
        assert_eq!(in_args.ChangingIdxs, args.ChangingIdxs);
        assert_eq!(in_args.RedundantIdxs, args.RedundantIdxs);
        if v == JobVersion1 {
            let raw_args: Vec<serde_json::Value> =
                serde_json::from_slice(&j2.raw_args).expect("decode modify column raw args");
            assert_eq!(9, raw_args.len());
        }
    }

    for v in [JobVersion1, JobVersion2] {
        let mut j2 = decode_job(get_finished_job_bytes(&in_args, v, ActionModifyColumn));
        let args = GetFinishedModifyColumnArgs(&mut j2).expect("finished modify column args");
        assert_eq!(in_args.IndexIDs, args.IndexIDs);
        assert_eq!(in_args.PartitionIDs, args.PartitionIDs);
    }

    let j2 = decode_job(get_job_bytes(
        &ModifyColumnArgs::default(),
        JobVersion1,
        ActionModifyColumn,
    ));
    let raw_args: Vec<serde_json::Value> =
        serde_json::from_slice(&j2.raw_args).expect("decode empty modify column raw args");
    assert_eq!(5, raw_args.len());
}
// 实际可执行测试使用 group3 的 Job 克隆路径。
use crate::group_3::{
    ACTION_ADD_INDEX as FULL_ACTION_ADD_INDEX, Job as FullJob, JobVersion as FullJobVersion,
};

#[test]
/// clone_job 后动作类型与 JobVersion 保持不变。
fn job_version_and_action_survive_clone() {
    let mut job = FullJob::default();
    job.tp = FULL_ACTION_ADD_INDEX;
    job.version = FullJobVersion::V1;
    let cloned = job.clone_job().expect("clone job");
    assert_eq!(cloned.tp, FULL_ACTION_ADD_INDEX);
    assert_eq!(cloned.version, FullJobVersion::V1);
}
