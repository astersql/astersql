// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Precheck dispatch methods on Controller matching Go `check_info.go`.
//!
//! 中文概览：这个文件负责把 importer 的各类预检查项挂到 `Controller` 上，
//! 让调用方可以按“本地资源、目标集群、目标表、源文件、checkpoint 状态”等维度逐项触发检查。
//! 它本身并不实现每条检查规则，而是把规则 ID 分发给 `precheckItemBuilder`。
//! 因而这里更像调度层，而不是具体校验逻辑本体。
//! 常量部分给出了 CSV 采样、region 计数和分布阈值等默认标准。
//! 这些值不是随手挑的魔法数，而是上层预检查判定严重级别时会复用的边界。
//! `doPreCheckOnItem` 是核心入口：构造检查器、执行检查，并把结果回写到 `checkTemplate`。
//! 其余方法则按语义把检查项包装成更可读的控制器方法。
//! 例如 `clusterResource`、`checkClusterRegion` 和 `localResource` 分别对应不同资源面。
//! 其中 `checkClusterRegion` 还保留了 Go 版“任务已启动时跳过 region 检查”的短路语义。
//! 这样可以避免恢复或续跑任务在不合适的时机重复做昂贵检查。
//! 多个方法在 TiDB backend 下直接返回 `Ok(())`，
//! 说明这些检查只对 local/importer 路径有意义，不能机械地强加给所有后端。
//! 阅读本文件时，可把它理解成“预检查开关板”：真正的规则在别处，这里负责接线和跳过逻辑。
//! 因为只补注释，本次不会调整任何检查项编号、阈值和短路条件。

use crate::context::Context;
use crate::errors::{self, Result};
use crate::import::Controller;
use crate::meta_manager::taskMetaStatusInitial;
use crate::objstore;
use crate::precheck as local_precheck;
use astersql_lightning_pkg_precheck as precheck;

pub const DEFAULT_CSV_SIZE: u64 = 10 * 1024 * 1024 * 1024;
// 采样数据量与 region 阈值常量被集中放在这里，方便与 Go 版默认值逐项核对。
pub const MAX_SAMPLE_DATA_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_SAMPLE_ROW_COUNT: u64 = 10 * 1024;

pub const WARN_EMPTY_REGION_CNT_PER_STORE: i64 = 500;
pub const ERROR_EMPTY_REGION_CNT_PER_STORE: i64 = 1000;
pub const WARN_REGION_CNT_MIN_MAX_RATIO: f64 = 0.75;
pub const ERROR_REGION_CNT_MIN_MAX_RATIO: f64 = 0.5;
pub const CHECK_REGION_CNT_RATIO_THRESHOLD: i64 = 1000;

pub(crate) fn toPrecheckContext(ctx: Context) -> precheck::context::Context {
    precheck::context::Context {
        str_values: ctx.str_values,
        cancelled: ctx.cancelled,
    }
}

impl Controller {
    pub fn isSourceInLocal(&self) -> bool {
        // 本地源路径会决定是否需要额外检查磁盘布局与本地临时目录。
        self.store.URI().starts_with(objstore::LocalURIPrefix)
    }

    pub fn doPreCheckOnItem(
        &mut self,
        ctx: Context,
        checkItemID: precheck::CheckItemID,
    ) -> Result<()> {
        // 这里统一封装“构造检查器 -> 执行 -> 收集结果”的公共骨架，
        // 避免每个上层方法各自重复拼装 builder 和 template 回写逻辑。
        let builder = self
            .precheckItemBuilder
            .as_mut()
            .ok_or_else(|| errors::New("precheckItemBuilder is nil"))?;
        builder.keyspaceName = self.keyspaceName.clone();
        let mut theChecker = self
            .precheckItemBuilder
            .as_ref()
            .ok_or_else(|| errors::New("precheckItemBuilder is nil"))?
            .BuildPrecheckItem(checkItemID)?;
        let result = theChecker
            .Check(toPrecheckContext(ctx))
            .map_err(|e| errors::New(e.to_string()))?;
        if let Some(result) = result {
            self.checkTemplate
                .Collect(result.Severity, result.Passed, result.Message);
        }
        Ok(())
    }

    pub fn clusterResource(&mut self, ctx: Context) -> Result<()> {
        // 当任务管理器存在时，把它挂进 precheck context，
        // 这样集群资源检查可以感知任务级排它信息。
        let checkCtx = if let Some(task_mgr) = self.taskMgr.as_ref() {
            local_precheck::WithPrecheckKey(
                ctx,
                local_precheck::taskManagerKey.to_string(),
                std::sync::Arc::new(task_mgr.clone()),
            )
        } else {
            ctx
        };
        self.doPreCheckOnItem(checkCtx, precheck::CheckTargetClusterSize)
    }

    pub fn ClusterIsAvailable(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckTargetClusterVersion)
    }

    pub fn checkEmptyRegion(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckTargetClusterEmptyRegion)
    }

    pub fn checkRegionDistribution(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckTargetClusterRegionDist)
    }

    pub fn checkClusterRegion(&mut self, ctx: Context) -> Result<()> {
        // region 检查会先看任务是否已经开始恢复，
        // 若已启动则跳过，避免在续跑阶段重复做启动前检查。
        let taskMgr = self
            .taskMgr
            .as_mut()
            .ok_or_else(|| errors::New("taskMgr is nil"))?;
        // Capture checks via exclusive action like Go.
        // We need mutable self for doPreCheckOnItem; do checks outside closure after scan.
        let restoreStarted = {
            let mut started = false;
            taskMgr.CheckTasksExclusively(ctx.clone(), &mut |tasks| {
                for task in tasks.iter() {
                    if task.status > taskMetaStatusInitial {
                        started = true;
                        break;
                    }
                }
                Ok(None)
            })?;
            started
        };
        if restoreStarted {
            return Ok(());
        }
        self.checkEmptyRegion(ctx.clone()).map_err(errors::Trace)?;
        self.checkRegionDistribution(ctx).map_err(errors::Trace)?;
        Ok(())
    }

    pub fn StoragePermission(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckSourcePermission)
    }

    pub fn HasLargeCSV(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckLargeDataFile)
    }

    pub fn localResource(&mut self, ctx: Context) -> Result<()> {
        // 本地源场景需要额外检查磁盘放置与临时 KV 目录；
        // 非本地源则只保留临时 KV 目录检查。
        if self.isSourceInLocal() {
            self.doPreCheckOnItem(ctx.clone(), precheck::CheckLocalDiskPlacement)
                .map_err(errors::Trace)?;
        }
        self.doPreCheckOnItem(ctx, precheck::CheckLocalTempKVDir)
    }

    pub fn checkCSVHeader(&mut self, ctx: Context) -> Result<()> {
        self.doPreCheckOnItem(ctx, precheck::CheckCSVHeader)
    }

    pub fn checkTableEmpty(&mut self, ctx: Context) -> Result<()> {
        // TiDB backend 和并行导入路径不要求目标表为空，
        // 因此这里保留与 Go 一致的条件短路。
        if self.cfg.TikvImporter.Backend == crate::config::BackendTiDB
            || self.cfg.TikvImporter.ParallelImport
        {
            return Ok(());
        }
        self.doPreCheckOnItem(ctx, precheck::CheckTargetTableEmpty)
    }

    pub fn checkCheckpoints(&mut self, ctx: Context) -> Result<()> {
        // 只有显式启用 checkpoint 时，才检查已有断点状态是否与当前任务兼容。
        if !self.cfg.Checkpoint.Enable {
            return Ok(());
        }
        self.doPreCheckOnItem(ctx, precheck::CheckCheckpoints)
    }

    pub fn checkSourceSchema(&mut self, ctx: Context) -> Result<()> {
        // TiDB backend 不走 local/importer 那套源 schema 校验，因此直接跳过。
        if self.cfg.TikvImporter.Backend == crate::config::BackendTiDB {
            return Ok(());
        }
        self.doPreCheckOnItem(ctx, precheck::CheckSourceSchemaValid)
    }

    pub fn checkCDCPiTR(&mut self, ctx: Context) -> Result<()> {
        // CDC / PiTR 冲突检查只在非 TiDB backend 下有意义。
        if self.cfg.TikvImporter.Backend == crate::config::BackendTiDB {
            return Ok(());
        }
        self.doPreCheckOnItem(ctx, precheck::CheckTargetUsingCDCPITR)
    }

    pub fn checkPDTiDBFromSameCluster(&mut self, ctx: Context) -> Result<()> {
        // 该检查保护 PD 与 TiDB 是否来自同一集群，避免误连到错误目标环境。
        if self.cfg.TikvImporter.Backend == crate::config::BackendTiDB {
            return Ok(());
        }
        self.doPreCheckOnItem(ctx, precheck::CheckPDTiDBFromSameCluster)
    }
}
