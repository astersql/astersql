// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/checkpoints` public contracts vs Go.
//!
//! 中文总览：这组测试不尝试重放完整 Lightning 导入流程，而是把 `checkpoints`
//! 对外暴露的公共契约拆成四类可观察行为，确认 Rust 迁移版继续与 Go 保持同一语义。
//! 第一类是正常路径，覆盖状态名、diff 合并、checkpoint 应用、文件持久化和路径拆分。
//! 第二类是边界路径，重点看“不能回退”的 rebase 语义、关闭 checkpoint 时的空实现契约，
//! 以及特殊常量和目录路径校验这类容易在移植时被忽略的边缘条件。
//! 第三类是错误路径，验证 Null/File/MySQL 三类后端在失败时返回的错误文本和分类仍然稳定。
//! 第四类是资源回收，确认文件检查点在 `Close` 与 `RemoveCheckpoint(all)` 之后的可观测副作用。
//! 这些断言共同构成 Go 包级行为的最小护栏：即便内部实现继续重构，只要这里保持稳定，
//! 上层 importer 和恢复逻辑就不会因为接口漂移而在运行期才暴露兼容性问题。
//! 阅读时可把每个 `contract_*` 当成一份“对外保证清单”而不是普通单元测试。
//! 其中很多数据值刻意使用非零、负数或特殊常量，目的是让合并和序列化路径更接近 Go 真实现。
//! 因为这里只补中文注释，下面所有测试步骤、断言顺序与使用的桩对象都保持原样。

use std::collections::HashMap;

use crate::mydump;
use crate::storeapi::StorageHandle;
use crate::verify::MakeKVChecksum;
use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 统一入口按四段顺序执行，方便在失败时直接定位是正常语义、边界、错误还是回收约束漂移。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn contract_normal() {
    // 这部分对应“常规使用 checkpoint 系统时，调用方最依赖的公共语义”。
    // 断言顺序大体沿着 Go 测试的阅读顺序展开：先看状态判断，再看 diff 合并，
    // 接着看应用到表级快照后的结果，最后才看文件后端与路径辅助函数。
    // 每一组断言都尽量挑选最能代表契约边界的字段，
    // 避免把测试写成仅仅重复实现细节的逐字段抄录。

    // IsCheckpointTable / MetricName
    // `IsCheckpointTable` 要能识别系统保留表，避免用户表被误当成内部元数据跳过。
    // `MetricName` 则把状态码映射成指标标签文本，导入观测面板依赖这个字符串稳定。
    assert!(IsCheckpointTable(CheckpointTableNameTask));
    assert!(IsCheckpointTable(CheckpointTableNameTable));
    assert!(!IsCheckpointTable("user_table"));
    assert_eq!(MetricName(CheckpointStatusLoaded), "pending");
    assert_eq!(MetricName(CheckpointStatusImported), "imported");
    assert_eq!(MetricName(CheckpointStatusChecksummed), "checksum");
    assert_eq!(MetricName(99), "invalid");

    // Status merge (TestMergeStatusCheckpoint)
    // engine 级状态更新不应直接篡改 whole-table 状态；
    // 只有使用 `WholeTableEngineID` 时，diff 才代表整表生命周期推进。
    // 这能保证 importer 在推进单个 engine 时，不会误把整表提前标记为完成。
    let mut cpd = NewTableCheckpointDiff();
    StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusImported,
    }
    .MergeInto(&mut cpd);
    assert!(!cpd.hasStatus);
    assert_eq!(
        cpd.engines.get(&0).map(|e| e.status),
        Some(CheckpointStatusImported)
    );

    StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusClosed,
    }
    .MergeInto(&mut cpd);
    assert!(cpd.hasStatus);
    assert_eq!(cpd.status, CheckpointStatusClosed);

    // Invalid status (TestMergeInvalidStatusCheckpoint)
    // Go 版允许通过 `SetInvalid` 把状态打上“无效但仍需保留信息”的标记。
    // 这里验证 Rust 仍沿用同样的降级编码方式，而不是把无效状态直接丢弃。
    // 对恢复诊断而言，这类“保留但降级”的状态比直接消失更有排障价值。
    let mut cpd = NewTableCheckpointDiff();
    StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusLoaded,
    }
    .MergeInto(&mut cpd);
    let mut m = StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusAllWritten,
    };
    m.SetInvalid();
    m.MergeInto(&mut cpd);
    assert!(cpd.hasStatus);
    assert_eq!(cpd.status, CheckpointStatusAllWritten / 10);
    assert_eq!(
        cpd.engines.get(&-1).map(|e| e.status),
        Some(CheckpointStatusAllWritten / 10)
    );

    // Chunk merge overwrite
    // chunk diff 的关键语义是“同一 key 的后写覆盖前写”，
    // 因为恢复流程总是希望拿到最新偏移、最新 checksum 和最新 row id。
    // 如果这里变成累加或首次写入生效，断点续跑就会回退到旧位置。
    // 这种覆盖行为也与 Go 版 map 合并习惯保持一致，便于跨语言对照问题。
    let mut cpd = NewTableCheckpointDiff();
    let key = ChunkCheckpointKey {
        Path: "/tmp/path/1.sql".into(),
        Offset: 0,
    };
    ChunkCheckpointMerger {
        EngineID: 2,
        Key: key.clone(),
        Checksum: MakeKVChecksum(700, 15, 1234567890),
        Pos: 1055,
        RealPos: 1053,
        RowID: 31,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    ChunkCheckpointMerger {
        EngineID: 2,
        Key: key.clone(),
        Checksum: MakeKVChecksum(800, 20, 1357924680),
        Pos: 1080,
        RealPos: 1070,
        RowID: 42,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    let d = &cpd.engines.get(&2).unwrap().chunks[&key];
    assert_eq!(d.pos, 1080);
    assert_eq!(d.realPos, 1070);
    assert_eq!(d.rowID, 42);
    assert_eq!(d.checksum, MakeKVChecksum(800, 20, 1357924680));

    // Rebase does not go backwards
    // 自增基线只能向前推进，绝不能因为旧 diff 回放把 base 拉回去。
    // 这也是恢复场景里防止重复分配 ID 的核心保障。
    // 三个 base 分别对应 auto-random、auto-increment 与隐式 row id，规则必须一致。
    let mut cpd = NewTableCheckpointDiff();
    RebaseCheckpointMerger {
        AutoRandBase: 132861,
        AutoIncrBase: 132862,
        AutoRowIDBase: 132863,
    }
    .MergeInto(&mut cpd);
    RebaseCheckpointMerger {
        AutoRandBase: 131,
        AutoIncrBase: 132,
        AutoRowIDBase: 133,
    }
    .MergeInto(&mut cpd);
    assert_eq!(cpd.autoRandBase, 132861);
    assert_eq!(cpd.autoIncrBase, 132862);
    assert_eq!(cpd.autoRowIDBase, 132863);

    // ApplyDiff (TestApplyDiff core)
    // 这段是整个测试里最接近真实表级恢复语义的部分：
    // 它先构造一个已有 engine/chunk 的快照，再应用混合 diff，
    // 观察整表状态、基线和指定 chunk 是否只按目标字段被更新。
    // 这里尤其要防止“更新一个 chunk 时把同 engine 其他 chunk 一起覆盖”的回归。
    // 如果该行为漂移，恢复时可能出现某个文件片段进度被另一个片段覆盖的问题。
    let mut cp = TableCheckpoint {
        Status: CheckpointStatusLoaded,
        AutoRandBase: 131,
        AutoIncrBase: 132,
        AutoRowIDBase: 133,
        Engines: HashMap::from([
            (
                -1,
                EngineCheckpoint {
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![],
                },
            ),
            (
                0,
                EngineCheckpoint {
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![
                        ChunkCheckpoint {
                            Key: ChunkCheckpointKey {
                                Path: "/tmp/01.sql".into(),
                                Offset: 0,
                            },
                            Chunk: mydump::Chunk {
                                Offset: 0,
                                RealOffset: 0,
                                EndOffset: 20000,
                                PrevRowIDMax: 0,
                                RowIDMax: 1000,
                            },
                            ..Default::default()
                        },
                        ChunkCheckpoint {
                            Key: ChunkCheckpointKey {
                                Path: "/tmp/04.sql".into(),
                                Offset: 0,
                            },
                            Chunk: mydump::Chunk {
                                Offset: 0,
                                RealOffset: 0,
                                EndOffset: 15000,
                                PrevRowIDMax: 1000,
                                RowIDMax: 1300,
                            },
                            ..Default::default()
                        },
                    ],
                },
            ),
        ]),
        ..Default::default()
    };
    let mut cpd = NewTableCheckpointDiff();
    StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusImported,
    }
    .MergeInto(&mut cpd);
    StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusAllWritten,
    }
    .MergeInto(&mut cpd);
    RebaseCheckpointMerger {
        AutoRandBase: 1131,
        AutoIncrBase: 1132,
        AutoRowIDBase: 1133,
    }
    .MergeInto(&mut cpd);
    ChunkCheckpointMerger {
        EngineID: 0,
        Key: ChunkCheckpointKey {
            Path: "/tmp/01.sql".into(),
            Offset: 0,
        },
        Checksum: MakeKVChecksum(3333, 4444, 5555),
        Pos: 6666,
        RealPos: 6565,
        RowID: 777,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    cp.Apply(&cpd);
    assert_eq!(cp.Status, CheckpointStatusAllWritten);
    assert_eq!(cp.AutoRandBase, 1131);
    assert_eq!(
        cp.Engines.get(&-1).unwrap().Status,
        CheckpointStatusImported
    );
    let c0 = &cp.Engines.get(&0).unwrap().Chunks[0];
    assert_eq!(c0.Chunk.Offset, 6666);
    assert_eq!(c0.Chunk.RealOffset, 6565);
    assert_eq!(c0.Chunk.PrevRowIDMax, 777);
    assert_eq!(c0.Checksum, MakeKVChecksum(3333, 4444, 5555));
    // 同 engine 下未命中的第二个 chunk 必须保持原值，
    // 否则说明 diff 应用逻辑把局部更新误实现成了整批重写。
    assert_eq!(cp.Engines.get(&0).unwrap().Chunks[1].Chunk.Offset, 0);

    // Chunk sizes
    // 这三个尺寸函数常被上层用于展示进度和估算剩余工作量，
    // 因此既要符合直觉，也要和 Go 里的偏移解释保持一致。
    // 这里选用有明显差值的偏移，避免零值或相等值掩盖公式错误。
    let ccp = ChunkCheckpoint {
        Key: ChunkCheckpointKey {
            Path: "a".into(),
            Offset: 10,
        },
        FileMeta: mydump::SourceFileMeta {
            Compression: mydump::CompressionNone,
            FileSize: 1000,
            ..Default::default()
        },
        Chunk: mydump::Chunk {
            Offset: 40,
            EndOffset: 100,
            RealOffset: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(ccp.UnfinishedSize(), 60);
    assert_eq!(ccp.TotalSize(), 90);
    assert_eq!(ccp.FinishedSize(), 30);

    // File checkpoint marshal/unmarshal roundtrip
    // 文件后端 round-trip 验证的是“关掉后再打开，最小必要状态仍可恢复”。
    // 这里不追求 MySQL 那种细粒度事务语义，而是确认 protobuf 文件至少没有丢表或丢状态。
    // 之所以使用内存存储句柄，是为了把测试焦点固定在 checkpoint 编码/解码本身。
    let ctx = context::Background();
    let mem = StorageHandle::memory();
    let mut file_cp = NewFileCheckpointsDBWithExstorageFileName(
        ctx,
        "/tmp/filecheckpoint",
        mem.clone(),
        "filecheckpoint",
    )
    .unwrap();
    file_cp
        .checkpoints
        .Checkpoints
        .insert("a".into(), checkpointspb_table_loaded());
    file_cp.Close().unwrap();

    let file_cp2 = NewFileCheckpointsDBWithExstorageFileName(
        ctx,
        "/tmp/filecheckpoint",
        mem,
        "filecheckpoint",
    )
    .unwrap();
    assert!(file_cp2.checkpoints.Checkpoints.get("a").is_some());
    assert!(
        file_cp2
            .checkpoints
            .Checkpoints
            .get("a")
            .unwrap()
            .Engines
            .is_empty()
    );
    // Go 中 map 恢复后常要求非 nil；Rust 对应约束是空 `HashMap` 也必须可直接使用。
    // 这样后续增量更新无需再做额外初始化分支判断。
    let _ = &file_cp2.checkpoints.Checkpoints["a"].Engines;

    // separateCompletePath local cases (Go TestSeparateCompletePath subset)
    // 路径拆分看似只是辅助函数，但 checkpoint 文件名和目录名的恢复都依赖它。
    // 这里同时覆盖本地路径、相对路径、根目录路径和对象存储 URL，
    // 防止 Rust 在路径标准化时无意改变 Go 已接受的输入集合。
    // 这些 case 还顺手覆盖了是否保留尾部斜杠、对象存储 bucket 前缀等细节。
    let cases = [
        ("", "", ""),
        ("/a/", "", "/a/"),
        ("test.log", "test.log", "."),
        ("./test.log", "test.log", "."),
        ("./tmp/test.log", "test.log", "tmp"),
        ("tmp/test.log", "test.log", "tmp"),
        ("/test.log", "test.log", "/"),
        ("/tmp/test.log", "test.log", "/tmp"),
        ("s3://bucket2/test.log", "test.log", "s3://bucket2/"),
        (
            "s3://bucket2/test/test.log",
            "test.log",
            "s3://bucket2/test",
        ),
    ];
    for (complete, expect_file, expect_path) in cases {
        let (file_name, new_path) = separateCompletePath(complete).unwrap();
        assert_eq!(file_name, expect_file, "file for {complete}");
        assert_eq!(new_path, expect_path, "path for {complete}");
    }
}

fn checkpointspb_table_loaded()
-> astersql_lightning_pkg_checkpoints_checkpointspb::TableCheckpointModel {
    // 这个辅助构造器显式把 `Engines` 设为空 map，
    // 用来模拟 Go 侧“已加载但尚未展开 engine 明细”的最小表级 checkpoint。
    // 之所以单独抽函数，是为了让多个 round-trip 断言共享同一份基准输入，
    // 减少不同 case 手工构造时引入的无关差异。
    astersql_lightning_pkg_checkpoints_checkpointspb::TableCheckpointModel {
        Status: CheckpointStatusLoaded as u32,
        Engines: HashMap::new(),
        ..Default::default()
    }
}

fn contract_boundary() {
    // 边界场景主要保护那些“平时不显眼，但一旦回归就会影响恢复判断”的规则。
    // 它们往往不是主流程里最常见的路径，却最容易在重构时被顺手改坏。

    // Apply rebase does not go backwards (TestTableCheckpointApplyBases)
    // 即使 `Apply` 输入了更小的 base，也不能覆盖已有更大的自增基线。
    let mut tbl = TableCheckpoint {
        AutoRowIDBase: 11,
        AutoIncrBase: 12,
        AutoRandBase: 13,
        ..Default::default()
    };
    tbl.Apply(&TableCheckpointDiff {
        hasRebase: true,
        autoRowIDBase: 1,
        autoIncrBase: 2,
        autoRandBase: 3,
        ..Default::default()
    });
    assert_eq!(tbl.AutoRowIDBase, 11);
    assert_eq!(tbl.AutoIncrBase, 12);
    assert_eq!(tbl.AutoRandBase, 13);

    // OpenCheckpointsDB disabled -> null
    // 当用户关闭 checkpoint 时，返回的空实现要保持可调用，
    // 但读取到的状态只能是默认空值，不能偷偷创建真实后端。
    // 否则“关闭 checkpoint”会在行为上名不副实，还可能生成用户意料外的副作用。
    let cfg = config::Config {
        Checkpoint: config::Checkpoint {
            Enable: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut db = OpenCheckpointsDB(context::Background(), &cfg).unwrap();
    assert!(db.TaskCheckpoint(context::Background()).unwrap().is_none());
    let got = db.Get(context::Background(), "`db`.`t`").unwrap();
    assert_eq!(got.Status, CheckpointStatusLoaded);
    assert!(got.Engines.is_empty());

    // WholeTableEngineID
    // 这个特殊常量承担“整表状态”与“具体 engine 状态”分流的职责；
    // 值变化会让大量 merge/apply 逻辑的判定分支失效。
    assert_eq!(WholeTableEngineID, i32::MAX);

    // Directory DSN rejected
    // 文件驱动需要的是具体文件名而非目录。
    // 如果这里放宽为目录，后续 close/remove 时的副作用就会与 Go 不一致。
    let err = match newFileCheckpointsDB(
        context::Background(),
        "/tmp/dir/",
        StorageHandle::memory(),
        "",
    ) {
        Ok(_) => panic!("expected directory DSN error"),
        Err(e) => e,
    };
    assert!(err.Error().contains("must not be a directory"));
}

fn contract_error() {
    // 错误契约关注的是“调用失败时返回什么”，而不仅仅是“会不会失败”。
    // Go 上层常直接匹配错误文本中的关键短语，因此这里维持字符串片段稳定很重要。
    // 这类断言一旦失败，往往说明错误分类、包装层级或边界分支发生了非预期变化。

    let mut null = NewNullCheckpointsDB();
    // Null backend 代表 checkpoint 被禁用；所有写操作都应统一返回 disabled 语义。
    let err = null
        .RemoveCheckpoint(context::Background(), "all")
        .unwrap_err();
    assert!(err.Error().contains("checkpoints is disabled"));
    let err = null
        .IgnoreErrorCheckpoint(context::Background(), "t")
        .unwrap_err();
    assert!(err.Error().contains("checkpoints is disabled"));
    let err = null
        .DestroyErrorCheckpoint(context::Background(), "t")
        .unwrap_err();
    assert!(err.Error().contains("checkpoints is disabled"));
    let mut buf = Vec::new();
    let err = null
        .DumpTables(context::Background(), &mut buf)
        .unwrap_err();
    assert!(err.Error().contains("checkpoints is disabled"));

    // Unknown driver
    // 未知 driver 必须在打开阶段立刻失败，不能推迟到首次读写时才暴露。
    let cfg = config::Config {
        Checkpoint: config::Checkpoint {
            Enable: true,
            Driver: "weird".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let err = match OpenCheckpointsDB(context::Background(), &cfg) {
        Ok(_) => panic!("expected unknown driver"),
        Err(e) => e,
    };
    assert!(err.Error().contains("unknown checkpoint driver"));

    // File IgnoreErrorCheckpoint missing table
    // 文件后端忽略错误时找不到表，应返回 not found，
    // 这样上层才能区分“表不存在”和“IO/编解码失败”两类问题。
    let mut file_cp = NewFileCheckpointsDBWithExstorageFileName(
        context::Background(),
        "p",
        StorageHandle::memory(),
        "cp.pb",
    )
    .unwrap();
    let err = file_cp
        .IgnoreErrorCheckpoint(context::Background(), "`db`.`missing`")
        .unwrap_err();
    assert!(err.Error().contains("not found"));

    // File DumpTables unsupported
    // 文件 checkpoint 并不支持像 SQL 后端那样导出表列表；
    // 明确返回 unsupported 能避免调用方误判成空结果。
    let err = file_cp
        .DumpTables(context::Background(), &mut buf)
        .unwrap_err();
    assert!(err.Error().contains("not unsupported"));
}

fn contract_resource_cleanup() {
    // 回收场景负责验证持久化文件、任务快照与关闭语义之间的关系。
    // 这里的重点不是数据内容本身，而是生命周期动作带来的可观测副作用是否与 Go 对齐。
    // 对运维来说，这些副作用直接决定“重跑时还能否找到旧 checkpoint 文件”。

    // Close persists file checkpoint; RemoveCheckpoint all deletes file.
    // 初始化后应立刻创建 checkpoint 文件，说明最小任务元信息已经落盘。
    let ctx = context::Background();
    let mem = StorageHandle::memory();
    let mut file_cp =
        NewFileCheckpointsDBWithExstorageFileName(ctx, "p", mem.clone(), "cleanup.pb").unwrap();
    let mut db_info = HashMap::new();
    db_info.insert(
        "db".into(),
        importdef::DBInfo {
            Name: "db".into(),
            Tables: vec![importdef::TableInfo {
                Name: "t".into(),
                ID: 1,
                Desired: None,
            }],
        },
    );
    let cfg = config::Config {
        TaskID: 42,
        Mydumper: config::Mydumper {
            SourceDir: "/data".into(),
        },
        TikvImporter: config::TikvImporter {
            Backend: "local".into(),
            ..Default::default()
        },
        TiDB: config::TiDB {
            Host: "127.0.0.1".into(),
            Port: 4000,
            ..Default::default()
        },
        ..Default::default()
    };
    file_cp.Initialize(ctx, &cfg, db_info).unwrap();
    assert!(mem.FileExists(ctx, "cleanup.pb").unwrap());
    let task = file_cp.TaskCheckpoint(ctx).unwrap().unwrap();
    assert_eq!(task.TaskID, 42);
    assert_eq!(task.SourceDir, "/data");

    // `Close` 只负责刷盘和释放句柄，不应擅自删除文件。
    file_cp.Close().unwrap();
    assert!(mem.FileExists(ctx, "cleanup.pb").unwrap());

    // 而 `RemoveCheckpoint(all)` 对文件后端则意味着删除整个 checkpoint 文件。
    let mut file_cp =
        NewFileCheckpointsDBWithExstorageFileName(ctx, "p", mem.clone(), "cleanup.pb").unwrap();
    file_cp.RemoveCheckpoint(ctx, all_tables_name()).unwrap();
    assert!(!mem.FileExists(ctx, "cleanup.pb").unwrap());

    // MySQL NewMySQLCheckpointsDB creates schema via stub and Close closes DB
    // 最后补一个 MySQL 路径的轻量契约检查，确认构造和关闭接口至少可被上层安全调用。
    let db = sql::DB::new_memory();
    let mut mysql = NewMySQLCheckpointsDB(ctx, db, "tidb_lightning_checkpoint").unwrap();
    mysql.Close().unwrap();
}

fn all_tables_name() -> &'static str {
    // 抽成辅助函数而不是直接写字面量，
    // 是为了强调 `all` 在 checkpoint 协议里是“删除所有表检查点”的特殊保留值。
    // 这与普通表名参数的语义不同，测试也需要把它当成协议常量来保护。
    "all"
}
