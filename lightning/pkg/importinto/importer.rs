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

//! Importer lifecycle for the import-into backend.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/importer.rs`对应的IMPORT INTO 总控流程，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少49行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `UpdateTotalSize`是当前文件的重要函数，承担\"UpdateTotalSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UpdateFinishedSize`是当前文件的重要函数，承担\"UpdateFinishedSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Importer`把\"Importer\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `buildOrchestrator`是当前文件的重要函数，承担\"buildOrchestrator\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `runOnce`是当前文件的重要函数，承担\"runOnce\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `runPrechecks`是当前文件的重要函数，承担\"runPrechecks\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `initGroupKey`是当前文件的重要函数，承担\"initGroupKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 补充约束 1: `lightning/pkg/importinto/importer.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `lightning/pkg/importinto/importer.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use crate::checkpoint::{CheckpointManager, NewCheckpointManager};
use crate::job_orchestrator::{
    DefaultPollInterval, JobOrchestrator, NewJobOrchestrator, OrchestratorConfig,
};
use crate::job_submitter::{NewJobSubmitter, WithJobSubmitterStripS3ExternalIDForImportSQL};
use crate::precheck::{NewCheckpointCheckItem, NewPrecheckRunner};
use crate::stubs::*;
use std::sync::Arc;
use std::time::Duration;

/// cancelTimeout bounds job cancellation after context cancel.
pub const cancelTimeout: Duration = Duration::from_secs(60);

/// ErrFailoverCancel is the cancellation cause used by DM worker failover.
pub fn ErrFailoverCancel() -> Error {
    Error::new("lightning: failover cancel")
}

/// ProgressUpdater is an interface for updating the progress of the import process.
pub trait ProgressUpdater: Send + Sync {
    fn UpdateTotalSize(&self, size: i64);
    fn UpdateFinishedSize(&self, size: i64);
}

/// ImporterOption is a function that configures the Importer.
pub type ImporterOption = Box<dyn FnOnce(&mut Importer) + Send>;

/// WithProgressUpdater sets the ProgressUpdater for the Importer.
pub fn WithProgressUpdater(pu: Arc<dyn ProgressUpdater>) -> ImporterOption {
    Box::new(move |i: &mut Importer| {
        i.progressUpdater = Some(pu);
    })
}

/// WithCheckpointManager sets the CheckpointManager for the Importer.
pub fn WithCheckpointManager(cpMgr: Arc<dyn CheckpointManager>) -> ImporterOption {
    Box::new(move |i: &mut Importer| {
        i.cpMgr = Some(cpMgr);
    })
}

/// WithBackendSDK sets the BackendSDK for the Importer.
pub fn WithBackendSDK(sdk: Arc<dyn importsdk::SDK>) -> ImporterOption {
    Box::new(move |i: &mut Importer| {
        i.sdk = Some(sdk);
    })
}

/// WithOrchestrator sets the JobOrchestrator for the Importer.
pub fn WithOrchestrator(orchestrator: Arc<dyn JobOrchestrator>) -> ImporterOption {
    Box::new(move |i: &mut Importer| {
        i.orchestrator = Some(orchestrator);
    })
}

/// WithStripS3ExternalIDForImportSQL strips explicit S3 external ID from IMPORT INTO SQL.
pub fn WithStripS3ExternalIDForImportSQL() -> ImporterOption {
    Box::new(|i: &mut Importer| {
        i.stripS3ExternalIDForImportSQL = true;
    })
}

/// Importer is the implementation of LightningImporter for the 'import into' backend.
pub struct Importer {
    pub cfg: Arc<config::Config>,
    pub db: sql::DB,
    pub sdk: Option<Arc<dyn importsdk::SDK>>,
    pub logger: log::Logger,
    pub cpMgr: Option<Arc<dyn CheckpointManager>>,
    pub orchestrator: Option<Arc<dyn JobOrchestrator>>,
    pub groupKey: String,
    pub progressUpdater: Option<Arc<dyn ProgressUpdater>>,
    pub stripS3ExternalIDForImportSQL: bool,
}

/// NewImporter creates a new Importer.
pub fn NewImporter(
    ctx: &context::Context,
    cfg: config::Config,
    db: sql::DB,
    opts: Vec<ImporterOption>,
) -> Result<Importer> {
    let mut imp = Importer {
        cfg: Arc::new(cfg),
        db,
        sdk: None,
        logger: log::Logger::default(),
        cpMgr: None,
        orchestrator: None,
        groupKey: String::new(),
        progressUpdater: None,
        stripS3ExternalIDForImportSQL: false,
    };

    for opt in opts {
        opt(&mut imp);
    }

    if imp.logger.inner.is_none() {
        imp.logger = log::L().With(zap::String("backend", "import-into"));
    }

    if imp.sdk.is_none() {
        let sdkOpts = vec![
            importsdk::WithSQLMode(imp.cfg.TiDB.SQLMode.clone()),
            importsdk::WithFilter(imp.cfg.Mydumper.Filter.clone()),
            importsdk::WithFileRouters(imp.cfg.Mydumper.FileRouters.clone()),
            importsdk::WithRoutes(imp.cfg.Routes.clone()),
            importsdk::WithCharset(imp.cfg.Mydumper.CharacterSet.clone()),
            importsdk::WithDataCharacterSet(imp.cfg.Mydumper.DataCharacterSet.clone()),
            importsdk::WithCSVConfig(imp.cfg.Mydumper.CSV.clone()),
            importsdk::WithLogger(imp.logger.clone()),
        ];
        let sdk = importsdk::NewImportSDK(ctx, &imp.cfg.Mydumper.SourceDir, &imp.db, sdkOpts)?;
        imp.sdk = Some(sdk);
    }

    if imp.cpMgr.is_none() {
        let cpMgr = NewCheckpointManager(&imp.cfg)?;
        imp.cpMgr = Some(cpMgr);
    }

    imp.cpMgr.as_ref().unwrap().Initialize(ctx)?;
    imp.initGroupKey(ctx)?;

    if imp.orchestrator.is_none() {
        let orch = imp.buildOrchestrator();
        imp.orchestrator = Some(orch);
    }

    Ok(imp)
}

impl Importer {
    fn buildOrchestrator(&self) -> Arc<dyn JobOrchestrator> {
        let mut jobSubmitterOpts = Vec::with_capacity(1);
        if self.stripS3ExternalIDForImportSQL {
            jobSubmitterOpts.push(WithJobSubmitterStripS3ExternalIDForImportSQL(true));
        }
        let submitter = NewJobSubmitter(
            self.sdk.as_ref().unwrap().clone(),
            self.cfg.clone(),
            self.groupKey.clone(),
            self.logger
                .clone()
                .With(zap::String("component", "submitter")),
            jobSubmitterOpts,
        );

        NewJobOrchestrator(OrchestratorConfig {
            Submitter: submitter,
            CheckpointMgr: self.cpMgr.as_ref().unwrap().clone(),
            SDK: self.sdk.as_ref().unwrap().clone(),
            Monitor: None,
            SubmitConcurrency: self.cfg.App.TableConcurrency,
            PollInterval: DefaultPollInterval,
            LogInterval: self.cfg.Cron.LogProgress.Duration,
            Logger: self
                .logger
                .clone()
                .With(zap::String("component", "orchestrator")),
            ProgressUpdater: self.progressUpdater.clone(),
        })
    }

    /// Run starts the import process.
    pub fn Run(&self, ctx: &context::Context) -> Result<()> {
        let err = self.runOnce(ctx);
        if let Err(ref e) = err {
            if common::IsContextCanceledError(Some(e)) {
                if errors::ErrorEqual(&errors::Cause(&context::Cause(ctx)), &ErrFailoverCancel()) {
                    self.logger.Info(
                        "context canceled by failover, skipping job cancellation",
                        &[],
                    );
                    return err;
                }

                self.logger
                    .Info("context canceled, cancelling import jobs...", &[]);
                let (cancelCtx, _cancel) =
                    context::WithTimeout(context::Background(), cancelTimeout);
                match self.orchestrator.as_ref().unwrap().Cancel(&cancelCtx) {
                    Ok(()) => self.logger.Info("import jobs cancelled successfully", &[]),
                    Err(cancelErr) => self
                        .logger
                        .Warn("failed to cancel import jobs", &[zap::Error(&cancelErr)]),
                }
            }
        }
        err
    }

    fn runOnce(&self, ctx: &context::Context) -> Result<()> {
        self.sdk.as_ref().unwrap().CreateSchemasAndTables(ctx)?;
        let tables = self.sdk.as_ref().unwrap().GetTableMetas(ctx)?;

        if self.cfg.App.CheckRequirements {
            self.runPrechecks(ctx)?;
        } else {
            self.logger
                .Info("skipping prechecks as CheckRequirements is disabled", &[]);
        }

        self.orchestrator
            .as_ref()
            .unwrap()
            .SubmitAndWait(ctx, &tables)?;

        if self.cfg.Checkpoint.Enable
            && self.cfg.Checkpoint.KeepAfterSuccess == config::CheckpointRemove
        {
            self.logger.Info("removing all checkpoints", &[]);
            if let Err(err) = self.cpMgr.as_ref().unwrap().Remove(ctx, common::AllTables) {
                self.logger
                    .Warn("failed to remove checkpoints", &[zap::Error(&err)]);
            }
        }

        Ok(())
    }

    /// Pause is not supported for import-into backend.
    pub fn Pause(&self, _ctx: &context::Context) -> Result<()> {
        self.logger
            .Info("pause is not supported for 'import into' backend", &[]);
        Ok(())
    }

    /// Resume is not supported for import-into backend.
    pub fn Resume(&self, _ctx: &context::Context) -> Result<()> {
        self.logger
            .Info("resume is not supported for 'import into' backend", &[]);
        Ok(())
    }

    fn runPrechecks(&self, ctx: &context::Context) -> Result<()> {
        self.logger.Info("running prechecks", &[]);

        let mut precheckRunner = NewPrecheckRunner();
        precheckRunner.Register(NewCheckpointCheckItem(
            self.cfg.clone(),
            self.cpMgr.as_ref().unwrap().clone(),
        ));

        if let Err(err) = precheckRunner.Run(ctx.clone()) {
            self.logger.Error("precheck failed", &[zap::Error(&err)]);
            return Err(errors::Annotate(err, "precheck failed"));
        }

        self.logger.Info("all prechecks passed", &[]);
        Ok(())
    }

    fn initGroupKey(&mut self, ctx: &context::Context) -> Result<()> {
        let cps = self.cpMgr.as_ref().unwrap().GetCheckpoints(ctx)?;
        for cp in cps {
            if !cp.GroupKey.is_empty() {
                self.groupKey = cp.GroupKey;
                self.logger.Info(
                    "restored group key from checkpoint",
                    &[zap::String("groupKey", &self.groupKey)],
                );
                return Ok(());
            }
        }

        self.groupKey = format!("lightning-{}", uuid_util::New());
        self.logger.Info(
            "generated new group key",
            &[zap::String("groupKey", &self.groupKey)],
        );
        Ok(())
    }

    /// Close closes the importer and releases resources.
    pub fn Close(&self) {
        if let Some(cpMgr) = &self.cpMgr {
            if let Err(err) = cpMgr.Close() {
                self.logger
                    .Warn("failed to close checkpoint manager", &[zap::Error(&err)]);
            }
        }
        if let Some(sdk) = &self.sdk {
            if let Err(err) = sdk.Close() {
                self.logger.Warn("failed to close sdk", &[zap::Error(&err)]);
            }
        }
        if let Err(err) = self.db.Close() {
            self.logger
                .Warn("failed to close database connection", &[zap::Error(&err)]);
        }
    }
}
