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

//! Mock import source and target info matching Go `lightning/pkg/importer/mock`.
//!
//! 这个模块给 importer 相关测试提供两类可组合的假实现：
//! 一类是 `ImportSource`，把手写的数据库/表/文件树转换成 mydump 元数据和内存存储；
//! 另一类是 `TargetInfo`，模拟目标集群返回的系统变量、表结构、store 容量与 region 分布。
//! 这样测试既能覆盖 importer 读取源数据的入口，也能覆盖预检阶段对目标端的查询逻辑，
//! 同时完全避免真实对象存储、PD、TiKV 或 TiDB 依赖。

use std::collections::HashMap;

use astersql_lightning_pkg_importer_opts as ropts;

use crate::stubs::ast;
use crate::stubs::context::{self, Context};
use crate::stubs::dbterror;
use crate::stubs::errno;
use crate::stubs::errors;
use crate::stubs::filter;
use crate::stubs::model;
use crate::stubs::mydump::{
    self, Compression, FileInfo, MDDatabaseMeta, SourceFileMeta, SourceType,
};
use crate::stubs::objstore::{self, MemStorage};
use crate::stubs::pdhttp::{
    self, MetaStore, RegionInfo, RegionPeer, RegionsInfo, StoreInfo, StoreStatus, StoresInfo,
};
use crate::stubs::units;
use crate::stubs::{Error, Result};

// ---------------------------------------------------------------------------
// 源数据侧 mock：把测试描述转成 mydump 可消费的文件元数据。
// ---------------------------------------------------------------------------

/// SourceFile defines a mock source file.
/// 它同时保存字节内容和声明的总大小。
/// 当 `TotalSize` 为 0 时，构建逻辑会退回到真实字节长度。
#[derive(Clone, Debug, Default)]
pub struct SourceFile {
    pub FileName: String,
    pub Data: Vec<u8>,
    pub TotalSize: isize,
}

/// TableSourceData defines a mock source information for a table.
/// 表级 mock 数据由一个 schema 文件和若干数据文件组成。
/// 结构故意贴近 Go 测试里手工拼装输入的方式，降低迁移成本。
#[derive(Clone, Debug, Default)]
pub struct TableSourceData {
    pub DBName: String,
    pub TableName: String,
    pub SchemaFile: Option<Box<SourceFile>>,
    pub DataFiles: Vec<Box<SourceFile>>,
}

/// DBSourceData defines a mock source information for a database.
/// 一个数据库只持有其名下的表集合。
/// `Tables` 以表名索引，便于测试按名字构造和查找。
#[derive(Clone, Debug, Default)]
pub struct DBSourceData {
    pub Name: String,
    pub Tables: HashMap<String, Box<TableSourceData>>,
}

/// ImportSource defines a mock import source.
/// 它把“测试输入描述”与“供 importer 消费的派生元数据”同时保存下来。
/// 调用方既可以看原始 map，也可以直接读取 mydump 视图和内存存储。
#[derive(Clone, Debug)]
pub struct ImportSource {
    dbSrcDataMap: HashMap<String, Box<DBSourceData>>,
    dbFileMetaMap: HashMap<String, Box<MDDatabaseMeta>>,
    srcStorage: MemStorage,
}

/// NewImportSource creates a ImportSource object.
/// 构造过程会遍历数据库和表描述，生成与 Go 相同的大体 mydump 元数据形状。
/// 同时每个 schema/data 文件都会写入 `MemStorage`，让后续测试可以按路径回读。
/// 如果数据文件扩展名不在支持集合里，直接返回错误，避免把无效输入静默吞掉。
pub fn NewImportSource(
    dbSrcDataMap: HashMap<String, Box<DBSourceData>>,
) -> Result<Box<ImportSource>> {
    let ctx = context::Background();
    // 一个 `dbFileMetaMap` 条目对应一个数据库级 mydump 元数据节点。
    let mut dbFileMetaMap: HashMap<String, Box<MDDatabaseMeta>> = HashMap::new();
    // 所有文件都写入同一个内存对象存储，模拟 importer 通过统一 storage 读取源文件。
    let mapStore = objstore::NewMemStorage();
    for (dbName, dbData) in &dbSrcDataMap {
        // 先为数据库本身创建 schema 元数据，再逐表追加子项。
        let dbFileInfo = FileInfo {
            TableName: filter::Table {
                Schema: dbName.clone(),
                Name: String::new(),
            },
            FileMeta: SourceFileMeta {
                Type: SourceType::SchemaSchema,
                ..Default::default()
            },
        };
        let mut dbMeta = mydump::NewMDDatabaseMeta("binary");
        // 数据库级字符集固定写成 `binary`，与现有 Go mock 一致，不引入额外编码变量。
        dbMeta.Name = dbName.clone();
        dbMeta.SchemaFile = dbFileInfo;
        dbMeta.Tables = Vec::new();
        for (tblName, tblData) in &dbData.Tables {
            // 这里保持与 Go 一样的假设：表必须有 schema 文件，否则测试输入无效。
            let schema_file = tblData
                .SchemaFile
                .as_ref()
                .expect("SchemaFile required like Go non-nil *SourceFile");
            let mut tblMeta = mydump::NewMDTableMeta("binary");
            // 表级元数据同样采用固定字符集，只强调文件布局而非真实解析编码。
            tblMeta.DB = dbName.clone();
            tblMeta.Name = tblName.clone();
            let mut compression = Compression::None;
            if schema_file.FileName.ends_with(".gz") {
                compression = Compression::GZ;
            }
            tblMeta.SchemaFile = FileInfo {
                TableName: filter::Table {
                    Schema: dbName.clone(),
                    Name: tblName.clone(),
                },
                FileMeta: SourceFileMeta {
                    Path: schema_file.FileName.clone(),
                    Type: SourceType::TableSchema,
                    Compression: compression,
                    ..Default::default()
                },
            };
            tblMeta.DataFiles = Vec::new();
            // schema 文件和 data 文件都写入同一份内存存储，便于按路径回读校验。
            if let Err(err) = mapStore.WriteFile(&ctx, &schema_file.FileName, &schema_file.Data) {
                return Err(errors::Trace(err));
            }
            let mut totalFileSize = 0isize;
            for tblDataFile in &tblData.DataFiles {
                // Go 测试允许显式覆盖文件大小；未覆盖时退回到真实字节数。
                let mut fileSize = tblDataFile.TotalSize;
                if fileSize == 0 {
                    fileSize = tblDataFile.Data.len() as isize;
                }
                totalFileSize = totalFileSize.wrapping_add(fileSize);
                let mut fileInfo = FileInfo {
                    TableName: filter::Table {
                        Schema: dbName.clone(),
                        Name: tblName.clone(),
                    },
                    FileMeta: SourceFileMeta {
                        Path: tblDataFile.FileName.clone(),
                        FileSize: fileSize as i64,
                        RealSize: fileSize as i64,
                        ..Default::default()
                    },
                };
                let mut fileName = tblDataFile.FileName.clone();
                if let Some(uncompressed_name) = fileName.strip_suffix(".gz") {
                    // 压缩格式由后缀驱动，但实际文件类型仍依据去掉 `.gz` 后的名称判断。
                    fileName = uncompressed_name.to_string();
                    fileInfo.FileMeta.Compression = Compression::GZ;
                }
                if fileName.ends_with(".csv") {
                    // CSV/SQL/Parquet 三类路径覆盖 importer 在测试里关心的主要文件类型。
                    fileInfo.FileMeta.Type = SourceType::CSV;
                } else if fileName.ends_with(".sql") {
                    fileInfo.FileMeta.Type = SourceType::SQL;
                } else if fileName.ends_with(".parquet") {
                    fileInfo.FileMeta.Type = SourceType::Parquet;
                } else {
                    // 其他扩展名一律视为测试输入错误，便于 parity 用例直接断言失败消息。
                    return Err(errors::Errorf(format!(
                        "unsupported file type: {}",
                        tblDataFile.FileName
                    )));
                }
                tblMeta.DataFiles.push(fileInfo);
                if let Err(err) = mapStore.WriteFile(&ctx, &tblDataFile.FileName, &tblDataFile.Data)
                {
                    return Err(errors::Trace(err));
                }
            }
            tblMeta.TotalSize = totalFileSize as i64;
            dbMeta.Tables.push(tblMeta);
        }
        dbFileMetaMap.insert(dbName.clone(), Box::new(dbMeta));
    }
    Ok(Box::new(ImportSource {
        dbSrcDataMap,
        dbFileMetaMap,
        srcStorage: mapStore,
    }))
}

// `ImportSource` 的 getter 基本都只暴露构造时生成的缓存，
// 不在读取路径上再做任何派生或 IO。
impl ImportSource {
    /// GetStorage gets the External Storage object on the mock source.
    /// 返回克隆后的 `MemStorage` 句柄，读者与源对象共享同一底层文件表。
    pub fn GetStorage(&self) -> MemStorage {
        self.srcStorage.clone()
    }

    /// GetDBMetaMap gets the Mydumper database metadata map on the mock source.
    /// 该接口暴露按数据库索引的元数据视图，适合断言构造结果的整体结构。
    pub fn GetDBMetaMap(&self) -> &HashMap<String, Box<MDDatabaseMeta>> {
        &self.dbFileMetaMap
    }

    /// GetAllDBFileMetas gets all the Mydumper database metadatas on the mock source.
    /// 这里返回借用切片的扁平集合，贴合 Go 测试常见的“遍历所有 DB 元数据”方式。
    pub fn GetAllDBFileMetas(&self) -> Vec<&MDDatabaseMeta> {
        let mut result = Vec::with_capacity(self.dbFileMetaMap.len());
        for dbMeta in self.dbFileMetaMap.values() {
            result.push(dbMeta.as_ref());
        }
        result
    }

    /// Same-package access used by Go tests (`mockEnv.srcStorage.ReadFile`).
    /// Rust 通过显式 getter 替代 Go 的同包字段访问。
    pub fn src_storage(&self) -> &MemStorage {
        &self.srcStorage
    }
}

// ---------------------------------------------------------------------------
// 目标端 mock：为 precheck/get-pre-info 暴露最小查询面。
// ---------------------------------------------------------------------------

/// StorageInfo defines the storage information for a mock target.
/// 它描述单个 store 的容量、已用空间和 region 数量。
#[derive(Clone, Debug, Default)]
pub struct StorageInfo {
    pub TotalSize: u64,
    pub UsedSize: u64,
    pub AvailableSize: u64,
    pub RegionCount: isize,
}

/// TableInfo defines a mock table structure information for a mock target.
/// `RowCount` 用于判断空表，`TableModel` 用于模拟远端返回的结构信息。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub RowCount: isize,
    pub TableModel: Option<Box<model::TableInfo>>,
}

/// TargetInfo defines a mock target information.
/// 该类型实现 importer 预检需要的目标端查询面。
/// 所有返回值都来源于本地字段，不触发任何网络或数据库交互。
#[derive(Clone, Debug, Default)]
pub struct TargetInfo {
    pub MaxReplicasPerRegion: isize,
    pub EmptyRegionCountMap: HashMap<u64, isize>,
    pub StorageInfos: Vec<StorageInfo>,
    sysVarMap: HashMap<String, String>,
    dbTblInfoMap: HashMap<String, HashMap<String, Box<TableInfo>>>,
}

/// NewTargetInfo creates a TargetInfo object.
/// 默认实例从空系统变量、空表信息和空 store 列表开始，
/// 测试可以按需逐项填充关心的维度。
pub fn NewTargetInfo() -> Box<TargetInfo> {
    Box::new(TargetInfo {
        StorageInfos: Vec::new(),
        sysVarMap: HashMap::new(),
        dbTblInfoMap: HashMap::new(),
        ..Default::default()
    })
}

// 与 Go 一样，`TargetInfo` 更像可编程的响应录制器：
// 先由测试填值，再让 importer 按接口读取这些值。
impl TargetInfo {
    /// SetSysVar sets the system variables of the mock target.
    /// 系统变量使用普通 map 保存，覆盖写入行为与 Go `map` 一致。
    pub fn SetSysVar(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.sysVarMap.insert(key.into(), value.into());
    }

    /// SetTableInfo sets the table structure information of the mock target.
    /// 首层按 schema 聚合，二层按表名存储，方便分别模拟“库不存在”和“表不存在”。
    pub fn SetTableInfo(
        &mut self,
        schemaName: impl Into<String>,
        tableName: impl Into<String>,
        tblInfo: Box<TableInfo>,
    ) {
        let schemaName = schemaName.into();
        let tableName = tableName.into();
        self.dbTblInfoMap
            .entry(schemaName)
            .or_default()
            .insert(tableName, tblInfo);
    }

    /// FetchRemoteDBModels implements the TargetInfoGetter interface.
    /// 该接口只把 schema 名字投影成最小 `DBInfo` 列表，
    /// 足以支撑 importer 在预检阶段做库级枚举。
    pub fn FetchRemoteDBModels(&self, _ctx: &Context) -> Result<Vec<Box<model::DBInfo>>> {
        let mut resultInfos = Vec::new();
        // 这里只关心库名，不模拟额外字段，避免把 mock 变成沉重的 schema 副本。
        for dbName in self.dbTblInfoMap.keys() {
            // `ast::NewCIStr` 让大小写字段同时具备，方便复用真实代码的比较逻辑。
            resultInfos.push(Box::new(model::DBInfo {
                Name: ast::NewCIStr(dbName.clone()),
            }));
        }
        Ok(resultInfos)
    }

    /// FetchRemoteTableModels fetches the table structures from the remote target.
    /// It implements the TargetInfoGetter interface.
    /// 若 schema 不存在，这里故意返回带 HTTP 外壳的错误文案，保持与 Go 测试期望一致。
    /// 对于存在但没有结构的表，会插入 `None`，模拟 Go map 中的 nil 值。
    pub fn FetchRemoteTableModels(
        &self,
        _ctx: &Context,
        schemaName: &str,
        tableNames: &[String],
    ) -> Result<HashMap<String, Option<Box<model::TableInfo>>>> {
        let Some(tblMap) = self.dbTblInfoMap.get(schemaName) else {
            // 错误文本保留 Go 风格的“HTTP 请求失败 + 内层 DB 错误”拼接形式。
            let dbNotExistErr = dbterror::ClassSchema
                .NewStd(errno::ErrBadDB)
                .FastGenByArgs(schemaName);
            return Err(errors::Errorf(format!(
                "get xxxxxx http status code != 200, message {}",
                dbNotExistErr.Error()
            )));
        };
        let mut ret = HashMap::with_capacity(tableNames.len());
        for tableName in tableNames {
            // 未命中的表不会主动补 `Some(default)`，而是沿用 Go 的“缺项或 nil”语义。
            if let Some(tblInfo) = tblMap.get(tableName) {
                ret.insert(tableName.clone(), tblInfo.TableModel.clone());
            }
        }
        Ok(ret)
    }

    /// GetTargetSysVariablesForImport gets some important systam variables for importing on the target.
    /// It implements the TargetInfoGetter interface.
    /// 返回克隆结果，确保调用方修改拿到的 map 时不会反向污染内部状态。
    pub fn GetTargetSysVariablesForImport(
        &self,
        _ctx: &Context,
        _opts: &[ropts::GetPreInfoOption],
    ) -> HashMap<String, String> {
        self.sysVarMap.clone()
    }

    /// GetMaxReplica implements the TargetInfoGetter interface.
    /// Go 版本把非正副本数视为未配置；这里同样回退到 1。
    pub fn GetMaxReplica(&self, _ctx: &Context) -> Result<u64> {
        let mut replCount = self.MaxReplicasPerRegion;
        if replCount <= 0 {
            replCount = 1;
        }
        Ok(replCount as u64)
    }

    /// GetStorageInfo gets the storage information on the target.
    /// It implements the TargetInfoGetter interface.
    /// store ID 由遍历顺序派生，重点是给预检逻辑提供容量和已用空间的稳定输入。
    pub fn GetStorageInfo(&self, _ctx: &Context) -> Result<Box<StoresInfo>> {
        let mut resultStoreInfos = Vec::with_capacity(self.StorageInfos.len());
        for (i, storeInfo) in self.StorageInfos.iter().enumerate() {
            // store 序号从 1 开始生成，使断言更接近真实集群返回值。
            resultStoreInfos.push(StoreInfo {
                Store: MetaStore {
                    ID: (i as i64) + 1,
                    StateName: "Up".to_string(),
                },
                Status: StoreStatus {
                    Capacity: units::BytesSize(storeInfo.TotalSize as f64),
                    Available: units::BytesSize(storeInfo.AvailableSize as f64),
                    RegionSize: storeInfo.UsedSize as i64,
                    RegionCount: storeInfo.RegionCount as i64,
                },
            });
        }
        Ok(Box::new(StoresInfo {
            Count: resultStoreInfos.len(),
            Stores: resultStoreInfos,
        }))
    }

    /// GetEmptyRegionsInfo gets the region information of all the empty regions on the target.
    /// It implements the TargetInfoGetter interface.
    /// 每个空 region 都只挂一个 peer，足以模拟“某 store 拥有多少空 region”的统计语义。
    pub fn GetEmptyRegionsInfo(&self, _ctx: &Context) -> Result<Box<RegionsInfo>> {
        let mut totalEmptyRegions: Vec<RegionInfo> = Vec::new();
        let mut totalEmptyRegionCount = 0isize;
        for (&storeID, &storeEmptyRegionCount) in &self.EmptyRegionCountMap {
            let storeEmptyRegionCount =
                usize::try_from(storeEmptyRegionCount).expect("makeslice: len out of range");
            // 一个计数展开成多个 region 条目，便于调用方按长度和 peer store 断言。
            for _ in 0..storeEmptyRegionCount {
                totalEmptyRegions.push(RegionInfo {
                    Peers: vec![RegionPeer {
                        StoreID: storeID as i64,
                    }],
                });
            }
            totalEmptyRegionCount =
                totalEmptyRegionCount.wrapping_add(storeEmptyRegionCount as isize);
        }
        Ok(Box::new(RegionsInfo {
            Count: totalEmptyRegionCount as i64,
            Regions: totalEmptyRegions,
        }))
    }

    /// IsTableEmpty checks whether the specified table on the target DB contains data or not.
    /// It implements the TargetInfoGetter interface.
    /// 缺库或缺表都按“空表”处理，这与预检阶段的宽松探测逻辑一致。
    pub fn IsTableEmpty(
        &self,
        _ctx: &Context,
        schemaName: &str,
        tableName: &str,
    ) -> Result<Box<bool>> {
        let Some(tblInfoMap) = self.dbTblInfoMap.get(schemaName) else {
            // 预检把“远端不存在”视为无需额外处理的空表分支。
            return Ok(Box::new(true));
        };
        let Some(tblInfo) = tblInfoMap.get(tableName) else {
            return Ok(Box::new(true));
        };
        Ok(Box::new(tblInfo.RowCount == 0))
    }

    /// CheckVersionRequirements performs the check whether the target satisfies the version requirements.
    /// It implements the TargetInfoGetter interface.
    /// 当前 mock 不模拟真实版本门槛，只保留一个恒成功入口供上层流程调用。
    pub fn CheckVersionRequirements(&self, _ctx: &Context) -> Result<()> {
        // 额外构造一个默认 `StoresInfo` 只为保持与 Go 代码里引用路径相近的依赖面。
        let _ = pdhttp::StoresInfo::default();
        Ok(())
    }
}

// Silence unused import of Error when only Result is used in signatures above.
#[allow(dead_code)]
fn _error_ty(_: Error) {}
