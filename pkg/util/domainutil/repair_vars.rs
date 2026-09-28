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

// Domain 修复模式（repair mode）状态变量与 API。
//
// 对应 Go `pkg/util/domainutil/repair_vars.go`。当 TiDB 元数据损坏时，可进入
// repair mode，仅加载指定 `db.table` 列表以便 `ADMIN REPAIR TABLE`。本模块维护
// 待修复表名、已缓存的 `DBInfo`/`TableInfo`，以及 sessionCtx 用的缓存键枚举。

use crate::model;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

/// 修复模式运行时状态：待修复表列表、已缓存 DBInfo 与开关。
// repairInfo 对应 Go 的同名结构体，保存 repair mode、待修复表名列表和已缓存的 DBInfo。
// Go 结构体内嵌 sync.RWMutex；Rust 实现把锁放在全局 RepairInfo 外层，字段顺序保留主要数据字段。
#[derive(Default)]
pub struct repairInfo {
    repairDBInfoMap: HashMap<i64, model::DBInfo>,
    repairTableList: Vec<String>,
    repairMode: bool,
}

impl repairInfo {
    // new 对应 Go init 中对 repairInfo 零值后的显式初始化。
    fn new() -> Self {
        Self {
            repairDBInfoMap: HashMap::new(),
            repairTableList: Vec::new(),
            repairMode: false,
        }
    }

    // InRepairMode indicates whether TiDB is in repairMode.
    // InRepairMode 对应 Go 的 RLock/RUnlock 读路径；锁由外层 RwLock 读守卫表达。
    pub fn InRepairMode(&self) -> bool {
        self.repairMode
    }

    // SetRepairMode sets whether TiDB is in repairMode.
    // SetRepairMode 对应 Go 的写锁保护赋值。
    pub fn SetRepairMode(&mut self, mode: bool) {
        self.repairMode = mode;
    }

    // GetRepairTableList gets repairing table list.
    // GetRepairTableList 返回当前修复表列表；Go 返回 slice，Rust 实现返回借用以保留只读观察语义。
    pub fn GetRepairTableList(&self) -> &[String] {
        &self.repairTableList
    }

    // GetMustLoadRepairTableListByDB gets must load repair table ID list.
    // 该函数先按 dbName 前缀筛选 repairTableList，再遍历大小写敏感的 tableName2ID 反查表 ID。
    pub fn GetMustLoadRepairTableListByDB(
        &self,
        dbName: &str,
        tableName2ID: &HashMap<String, i64>,
    ) -> Vec<i64> {
        let dbNamePrefix = format!("{}.", dbName);
        let mut repairTableSet: HashSet<String> =
            HashSet::with_capacity(self.repairTableList.len());
        for fullTableName in &self.repairTableList {
            let lowerFullTableName = fullTableName.to_lowercase();
            if lowerFullTableName.starts_with(&dbNamePrefix) {
                repairTableSet.insert(lowerFullTableName);
            }
        }

        let mut tableIDList = Vec::new();
        // tableName2ID is case sensitive and needs to be traversed to match the table id
        // Go 这里不能直接用 map lookup，因为 tableName2ID 保留原大小写；Rust 实现保持遍历比较。
        for (tableName, id) in tableName2ID {
            let fullName = format!("{}.{}", dbName, tableName);
            if repairTableSet.contains(&fullName.to_lowercase()) {
                tableIDList.push(*id);
            }
        }
        tableIDList
    }

    // SetRepairTableList sets repairing table list.
    // SetRepairTableList 会原地转小写后替换列表，保持 Go 修改传入 slice 元素的效果。
    pub fn SetRepairTableList(&mut self, mut list: Vec<String>) {
        for one in &mut list {
            *one = one.to_lowercase();
        }
        self.repairTableList = list;
    }

    // CheckAndFetchRepairedTable fetches the repairing table list from meta, true indicates fetch success.
    // 该方法在 repairMode 开启时匹配 db.table，命中后把修复表缓存到 repairDBInfoMap。
    pub fn CheckAndFetchRepairedTable(
        &mut self,
        di: &model::DBInfo,
        tbl: Arc<model::TableInfo>,
    ) -> bool {
        if !self.repairMode {
            return false;
        }

        let repairedName = format!("{}.{}", di.Name.L, tbl.Name.L);
        let mut isRepair = false;
        for tn in &self.repairTableList {
            // Use dbName and tableName to specify a table.
            // Go 每次比较都对 repairTableList 项转小写，Rust 实现保留这个宽容匹配。
            if tn.to_lowercase() == repairedName {
                isRepair = true;
                break;
            }
        }

        if isRepair {
            // Record the repaired table in Map.
            if let Some(repairedDB) = self.repairDBInfoMap.get_mut(&di.ID) {
                repairedDB.Deprecated.Tables.push(tbl);
            } else {
                // Shallow copy the DBInfo.
                let mut repairedDB = di.Copy();
                // Clean the tables and set repaired table.
                repairedDB.Deprecated.Tables = vec![tbl];
                self.repairDBInfoMap.insert(di.ID, repairedDB);
            }
            return true;
        }
        false
    }

    // GetRepairedTableInfoByTableName is exported for test.
    // 返回值对应 Go 的 (*model.TableInfo, *model.DBInfo)：未命中表但命中库时返回 (None, Some(db))。
    pub fn GetRepairedTableInfoByTableName(
        &self,
        schemaLowerName: &str,
        tableLowerName: &str,
    ) -> (Option<&Arc<model::TableInfo>>, Option<&model::DBInfo>) {
        for db in self.repairDBInfoMap.values() {
            if db.Name.L != schemaLowerName {
                continue;
            }
            for t in &db.Deprecated.Tables {
                if t.Name.L == tableLowerName {
                    return (Some(t), Some(db));
                }
            }
            return (None, Some(db));
        }
        (None, None)
    }

    // RemoveFromRepairInfo remove the table from repair info when repaired.
    // RemoveFromRepairInfo 同时从修复表名列表和已缓存 DBInfo 中移除表，最后按 map 是否为空关闭 repairMode。
    pub fn RemoveFromRepairInfo(&mut self, schemaLowerName: &str, tableLowerName: &str) {
        let repairedLowerName = format!("{}.{}", schemaLowerName, tableLowerName);
        // Remove from the repair list.
        if let Some(i) = self
            .repairTableList
            .iter()
            .position(|rt| rt.to_lowercase() == repairedLowerName)
        {
            // Go 使用 slices.Delete(list, i, i+1)；Vec::remove 保留同样的单元素删除效果。
            self.repairTableList.remove(i);
        }

        // Remove from the repair map.
        let dbID = self
            .repairDBInfoMap
            .iter()
            .find_map(|(id, db)| (db.Name.L == schemaLowerName).then_some(*id));
        if let Some(dbID) = dbID {
            let mut shouldRemoveDB = false;
            if let Some(db) = self.repairDBInfoMap.get_mut(&dbID) {
                let tables = &mut db.Deprecated.Tables;
                if let Some(j) = tables.iter().position(|t| t.Name.L == tableLowerName) {
                    tables.remove(j);
                }
                shouldRemoveDB = tables.is_empty();
            }
            if shouldRemoveDB {
                self.repairDBInfoMap.remove(&dbID);
            }
        }

        if self.repairDBInfoMap.is_empty() {
            self.repairMode = false;
        }
    }
}

/// 包级全局修复状态；外层 `RwLock` 对应 Go 内嵌 `sync.RWMutex`。
// RepairInfo indicates the repaired table info.
// RepairInfo 对应 Go 的包级全局变量；外层 RwLock 承担 Go 内嵌 sync.RWMutex 的并发保护职责。
pub static RepairInfo: LazyLock<RwLock<repairInfo>> =
    LazyLock::new(|| RwLock::new(repairInfo::new()));

/// sessionCtx 中缓存“待修复表/库”所用的键类型（对应 Go iota 枚举）。
// repairKeyType is keyType for admin repair table.
// repairKeyType 对应 Go 的 int 枚举；Rust 实现用 enum 表达两个 iota 值。
pub enum repairKeyType {
    // RepairedTable is the key type, caching the target repaired table in sessionCtx.
    /// 缓存目标修复表的 sessionCtx 键。
    RepairedTable,
    // RepairedDatabase is the key type, caching the target repaired database in sessionCtx.
    /// 缓存目标修复库的 sessionCtx 键。
    RepairedDatabase,
}

impl repairKeyType {
    // String 对应 Go 的 (repairKeyType).String，返回 sessionCtx 缓存键名。
    pub fn String(&self) -> &'static str {
        match self {
            repairKeyType::RepairedTable => "RepairedTable",
            repairKeyType::RepairedDatabase => "RepairedDatabase",
        }
    }
}

/// 构造零值 `repairInfo`（对应 Go 包 init；全局实例由 `RepairInfo` 的 LazyLock 完成）。
// init 对应 Go 的包初始化函数；Rust 中实际初始化由 RepairInfo 的 LazyLock 完成。
pub fn init() -> repairInfo {
    repairInfo::new()
}
