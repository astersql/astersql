// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Rust equivalents of `lightning_serial_test.go`.
//!
//!
//! 本文件覆盖不依赖真实 HTTP server 的串行行为，重点放在 `run()` 主流程的错误分支。
//! 这些测试围绕配置解析、空数据源、checkpoint 驱动校验和系统要求计算展开。
//! 原因是这几类错误在 Go 版本里都必须在真正导入前尽早失败。
//! `source_url()` 统一生成当前目录的 `file://` 路径，避免不同执行目录导致基线漂移。
//! `fresh_lightning()` 每次返回全新实例，防止取消函数、importer 和指标状态在测试之间串扰。
//! `test_init_env` 验证日志文件配置既允许默认空值，也会拒绝目录路径这种明显非法输入。
//! 这里保护的是启动前置条件，而不是导入逻辑本身。
//! `test_run` 把错误路径拆成多段，确认 server 层在失败后会清空 `cancel` 与 `importer`。
//! 这条断言很关键，因为它决定任务失败后 HTTP 控制面还能否正确反映“当前无任务”。
//! 同一测试还覆盖未知 backend 与未知 checkpoint driver 两类不同层面的报错。
//! 前者来自 importer 选择，后者来自 legacy backend 预先打开 checkpoint DB 的对齐逻辑。
//! 日志缓冲区断言关注 keyspace 名称，是因为 Go 版本也会在 local backend 中记录该值。
//! 这里同时验证显式 keyspace 与自动探测为空的两条分支。
//! 文件 checkpoint 指向缺失父目录时必须失败，这能证明 Rust 没把底层存储错误吞掉。
//! 同时也能保证 `run()` 在真正开始导入前就会暴露 checkpoint 初始化问题。
//! `test_check_system_requirement` 手工构造多个表大小，复现 Go 的打开文件数估算公式。
//! 通过手动注入 rlimit，可以把系统依赖变成稳定可断言的纯函数场景。
//! 这类测试真正守护的是“估算公式和阈值”而不是某个固定实现细节。
//! 只要公式偏移，local backend 在大任务下就可能出现隐蔽的资源问题。
//! `test_check_schema_conflict` 则覆盖 MySQL checkpoint schema 与导入数据重名的保护逻辑。
//! 只有在 checkpoint 开启且驱动为 MySQL 时，这个冲突才应阻断任务继续。
//! 禁用 checkpoint 或切换到 file 驱动后必须放行，才能保持 Go 的分支语义。
//! 这里还会断言错误类名，以保证上层脚本和调用方仍能按旧合同识别该异常。
//! 整体上，本文件像是 `lightning.rs` 的前置错误合同表。
//! 当这些测试通过时，可以较有把握地认为 server 层没有在迁移中改变失败时机。
//! 因此这里的中文注释重点解释“为什么在这里失败”，而不是重复断言语法。
//! 如果未来有人改动 `run()`、`initEnv()` 或两类检查函数，应优先参考本文件对应场景。
//! 这些场景的共同特征是：一旦失败，系统不应留下半初始化状态。
//! 这也是为什么多处断言都会回头检查 `cancel` 与 `importer` 是否被清空。
//! 从维护角度看，本文件把“前置校验的 Go 语义”集中到了一个成本较低的测试入口。
//! 这样后续即便扩展更多 backend，也能先在这里验证失败时机没有被破坏。
//! 可以把它理解成对 `lightning.rs` 主流程的静态护栏。
//! 护栏越清晰，越能减少迁移后“逻辑能跑但边界行为已漂移”的风险。
//! 所以这些注释会刻意强调测试意图、输入形状和它们守护的资源边界。
//! 其中最重要的边界有三类：配置合法性、外部资源准备、与 Go 一致的错误分类。
//! 前两类决定任务是否能启动，后一类决定调用方如何理解失败原因。
//! 这也是本文件虽然不长，却承担了很多回归价值的原因。
//! 额外地，这些 case 还共同验证了失败后不会残留当前任务句柄。
//! 对 server 控制面而言，这和错误消息本身一样重要。
//! 如果残留状态没有被清掉，后续 HTTP 请求就会看到错误的 current task。
//! 因此测试会反复回头检查 `cancel` 与 `importer` 字段。
//! 这种清理约束与 Go 行为保持一致，是迁移中非常容易遗漏的一类细节。
//! 把它们写成中文注释，能帮助后续维护者在改主流程时更快识别风险点。
//! 也能解释为什么某些断言虽然重复，却仍然值得保留。

use super::*;

fn source_url() -> String {
    format!(
        "file://{}",
        std::env::current_dir().unwrap().to_string_lossy()
    )
}

fn fresh_lightning() -> Box<Lightning> {
    New(config::GlobalConfig::default())
}

#[test]
fn test_init_env() {
    let mut cfg = config::GlobalConfig::default();
    cfg.App.StatusAddr = ":45678".into();
    initEnv(&cfg).unwrap();

    cfg.App.StatusAddr.clear();
    cfg.App.Config.File = ".".into();
    let err = initEnv(&cfg).unwrap_err();
    assert_eq!(err.Error(), "can't use directory as log file name");
}

#[test]
fn test_run() {
    let mut lightning = fresh_lightning();

    let mut missing = config::Config::NewConfig();
    missing.Mydumper.SourceDir = "not-exists".into();
    missing.TikvImporter.Backend = config::BackendLocal.into();
    missing.TikvImporter.SortedKVDir = std::env::temp_dir()
        .join(format!("lightning-sorted-{}", uuid_util::New()))
        .to_string_lossy()
        .into();
    let err = lightning
        .RunOnceWithOptions(context::Background(), missing, vec![])
        .unwrap_err();
    assert!(
        err.Error()
            .ends_with("`mydumper.data-source-dir` does not exist"),
        "unexpected error: {err}"
    );
    assert!(lightning.cancel.is_none());
    assert!(lightning.importer.is_none());

    let db = sql::DB::new_memory();
    let (logger, buffer) = log::MakeTestLogger();
    let mut opts = options {
        logger,
        db: Some(db.clone()),
        ..Default::default()
    };
    let mut invalid_backend = config::Config::NewConfig();
    invalid_backend.Mydumper.SourceDir = source_url();
    invalid_backend.Checkpoint.Enable = true;
    invalid_backend.Checkpoint.Driver = "invalid".into();
    invalid_backend.TikvImporter.Backend.clear();
    let err = lightning
        .run(context::Background(), invalid_backend, &mut opts)
        .unwrap_err();
    assert_eq!(err.Error(), "unknown backend ");

    let mut invalid_driver = config::Config::NewConfig();
    invalid_driver.Mydumper.SourceDir = source_url();
    invalid_driver.TikvImporter.Backend = config::BackendLocal.into();
    invalid_driver.Checkpoint.Enable = true;
    invalid_driver.Checkpoint.Driver = "invalid".into();
    let err = lightning
        .run(context::Background(), invalid_driver.clone(), &mut opts)
        .unwrap_err();
    assert_eq!(
        err.Error(),
        "[Lightning:Checkpoint:ErrUnknownCheckpointDriver]unknown checkpoint driver 'invalid'"
    );
    assert!(
        buffer
            .String()
            .contains("\"acquired keyspace name\",\"keyspaceName\":\"\""),
        "missing empty-keyspace log: {}",
        buffer.String()
    );

    invalid_driver.TikvImporter.KeyspaceName = "test".into();
    let err = lightning
        .run(context::Background(), invalid_driver.clone(), &mut opts)
        .unwrap_err();
    assert_eq!(
        err.Error(),
        "[Lightning:Checkpoint:ErrUnknownCheckpointDriver]unknown checkpoint driver 'invalid'"
    );
    assert!(
        buffer
            .String()
            .contains("\"acquired keyspace name\",\"keyspaceName\":\"test\"")
    );

    let missing_parent = std::env::temp_dir().join(format!("server-cp-{}", uuid_util::New()));
    let mut bad_file = invalid_driver;
    bad_file.Checkpoint.Driver = config::CheckpointDriverFile.into();
    bad_file.Checkpoint.DSN = missing_parent.join("cp.pb").to_string_lossy().into();
    let err = lightning
        .run(context::Background(), bad_file, &mut opts)
        .unwrap_err();
    assert!(
        !err.Error().is_empty(),
        "file checkpoint with missing parent must fail"
    );
    assert!(lightning.cancel.is_none());
    assert!(lightning.importer.is_none());
}

#[test]
fn test_check_system_requirement() {
    let mut cfg = config::Config::NewConfig();
    cfg.App.RegionConcurrency = 16;
    cfg.App.CheckRequirements = true;
    cfg.App.TableConcurrency = 4;
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.TikvImporter.LocalWriterMemCacheSize = 128 * 1024 * 1024;
    cfg.TikvImporter.RangeConcurrency = 16;

    let db_metas = vec![
        mydump::MDDatabaseMeta {
            Tables: vec![
                mydump::MDTableMeta {
                    TotalSize: 500 << 20,
                    ..Default::default()
                },
                mydump::MDTableMeta {
                    TotalSize: 150_000 << 20,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        mydump::MDDatabaseMeta {
            Tables: vec![
                mydump::MDTableMeta {
                    TotalSize: 150_800 << 20,
                    ..Default::default()
                },
                mydump::MDTableMeta {
                    TotalSize: 35 << 20,
                    ..Default::default()
                },
                mydump::MDTableMeta {
                    TotalSize: 100_000 << 20,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        mydump::MDDatabaseMeta {
            Tables: vec![
                mydump::MDTableMeta {
                    TotalSize: 240 << 20,
                    ..Default::default()
                },
                mydump::MDTableMeta {
                    TotalSize: 124_000 << 20,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    ];

    ingestctrl::SetTestRLimit(Some(139_415), true);
    let err = checkSystemRequirement(&cfg, &db_metas).unwrap_err();
    assert_eq!(
        err.Error(),
        "cannot raise open file limit from 139415 to 139416"
    );

    ingestctrl::SetTestRLimit(Some(139_416), true);
    checkSystemRequirement(&cfg, &db_metas).unwrap();
    ingestctrl::SetTestRLimit(None, false);
}

#[test]
fn test_check_schema_conflict() {
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Schema = "cp".into();
    cfg.Checkpoint.Driver = config::CheckpointDriverMySQL.into();
    let mut db_metas = vec![
        mydump::MDDatabaseMeta {
            Name: "test".into(),
            Tables: vec![
                mydump::MDTableMeta {
                    Name: "table_v10".into(),
                    ..Default::default()
                },
                mydump::MDTableMeta {
                    Name: "engine_v5".into(),
                    ..Default::default()
                },
            ],
        },
        mydump::MDDatabaseMeta {
            Name: "cp".into(),
            Tables: vec![mydump::MDTableMeta {
                Name: "test".into(),
                ..Default::default()
            }],
        },
    ];
    checkSchemaConflict(&cfg, &db_metas).unwrap();

    db_metas.push(mydump::MDDatabaseMeta {
        Name: "cp".into(),
        Tables: vec![
            mydump::MDTableMeta {
                Name: "chunk_v6".into(),
                ..Default::default()
            },
            mydump::MDTableMeta {
                Name: "test123".into(),
                ..Default::default()
            },
        ],
    });
    let err = checkSchemaConflict(&cfg, &db_metas).unwrap_err();
    assert_eq!(
        err.class,
        Some("Lightning:Checkpoint:ErrCheckpointSchemaConflict")
    );
    assert_eq!(
        err.Error(),
        "checkpoint table `cp`.`chunk_v6` conflict with data files. Please change the `checkpoint.schema` config or set `checkpoint.driver` to \"file\" instead"
    );

    cfg.Checkpoint.Enable = false;
    checkSchemaConflict(&cfg, &db_metas).unwrap();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    checkSchemaConflict(&cfg, &db_metas).unwrap();
}
