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

// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 本次只增加注释，不改变任何运行时逻辑或测试行为。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Import controller matching Go `import.go`.

use crate::atomic;
use crate::backend::{self, Backend, EngineManager};
use crate::check_template::{NewSimpleTemplate, Template};
use crate::common;
use crate::config::{self, Config};
use crate::context::Context;
use crate::encode::EncodingBuilder;
use crate::errors::{self, Result};
use crate::get_pre_info::PreImportInfoGetter;
use crate::importdef;
use crate::ingestctrl::TiKVModeSwitcher;
use crate::log::Logger;
use crate::logutil;
use crate::meta_manager::{
    metaMgrBuilder, noopMetaMgrBuilder, singleMgrBuilder, taskMeta, taskMetaMgr,
    taskMetaStatusInitial,
};
use crate::model;
use crate::mydump;
use crate::pd;
use crate::pdhttp;
use crate::precheck::PrecheckItemBuilder;
use crate::set::StringSet;
use crate::sql::DB;
use crate::storeapi::Storage;
use crate::table_import::{NewTableImporter, TableImporter};
use crate::types::{self, Datum};
use crate::worker;
use crate::zap;
use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_errormanager as errormanager;
use astersql_lightning_pkg_progress as progress;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// 自动补充的`FullLevelCompact` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub const FullLevelCompact: i32 = -1;
pub const Level1Compact: i32 = 1;

pub const compactStateIdle: i32 = 0;
// 自动补充的`compactStateDoing` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub const compactStateDoing: i32 = 1;

pub const diskQuotaStateIdle: i32 = 0;
pub const diskQuotaStateChecking: i32 = 1;
// 自动补充的`diskQuotaStateImporting` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub const diskQuotaStateImporting: i32 = 2;

pub static DeliverPauser: once_pauser::Lazy = once_pauser::Lazy::new();

mod once_pauser {
    use crate::common::{NewPauser, Pauser};
    use std::sync::OnceLock;
    // 自动补充的`Lazy` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Lazy;
    impl Lazy {
        pub const fn new() -> Self {
            Self
        }
        // 自动补充的`get` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn get(&self) -> &'static Pauser {
            static P: OnceLock<Pauser> = OnceLock::new();
            P.get_or_init(NewPauser)
        }
    }
}

// 自动补充的`saveCp` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct saveCp {
    pub tableName: String,
    pub merger: Option<Box<dyn checkpoints::TableCheckpointMerger>>,
    pub waitCh: Option<std::sync::mpsc::Sender<Option<crate::Error>>>,
}

// 自动补充的`errorSummary` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct errorSummary {
    pub status: checkpoints::CheckpointStatus,
    pub err: crate::Error,
}

// 自动补充的`errorSummaries` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct errorSummaries {
    pub logger: Logger,
    pub summary: Mutex<HashMap<String, errorSummary>>,
}

// 自动补充的`makeErrorSummaries` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn makeErrorSummaries(logger: Logger) -> errorSummaries {
    errorSummaries {
        logger,
        summary: Mutex::new(HashMap::new()),
    }
}

// 自动补充的下面的 `impl errorSummaries` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl errorSummaries {
    pub fn emitLog(&self) {
        let guard = self.summary.lock().unwrap();
        let errorCount = guard.len();
        if errorCount > 0 {
            self.logger.Error(
                "tables failed to be imported",
                &[zap::Int("count", errorCount as i64)],
            );
            for (tableName, errorSummary) in guard.iter() {
                self.logger.Error(
                    "-",
                    &[
                        zap::String("table", tableName),
                        zap::String("status", checkpoints::MetricName(errorSummary.status)),
                        crate::log::ShortError(&errorSummary.err),
                    ],
                );
            }
        }
    }

    // 自动补充的`record` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn record(
        &self,
        tableName: &str,
        err: crate::Error,
        status: checkpoints::CheckpointStatus,
    ) {
        self.summary
            .lock()
            .unwrap()
            .insert(tableName.to_string(), errorSummary { status, err });
    }
}

// 自动补充的`Controller` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct Controller {
    pub taskCtx: Context,
    pub cfg: Config,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    pub dbInfos: HashMap<String, importdef::DBInfo>,
    pub tableWorkers: Option<Arc<worker::Pool>>,
    pub indexWorkers: Option<Arc<worker::Pool>>,
    pub regionWorkers: Option<Arc<worker::Pool>>,
    pub ioWorkers: Option<Arc<worker::Pool>>,
    pub checksumWorks: Option<Arc<worker::Pool>>,
    pub pauser: common::Pauser,
    pub engineMgr: EngineManager,
    pub backend: Option<Arc<dyn Backend>>,
    pub db: Option<DB>,
    pub pdCli: pd::Client,
    pub pdHTTPCli: Option<pdhttp::Client>,
    pub sysVars: HashMap<String, String>,
    pub tls: Option<common::TLS>,
    pub checkTemplate: Box<dyn Template>,
    pub errorSummaries: errorSummaries,
    pub checkpointsDB: Option<std::sync::Mutex<Box<dyn checkpoints::DB>>>,
    pub saveCpCh: Mutex<Vec<saveCp>>,
    pub closedEngineLimit: Option<Arc<worker::Pool>>,
    pub addIndexLimit: Option<Arc<worker::Pool>>,
    pub store: Storage,
    pub ownStore: bool,
    pub metaMgrBuilder: Arc<dyn metaMgrBuilder>,
    pub errorMgr: Option<errormanager::ErrorManager>,
    pub taskMgr: Option<Arc<dyn taskMetaMgr>>,
    pub diskQuotaState: atomic::Int32,
    pub compactState: atomic::Int32,
    pub status: Option<Arc<LightningStatus>>,
    pub dupIndicator: Option<atomic::Bool>,
    pub preInfoGetter: Option<Arc<dyn PreImportInfoGetter>>,
    pub precheckItemBuilder: Option<PrecheckItemBuilder>,
    pub encBuilder: Option<Arc<dyn EncodingBuilder>>,
    pub tikvModeSwitcher: Option<Arc<dyn TiKVModeSwitcher>>,
    pub keyspaceName: String,
    pub apiContext: pd::APIContext,
    pub resourceGroupName: String,
    pub taskType: String,
    pub closed: bool,
}

// 自动补充的`LightningStatus` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct LightningStatus {
    pub backend: String,
    pub FinishedFileSize: atomic::Int64,
    pub TotalFileSize: atomic::Int64,
}

// 自动补充的`ControllerParam` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct ControllerParam {
    pub DBMetas: Vec<mydump::MDDatabaseMeta>,
    pub Status: Option<Arc<LightningStatus>>,
    pub DumpFileStorage: Storage,
    pub OwnExtStorage: bool,
    pub Pauser: Option<common::Pauser>,
    pub DB: Option<DB>,
    pub CheckpointStorage: Option<Storage>,
    pub CheckpointName: String,
    pub DupIndicator: Option<atomic::Bool>,
    pub KeyspaceName: String,
    pub ResourceGroupName: String,
    pub TaskType: String,
}

// 自动补充的`componentName` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub static componentName: &str = "lightning-importer";

pub fn NewImportController(
    ctx: Context,
    cfg: &Config,
    mut param: ControllerParam,
) -> Result<Controller> {
    param.Pauser = Some(DeliverPauser.get().clone());
    NewImportControllerWithPauser(ctx, cfg, param)
}

// 自动补充的`NewImportControllerWithPauser` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn NewImportControllerWithPauser(
    ctx: Context,
    cfg: &Config,
    param: ControllerParam,
) -> Result<Controller> {
    let pauser = param.Pauser.unwrap_or_else(common::NewPauser);
    let checkTemplate = NewSimpleTemplate();
    let errorSummaries = makeErrorSummaries(logutil::Logger(ctx.clone()));
    let metaMgrBuilder: Arc<dyn metaMgrBuilder> = if isLocalBackend(cfg) {
        if cfg.TikvImporter.ParallelImport {
            Arc::new(crate::meta_manager::dbMetaMgrBuilder {
                db: param.DB.clone().unwrap_or_else(DB::new_memory),
                taskID: cfg.TaskID as u64,
                schema: cfg.App.TaskInfoSchemaName.clone(),
            })
        } else {
            Arc::new(singleMgrBuilder {
                taskID: cfg.TaskID as u64,
            })
        }
    } else {
        Arc::new(noopMetaMgrBuilder)
    };

    let errorMgr = param.DB.as_ref().map(|db| {
        // Bridge to errormanager config/sql stubs.
        let mut em_cfg = errormanager::config::Config::NewConfig();
        em_cfg.TaskID = cfg.TaskID;
        em_cfg.TikvImporter.Backend = cfg.TikvImporter.Backend.clone();
        em_cfg.Conflict.Strategy = cfg.Conflict.Strategy;
        em_cfg.Conflict.Threshold = cfg.Conflict.Threshold;
        em_cfg.Conflict.MaxRecordRows = cfg.Conflict.MaxRecordRows;
        em_cfg.App.TaskInfoSchemaName = cfg.App.TaskInfoSchemaName.clone();
        // Build errormanager DB from memory stand-in (separate type).
        let em_db = errormanager::sql::DB::new_memory();
        errormanager::New(Some(em_db), &em_cfg, errormanager::log::Logger::L())
    });

    Ok(Controller {
        taskCtx: ctx,
        cfg: cfg.clone(),
        dbMetas: param.DBMetas,
        dbInfos: HashMap::new(),
        tableWorkers: Some(worker::Pool::New(
            cfg.App.TableConcurrency.max(1) as usize,
            "table",
        )),
        indexWorkers: Some(worker::Pool::New(
            cfg.App.IndexConcurrency.max(1) as usize,
            "index",
        )),
        regionWorkers: Some(worker::Pool::New(
            cfg.App.RegionConcurrency.max(1) as usize,
            "region",
        )),
        ioWorkers: Some(worker::Pool::New(
            cfg.App.RegionConcurrency.max(1) as usize,
            "io",
        )),
        checksumWorks: Some(worker::Pool::New(1, "checksum")),
        pauser,
        engineMgr: EngineManager::default(),
        backend: Some(Arc::new(backend::LocalBackend {
            name: cfg.TikvImporter.Backend.clone(),
        })),
        db: param.DB,
        pdCli: pd::Client::default(),
        pdHTTPCli: None,
        sysVars: HashMap::new(),
        tls: None,
        checkTemplate,
        errorSummaries,
        checkpointsDB: Some(std::sync::Mutex::new(
            Box::new(checkpoints::NewNullCheckpointsDB()) as Box<dyn checkpoints::DB>,
        )),
        saveCpCh: Mutex::new(Vec::new()),
        closedEngineLimit: None,
        addIndexLimit: None,
        store: param.DumpFileStorage,
        ownStore: param.OwnExtStorage,
        metaMgrBuilder,
        errorMgr,
        taskMgr: None,
        diskQuotaState: atomic::NewInt32(diskQuotaStateIdle),
        compactState: atomic::NewInt32(compactStateIdle),
        status: param.Status,
        dupIndicator: param.DupIndicator,
        preInfoGetter: None,
        precheckItemBuilder: None,
        encBuilder: Some(Arc::new(crate::encode::NoopEncBuilder)),
        tikvModeSwitcher: Some(Arc::new(crate::ingestctrl::NoopModeSwitcher)),
        keyspaceName: param.KeyspaceName,
        apiContext: pd::APIContext::default(),
        resourceGroupName: param.ResourceGroupName,
        taskType: param.TaskType,
        closed: false,
    })
}

// 自动补充的下面的 `impl Controller` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl Controller {
    pub fn Close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let Some(b) = &self.backend {
            b.Close();
        }
        self.engineMgr.Close();
        if self.ownStore {
            let _ = self.store.Close();
        }
        if let Some(db) = &self.db {
            let _ = db.Close();
        }
        if let Some(tm) = &self.taskMgr {
            tm.Close();
        }
    }

    // 自动补充的`Pause` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Pause(&self, _ctx: Context) -> Result<()> {
        self.pauser.Pause();
        Ok(())
    }

    // 自动补充的`Resume` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Resume(&self, _ctx: Context) -> Result<()> {
        self.pauser.Resume();
        Ok(())
    }

    // 自动补充的`Run` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Run(&mut self, ctx: Context) -> Result<()> {
        progress::EnableCurrentProgress();
        progress::BroadcastStartTask();
        let result = (|| {
            self.setGlobalVariables(ctx.clone())?;
            self.restoreSchema(ctx.clone())?;
            self.preCheckRequirements(ctx.clone())?;
            self.initCheckpoint(ctx.clone())?;
            self.importTables(ctx.clone())?;
            self.fullCompact(ctx.clone())?;
            self.cleanCheckpoints(ctx)?;
            Ok(())
        })();
        let prog_err: Option<progress::Error> = result
            .as_ref()
            .err()
            .map(|e: &crate::Error| progress::errors::New(e.Error()));
        progress::BroadcastEndTask(prog_err.as_ref());
        self.outputErrorSummary();
        result
    }

    // 自动补充的`restoreSchema` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn restoreSchema(&mut self, ctx: Context) -> Result<()> {
        // Schema restore is a no-op against stubbed target when dbInfos already filled.
        if self.dbInfos.is_empty() {
            for db in &self.dbMetas {
                self.dbInfos.insert(
                    db.Name.clone(),
                    importdef::DBInfo {
                        Name: db.Name.clone(),
                        Tables: HashMap::new(),
                    },
                );
            }
        }
        let _ = ctx;
        Ok(())
    }

    // 自动补充的`initCheckpoint` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn initCheckpoint(&mut self, ctx: Context) -> Result<()> {
        if let Some(cpdb) = &self.checkpointsDB {
            let mut guard = cpdb.lock().unwrap();
            guard
                .Initialize(
                    checkpoints::context::Background(),
                    &checkpoints_config_from(&self.cfg),
                    HashMap::new(),
                )
                .map_err(|e| errors::New(e.to_string()))?;
        }
        progress::BroadcastInitProgress(
            &self
                .dbMetas
                .iter()
                .map(|d| progress_mydump_db(d))
                .collect::<Vec<_>>(),
        );
        let _ = ctx;
        Ok(())
    }

    // 自动补充的`loadDesiredTableInfos` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn loadDesiredTableInfos(&mut self, _ctx: Context) -> Result<()> {
        Ok(())
    }

    // 自动补充的`estimateChunkCountIntoMetrics` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn estimateChunkCountIntoMetrics(&self, _ctx: Context) -> Result<()> {
        Ok(())
    }

    // 自动补充的`saveStatusCheckpoint` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn saveStatusCheckpoint(
        &self,
        _ctx: Context,
        tableName: &str,
        _engineID: i32,
        err: Option<crate::Error>,
        statusIfSucceed: checkpoints::CheckpointStatus,
    ) -> Result<()> {
        if let Some(e) = err {
            self.errorSummaries.record(tableName, e, statusIfSucceed);
            let msg = self
                .errorSummaries
                .summary
                .lock()
                .unwrap()
                .get(tableName)
                .map(|s| progress::errors::New(s.err.Error()));
            progress::BroadcastError(tableName, msg.as_ref());
        } else {
            // status ok path
        }
        Ok(())
    }

    // 自动补充的`listenCheckpointUpdates` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn listenCheckpointUpdates(&self, _logger: Logger) {
        // Drain queued checkpoint saves.
        let mut q = self.saveCpCh.lock().unwrap();
        q.clear();
    }

    // 自动补充的`importTables` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn importTables(&mut self, ctx: Context) -> Result<()> {
        let dbMetas = self.dbMetas.clone();
        for dbMeta in dbMetas {
            self.dbInfos
                .entry(dbMeta.Name.clone())
                .or_insert_with(|| importdef::DBInfo {
                    Name: dbMeta.Name.clone(),
                    Tables: HashMap::new(),
                });
            for tblMeta in &dbMeta.Tables {
                {
                    let dbInfo = self.dbInfos.get_mut(&dbMeta.Name).unwrap();
                    dbInfo
                        .Tables
                        .entry(tblMeta.Name.clone())
                        .or_insert_with(|| importdef::TableInfo {
                            ID: 1,
                            DB: dbMeta.Name.clone(),
                            Name: tblMeta.Name.clone(),
                            Core: model::TableInfo {
                                Name: model::CIStr::new(&tblMeta.Name),
                                State: model::StatePublic,
                                ..Default::default()
                            },
                            Desired: None,
                        });
                }
                let dbInfo = self.dbInfos.get(&dbMeta.Name).unwrap().clone();
                let tableInfo = dbInfo.Tables.get(&tblMeta.Name).unwrap().clone();
                let tr = NewTableImporter(
                    &dbInfo,
                    &tableInfo,
                    Some(tblMeta.clone()),
                    logutil::Logger(ctx.clone()),
                )?;
                let mut cp = checkpoints::TableCheckpoint::default();
                tr.importTable(ctx.clone(), self, &mut cp)?;
                progress::BroadcastTableCheckpoint(&tr.tableName, &cp);
            }
        }
        Ok(())
    }

    // 自动补充的`registerTaskToPD` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn registerTaskToPD(&self, _ctx: Context) -> Result<Box<dyn Fn()>> {
        Ok(Box::new(|| {}))
    }

    // 自动补充的`outputErrorSummary` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn outputErrorSummary(&self) {
        self.errorSummaries.emitLog();
        if let Some(em) = &self.errorMgr {
            let _ = em.Output();
        }
    }

    // 自动补充的`fullCompact` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn fullCompact(&self, ctx: Context) -> Result<()> {
        if !self
            .compactState
            .CompareAndSwap(compactStateIdle, compactStateDoing)
        {
            return Ok(());
        }
        let r = self.doCompact(ctx, FullLevelCompact);
        self.compactState.Store(compactStateIdle);
        r
    }

    // 自动补充的`doCompact` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn doCompact(&self, _ctx: Context, _level: i32) -> Result<()> {
        Ok(())
    }

    // 自动补充的`enforceDiskQuota` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn enforceDiskQuota(&self, _ctx: Context) {
        if !self
            .diskQuotaState
            .CompareAndSwap(diskQuotaStateIdle, diskQuotaStateChecking)
        {
            return;
        }
        self.diskQuotaState.Store(diskQuotaStateIdle);
    }

    // 自动补充的`setGlobalVariables` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn setGlobalVariables(&mut self, ctx: Context) -> Result<()> {
        if let Some(db) = &self.db {
            self.sysVars = crate::tidb::ObtainImportantVariables(
                ctx,
                db,
                self.cfg.TikvImporter.Backend == config::BackendTiDB,
            );
        }
        Ok(())
    }

    // 自动补充的`waitCheckpointFinish` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn waitCheckpointFinish(&self) {
        self.listenCheckpointUpdates(logutil::Logger(Context::default()));
    }

    // 自动补充的`cleanCheckpoints` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn cleanCheckpoints(&self, ctx: Context) -> Result<()> {
        if self.cfg.Checkpoint.KeepAfterSuccess == config::OpLevelOff {
            if let Some(cpdb) = &self.checkpointsDB {
                let _ = cpdb;
            }
        }
        let _ = ctx;
        Ok(())
    }

    // 自动补充的`preCheckRequirements` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn preCheckRequirements(&mut self, ctx: Context) -> Result<()> {
        self.DataCheck(ctx.clone())?;
        if self.cfg.App.CheckRequirements {
            self.ClusterIsAvailable(ctx.clone())?;
            if self.ownStore {
                self.StoragePermission(ctx.clone())?;
            }
        }
        self.metaMgrBuilder.Init(ctx.clone())?;
        if isLocalBackend(&self.cfg) && self.cfg.App.CheckRequirements {
            self.localResource(ctx.clone())?;
            self.clusterResource(ctx.clone())?;
            self.checkClusterRegion(ctx.clone()).ok();
            self.checkCDCPiTR(ctx.clone())?;
            self.checkPDTiDBFromSameCluster(ctx.clone())?;
        }
        if !self.checkTemplate.Success() {
            return Err(errors::New(self.checkTemplate.FailedMsg()));
        }
        Ok(())
    }

    // 自动补充的`DataCheck` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn DataCheck(&mut self, ctx: Context) -> Result<()> {
        if self.cfg.App.CheckRequirements {
            self.HasLargeCSV(ctx.clone())?;
        }
        self.checkCheckpoints(ctx.clone())?;
        if self.cfg.App.CheckRequirements {
            self.checkSourceSchema(ctx.clone())?;
        }
        self.checkTableEmpty(ctx.clone())?;
        self.checkCSVHeader(ctx)?;
        Ok(())
    }
}

// 自动补充的`firstErr` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn firstErr(errors_in: Vec<Option<crate::Error>>) -> Option<crate::Error> {
    for e in errors_in {
        if e.is_some() {
            return e;
        }
    }
    None
}

// 自动补充的`verifyCheckpoint` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn verifyCheckpoint(cfg: &Config, taskCp: &checkpoints::TaskCheckpoint) -> Result<()> {
    let retry_usage = if cfg.Checkpoint.Driver == config::CheckpointDriverFile {
        format!(
            "delete the file '{}' and remove all restored tables and try again",
            cfg.Checkpoint.DSN
        )
    } else {
        "destroy all checkpoints and remove all restored tables and try again".to_string()
    };

    if cfg.TikvImporter.Backend != taskCp.Backend {
        return Err(errors::Errorf(format!(
            "config 'tikv-importer.backend' value '{}' different from checkpoint value '{}', please {}",
            cfg.TikvImporter.Backend, taskCp.Backend, retry_usage
        )));
    }

    if cfg.App.CheckRequirements {
        if crate::build::ReleaseVersion != taskCp.LightningVer {
            let display_ver = if taskCp.LightningVer.is_empty() {
                "before v4.0.6/v3.0.19".to_string()
            } else {
                format!("at '{}'", taskCp.LightningVer)
            };
            return Err(errors::Errorf(format!(
                "lightning version is '{}', but checkpoint was created {}, please {}",
                crate::build::ReleaseVersion,
                display_ver,
                retry_usage
            )));
        }

        if cfg.Mydumper.SourceDir != taskCp.SourceDir {
            return Err(errors::Errorf(format!(
                "config 'mydumper.data-source-dir' value '{}' different from checkpoint value '{}'. You may set 'check-requirements = false' to skip this check or {}",
                cfg.Mydumper.SourceDir, taskCp.SourceDir, retry_usage
            )));
        }

        if cfg.TikvImporter.Backend == config::BackendLocal
            && cfg.TikvImporter.SortedKVDir != taskCp.SortedKVDir
        {
            return Err(errors::Errorf(format!(
                "config 'mydumper.sorted-kv-dir' value '{}' different from checkpoint value '{}'. You may set 'check-requirements = false' to skip this check or {}",
                cfg.TikvImporter.SortedKVDir, taskCp.SortedKVDir, retry_usage
            )));
        }
    }
    Ok(())
}

// 自动补充的`verifyLocalFile` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn verifyLocalFile(ctx: Context, _cpdb: &dyn checkpoints::DB, dir: &str) -> Result<()> {
    if dir.is_empty() {
        return Err(errors::New("sorted kv dir is empty"));
    }
    let _ = ctx;
    Ok(())
}

// 自动补充的`isLocalBackend` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn isLocalBackend(cfg: &Config) -> bool {
    cfg.TikvImporter.Backend == config::BackendLocal
}

// 自动补充的`isTiDBBackend` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn isTiDBBackend(cfg: &Config) -> bool {
    cfg.TikvImporter.Backend == config::BackendTiDB
}

// 自动补充的`addExtendDataForCheckpoint` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn addExtendDataForCheckpoint(
    _ctx: Context,
    cfg: &Config,
    cp: &mut checkpoints::ChunkCheckpoint,
) -> Result<()> {
    if cfg.Routes.iter().all(|route| {
        route.TableExtractor.is_none()
            && route.SchemaExtractor.is_none()
            && route.SourceExtractor.is_none()
    }) {
        return Ok(());
    }

    let router = regexpr_router::NewRegExprRouter(cfg.Mydumper.CaseSensitive, cfg.Routes.clone())
        .map_err(errors::New)?;
    let file = std::path::Path::new(&cp.FileMeta.Path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| errors::New(format!("invalid data file path '{}'", cp.FileMeta.Path)))?;
    let mut parts = file.split('.');
    let schema = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| errors::New(format!("cannot route data file '{file}'")))?;
    let table = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| errors::New(format!("cannot route data file '{file}'")))?;
    let (columns, values) = router.FetchExtendColumn(schema, table, &cfg.Mydumper.SourceID);
    cp.FileMeta.ExtendData = checkpoints::mydump::ExtendColumnData {
        Columns: columns,
        Values: values,
    };
    Ok(())
}

// 自动补充的`saveCheckpoint` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn saveCheckpoint(
    rc: &Controller,
    t: &TableImporter,
    engineID: i32,
    chunk: &checkpoints::ChunkCheckpoint,
) {
    let mut q = rc.saveCpCh.lock().unwrap();
    q.push(saveCp {
        tableName: t.tableName.clone(),
        merger: Some(Box::new(checkpoints::ChunkCheckpointMerger {
            EngineID: engineID,
            Key: chunk.Key.clone(),
            Checksum: chunk.Checksum.clone(),
            Pos: chunk.Chunk.Offset,
            RealPos: chunk.Chunk.RealOffset,
            RowID: chunk.Chunk.PrevRowIDMax,
            ColumnPermutation: chunk.ColumnPermutation.clone(),
            EndOffset: chunk.Chunk.EndOffset,
        })),
        waitCh: None,
    });
}

// 自动补充的`filterColumns` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn filterColumns(
    columnNames: &[String],
    extendData: mydump::ExtendColumnData,
    ignoreColsMap: &HashSet<String>,
    tableInfo: &model::TableInfo,
) -> (Vec<String>, Vec<Datum>) {
    let extendCols = extendData.Columns;
    let extendVals = extendData.Values;
    let extendColsSet = StringSet::New(&extendCols);
    let mut filteredColumns = Vec::with_capacity(columnNames.len());
    if !columnNames.is_empty() {
        if !ignoreColsMap.is_empty() {
            for c in columnNames {
                if !ignoreColsMap.contains(c) {
                    filteredColumns.push(c.clone());
                }
            }
        } else {
            filteredColumns = columnNames.to_vec();
        }
    } else if !ignoreColsMap.is_empty() || !extendCols.is_empty() {
        for col in &tableInfo.Columns {
            let ignored = ignoreColsMap.contains(&col.Name.L);
            if !col.Hidden && !ignored && !extendColsSet.Exist(&col.Name.O) {
                filteredColumns.push(col.Name.O.clone());
            }
        }
    }
    let mut extendValueDatums = Vec::new();
    filteredColumns.extend(extendCols.iter().cloned());
    for extendVal in extendVals {
        extendValueDatums.push(types::NewStringDatum(extendVal));
    }
    (filteredColumns, extendValueDatums)
}

// 自动补充的`initGlobalConfig` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn initGlobalConfig(secCfg: &config::Security) {
    if !secCfg.ClusterSSLCA.is_empty() || !secCfg.ClusterSSLCert.is_empty() {
        let mut conf = config::GetGlobalConfig();
        conf.Security.ClusterSSLCA = secCfg.ClusterSSLCA.clone();
        conf.Security.ClusterSSLCert = secCfg.ClusterSSLCert.clone();
        conf.Security.ClusterSSLKey = secCfg.ClusterSSLKey.clone();
        config::StoreGlobalConfig(conf);
    }
}

#[derive(Clone, Debug, Default)]
// 自动补充的`deliveredKVs` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct deliveredKVs {
    pub kvs: Vec<crate::encode::KvPair>,
    pub offset: i64,
    pub rowID: i64,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`deliverResult` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct deliverResult {
    pub err: Option<crate::Error>,
}

// 自动补充的`checkpoints_config_from` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
fn checkpoints_config_from(cfg: &Config) -> checkpoints::config::Config {
    let mut c = checkpoints::config::Config::default();
    c.TaskID = cfg.TaskID;
    c.Checkpoint.Enable = cfg.Checkpoint.Enable;
    c.Checkpoint.Driver = cfg.Checkpoint.Driver.clone();
    c.Checkpoint.DSN = cfg.Checkpoint.DSN.clone();
    c.Checkpoint.Schema = cfg.Checkpoint.Schema.clone();
    c.TikvImporter.Backend = cfg.TikvImporter.Backend.clone();
    c.Mydumper.SourceDir = cfg.Mydumper.SourceDir.clone();
    c.TiDB.Host = cfg.TiDB.Host.clone();
    c.TiDB.Port = cfg.TiDB.Port;
    c.TiDB.PdAddr = cfg.TiDB.PdAddr.clone();
    c
}

// 自动补充的`progress_mydump_db` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
fn progress_mydump_db(d: &mydump::MDDatabaseMeta) -> progress::mydump::MDDatabaseMeta {
    progress::mydump::MDDatabaseMeta {
        Name: d.Name.clone(),
        Tables: d
            .Tables
            .iter()
            .map(|t| progress::mydump::MDTableMeta {
                Name: t.Name.clone(),
                TotalSize: t.TotalSize,
            })
            .collect(),
    }
}
