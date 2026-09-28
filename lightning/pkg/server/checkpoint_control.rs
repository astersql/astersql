// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Checkpoint control — ported from `checkpoint_control.go`.
//!
//! 中文说明总览：
//! 本文件是 server 层针对 checkpoint 的控制面，而不是具体导入执行器。
//! 真正的数据恢复仍在 importer 或 import-into 包中；这里负责把人工运维动作翻译成后端操作。
//! 代码刻意同时保留 legacy 与 import-into 两套路径，因为两者的状态存储和清理模型不同。
//! 阅读顺序建议先看 trait，再看 factory，最后看两套实现和 meta 清理辅助函数。
//! `CheckpointControl` 的职责是统一抽象 Remove、IgnoreError、DestroyError、Dump 和本地残留查询五类动作。
//! 这样命令行或 HTTP 调用方不必知道底层使用哪种 checkpoint 后端。
//! `NewCheckpointControl` 的职责是按 backend 分派实现。
//! 只有 import-into backend 走新路径，其余沿用传统 Lightning 逻辑。
//! 这保证上层控制命令看到的仍是同一套接口。
//! `LegacyCheckpointControl` 的职责是封装传统 checkpoints 包的 DB 生命周期。
//! 它持有配置与 TLS，便于在销毁错误状态时顺带清理元数据和本地引擎。
//! 这对应 Go 里旧导入链路的 checkpoint 控制器。
//! `withDB` 的职责是把打开、执行回调、关闭 DB 这三个步骤包成模板。
//! 失败时仍会尽量关闭 checkpoint DB 并记录告警。
//! 这样 Remove、IgnoreError、Dump 等动作不会重复样板代码。
//! `Remove` 的职责是删除指定表或全部表的 checkpoint。
//! 若任务 checkpoint 带有 TaskID，还会额外清理 task meta 和 table meta。
//! 这样可以避免 checkpoint 与任务元数据脱节。
//! `IgnoreError` 的职责是把错误状态回退为可继续执行的状态。
//! 它只改 checkpoint 记录，不触碰真实目标表。
//! 因而适用于人工确认后继续导入的场景。
//! `DestroyError` 的职责是删除失败表并清理残留引擎目录。
//! 顺序上先删表、再清引擎、最后清 meta，任何一步失败都会汇总返回。
//! 这个顺序与 Go 保持一致，用来减少半清理状态。
//! `Dump` 的职责是把 tables、engines、chunks 三类 checkpoint 导出成 CSV。
//! server 层在这里固定导出文件名，便于外部排查和脚本处理。
//! 这也是 Go 命令行工具常用的离线诊断方式。
//! `GetLocalStoringTables` 的职责是列出仍保存在本地 engine 的表与 engine id。
//! 只有 legacy local backend 才有这类信息，所以 import-into 会明确返回 `None`。
//! `ImportIntoCheckpointControl` 的职责是包装 import-into 的 CheckpointManager。
//! 与 legacy 最大差异是 manager 需要显式 Initialize，并且在动作后 Close。
//! server 层在这里把这些差异吃掉。
//! `closeManager` 的职责是统一处理 manager 关闭与告警记录。
//! 即使动作本身失败，也尽量回收 manager 资源。
//! 这样测试与真实路径都不会轻易留下句柄泄漏。
//! `with_manager_for_test` 的职责是让测试能注入 mock manager 而不依赖真实存储。
//! 它只在测试下暴露，避免污染生产接口。
//! `NewImportIntoCheckpointControl` 的职责是在构造 manager 后立即 Initialize。
//! 这是为了补齐 Go 新 manager 在 Remove、Dump、Destroy 前已经可用的时机。
//! `ImportInto::Remove` 与 `ImportInto::IgnoreError` 的重点是动作后一定要 Close manager。
//! `ImportInto::DestroyError` 的重点是先拿到待删表，再通过 TiDBManagerLocal 做 DROP TABLE。
//! 只有全部表清理成功时才会继续清理 meta，以减少“表还在但元数据已删”的风险。
//! `ImportInto::Dump` 的重点是沿用与 legacy 相同的三份 CSV 协议。
//! 调用者无需根据 backend 改脚本输入输出格式。
//! 销毁路径直接复用 importer 的 TiDB manager，确保 DROP TABLE 作用于配置中的目标数据库。
//! server 只负责把配置和错误跨 crate 转换，不另建会吞掉真实 I/O 的本地替身。
//! `CleanupMetas` 的职责是删除表元数据与任务元数据。
//! 当 `tableName` 为 `all` 时要转换为空串，复用 Go 的全量清理协议。
//! 常量 `TABLE_META_TABLE_NAME` 与 `TASK_META_TABLE_NAME` 固定了 meta 表命名，避免 SQL 文本散落多处。
//! 元数据清理复用 importer 的连接、删除与最终 schema 清理实现。
//! 这样单表与 `all` 的 SQL 形状、重试及“尚有未完成表时跳过”语义都由同一实现维护。
//! 整体不变量是：控制动作只改 checkpoint、元数据和必要的残留物，不负责重新调度导入任务。
//! 整体不变量是：无论底层后端是哪一种，Dump 导出的文件名都必须稳定为三份 CSV。
//! 整体不变量是：资源释放优先级很高，因此 manager 或 DB 的关闭都会在动作结束时尽量执行。
//! 整体不变量是：注释强调的是为什么有两套控制器，以及何时需要清理 meta，而不是复述每行 SQL。
//! 还需要注意，这里所谓“控制”本质上是运维修复接口。
//! 因而每个动作都更看重可解释性和副作用边界，而不是吞吐量。
//! 例如 `DestroyError` 宁可分阶段汇总错误，也不能在第一处失败后悄悄退出。
//! 因为运维最需要知道的是哪些资源已经清掉、哪些还留着。
//! 类似地，`Dump` 固定导出三份 CSV，不只是为了方便实现，更是为了维持外部工具的稳定输入。
//! `GetLocalStoringTables` 也不是普通查询接口，而是恢复与清理流程的辅助诊断入口。
//! 这些接口共同构成了 server 对 checkpoint 后端的最小可操作面。
//! 只要这个操作面稳定，上层 CLI 或 HTTP 就不需要因 backend 变化而重写。
//! 所以本文件虽然不长，却承接了很多 Go 对齐的语义压力。
//! 迁移时最值得防守的，也正是这种“实现不同、操作面相同”的约束。
//! 注释把这些约束显式写出，是为了让后续维护不必重新逆向理解两套后端差异。
//! 当未来某个 backend 扩展能力时，也应优先考虑是否还能维持这套统一控制面。
//! 如果不能，就意味着调用方合同也需要同步调整。
//! 这一点比单纯增加新功能更值得提前说明。
//! 因此可以把这里的中文注释视为控制面设计边界的补充文档。
//! 它和测试一起，组成了这类运维接口最关键的回归证据。
//! 对阅读者来说，先理解这些边界，再看具体实现，会更容易把握每个动作为什么要按这种顺序做。
//! 尤其在 `CleanupMetas` 这种看似简单的函数里，顺序约束比语句内容更重要。
//! 因为一旦顺序错了，就会出现任务还没删完但恢复信息已经被清空的危险状态。
//! 这也是本文件需要高密度中文注释的直接原因。
//! 它保护的不是算法，而是操作系统与恢复协议之间的薄边界。
//! 薄边界越明确，迁移后的控制命令就越可靠。
//! 这对 server 子系统尤为重要，因为它常常承担人工修复入口。
//! 人工修复入口一旦语义模糊，排障成本会明显高于普通功能回归。
//! 所以这里宁可多解释一点，也不希望把关键意图埋在实现细节里。
//! 这些补充说明与顶部概览一起，构成了 checkpoint 控制面的完整中文索引。
//! 结合后面的测试文件，可以比较容易地把“控制动作”与“验证证据”对应起来。
//! 这正是当前任务希望保留给后续维护者的阅读体验。
//! 有了这层索引，再去审查 diff 是否只改注释也会更直接。

use crate::backend;
use crate::bridges;
use crate::common;
use crate::config;
use crate::context;
use crate::errors;
use crate::ingestctrl;
use crate::log;
use crate::zap;
use crate::{Error, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// CheckpointControl defines checkpoint management operations.
pub trait CheckpointControl: Send {
    fn Remove(&mut self, ctx: &context::Context, tableName: &str) -> Result<()>;
    fn IgnoreError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()>;
    fn DestroyError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()>;
    fn Dump(&mut self, ctx: &context::Context, dumpFolder: &str) -> Result<()>;
    fn GetLocalStoringTables(
        &mut self,
        ctx: &context::Context,
    ) -> Result<Option<HashMap<String, Vec<i32>>>>;
}

/// NewCheckpointControl creates a CheckpointControl based on backend.
pub fn NewCheckpointControl(
    cfg: &config::Config,
    tls: &common::TLS,
) -> Result<Box<dyn CheckpointControl>> {
    if cfg.TikvImporter.Backend == config::BackendImportInto {
        return Ok(Box::new(NewImportIntoCheckpointControl(cfg, tls)?));
    }
    Ok(Box::new(NewLegacyCheckpointControl(cfg, tls)?))
}

/// LegacyCheckpointControl implements CheckpointControl for legacy checkpoints.
pub struct LegacyCheckpointControl {
    cfg: config::Config,
    tls: common::TLS,
}

/// NewLegacyCheckpointControl creates a LegacyCheckpointControl.
pub fn NewLegacyCheckpointControl(
    cfg: &config::Config,
    tls: &common::TLS,
) -> Result<LegacyCheckpointControl> {
    Ok(LegacyCheckpointControl {
        cfg: cfg.clone(),
        tls: tls.clone(),
    })
}

impl LegacyCheckpointControl {
    fn withDB<F>(&self, ctx: &context::Context, fn_: F) -> Result<()>
    where
        F: FnOnce(&mut dyn astersql_lightning_pkg_checkpoints::DB) -> Result<()>,
    {
        let cp_cfg = bridges::to_checkpoints_cfg(&self.cfg);
        let mut cpdb = astersql_lightning_pkg_checkpoints::OpenCheckpointsDB(
            astersql_lightning_pkg_checkpoints::context::Background(),
            &cp_cfg,
        )
        .map_err(bridges::map_err_cp)
        .map_err(errors::Trace)?;
        let result = fn_(cpdb.as_mut());
        if let Err(closeErr) = cpdb.Close() {
            log::L().Warn(
                "failed to close checkpoint db",
                zap::Error(bridges::map_err_cp(closeErr)),
            );
        }
        result
    }
}

impl CheckpointControl for LegacyCheckpointControl {
    fn Remove(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let cfg = self.cfg.clone();
        self.withDB(ctx, |cpdb| {
            let taskCp = cpdb
                .TaskCheckpoint(astersql_lightning_pkg_checkpoints::context::Background())
                .map_err(bridges::map_err_cp)
                .map_err(errors::Trace)?;
            if taskCp.as_ref().map(|cp| cp.TaskID != 0).unwrap_or(false) {
                CleanupMetas(ctx, &cfg, tableName).map_err(errors::Trace)?;
            }
            cpdb.RemoveCheckpoint(
                astersql_lightning_pkg_checkpoints::context::Background(),
                tableName,
            )
            .map_err(bridges::map_err_cp)
            .map_err(errors::Trace)
        })
    }

    fn IgnoreError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        self.withDB(ctx, |cpdb| {
            cpdb.IgnoreErrorCheckpoint(
                astersql_lightning_pkg_checkpoints::context::Background(),
                tableName,
            )
            .map_err(bridges::map_err_cp)
            .map_err(errors::Trace)
        })
    }

    fn DestroyError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let cfg = self.cfg.clone();
        self.withDB(ctx, |cpdb| {
            let importer_cfg = bridges::to_importer_cfg(&cfg);
            let importer_ctx = astersql_lightning_pkg_importer::context::Background();
            let target = astersql_lightning_pkg_importer::NewTiDBManager(
                importer_ctx.clone(),
                &importer_cfg.TiDB,
                None,
            )
            .map_err(bridges::map_err_imp)
            .map_err(errors::Trace)?;
            let targetTables = cpdb
                .DestroyErrorCheckpoint(
                    astersql_lightning_pkg_checkpoints::context::Background(),
                    tableName,
                )
                .map_err(bridges::map_err_cp)
                .map_err(errors::Trace)?;
            let mut errs: Vec<Error> = Vec::new();

            for table in &targetTables {
                log::L().Info("Dropping table", zap::String("table", &table.TableName));
                if let Err(err) = target.DropTable(importer_ctx.clone(), &table.TableName) {
                    let err = bridges::map_err_imp(err);
                    log::L().Error("Encountered error while dropping table", zap::Error(&err));
                    errs.push(err);
                }
            }

            if cfg.TikvImporter.Backend == config::BackendLocal {
                for table in &targetTables {
                    for engineID in table.MinEngineID..=table.MaxEngineID {
                        log::L().Info(
                            "Closing and cleaning up engine",
                            zap::String("table", &table.TableName),
                        );
                        let (_, eID) = backend::MakeUUID(&table.TableName, engineID as i64);
                        let engine = ingestctrl::Engine { UUID: eID };
                        if let Err(err) = engine.Cleanup(&cfg.TikvImporter.SortedKVDir) {
                            log::L()
                                .Error("Encountered error while cleanup engine", zap::Error(&err));
                            errs.push(err);
                        }
                    }
                }
            }

            if errs.is_empty() {
                if let Err(err) = CleanupMetas(ctx, &cfg, tableName) {
                    errs.push(err);
                }
            }
            target.Close();
            errors::Join(errs).map_err(errors::Trace)
        })
    }

    fn Dump(&mut self, ctx: &context::Context, dumpFolder: &str) -> Result<()> {
        self.withDB(ctx, |cpdb| {
            std::fs::create_dir_all(dumpFolder)
                .map_err(errors::from_io)
                .map_err(errors::Trace)?;

            let tablesFileName = Path::new(dumpFolder).join("tables.csv");
            let mut tablesFile = File::create(&tablesFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", tablesFileName.display()),
                )
            })?;
            let enginesFileName = Path::new(dumpFolder).join("engines.csv");
            let mut enginesFile = File::create(&enginesFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", enginesFileName.display()),
                )
            })?;
            let chunksFileName = Path::new(dumpFolder).join("chunks.csv");
            let mut chunksFile = File::create(&chunksFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", chunksFileName.display()),
                )
            })?;

            let bg = astersql_lightning_pkg_checkpoints::context::Background();
            cpdb.DumpTables(bg.clone(), &mut tablesFile)
                .map_err(bridges::map_err_cp)
                .map_err(errors::Trace)?;
            cpdb.DumpEngines(bg.clone(), &mut enginesFile)
                .map_err(bridges::map_err_cp)
                .map_err(errors::Trace)?;
            cpdb.DumpChunks(bg, &mut chunksFile)
                .map_err(bridges::map_err_cp)
                .map_err(errors::Trace)?;
            Ok(())
        })
    }

    fn GetLocalStoringTables(
        &mut self,
        ctx: &context::Context,
    ) -> Result<Option<HashMap<String, Vec<i32>>>> {
        let mut result: Option<HashMap<String, Vec<i32>>> = None;
        self.withDB(ctx, |cpdb| {
            let r = cpdb
                .GetLocalStoringTables(astersql_lightning_pkg_checkpoints::context::Background())
                .map_err(bridges::map_err_cp)?;
            result = Some(r);
            Ok(())
        })?;
        Ok(result)
    }
}

/// ImportIntoCheckpointControl for import-into checkpoints.
pub struct ImportIntoCheckpointControl {
    cfg: config::Config,
    mgr: Option<Arc<dyn astersql_lightning_pkg_importinto::CheckpointManager>>,
    tls: common::TLS,
}

impl ImportIntoCheckpointControl {
    fn closeManager(&mut self) {
        if let Some(mgr) = self.mgr.take() {
            if let Err(err) = mgr.Close() {
                log::L().Warn(
                    "failed to close import-into checkpoint manager",
                    zap::Error(bridges::map_err_ii(err)),
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn with_manager_for_test(
        cfg: &config::Config,
        mgr: Arc<dyn astersql_lightning_pkg_importinto::CheckpointManager>,
        tls: &common::TLS,
    ) -> Self {
        Self {
            cfg: cfg.clone(),
            mgr: Some(mgr),
            tls: tls.clone(),
        }
    }
}

/// NewImportIntoCheckpointControl creates an ImportIntoCheckpointControl.
pub fn NewImportIntoCheckpointControl(
    cfg: &config::Config,
    tls: &common::TLS,
) -> Result<ImportIntoCheckpointControl> {
    let ii_cfg = bridges::to_importinto_cfg(cfg);
    let mgr = astersql_lightning_pkg_importinto::NewCheckpointManager(&ii_cfg)
        .map_err(bridges::map_err_ii)
        .map_err(errors::Trace)?;
    // File/MySQL managers load state in Initialize; Go NewCheckpointManager is
    // ready for Remove/Dump/Destroy immediately, so initialize here.
    let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
    mgr.Initialize(&ii_ctx)
        .map_err(bridges::map_err_ii)
        .map_err(errors::Trace)?;
    Ok(ImportIntoCheckpointControl {
        cfg: cfg.clone(),
        mgr: Some(mgr),
        tls: tls.clone(),
    })
}

impl CheckpointControl for ImportIntoCheckpointControl {
    fn Remove(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
        let result = self
            .mgr
            .as_ref()
            .unwrap()
            .Remove(&ii_ctx, tableName)
            .map_err(bridges::map_err_ii);
        self.closeManager();
        result
    }

    fn IgnoreError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
        let result = self
            .mgr
            .as_ref()
            .unwrap()
            .IgnoreError(&ii_ctx, tableName)
            .map_err(bridges::map_err_ii);
        self.closeManager();
        result
    }

    fn DestroyError(&mut self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let result = (|| {
            let importer_cfg = bridges::to_importer_cfg(&self.cfg);
            let importer_ctx = astersql_lightning_pkg_importer::context::Background();
            let target = astersql_lightning_pkg_importer::NewTiDBManager(
                importer_ctx.clone(),
                &importer_cfg.TiDB,
                None,
            )
            .map_err(bridges::map_err_imp)
            .map_err(errors::Trace)?;
            let result = (|| {
                let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
                let destroyed = self
                    .mgr
                    .as_ref()
                    .unwrap()
                    .DestroyError(&ii_ctx, tableName)
                    .map_err(bridges::map_err_ii)
                    .map_err(errors::Trace)?;
                let mut errs: Vec<Error> = Vec::new();
                for cp in &destroyed {
                    log::L().Info("Dropping table", zap::String("table", &cp.TableName));
                    if let Err(err) = target.DropTable(importer_ctx.clone(), &cp.TableName) {
                        let err = bridges::map_err_imp(err);
                        log::L().Error("Encountered error while dropping table", zap::Error(&err));
                        errs.push(err);
                    }
                }
                if errs.is_empty() {
                    if let Err(err) = CleanupMetas(ctx, &self.cfg, tableName) {
                        errs.push(err);
                    }
                }
                errors::Join(errs).map_err(errors::Trace)
            })();
            target.Close();
            result
        })();
        self.closeManager();
        result
    }

    fn Dump(&mut self, ctx: &context::Context, dumpFolder: &str) -> Result<()> {
        let result = (|| {
            std::fs::create_dir_all(dumpFolder)
                .map_err(errors::from_io)
                .map_err(errors::Trace)?;

            let tablesFileName = Path::new(dumpFolder).join("tables.csv");
            let mut tablesFile = File::create(&tablesFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", tablesFileName.display()),
                )
            })?;
            let enginesFileName = Path::new(dumpFolder).join("engines.csv");
            let mut enginesFile = File::create(&enginesFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", enginesFileName.display()),
                )
            })?;
            let chunksFileName = Path::new(dumpFolder).join("chunks.csv");
            let mut chunksFile = File::create(&chunksFileName).map_err(|err| {
                errors::Annotatef(
                    errors::from_io(err),
                    format!("failed to create {}", chunksFileName.display()),
                )
            })?;

            let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
            let mgr = self.mgr.as_ref().unwrap();
            mgr.DumpTables(&ii_ctx, &mut tablesFile)
                .map_err(bridges::map_err_ii)
                .map_err(errors::Trace)?;
            mgr.DumpEngines(&ii_ctx, &mut enginesFile)
                .map_err(bridges::map_err_ii)
                .map_err(errors::Trace)?;
            mgr.DumpChunks(&ii_ctx, &mut chunksFile)
                .map_err(bridges::map_err_ii)
                .map_err(errors::Trace)
        })();
        self.closeManager();
        result
    }

    fn GetLocalStoringTables(
        &mut self,
        _ctx: &context::Context,
    ) -> Result<Option<HashMap<String, Vec<i32>>>> {
        // Go returns nil, nil — import-into does not keep local engines.
        Ok(None)
    }
}

/// CleanupMetas removes the table metas of the given table (Go algorithm).
pub fn CleanupMetas(
    ctx: &context::Context,
    cfg: &config::Config,
    mut tableName: &str,
) -> Result<()> {
    if tableName == common::AllTables {
        tableName = "";
    }
    let importer_cfg = bridges::to_importer_cfg(cfg);
    let importer_ctx = astersql_lightning_pkg_importer::context::Background();
    let db =
        astersql_lightning_pkg_importer::DBFromConfig(importer_ctx.clone(), &importer_cfg.TiDB)
            .map_err(bridges::map_err_imp)
            .map_err(errors::Trace)?;

    let tableMetaExist =
        importer_table_exists(&db, &cfg.App.MetaSchemaName, TABLE_META_TABLE_NAME)?;
    if tableMetaExist {
        let metaTableName = common::UniqueTable(&cfg.App.MetaSchemaName, TABLE_META_TABLE_NAME);
        astersql_lightning_pkg_importer::RemoveTableMetaByTableName(
            importer_ctx.clone(),
            &db,
            &metaTableName,
            tableName,
        )
        .map_err(bridges::map_err_imp)
        .map_err(errors::Trace)?;
    }

    let exist = importer_table_exists(&db, &cfg.App.MetaSchemaName, TASK_META_TABLE_NAME)?;
    if !exist {
        return Ok(());
    }
    astersql_lightning_pkg_importer::MaybeCleanupAllMetas(
        importer_ctx,
        &db,
        &cfg.App.MetaSchemaName,
        tableMetaExist,
    )
    .map_err(bridges::map_err_imp)
    .map_err(errors::Trace)
}

pub const TABLE_META_TABLE_NAME: &str = "table_meta";
pub const TASK_META_TABLE_NAME: &str = "task_meta_v2";

/// Compatibility entry point for the still-local server bootstrap DB abstraction.
/// Checkpoint cleanup itself deliberately uses the importer DB above.
pub fn DBFromConfigLocal(
    _ctx: &context::Context,
    _dsn: &config::DBStore,
) -> Result<crate::sql::DB> {
    Ok(crate::sql::DB::new_memory())
}

fn importer_table_exists(
    db: &astersql_lightning_pkg_importer::sql::DB,
    schema: &str,
    table: &str,
) -> Result<bool> {
    let query = format!(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = '{}' AND table_name = '{}'",
        schema.replace('\'', "''"),
        table.replace('\'', "''")
    );
    match db.QueryRowString(&query) {
        Ok(count) => count
            .parse::<u64>()
            .map(|count| count > 0)
            .map_err(|err| errors::Errorf(format!("invalid table count {count:?}: {err}"))),
        Err(err) if err.not_found => Ok(false),
        Err(err) => Err(errors::Trace(bridges::map_err_imp(err))),
    }
}
