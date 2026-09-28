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

// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 本次只增加注释，不改变任何运行时逻辑或测试行为。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Pre-import info getters matching Go `get_pre_info.go`.

use crate::config::Config;
use crate::context::{self, Context};
use crate::encode::EncodingBuilder;
use crate::errors::{self, Result};
use crate::importdef;
use crate::model;
use crate::mydump;
use crate::parser::Parser;
use crate::pdhttp;
use crate::sql::DB;
use crate::storeapi::Storage;
use crate::tidb::ObtainImportantVariables;
use crate::types::{self, Datum};
use crate::worker;
use astersql_lightning_mydump as parser_impl;
use astersql_lightning_pkg_importer_opts as ropts;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
// 自动补充的`EstimateSourceDataSizeResult` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct EstimateSourceDataSizeResult {
    pub SizeWithIndex: i64,
    pub SizeWithoutIndex: i64,
    pub HasUnsortedBigTables: bool,
    pub TiFlashSize: i64,
}

pub trait TargetInfoGetter: Send + Sync {
    // 自动补充的`FetchRemoteDBModels` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn FetchRemoteDBModels(&self, ctx: Context) -> Result<Vec<model::DBInfo>>;
    fn FetchRemoteTableModels(
        &self,
        ctx: Context,
        schemaName: &str,
    ) -> Result<Vec<model::TableInfo>>;
    // 自动补充的`CheckVersionRequirements` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn CheckVersionRequirements(&self, ctx: Context) -> Result<()>;
    fn IsTableEmpty(&self, ctx: Context, schemaName: &str, tableName: &str)
    -> Result<Option<bool>>;
    fn GetTargetSysVariablesForImport(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> HashMap<String, String>;
    // 自动补充的`GetMaxReplica` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetMaxReplica(&self, ctx: Context) -> Result<u64>;
    fn GetStorageInfo(&self, ctx: Context) -> Result<StoresInfo>;
    fn GetEmptyRegionsInfo(&self, ctx: Context) -> Result<RegionsInfo>;
}

pub trait PreImportInfoGetter: TargetInfoGetter {
    // 自动补充的`Init` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn Init(&self);
    fn GetAllTableStructures(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> Result<HashMap<String, importdef::DBInfo>>;
    // 自动补充的`ReadFirstNRowsByTableName` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn ReadFirstNRowsByTableName(
        &self,
        ctx: Context,
        schemaName: &str,
        tableName: &str,
        n: i32,
    ) -> Result<(Vec<String>, Vec<Vec<Datum>>)>;
    // 自动补充的`ReadFirstNRowsByFileMeta` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn ReadFirstNRowsByFileMeta(
        &self,
        ctx: Context,
        dataFileMeta: mydump::SourceFileMeta,
        n: i32,
    ) -> Result<(Vec<String>, Vec<Vec<Datum>>)>;
    // 自动补充的`EstimateSourceDataSize` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn EstimateSourceDataSize(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> Result<EstimateSourceDataSizeResult>;
}

#[derive(Clone, Debug, Default)]
// 自动补充的`StoresInfo` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct StoresInfo {
    pub Count: i32,
    pub Stores: Vec<StoreInfo>,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`StoreInfo` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct StoreInfo {
    pub Store: StoreMeta,
    pub Status: StoreStatus,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`StoreMeta` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct StoreMeta {
    pub Id: u64,
    pub Address: String,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`StoreStatus` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct StoreStatus {
    pub Capacity: u64,
    pub Available: u64,
    pub RegionCount: i64,
    pub EmptyRegionCount: i64,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`RegionsInfo` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct RegionsInfo {
    pub Count: i32,
    pub Regions: Vec<RegionInfo>,
}

#[derive(Clone, Debug, Default)]
// 自动补充的`RegionInfo` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct RegionInfo {
    pub Id: u64,
    pub StoreId: u64,
}

// 自动补充的`TargetInfoGetterImpl` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct TargetInfoGetterImpl {
    pub cfg: Config,
    pub db: DB,
    pub pdHTTPCli: Option<pdhttp::Client>,
    pub sysVars: Mutex<HashMap<String, String>>,
}

// 自动补充的`NewTargetInfoGetterImpl` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn NewTargetInfoGetterImpl(
    cfg: &Config,
    db: DB,
    pdHTTPCli: Option<pdhttp::Client>,
) -> Result<Arc<TargetInfoGetterImpl>> {
    if !cfg.TikvImporter.Backend.is_empty()
        && cfg.TikvImporter.Backend != crate::config::BackendTiDB
        && cfg.TikvImporter.Backend != crate::config::BackendLocal
    {
        return Err(errors::Errorf(format!(
            "unknown backend '{}'",
            cfg.TikvImporter.Backend
        )));
    }
    Ok(Arc::new(TargetInfoGetterImpl {
        cfg: cfg.clone(),
        db,
        pdHTTPCli,
        sysVars: Mutex::new(HashMap::new()),
    }))
}

// 自动补充的下面的 `impl TargetInfoGetter` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl TargetInfoGetter for TargetInfoGetterImpl {
    fn FetchRemoteDBModels(&self, _ctx: Context) -> Result<Vec<model::DBInfo>> {
        Ok(self
            .db
            .query_string_matrix("SHOW DATABASES")?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|name| model::DBInfo {
                Name: model::CIStr::new(&name),
                Tables: Vec::new(),
            })
            .collect())
    }
    fn FetchRemoteTableModels(
        &self,
        _ctx: Context,
        schemaName: &str,
    ) -> Result<Vec<model::TableInfo>> {
        let query = format!(
            "SHOW TABLES FROM {}",
            crate::common::EscapeIdentifier(schemaName)
        );
        Ok(self
            .db
            .query_string_matrix(&query)?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|name| model::TableInfo {
                Name: model::CIStr::new(&name),
                State: model::StatePublic,
                ..Default::default()
            })
            .collect())
    }
    // 自动补充的`CheckVersionRequirements` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn CheckVersionRequirements(&self, _ctx: Context) -> Result<()> {
        if self.cfg.TikvImporter.Backend == crate::config::BackendLocal && self.pdHTTPCli.is_none()
        {
            return Err(errors::New(
                "pd HTTP client is required for component version check in local backend",
            ));
        }
        Ok(())
    }
    fn IsTableEmpty(
        &self,
        ctx: Context,
        schemaName: &str,
        tableName: &str,
    ) -> Result<Option<bool>> {
        let q = format!(
            "SELECT 1 FROM {} USE INDEX() LIMIT 1",
            crate::common::UniqueTable(schemaName, tableName)
        );
        match self.db.QueryRowString(&q) {
            Ok(_) => Ok(Some(false)),
            Err(e) if e.not_found || e.class == Some("ErrNoRows") => Ok(Some(true)),
            Err(e) => Err(errors::Trace(e)),
        }
    }
    // 自动补充的`GetTargetSysVariablesForImport` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetTargetSysVariablesForImport(
        &self,
        ctx: Context,
        _opts: &[ropts::GetPreInfoOption],
    ) -> HashMap<String, String> {
        let need = self.cfg.TikvImporter.Backend == crate::config::BackendTiDB;
        let mut vars = ObtainImportantVariables(ctx, &self.db, need);
        if let Some(overrides) = &self.cfg.TiDB.Vars {
            vars.extend(overrides.clone());
        }
        *self.sysVars.lock().unwrap() = vars.clone();
        vars
    }
    // 自动补充的`GetMaxReplica` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetMaxReplica(&self, _ctx: Context) -> Result<u64> {
        let Some(client) = self.pdHTTPCli.as_ref() else {
            return Ok(3);
        };
        if let Some(error) = &client.request_error {
            return Err(errors::New(error));
        }
        Ok(client.max_replicas)
    }
    fn GetStorageInfo(&self, _ctx: Context) -> Result<StoresInfo> {
        let Some(client) = self.pdHTTPCli.as_ref() else {
            return Ok(StoresInfo::default());
        };
        if let Some(error) = &client.request_error {
            return Err(errors::New(error));
        }
        Ok(StoresInfo {
            Count: client.stores.len() as i32,
            Stores: client
                .stores
                .iter()
                .map(
                    |(id, address, capacity, available, regions, empty)| StoreInfo {
                        Store: StoreMeta {
                            Id: *id,
                            Address: address.clone(),
                        },
                        Status: StoreStatus {
                            Capacity: *capacity,
                            Available: *available,
                            RegionCount: *regions,
                            EmptyRegionCount: *empty,
                        },
                    },
                )
                .collect(),
        })
    }
    // 自动补充的`GetEmptyRegionsInfo` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetEmptyRegionsInfo(&self, _ctx: Context) -> Result<RegionsInfo> {
        let Some(client) = self.pdHTTPCli.as_ref() else {
            return Ok(RegionsInfo::default());
        };
        if let Some(error) = &client.request_error {
            return Err(errors::New(error));
        }
        Ok(RegionsInfo {
            Count: client.empty_regions.len() as i32,
            Regions: client
                .empty_regions
                .iter()
                .map(|(id, store_id)| RegionInfo {
                    Id: *id,
                    StoreId: *store_id,
                })
                .collect(),
        })
    }
}

// 自动补充的`PreImportInfoGetterImpl` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct PreImportInfoGetterImpl {
    pub cfg: Config,
    pub dbMetas: Vec<mydump::MDDatabaseMeta>,
    pub srcStorage: Storage,
    pub target: Arc<dyn TargetInfoGetter>,
    pub ioWorkers: Option<Arc<worker::Pool>>,
    pub encBuilder: Option<Arc<dyn EncodingBuilder>>,
    pub tableStructs: Mutex<HashMap<String, importdef::DBInfo>>,
    pub estimated: Mutex<Option<EstimateSourceDataSizeResult>>,
}

// 自动补充的`NewPreImportInfoGetter` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn NewPreImportInfoGetter(
    cfg: &Config,
    dbMetas: Vec<mydump::MDDatabaseMeta>,
    srcStorage: Storage,
    target: Arc<dyn TargetInfoGetter>,
    ioWorkers: Option<Arc<worker::Pool>>,
    encBuilder: Option<Arc<dyn EncodingBuilder>>,
    _opts: Vec<ropts::GetPreInfoOption>,
) -> Result<Arc<dyn PreImportInfoGetter>> {
    Ok(Arc::new(PreImportInfoGetterImpl {
        cfg: cfg.clone(),
        dbMetas,
        srcStorage,
        target,
        ioWorkers,
        encBuilder,
        tableStructs: Mutex::new(HashMap::new()),
        estimated: Mutex::new(None),
    }))
}

// 自动补充的下面的 `impl TargetInfoGetter` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl TargetInfoGetter for PreImportInfoGetterImpl {
    fn FetchRemoteDBModels(&self, ctx: Context) -> Result<Vec<model::DBInfo>> {
        self.target.FetchRemoteDBModels(ctx)
    }
    fn FetchRemoteTableModels(
        &self,
        ctx: Context,
        schemaName: &str,
    ) -> Result<Vec<model::TableInfo>> {
        self.target.FetchRemoteTableModels(ctx, schemaName)
    }
    // 自动补充的`CheckVersionRequirements` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn CheckVersionRequirements(&self, ctx: Context) -> Result<()> {
        self.target.CheckVersionRequirements(ctx)
    }
    fn IsTableEmpty(
        &self,
        ctx: Context,
        schemaName: &str,
        tableName: &str,
    ) -> Result<Option<bool>> {
        self.target.IsTableEmpty(ctx, schemaName, tableName)
    }
    // 自动补充的`GetTargetSysVariablesForImport` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetTargetSysVariablesForImport(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> HashMap<String, String> {
        self.target.GetTargetSysVariablesForImport(ctx, opts)
    }
    // 自动补充的`GetMaxReplica` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetMaxReplica(&self, ctx: Context) -> Result<u64> {
        self.target.GetMaxReplica(ctx)
    }
    fn GetStorageInfo(&self, ctx: Context) -> Result<StoresInfo> {
        self.target.GetStorageInfo(ctx)
    }
    // 自动补充的`GetEmptyRegionsInfo` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn GetEmptyRegionsInfo(&self, ctx: Context) -> Result<RegionsInfo> {
        self.target.GetEmptyRegionsInfo(ctx)
    }
}

// 自动补充的下面的 `impl PreImportInfoGetter` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl PreImportInfoGetter for PreImportInfoGetterImpl {
    fn Init(&self) {}

    fn GetAllTableStructures(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> Result<HashMap<String, importdef::DBInfo>> {
        let cfg = ropts::ApplyGetPreInfoOptions(None, opts);
        if !cfg.ForceReloadCache {
            let cached = self.tableStructs.lock().unwrap();
            if !cached.is_empty() {
                return Ok(cached.clone());
            }
        }
        let mut result = HashMap::new();
        for dbMeta in &self.dbMetas {
            let tables = self.getTableStructuresByFileMeta(ctx.clone(), dbMeta, &cfg)?;
            let mut dbInfo = importdef::DBInfo {
                Name: dbMeta.Name.clone(),
                Tables: HashMap::new(),
            };
            for (i, t) in tables.into_iter().enumerate() {
                let name = dbMeta
                    .Tables
                    .get(i)
                    .map(|x| x.Name.clone())
                    .unwrap_or_else(|| t.Name.O.clone());
                dbInfo.Tables.insert(
                    name.clone(),
                    importdef::TableInfo {
                        ID: t.ID,
                        DB: dbMeta.Name.clone(),
                        Name: name,
                        Core: t.clone(),
                        Desired: Some(t),
                    },
                );
            }
            result.insert(dbMeta.Name.clone(), dbInfo);
        }
        *self.tableStructs.lock().unwrap() = result.clone();
        Ok(result)
    }

    // 自动补充的`ReadFirstNRowsByTableName` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn ReadFirstNRowsByTableName(
        &self,
        ctx: Context,
        schemaName: &str,
        tableName: &str,
        n: i32,
    ) -> Result<(Vec<String>, Vec<Vec<Datum>>)> {
        let mut schema_found = false;
        for db in &self.dbMetas {
            if db.Name == schemaName {
                schema_found = true;
                for tbl in &db.Tables {
                    if tbl.Name == tableName {
                        if let Some(f) = tbl.DataFiles.first() {
                            return self.ReadFirstNRowsByFileMeta(ctx, f.FileMeta.clone(), n);
                        }
                        return Ok((Vec::new(), Vec::new()));
                    }
                }
            }
        }
        if !schema_found {
            return Err(errors::Errorf(format!(
                "cannot find the schema: {schemaName}"
            )));
        }
        Err(errors::Errorf(format!(
            "cannot find the table: {schemaName}.{tableName}"
        )))
    }

    // 自动补充的`ReadFirstNRowsByFileMeta` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn ReadFirstNRowsByFileMeta(
        &self,
        ctx: Context,
        dataFileMeta: mydump::SourceFileMeta,
        n: i32,
    ) -> Result<(Vec<String>, Vec<Vec<Datum>>)> {
        if let Some(err) = ctx.Err() {
            return Err(err);
        }
        if n <= 0 {
            return Ok((Vec::new(), Vec::new()));
        }

        let raw = self.srcStorage.Read(&dataFileMeta.Path)?;
        let raw = match dataFileMeta.Compression {
            mydump::CompressionNone => raw,
            1 => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(raw.as_slice())
                    .read_to_end(&mut decoded)
                    .map_err(|error| errors::Errorf(format!("decompress gzip source: {error}")))?;
                decoded
            }
            compression => {
                return Err(errors::Errorf(format!(
                    "file '{}' uses unsupported compression '{}'",
                    dataFileMeta.Path, compression
                )));
            }
        };
        let mut parser: Box<dyn parser_impl::Parser> = match dataFileMeta.Type {
            mydump::SourceTypeCSV => {
                let reader: Box<dyn parser_impl::ReadSeekCloser> =
                    Box::new(parser_impl::StringReader::from_bytes(raw));
                let csv = parser_impl::CsvConfig {
                    header: self.cfg.Mydumper.CSV.Header,
                    ..Default::default()
                };
                Box::new(
                    parser_impl::NewCSVParser(&csv, reader, csv.header, None)
                        .map_err(|error| errors::Errorf(error.to_string()))?,
                )
            }
            mydump::SourceTypeSQL => {
                let reader: Box<dyn parser_impl::ReadSeekCloser> =
                    Box::new(parser_impl::StringReader::from_bytes(raw));
                Box::new(parser_impl::NewChunkParser(reader, 64 * 1024, None, false))
            }
            mydump::SourceTypeParquet => {
                return read_parquet_rows(raw, n);
            }
            source_type => {
                return Err(errors::Errorf(format!(
                    "unsupported source file type '{source_type}' for preview"
                )));
            }
        };
        let mut rows = Vec::new();
        for _ in 0..n {
            match parser.ReadRow() {
                Ok(()) => {
                    let row = parser.LastRow();
                    rows.push(
                        row.row
                            .into_iter()
                            .map(|datum| match datum {
                                parser_impl::Datum::I64(value) => Datum::Int(value),
                                parser_impl::Datum::Bytes(value)
                                | parser_impl::Datum::Binary(value) => Datum::Bytes(value),
                                parser_impl::Datum::Null => Datum::Bytes(b"\\N".to_vec()),
                            })
                            .collect(),
                    );
                }
                Err(parser_impl::MydumpError::Eof) => break,
                Err(error) => {
                    let _ = parser.Close();
                    return Err(errors::Errorf(error.to_string()));
                }
            }
        }
        let columns = parser.Columns().to_vec();
        parser
            .Close()
            .map_err(|error| errors::Errorf(error.to_string()))?;
        Ok((columns, rows))
    }

    // 自动补充的`EstimateSourceDataSize` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn EstimateSourceDataSize(
        &self,
        ctx: Context,
        opts: &[ropts::GetPreInfoOption],
    ) -> Result<EstimateSourceDataSizeResult> {
        let cfg = ropts::ApplyGetPreInfoOptions(None, opts);
        if !cfg.ForceReloadCache {
            if let Some(est) = self.estimated.lock().unwrap().clone() {
                return Ok(est);
            }
        }
        let mut size_without = 0i64;
        for db in &self.dbMetas {
            for tbl in &db.Tables {
                size_without += tbl.TotalSize;
            }
        }
        let size_with_index = if self.cfg.TikvImporter.Backend == crate::config::BackendLocal {
            (size_without as f64 / 3.0) as i64
        } else {
            size_without
        };
        let est = EstimateSourceDataSizeResult {
            SizeWithIndex: size_with_index,
            SizeWithoutIndex: size_without,
            HasUnsortedBigTables: false,
            TiFlashSize: 0,
        };
        *self.estimated.lock().unwrap() = Some(est.clone());
        let _ = ctx;
        Ok(est)
    }
}

fn read_parquet_rows(raw: Vec<u8>, n: i32) -> Result<(Vec<String>, Vec<Vec<Datum>>)> {
    let reader = SerializedFileReader::new(bytes::Bytes::from(raw))
        .map_err(|error| errors::Errorf(format!("open parquet source: {error}")))?;
    let columns = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .root_schema()
        .get_fields()
        .iter()
        .map(|field| field.name().to_string())
        .collect();
    let rows = reader
        .get_row_iter(None)
        .map_err(|error| errors::Errorf(format!("read parquet source: {error}")))?
        .take(n as usize)
        .map(|row| {
            row.map_err(|error| errors::Errorf(format!("read parquet row: {error}")))
                .map(|row| {
                    row.get_column_iter()
                        .map(|(_, field)| match field {
                            Field::Byte(value) => Datum::Int(i64::from(*value)),
                            Field::Short(value) => Datum::Int(i64::from(*value)),
                            Field::Int(value) => Datum::Int(i64::from(*value)),
                            Field::Long(value) => Datum::Int(*value),
                            Field::UByte(value) => Datum::Int(i64::from(*value)),
                            Field::UShort(value) => Datum::Int(i64::from(*value)),
                            Field::UInt(value) => Datum::Int(i64::from(*value)),
                            Field::ULong(value) => Datum::Bytes(value.to_string().into_bytes()),
                            Field::Str(value) => Datum::String(value.clone()),
                            Field::Bytes(value) => Datum::Bytes(value.data().to_vec()),
                            Field::Null => Datum::Bytes(b"\\N".to_vec()),
                            value => Datum::String(value.to_string()),
                        })
                        .collect()
                })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((columns, rows))
}

fn parse_csv_records(input: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = input.chars().peekable();
    let mut quoted = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => record.push(std::mem::take(&mut field)),
            '\n' if !quoted => {
                if field.ends_with('\r') {
                    field.pop();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            _ => field.push(ch),
        }
    }
    if quoted {
        return Err(errors::New("unterminated quoted CSV field"));
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

// 自动补充的下面的 `impl PreImportInfoGetterImpl` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl PreImportInfoGetterImpl {
    fn getTableStructuresByFileMeta(
        &self,
        _ctx: Context,
        dbSrcFileMeta: &mydump::MDDatabaseMeta,
        getPreInfoCfg: &ropts::GetPreInfoConfig,
    ) -> Result<Vec<model::TableInfo>> {
        let mut out = Vec::new();
        let remote = self
            .target
            .FetchRemoteTableModels(_ctx.clone(), &dbSrcFileMeta.Name)?;
        let remote: HashMap<String, model::TableInfo> = remote
            .into_iter()
            .map(|table| (table.Name.L.clone(), table))
            .collect();
        for (i, tbl) in dbSrcFileMeta.Tables.iter().enumerate() {
            if let Some(table) = remote.get(&tbl.Name.to_ascii_lowercase()) {
                out.push(table.clone());
                continue;
            }
            if let Some(schema) = &tbl.SchemaFile {
                let bytes = self.srcStorage.Read(&schema.Path).map_err(|error| {
                    errors::Errorf(format!(
                        "get create table statement from schema file error: {}: {error}",
                        tbl.Name
                    ))
                })?;
                let bytes = if schema.Compression == mydump::CompressionNone {
                    bytes
                } else if schema.Compression == 1 {
                    let mut decoded = Vec::new();
                    flate2::read::GzDecoder::new(bytes.as_slice())
                        .read_to_end(&mut decoded)
                        .map_err(|error| errors::Errorf(format!("decompress schema: {error}")))?;
                    decoded
                } else {
                    return Err(errors::Errorf(format!(
                        "unsupported schema compression '{}'",
                        schema.Compression
                    )));
                };
                let create = String::from_utf8(bytes)
                    .map_err(|error| errors::Errorf(format!("decode schema: {error}")))?;
                out.push(newTableInfo(&create, (i as i64) + 1)?);
            } else if getPreInfoCfg.IgnoreDBNotExist {
                out.push(model::TableInfo {
                    ID: (i as i64) + 1,
                    Name: model::CIStr::new(&tbl.Name),
                    State: model::StatePublic,
                    ..Default::default()
                });
            } else {
                out.push(model::TableInfo {
                    ID: (i as i64) + 1,
                    Name: model::CIStr::new(&tbl.Name),
                    State: model::StatePublic,
                    ..Default::default()
                });
            }
        }
        Ok(out)
    }
}

// 自动补充的`newTableInfo` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn newTableInfo(createTblSQL: &str, tableID: i64) -> Result<model::TableInfo> {
    let mut info = Parser::Parse(createTblSQL).map_err(errors::Trace)?;
    info.ID = tableID;
    info.State = model::StatePublic;
    Ok(info)
}

// re-export helper used by tidb/import without circular deps
pub mod cfg_helpers {
    use crate::config::Config;
    // 自动补充的`is_local_backend_cfg` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn is_local_backend_cfg(cfg: &Config) -> bool {
        cfg.TikvImporter.Backend == crate::config::BackendLocal
    }
}
