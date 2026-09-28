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

//! Foreign-key recording matching `br/pkg/restore/ingestrec/foreign_key.go`.
//!
//! 模块职责：在 ingest 恢复录制阶段收集表的外键与被引用外键，供后续重建/校验。
//! 对应 Go `foreign_key.go`；依赖 InfoSchema 查询 referred FK 与子表定义。
//! 约束：PK handle 单列且带主键标志的 FK 可跳过；Remove 按索引前缀覆盖删除。

use std::collections::HashMap;

use astersql_errors::SharedError;

use crate::model_stub::{
    CIStr, Context, FKInfo, FindColumnInfo, HasPriKeyFlag, IndexInfo, InfoSchema,
    IsIndexPrefixCoveredForForeignKey, TableInfo, trace,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// FK 记录主键：子库/子表/FK 名（O 原文），用于 HashMap 去重。
pub struct ForeignKeyRecordKey {
    pub ChildSchemaNameO: String,
    pub ChildTableNameO: String,
    pub FKNameO: String,
}

#[derive(Clone, Debug)]
/// 一条外键记录：完整 FKInfo 加上子表定位信息。
pub struct ForeignKeyRecord {
    pub FKInfo: FKInfo,
    pub ChildSchemaNameO: String,
    pub ChildTableNameO: String,
}

/// 从子表定位与 FKInfo 构造 (key, value) 对，Name 取 O 原文。
fn newForeignKeyRecordKey(
    childSchemaNameO: String,
    childTableNameO: String,
    fk: &FKInfo,
) -> (ForeignKeyRecordKey, ForeignKeyRecord) {
    (
        ForeignKeyRecordKey {
            ChildSchemaNameO: childSchemaNameO.clone(),
            ChildTableNameO: childTableNameO.clone(),
            FKNameO: fk.Name.O.clone(),
        },
        ForeignKeyRecord {
            FKInfo: fk.clone(),
            ChildSchemaNameO: childSchemaNameO,
            ChildTableNameO: childTableNameO,
        },
    )
}

#[derive(Default, Clone, Debug)]
/// 跨表合并后的全局 FK 集合（含 referred）。
pub struct ForeignKeyRecordManager {
    pub fkRecordMap: HashMap<ForeignKeyRecordKey, ForeignKeyRecord>,
}

impl ForeignKeyRecordManager {
    /// 空管理器。
    pub fn New() -> Self {
        Self {
            fkRecordMap: HashMap::new(),
        }
    }

    /// 把表级自身 FK 与 referred FK 一并并入全局 map（后者覆盖同键）。
    pub fn Merge(&mut self, tm: &TableForeignKeyRecordManager) {
        // 先合并本表 FK。
        for (k, v) in &tm.fkRecordMap {
            self.fkRecordMap.insert(k.clone(), v.clone());
        }
        // 再合并指向本表的 referred FK。
        for (k, v) in &tm.referredFKRecordMap {
            self.fkRecordMap.insert(k.clone(), v.clone());
        }
    }
}

/// Go `NewForeignKeyRecordManager`.
/// 包级构造入口，与方法 New 等价。
pub fn NewForeignKeyRecordManager() -> ForeignKeyRecordManager {
    ForeignKeyRecordManager::New()
}

#[derive(Default, Clone, Debug)]
/// 单表视角：自身 FK 与 referred FK 分图存放，供 Remove/Merge。
pub struct TableForeignKeyRecordManager {
    pub fkRecordMap: HashMap<ForeignKeyRecordKey, ForeignKeyRecord>,
    pub referredFKRecordMap: HashMap<ForeignKeyRecordKey, ForeignKeyRecord>,
}

/// 扫描 tableInfo.ForeignKeys 与 InfoSchema referred 列表，跳过 PK-handle 特例。
pub fn NewForeignKeyRecordManagerForTables(
    _ctx: &Context,
    infoSchema: &dyn InfoSchema,
    dbName: &CIStr,
    tableInfo: &TableInfo,
) -> Result<TableForeignKeyRecordManager, SharedError> {
    let mut tm = TableForeignKeyRecordManager::default();
    // 本表声明的外键；PKIsHandle 单列主键列上的 FK 可忽略。
    for tableFK in &tableInfo.ForeignKeys {
        // 与 Go 一致：单列且列带 PriKey 标志时 continue。
        if tableInfo.PKIsHandle && tableFK.Cols.len() == 1 {
            if let Some(refColInfo) = FindColumnInfo(&tableInfo.Columns, &tableFK.Cols[0].L) {
                if HasPriKeyFlag(refColInfo.GetFlag()) {
                    continue;
                }
            }
        }
        let (key, value) =
            newForeignKeyRecordKey(dbName.O.clone(), tableInfo.Name.O.clone(), tableFK);
        tm.fkRecordMap.insert(key, value);
    }
    // 查询引用本表的外键描述，再回子表取完整 FKInfo。
    let tableReferredFKs = infoSchema.GetTableReferredForeignKeys(&dbName.L, &tableInfo.Name.L);
    // referred 侧同样跳过 PK-handle 单列主键场景。
    for tableReferredFK in &tableReferredFKs {
        if tableInfo.PKIsHandle && tableReferredFK.Cols.len() == 1 {
            if let Some(refColInfo) = FindColumnInfo(&tableInfo.Columns, &tableReferredFK.Cols[0].L)
            {
                if HasPriKeyFlag(refColInfo.GetFlag()) {
                    continue;
                }
            }
        }
        // 按 ChildSchema/ChildTable 取子表，匹配 ChildFKName 后写入 referred 图。
        let childTableInfo = infoSchema
            .TableByName(&tableReferredFK.ChildSchema, &tableReferredFK.ChildTable)
            .map_err(trace)?;
        for tableFK in &childTableInfo.ForeignKeys {
            // 名称 O 原文相等才视为同一条 FK 定义。
            if tableReferredFK.ChildFKName.O == tableFK.Name.O {
                let (key, value) = newForeignKeyRecordKey(
                    tableReferredFK.ChildSchema.O.clone(),
                    tableReferredFK.ChildTable.O.clone(),
                    tableFK,
                );
                tm.referredFKRecordMap.insert(key, value);
                // 子表上同名 FK 只会有一条，找到即可 break。
                break;
            }
        }
    }
    Ok(tm)
}

impl TableForeignKeyRecordManager {
    /// 索引被删除/重建时：若索引前缀覆盖 FK 列（或 RefCols），则剔除对应记录。
    pub fn RemoveForeignKeys(&mut self, tableInfo: &TableInfo, indexInfo: &IndexInfo) {
        // 自身 FK 看 Cols 是否被索引前缀覆盖。
        self.fkRecordMap.retain(|_, fkRecord| {
            !IsIndexPrefixCoveredForForeignKey(tableInfo, indexInfo, &fkRecord.FKInfo.Cols)
        });
        // referred FK 看 RefCols（被引用列）与索引关系。
        self.referredFKRecordMap.retain(|_, fkRecord| {
            !IsIndexPrefixCoveredForForeignKey(tableInfo, indexInfo, &fkRecord.FKInfo.RefCols)
        });
    }
}
