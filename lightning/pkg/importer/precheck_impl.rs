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

//! Precheck item implementations matching Go `precheck_impl.go`.

use crate::check_info::{
    CHECK_REGION_CNT_RATIO_THRESHOLD, DEFAULT_CSV_SIZE, ERROR_EMPTY_REGION_CNT_PER_STORE,
    ERROR_REGION_CNT_MIN_MAX_RATIO, WARN_EMPTY_REGION_CNT_PER_STORE, WARN_REGION_CNT_MIN_MAX_RATIO,
};
use crate::config::{self, Config};
use crate::context::Context;
use crate::etcd;
use crate::get_pre_info::PreImportInfoGetter;
use crate::importdef;
use crate::mydump;
use crate::sql::DB;
use crate::streamhelper;
use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_precheck as precheck;
use std::sync::Arc;

// 语义说明：`map_err` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
fn map_err(e: crate::Error) -> precheck::errors::Error {
    precheck::errors::New(e.Error())
}

// 语义说明：`ok_result` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
fn ok_result(
    item: precheck::CheckItemID,
    severity: precheck::CheckType,
    passed: bool,
    msg: impl Into<String>,
) -> Option<precheck::CheckResult> {
    Some(precheck::CheckResult {
        Item: item,
        Severity: severity,
        Passed: passed,
        Message: msg.into(),
    })
}

// 语义说明：`clusterResourceCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct clusterResourceCheckItem {
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
}
// 语义说明：`NewClusterResourceCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewClusterResourceCheckItem(
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
) -> Box<dyn precheck::Checker> {
    Box::new(clusterResourceCheckItem { preInfoGetter })
}
// 语义说明：`clusterResourceCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for clusterResourceCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetClusterSize
    }
    fn Check(
        &mut self,
        ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let _ = ctx;
        let local_ctx = Context::default();
        let est = self
            .preInfoGetter
            .EstimateSourceDataSize(local_ctx.clone(), &[])
            .map_err(map_err)?;
        let storage = self
            .preInfoGetter
            .GetStorageInfo(local_ctx)
            .map_err(map_err)?;
        let replica_count = self
            .preInfoGetter
            .GetMaxReplica(Context::default())
            .map_err(map_err)?;
        let mut avail = 0u64;
        for s in &storage.Stores {
            avail = avail.saturating_add(s.Status.Available);
        }
        let need = (est.SizeWithIndex.max(0) as u64).saturating_mul(replica_count);
        let passed = avail >= need;
        let msg = if passed {
            format!(
                "cluster available storage {avail} bytes is enough for source size {need} bytes"
            )
        } else {
            format!("cluster available storage {avail} bytes is less than source size {need} bytes")
        };
        Ok(ok_result(
            precheck::CheckTargetClusterSize,
            precheck::Warn,
            passed,
            msg,
        ))
    }
}

// 语义说明：`clusterVersionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct clusterVersionCheckItem {
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewClusterVersionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewClusterVersionCheckItem(
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(clusterVersionCheckItem {
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`clusterVersionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for clusterVersionCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetClusterVersion
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let (passed, message) = match self
            .preInfoGetter
            .CheckVersionRequirements(Context::default())
        {
            Ok(()) => (true, "cluster version check passed".to_string()),
            Err(err) => (
                false,
                format!("cluster version check failed: {}", err.Error()),
            ),
        };
        Ok(ok_result(
            precheck::CheckTargetClusterVersion,
            precheck::Critical,
            passed,
            message,
        ))
    }
}

// 语义说明：`emptyRegionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct emptyRegionCheckItem {
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewEmptyRegionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewEmptyRegionCheckItem(
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(emptyRegionCheckItem {
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`emptyRegionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for emptyRegionCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetClusterEmptyRegion
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let info = self
            .preInfoGetter
            .GetStorageInfo(Context::default())
            .map_err(map_err)?;
        let mut max_empty = 0i64;
        for s in &info.Stores {
            max_empty = max_empty.max(s.Status.EmptyRegionCount);
        }
        let (passed, msg) = if max_empty > ERROR_EMPTY_REGION_CNT_PER_STORE {
            (
                false,
                format!("too many empty regions per store: {max_empty}"),
            )
        } else if max_empty > WARN_EMPTY_REGION_CNT_PER_STORE {
            (
                true,
                format!("empty regions per store is high: {max_empty}"),
            )
        } else {
            (true, format!("empty regions check passed, max={max_empty}"))
        };
        Ok(ok_result(
            precheck::CheckTargetClusterEmptyRegion,
            precheck::Warn,
            passed,
            msg,
        ))
    }
}

// 语义说明：`regionDistributionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct regionDistributionCheckItem {
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewRegionDistributionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewRegionDistributionCheckItem(
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(regionDistributionCheckItem {
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`regionDistributionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for regionDistributionCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetClusterRegionDist
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let info = self
            .preInfoGetter
            .GetStorageInfo(Context::default())
            .map_err(map_err)?;
        let mut min_c = i64::MAX;
        let mut max_c = 0i64;
        for s in &info.Stores {
            min_c = min_c.min(s.Status.RegionCount);
            max_c = max_c.max(s.Status.RegionCount);
        }
        if info.Stores.len() <= 1 || max_c <= CHECK_REGION_CNT_RATIO_THRESHOLD {
            return Ok(ok_result(
                precheck::CheckTargetClusterRegionDist,
                precheck::Warn,
                true,
                "region distribution check skipped",
            ));
        }
        let ratio = if max_c == 0 {
            1.0
        } else {
            (min_c as f64) / (max_c as f64)
        };
        let (passed, msg) = if ratio < ERROR_REGION_CNT_MIN_MAX_RATIO {
            (
                false,
                format!("region distribution unbalanced, min/max={ratio:.3}"),
            )
        } else if ratio < WARN_REGION_CNT_MIN_MAX_RATIO {
            (
                true,
                format!("region distribution skewed, min/max={ratio:.3}"),
            )
        } else {
            (
                true,
                format!("region distribution check passed, min/max={ratio:.3}"),
            )
        };
        Ok(ok_result(
            precheck::CheckTargetClusterRegionDist,
            precheck::Warn,
            passed,
            msg,
        ))
    }
}

// 语义说明：`storagePermissionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct storagePermissionCheckItem {
    pub cfg: Config,
}
// 语义说明：`NewStoragePermissionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewStoragePermissionCheckItem(cfg: &Config) -> Box<dyn precheck::Checker> {
    Box::new(storagePermissionCheckItem { cfg: cfg.clone() })
}
// 语义说明：`storagePermissionCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for storagePermissionCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckSourcePermission
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let ok = !self.cfg.Mydumper.SourceDir.is_empty();
        Ok(ok_result(
            precheck::CheckSourcePermission,
            precheck::Critical,
            ok,
            if ok {
                "source storage permission check passed"
            } else {
                "source storage path is empty"
            },
        ))
    }
}

// 语义说明：`largeFileCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct largeFileCheckItem {
    pub cfg: Config,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewLargeFileCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewLargeFileCheckItem(
    cfg: &Config,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(largeFileCheckItem {
        cfg: cfg.clone(),
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`largeFileCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for largeFileCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckLargeDataFile
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if self.cfg.Mydumper.StrictFormat {
            return Ok(ok_result(
                precheck::CheckLargeDataFile,
                precheck::Warn,
                true,
                "skip large file check because strict-format is enabled",
            ));
        }
        let mut large = Vec::new();
        for db in &self.dbMetas {
            for tbl in &db.Tables {
                for f in &tbl.DataFiles {
                    if f.FileMeta.Type == mydump::SourceTypeCSV
                        && f.FileMeta.FileSize as u64 > DEFAULT_CSV_SIZE
                    {
                        large.push(f.FileMeta.Path.clone());
                    }
                }
            }
        }
        let passed = large.is_empty();
        let msg = if passed {
            "no large CSV files".into()
        } else {
            format!(
                "large CSV files detected (>{DEFAULT_CSV_SIZE} bytes): {}",
                large.join(",")
            )
        };
        Ok(ok_result(
            precheck::CheckLargeDataFile,
            precheck::Warn,
            passed,
            msg,
        ))
    }
}

// 语义说明：`localDiskPlacementCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct localDiskPlacementCheckItem {
    pub cfg: Config,
}
// 语义说明：`NewLocalDiskPlacementCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewLocalDiskPlacementCheckItem(cfg: &Config) -> Box<dyn precheck::Checker> {
    Box::new(localDiskPlacementCheckItem { cfg: cfg.clone() })
}
// 语义说明：`localDiskPlacementCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for localDiskPlacementCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckLocalDiskPlacement
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let src = &self.cfg.Mydumper.SourceDir;
        let sorted = &self.cfg.TikvImporter.SortedKVDir;
        let same = !src.is_empty() && src == sorted;
        Ok(ok_result(
            precheck::CheckLocalDiskPlacement,
            precheck::Warn,
            !same,
            if same {
                "source data and sorted-kv-dir are on the same path"
            } else {
                "local disk placement check passed"
            },
        ))
    }
}

// 语义说明：`localTempKVDirCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct localTempKVDirCheckItem {
    pub cfg: Config,
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewLocalTempKVDirCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewLocalTempKVDirCheckItem(
    cfg: &Config,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(localTempKVDirCheckItem {
        cfg: cfg.clone(),
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`localTempKVDirCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl localTempKVDirCheckItem {
    fn hasCompressedFiles(&self) -> bool {
        for db in &self.dbMetas {
            for tbl in &db.Tables {
                for f in &tbl.DataFiles {
                    if f.FileMeta.Compression != mydump::CompressionNone {
                        return true;
                    }
                }
            }
        }
        false
    }
}
// 语义说明：`localTempKVDirCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for localTempKVDirCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckLocalTempKVDir
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if self.cfg.TikvImporter.Backend != config::BackendLocal {
            return Ok(None);
        }
        let dir = &self.cfg.TikvImporter.SortedKVDir;
        let passed = !dir.is_empty();
        let msg = if passed {
            format!(
                "local temp kv dir `{dir}` ok (compressed={})",
                self.hasCompressedFiles()
            )
        } else {
            "sorted-kv-dir is empty".into()
        };
        Ok(ok_result(
            precheck::CheckLocalTempKVDir,
            precheck::Critical,
            passed,
            msg,
        ))
    }
}

// 语义说明：`checkpointCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct checkpointCheckItem {
    pub cfg: Config,
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    pub checkpointsDB: Option<Arc<dyn checkpoints::DB>>,
}
// 语义说明：`NewCheckpointCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewCheckpointCheckItem(
    cfg: &Config,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
    checkpointsDB: Option<Arc<dyn checkpoints::DB>>,
) -> Box<dyn precheck::Checker> {
    Box::new(checkpointCheckItem {
        cfg: cfg.clone(),
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
        checkpointsDB,
    })
}
// 语义说明：`checkpointCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for checkpointCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckCheckpoints
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if !self.cfg.Checkpoint.Enable {
            return Ok(None);
        }
        let passed = self.checkpointsDB.is_some();
        Ok(ok_result(
            precheck::CheckCheckpoints,
            precheck::Critical,
            passed,
            if passed {
                "checkpoints db available"
            } else {
                "checkpoints db missing"
            },
        ))
    }
}

// 语义说明：`CDCPITRCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub(crate) type CDCPITRStatusGetter =
    Arc<dyn Fn(Context, &Config, &[String], &str) -> crate::Result<bool> + Send + Sync>;

pub struct CDCPITRCheckItem {
    pub keyspaceName: String,
    pub cfg: Config,
    pub pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
    statusGetter: CDCPITRStatusGetter,
}
// 语义说明：`NewCDCPITRCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewCDCPITRCheckItem(
    cfg: &Config,
    pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
) -> Box<dyn precheck::Checker> {
    NewCDCPITRCheckItemWithStatusGetter(cfg, pdAddrsGetter, Arc::new(getCDCPiTRStatus))
}
pub(crate) fn NewCDCPITRCheckItemWithStatusGetter(
    cfg: &Config,
    pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
    statusGetter: CDCPITRStatusGetter,
) -> Box<dyn precheck::Checker> {
    newCDCPITRCheckItem(
        cfg,
        pdAddrsGetter,
        &cfg.TikvImporter.KeyspaceName,
        statusGetter,
    )
}
pub fn NewCDCPITRCheckItemWithKeyspaceName(
    cfg: &Config,
    pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
    keyspaceName: &str,
) -> Box<dyn precheck::Checker> {
    newCDCPITRCheckItem(cfg, pdAddrsGetter, keyspaceName, Arc::new(getCDCPiTRStatus))
}
fn newCDCPITRCheckItem(
    cfg: &Config,
    pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
    keyspaceName: &str,
    statusGetter: CDCPITRStatusGetter,
) -> Box<dyn precheck::Checker> {
    Box::new(CDCPITRCheckItem {
        keyspaceName: keyspaceName.into(),
        cfg: cfg.clone(),
        pdAddrsGetter,
        statusGetter,
    })
}
// 语义说明：`CDCPITRCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for CDCPITRCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetUsingCDCPITR
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if self.cfg.TikvImporter.Backend == config::BackendTiDB {
            return Ok(None);
        }
        let addrs = (self.pdAddrsGetter)(Context::default());
        let active = (self.statusGetter)(Context::default(), &self.cfg, &addrs, &self.keyspaceName)
            .map_err(map_err)?;
        Ok(ok_result(
            precheck::CheckTargetUsingCDCPITR,
            precheck::Critical,
            !active,
            if active {
                "cluster has active CDC/PiTR tasks"
            } else {
                "no active CDC/PiTR"
            },
        ))
    }
}

fn getCDCPiTRStatus(
    ctx: Context,
    cfg: &Config,
    addrs: &[String],
    keyspaceName: &str,
) -> crate::Result<bool> {
    let cli = etcd::Client(dialEtcdWithCfg(ctx, cfg, addrs, keyspaceName)?);
    let active = streamhelper::GetCDCPiTRStatus(&cli);
    cli.Close();
    active
}

// 语义说明：`schemaCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct schemaCheckItem {
    pub cfg: Config,
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    pub cpdb: Option<Arc<dyn checkpoints::DB>>,
}
// 语义说明：`NewSchemaCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewSchemaCheckItem(
    cfg: &Config,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
    cpdb: Option<Arc<dyn checkpoints::DB>>,
) -> Box<dyn precheck::Checker> {
    Box::new(schemaCheckItem {
        cfg: cfg.clone(),
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
        cpdb,
    })
}
// 语义说明：`schemaCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for schemaCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckSourceSchemaValid
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let structs = self
            .preInfoGetter
            .GetAllTableStructures(Context::default(), &[])
            .map_err(map_err)?;
        let passed = !structs.is_empty() || self.dbMetas.is_empty();
        Ok(ok_result(
            precheck::CheckSourceSchemaValid,
            precheck::Critical,
            passed,
            if passed {
                "source schema valid"
            } else {
                "failed to load source schema"
            },
        ))
    }
}

// 语义说明：`csvHeaderCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct csvHeaderCheckItem {
    pub cfg: Config,
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
}
// 语义说明：`NewCSVHeaderCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewCSVHeaderCheckItem(
    cfg: &Config,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
) -> Box<dyn precheck::Checker> {
    Box::new(csvHeaderCheckItem {
        cfg: cfg.clone(),
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
    })
}
// 语义说明：`csvHeaderCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for csvHeaderCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckCSVHeader
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        // SchemaCheck already validates the opposite mismatch when headers are enabled.
        if self.cfg.Mydumper.CSV.Header {
            return Ok(None);
        }
        Ok(ok_result(
            precheck::CheckCSVHeader,
            precheck::Critical,
            true,
            "the config [mydumper.csv.header] is set to false, and CSV header lines are really not detected in the data files",
        ))
    }
}

// 语义说明：`checkFieldCompatibility` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn checkFieldCompatibility(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

// 语义说明：`tableEmptyCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct tableEmptyCheckItem {
    pub cfg: Config,
    pub preInfoGetter: Arc<dyn PreImportInfoGetter>,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    pub cpdb: Option<Arc<dyn checkpoints::DB>>,
}
// 语义说明：`NewTableEmptyCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewTableEmptyCheckItem(
    cfg: &Config,
    preInfoGetter: Arc<dyn PreImportInfoGetter>,
    dbMetas: &[mydump::MDDatabaseMeta],
    cpdb: Option<Arc<dyn checkpoints::DB>>,
) -> Box<dyn precheck::Checker> {
    Box::new(tableEmptyCheckItem {
        cfg: cfg.clone(),
        preInfoGetter,
        dbMetas: dbMetas.to_vec(),
        cpdb,
    })
}
// 语义说明：`tableEmptyCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for tableEmptyCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckTargetTableEmpty
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if self.cfg.TikvImporter.Backend == config::BackendTiDB
            || self.cfg.TikvImporter.ParallelImport
        {
            return Ok(None);
        }
        let mut non_empty = Vec::new();
        for db in &self.dbMetas {
            for tbl in &db.Tables {
                match self
                    .preInfoGetter
                    .IsTableEmpty(Context::default(), &db.Name, &tbl.Name)
                    .map_err(map_err)?
                {
                    Some(false) => non_empty.push(format!("{}.{}", db.Name, tbl.Name)),
                    _ => {}
                }
            }
        }
        let passed = non_empty.is_empty();
        Ok(ok_result(
            precheck::CheckTargetTableEmpty,
            precheck::Critical,
            passed,
            if passed {
                "target tables are empty".into()
            } else {
                format!("target tables not empty: {}", non_empty.join(","))
            },
        ))
    }
}

// 语义说明：`hasDefault` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn hasDefault(col: &crate::model::ColumnInfo) -> bool {
    col.DefaultValue.is_some()
        || col.AutoIncrement
        || (!col.NotNull && !col.Name.L.is_empty())
        || col.IsGenerated()
        || col.Name.L == "_tidb_rowid"
}

// 语义说明：`pdTiDBFromSameClusterCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub struct pdTiDBFromSameClusterCheckItem {
    pub targetDB: Option<DB>,
    pub pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
}
// 语义说明：`NewPDTiDBFromSameClusterCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
pub fn NewPDTiDBFromSameClusterCheckItem(
    targetDB: Option<DB>,
    pdAddrsGetter: Arc<dyn Fn(crate::context::Context) -> Vec<String> + Send + Sync>,
) -> Box<dyn precheck::Checker> {
    Box::new(pdTiDBFromSameClusterCheckItem {
        targetDB,
        pdAddrsGetter,
    })
}
// 语义说明：`pdTiDBFromSameClusterCheckItem` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
impl precheck::Checker for pdTiDBFromSameClusterCheckItem {
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        precheck::CheckPDTiDBFromSameCluster
    }
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        let addrs = (self.pdAddrsGetter)(Context::default());
        let passed = self.targetDB.is_some() && !addrs.is_empty();
        Ok(ok_result(
            precheck::CheckPDTiDBFromSameCluster,
            precheck::Critical,
            passed,
            if passed {
                "PD and TiDB appear from same cluster configuration"
            } else {
                "unable to verify PD/TiDB same cluster"
            },
        ))
    }
}

/// One explicit V1 PD metadata lookup followed by the keyspace-selected etcd dial.
pub fn dialEtcdWithCfg(
    ctx: Context,
    cfg: &Config,
    addrs: &[String],
    keyspaceName: &str,
) -> crate::Result<astersql_metaservice::NamespacedEtcdClient> {
    let security = astersql_metaservice::PdSecurity {
        ca: cfg.Security.ClusterSSLCA.clone(),
        cert: cfg.Security.ClusterSSLCert.clone(),
        key: cfg.Security.ClusterSSLKey.clone(),
    };
    let config = astersql_metaservice::EtcdDialConfig {
        tls: security
            .etcd_tls()
            .map_err(|error| crate::Error::new(error.to_string()))?,
        ..Default::default()
    };
    let context =
        astersql_metaservice::Context::with_cancellation_checker(move || ctx.Err().is_some());
    astersql_metaservice::DialEtcdClient(
        &context,
        keyspaceName,
        addrs,
        &security,
        cfg.MetadataRuntime.pd_factory.as_ref(),
        config,
    )
    .map_err(|error| crate::Error::new(error.to_string()))
}
