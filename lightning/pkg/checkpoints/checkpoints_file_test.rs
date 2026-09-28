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

//! Go-equivalent tests for `lightning/pkg/checkpoints/checkpoints_file_test.go`
//! (package `checkpoints_test`). Real FileCheckpointsDB + local temp files.
//!
//! 这组测试直接驱动真实的 `FileCheckpointsDB` 文件实现，
//! 用本地临时目录里的 protobuf 检查点文件验证持久化后的读取结果。
//! 整体结构按 Go 同名测试铺开，方便逐项比对 Rust 迁移后的语义是否一致。
//! 关注点集中在检查点元数据的落盘与回读：
//! 初始化是否完整、增量更新是否只改目标字段、
//! 删除与错误恢复是否只影响指定表、找不到表时的错误分类与文本是否稳定。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::common;
use crate::config;
use crate::errors;
use crate::importdef;
use crate::model;
use crate::mydump;
use crate::verify::MakeKVChecksum;
use crate::*;

// 这个常量复用 Go 测试里 SQL 文件类型的语义，
// 让 `SourceFileMeta` 在结构体全量比较时能保持一致的输入。
const SOURCE_TYPE_SQL: mydump::SourceType = mydump::SourceType(3);

// 构造最小可用的 Lightning 配置。
// 这里只保留文件检查点初始化和比较真正依赖的字段，
// 避免无关默认值把测试注意力从检查点行为上移开。
// 各取值与 Go 夹具保持同一组语义，便于跨语言对照失败原因。
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

// 生成唯一临时目录，确保每次测试都写入独立的 checkpoint 文件。
// 目录名同时包含进程号和纳秒时间戳，
// 足以隔离串行运行与潜在并发运行下的路径冲突。
fn temp_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "lightning-cp-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// 组装一份可复用的文件检查点数据库夹具。
// 该函数既负责初始化库表元信息，也预写入若干 engine/chunk 状态，
// 这样各测试用例可以直接聚焦读取、删除和错误处理的结果。
// 返回临时目录路径是为了把目录生命周期交给调用方持有，
// 防止文件在数据库关闭前被提早清理。
// `add_index_by_sql` 是唯一的分支开关，
// 用来覆盖 `Get` 是否把 `Desired` 表结构映射到 `TableInfo` 的行为差异。
fn new_file_checkpoints_db(add_index_by_sql: bool) -> (PathBuf, FileCheckpointsDB) {
    let dir = temp_dir();
    let ctx = context::Background();
    let path = dir.join("cp.pb");
    let path_str = path.to_string_lossy().to_string();
    let mut cpdb = NewFileCheckpointsDB(ctx, &path_str).unwrap();

    // 先建立基础配置，再切换 `AddIndexBySQL`，
    // 保证两种场景除了这一项外完全共享同一套初始数据。
    let mut cfg = new_test_config();
    cfg.TikvImporter.AddIndexBySQL = add_index_by_sql;

    // 初始化两个数据库、三张表：
    // `db1.t2` 是主要断言对象，
    // `db2.t3` 用于验证 whole-table engine 与表结构透传，
    // `db1.t1` 仅证明初始化可以包含未进一步写入 checkpoint 的表。
    let mut db_info = HashMap::new();
    db_info.insert(
        "db1".into(),
        importdef::DBInfo {
            Name: "db1".into(),
            Tables: vec![
                importdef::TableInfo {
                    Name: "t1".into(),
                    ..Default::default()
                },
                importdef::TableInfo {
                    Name: "t2".into(),
                    ..Default::default()
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
                Desired: Some(model::TableInfo {
                    name: "t3".into(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        },
    );
    cpdb.Initialize(ctx, &cfg, db_info).unwrap();

    // `db1.t2` 同时写入普通 engine 和 whole-table engine，
    // 用来覆盖分片状态与整表状态共存时的序列化行为。
    cpdb.InsertEngineCheckpoints(
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
                            FileSize: 12345,
                            ..Default::default()
                        },
                        Chunk: mydump::Chunk {
                            Offset: 12,
                            RealOffset: 10,
                            EndOffset: 102400,
                            PrevRowIDMax: 1,
                            RowIDMax: 5000,
                        },
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

    // `db2.t3` 只保留 whole-table engine，
    // 这样后续 `AddIndexBySQL` 分支的差异就只体现在 `TableInfo` 上。
    cpdb.InsertEngineCheckpoints(
        ctx,
        "`db2`.`t3`",
        HashMap::from([(
            -1,
            EngineCheckpoint {
                Status: CheckpointStatusLoaded,
                Chunks: vec![],
            },
        )]),
    )
    .unwrap();

    // 再对 `db1.t2` 应用一轮增量更新，
    // 模拟导入过程推进后 checkpoint 文件里的真实落盘形态。
    // 这里同时覆盖状态推进、自增基线回写、整表 checksum 汇总，
    // 以及 chunk 级别的位置、行号和 checksum 更新。
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

    cpdb.Update(ctx, HashMap::from([("`db1`.`t2`".into(), cpd)]))
        .unwrap();

    // 夹具返回时，文件里已经具备“一个被更新过的表”和“一个仅初始化的表”。
    // 这个布局刚好对应后续所有读取、删除和错误处理用例的最小覆盖面。
    (dir, cpdb)
}

// 把两张表的 whole-table engine 都标记为无效状态。
// 后续“忽略错误”和“销毁错误”测试基于这一步制造异常检查点，
// 从而验证批量操作与单表操作的作用范围。
fn set_invalid_status(cpdb: &mut FileCheckpointsDB) {
    let mut cpd = NewTableCheckpointDiff();
    let mut scm = StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusAllWritten,
    };
    scm.SetInvalid();
    scm.MergeInto(&mut cpd);
    cpdb.Update(
        context::Background(),
        HashMap::from([
            ("`db1`.`t2`".into(), cpd.clone()),
            ("`db2`.`t3`".into(), cpd),
        ]),
    )
    .unwrap();
}

// `db1.t2` 的期望值对应“初始化后再应用增量更新”的最终结果。
// 显式写出这些字段，是为了同时验证状态机推进、
// chunk 偏移与行号回写，以及 checksum 持久化都没有偏差。
fn expect_t2() -> TableCheckpoint {
    TableCheckpoint {
        // 表级状态来自 whole-table engine 的推进结果，
        // 表示整个表的数据写入阶段已经结束。
        Status: CheckpointStatusAllWritten,
        AutoRandBase: 132861,
        AutoIncrBase: 132862,
        AutoRowIDBase: 132863,
        Checksum: MakeKVChecksum(4492, 686, 486070148910),
        Engines: HashMap::from([
            (
                -1,
                EngineCheckpoint {
                    // whole-table engine 不携带 chunk，
                    // 仅负责记录整表级别的汇总状态。
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![],
                },
            ),
            (
                0,
                EngineCheckpoint {
                    // 普通 engine 保留实际 chunk 明细，
                    // 供断言验证回写后的偏移和 checksum。
                    Status: CheckpointStatusImported,
                    Chunks: vec![ChunkCheckpoint {
                        Key: ChunkCheckpointKey {
                            Path: "/tmp/path/1.sql".into(),
                            Offset: 0,
                        },
                        FileMeta: mydump::SourceFileMeta {
                            Path: "/tmp/path/1.sql".into(),
                            Type: SOURCE_TYPE_SQL,
                            FileSize: 12345,
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
                        Timestamp: 0,
                    }],
                },
            ),
        ]),
        ..Default::default()
    }
}

// `db2.t3` 的基础读取结果只包含 whole-table engine。
// 这个样本用于证明没有 chunk 数据的表也能被完整读取。
fn expect_t3() -> TableCheckpoint {
    TableCheckpoint {
        // `t3` 不需要 chunk 级验证，
        // 因此 whole-table engine 就足以表达它的基线状态。
        Status: CheckpointStatusLoaded,
        Engines: HashMap::from([(
            -1,
            EngineCheckpoint {
                Status: CheckpointStatusLoaded,
                Chunks: vec![],
            },
        )]),
        ..Default::default()
    }
}

// 开启 `AddIndexBySQL` 后，`Get` 需要额外暴露初始化时保存的表结构信息。
// 除了 `TableInfo` 外，其余状态字段和 engine 布局都应保持不变。
fn expect_t3_add_index() -> TableCheckpoint {
    TableCheckpoint {
        // 状态与 `expect_t3` 相同，
        // 唯一差别是读取结果附带初始化阶段保存的 `Desired` 表结构。
        Status: CheckpointStatusLoaded,
        Engines: HashMap::from([(
            -1,
            EngineCheckpoint {
                Status: CheckpointStatusLoaded,
                Chunks: vec![],
            },
        )]),
        TableInfo: Some(model::TableInfo {
            name: "t3".into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// Rust 侧错误类型尚未完整复刻 Go 的错误链行为，
// 因此这里通过标志位和消息文本双重判断“未找到”语义。
fn is_not_found(err: &errors::Error) -> bool {
    err.not_found || err.msg.contains("not found")
}

// 这个辅助断言模拟 Go `Normalize.Equal` 的核心语义：
// 即使错误被包装，也要能识别它是否属于“表检查点不存在”这一类别。
// 当前 Rust 桩实现仍以消息和标志位为主，因此测试显式锁定可观察条件。
fn err_checkpoint_table_not_found_equal(err: &errors::Error) -> bool {
    err.class == Some("ErrCheckpointTableNotFound")
}

// 手工构造带前缀的新错误，模拟 Go `errors.Annotate` 的包装效果。
// 这样可以验证错误类别判断不会因为外层追加上下文而失真。
fn annotate(err: errors::Error, msg: &str) -> errors::Error {
    errors::Error {
        msg: format!("{msg}: {}", err.msg),
        not_found: err.not_found,
        no_rows: err.no_rows,
        class: err.class,
    }
}

// 验证 `Get` 的两类核心行为：
// 正常读取时返回的结构体必须与预先写入的 checkpoint 完全匹配，
// 读取不存在的表时则必须暴露稳定的 not-found 语义。
// 这里分别覆盖 `AddIndexBySQL` 关闭和开启的分支，
// 因为它们只应在 `t3.TableInfo` 上表现出差异。
/// TestGet
#[test]
fn test_get() {
    let ctx = context::Background();
    let expect_t2 = expect_t2();
    let expect_t3 = expect_t3();
    let expect_t3_add = expect_t3_add_index();

    // 基线场景：不开启 `AddIndexBySQL`，
    // 因此 `t3` 只应返回状态和 engine 信息。
    {
        let (_dir, mut cpdb) = new_file_checkpoints_db(false);
        let cp = cpdb.Get(ctx, "`db1`.`t2`").unwrap();
        assert_eq!(cp, expect_t2);
        let cp = cpdb.Get(ctx, "`db2`.`t3`").unwrap();
        assert_eq!(cp, expect_t3);
        let err = cpdb.Get(ctx, "`db3`.`not-exists`").unwrap_err();
        assert!(is_not_found(&err));
        cpdb.Close().unwrap();
    }

    // 开启 `AddIndexBySQL` 后，
    // `t2` 的结果保持不变，`t3` 额外携带 `TableInfo`。
    {
        let (_dir, mut cpdb) = new_file_checkpoints_db(true);
        let cp = cpdb.Get(ctx, "`db1`.`t2`").unwrap();
        assert_eq!(cp, expect_t2);
        let cp = cpdb.Get(ctx, "`db2`.`t3`").unwrap();
        assert_eq!(cp, expect_t3_add);
        let err = cpdb.Get(ctx, "`db3`.`not-exists`").unwrap_err();
        assert!(is_not_found(&err));
        cpdb.Close().unwrap();
    }
}

// 验证传入 `"all"` 时会移除当前文件中的全部表检查点。
// 删除后对任意已知表执行 `Get` 都必须表现为“未找到”。
/// TestRemoveAllCheckpoints
#[test]
fn test_remove_all_checkpoints() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    cpdb.RemoveCheckpoint(ctx, "all").unwrap();
    assert!(is_not_found(&cpdb.Get(ctx, "`db1`.`t2`").unwrap_err()));
    assert!(is_not_found(&cpdb.Get(ctx, "`db2`.`t3`").unwrap_err()));
    cpdb.Close().unwrap();
}

// 验证按表名删除时只影响目标表，
// 其他表的 checkpoint 数据必须原样保留。
/// TestRemoveOneCheckpoint
#[test]
fn test_remove_one_checkpoint() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    // 先删除 `t2`，再确认 `t3` 仍能正常读取，
    // 从结果上证明删除逻辑按表键精确生效。
    cpdb.RemoveCheckpoint(ctx, "`db1`.`t2`").unwrap();
    assert!(is_not_found(&cpdb.Get(ctx, "`db1`.`t2`").unwrap_err()));
    let cp = cpdb.Get(ctx, "`db2`.`t3`").unwrap();
    assert_eq!(cp.Status, CheckpointStatusLoaded);
    cpdb.Close().unwrap();
}

// 批量忽略错误会把所有异常检查点恢复到可继续导入的状态。
// 这里要求两张被标记为无效的表最终都回到 `Loaded`。
/// TestIgnoreAllErrorCheckpoints
#[test]
fn test_ignore_all_error_checkpoints() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    cpdb.IgnoreErrorCheckpoint(ctx, "all").unwrap();
    assert_eq!(
        cpdb.Get(ctx, "`db1`.`t2`").unwrap().Status,
        CheckpointStatusLoaded
    );
    assert_eq!(
        cpdb.Get(ctx, "`db2`.`t3`").unwrap().Status,
        CheckpointStatusLoaded
    );
    cpdb.Close().unwrap();
}

// 单表忽略错误只应修复目标表，
// 非目标表仍保留其异常状态演化出的数值，证明接口具备隔离性。
/// TestIgnoreOneErrorCheckpoints
#[test]
fn test_ignore_one_error_checkpoints() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    // 只恢复 `t2`，
    // `t3` 继续保留异常状态对应的数值以作对照。
    cpdb.IgnoreErrorCheckpoint(ctx, "`db1`.`t2`").unwrap();
    assert_eq!(
        cpdb.Get(ctx, "`db1`.`t2`").unwrap().Status,
        CheckpointStatusLoaded
    );
    assert_eq!(
        cpdb.Get(ctx, "`db2`.`t3`").unwrap().Status,
        CheckpointStatusAllWritten / 10
    );
    cpdb.Close().unwrap();
}

// 故意传入未加反引号的表名，使其无法命中文件中的真实键格式。
// 除了检查 not-found 语义，这里还锁定错误文本，
// 确保底层错误不会夹带 CLI 层才该追加的参数提示。
/// TestIgnoreOneErrorCheckpointsNotFound
#[test]
fn test_ignore_one_error_checkpoints_not_found() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    let err = cpdb.IgnoreErrorCheckpoint(ctx, "db1.t2").unwrap_err();
    assert!(errors::IsNotFound(&err));
    assert!(is_not_found(&err));
    assert!(err_checkpoint_table_not_found_equal(&err));
    assert!(err_checkpoint_table_not_found_equal(&annotate(
        err.clone(),
        "wrapped"
    )));
    assert!(
        err.Error()
            .contains("checkpoint for table db1.t2 not found")
    );
    assert!(!err.Error().contains("--checkpoint-error-ignore"));
    assert!(!err.Error().contains("--checkpoint-error-destroy"));
    assert!(!err_checkpoint_table_not_found_equal(&errors::NotFoundf(
        "checkpoint for table `db`.`table` not found"
    )));
    let _ = common::ErrCheckpointTableNotFound;
    cpdb.Close().unwrap();
}

// 批量销毁错误检查点会直接删除异常表，并返回被删除表的摘要。
// 先排序再断言，是为了消除 map/遍历顺序带来的不稳定性。
/// TestDestroyAllErrorCheckpoints
#[test]
fn test_destroy_all_error_checkpoints() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    let mut dtc = cpdb.DestroyErrorCheckpoint(ctx, "all").unwrap();
    dtc.sort_by(|a, b| a.TableName.cmp(&b.TableName));
    assert_eq!(
        dtc,
        vec![
            DestroyedTableCheckpoint {
                TableName: "`db1`.`t2`".into(),
                MinEngineID: -1,
                MaxEngineID: 0,
            },
            DestroyedTableCheckpoint {
                TableName: "`db2`.`t3`".into(),
                MinEngineID: -1,
                MaxEngineID: -1,
            },
        ]
    );
    assert!(is_not_found(&cpdb.Get(ctx, "`db1`.`t2`").unwrap_err()));
    assert!(is_not_found(&cpdb.Get(ctx, "`db2`.`t3`").unwrap_err()));
    cpdb.Close().unwrap();
}

// 单表销毁只应返回一个删除摘要，
// 同时未指定的表仍保留原来的异常状态。
/// TestDestroyOneErrorCheckpoint
#[test]
fn test_destroy_one_error_checkpoint() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    // 销毁动作应返回被删表覆盖的 engine 范围，
    // 供调用方继续决定需要清理哪些外部产物。
    let dtc = cpdb.DestroyErrorCheckpoint(ctx, "`db1`.`t2`").unwrap();
    assert_eq!(
        dtc,
        vec![DestroyedTableCheckpoint {
            TableName: "`db1`.`t2`".into(),
            MinEngineID: -1,
            MaxEngineID: 0,
        }]
    );
    assert!(is_not_found(&cpdb.Get(ctx, "`db1`.`t2`").unwrap_err()));
    assert_eq!(
        cpdb.Get(ctx, "`db2`.`t3`").unwrap().Status,
        CheckpointStatusAllWritten / 10
    );
    cpdb.Close().unwrap();
}

// 销毁接口在找不到目标表时也必须维持稳定的 not-found 语义和消息模板。
// 这里关注错误分类与消息约束，不关心成功删除时才会存在的摘要结果。
/// TestDestroyOneErrorCheckpointNotFound
#[test]
fn test_destroy_one_error_checkpoint_not_found() {
    let ctx = context::Background();
    let (_dir, mut cpdb) = new_file_checkpoints_db(false);
    set_invalid_status(&mut cpdb);
    let err = cpdb.DestroyErrorCheckpoint(ctx, "db1.t2").unwrap_err();
    assert!(errors::IsNotFound(&err));
    assert!(is_not_found(&err));
    assert!(err_checkpoint_table_not_found_equal(&err));
    assert!(
        err.Error()
            .contains("checkpoint for table db1.t2 not found")
    );
    assert!(!err.Error().contains("--checkpoint-error-ignore"));
    assert!(!err.Error().contains("--checkpoint-error-destroy"));
    cpdb.Close().unwrap();
}
