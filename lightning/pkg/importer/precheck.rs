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

//! Precheck item builder matching Go `precheck.go`.
//!
//! 该模块负责把 importer 的配置、源数据元信息、目标端连接与 checkpoint 状态，
//! 组装成一组可执行的 precheck checker。
//! 它本身并不实现具体检查逻辑，而是承担接线层职责：
//! 创建 `PreImportInfoGetter`，
//! 准备 PD 地址获取器，
//! 再按 `CheckItemID` 分发到 `precheck_impl.rs` 中的具体实现。
//! 因而这里最需要说明的是依赖流向和回退策略，
//! 而不是每个检查项的业务细节。

use crate::config::Config;
use crate::context::{self, Context};
use crate::errors::{self, Result};
use crate::get_pre_info::{NewPreImportInfoGetter, NewTargetInfoGetterImpl, PreImportInfoGetter};
use crate::mydump;
use crate::pdhttp;
use crate::precheck_impl::*;
use crate::sql::DB;
use crate::tidb::DBFromConfig;
use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_importer_opts as ropts;
use astersql_lightning_pkg_precheck as precheck;
use std::sync::Arc;

pub type precheckContextKey = String;
/// `Context` 中保存任务管理器的键名。
pub const taskManagerKey: &str = "PRECHECK/TASK_MANAGER";

/// 往预检上下文里注入额外对象。
/// 这里沿用 Go 的 `context.WithValue` 风格，方便调用方复用既有接线方式。
pub fn WithPrecheckKey(
    ctx: Context,
    key: precheckContextKey,
    val: Arc<dyn std::any::Any + Send + Sync>,
) -> Context {
    context::WithValue(ctx, key, val)
}

pub struct PrecheckItemBuilder {
    pub keyspaceName: String,
    /// 导入配置决定启用哪些 checker 以及部分阈值。
    pub cfg: Config,
    /// 源数据的 mydump 元数据视图，供多个检查项共享读取。
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    /// 预检信息获取器封装了对目标端和源数据的统一查询入口。
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    /// checkpoint 连接可选，是因为某些 slim 路径下只需要空实现。
    pub checkpointsDB: Option<Arc<dyn checkpoints::DB>>,
    /// PD 地址获取器既支持实时请求 leader，也支持直接回退配置值。
    pub pdAddrsGetter: Arc<dyn Fn(Context) -> Vec<String> + Send + Sync>,
    /// 目标端 DB 句柄仅在部分检查项需要时使用。
    pub targetDB: Option<DB>,
}

/// 从完整配置接线出一个 `PrecheckItemBuilder`。
/// 返回值中的第二项表示 loader 虽然构造成功，但曾产生可继续运行的告警错误。
pub fn NewPrecheckItemBuilderFromConfig(
    ctx: Context,
    cfg: &Config,
    pdHTTPCli: Option<pdhttp::Client>,
    opts: Vec<ropts::PrecheckItemBuilderOption>,
) -> Result<(PrecheckItemBuilder, Option<crate::Error>)> {
    let mut gerr = None;
    let mut builderCfg = ropts::PrecheckItemBuilderConfig::default();
    // 先把调用方传入的 builder option 归并成一份配置快照。
    // 这样后续接线阶段只消费最终配置，不再关心 option 来源。
    for o in opts {
        o(&mut builderCfg);
    }
    // Apply the caller's options before appending Lightning's concurrency
    // override, matching the order in Go.  The local mydump boundary currently
    // exposes only scan-file concurrency, so translate that shared field.
    let mut supplied_md_cfg = ropts::MDLoaderSetupConfig::default();
    for option in &builderCfg.MDLoaderSetupOptions {
        option(&mut supplied_md_cfg);
    }
    let mut md_opts = Vec::with_capacity(2);
    if supplied_md_cfg.scan_file_concurrency > 0 {
        md_opts.push(mydump::WithScanFileConcurrency(
            supplied_md_cfg.scan_file_concurrency,
        ));
    }
    md_opts.push(mydump::WithScanFileConcurrency(
        (cfg.App.RegionConcurrency.max(1) as usize) * 2,
    ));

    let targetDB = DBFromConfig(ctx.clone(), &cfg.TiDB).map_err(errors::Trace)?;
    // 目标端 getter 负责把 SQL/PD HTTP 查询统一封装给预检逻辑消费。
    let targetInfoGetter =
        NewTargetInfoGetterImpl(cfg, targetDB.clone(), pdHTTPCli.clone()).map_err(errors::Trace)?;
    let loader = match mydump::NewLoader(ctx.clone(), mydump::NewLoaderCfg(cfg), md_opts) {
        Ok(mdl) => mdl,
        Err((Some(mdl), err)) => {
            // 有些错误允许在保留部分 loader 结果时继续推进，交给调用方自行决定是否终止。
            gerr = Some(err);
            mdl
        }
        Err((None, err)) => return Err(errors::Trace(err)),
    };
    let dbMetas = loader.GetDatabases();
    let srcStorage = loader.GetStore();
    let preInfoGetter = NewPreImportInfoGetter(
        cfg,
        dbMetas.clone(),
        srcStorage,
        targetInfoGetter,
        None,
        None,
        builderCfg.PreInfoGetterOptions,
    )
    .map_err(errors::Trace)?;
    // The checkpoints crate owns an equivalent slim config type.  Translate
    // the fields observed by OpenCheckpointsDB instead of silently replacing
    // every configured backend with the null implementation.
    let mut checkpoint_cfg = checkpoints::config::Config::default();
    checkpoint_cfg.TaskID = cfg.TaskID;
    checkpoint_cfg.Checkpoint.Enable = cfg.Checkpoint.Enable;
    checkpoint_cfg.Checkpoint.Driver = cfg.Checkpoint.Driver.clone();
    checkpoint_cfg.Checkpoint.DSN = cfg.Checkpoint.DSN.clone();
    checkpoint_cfg.Checkpoint.Schema = cfg.Checkpoint.Schema.clone();
    checkpoint_cfg.Mydumper.SourceDir = cfg.Mydumper.SourceDir.clone();
    checkpoint_cfg.TikvImporter.Backend = cfg.TikvImporter.Backend.clone();
    checkpoint_cfg.TikvImporter.Addr = cfg.TikvImporter.Addr.clone();
    checkpoint_cfg.TikvImporter.SortedKVDir = cfg.TikvImporter.SortedKVDir.clone();
    checkpoint_cfg.TikvImporter.AddIndexBySQL = cfg.TikvImporter.AddIndexBySQL;
    checkpoint_cfg.TiDB.Host = cfg.TiDB.Host.clone();
    checkpoint_cfg.TiDB.Port = cfg.TiDB.Port;
    checkpoint_cfg.TiDB.PdAddr = cfg.TiDB.PdAddr.clone();
    let cpdb: Arc<dyn checkpoints::DB> = Arc::from(
        // The checkpoints slim crate's context is a marker type; its factory
        // does not currently expose cancellation/value propagation.
        checkpoints::OpenCheckpointsDB(checkpoints::context::Background(), &checkpoint_cfg)
            .map_err(|err| errors::Errorf(err.to_string()))?,
    );
    Ok((
        NewPrecheckItemBuilder(
            cfg,
            dbMetas,
            preInfoGetter,
            Some(cpdb),
            pdHTTPCli,
            Some(targetDB),
        ),
        gerr,
    ))
}

/// 直接从现成依赖构造 builder。
/// 这条路径主要被测试和更细粒度的接线代码复用。
pub fn NewPrecheckItemBuilder(
    cfg: &Config,
    dbMetas: Vec<mydump::MDDatabaseMeta>,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    checkpointsDB: Option<Arc<dyn checkpoints::DB>>,
    pdHTTPCli: Option<pdhttp::Client>,
    targetDB: Option<DB>,
) -> PrecheckItemBuilder {
    NewPrecheckItemBuilderWithKeyspaceName(
        cfg,
        dbMetas,
        preInfoGetter,
        checkpointsDB,
        pdHTTPCli,
        targetDB,
        &cfg.TikvImporter.KeyspaceName,
    )
}

pub fn NewPrecheckItemBuilderWithKeyspaceName(
    cfg: &Config,
    dbMetas: Vec<mydump::MDDatabaseMeta>,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    checkpointsDB: Option<Arc<dyn checkpoints::DB>>,
    pdHTTPCli: Option<pdhttp::Client>,
    targetDB: Option<DB>,
    keyspaceName: &str,
) -> PrecheckItemBuilder {
    let fallback = cfg.TiDB.PdAddr.clone();
    let pdAddrsGetter: Arc<dyn Fn(Context) -> Vec<String> + Send + Sync> =
        if let Some(cli) = pdHTTPCli {
            // 有 PD HTTP client 时优先读当前 leader 地址，失败再回退静态配置。
            Arc::new(move |ctx| match cli.GetLeader(ctx) {
                Ok(leader) if !leader.GetClientUrls().is_empty() => leader.GetClientUrls(),
                _ => vec![fallback.clone()],
            })
        } else {
            // 没有 client 时退化成恒定返回配置值，保证 checker 仍可运行。
            Arc::new(move |_| vec![fallback.clone()])
        };
    PrecheckItemBuilder {
        keyspaceName: keyspaceName.into(),
        cfg: cfg.clone(),
        dbMetas,
        preInfoGetter,
        checkpointsDB,
        pdAddrsGetter,
        targetDB,
    }
}

impl PrecheckItemBuilder {
    /// 根据 `CheckItemID` 构造对应的 checker 实例。
    /// 这里是整条预检链路的分发中心，负责把共享依赖传递给具体检查项。
    pub fn BuildPrecheckItem(
        &self,
        checkID: precheck::CheckItemID,
    ) -> Result<Box<dyn precheck::Checker>> {
        match checkID {
            id if id == precheck::CheckLargeDataFile => {
                // 大文件检查只依赖配置和源文件元数据。
                Ok(NewLargeFileCheckItem(&self.cfg, &self.dbMetas))
            }
            id if id == precheck::CheckSourcePermission => {
                Ok(NewStoragePermissionCheckItem(&self.cfg))
            }
            id if id == precheck::CheckTargetTableEmpty => Ok(NewTableEmptyCheckItem(
                &self.cfg,
                self.preInfoGetter.clone(),
                &self.dbMetas,
                self.checkpointsDB.clone(),
            )),
            id if id == precheck::CheckSourceSchemaValid => Ok(NewSchemaCheckItem(
                &self.cfg,
                self.preInfoGetter.clone(),
                &self.dbMetas,
                self.checkpointsDB.clone(),
            )),
            id if id == precheck::CheckCheckpoints => Ok(NewCheckpointCheckItem(
                &self.cfg,
                self.preInfoGetter.clone(),
                &self.dbMetas,
                self.checkpointsDB.clone(),
            )),
            id if id == precheck::CheckCSVHeader => Ok(NewCSVHeaderCheckItem(
                &self.cfg,
                self.preInfoGetter.clone(),
                &self.dbMetas,
            )),
            id if id == precheck::CheckTargetClusterSize => {
                // 集群容量检查只需要 pre-info getter 即可。
                Ok(NewClusterResourceCheckItem(self.preInfoGetter.clone()))
            }
            id if id == precheck::CheckTargetClusterEmptyRegion => Ok(NewEmptyRegionCheckItem(
                self.preInfoGetter.clone(),
                &self.dbMetas,
            )),
            id if id == precheck::CheckTargetClusterRegionDist => Ok(
                NewRegionDistributionCheckItem(self.preInfoGetter.clone(), &self.dbMetas),
            ),
            id if id == precheck::CheckTargetClusterVersion => Ok(NewClusterVersionCheckItem(
                self.preInfoGetter.clone(),
                &self.dbMetas,
            )),
            id if id == precheck::CheckLocalDiskPlacement => {
                Ok(NewLocalDiskPlacementCheckItem(&self.cfg))
            }
            id if id == precheck::CheckLocalTempKVDir => Ok(NewLocalTempKVDirCheckItem(
                &self.cfg,
                self.preInfoGetter.clone(),
                &self.dbMetas,
            )),
            id if id == precheck::CheckTargetUsingCDCPITR => {
                Ok(NewCDCPITRCheckItemWithKeyspaceName(
                    &self.cfg,
                    self.pdAddrsGetter.clone(),
                    &self.keyspaceName,
                ))
            }
            id if id == precheck::CheckPDTiDBFromSameCluster => {
                // 这类检查需要同时触达目标 DB 与 PD 地址源。
                Ok(NewPDTiDBFromSameClusterCheckItem(
                    self.targetDB.clone(),
                    self.pdAddrsGetter.clone(),
                ))
            }
            other => Err(errors::Errorf(format!("unsupported check item: {other}"))),
        }
    }

    /// 暴露共享的 pre-info getter，供调用方在 builder 外复用。
    /// 返回克隆句柄而不是借用，避免生命周期把 builder 绑死。
    pub fn GetPreInfoGetter(&self) -> Arc<dyn PreImportInfoGetter> {
        self.preInfoGetter.clone()
    }
}
