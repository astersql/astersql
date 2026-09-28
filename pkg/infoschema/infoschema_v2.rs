// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema V2：基于版本化（MVCC 风格）元数据与表缓存的实现。
//
// 相对 V1，V2 将库/表/分区/外键历史按 schema 版本追加记录（含 tomb 删除标记），
// 查询时取 `schema_version <= 快照版本` 的最新可见记录；表实体可经 SIEVE 缓存加速。
// 不发起 SQL / 网络 IO；机械草稿保留在上方块注释中。

// 原 Go 中可能读取系统表或存储快照的调用仅保留调用形状；本文件不会执行 SQL、网络 IO 或后台任务。
// 指针、接口、锁、原子指针、singleflight、context 与所有权无法直接一比一表达时，均在相邻位置补充中文说明。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicInt64, AtomicUint64, Ordering};

// Go imports（不虚构尚未可用的 Rust crate 路径）：
// "context"
// "fmt"
// "math"
// "slices"
// "sort"
// "strings"
// "sync"
// "sync/atomic"
// "time"
// "github.com/google/btree"
// "github.com/ngaut/pools"
// "github.com/pingcap/errors"
// "github.com/pingcap/failpoint"
// "github.com/pingcap/tidb/pkg/ddl/placement"
// infoschemacontext "github.com/pingcap/tidb/pkg/infoschema/context"
// "github.com/pingcap/tidb/pkg/kv"
// "github.com/pingcap/tidb/pkg/meta"
// "github.com/pingcap/tidb/pkg/meta/autoid"
// "github.com/pingcap/tidb/pkg/meta/metadef"
// "github.com/pingcap/tidb/pkg/meta/model"
// "github.com/pingcap/tidb/pkg/metrics"
// "github.com/pingcap/tidb/pkg/parser/ast"
// "github.com/pingcap/tidb/pkg/parser/terror"
// "github.com/pingcap/tidb/pkg/table"
// "github.com/pingcap/tidb/pkg/util/logutil"
// "github.com/pingcap/tidb/pkg/util/size"
// "github.com/pingcap/tidb/pkg/util/tracing"
// "go.uber.org/zap"
// "golang.org/x/sync/singleflight"

// tableItem is the btree item sorted by name or by id.
// tableItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct tableItem {
    pub dbName: ast::CIStr,
    pub dbID: i64,
    pub tableName: ast::CIStr,
    pub tableID: i64,
    pub schemaVersion: i64,
    pub tomb: bool,
}

// schemaItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct schemaItem {
    pub schemaVersion: i64,
    pub dbInfo: &mut model::DBInfo,
    pub tomb: bool,
}

// schemaIDName 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct schemaIDName {
    pub schemaVersion: i64,
    pub id: i64,
    pub name: ast::CIStr,
    pub tomb: bool,
}

// referredForeignKeyItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct referredForeignKeyItem {
    pub schemaVersion: i64,
    pub dbName: String,
    pub tableName: String,
    pub referredFKInfo: Vec<&mut model::ReferredFKInfo>,
    pub tomb: bool,
}

// Name 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn Name(si: &mut schemaItem) -> String {
    return si.dbInfo.Name.L
}

// btreeSet updates the btree.
// Concurrent write is supported, but should be avoided as much as possible.
// btreeSet 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn btreeSet<T: Any>(ptr: &mut atomic::Pointer[btree::BTreeG[T]], item: T) {
    let mut succ = false
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for !succ {
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        var t = ptr.Load()
        let mut t2 = t.Clone()
        t2.ReplaceOrInsert(item)
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        succ = ptr.CompareAndSwap(t, t2)
        if !succ {
            logutil.BgLogger().Info("infoschema v2 btree concurrently multiple writes detected, this should be rare")
        }
    }
}

// Data is the core data struct of infoschema V2.
// Data 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct Data {
    // For the TableByName API, sorted by {dbName, tableName, schemaVersion} => tableID
    // If the schema version +1 but a specific table does not change, the old record is
    // kept and no new {dbName, tableName, schemaVersion+1} => tableID record been added.
    // It means as long as we can find an item in it, the item is available, even through the
    // schema version maybe smaller than required.
    pub byName: atomic::Pointer[btree::BTreeG[&mut tableItem]],

    // For the TableByID API, sorted by {tableID, schemaVersion} => dbID
    // To reload model.TableInfo, we need both table ID and database ID for meta kv API.
    // It provides the tableID => databaseID mapping.
    // This mapping MUST be synced with byName.
    pub byID: atomic::Pointer[btree::BTreeG[&mut tableItem]],

    // For the SchemaByName API, sorted by {dbName, schemaVersion} => model.DBInfo
    // Stores the full data in memory.
    pub schemaMap: atomic::Pointer[btree::BTreeG[schemaItem]],

    // For the SchemaByID API, sorted by {id, schemaVersion}
    // Stores only id, name and schemaVersion in memory.
    pub schemaID2Name: atomic::Pointer[btree::BTreeG[schemaIDName]],

    // referredForeignKeys records all table's ReferredFKInfo.
    pub referredForeignKeys: atomic::Pointer[btree::BTreeG[&mut referredForeignKeyItem]],

    pub tableCache: &mut Sieve[tableCacheKey, table::Table],

    // For information_schema/metrics_schema/performance_schema etc
    pub specials: sync::Map,

    // pid2tid is used by FindTableInfoByPartitionID, it stores {partitionID, schemaVersion} => table ID
    // Need full data in memory!
    pub pid2tid: atomic::Pointer[btree::BTreeG[partitionItem]],

    // tableInfoResident stores {dbName, tableID, schemaVersion} => model.TableInfo
    // It is part of the model.TableInfo data kept in memory to accelerate the list tables API.
    // We observe the pattern that list table API always come with filter.
    // All model.TableInfo with special attributes are here, currently the special attributes including:
    //     TTLInfo, TiFlashReplica
    // PlacementPolicyRef, Partition might be added later, and also TableLock etc
    pub tableInfoResident: atomic::Pointer[btree::BTreeG[tableInfoItem]],

    // the minimum ts of the recent used infoschema
    pub recentMinTS: atomic::Uint64,
}

// tableInfoItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct tableInfoItem {
    pub dbName: ast::CIStr,
    pub tableID: i64,
    pub schemaVersion: i64,
    pub tableInfo: &mut model::TableInfo,
    pub tomb: bool,
}

// partitionItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct partitionItem {
    pub partitionID: i64,
    pub schemaVersion: i64,
    pub tableID: i64,
    pub tomb: bool,
}

// tableCacheKey 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct tableCacheKey {
    pub tableID: i64,
    pub schemaVersion: i64,
}

// btreeDegree 保留 Go 常量值及其索引/容量语义。
pub const btreeDegree: i32 = 16;

// NewData creates an infoschema V2 data struct.
// NewData 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn NewData() -> &mut Data {
    let mut ret = &Data{
        tableCache: newSieve[tableCacheKey, table.Table](1024 * 1024 * size.MB),
    }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.byID.Store(btree.NewG[*tableItem](btreeDegree, compareByID))
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.byName.Store(btree.NewG[*tableItem](btreeDegree, compareByName))
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.schemaMap.Store(btree.NewG[schemaItem](btreeDegree, compareSchemaItem))
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.schemaID2Name.Store(btree.NewG[schemaIDName](btreeDegree, compareSchemaByID))
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.pid2tid.Store(btree.NewG[partitionItem](btreeDegree, comparePartitionItem))
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.tableInfoResident.Store(btree.NewG[tableInfoItem](btreeDegree, compareTableInfoItem))
    ret.tableCache.SetStatusHook(newSieveStatusHookImpl())
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    ret.referredForeignKeys.Store(btree.NewG[*referredForeignKeyItem](btreeDegree, compareReferredForeignKeyItem))
    return ret
}

// CacheCapacity is exported for testing.
// CacheCapacity 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn CacheCapacity(isd: &mut Data) -> u64 {
    return isd.tableCache.Capacity()
}

// SetCacheCapacity sets the cache capacity size in bytes.
// SetCacheCapacity 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn SetCacheCapacity(isd: &mut Data, capacity: u64) {
    isd.tableCache.SetCapacityAndWaitEvict(capacity)
}

// add 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn add(isd: &mut Data, item: tableItem, tbl: table::Table) {
    btreeSet(&isd.byID, &item)
    btreeSet(&isd.byName, &item)
    isd.tableCache.Set(tableCacheKey{item.tableID, item.schemaVersion}, tbl)
    let mut ti = tbl.Meta()
    if pi := ti.GetPartitionInfo(); pi != None {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, def := range pi.Definitions {
            btreeSet(&isd.pid2tid, partitionItem{def.ID, item.schemaVersion, tbl.Meta().ID, false})
        }
    }
    if infoschemacontext.HasSpecialAttributes(ti) {
        btreeSet(&isd.tableInfoResident, tableInfoItem{
            dbName:        item.dbName,
            tableID:       item.tableID,
            schemaVersion: item.schemaVersion,
            tableInfo:     ti,
            tomb:          false})
    }
}

// addSpecialDB 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn addSpecialDB(isd: &mut Data, di: &mut model::DBInfo, tables: &mut schemaTables) {
    isd.specials.LoadOrStore(di.Name.L, tables)
}

// addDB 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn addDB(isd: &mut Data, schemaVersion: i64, dbInfo: &mut model::DBInfo) {
    dbInfo.Deprecated.Tables = None
    btreeSet(&isd.schemaID2Name, schemaIDName{schemaVersion: schemaVersion, id: dbInfo.ID, name: dbInfo.Name})
    btreeSet(&isd.schemaMap, schemaItem{schemaVersion: schemaVersion, dbInfo: dbInfo})
}

// remove 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn remove(isd: &mut Data, item: tableItem) {
    item.tomb = true
    btreeSet(&isd.byID, &item)
    btreeSet(&isd.byName, &item)
    btreeSet(&isd.tableInfoResident, tableInfoItem{
        dbName:        item.dbName,
        tableID:       item.tableID,
        schemaVersion: item.schemaVersion,
        tableInfo:     None,
        tomb:          true})
    isd.tableCache.Remove(tableCacheKey{item.tableID, item.schemaVersion})
}

// deleteDB 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn deleteDB(isd: &mut Data, dbInfo: &mut model::DBInfo, schemaVersion: i64) {
    let mut item = schemaItem{schemaVersion: schemaVersion, dbInfo: dbInfo, tomb: true}
    btreeSet(&isd.schemaMap, item)
    btreeSet(&isd.schemaID2Name, schemaIDName{schemaVersion: schemaVersion, id: dbInfo.ID, name: dbInfo.Name, tomb: true})
}

// referredForeignKeysHelper 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct referredForeignKeysHelper {
    pub start: referredForeignKeyItem,
    pub schemaVersion: i64,
    pub referredFKInfos: Vec<&mut model::ReferredFKInfo>,
}

// onItem 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn onItem(h: &mut referredForeignKeysHelper, item: &mut referredForeignKeyItem) -> bool {
    if item.dbName != h.start.dbName || item.tableName != h.start.tableName {
        return false
    }

    if item.schemaVersion <= h.schemaVersion {
        if !item.tomb { // If the item is a tomb record, all the foreign keys are deleted.
            h.referredFKInfos = item.referredFKInfo
        }
        return false
    }
    return true
}

// getTableReferredForeignKeys 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn getTableReferredForeignKeys(isd: &mut Data, schema: String, table: String, schemaMetaVersion: i64) -> Vec<&mut model::ReferredFKInfo> {
    let mut helper = referredForeignKeysHelper{
        start:           referredForeignKeyItem{dbName: schema, tableName: table, schemaVersion: math.MaxInt64},
        schemaVersion:   schemaMetaVersion,
        referredFKInfos: make([]*model.ReferredFKInfo, 0),
    }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    isd.referredForeignKeys.Load().DescendLessOrEqual(&helper.start, helper.onItem)
    return helper.referredFKInfos
}

// hasForeignKeyReference checks if a specific foreign key reference already exists
// hasForeignKeyReference 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn hasForeignKeyReference(isd: &mut Data, refs: Vec<&mut model::ReferredFKInfo>, schema: ast::CIStr, table: ast::CIStr, fkName: ast::CIStr) -> bool {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, ref := range refs {
        if ref.ChildSchema.L == schema.L &&
            ref.ChildTable.L == table.L &&
            ref.ChildFKName.L == fkName.L {
            return true
        }
    }
    return false
}

// addReferredForeignKeys 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn addReferredForeignKeys(isd: &mut Data, schema: ast::CIStr, tbInfo: &mut model::TableInfo, schemaMetaVersion: i64) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, fk := range tbInfo.ForeignKeys {
        if fk.Version < model.FKVersion1 {
            continue
        }

        // Get current foreign key references for the table
        let mut (refSchema, refTable) = fk.RefSchema.L, fk.RefTable.L
        let mut existingRefs = isd.getTableReferredForeignKeys(fk.RefSchema.L, fk.RefTable.L, schemaMetaVersion)

        // Skip if this specific foreign key reference already exists
        if isd.hasForeignKeyReference(existingRefs, schema, tbInfo.Name, fk.Name) {
            continue
        }

        // Create a new array with existing refs + new reference
        let mut newRefs = make([]*model.ReferredFKInfo, 0, len(existingRefs)+1)
        newRefs = append(newRefs, existingRefs...)
        newRefs = append(newRefs, &model.ReferredFKInfo{
            Cols:        fk.RefCols,
            ChildSchema: schema,
            ChildTable:  tbInfo.Name,
            ChildFKName: fk.Name,
        })
        sort.Slice(newRefs, func(i, j int) bool {
            if newRefs[i].ChildSchema.L != newRefs[j].ChildSchema.L {
                return newRefs[i].ChildSchema.L < newRefs[j].ChildSchema.L
            }
            if newRefs[i].ChildTable.L != newRefs[j].ChildTable.L {
                return newRefs[i].ChildTable.L < newRefs[j].ChildTable.L
            }
            return newRefs[i].ChildFKName.L < newRefs[j].ChildFKName.L
        })
        btreeSet(&isd.referredForeignKeys, &referredForeignKeyItem{
            dbName:         refSchema,
            tableName:      refTable,
            schemaVersion:  schemaMetaVersion,
            referredFKInfo: newRefs,
        })
    }
}

// deleteReferredForeignKeys 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn deleteReferredForeignKeys(isd: &mut Data, schema: ast::CIStr, tbInfo: &mut model::TableInfo, schemaMetaVersion: i64) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, fk := range tbInfo.ForeignKeys {
        if fk.Version < model.FKVersion1 {
            continue
        }

        // Get current foreign key references for the table
        let mut (refSchema, refTable) = fk.RefSchema.L, fk.RefTable.L
        let mut existingRefs = isd.getTableReferredForeignKeys(refSchema, refTable, schemaMetaVersion)

        // Skip if this specific foreign key reference doesn't exist
        if !isd.hasForeignKeyReference(existingRefs, schema, tbInfo.Name, fk.Name) {
            continue
        }

        // Delete the reference
        if len(existingRefs) == 1 {
            // If this is the only reference, mark the whole item as deleted
            btreeSet(&isd.referredForeignKeys, &referredForeignKeyItem{
                dbName:         refSchema,
                tableName:      refTable,
                schemaVersion:  schemaMetaVersion,
                tomb:           true,
                referredFKInfo: None,
            })
        } else {
            // If there are multiple references, create new array excluding this one
            // clone existingRefs to avoid modifying the original slice
            let mut tmpRefs = append([]*model.ReferredFKInfo(None), existingRefs...)
            let mut newRefs = slices.DeleteFunc(tmpRefs, func(ref *model.ReferredFKInfo) bool {
                return ref.ChildSchema.L == schema.L &&
                    ref.ChildTable.L == tbInfo.Name.L &&
                    ref.ChildFKName.L == fk.Name.L
            })

            btreeSet(&isd.referredForeignKeys, &referredForeignKeyItem{
                dbName:         refSchema,
                tableName:      refTable,
                schemaVersion:  schemaMetaVersion,
                tomb:           false,
                referredFKInfo: newRefs,
            })
        }
    }
}

// gcCollectTableItem returns up to maxItems old tableItem versions for GC.
// gcCollectTableItem 对应 Go 同名函数或方法：保留多版本索引压缩、每个对象的版本枢轴和并发发布冲突处理。
pub fn gcCollectTableItem(bt: &mut btree::BTreeG[&mut tableItem], cutVer: i64, maxItems: i32) -> Vec<&mut tableItem> {
    var dels []*tableItem
    var prev *tableItem
    // Example:
    // gcOldVersion to v4
    //	db3 tbl1 v5
    //	db3 tbl1 v4
    // db3 tbl1 v3 <- delete, because v3 < v4
    // db2 tbl2 v1 <- keep, need to keep the latest version if all versions are less than v4
    // db2 tbl2 v0 <- delete, because v0 < v4
    //	db1 tbl3 v4
    //	...
    // So the rule can be simplify to "remove all items whose (version < cutVer && previous item is same table && previous
    // item is also < cutVer)". This keeps the pivot record (latest version < cutVer) for every table name, which is still
    // needed to serve requests whose schemaVersion is in [cutVer, next_change_of_the_table).
    bt.Descend(func(item *tableItem) bool {
        if item.schemaVersion < cutVer &&
            prev != None &&
            prev.dbName.L == item.dbName.L &&
            prev.tableName.L == item.tableName.L &&
            prev.schemaVersion < cutVer {
            dels = append(dels, item)
            if len(dels) >= maxItems {
                return false
            }
        }
        prev = item
        return true
    })
    return dels
}

// gcCollectReferredForeignKeyItem returns up to maxItems old referredForeignKeyItem versions for GC.
// gcCollectReferredForeignKeyItem 对应 Go 同名函数或方法：保留多版本索引压缩、每个对象的版本枢轴和并发发布冲突处理。
pub fn gcCollectReferredForeignKeyItem(bt: &mut btree::BTreeG[&mut referredForeignKeyItem], cutVer: i64, maxItems: i32) -> Vec<&mut referredForeignKeyItem> {
    var dels []*referredForeignKeyItem
    var prev *referredForeignKeyItem
    bt.Descend(func(item *referredForeignKeyItem) bool {
        if item.schemaVersion < cutVer &&
            prev != None &&
            prev.dbName == item.dbName &&
            prev.tableName == item.tableName &&
            prev.schemaVersion < cutVer {
            dels = append(dels, item)
            if len(dels) >= maxItems {
                return false
            }
        }
        prev = item
        return true
    })
    return dels
}

// gcOldFKVersion performs GC of old referredForeignKeyItem entries up to maxItems.
// gcOldFKVersion 对应 Go 同名函数或方法：保留多版本索引压缩、每个对象的版本枢轴和并发发布冲突处理。
pub fn gcOldFKVersion(isd: &mut Data, schemaVersion: i64) -> i32 {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut rfOld = isd.referredForeignKeys.Load()
    let mut rfNew = rfOld.Clone()
    let mut rfDels = gcCollectReferredForeignKeyItem(rfOld, schemaVersion, 1024)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, fk := range rfDels {
        rfNew.Delete(fk)
    }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    if !isd.referredForeignKeys.CompareAndSwap(rfOld, rfNew) {
        logutil.BgLogger().Info("infoschema v2 GCOldVersion() referredForeignKeys gc conflict")
    }
    return len(rfDels)
}

// GCOldVersion compacts btree nodes by removing items older than schema version.
// exported for testing
// GCOldVersion 对应 Go 同名函数或方法：保留多版本索引压缩、每个对象的版本枢轴和并发发布冲突处理。
pub fn GCOldVersion(isd: &mut Data, schemaVersion: i64) -> (i32, i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    if isd.byName.Load().Len() == 0 {
        return 0, 0
    }

    // collect and remove old tableItems
    let mut dels = gcCollectTableItem(isd.byName.Load(), schemaVersion, 1024)
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut byNameOld = isd.byName.Load()
    let mut byNameNew = byNameOld.Clone()
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut byIDOld = isd.byID.Load()
    let mut byIDNew = byIDOld.Clone()
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, ti := range dels {
        byNameNew.Delete(ti)
        byIDNew.Delete(ti)
    }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut succ1 = isd.byID.CompareAndSwap(byIDOld, byIDNew)
    var succ2 bool
    if succ1 {
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        succ2 = isd.byName.CompareAndSwap(byNameOld, byNameNew)
    }
    if !succ1 || !succ2 {
        logutil.BgLogger().Info("infoschema v2 GCOldVersion() writes conflict",
            zap.Bool("byID", succ1), zap.Bool("byName", succ2))
    }

    // collect and remove old referredForeignKeyItems
    _ = isd.gcOldFKVersion(schemaVersion)

    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    return len(dels), int64(isd.byName.Load().Len())
}

// resetBeforeFullLoad is called before a full recreate operation within builder.InitWithDBInfos().
// TODO: write a generics version to avoid repeated code.
// resetBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetBeforeFullLoad(isd: &mut Data, schemaVersion: i64) {
    resetTableInfoResidentBeforeFullLoad(&isd.tableInfoResident, schemaVersion)

    resetByIDBeforeFullLoad(&isd.byID, schemaVersion)
    resetByNameBeforeFullLoad(&isd.byName, schemaVersion)

    resetSchemaMapBeforeFullLoad(&isd.schemaMap, schemaVersion)
    resetSchemaID2NameBeforeFullLoad(&isd.schemaID2Name, schemaVersion)

    resetPID2TIDBeforeFullLoad(&isd.pid2tid, schemaVersion)
    resetFKBeforeFullLoad(&isd.referredForeignKeys, schemaVersion)
}

// resetByIDBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetByIDBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[&mut tableItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }

    let mut batchSize = min(bt.Len(), 1000)
    let mut items = make([]*tableItem, 0, batchSize)
    items = append(items, pivot)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for {
        bt.DescendLessOrEqual(pivot, func(item *tableItem) bool {
            if pivot.tableID == item.tableID {
                return true // skip MVCC version
            }
            pivot = item
            items = append(items, pivot)
            return len(items) < cap(items)
        })
        if len(items) == 0 {
            break
        }
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, item := range items {
            btreeSet(ptr, &tableItem{
                dbName:        item.dbName,
                dbID:          item.dbID,
                tableName:     item.tableName,
                tableID:       item.tableID,
                schemaVersion: schemaVersion,
                tomb:          true,
            })
        }
        items = items[:0]
    }
}

// resetByNameBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetByNameBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[&mut tableItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }

    let mut batchSize = min(bt.Len(), 1000)
    let mut items = make([]*tableItem, 0, batchSize)
    items = append(items, pivot)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for {
        bt.DescendLessOrEqual(pivot, func(item *tableItem) bool {
            if pivot.dbName == item.dbName && pivot.tableName == item.tableName {
                return true // skip MVCC version
            }
            pivot = item
            items = append(items, pivot)
            return len(items) < cap(items)
        })
        if len(items) == 0 {
            break
        }
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, item := range items {
            btreeSet(ptr, &tableItem{
                dbName:        item.dbName,
                dbID:          item.dbID,
                tableName:     item.tableName,
                tableID:       item.tableID,
                schemaVersion: schemaVersion,
                tomb:          true,
            })
        }
        items = items[:0]
    }
}

// resetTableInfoResidentBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetTableInfoResidentBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[tableInfoItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }
    let mut items = make([]tableInfoItem, 0, bt.Len())
    items = append(items, pivot)
    bt.DescendLessOrEqual(pivot, func(item tableInfoItem) bool {
        if pivot.dbName == item.dbName && pivot.tableID == item.tableID {
            return true // skip MVCC version
        }
        pivot = item
        items = append(items, pivot)
        return true
    })
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, item := range items {
        btreeSet(ptr, tableInfoItem{
            dbName:        item.dbName,
            tableID:       item.tableID,
            schemaVersion: schemaVersion,
            tomb:          true,
        })
    }
}

// resetSchemaMapBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetSchemaMapBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[schemaItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }
    let mut items = make([]schemaItem, 0, bt.Len())
    items = append(items, pivot)
    bt.DescendLessOrEqual(pivot, func(item schemaItem) bool {
        if pivot.Name() == item.Name() {
            return true // skip MVCC version
        }
        pivot = item
        items = append(items, pivot)
        return true
    })
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, item := range items {
        btreeSet(ptr, schemaItem{
            dbInfo:        item.dbInfo,
            schemaVersion: schemaVersion,
            tomb:          true,
        })
    }
}

// resetSchemaID2NameBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetSchemaID2NameBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[schemaIDName]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }
    let mut items = make([]schemaIDName, 0, bt.Len())
    items = append(items, pivot)
    bt.DescendLessOrEqual(pivot, func(item schemaIDName) bool {
        if pivot.id == item.id {
            return true // skip MVCC version
        }
        pivot = item
        items = append(items, pivot)
        return true
    })
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, item := range items {
        btreeSet(ptr, schemaIDName{
            id:            item.id,
            name:          item.name,
            schemaVersion: schemaVersion,
            tomb:          true,
        })
    }
}

// resetPID2TIDBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetPID2TIDBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[partitionItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }

    let mut batchSize = min(bt.Len(), 1000)
    let mut items = make([]partitionItem, 0, batchSize)
    items = append(items, pivot)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for {
        bt.DescendLessOrEqual(pivot, func(item partitionItem) bool {
            if pivot.partitionID == item.partitionID {
                return true // skip MVCC version
            }
            pivot = item
            items = append(items, pivot)
            return len(items) < cap(items)
        })
        if len(items) == 0 {
            break
        }
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, item := range items {
            btreeSet(ptr, partitionItem{
                partitionID:   item.partitionID,
                tableID:       item.tableID,
                schemaVersion: schemaVersion,
                tomb:          true,
            })
        }
        items = items[:0]
    }
}

// resetFKBeforeFullLoad 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn resetFKBeforeFullLoad(ptr: &mut atomic::Pointer[btree::BTreeG[&mut referredForeignKeyItem]], schemaVersion: i64) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut bt = ptr.Load()
    let mut (pivot, ok) = bt.Max()
    if !ok {
        return
    }
    let mut items = make([]*referredForeignKeyItem, 0, bt.Len())
    items = append(items, pivot)
    bt.DescendLessOrEqual(pivot, func(item *referredForeignKeyItem) bool {
        if pivot.dbName == item.dbName && pivot.tableName == item.tableName {
            return true
        }
        pivot = item
        items = append(items, pivot)
        return true
    })
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, item := range items {
        btreeSet(ptr, &referredForeignKeyItem{
            dbName:         item.dbName,
            tableName:      item.tableName,
            schemaVersion:  schemaVersion,
            tomb:           true,
            referredFKInfo: None,
        })
    }
}

// compareByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn compareByID(a: &mut tableItem, b: &mut tableItem) -> bool {
    if a.tableID < b.tableID {
        return true
    }
    if a.tableID > b.tableID {
        return false
    }

    return a.schemaVersion < b.schemaVersion
}

// compareByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn compareByName(a: &mut tableItem, b: &mut tableItem) -> bool {
    if a.dbName.L < b.dbName.L {
        return true
    }
    if a.dbName.L > b.dbName.L {
        return false
    }

    if a.tableName.L < b.tableName.L {
        return true
    }
    if a.tableName.L > b.tableName.L {
        return false
    }

    return a.schemaVersion < b.schemaVersion
}

// compareTableInfoItem 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn compareTableInfoItem(a: tableInfoItem, b: tableInfoItem) -> bool {
    if a.dbName.L < b.dbName.L {
        return true
    }
    if a.dbName.L > b.dbName.L {
        return false
    }

    if a.tableID < b.tableID {
        return true
    }
    if a.tableID > b.tableID {
        return false
    }
    return a.schemaVersion < b.schemaVersion
}

// comparePartitionItem 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn comparePartitionItem(a: partitionItem, b: partitionItem) -> bool {
    if a.partitionID < b.partitionID {
        return true
    }
    if a.partitionID > b.partitionID {
        return false
    }
    return a.schemaVersion < b.schemaVersion
}

// compareSchemaItem 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn compareSchemaItem(a: schemaItem, b: schemaItem) -> bool {
    if a.Name() < b.Name() {
        return true
    }
    if a.Name() > b.Name() {
        return false
    }
    return a.schemaVersion < b.schemaVersion
}

// compareSchemaByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn compareSchemaByID(a: schemaIDName, b: schemaIDName) -> bool {
    if a.id < b.id {
        return true
    }
    if a.id > b.id {
        return false
    }
    return a.schemaVersion < b.schemaVersion
}

// compareReferredForeignKeyItem 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn compareReferredForeignKeyItem(a: &mut referredForeignKeyItem, b: &mut referredForeignKeyItem) -> bool {
    if a.dbName != b.dbName {
        return a.dbName < b.dbName
    }
    if a.tableName != b.tableName {
        return a.tableName < b.tableName
    }
    return a.schemaVersion < b.schemaVersion
}

// Go 编译期接口实现断言保留为说明；Rust trait 约束等待模块接线后恢复。
// var _ InfoSchema = &infoschemaV2{}

// infoschemaV2 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct infoschemaV2 {
    *infoSchema // in fact, we only need the infoSchemaMisc inside it, but the builder rely on it.
    pub factory: func() (pools::Resource, error),
    pub ts: u64,
    pub Data: &mut Data,
}

// NewInfoSchemaV2 create infoschemaV2.
// NewInfoSchemaV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn NewInfoSchemaV2(r: autoid::Requirement, factory: func() (pools::Resource, error), infoData: &mut Data) -> infoschemaV2 {
    return infoschemaV2{
        infoSchema: newInfoSchema(r, factory),
        Data:       infoData,
        factory:    factory,
    }
}

// search 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn search(bt: &mut btree::BTreeG[&mut tableItem], schemaVersion: i64, end: tableItem, matchFn: func(a, b &mut tableItem) bool) -> (&mut tableItem, bool) {
    var ok bool
    var target *tableItem
    // Iterate through the btree, find the query item whose schema version is the largest one (latest).
    bt.DescendLessOrEqual(&end, func(item *tableItem) bool {
        if !matchFn(&end, item) {
            return false
        }
        if item.schemaVersion > schemaVersion {
            // We're seaching historical snapshot, and this record is newer than us, we can't use it.
            // Skip the record.
            return true
        }
        // schema version of the items should <= query's schema version.
        if !ok { // The first one found.
            ok = true
            target = item
        } else { // The latest one
            if item.schemaVersion > target.schemaVersion {
                target = item
            }
        }
        return true
    })
    if ok && target.tomb {
        // If the item is a tomb record, the table is dropped.
        ok = false
    }
    return target, ok
}

// base 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn base(is: &mut infoschemaV2) -> &mut infoSchema {
    return is.infoSchema
}

// CloneAndUpdateTS 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn CloneAndUpdateTS(is: &mut infoschemaV2, startTS: u64) -> &mut infoschemaV2 {
    let mut tmp = *is
    tmp.ts = startTS
    return &tmp
}

// searchTableItemByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn searchTableItemByID(is: &mut infoschemaV2, tableID: i64) -> (&mut tableItem, bool) {
    let mut eq = func(a, b *tableItem) bool { return a.tableID == b.tableID }
    return search(
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        is.byID.Load(),
        is.infoSchema.schemaMetaVersion,
        tableItem{tableID: tableID, schemaVersion: math.MaxInt64},
        eq,
    )
}

// TableByID implements the InfoSchema interface.
// As opposed to TableByName, TableByID will not refill cache when schema cache miss,
// unless the caller changes the behavior by passing a context use WithRefillOption.
// TableByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByID(is: &mut infoschemaV2, ctx: context::Context, id: i64) -> (table::Table, bool) {
    if !tableIDIsValid(id) {
        return
    }

    is.keepAlive()
    let mut (itm, ok) = is.searchTableItemByID(id)
    if !ok {
        return None, false
    }

    if autoid.IsMemSchemaID(id) {
        if raw, exist := is.Data.specials.Load(itm.dbName.L); exist {
            let mut schTbls = raw.(*schemaTables)
            val, ok = schTbls.tables[itm.tableName.L]
            return
        }
        return None, false
    }

    let mut refill = false
    if opt := ctx.Value(refillOptionKey); opt != None {
        refill = opt.(bool)
    }

    // get cache with item key
    let mut key = tableCacheKey{itm.tableID, itm.schemaVersion}
    let mut (tbl, found) = is.tableCache.Get(key)
    if found && tbl != None {
        return tbl, true
    }

    // Maybe the table is evicted? need to reload.
    let mut (ret, err) = is.loadTableInfo(ctx, id, itm.dbID, is.ts, is.infoSchema.schemaMetaVersion)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None || ret == None {
        return None, false
    }

    if refill {
        is.tableCache.Set(key, ret)
    }
    return ret, true
}

// TableItemByID implements the InfoSchema interface.
// It only contains memory operations, no worries about accessing the storage.
// TableItemByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableItemByID(is: &mut infoschemaV2, tableID: i64) -> (TableItem, bool) {
    let mut (itm, ok) = is.searchTableItemByID(tableID)
    if !ok {
        return TableItem{}, false
    }
    return TableItem{DBName: itm.dbName, TableName: itm.tableName}, true
}

// TableItem is exported from tableItem.
// TableItem 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct TableItem {
    pub DBName: ast::CIStr,
    pub TableName: ast::CIStr,
}

// IterateAllTableItems is used for special performance optimization.
// Used by executor/infoschema_reader.go to handle reading from INFORMATION_SCHEMA.TABLES.
// If visit return false, stop the iterate process.
// IterateAllTableItems 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn IterateAllTableItems(is: &mut infoschemaV2, visit: func(TableItem) bool) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut (maxv, ok) = is.byName.Load().Max()
    if !ok {
        return
    }
    var pivot *tableItem
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.byName.Load().DescendLessOrEqual(maxv, func(item *tableItem) bool {
        if item.schemaVersion > is.schemaMetaVersion {
            // skip MVCC version, those items are not visible to the queried schema version
            return true
        }
        if pivot != None && pivot.dbName == item.dbName && pivot.tableName == item.tableName {
            // skip MVCC version, this db.table has been visited already
            return true
        }
        pivot = item
        if !item.tomb {
            return visit(TableItem{DBName: item.dbName, TableName: item.tableName})
        }
        return true
    })
}

// TableIsCached checks whether the table is cached.
// TableIsCached 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableIsCached(is: &mut infoschemaV2, id: i64) -> (bool) {
    if !tableIDIsValid(id) {
        return false
    }

    let mut (itm, ok) = is.searchTableItemByID(id)
    if !ok {
        return false
    }

    if autoid.IsMemSchemaID(id) {
        if raw, exist := is.Data.specials.Load(itm.dbName.L); exist {
            let mut schTbls = raw.(*schemaTables)
            _, ok = schTbls.tables[itm.tableName.L]
            return ok
        }
        return false
    }

    let mut key = tableCacheKey{itm.tableID, itm.schemaVersion}
    let mut (tbl, found) = is.tableCache.Get(key)
    return found && tbl != None
}

// IsSpecialDB tells whether the database is a special database.
// IsSpecialDB 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn IsSpecialDB(dbName: String) -> bool {
    return metadef.IsMemDB(dbName)
}

// EvictTable is exported for testing only.
// EvictTable 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn EvictTable(is: &mut infoschemaV2, schema: ast::CIStr, tbl: ast::CIStr) {
    let mut eq = func(a, b *tableItem) bool { return a.dbName == b.dbName && a.tableName == b.tableName }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut (itm, ok) = search(is.byName.Load(), is.infoSchema.schemaMetaVersion, tableItem{dbName: schema, tableName: tbl, schemaVersion: math.MaxInt64}, eq)
    if !ok {
        return
    }
    is.tableCache.Remove(tableCacheKey{itm.tableID, is.infoSchema.schemaMetaVersion})
    is.tableCache.Remove(tableCacheKey{itm.tableID, itm.schemaVersion})
}

// tableByNameHelper 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct tableByNameHelper {
    pub end: tableItem,
    pub schemaVersion: i64,
    pub found: bool,
    pub res: &mut tableItem,
}

// onItem 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn onItem(h: &mut tableByNameHelper, item: &mut tableItem) -> bool {
    if item.dbName.L != h.end.dbName.L || item.tableName.L != h.end.tableName.L {
        h.found = false
        return false
    }
    if item.schemaVersion <= h.schemaVersion {
        if !item.tomb { // If the item is a tomb record, the database is dropped.
            h.found = true
            h.res = item
        }
        return false
    }
    return true
}

// TableByName implements the InfoSchema interface.
// When schema cache miss, it will fetch the TableInfo from TikV and refill cache.
// TableByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByName(is: &mut infoschemaV2, ctx: context::Context, schema: ast::CIStr, tbl: ast::CIStr) -> (table::Table, errors::Error) {
    if IsSpecialDB(schema.L) {
        if raw, ok := is.specials.Load(schema.L); ok {
            let mut tbNames = raw.(*schemaTables)
            if t, ok = tbNames.tables[tbl.L]; ok {
                return
            }
        }
        return None, ErrTableNotExists.FastGenByArgs(schema, tbl)
    }

    is.keepAlive()
    let mut start = time.Now()
    var h tableByNameHelper
    h.end = tableItem{dbName: schema, tableName: tbl, schemaVersion: math.MaxInt64}
    h.schemaVersion = is.infoSchema.schemaMetaVersion
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.byName.Load().DescendLessOrEqual(&h.end, h.onItem)

    if !h.found {
        return None, ErrTableNotExists.FastGenByArgs(schema, tbl)
    }
    let mut itm = h.res

    // Get from the cache with old key
    let mut oldKey = tableCacheKey{itm.tableID, itm.schemaVersion}
    let mut (res, found) = is.tableCache.Get(oldKey)
    if found && res != None {
        metrics.TableByNameHitDuration.Observe(float64(time.Since(start)))
        return res, None
    }

    // Maybe the table is evicted? need to reload.
    let mut (ret, err) = is.loadTableInfo(ctx, itm.tableID, itm.dbID, is.ts, is.infoSchema.schemaMetaVersion)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, errors.Trace(err)
    }

    let mut refill = true
    if opt := ctx.Value(refillOptionKey); opt != None {
        refill = opt.(bool)
    }
    if refill {
        is.tableCache.Set(oldKey, ret)
    }

    metrics.TableByNameMissDuration.Observe(float64(time.Since(start)))
    return ret, None
}

// TableInfoByName implements InfoSchema.TableInfoByName
// TableInfoByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByName(is: &mut infoschemaV2, schema: ast::CIStr, table: ast::CIStr) -> (&mut model::TableInfo, errors::Error) {
    let mut (tbl, err) = is.TableByName(context.Background(), schema, table)
    return getTableInfo(tbl), err
}

// TableInfoByID implements InfoSchema.TableInfoByID
// TableInfoByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByID(is: &mut infoschemaV2, id: i64) -> (&mut model::TableInfo, bool) {
    let mut (tbl, ok) = is.TableByID(context.Background(), id)
    return getTableInfo(tbl), ok
}

// keepAlive prevents the "GC life time is shorter than transaction duration" error on infoschema v2.
// It works by collecting the min TS of the during infoschem v2 API calls, and
// reports the min TS to info.InfoSyncer.
// keepAlive 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn keepAlive(is: &mut infoschemaV2) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for {
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        let mut v = is.Data.recentMinTS.Load()
        if v <= is.ts {
            break
        }
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        let mut succ = is.Data.recentMinTS.CompareAndSwap(v, is.ts)
        if succ {
            break
        }
    }
}

// SchemaTableInfos implements MetaOnlyInfoSchema.
// SchemaTableInfos 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaTableInfos(is: &mut infoschemaV2, ctx: context::Context, schema: ast::CIStr) -> (Vec<&mut model::TableInfo>, errors::Error) {
    if IsSpecialDB(schema.L) {
        let mut (raw, ok) = is.Data.specials.Load(schema.L)
        if ok {
            let mut schTbls = raw.(*schemaTables)
            let mut tables = make([]table.Table, 0, len(schTbls.tables))
            // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
            for _, tbl := range schTbls.tables {
                tables = append(tables, tbl)
            }
            return getTableInfoList(tables), None
        }
        return None, None // something wrong?
    }

    is.keepAlive()
retry:
    let mut (dbInfo, ok) = is.SchemaByName(schema)
    if !ok {
        return None, None
    }
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut snapshot = is.r.Store().GetSnapshot(kv.NewVersion(is.ts))
    // Using the KV timeout read feature to address the issue of potential DDL lease expiration when
    // the meta region leader is slow.
    snapshot.SetOption(kv.TiKVClientReadTimeout, uint64(3000)) // 3000ms.
    let mut m = meta.NewReader(snapshot)
    let mut (tblInfos, err) = m.ListTables(ctx, dbInfo.ID)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        if meta.ErrDBNotExists.Equal(err) {
            return None, None
        }
        // Flashback statement could cause such kind of error.
        // In theory that error should be handled in the lower layer, like client-go.
        // But it's not done, so we retry here.
        if strings.Contains(err.Error(), "in flashback progress") {
            select {
            case <-time.After(200 * time.Millisecond):
            case <-ctx.Done():
                return None, ctx.Err()
            }
            goto retry
        }
        return None, errors.Trace(err)
    }
    return tblInfos, None
}

// SchemaSimpleTableInfos implements MetaOnlyInfoSchema.
// SchemaSimpleTableInfos 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaSimpleTableInfos(is: &mut infoschemaV2, ctx: context::Context, schema: ast::CIStr) -> (Vec<&mut model::TableNameInfo>, errors::Error) {
    if IsSpecialDB(schema.L) {
        let mut (raw, ok) = is.Data.specials.Load(schema.L)
        if ok {
            let mut schTbls = raw.(*schemaTables)
            let mut ret = make([]*model.TableNameInfo, 0, len(schTbls.tables))
            // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
            for _, tbl := range schTbls.tables {
                ret = append(ret, &model.TableNameInfo{
                    ID:   tbl.Meta().ID,
                    Name: tbl.Meta().Name,
                })
            }
            return ret, None
        }
        return None, None // something wrong?
    }

    // Ascend is much more difficult than Descend.
    // So the data is taken out first and then dedup in Descend order.
    var tableItems []*tableItem
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.byName.Load().AscendGreaterOrEqual(&tableItem{dbName: schema}, func(item *tableItem) bool {
        if item.dbName.L != schema.L {
            return false
        }
        if is.infoSchema.schemaMetaVersion >= item.schemaVersion {
            tableItems = append(tableItems, item)
        }
        return true
    })
    if len(tableItems) == 0 {
        return None, None
    }
    let mut tblInfos = make([]*model.TableNameInfo, 0, len(tableItems))
    var curr *tableItem
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for i := len(tableItems) - 1; i >= 0; i-- {
        let mut item = tableItems[i]
        if curr == None || curr.tableName != tableItems[i].tableName {
            curr = item
            if !item.tomb {
                tblInfos = append(tblInfos, &model.TableNameInfo{
                    ID:   item.tableID,
                    Name: item.tableName,
                })
            }
        }
    }
    return tblInfos, None
}

// FindTableInfoByPartitionID implements InfoSchema.FindTableInfoByPartitionID
// FindTableInfoByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableInfoByPartitionID(is: &mut infoschemaV2, partitionID: i64) -> (&mut model::TableInfo, &mut model::DBInfo, &mut model::PartitionDefinition) {
    let mut (tbl, db, partDef) = is.FindTableByPartitionID(partitionID)
    return getTableInfo(tbl), db, partDef
}

// SchemaByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByName(is: &mut infoschemaV2, schema: ast::CIStr) -> (&mut model::DBInfo, bool) {
    if IsSpecialDB(schema.L) {
        let mut (raw, ok) = is.Data.specials.Load(schema.L)
        if !ok {
            return None, false
        }
        let mut (schTbls, ok) = raw.(*schemaTables)
        return schTbls.dbInfo, ok
    }

    var dbInfo model.DBInfo
    dbInfo.Name = schema
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.Data.schemaMap.Load().DescendLessOrEqual(schemaItem{
        dbInfo:        &dbInfo,
        schemaVersion: math.MaxInt64,
    }, func(item schemaItem) bool {
        if item.Name() != schema.L {
            ok = false
            return false
        }
        if item.schemaVersion <= is.infoSchema.schemaMetaVersion {
            if !item.tomb { // If the item is a tomb record, the database is dropped.
                ok = true
                val = item.dbInfo
            }
            return false
        }
        return true
    })
    return
}

// allSchemas 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn allSchemas(is: &mut infoschemaV2, visit: func(&mut model::DBInfo)) {
    var last *model.DBInfo
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.Data.schemaMap.Load().Descend(func(item schemaItem) bool {
        if item.schemaVersion > is.infoSchema.schemaMetaVersion {
            // Skip the versions that we are not looking for.
            return true
        }

        // Dedup the same db record of different versions.
        if last != None && last.Name == item.dbInfo.Name {
            return true
        }
        last = item.dbInfo

        if !item.tomb {
            visit(item.dbInfo)
        }
        return true
    })
    is.Data.specials.Range(func(key, value any) bool {
        let mut sc = value.(*schemaTables)
        visit(sc.dbInfo)
        return true
    })
}

// AllSchemas 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn AllSchemas(is: &mut infoschemaV2) -> (Vec<&mut model::DBInfo>) {
    is.allSchemas(func(di *model.DBInfo) {
        schemas = append(schemas, di)
    })
    return
}

// AllSchemaNames 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn AllSchemaNames(is: &mut infoschemaV2) -> Vec<ast::CIStr> {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    let mut rs = make([]ast.CIStr, 0, is.Data.schemaMap.Load().Len())
    is.allSchemas(func(di *model.DBInfo) {
        rs = append(rs, di.Name)
    })
    return rs
}

// SchemaExists 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaExists(is: &mut infoschemaV2, schema: ast::CIStr) -> bool {
    let mut (_, ok) = is.SchemaByName(schema)
    return ok
}

// searchPartitionItemByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn searchPartitionItemByPartitionID(is: &mut infoschemaV2, partitionID: i64) -> (partitionItem, bool) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.pid2tid.Load().DescendLessOrEqual(partitionItem{partitionID: partitionID, schemaVersion: math.MaxInt64},
        func(item partitionItem) bool {
            if item.partitionID != partitionID {
                return false
            }
            if item.schemaVersion > is.infoSchema.schemaMetaVersion {
                // Skip the record.
                return true
            }
            if item.schemaVersion <= is.infoSchema.schemaMetaVersion {
                pi = item
                ok = !item.tomb
                return false
            }
            return true
        },
    )
    return pi, ok
}

// TableItemByPartitionID implements InfoSchema.TableItemByPartitionID.
// It returns the lightweight meta info, no worries about access the storage.
// TableItemByPartitionID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableItemByPartitionID(is: &mut infoschemaV2, partitionID: i64) -> (TableItem, bool) {
    let mut (pi, ok) = is.searchPartitionItemByPartitionID(partitionID)
    if !ok {
        return TableItem{}, false
    }
    return is.TableItemByID(pi.tableID)
}

// TableIDByPartitionID implements InfoSchema.TableIDByPartitionID.
// TableIDByPartitionID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableIDByPartitionID(is: &mut infoschemaV2, partitionID: i64) -> (i64, bool) {
    let mut (pi, ok) = is.searchPartitionItemByPartitionID(partitionID)
    if !ok {
        return
    }
    return pi.tableID, true
}

// FindTableByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableByPartitionID(is: &mut infoschemaV2, partitionID: i64) -> (table::Table, &mut model::DBInfo, &mut model::PartitionDefinition) {
    let mut (pi, ok) = is.searchPartitionItemByPartitionID(partitionID)
    if !ok {
        return None, None, None
    }

    let mut (tbl, ok) = is.TableByID(context.Background(), pi.tableID)
    if !ok {
        // something wrong?
        return None, None, None
    }

    let mut dbID = tbl.Meta().DBID
    let mut (dbInfo, ok) = is.SchemaByID(dbID)
    if !ok {
        // something wrong?
        return None, None, None
    }

    let mut partInfo = tbl.Meta().GetPartitionInfo()
    var def *model.PartitionDefinition
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for i := range partInfo.Definitions {
        let mut pdef = &partInfo.Definitions[i]
        if pdef.ID == partitionID {
            def = pdef
            break
        }
    }

    return tbl, dbInfo, def
}

// TableExists 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableExists(is: &mut infoschemaV2, schema: ast::CIStr, table: ast::CIStr) -> bool {
    let mut (_, err) = is.TableByName(context.Background(), schema, table)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    return err == None
}

// SchemaByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByID(is: &mut infoschemaV2, id: i64) -> (&mut model::DBInfo, bool) {
    if autoid.IsMemSchemaID(id) {
        var st *schemaTables
        is.Data.specials.Range(func(key, value any) bool {
            let mut tmp = value.(*schemaTables)
            if tmp.dbInfo.ID == id {
                st = tmp
                return false
            }
            return true
        })
        if st == None {
            return None, false
        }
        return st.dbInfo, true
    }
    var ok bool
    var name ast.CIStr
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.Data.schemaID2Name.Load().DescendLessOrEqual(schemaIDName{
        id:            id,
        schemaVersion: math.MaxInt64,
    }, func(item schemaIDName) bool {
        if item.id != id {
            ok = false
            return false
        }
        if item.schemaVersion <= is.infoSchema.schemaMetaVersion {
            if !item.tomb { // If the item is a tomb record, the database is dropped.
                ok = true
                name = item.name
            }
            return false
        }
        return true
    })
    if !ok {
        return None, false
    }
    return is.SchemaByName(name)
}

// GetTableReferredForeignKeys implements InfoSchema.GetTableReferredForeignKeys
// GetTableReferredForeignKeys 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn GetTableReferredForeignKeys(is: &mut infoschemaV2, schema: String, table: String) -> Vec<&mut model::ReferredFKInfo> {
    is.keepAlive()
    return is.Data.getTableReferredForeignKeys(schema, table, is.infoSchema.schemaMetaVersion)
}

// loadTableInfo 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn loadTableInfo(is: &mut infoschemaV2, ctx: context::Context, tblID: i64, dbID: i64, ts: u64, schemaVersion: i64) -> (table::Table, errors::Error) {
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer tracing.StartRegion(ctx, "infoschema.loadTableInfo").End()
    failpoint.Inject("mockLoadTableInfoError", func(_ failpoint.Value) {
        failpoint.Return(None, errors.New("mockLoadTableInfoError"))
    })
    // Try to avoid repeated concurrency loading.
    // singleflight 合并同表同版本的并发加载，失败结果不得写入共享 table cache。
    let mut (res, err, _) = loadTableSF.Do(fmt.Sprintf("%d-%d-%d", dbID, tblID, schemaVersion), func() (any, error) {
    retry:
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        let mut snapshot = is.r.Store().GetSnapshot(kv.NewVersion(ts))
        // Using the KV timeout read feature to address the issue of potential DDL lease expiration when
        // the meta region leader is slow.
        snapshot.SetOption(kv.TiKVClientReadTimeout, uint64(3000)) // 3000ms.
        let mut m = meta.NewReader(snapshot)

        let mut (tblInfo, err) = m.GetTable(dbID, tblID)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            // Flashback statement could cause such kind of error.
            // In theory that error should be handled in the lower layer, like client-go.
            // But it's not done, so we retry here.
            if strings.Contains(err.Error(), "in flashback progress") {
                time.Sleep(200 * time.Millisecond)
                goto retry
            }

            return None, errors.Trace(err)
        }

        // table removed.
        if tblInfo == None {
            return None, errors.Trace(ErrTableNotExists.FastGenByArgs(
                fmt.Sprintf("(Schema ID %d)", dbID),
                fmt.Sprintf("(Table ID %d)", tblID),
            ))
        }

        ConvertCharsetCollateToLowerCaseIfNeed(tblInfo)
        ConvertOldVersionUTF8ToUTF8MB4IfNeed(tblInfo)
        let mut allocs = autoid.NewAllocatorsFromTblInfo(is.r, dbID, tblInfo)
        let mut (ret, err) = tableFromMeta(allocs, is.factory, tblInfo)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, errors.Trace(err)
        }
        return ret, err
    })

    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, errors.Trace(err)
    }
    if res == None {
        return None, errors.Trace(ErrTableNotExists.FastGenByArgs(
            fmt.Sprintf("(Schema ID %d)", dbID),
            fmt.Sprintf("(Table ID %d)", tblID),
        ))
    }
    return res.(table.Table), None
}

var loadTableSF = &singleflight.Group{}

// IsV2 tells whether an InfoSchema is v2 or not.
// IsV2 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn IsV2(is: InfoSchema) -> (bool, &mut infoschemaV2) {
    let mut (ret, ok) = is.(*infoschemaV2)
    return ok, ret
}

// applyTableUpdate 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn applyTableUpdate(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> (Vec<i64>, errors::Error) {
    if b.enableV2 {
        return b.applyTableUpdateV2(m, diff)
    }
    return b.applyTableUpdate(m, diff)
}

// applyCreateSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyCreateSchema(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> errors::Error {
    return b.applyCreateSchema(m, diff)
}

// applyDropSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyDropSchema(b: &mut Builder, diff: &mut model::SchemaDiff) -> Vec<i64> {
    if b.enableV2 {
        return b.applyDropSchemaV2(diff)
    }
    return b.applyDropSchema(diff)
}

// applyRecoverSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyRecoverSchema(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> (Vec<i64>, errors::Error) {
    if diff.ReadTableFromMeta {
        // recover tables under the database and set them to diff.AffectedOpts
        let mut s = b.store.GetSnapshot(kv.MaxVersion)
        let mut recoverMeta = meta.NewReader(s)
        let mut (tables, err) = recoverMeta.ListSimpleTables(diff.SchemaID)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, err
        }
        diff.AffectedOpts = make([]*model.AffectedOption, 0, len(tables))
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, t := range tables {
            diff.AffectedOpts = append(diff.AffectedOpts, &model.AffectedOption{
                SchemaID:    diff.SchemaID,
                OldSchemaID: diff.SchemaID,
                TableID:     t.ID,
                OldTableID:  t.ID,
            })
        }
    }

    if b.enableV2 {
        return b.applyRecoverSchemaV2(m, diff)
    }
    return b.applyRecoverSchema(m, diff)
}

// applyRecoverSchemaV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyRecoverSchemaV2(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> (Vec<i64>, errors::Error) {
    if di, ok := b.infoschemaV2.SchemaByID(diff.SchemaID); ok {
        return None, ErrDatabaseExists.GenWithStackByArgs(
            fmt.Sprintf("(Schema ID %d)", di.ID),
        )
    }
    let mut (di, err) = m.GetDatabase(diff.SchemaID)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, errors.Trace(err)
    }
    b.infoschemaV2.addDB(diff.Version, di)
    return applyCreateTables(b, m, diff)
}

// applyModifySchemaCharsetAndCollate 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyModifySchemaCharsetAndCollate(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> errors::Error {
    if b.enableV2 {
        return b.applyModifySchemaCharsetAndCollateV2(m, diff)
    }
    return b.applyModifySchemaCharsetAndCollate(m, diff)
}

// applyModifySchemaDefaultPlacement 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyModifySchemaDefaultPlacement(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> errors::Error {
    if b.enableV2 {
        return b.applyModifySchemaDefaultPlacementV2(m, diff)
    }
    return b.applyModifySchemaDefaultPlacement(m, diff)
}

// applyDropTable 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn applyDropTable(b: &mut Builder, diff: &mut model::SchemaDiff, dbInfo: &mut model::DBInfo, tableID: i64, affected: Vec<i64>) -> Vec<i64> {
    if b.enableV2 {
        return b.applyDropTableV2(diff, dbInfo, tableID, affected)
    }
    return b.applyDropTable(diff, dbInfo, tableID, affected)
}

// applyCreateTables 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn applyCreateTables(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> (Vec<i64>, errors::Error) {
    return b.applyCreateTables(m, diff)
}

// updateInfoSchemaBundles 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn updateInfoSchemaBundles(b: &mut Builder) {
    if b.enableV2 {
        b.updateInfoSchemaBundlesV2(&b.infoschemaV2)
    } else {
        b.updateInfoSchemaBundles(b.infoSchema)
    }
}

// oldSchemaInfo 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn oldSchemaInfo(b: &mut Builder, diff: &mut model::SchemaDiff) -> (&mut model::DBInfo, bool) {
    if b.enableV2 {
        return b.infoschemaV2.SchemaByID(diff.OldSchemaID)
    }

    let mut (oldRoDBInfo, ok) = b.infoSchema.SchemaByID(diff.OldSchemaID)
    if ok {
        oldRoDBInfo = b.getSchemaAndCopyIfNecessary(oldRoDBInfo.Name.L)
    }
    return oldRoDBInfo, ok
}

// allocByID returns the Allocators of a table.
// allocByID 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn allocByID(b: &mut Builder, id: i64) -> (autoid::Allocators, bool) {
    var is InfoSchema
    if b.enableV2 {
        is = &b.infoschemaV2
    } else {
        is = b.infoSchema
    }
    let mut (tbl, ok) = is.TableByID(context.Background(), id)
    if !ok {
        return autoid.Allocators{}, false
    }
    return tbl.Allocators(None), true
}

// TODO: more UT to check the correctness.
// applyTableUpdateV2 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn applyTableUpdateV2(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> (Vec<i64>, errors::Error) {
    let mut (oldDBInfo, ok) = b.infoschemaV2.SchemaByID(diff.SchemaID)
    if !ok {
        return None, ErrDatabaseNotExists.GenWithStackByArgs(
            fmt.Sprintf("(Schema ID %d)", diff.SchemaID),
        )
    }

    let mut (oldTableID, newTableID, err) = b.getTableIDs(m, diff)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }
    b.updateBundleForTableUpdate(diff, newTableID, oldTableID)

    let mut (tblIDs, allocs, err) = dropTableForUpdate(b, newTableID, oldTableID, oldDBInfo, diff)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }

    if tableIDIsValid(newTableID) {
        // All types except DropTableOrView.
        var err error
        tblIDs, err = applyCreateTable(b, m, oldDBInfo, newTableID, allocs, diff.Type, tblIDs, diff.Version)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, errors.Trace(err)
        }
    }
    return tblIDs, None
}

// applyDropSchemaV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyDropSchemaV2(b: &mut Builder, diff: &mut model::SchemaDiff) -> Vec<i64> {
    let mut (di, ok) = b.infoschemaV2.SchemaByID(diff.SchemaID)
    if !ok {
        return None
    }

    let mut tableIDs = make([]int64, 0, len(di.Deprecated.Tables))
    let mut (tables, err) = b.infoschemaV2.SchemaTableInfos(context.Background(), di.Name)
    terror.Log(err)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tbl := range tables {
        tableIDs = appendAffectedIDs(tableIDs, tbl)
    }

    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, id := range tableIDs {
        b.deleteBundle(b.infoSchema, id)
        b.applyDropTableV2(diff, di, id, None)
    }
    b.infoData.deleteDB(di, diff.Version)
    return tableIDs
}

// applyDropTableV2 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn applyDropTableV2(b: &mut Builder, diff: &mut model::SchemaDiff, dbInfo: &mut model::DBInfo, tableID: i64, affected: Vec<i64>) -> Vec<i64> {
    // Remove the table in temporaryTables
    if b.infoSchemaMisc.temporaryTableIDs != None {
        delete(b.infoSchemaMisc.temporaryTableIDs, tableID)
    }

    let mut (table, ok) = b.infoschemaV2.TableByID(context.Background(), tableID)
    if !ok {
        return None
    }
    let mut tblInfo = table.Meta()

    // The old DBInfo still holds a reference to old table info, we need to remove it.
    b.infoSchema.deleteReferredForeignKeys(dbInfo.Name, tblInfo)
    b.infoschemaV2.Data.deleteReferredForeignKeys(dbInfo.Name, tblInfo, diff.Version)

    if pi := table.Meta().GetPartitionInfo(); pi != None {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, def := range pi.Definitions {
            btreeSet(&b.infoData.pid2tid, partitionItem{def.ID, diff.Version, table.Meta().ID, true})
        }
    }

    b.infoData.remove(tableItem{
        dbName:        dbInfo.Name,
        dbID:          dbInfo.ID,
        tableName:     tblInfo.Name,
        tableID:       tblInfo.ID,
        schemaVersion: diff.Version,
    })
    affected = appendAffectedIDs(affected, tblInfo)

    return affected
}

// applyModifySchemaCharsetAndCollateV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyModifySchemaCharsetAndCollateV2(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> errors::Error {
    let mut (di, err) = m.GetDatabase(diff.SchemaID)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return errors.Trace(err)
    }
    if di == None {
        // This should never happen.
        return ErrDatabaseNotExists.GenWithStackByArgs(
            fmt.Sprintf("(Schema ID %d)", diff.SchemaID),
        )
    }
    let mut (oldDBInfo, _) = b.infoschemaV2.SchemaByID(diff.SchemaID)
    let mut newDBInfo = oldDBInfo.Clone()
    newDBInfo.Charset = di.Charset
    newDBInfo.Collate = di.Collate
    b.infoschemaV2.deleteDB(di, diff.Version)
    b.infoschemaV2.addDB(diff.Version, newDBInfo)
    return None
}

// applyModifySchemaDefaultPlacementV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn applyModifySchemaDefaultPlacementV2(b: &mut Builder, m: meta::Reader, diff: &mut model::SchemaDiff) -> errors::Error {
    let mut (di, err) = m.GetDatabase(diff.SchemaID)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return errors.Trace(err)
    }
    if di == None {
        // This should never happen.
        return ErrDatabaseNotExists.GenWithStackByArgs(
            fmt.Sprintf("(Schema ID %d)", diff.SchemaID),
        )
    }
    let mut (oldDBInfo, _) = b.infoschemaV2.SchemaByID(diff.SchemaID)
    let mut newDBInfo = oldDBInfo.Clone()
    newDBInfo.PlacementPolicyRef = di.PlacementPolicyRef
    b.infoschemaV2.deleteDB(di, diff.Version)
    b.infoschemaV2.addDB(diff.Version, newDBInfo)
    return None
}

// updateInfoSchemaBundlesV2 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn updateInfoSchemaBundlesV2(b: &mut bundleInfoBuilder, is: &mut infoschemaV2) {
    if b.deltaUpdate {
        b.completeUpdateTablesV2(is)
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for tblID := range b.updateTables {
            b.updateTableBundles(is, tblID)
        }
        return
    }

    // do full update bundles
    is.ruleBundleMap = make(map[int64]*placement.Bundle)
    let mut tmp = is.ListTablesWithSpecialAttribute(infoschemacontext.PlacementPolicyAttribute)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range tmp {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, tbl := range v.TableInfos {
            b.updateTableBundles(is, tbl.ID)
        }
    }
}

// completeUpdateTablesV2 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn completeUpdateTablesV2(b: &mut bundleInfoBuilder, is: &mut infoschemaV2) {
    if len(b.updatePolicies) == 0 {
        return
    }

    let mut dbs = is.ListTablesWithSpecialAttribute(infoschemacontext.AllSpecialAttribute)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, db := range dbs {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, tbl := range db.TableInfos {
            let mut tblInfo = tbl
            if tblInfo.PlacementPolicyRef != None {
                if _, ok := b.updatePolicies[tblInfo.PlacementPolicyRef.ID]; ok {
                    b.markTableBundleShouldUpdate(tblInfo.ID)
                }
            }
        }
    }
}

// ListTablesWithSpecialAttribute 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn ListTablesWithSpecialAttribute(is: &mut infoschemaV2, filter: infoschemacontext::SpecialAttributeFilter) -> Vec<infoschemacontext::TableInfoResult> {
    let mut ret = make([]infoschemacontext.TableInfoResult, 0, 10)
    var currDB string
    var lastTableID int64
    var res infoschemacontext.TableInfoResult
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.Data.tableInfoResident.Load().Descend(func(item tableInfoItem) bool {
        if item.schemaVersion > is.infoSchema.schemaMetaVersion {
            // Skip the versions that we are not looking for.
            return true
        }
        // Dedup the same record of different versions.
        if lastTableID != 0 && lastTableID == item.tableID {
            return true
        }
        lastTableID = item.tableID

        if item.tomb {
            return true
        }

        if !filter(item.tableInfo) {
            return true
        }

        if currDB == "" {
            currDB = item.dbName.L
            res = infoschemacontext.TableInfoResult{DBName: item.dbName}
            res.TableInfos = append(res.TableInfos, item.tableInfo)
        } else if currDB == item.dbName.L {
            res.TableInfos = append(res.TableInfos, item.tableInfo)
        } else {
            ret = append(ret, res)
            res = infoschemacontext.TableInfoResult{DBName: item.dbName}
            res.TableInfos = append(res.TableInfos, item.tableInfo)
        }
        return true
    })
    if len(res.TableInfos) > 0 {
        ret = append(ret, res)
    }
    return ret
}

// refillOption 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct refillOption {

pub var: refillOptionKey refillOption,

// WithRefillOption controls the infoschema v2 cache refill operation.
// By default, TableByID does not refill schema cache, and TableByName does.
// The behavior can be changed by providing the context.Context.
// WithRefillOption 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn WithRefillOption(ctx: context::Context, refill: bool) -> context::Context {
    pub return: context::WithValue(ctx, refillOptionKey, refill),
}
*/

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::infoschema::{
    CiString, DBInfo, InfoSchema, InfoSchemaError, PartitionDefinition, ReferredFKInfo, Table,
    TableInfo, TableItem,
};
use crate::sieve::{Sieve, newSieve};
use astersql_infoschema_context as context_dependency;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 表缓存键：表 ID + schema 版本。
struct TableCacheKey {
    table_id: i64,
    schema_version: i64,
}

#[derive(Clone)]
/// 某一 schema 版本下的表记录（可含 tomb 表示删除）。
struct TableRecord {
    db_name: CiString,
    db_id: i64,
    table_name: CiString,
    table_id: i64,
    schema_version: i64,
    table: Option<Table>,
    tomb: bool,
}
#[derive(Clone)]
/// 某一 schema 版本下的库记录。
struct SchemaRecord {
    schema_version: i64,
    db: Arc<DBInfo>,
    tomb: bool,
}
#[derive(Clone)]
/// 分区 ID → 表 ID 的版本化映射记录。
struct PartitionRecord {
    schema_version: i64,
    table_id: i64,
    tomb: bool,
}
#[derive(Clone)]
/// 某一版本下父表的反向外键列表。
struct ForeignKeyRecord {
    schema_version: i64,
    refs: Vec<ReferredFKInfo>,
    tomb: bool,
}

#[derive(Default)]
/// 全部版本化索引的内存容器。
struct VersionedData {
    by_id: HashMap<i64, Vec<TableRecord>>,
    by_name: HashMap<(String, String), Vec<TableRecord>>,
    schema_by_name: HashMap<String, Vec<SchemaRecord>>,
    schema_id_to_name: HashMap<i64, Vec<(i64, CiString, bool)>>,
    partitions: HashMap<i64, Vec<PartitionRecord>>,
    referred_foreign_keys: HashMap<(String, String), Vec<ForeignKeyRecord>>,
    specials: HashMap<String, (Arc<DBInfo>, Vec<Table>)>,
}

/// 所有 InfoSchema V2 快照共享的 MVCC（多版本并发控制风格）元数据后端。
/// Shared MVCC metadata backing every InfoSchema v2 snapshot.
pub struct Data {
    inner: RwLock<VersionedData>,
    table_cache: Sieve<TableCacheKey, Table>,
    recent_min_ts: AtomicU64,
    temporary_table_ids: RwLock<HashSet<i64>>,
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

impl Data {
    /// 创建默认容量的共享 Data。
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(VersionedData::default()),
            table_cache: newSieve(1024 * 1024 * 1024),
            recent_min_ts: AtomicU64::new(0),
            temporary_table_ids: RwLock::new(HashSet::new()),
        }
    }
    /// 表 SIEVE 缓存容量。
    pub fn CacheCapacity(&self) -> u64 {
        self.table_cache.Capacity()
    }
    /// 设置缓存容量并等待淘汰完成。
    pub fn SetCacheCapacity(&self, capacity: u64) {
        self.table_cache.SetCapacityAndWaitEvict(capacity);
    }

    /// 在底层表 SIEVE 缓存上安装状态钩子（对应 Go 测试路径）。
    /// Install a status hook on the underlying table SIEVE cache (Go test path).
    pub fn SetStatusHook(&self, hook: Arc<dyn crate::sieve::SieveStatusHook>) {
        self.table_cache.SetStatusHook(hook);
    }

    pub fn addTemporaryTable(&self, table_id: i64) {
        self.temporary_table_ids
            .write()
            .expect("infoschema v2 temporary-table lock poisoned")
            .insert(table_id);
    }

    pub fn removeTemporaryTable(&self, table_id: i64) {
        self.temporary_table_ids
            .write()
            .expect("infoschema v2 temporary-table lock poisoned")
            .remove(&table_id);
    }

    pub fn hasTemporaryTable(&self) -> bool {
        !self
            .temporary_table_ids
            .read()
            .expect("infoschema v2 temporary-table lock poisoned")
            .is_empty()
    }

    /// 同 `CacheCapacity` 的别名访问。
    pub fn table_cache_capacity(&self) -> u64 {
        self.table_cache.Capacity()
    }

    /// 在指定 schema 版本登记一张表（含分区与外键反向索引），并写入缓存。
    pub fn add(&self, db: &DBInfo, table: Table, schema_version: i64) {
        let record = TableRecord {
            db_name: db.name.clone(),
            db_id: db.id,
            table_name: table.Meta().name.clone(),
            table_id: table.Meta().id,
            schema_version,
            table: Some(table.clone()),
            tomb: false,
        };
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_table(
            data.by_id.entry(record.table_id).or_default(),
            record.clone(),
        );
        insert_table(
            data.by_name
                .entry((
                    record.db_name.lower.clone(),
                    record.table_name.lower.clone(),
                ))
                .or_default(),
            record.clone(),
        );
        if let Some(partitions) = &table.Meta().partition {
            for partition in &partitions.definitions {
                insert_partition(
                    data.partitions.entry(partition.id).or_default(),
                    PartitionRecord {
                        schema_version,
                        table_id: table.Meta().id,
                        tomb: false,
                    },
                );
            }
        }
        for foreign_key in &table.Meta().foreign_keys {
            let key = (
                foreign_key.ref_schema.lower.clone(),
                foreign_key.ref_table.lower.clone(),
            );
            let mut refs = visible_fk(data.referred_foreign_keys.get(&key), schema_version)
                .unwrap_or_default();
            let reference = ReferredFKInfo {
                child_schema: db.name.clone(),
                child_table: table.Meta().name.clone(),
                child_fk_name: foreign_key.name.clone(),
            };
            if !refs.contains(&reference) {
                refs.push(reference);
            }
            refs.sort_by(|a, b| {
                (
                    &a.child_schema.lower,
                    &a.child_table.lower,
                    &a.child_fk_name.lower,
                )
                    .cmp(&(
                        &b.child_schema.lower,
                        &b.child_table.lower,
                        &b.child_fk_name.lower,
                    ))
            });
            insert_fk(
                data.referred_foreign_keys.entry(key).or_default(),
                ForeignKeyRecord {
                    schema_version,
                    refs,
                    tomb: false,
                },
            );
        }
        drop(data);
        self.table_cache.Set(
            TableCacheKey {
                table_id: table.Meta().id,
                schema_version,
            },
            table,
        );
    }

    /// 登记特殊系统库（如 information_schema）及其内存表。
    pub fn addSpecialDB(&self, db: DBInfo, tables: Vec<Table>) {
        self.inner
            .write()
            .expect("infoschema v2 lock poisoned")
            .specials
            .entry(db.name.lower.clone())
            .or_insert((Arc::new(db), tables));
    }
    /// 在指定版本登记一个库（清空内嵌 tables，表走独立索引）。
    pub fn addDB(&self, schema_version: i64, mut db: DBInfo) {
        db.tables.clear();
        let db = Arc::new(db);
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_schema(
            data.schema_by_name
                .entry(db.name.lower.clone())
                .or_default(),
            SchemaRecord {
                schema_version,
                db: db.clone(),
                tomb: false,
            },
        );
        let versions = data.schema_id_to_name.entry(db.id).or_default();
        versions.push((schema_version, db.name.clone(), false));
        versions.sort_by_key(|record| std::cmp::Reverse(record.0));
    }
    /// 在指定版本以 tomb 删除表，并清理其外键反向引用。
    pub fn remove(
        &self,
        db_name: CiString,
        db_id: i64,
        table_name: CiString,
        table_id: i64,
        schema_version: i64,
    ) {
        let previous = {
            let data = self.inner.read().expect("infoschema v2 lock poisoned");
            visible_table(data.by_id.get(&table_id), schema_version.saturating_sub(1))
        };
        let record = TableRecord {
            db_name,
            db_id,
            table_name,
            table_id,
            schema_version,
            table: None,
            tomb: true,
        };
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_table(data.by_id.entry(table_id).or_default(), record.clone());
        insert_table(
            data.by_name
                .entry((
                    record.db_name.lower.clone(),
                    record.table_name.lower.clone(),
                ))
                .or_default(),
            record.clone(),
        );
        // Mirror Go deleteReferredForeignKeys: dropping a child table removes
        // its FK entries from each referenced parent at this schema version.
        if let Some(prev) = previous.and_then(|item| item.table) {
            for foreign_key in &prev.Meta().foreign_keys {
                let key = (
                    foreign_key.ref_schema.lower.clone(),
                    foreign_key.ref_table.lower.clone(),
                );
                let mut refs = visible_fk(data.referred_foreign_keys.get(&key), schema_version)
                    .unwrap_or_default();
                refs.retain(|reference| {
                    !(reference.child_schema.lower == record.db_name.lower
                        && reference.child_table.lower == record.table_name.lower
                        && reference.child_fk_name.lower == foreign_key.name.lower)
                });
                insert_fk(
                    data.referred_foreign_keys.entry(key).or_default(),
                    ForeignKeyRecord {
                        schema_version,
                        refs,
                        tomb: false,
                    },
                );
            }
        }
    }
    /// 在指定版本以 tomb 删除库。
    pub fn deleteDB(&self, db: DBInfo, schema_version: i64) {
        let db = Arc::new(db);
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_schema(
            data.schema_by_name
                .entry(db.name.lower.clone())
                .or_default(),
            SchemaRecord {
                schema_version,
                db: db.clone(),
                tomb: true,
            },
        );
        let versions = data.schema_id_to_name.entry(db.id).or_default();
        versions.push((schema_version, db.name.clone(), true));
        versions.sort_by_key(|record| std::cmp::Reverse(record.0));
    }
    /// 在给定版本查询引用某父表的外键列表。
    pub fn getTableReferredForeignKeys(
        &self,
        schema: &str,
        table: &str,
        version: i64,
    ) -> Vec<ReferredFKInfo> {
        visible_fk(
            self.inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .referred_foreign_keys
                .get(&(schema.to_lowercase(), table.to_lowercase())),
            version,
        )
        .unwrap_or_default()
    }
    /// 垃圾回收低于 cut_version 的旧表历史；返回删除条数与剩余名索引规模。
    pub fn GCOldVersion(&self, cut_version: i64) -> (usize, i64) {
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        let mut removed = Vec::new();
        for history in data.by_name.values_mut() {
            let remaining = 1024usize.saturating_sub(removed.len());
            if remaining == 0 {
                break;
            }
            let Some(pivot) = history
                .iter()
                .position(|item| item.schema_version < cut_version)
            else {
                continue;
            };
            let remove_count = history.len().saturating_sub(pivot + 1).min(remaining);
            let split_at = history.len() - remove_count;
            removed.extend(
                history
                    .drain(split_at..)
                    .map(|item| (item.table_id, item.schema_version)),
            );
        }
        let removed: HashSet<_> = removed.into_iter().collect();
        for history in data.by_id.values_mut() {
            history.retain(|item| !removed.contains(&(item.table_id, item.schema_version)));
        }
        for history in data.referred_foreign_keys.values_mut() {
            if let Some(pivot) = history
                .iter()
                .position(|item| item.schema_version < cut_version)
            {
                history.truncate(pivot + 1);
            }
        }
        (
            removed.len(),
            data.by_name.values().map(Vec::len).sum::<usize>() as i64,
        )
    }
    /// 全量加载前：对现有历史追加 tomb，避免旧版本残留可见。
    pub fn resetBeforeFullLoad(&self, schema_version: i64) {
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        for history in data.by_id.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_table(
                    history,
                    TableRecord {
                        schema_version,
                        table: None,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.by_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_table(
                    history,
                    TableRecord {
                        schema_version,
                        table: None,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.schema_by_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_schema(
                    history,
                    SchemaRecord {
                        schema_version,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.schema_id_to_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                history.retain(|old| old.0 != schema_version);
                history.push((schema_version, latest.1, true));
                history.sort_by_key(|record| std::cmp::Reverse(record.0));
            }
        }
        for history in data.partitions.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_partition(
                    history,
                    PartitionRecord {
                        schema_version,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.referred_foreign_keys.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_fk(
                    history,
                    ForeignKeyRecord {
                        schema_version,
                        refs: Vec::new(),
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
    }
    /// 记录近期最小时间戳，防止 GC 过早回收仍被引用的版本。
    fn keep_alive(&self, ts: u64) {
        let mut current = self.recent_min_ts.load(Ordering::Acquire);
        while (current == 0 || ts < current)
            && self
                .recent_min_ts
                .compare_exchange_weak(current, ts, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            current = self.recent_min_ts.load(Ordering::Acquire);
        }
    }
}

/// 按版本降序插入/替换表历史记录。
fn insert_table(history: &mut Vec<TableRecord>, record: TableRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换库历史记录。
fn insert_schema(history: &mut Vec<SchemaRecord>, record: SchemaRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换分区历史记录。
fn insert_partition(history: &mut Vec<PartitionRecord>, record: PartitionRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换外键历史记录。
fn insert_fk(history: &mut Vec<ForeignKeyRecord>, record: ForeignKeyRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 取 `schema_version <= version` 且非 tomb 的最新表记录。
fn visible_table(history: Option<&Vec<TableRecord>>, version: i64) -> Option<TableRecord> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .cloned()
}
/// 取可见的最新库元数据。
fn visible_schema(history: Option<&Vec<SchemaRecord>>, version: i64) -> Option<Arc<DBInfo>> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.db.clone())
}
/// 取可见的分区→表 ID 映射。
fn visible_partition(history: Option<&Vec<PartitionRecord>>, version: i64) -> Option<i64> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.table_id)
}
/// 取可见的反向外键列表。
fn visible_fk(
    history: Option<&Vec<ForeignKeyRecord>>,
    version: i64,
) -> Option<Vec<ReferredFKInfo>> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.refs.clone())
}
/// InfoSchema V2 快照：持有共享 Data、当前 schema 元版本与读时间戳。
pub struct infoschemaV2 {
    pub Data: Arc<Data>,
    schema_meta_version: i64,
    start_ts: u64,
}

impl infoschemaV2 {
    /// 基于共享 Data 构造指定版本/时间戳的 V2 快照。
    pub fn new(data: Arc<Data>, schema_meta_version: i64, start_ts: u64) -> Self {
        Self {
            Data: data,
            schema_meta_version,
            start_ts,
        }
    }
    /// 克隆快照并更新读时间戳。
    pub fn CloneAndUpdateTS(&self, start_ts: u64) -> Self {
        Self::new(self.Data.clone(), self.schema_meta_version, start_ts)
    }
    /// 表是否已在 SIEVE 缓存中。
    pub fn TableIsCached(&self, id: i64) -> bool {
        self.Data.table_cache.Contains(&TableCacheKey {
            table_id: id,
            schema_version: self.schema_meta_version,
        })
    }
    /// 从缓存中淘汰指定表。
    pub fn EvictTable(&self, schema: &CiString, table: &CiString) {
        if let Some(record) = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_name
                .get(&(schema.lower.clone(), table.lower.clone())),
            self.schema_meta_version,
        ) {
            self.Data.table_cache.Remove(&TableCacheKey {
                table_id: record.table_id,
                schema_version: record.schema_version,
            });
        }
    }
    /// 按库表名取 `TableInfo`。
    pub fn TableInfoByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Arc<TableInfo>, InfoSchemaError> {
        self.TableByName(schema, table).map(|table| table.0)
    }
    /// 按表 ID 取 `TableInfo`。
    pub fn TableInfoByID(&self, id: i64) -> Option<Arc<TableInfo>> {
        self.TableByID(id).map(|table| table.0)
    }
    /// 列出指定 schema 在当前版本可见的全部表信息。
    pub fn SchemaTableInfos(
        &self,
        schema: &CiString,
    ) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        Ok(data
            .by_name
            .iter()
            .filter(|((db, _), _)| db == &schema.lower)
            .filter_map(|(_, history)| {
                visible_table(Some(history), self.schema_meta_version)
                    .and_then(|record| record.table.map(|table| table.0))
            })
            .collect())
    }
    /// 当前版本可见的全部 schema 名。
    pub fn AllSchemaNames(&self) -> Vec<CiString> {
        self.AllSchemas()
            .into_iter()
            .map(|db| db.name.clone())
            .collect()
    }
    /// schema 是否在当前版本可见。
    pub fn SchemaExists(&self, schema: &CiString) -> bool {
        self.SchemaByName(schema).is_some()
    }
    /// 库表是否在当前版本可见。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.TableByName(schema, table).is_ok()
    }
    /// 由分区 ID 解析所属表 ID。
    pub fn TableIDByPartitionID(&self, partition_id: i64) -> Option<i64> {
        visible_partition(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .partitions
                .get(&partition_id),
            self.schema_meta_version,
        )
    }
    /// 由分区 ID 得到 TableItem。
    pub fn TableItemByPartitionID(&self, partition_id: i64) -> Option<TableItem> {
        self.TableItemByID(self.TableIDByPartitionID(partition_id)?)
    }
    /// 遍历当前版本全部可见表的 TableItem；visit 返回 false 则停止。
    pub fn IterateAllTableItems(&self, mut visit: impl FnMut(TableItem) -> bool) {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        for history in data.by_id.values() {
            if let Some(record) = visible_table(Some(history), self.schema_meta_version) {
                if !visit(TableItem {
                    DBName: record.db_name,
                    TableName: record.table_name,
                }) {
                    break;
                }
            }
        }
    }
    /// 查询当前版本下引用指定父表的外键。
    pub fn GetTableReferredForeignKeys(&self, schema: &str, table: &str) -> Vec<ReferredFKInfo> {
        self.Data
            .getTableReferredForeignKeys(schema, table, self.schema_meta_version)
    }
}

impl InfoSchema for infoschemaV2 {
    fn SchemaMetaVersion(&self) -> i64 {
        self.schema_meta_version
    }
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        data.specials
            .get(&schema.lower)
            .map(|special| special.0.clone())
            .or_else(|| {
                visible_schema(
                    data.schema_by_name.get(&schema.lower),
                    self.schema_meta_version,
                )
            })
    }
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let name = data
            .schema_id_to_name
            .get(&id)?
            .iter()
            .find(|record| record.0 <= self.schema_meta_version)
            .filter(|record| !record.2)?
            .1
            .clone();
        visible_schema(
            data.schema_by_name.get(&name.lower),
            self.schema_meta_version,
        )
        .or_else(|| {
            data.specials
                .get(&name.lower)
                .map(|special| special.0.clone())
        })
    }
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError> {
        self.Data.keep_alive(self.start_ts);
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        if let Some((_, tables)) = data.specials.get(&schema.lower) {
            if let Some(found) = tables
                .iter()
                .find(|candidate| candidate.Meta().name.lower == table.lower)
            {
                return Ok(found.clone());
            }
        }
        let record = visible_table(
            data.by_name
                .get(&(schema.lower.clone(), table.lower.clone())),
            self.schema_meta_version,
        )
        .ok_or_else(|| InfoSchemaError {
            code: "ErrNoSuchTable",
            message: format!("{}.{}", schema.original, table.original),
        })?;
        drop(data);
        let key = TableCacheKey {
            table_id: record.table_id,
            schema_version: record.schema_version,
        };
        if let Some(cached) = self.Data.table_cache.Get(&key) {
            return Ok(cached);
        }
        let loaded = record.table.ok_or_else(|| InfoSchemaError {
            code: "ErrNoSuchTable",
            message: table.original.clone(),
        })?;
        self.Data.table_cache.Set(key, loaded.clone());
        Ok(loaded)
    }
    fn TableByID(&self, id: i64) -> Option<Table> {
        self.Data.keep_alive(self.start_ts);
        let record = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_id
                .get(&id),
            self.schema_meta_version,
        )?;
        let key = TableCacheKey {
            table_id: id,
            schema_version: record.schema_version,
        };
        if let Some(cached) = self.Data.table_cache.Get(&key) {
            return Some(cached);
        }
        let loaded = record.table?;
        self.Data.table_cache.Set(key, loaded.clone());
        Some(loaded)
    }
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        self.SchemaTableInfos(schema)
    }
    fn HasTemporaryTable(&self) -> bool {
        self.Data.hasTemporaryTable()
    }
    fn TableItemByID(&self, id: i64) -> Option<TableItem> {
        let record = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_id
                .get(&id),
            self.schema_meta_version,
        )?;
        Some(TableItem {
            DBName: record.db_name,
            TableName: record.table_name,
        })
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)> {
        let table = self.TableByID(self.TableIDByPartitionID(partition_id)?)?;
        let db = self.SchemaByID(table.Meta().db_id)?;
        let partition = table
            .Meta()
            .partition
            .as_ref()?
            .definitions
            .iter()
            .find(|partition| partition.id == partition_id)?
            .clone();
        Some((table, db, partition))
    }
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let mut schemas: Vec<_> = data
            .schema_by_name
            .values()
            .filter_map(|history| visible_schema(Some(history), self.schema_meta_version))
            .collect();
        schemas.extend(data.specials.values().map(|special| special.0.clone()));
        schemas
    }
    fn ListTablesWithSpecialAttribute(
        &self,
        filter: context_dependency::SpecialAttributeFilter,
    ) -> Vec<context_dependency::TableInfoResult> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let mut matches = data
            .by_id
            .values()
            .filter_map(|history| {
                let record = visible_table(Some(history), self.schema_meta_version)?;
                let table_info = record.table?.Meta().model_meta.clone()?;
                filter(table_info.as_ref()).then_some((record.db_name, record.table_id, table_info))
            })
            .collect::<Vec<_>>();
        drop(data);

        // Go's tableInfoResident btree is traversed with Descend: database
        // name, table ID, and schema version are all visited in descending
        // order. `visible_table` has already selected one version per ID.
        matches.sort_by(|left, right| {
            right
                .0
                .lower
                .cmp(&left.0.lower)
                .then_with(|| right.1.cmp(&left.1))
        });

        let mut results: Vec<context_dependency::TableInfoResult> = Vec::new();
        for (db_name, _, table_info) in matches {
            if let Some(current) = results
                .last_mut()
                .filter(|current| current.DBName.L == db_name.lower)
            {
                current.TableInfos.push(table_info);
                continue;
            }
            results.push(context_dependency::TableInfoResult {
                DBName: astersql_parser_ast::NewCIStr(&db_name.original),
                TableInfos: vec![table_info],
            });
        }
        results
    }
    fn IsV2(&self) -> bool {
        true
    }
    fn GCOldVersion(&self, cut_version: i64) -> Option<(usize, i64)> {
        Some(self.Data.GCOldVersion(cut_version))
    }
}

/// 构造共享的空 Data。
pub fn NewData() -> Arc<Data> {
    Arc::new(Data::new())
}
/// 构造 InfoSchema V2 快照。
pub fn NewInfoSchemaV2(data: Arc<Data>, schema_meta_version: i64, start_ts: u64) -> infoschemaV2 {
    infoschemaV2::new(data, schema_meta_version, start_ts)
}
/// 判断给定 InfoSchema 是否为 V2 实现。
pub fn IsV2(schema: &dyn InfoSchema) -> bool {
    schema.IsV2()
}
/// 是否为系统特殊库（information_schema / performance_schema / metrics_schema 等）。
pub fn IsSpecialDB(db_name: &str) -> bool {
    matches!(
        db_name.to_ascii_lowercase().as_str(),
        "information_schema" | "performance_schema" | "metrics_schema" | "inspection_schema"
    )
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 控制查表时是否回填（refill）SIEVE 缓存的选项。
pub struct RefillOption(pub bool);
/// 构造 RefillOption。
pub fn WithRefillOption(refill: bool) -> RefillOption {
    RefillOption(refill)
}
