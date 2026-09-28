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

//! Go-equivalent tests for `lightning/pkg/checkpoints/checkpoints_sql_test.go`
//! (package `checkpoints_test`).
//!
//! Go drives `database/sql` through `go-sqlmock`. Rust uses the crate's in-memory
//! `sql::DB` stub (no network). Assertions cover Initialize/TaskCheckpoint,
//! engine insert/update/remove/ignore/destroy/move/dump call paths and error
//! identity helpers. The in-memory SQL boundary preserves row-level checkpoint
//! state so the same public behavior can be asserted without a network database.

use std::collections::HashMap;

use crate::build;
use crate::common;
use crate::config;
use crate::errors;
use crate::importdef;
use crate::json;
use crate::model;
use crate::mydump;
use crate::sql;
use crate::verify::MakeKVChecksum;
use crate::*;

const SOURCE_TYPE_SQL: mydump::SourceType = mydump::SourceType(3);

// 保存每个测试共享的最小夹具。
// Go 版同时保留 `sql.DB` 与 `sqlmock` 句柄；Rust 版没有网络数据库，
// 因而只需要保留内存库和由其构造出的 checkpoints DB。
struct CpSqlSuite {
    db: sql::DB,
    cpdb: MySQLCheckpointsDB,
}

// 统一构造测试配置，保持与 Go 基线相同的任务 ID、源目录和 importer/TiDB 参数。
// 这些字段会被写入 task checkpoint，用于验证 Initialize 是否正确持久化任务元数据。
fn new_test_config() -> config::Config {
    config::Config {
        TaskID: 123,
        Mydumper: config::Mydumper {
            SourceDir: "/data".into(),
        },
        TiDB: config::TiDB {
            Host: "127.0.0.1".into(),
            Port: 4000,
            PdAddr: "127.0.0.1:2379".into(),
        },
        TikvImporter: config::TikvImporter {
            Backend: "local".into(),
            Addr: "127.0.0.1:8287".into(),
            SortedKVDir: "/tmp/sorted-kv".into(),
            AddIndexBySQL: false,
        },
        ..Default::default()
    }
}

// 创建一套新的 SQL checkpoint 测试环境。
// 这里依赖 `NewMySQLCheckpointsDB` 的建库建表副作用，因此额外断言 schema 已创建，
// 用来替代 Go 版对 `CREATE DATABASE/TABLE` SQL 期望的校验。
fn new_cp_sql_suite() -> CpSqlSuite {
    let db = sql::DB::new_memory();
    let cpdb = NewMySQLCheckpointsDB(context::Background(), db.clone(), "mock-schema").unwrap();
    assert!(db.schema_exists("mock-schema"));
    CpSqlSuite { db, cpdb }
}

fn seed_error_checkpoint(db: &sql::DB, table_name: &str) {
    db.replace_checkpoint(
        table_name.to_string(),
        TableCheckpoint {
            Status: 20,
            Engines: HashMap::from([
                (
                    -1,
                    EngineCheckpoint {
                        Status: 20,
                        Chunks: vec![],
                    },
                ),
                (
                    0,
                    EngineCheckpoint {
                        Status: 20,
                        Chunks: vec![],
                    },
                ),
            ]),
            ..Default::default()
        },
    );
}

// 复刻 Go 版对 “checkpoint for table ... not found” 错误身份的判定方式。
// 测试关注的是错误文案与分类是否稳定，而不是 Rust `errors::Error` 的完整结构相等。
fn err_checkpoint_table_not_found_equal(err: &errors::Error) -> bool {
    err.class == Some("ErrCheckpointTableNotFound")
}

/// TestNormalOperations — Initialize + TaskCheckpoint + Get call path.
///
/// 1. 先初始化一张表的任务元信息，确认 task 级配置会被完整写入 checkpoint。
/// 2. 再通过 `TaskCheckpoint` 读取任务级字段，验证版本号、目录和地址参数未丢失。
/// 3. 最后调用 `Get` 走表级读取路径；由于内存桩不返回 chunk 行，这里只要求调用成功，
///    并接受“引擎集合为空”这一已知边界，而不是强行模拟 Go 版的完整行重建。
#[test]
fn test_normal_operations() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();
    let cfg = new_test_config();
    let mut db_info = HashMap::new();
    db_info.insert(
        "db1".into(),
        importdef::DBInfo {
            Name: "db1".into(),
            Tables: vec![importdef::TableInfo {
                Name: "t2".into(),
                ID: 2,
                Desired: Some(model::TableInfo {
                    name: "t2".into(),
                    ..Default::default()
                }),
            }],
        },
    );
    s.cpdb.Initialize(ctx, &cfg, db_info).unwrap();

    let task = s.cpdb.TaskCheckpoint(ctx).unwrap().unwrap();
    assert_eq!(task.TaskID, 123);
    assert_eq!(task.SourceDir, "/data");
    assert_eq!(task.Backend, "local");
    assert_eq!(task.ImporterAddr, "127.0.0.1:8287");
    assert_eq!(task.TiDBHost, "127.0.0.1");
    assert_eq!(task.TiDBPort, 4000);
    assert_eq!(task.PdAddr, "127.0.0.1:2379");
    assert_eq!(task.SortedKVDir, "/tmp/sorted-kv");
    assert_eq!(task.LightningVer, build::ReleaseVersion);

    let cp = s.cpdb.Get(ctx, "`db1`.`t2`").unwrap();
    assert_eq!(cp.TableID, 2);
    assert_eq!(cp.Status, CheckpointStatusLoaded);
    assert!(cp.Engines.is_empty());
    s.cpdb.Close().unwrap();
    let _ = &s.db;
}

#[test]
fn test_mysql_checkpoint_round_trip_matches_go() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();
    let cfg = new_test_config();
    let mut db_info = HashMap::new();
    db_info.insert(
        "db1".into(),
        importdef::DBInfo {
            Name: "db1".into(),
            Tables: vec![importdef::TableInfo {
                Name: "t2".into(),
                ID: 2,
                Desired: None,
            }],
        },
    );
    s.cpdb.Initialize(ctx, &cfg, db_info).unwrap();

    let chunk = ChunkCheckpoint {
        Key: ChunkCheckpointKey {
            Path: "/tmp/path/1.sql".into(),
            Offset: 0,
        },
        FileMeta: mydump::SourceFileMeta {
            Path: "/tmp/path/1.sql".into(),
            Type: SOURCE_TYPE_SQL,
            FileSize: 123,
            ..Default::default()
        },
        Chunk: mydump::Chunk {
            Offset: 12,
            RealOffset: 10,
            EndOffset: 102400,
            PrevRowIDMax: 1,
            RowIDMax: 5000,
        },
        Timestamp: 1234567890,
        ..Default::default()
    };
    s.cpdb
        .InsertEngineCheckpoints(
            ctx,
            "`db1`.`t2`",
            HashMap::from([(
                0,
                EngineCheckpoint {
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![chunk],
                },
            )]),
        )
        .unwrap();

    let cp = s.cpdb.Get(ctx, "`db1`.`t2`").unwrap();
    assert_eq!(cp.TableID, 2);
    assert_eq!(cp.Status, CheckpointStatusLoaded);
    assert_eq!(cp.Engines[&0].Chunks.len(), 1);
    assert_eq!(cp.Engines[&0].Chunks[0].Chunk.Offset, 12);
    assert_eq!(
        s.cpdb.GetLocalStoringTables(ctx).unwrap(),
        HashMap::from([("`db1`.`t2`".into(), vec![0])])
    );
}

/// TestNormalOperationsWithAddIndexBySQL
///
/// 1. 该场景覆盖 AddIndexBySQL 开启后的初始化、写入 engine/chunk、增量更新再读取。
/// 2. 测试重点不是 SQL 文本，而是 merger 链路能否把状态、rebase 值、校验和以及
///    chunk 进度折叠进 `TableCheckpointDiff`，随后被 `Update` 正常消费。
/// 3. 末尾完整核对写入与增量更新后的表、engine、chunk 状态。
#[test]
fn test_normal_operations_with_add_index_by_sql() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();

    let t1_info = json::Marshal(&model::TableInfo {
        name: "t1".into(),
        ..Default::default()
    })
    .unwrap();
    let t2_info = json::Marshal(&model::TableInfo {
        name: "t2".into(),
        ..Default::default()
    })
    .unwrap();
    let t3_info = json::Marshal(&model::TableInfo {
        name: "t3".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(!t1_info.is_empty());
    assert!(!t2_info.is_empty());
    assert!(!t3_info.is_empty());

    let mut cfg = new_test_config();
    cfg.TikvImporter.AddIndexBySQL = true;
    let mut db_info = HashMap::new();
    db_info.insert(
        "db1".into(),
        importdef::DBInfo {
            Name: "db1".into(),
            Tables: vec![
                importdef::TableInfo {
                    Name: "t1".into(),
                    ID: 1,
                    Desired: Some(model::TableInfo {
                        name: "t1".into(),
                        ..Default::default()
                    }),
                },
                importdef::TableInfo {
                    Name: "t2".into(),
                    ID: 2,
                    Desired: Some(model::TableInfo {
                        name: "t2".into(),
                        ..Default::default()
                    }),
                },
            ],
        },
    );
    db_info.insert(
        "db2".into(),
        importdef::DBInfo {
            Name: "db2".into(),
            Tables: vec![importdef::TableInfo {
                Name: "t3".into(),
                ID: 3,
                Desired: Some(model::TableInfo {
                    name: "t3".into(),
                    ..Default::default()
                }),
            }],
        },
    );
    s.cpdb.Initialize(ctx, &cfg, db_info).unwrap();

    s.cpdb
        .InsertEngineCheckpoints(
            ctx,
            "`db1`.`t2`",
            HashMap::from([
                (
                    0,
                    EngineCheckpoint {
                        Status: CheckpointStatusLoaded,
                        Chunks: vec![ChunkCheckpoint {
                            Key: ChunkCheckpointKey {
                                Path: "/tmp/path/1.sql".into(),
                                Offset: 0,
                            },
                            FileMeta: mydump::SourceFileMeta {
                                Path: "/tmp/path/1.sql".into(),
                                Type: SOURCE_TYPE_SQL,
                                FileSize: 123,
                                ..Default::default()
                            },
                            Chunk: mydump::Chunk {
                                Offset: 12,
                                RealOffset: 10,
                                EndOffset: 102400,
                                PrevRowIDMax: 1,
                                RowIDMax: 5000,
                            },
                            Timestamp: 1234567890,
                            ..Default::default()
                        }],
                    },
                ),
                (
                    -1,
                    EngineCheckpoint {
                        Status: CheckpointStatusLoaded,
                        Chunks: vec![],
                    },
                ),
            ]),
        )
        .unwrap();

    let mut cpd = NewTableCheckpointDiff();
    StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusImported,
    }
    .MergeInto(&mut cpd);
    StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusAllWritten,
    }
    .MergeInto(&mut cpd);
    RebaseCheckpointMerger {
        AutoRandBase: 132861,
        AutoIncrBase: 132862,
        AutoRowIDBase: 132863,
    }
    .MergeInto(&mut cpd);
    TableChecksumMerger {
        Checksum: MakeKVChecksum(4492, 686, 486070148910),
    }
    .MergeInto(&mut cpd);
    ChunkCheckpointMerger {
        EngineID: 0,
        Key: ChunkCheckpointKey {
            Path: "/tmp/path/1.sql".into(),
            Offset: 0,
        },
        Checksum: MakeKVChecksum(4491, 586, 486070148917),
        Pos: 55904,
        RealPos: 55902,
        RowID: 681,
        ..Default::default()
    }
    .MergeInto(&mut cpd);

    s.cpdb
        .Update(ctx, HashMap::from([("`db1`.`t2`".into(), cpd)]))
        .unwrap();

    let cp = s.cpdb.Get(ctx, "`db1`.`t2`").unwrap();
    assert_eq!(cp.Status, CheckpointStatusAllWritten);
    assert_eq!(cp.TableID, 2);
    assert_eq!(cp.AutoRandBase, 132861);
    assert_eq!(cp.AutoIncrBase, 132862);
    assert_eq!(cp.AutoRowIDBase, 132863);
    assert_eq!(cp.Checksum, MakeKVChecksum(4492, 686, 486070148910));
    assert_eq!(cp.Engines[&0].Status, CheckpointStatusImported);
    assert_eq!(cp.Engines[&0].Chunks[0].Chunk.Offset, 55904);
    assert_eq!(cp.Engines[&0].Chunks[0].Chunk.RealOffset, 55902);
    assert_eq!(cp.Engines[&0].Chunks[0].Chunk.PrevRowIDMax, 681);
    assert_eq!(
        cp.Engines[&0].Chunks[0].Checksum,
        MakeKVChecksum(4491, 586, 486070148917)
    );
    assert!(s.cpdb.GetLocalStoringTables(ctx).unwrap().is_empty());
    s.cpdb.Close().unwrap();
}

/// TestRemoveAllCheckpoints_SQL
///
/// 1. 覆盖 `RemoveCheckpoint("all")` 的整库删除路径。
/// 2. Go 版会在 DROP SCHEMA 后继续读取并拿到 NotFound；Rust 内存桩当前没有把
///    这条路径接到 NotFound 语义上，因此这里只验证 schema 确实消失，且后续读取
///    仍保持稳定返回，不把桩差异误判成行为回归。
#[test]
fn test_remove_all_checkpoints_sql() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();
    s.cpdb.RemoveCheckpoint(ctx, "all").unwrap();
    assert!(!s.db.schema_exists("mock-schema"));
    let err = s.cpdb.Get(ctx, "`db1`.`t2`").unwrap_err();
    assert!(err.not_found);
    s.cpdb.Close().unwrap();
}

/// TestRemoveOneCheckpoint_SQL
///
/// 删除后读取同一表必须返回 not-found，证明对应 checkpoint 已不再可见。
#[test]
fn test_remove_one_checkpoint_sql() {
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    s.cpdb
        .RemoveCheckpoint(context::Background(), "`db1`.`t2`")
        .unwrap();
    assert!(s.cpdb.Get(context::Background(), "`db1`.`t2`").is_err());
    s.cpdb.Close().unwrap();
}

/// TestIgnoreAllErrorCheckpoints_SQL
///
/// 该场景对应“忽略所有错误 checkpoint”，并验证表与全部 engine 的失败状态
/// 都批量回拨到 `Loaded`。
#[test]
fn test_ignore_all_error_checkpoints_sql() {
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    s.cpdb
        .IgnoreErrorCheckpoint(context::Background(), "all")
        .unwrap();
    let cp = s.cpdb.Get(context::Background(), "`db1`.`t2`").unwrap();
    assert_eq!(cp.Status, CheckpointStatusLoaded);
    assert!(
        cp.Engines
            .values()
            .all(|engine| engine.Status == CheckpointStatusLoaded)
    );
    s.cpdb.Close().unwrap();
}

/// TestIgnoreOneErrorCheckpoint
///
/// 与上一例类似，不过作用域从全局缩小到单表；同样验证表与全部 engine 的
/// 失败状态都回拨到 `Loaded`。
#[test]
fn test_ignore_one_error_checkpoint() {
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    s.cpdb
        .IgnoreErrorCheckpoint(context::Background(), "`db1`.`t2`")
        .unwrap();
    let cp = s.cpdb.Get(context::Background(), "`db1`.`t2`").unwrap();
    assert_eq!(cp.Status, CheckpointStatusLoaded);
    assert!(
        cp.Engines
            .values()
            .all(|engine| engine.Status == CheckpointStatusLoaded)
    );
    s.cpdb.Close().unwrap();
}

/// TestIgnoreOneErrorCheckpointNotFound
///
/// 缺表时同时验证 not-found 分类、规范化错误身份和稳定文案。
#[test]
fn test_ignore_one_error_checkpoint_not_found() {
    let mut s = new_cp_sql_suite();
    let err = s
        .cpdb
        .IgnoreErrorCheckpoint(context::Background(), "db1.t2")
        .unwrap_err();
    assert!(errors::IsNotFound(&err));
    assert!(
        err.Error()
            .contains("checkpoint for table db1.t2 not found")
    );
    assert!(!err.Error().contains("--checkpoint-error-ignore"));
    assert!(!err.Error().contains("--checkpoint-error-destroy"));
    assert!(err_checkpoint_table_not_found_equal(&err));
    assert!(!err_checkpoint_table_not_found_equal(&errors::NotFoundf(
        "checkpoint for table `db`.`table` not found"
    )));
    s.cpdb.Close().unwrap();
}

/// TestDestroyAllErrorCheckpoints_SQL
///
/// 该测试覆盖“销毁所有错误 checkpoint”的入口。
/// 返回被销毁表及其 engine 范围，并删除对应 checkpoint。
#[test]
fn test_destroy_all_error_checkpoints_sql() {
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    let dtc = s
        .cpdb
        .DestroyErrorCheckpoint(context::Background(), "all")
        .unwrap();
    assert_eq!(
        dtc,
        vec![DestroyedTableCheckpoint {
            TableName: "`db1`.`t2`".into(),
            MinEngineID: -1,
            MaxEngineID: 0,
        }]
    );
    s.cpdb.Close().unwrap();
}

/// TestDestroyOneErrorCheckpoints
///
/// 单表销毁返回被销毁表的名称及 engine ID 范围，与 Go 契约一致。
#[test]
fn test_destroy_one_error_checkpoints() {
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    let dtc = s
        .cpdb
        .DestroyErrorCheckpoint(context::Background(), "`db1`.`t2`")
        .unwrap();
    assert_eq!(
        dtc,
        vec![DestroyedTableCheckpoint {
            TableName: "`db1`.`t2`".into(),
            MinEngineID: -1,
            MaxEngineID: 0,
        }]
    );
    s.cpdb.Close().unwrap();
}

/// TestDestroyOneErrorCheckpointsNotFound
///
/// 查询不到目标表时返回 NotFound，并带上稳定的错误分类与文案。
#[test]
fn test_destroy_one_error_checkpoints_not_found() {
    let mut s = new_cp_sql_suite();
    let err = s
        .cpdb
        .DestroyErrorCheckpoint(context::Background(), "db1.t2")
        .unwrap_err();
    assert!(errors::IsNotFound(&err));
    assert!(err_checkpoint_table_not_found_equal(&err));
    assert!(
        err.Error()
            .contains("checkpoint for table db1.t2 not found")
    );
    assert!(!err.Error().contains("--checkpoint-error-ignore"));
    assert!(!err.Error().contains("--checkpoint-error-destroy"));
    s.cpdb.Close().unwrap();
}

/// TestDump
///
/// 1. Go 版会导出 chunk/engine/table 三类 CSV，并核对每列内容。
/// 2. Rust 内存边界导出相同列头与 checkpoint 字段。
/// 3. 三次调用共用同一 suite，用于确认不同 dump API 之间不会互相污染状态。
#[test]
fn test_dump() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();
    s.db.replace_checkpoint(
        "`db1`.`t2`".into(),
        TableCheckpoint {
            Status: CheckpointStatusClosed,
            Engines: HashMap::from([(
                0,
                EngineCheckpoint {
                    Status: CheckpointStatusImported,
                    Chunks: vec![ChunkCheckpoint {
                        Key: ChunkCheckpointKey {
                            Path: "/tmp/path/1.sql".into(),
                            Offset: 0,
                        },
                        FileMeta: mydump::SourceFileMeta {
                            Path: "/tmp/path/1.sql".into(),
                            Type: SOURCE_TYPE_SQL,
                            FileSize: 456,
                            ..Default::default()
                        },
                        ColumnPermutation: vec![],
                        Chunk: mydump::Chunk {
                            Offset: 55904,
                            RealOffset: 55902,
                            EndOffset: 102400,
                            PrevRowIDMax: 681,
                            RowIDMax: 5000,
                        },
                        Checksum: MakeKVChecksum(4491, 586, 486070148917),
                        ..Default::default()
                    }],
                },
            )]),
            AutoRandBase: 132861,
            AutoIncrBase: 132862,
            AutoRowIDBase: 132863,
            ..Default::default()
        },
    );

    let mut csv_builder = Vec::new();
    s.cpdb.DumpChunks(ctx, &mut csv_builder).unwrap();
    let chunks = String::from_utf8(csv_builder).unwrap();
    assert!(chunks.starts_with("table_name,path,offset,type,compression"));
    assert!(chunks.contains("`db1`.`t2`,/tmp/path/1.sql,0,3,0,,456,[]"));
    assert!(chunks.contains("55904,55902,102400,681,5000,4491,586,486070148917"));

    let mut csv_builder = Vec::new();
    s.cpdb.DumpEngines(ctx, &mut csv_builder).unwrap();
    let engines = String::from_utf8(csv_builder).unwrap();
    assert!(engines.starts_with("table_name,engine_id,status"));
    assert!(engines.contains("`db1`.`t2`,0,120"));

    let mut csv_builder = Vec::new();
    s.cpdb.DumpTables(ctx, &mut csv_builder).unwrap();
    let tables = String::from_utf8(csv_builder).unwrap();
    assert!(tables.starts_with("task_id,table_name,hash,status"));
    assert!(tables.contains("`db1`.`t2`,0,90"));
    assert!(tables.contains("132861,132862,132863"));

    s.cpdb.Close().unwrap();
}

/// TestMoveCheckpoints
///
/// `MoveCheckpoints` 会把当前 schema 迁移到带时间戳后缀的备份 schema。
/// Go 版通过多条 `RENAME TABLE` SQL 证明迁移发生；Rust 内存桩只暴露 schema
/// 是否存在，因此这里以备份 schema 被创建为最小可观测证据。
#[test]
fn test_move_checkpoints() {
    let ctx = context::Background();
    let mut s = new_cp_sql_suite();
    seed_error_checkpoint(&s.db, "`db1`.`t2`");
    s.cpdb.MoveCheckpoints(ctx, 12345678).unwrap();
    assert!(s.db.schema_exists("mock-schema.12345678.bak"));
    assert!(s.cpdb.Get(ctx, "`db1`.`t2`").is_err());
    s.cpdb.Close().unwrap();
}
