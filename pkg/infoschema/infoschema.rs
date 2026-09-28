// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// InfoSchema（信息模式）核心实现：内存中的库/表/策略元数据视图。
//
// 对应 Go `pkg/infoschema`：提供按名/ID 查找 schema 与表、放置策略、资源组、
// 脱敏策略（Masking Policy）、外键反向引用，以及会话级临时表。
// MVCC（多版本并发控制）语义由上层 schema 版本号表达；本文件的可运行实现
// 位于机械草稿块注释之后，不发起 SQL / 网络 IO。

// 原 Go 中可能读取系统表或存储快照的调用仅保留调用形状；本文件不会执行 SQL、网络 IO 或后台任务。
// 指针、接口、锁、原子指针、singleflight、context 与所有权无法直接一比一表达时，均在相邻位置补充中文说明。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicInt64, AtomicUint64, Ordering};

// Go imports（不虚构尚未可用的 Rust crate 路径）：
// "cmp"
// stdctx "context"
// "fmt"
// "maps"
// "slices"
// "sort"
// "strings"
// "sync"
// "time"
// "github.com/ngaut/pools"
// "github.com/pingcap/errors"
// "github.com/pingcap/tidb/pkg/ddl/placement"
// "github.com/pingcap/tidb/pkg/infoschema/context"
// "github.com/pingcap/tidb/pkg/kv"
// "github.com/pingcap/tidb/pkg/meta/autoid"
// "github.com/pingcap/tidb/pkg/meta/metadef"
// "github.com/pingcap/tidb/pkg/meta/model"
// "github.com/pingcap/tidb/pkg/parser/ast"
// "github.com/pingcap/tidb/pkg/parser/mysql"
// "github.com/pingcap/tidb/pkg/parser/terror"
// "github.com/pingcap/tidb/pkg/sessionctx"
// "github.com/pingcap/tidb/pkg/table"
// "github.com/pingcap/tidb/pkg/util"
// "github.com/pingcap/tidb/pkg/util/chunk"
// "github.com/pingcap/tidb/pkg/util/intest"
// "github.com/pingcap/tidb/pkg/util/logutil"
// "github.com/pingcap/tidb/pkg/util/mock"
// "github.com/pingcap/tidb/pkg/util/sqlexec"
// "go.uber.org/zap"

// Go 编译期接口实现断言保留为说明；Rust trait 约束等待模块接线后恢复。
// var _ context.Misc = &infoSchema{}

// sortedTables 对应 Go 同名类型，底层集合或标量表示保持一致。
pub type sortedTables = Vec<table::Table>;

// searchTable 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn searchTable(s: &sortedTables, id: i64) -> i32 {
    let mut idx = sort.Search(len(s), func(i int) bool {
        return s[i].Meta().ID >= id
    })
    if idx == len(s) || s[idx].Meta().ID != id {
        return -1
    }
    return idx
}

// schemaTables 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct schemaTables {
    pub dbInfo: &mut model::DBInfo,
    pub tables: HashMap<String, table::Table>,
}

// bucketCount 保留 Go 常量值及其索引/容量语义。
pub const bucketCount: i32 = 512;

// infoSchema 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct infoSchema {
    pub infoSchemaMisc: infoSchemaMisc,
    pub schemaMap: HashMap<String, &mut schemaTables>,
    // schemaID2Name is a map from schema ID to schema name.
    // it should be enough to query by name only theoretically, but there are some
    // places we only have schema ID, and we check both name and id in some sanity checks.
    pub schemaID2Name: HashMap<i64, String>,

    // sortedTablesBuckets is a slice of sortedTables, a table's bucket index is (tableID % bucketCount).
    pub sortedTablesBuckets: Vec<sortedTables>,

    // referredForeignKeyMap records all table's ReferredFKInfo.
    // referredSchemaAndTableName => child SchemaAndTableAndForeignKeyName => *model.ReferredFKInfo
    pub referredForeignKeyMap: HashMap<SchemaAndTableName, Vec<&mut model::ReferredFKInfo>>,
    // maskingPolicyTableColumnMap stores masking policy metadata by table and column IDs.
    // Note: Policy name is only unique per table, not globally. We use [TableID][ColumnID] as key
    // to avoid name collision when different tables have policies with the same name.
    pub maskingPolicyTableColumnMap: HashMap<i64, HashMap<i64, &mut model::MaskingPolicyInfo>>,
    // maskingPoliciesLoaded indicates whether masking policies have been loaded.
    pub maskingPoliciesLoaded: bool,
    // maskingPoliciesLoadCh is non-None when a masking-policy load is in progress.
    // Waiters block on this channel to avoid serving partially initialized policy maps.
    pub maskingPoliciesLoadCh: chan struct{},
    // maskingPolicyMutex protects maskingPolicyTableColumnMap and loading state.
    pub maskingPolicyMutex: sync::RWMutex,
    // factory is used to execute SQL for delayed loading of masking policies.
    pub factory: func() (pools::Resource, error),
    // ts is the timestamp at which this InfoSchema was loaded.
    // Used for snapshot-aware lazy loading of masking policies.
    pub ts: u64,

    pub r: autoid::Requirement,
}

// infoSchemaMisc 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct infoSchemaMisc {
    // schemaMetaVersion is the version of schema, and we should check version when change schema.
    pub schemaMetaVersion: i64,

    // ruleBundleMap stores all placement rules
    pub ruleBundleMap: HashMap<i64, &mut placement::Bundle>,

    // policyMap stores all placement policies.
    pub policyMutex: sync::RWMutex,
    pub policyMap: HashMap<String, &mut model::PolicyInfo>,

    // resourceGroupMap stores all resource groups.
    pub resourceGroupMutex: sync::RWMutex,
    pub resourceGroupMap: HashMap<String, &mut model::ResourceGroupInfo>,

    // temporaryTables stores the temporary table ids
    pub temporaryTableIDs: HashMap<i64, struct{}>,
}

// SchemaAndTableName contains the lower-case schema name and table name.
// SchemaAndTableName 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct SchemaAndTableName {
    pub schema: String,
    pub table: String,
}

// MockInfoSchema only serves for test.
// MockInfoSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn MockInfoSchema(tbList: Vec<&mut model::TableInfo>) -> InfoSchema {
    let mut result = newInfoSchema(None, None)
    let mut dbInfo = &model.DBInfo{ID: 1, Name: ast.NewCIStr("test")}
    dbInfo.Deprecated.Tables = tbList
    let mut tableNames = &schemaTables{
        dbInfo: dbInfo,
        tables: make(map[string]table.Table),
    }
    result.addSchema(tableNames)
    var tableIDs map[int64]struct{}
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tb := range tbList {
        intest.AssertFunc(func() bool {
            if tableIDs == None {
                tableIDs = make(map[int64]struct{})
            }
            let mut (_, ok) = tableIDs[tb.ID]
            intest.Assert(!ok)
            tableIDs[tb.ID] = struct{}{}
            return true
        })
        tb.DBID = dbInfo.ID
        let mut tbl = table.MockTableFromMeta(tb)
        tableNames.tables[tb.Name.L] = tbl
        let mut bucketIdx = tableBucketIdx(tb.ID)
        result.sortedTablesBuckets[bucketIdx] = append(result.sortedTablesBuckets[bucketIdx], tbl)
    }
    // Add a system table.
    let mut tables = []*model.TableInfo{
        {
            // Use a very big ID to avoid conflict with normal tables.
            ID:   9999,
            Name: ast.NewCIStr("stats_meta"),
            Columns: []*model.ColumnInfo{
                {
                    State:  model.StatePublic,
                    Offset: 0,
                    Name:   ast.NewCIStr("a"),
                    ID:     1,
                },
            },
            State: model.StatePublic,
        },
    }
    let mut mysqlDBInfo = &model.DBInfo{ID: 2, Name: ast.NewCIStr("mysql")}
    mysqlDBInfo.Deprecated.Tables = tables
    tableNames = &schemaTables{
        dbInfo: mysqlDBInfo,
        tables: make(map[string]table.Table),
    }
    result.addSchema(tableNames)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tb := range tables {
        tb.DBID = mysqlDBInfo.ID
        let mut tbl = table.MockTableFromMeta(tb)
        tableNames.tables[tb.Name.L] = tbl
        let mut bucketIdx = tableBucketIdx(tb.ID)
        result.sortedTablesBuckets[bucketIdx] = append(result.sortedTablesBuckets[bucketIdx], tbl)
    }
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for i := range result.sortedTablesBuckets {
        slices.SortFunc(result.sortedTablesBuckets[i], func(i, j table.Table) int {
            return cmp.Compare(i.Meta().ID, j.Meta().ID)
        })
    }
    return result
}

// MockInfoSchemaWithSchemaVer only serves for test.
// MockInfoSchemaWithSchemaVer 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn MockInfoSchemaWithSchemaVer(tbList: Vec<&mut model::TableInfo>, schemaVer: i64) -> InfoSchema {
    let mut result = newInfoSchema(None, None)
    let mut dbInfo = &model.DBInfo{ID: 1, Name: ast.NewCIStr("test")}
    dbInfo.Deprecated.Tables = tbList
    let mut tableNames = &schemaTables{
        dbInfo: dbInfo,
        tables: make(map[string]table.Table),
    }
    result.addSchema(tableNames)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tb := range tbList {
        tb.DBID = dbInfo.ID
        let mut tbl = table.MockTableFromMeta(tb)
        tableNames.tables[tb.Name.L] = tbl
        let mut bucketIdx = tableBucketIdx(tb.ID)
        result.sortedTablesBuckets[bucketIdx] = append(result.sortedTablesBuckets[bucketIdx], tbl)
    }
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for i := range result.sortedTablesBuckets {
        slices.SortFunc(result.sortedTablesBuckets[i], func(i, j table.Table) int {
            return cmp.Compare(i.Meta().ID, j.Meta().ID)
        })
    }
    result.schemaMetaVersion = schemaVer
    return result
}

// Go 编译期接口实现断言保留为说明；Rust trait 约束等待模块接线后恢复。
// var _ InfoSchema = (*infoSchema)(nil)

// base 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn base(is: &mut infoSchema) -> &mut infoSchema {
    return is
}

// newInfoSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn newInfoSchema(r: autoid::Requirement, factory: func() (pools::Resource, error)) -> &mut infoSchema {
    return &infoSchema{
        infoSchemaMisc: infoSchemaMisc{
            policyMap:        map[string]*model.PolicyInfo{},
            resourceGroupMap: map[string]*model.ResourceGroupInfo{},
            ruleBundleMap:    map[int64]*placement.Bundle{},
        },
        schemaMap:                   map[string]*schemaTables{},
        schemaID2Name:               map[int64]string{},
        sortedTablesBuckets:         make([]sortedTables, bucketCount),
        referredForeignKeyMap:       make(map[SchemaAndTableName][]*model.ReferredFKInfo),
        maskingPolicyTableColumnMap: make(map[int64]map[int64]*model.MaskingPolicyInfo),
        factory:                     factory,
        r:                           r,
    }
}

// SchemaByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByName(is: &mut infoSchema, schema: ast::CIStr) -> (&mut model::DBInfo, bool) {
    return is.schemaByName(schema.L)
}

// schemaByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn schemaByName(is: &mut infoSchema, name: String) -> (&mut model::DBInfo, bool) {
    let mut (tableNames, ok) = is.schemaMap[name]
    if !ok {
        return
    }
    return tableNames.dbInfo, true
}

// SchemaExists 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaExists(is: &mut infoSchema, schema: ast::CIStr) -> bool {
    let mut (_, ok) = is.schemaMap[schema.L]
    return ok
}

// TableByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByName(is: &mut infoSchema, ctx: stdctx::Context, schema: ast::CIStr, table: ast::CIStr) -> (table::Table, errors::Error) {
    if tbNames, ok := is.schemaMap[schema.L]; ok {
        if t, ok = tbNames.tables[table.L]; ok {
            return
        }
    }
    return None, ErrTableNotExists.FastGenByArgs(schema, table)
}

// TableInfoByName implements InfoSchema.TableInfoByName
// TableInfoByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByName(is: &mut infoSchema, schema: ast::CIStr, table: ast::CIStr) -> (&mut model::TableInfo, errors::Error) {
    let mut (tbl, err) = is.TableByName(stdctx.Background(), schema, table)
    return getTableInfo(tbl), err
}

// TableIsView indicates whether the schema.table is a view.
// TableIsView 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableIsView(is: InfoSchema, schema: ast::CIStr, table: ast::CIStr) -> bool {
    let mut (tbl, err) = is.TableByName(stdctx.Background(), schema, table)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err == None {
        return tbl.Meta().IsView()
    }
    return false
}

// TableIsSequence indicates whether the schema.table is a sequence.
// TableIsSequence 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableIsSequence(is: InfoSchema, schema: ast::CIStr, table: ast::CIStr) -> bool {
    let mut (tbl, err) = is.TableByName(stdctx.Background(), schema, table)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err == None {
        return tbl.Meta().IsSequence()
    }
    return false
}

// TableExists 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableExists(is: &mut infoSchema, schema: ast::CIStr, table: ast::CIStr) -> bool {
    if tbNames, ok := is.schemaMap[schema.L]; ok {
        if _, ok = tbNames.tables[table.L]; ok {
            return true
        }
    }
    return false
}

// PolicyByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn PolicyByID(is: &mut infoSchema, id: i64) -> (&mut model::PolicyInfo, bool) {
    // TODO: use another hash map to avoid traveling on the policy map
    for _, v := range is.policyMap {
        if v.ID == id {
            return v, true
        }
    }
    return None, false
}

// MaskingPolicyByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn MaskingPolicyByID(is: &mut infoSchema, id: i64) -> (&mut model::MaskingPolicyInfo, bool) {
    is.loadMaskingPoliciesIfNeeded()
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.RUnlock()
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, colMap := range is.maskingPolicyTableColumnMap {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, policy := range colMap {
            if policy.ID == id {
                return policy, true
            }
        }
    }
    return None, false
}

// SchemaByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByID(is: &mut infoSchema, id: i64) -> (&mut model::DBInfo, bool) {
    let mut (name, ok) = is.schemaID2Name[id]
    if !ok {
        return None, false
    }
    return is.schemaByName(name)
}

// SchemaByTable get a table's schema name
// SchemaByTable 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByTable(is: InfoSchema, tableInfo: &mut model::TableInfo) -> (&mut model::DBInfo, bool) {
    if tableInfo == None {
        return None, false
    }
    if tableInfo.DBID > 0 {
        return is.SchemaByID(tableInfo.DBID)
    }
    let mut (tbl, ok) = is.TableByID(stdctx.Background(), tableInfo.ID)
    if !ok {
        return None, false
    }
    return is.SchemaByID(tbl.Meta().DBID)
}

// TableByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByID(is: &mut infoSchema, _: stdctx::Context, id: i64) -> (table::Table, bool) {
    if !tableIDIsValid(id) {
        return None, false
    }

    let mut slice = is.sortedTablesBuckets[tableBucketIdx(id)]
    let mut idx = slice.searchTable(id)
    if idx == -1 {
        return None, false
    }
    return slice[idx], true
}

// TableItemByID implements InfoSchema.TableItemByID.
// TableItemByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableItemByID(is: &mut infoSchema, id: i64) -> (TableItem, bool) {
    let mut (tbl, ok) = is.TableByID(stdctx.Background(), id)
    if !ok {
        return TableItem{}, false
    }
    let mut (db, ok) = is.SchemaByID(tbl.Meta().DBID)
    if !ok {
        return TableItem{}, false
    }
    return TableItem{DBName: db.Name, TableName: tbl.Meta().Name}, true
}

// TableInfoByID implements InfoSchema.TableInfoByID
// TableInfoByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByID(is: &mut infoSchema, id: i64) -> (&mut model::TableInfo, bool) {
    let mut (tbl, ok) = is.TableByID(stdctx.Background(), id)
    return getTableInfo(tbl), ok
}

// FindTableInfoByPartitionID implements InfoSchema.FindTableInfoByPartitionID
// FindTableInfoByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableInfoByPartitionID(is: &mut infoSchema, partitionID: i64) -> (&mut model::TableInfo, &mut model::DBInfo, &mut model::PartitionDefinition) {
    let mut (tbl, db, partDef) = is.FindTableByPartitionID(partitionID)
    return getTableInfo(tbl), db, partDef
}

// SchemaTableInfos implements MetaOnlyInfoSchema.
// SchemaTableInfos 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaTableInfos(is: &mut infoSchema, ctx: stdctx::Context, schema: ast::CIStr) -> (Vec<&mut model::TableInfo>, errors::Error) {
    let mut (schemaTables, ok) = is.schemaMap[schema.L]
    if !ok {
        return None, None
    }
    let mut tables = make([]*model.TableInfo, 0, len(schemaTables.tables))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tbl := range schemaTables.tables {
        tables = append(tables, tbl.Meta())
    }
    return tables, None
}

// SchemaSimpleTableInfos implements MetaOnlyInfoSchema.
// SchemaSimpleTableInfos 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaSimpleTableInfos(is: &mut infoSchema, ctx: stdctx::Context, schema: ast::CIStr) -> (Vec<&mut model::TableNameInfo>, errors::Error) {
    let mut (schemaTables, ok) = is.schemaMap[schema.L]
    if !ok {
        return None, None
    }
    let mut ret = make([]*model.TableNameInfo, 0, len(schemaTables.tables))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, t := range schemaTables.tables {
        ret = append(ret, &model.TableNameInfo{
            ID:   t.Meta().ID,
            Name: t.Meta().Name,
        })
    }
    return ret, None
}

// ListTablesWithSpecialAttribute 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn ListTablesWithSpecialAttribute(is: &mut infoSchema, filter: context::SpecialAttributeFilter) -> Vec<context::TableInfoResult> {
    let mut ret = make([]context.TableInfoResult, 0, 10)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, dbName := range is.AllSchemaNames() {
        let mut res = context.TableInfoResult{DBName: dbName}
        let mut (tblInfos, err) = is.SchemaTableInfos(stdctx.Background(), dbName)
        terror.Log(err)
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, tblInfo := range tblInfos {
            if !filter(tblInfo) {
                continue
            }
            res.TableInfos = append(res.TableInfos, tblInfo)
        }
        ret = append(ret, res)
    }
    return ret
}

// AllSchemaNames returns all the schemas' names.
// AllSchemaNames 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn AllSchemaNames(is: InfoSchema) -> (Vec<String>) {
    let mut schemas = is.AllSchemaNames()
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range schemas {
        names = append(names, v.O)
    }
    return
}

// AllSchemas 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn AllSchemas(is: &mut infoSchema) -> (Vec<&mut model::DBInfo>) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range is.schemaMap {
        schemas = append(schemas, v.dbInfo)
    }
    return
}

// AllSchemaNames 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn AllSchemaNames(is: &mut infoSchema) -> (Vec<ast::CIStr>) {
    let mut rs = make([]ast.CIStr, 0, len(is.schemaMap))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range is.schemaMap {
        rs = append(rs, v.dbInfo.Name)
    }
    return rs
}

// TableItemByPartitionID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableItemByPartitionID(is: &mut infoSchema, partitionID: i64) -> (TableItem, bool) {
    let mut (tbl, db, _) = is.FindTableByPartitionID(partitionID)
    if tbl == None {
        return TableItem{}, false
    }
    return TableItem{DBName: db.Name, TableName: tbl.Meta().Name}, true
}

// TableIDByPartitionID implements InfoSchema.TableIDByPartitionID.
// TableIDByPartitionID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableIDByPartitionID(is: &mut infoSchema, partitionID: i64) -> (i64, bool) {
    let mut (tbl, _, _) = is.FindTableByPartitionID(partitionID)
    if tbl == None {
        return
    }
    return tbl.Meta().ID, true
}

// FindTableByPartitionID finds the partition-table info by the partitionID.
// FindTableByPartitionID will traverse all the tables to find the partitionID partition in which partition-table.
// FindTableByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableByPartitionID(is: &mut infoSchema, partitionID: i64) -> (table::Table, &mut model::DBInfo, &mut model::PartitionDefinition) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range is.schemaMap {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, tbl := range v.tables {
            let mut pi = tbl.Meta().GetPartitionInfo()
            if pi == None {
                continue
            }
            // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
            for _, p := range pi.Definitions {
                if p.ID == partitionID {
                    return tbl, v.dbInfo, &p
                }
            }
        }
    }
    return None, None, None
}

// addSchema is used to add a schema to the infoSchema, it will overwrite the old
// one if it already exists.
// addSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn addSchema(is: &mut infoSchema, st: &mut schemaTables) {
    is.schemaMap[st.dbInfo.Name.L] = st
    is.schemaID2Name[st.dbInfo.ID] = st.dbInfo.Name.L
}

// delSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn delSchema(is: &mut infoSchema, di: &mut model::DBInfo) {
    delete(is.schemaMap, di.Name.L)
    delete(is.schemaID2Name, di.ID)
}

// HasTemporaryTable returns whether information schema has temporary table
// HasTemporaryTable 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn HasTemporaryTable(is: &mut infoSchemaMisc) -> bool {
    return len(is.temporaryTableIDs) != 0
}

// SchemaMetaVersion 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaMetaVersion(is: &mut infoSchemaMisc) -> i64 {
    return is.schemaMetaVersion
}

// GetSequenceByName gets the sequence by name.
// GetSequenceByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn GetSequenceByName(is: InfoSchema, schema: ast::CIStr, sequence: ast::CIStr) -> (util::SequenceTable, errors::Error) {
    let mut (tbl, err) = is.TableByName(stdctx.Background(), schema, sequence)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }
    if !tbl.Meta().IsSequence() {
        return None, ErrWrongObject.GenWithStackByArgs(schema, sequence, "SEQUENCE")
    }
    return tbl.(util.SequenceTable), None
}

// init 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn init() {
    // Initialize the information shema database and register the driver to `drivers`
    let mut dbID = autoid.InformationSchemaDBID
    let mut infoSchemaTables = make([]*model.TableInfo, 0, len(tableNameToColumns))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for name, cols := range tableNameToColumns {
        let mut tableInfo = buildTableMeta(name, cols)
        tableInfo.DBID = dbID
        infoSchemaTables = append(infoSchemaTables, tableInfo)
        var ok bool
        tableInfo.ID, ok = tableIDMap[tableInfo.Name.O]
        if !ok {
            panic(fmt.Sprintf("get information_schema table id failed, unknown system table `%v`", tableInfo.Name.O))
        }
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for i, c := range tableInfo.Columns {
            c.ID = int64(i) + 1
        }
        tableInfo.MaxColumnID = int64(len(tableInfo.Columns))
        tableInfo.MaxIndexID = int64(len(tableInfo.Indices))
    }
    let mut infoSchemaDB = &model.DBInfo{
        ID:      dbID,
        Name:    metadef.InformationSchemaName,
        Charset: mysql.DefaultCharset,
        Collate: mysql.DefaultCollationName,
    }
    infoSchemaDB.Deprecated.Tables = infoSchemaTables
    RegisterVirtualTable(infoSchemaDB, createInfoSchemaTable)
    util.GetSequenceByName = func(is context.MetaOnlyInfoSchema, schema, sequence ast.CIStr) (util.SequenceTable, error) {
        return GetSequenceByName(is.(InfoSchema), schema, sequence)
    }
    mock.MockInfoschema = func(tbList []*model.TableInfo) context.MetaOnlyInfoSchema {
        return MockInfoSchema(tbList)
    }
}

// HasAutoIncrementColumn checks whether the table has auto_increment columns, if so, return true and the column name.
// HasAutoIncrementColumn 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn HasAutoIncrementColumn(tbInfo: &mut model::TableInfo) -> (bool, String) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, col := range tbInfo.Columns {
        if mysql.HasAutoIncrementFlag(col.GetFlag()) {
            return true, col.Name.L
        }
    }
    return false, ""
}

// PolicyByName is used to find the policy.
// PolicyByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn PolicyByName(is: &mut infoSchemaMisc, name: ast::CIStr) -> (&mut model::PolicyInfo, bool) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.policyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.policyMutex.RUnlock()
    let mut (t, r) = is.policyMap[name.L]
    return t, r
}

// ResourceGroupByName is used to find the resource group.
// ResourceGroupByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn ResourceGroupByName(is: &mut infoSchemaMisc, name: ast::CIStr) -> (&mut model::ResourceGroupInfo, bool) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.RUnlock()
    let mut (t, r) = is.resourceGroupMap[name.L]
    return t, r
}

// MaskingPolicyByName returns masking policy metadata by policy name with delayed loading.
// Note: Policy name is only unique per table, not globally. This method returns the first matching
// policy if multiple tables have policies with the same name. For precise lookup, use MaskingPolicyByTableColumn.
// MaskingPolicyByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn MaskingPolicyByName(is: &mut infoSchema, name: ast::CIStr) -> (&mut model::MaskingPolicyInfo, bool) {
    is.loadMaskingPoliciesIfNeeded()

    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.RUnlock()
    var found *model.MaskingPolicyInfo
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, colMap := range is.maskingPolicyTableColumnMap {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, policy := range colMap {
            if policy.Name.L == name.L {
                if found != None {
                    return None, false
                }
                found = policy
            }
        }
    }
    return found, found != None
}

// MaskingPolicyByTableColumn returns masking policy metadata by table and column IDs with delayed loading.
// MaskingPolicyByTableColumn 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn MaskingPolicyByTableColumn(is: &mut infoSchema, tableID: i64, columnID: i64) -> (&mut model::MaskingPolicyInfo, bool) {
    is.loadMaskingPoliciesIfNeeded()

    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.RUnlock()
    let mut (colMap, ok) = is.maskingPolicyTableColumnMap[tableID]
    if !ok {
        return None, false
    }
    let mut (t, r) = colMap[columnID]
    return t, r
}

// ResourceGroupByID is used to find the resource group.
// ResourceGroupByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn ResourceGroupByID(is: &mut infoSchemaMisc, id: i64) -> (&mut model::ResourceGroupInfo, bool) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.RUnlock()
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range is.resourceGroupMap {
        if v.ID == id {
            return v, true
        }
    }
    return None, false
}

// AllResourceGroups returns all resource groups.
// AllResourceGroups 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn AllResourceGroups(is: &mut infoSchemaMisc) -> Vec<&mut model::ResourceGroupInfo> {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.RUnlock()
    let mut groups = make([]*model.ResourceGroupInfo, 0, len(is.resourceGroupMap))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, group := range is.resourceGroupMap {
        groups = append(groups, group)
    }
    return groups
}

// CloneResourceGroups 对应 Go 同名函数或方法：保留 Go 的快照复制与会话隔离语义，避免共享可变状态泄漏。
pub fn CloneResourceGroups(is: &mut infoSchemaMisc) -> HashMap<String, &mut model::ResourceGroupInfo> {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.RUnlock()
    return maps.Clone(is.resourceGroupMap)
}

// AllMaskingPolicies returns all masking policies in a stable order with delayed loading.
// AllMaskingPolicies 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn AllMaskingPolicies(is: &mut infoSchema) -> Vec<&mut model::MaskingPolicyInfo> {
    is.loadMaskingPoliciesIfNeeded()

    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.RUnlock()
    let mut policies = make([]*model.MaskingPolicyInfo, 0)
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, colMap := range is.maskingPolicyTableColumnMap {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, policy := range colMap {
            policies = append(policies, policy)
        }
    }
    sort.Slice(policies, func(i, j int) bool {
        if policies[i].Name.L == policies[j].Name.L {
            return policies[i].ID < policies[j].ID
        }
        return policies[i].Name.L < policies[j].Name.L
    })
    return policies
}

// CloneMaskingPoliciesByTableColumn 对应 Go 同名函数或方法：保留 Go 的快照复制与会话隔离语义，避免共享可变状态泄漏。
pub fn CloneMaskingPoliciesByTableColumn(is: &mut infoSchema) -> HashMap<i64, HashMap<i64, &mut model::MaskingPolicyInfo>> {
    is.loadMaskingPoliciesIfNeeded()

    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.RUnlock()
    let mut cloned = make(map[int64]map[int64]*model.MaskingPolicyInfo, len(is.maskingPolicyTableColumnMap))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for tableID, colMap := range is.maskingPolicyTableColumnMap {
        cloned[tableID] = maps.Clone(colMap)
    }
    return cloned
}

// loadMaskingPoliciesIfNeeded loads masking policies from system table on first access.
// Only one goroutine performs loading, others wait for completion.
// loadMaskingPoliciesIfNeeded 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn loadMaskingPoliciesIfNeeded(is: &mut infoSchema) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for {
        var loadCh chan struct{}
        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        is.maskingPolicyMutex.Lock()
        if is.maskingPoliciesLoaded {
            is.maskingPolicyMutex.Unlock()
            return
        }
        if is.factory == None {
            logutil.BgLogger().Debug("factory is None, skipping masking policies loading")
            is.maskingPoliciesLoaded = true
            is.maskingPolicyMutex.Unlock()
            return
        }
        if is.maskingPoliciesLoadCh != None {
            loadCh = is.maskingPoliciesLoadCh
            is.maskingPolicyMutex.Unlock()
            <-loadCh
            continue
        }
        loadCh = make(chan struct{})
        is.maskingPoliciesLoadCh = loadCh
        is.maskingPolicyMutex.Unlock()

        let mut (policies, err) = LoadMaskingPolicies(is.factory, is.ts)

        // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
        is.maskingPolicyMutex.Lock()
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            if isMaskingPolicyTableNotReady(err) {
                logutil.BgLogger().Debug("masking policy table not available yet, skipping", zap.Error(err))
                is.maskingPoliciesLoaded = true
            } else {
                logutil.BgLogger().Warn("failed to load masking policies", zap.Error(err))
            }
        } else if !is.maskingPoliciesLoaded {
            let mut newMap = make(map[int64]map[int64]*model.MaskingPolicyInfo, len(policies))
            // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
            for _, policy := range policies {
                if newMap[policy.TableID] == None {
                    newMap[policy.TableID] = make(map[int64]*model.MaskingPolicyInfo)
                }
                newMap[policy.TableID][policy.ColumnID] = policy
            }
            is.maskingPolicyTableColumnMap = newMap
            is.maskingPoliciesLoaded = true
            logutil.BgLogger().Info("masking policies loaded", zap.Int("count", len(policies)))
        }
        close(loadCh)
        is.maskingPoliciesLoadCh = None
        is.maskingPolicyMutex.Unlock()
        return
    }
}

// LoadMaskingPolicies loads all masking policy metadata through mysql.tidb_masking_policy.
// LoadMaskingPolicies 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn LoadMaskingPolicies(factory: func() (pools::Resource, error), snapshotTS: u64) -> (Vec<&mut model::MaskingPolicyInfo>, errors::Error) {
    return loadMaskingPoliciesWithTableIDs(factory, None, snapshotTS)
}

// loadMaskingPoliciesWithTableIDs loads masking policy metadata through mysql.tidb_masking_policy.
// If tableIDs is empty, all policies are loaded.
// snapshotTS is used for snapshot-aware loading: when non-zero, the query runs at that timestamp
// to preserve stale-read semantics.
// loadMaskingPoliciesWithTableIDs 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn loadMaskingPoliciesWithTableIDs(factory: func() (pools::Resource, error), tableIDs: Vec<i64>, snapshotTS: u64) -> (Vec<&mut model::MaskingPolicyInfo>, errors::Error) {
    const maxBatchSize = 1024

    let mut (resource, err) = factory()
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, errors.Trace(err)
    }
    if closer, ok := resource.(interface{ Close() }); ok {
        // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
        defer closer.Close()
    }

    let mut (sctx, ok) = resource.(sessionctx.Context)
    if !ok {
        return None, errors.New("failed to cast resource to sessionctx.Context")
    }

    let mut (ids, hasFilter) = normalizeMaskingPolicyTableIDs(tableIDs)
    if hasFilter && len(ids) == 0 {
        return None, None
    }

    let mut loadBatch = func(batchIDs []int64, policies []*model.MaskingPolicyInfo) ([]*model.MaskingPolicyInfo, error) {
        let mut (query, args) = buildLoadMaskingPoliciesQuery(batchIDs)
        let mut internalCtx = kv.WithInternalSourceType(stdctx.Background(), kv.InternalTxnDDL)
        let mut opts = []sqlexec.OptionFuncAlias{sqlexec.ExecOptionUseCurSession}
        if snapshotTS > 0 {
            opts = append(opts, sqlexec.ExecOptionWithSnapshot(snapshotTS))
        }
        let mut (rows, _, err) = sctx.GetRestrictedSQLExecutor().ExecRestrictedSQL(
            internalCtx,
            opts,
            query,
            args...,
        )
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, errors.Trace(err)
        }

        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, row := range rows {
            let mut (policy, err) = maskingPolicyInfoFromChunkRow(row)
            // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
            if err != None {
                return None, errors.Trace(err)
            }
            policies = append(policies, policy)
        }
        return policies, None
    }

    let mut policies = make([]*model.MaskingPolicyInfo, 0)
    if !hasFilter {
        policies, err = loadBatch(None, policies)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, err
        }
    } else {
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for start := 0; start < len(ids); start += maxBatchSize {
            let mut end = min(start+maxBatchSize, len(ids))
            policies, err = loadBatch(ids[start:end], policies)
            // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
            if err != None {
                return None, err
            }
        }
    }

    slices.SortFunc(policies, func(a, b *model.MaskingPolicyInfo) int {
        if x := cmp.Compare(a.TableID, b.TableID); x != 0 {
            return x
        }
        if x := cmp.Compare(a.ColumnID, b.ColumnID); x != 0 {
            return x
        }
        return cmp.Compare(a.ID, b.ID)
    })
    return policies, None
}

// buildLoadMaskingPoliciesQuery 对应 Go 同名函数或方法：保留读取、资源取得、结果解析和错误传播顺序；本身不会执行 SQL 或外部 IO。
pub fn buildLoadMaskingPoliciesQuery(tableIDs: Vec<i64>) -> (String, Vec<Box<dyn Any>>) {
    const baseQuery = `SELECT policy_id, policy_name, db_name, table_name, table_id, column_name, column_id, expression, status, masking_type, restrict_on, created_at, updated_at, created_by
FROM mysql.tidb_masking_policy`

    var sb strings.Builder
    sb.WriteString(baseQuery)
    let mut args = make([]any, 0, len(tableIDs))
    if len(tableIDs) > 0 {
        sb.WriteString(" WHERE table_id IN (")
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for i, id := range tableIDs {
            if i > 0 {
                sb.WriteString(", ")
            }
            sb.WriteString("%?")
            args = append(args, id)
        }
        sb.WriteString(")")
    }
    sb.WriteString(" ORDER BY table_id, column_id, policy_id")
    return sb.String(), args
}

// normalizeMaskingPolicyTableIDs 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn normalizeMaskingPolicyTableIDs(tableIDs: Vec<i64>) -> (Vec<i64>, bool) {
    let mut hasFilter = len(tableIDs) > 0
    if !hasFilter {
        return None, false
    }

    let mut idSet = make(map[int64]struct{}, len(tableIDs))
    let mut ids = make([]int64, 0, len(tableIDs))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, id := range tableIDs {
        if id <= 0 {
            continue
        }
        if _, ok := idSet[id]; ok {
            continue
        }
        idSet[id] = struct{}{}
        ids = append(ids, id)
    }
    slices.Sort(ids)
    return ids, true
}

// maskingPolicyInfoFromChunkRow 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn maskingPolicyInfoFromChunkRow(row: chunk::Row) -> (&mut model::MaskingPolicyInfo, errors::Error) {
    let mut (status, err) = maskingPolicyStatusFromString(row.GetString(8))
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }
    let mut restrictOn = ""
    if !row.IsNull(10) {
        restrictOn = row.GetString(10)
    }
    let mut (restrictOps, err) = maskingPolicyRestrictOpsFromString(restrictOn)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }

    let mut createdAt = time.Time{}
    if !row.IsNull(11) {
        createdAt, err = row.GetTime(11).GoTime(time.Local)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, errors.Trace(err)
        }
    }
    let mut updatedAt = time.Time{}
    if !row.IsNull(12) {
        updatedAt, err = row.GetTime(12).GoTime(time.Local)
        // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
        if err != None {
            return None, errors.Trace(err)
        }
    }
    let mut createdBy = ""
    if !row.IsNull(13) {
        createdBy = row.GetString(13)
    }
    let mut (maskingType, err) = maskingPolicyTypeFromString(row.GetString(9))
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return None, err
    }

    return &model.MaskingPolicyInfo{
        ID:          row.GetInt64(0),
        Name:        ast.NewCIStr(row.GetString(1)),
        DBName:      ast.NewCIStr(row.GetString(2)),
        TableName:   ast.NewCIStr(row.GetString(3)),
        TableID:     row.GetInt64(4),
        ColumnName:  ast.NewCIStr(row.GetString(5)),
        ColumnID:    row.GetInt64(6),
        Expression:  row.GetString(7),
        Status:      status,
        MaskingType: maskingType,
        RestrictOps: restrictOps,
        CreatedAt:   createdAt,
        UpdatedAt:   updatedAt,
        CreatedBy:   createdBy,
        State:       model.StatePublic,
    }, None
}

// maskingPolicyStatusFromString 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn maskingPolicyStatusFromString(status: String) -> (model::MaskingPolicyStatus, errors::Error) {
    switch strings.ToUpper(strings.TrimSpace(status)) {
    case "ENABLE", "ENABLED":
        return model.MaskingPolicyStatusEnable, None
    case "DISABLE", "DISABLED":
        return model.MaskingPolicyStatusDisable, None
    default:
        return model.MaskingPolicyStatusDisable, errors.Errorf("unknown masking policy status: %s", status)
    }
}

// maskingPolicyTypeFromString 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn maskingPolicyTypeFromString(tp: String) -> (model::MaskingPolicyType, errors::Error) {
    let mut normalized = model.MaskingPolicyType(strings.ToUpper(strings.TrimSpace(tp)))
    switch normalized {
    case model.MaskingPolicyTypeFull,
        model.MaskingPolicyTypePartial,
        model.MaskingPolicyTypeNull,
        model.MaskingPolicyTypeDate,
        model.MaskingPolicyTypeCustom:
        return normalized, None
    default:
        return "", errors.Errorf("unknown masking policy type: %s", tp)
    }
}

// maskingPolicyRestrictOpsFromString 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn maskingPolicyRestrictOpsFromString(restrictOn: String) -> (ast::MaskingPolicyRestrictOps, errors::Error) {
    restrictOn = strings.TrimSpace(strings.ToUpper(restrictOn))
    if restrictOn == "" || restrictOn == "NONE" {
        return ast.MaskingPolicyRestrictOpNone, None
    }
    let mut ops = ast.MaskingPolicyRestrictOpNone
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, token := range strings.Split(restrictOn, ",") {
        switch strings.TrimSpace(token) {
        case ast.MaskingPolicyRestrictNameInsertIntoSelect:
            ops |= ast.MaskingPolicyRestrictOpInsertIntoSelect
        case ast.MaskingPolicyRestrictNameUpdateSelect:
            ops |= ast.MaskingPolicyRestrictOpUpdateSelect
        case ast.MaskingPolicyRestrictNameDeleteSelect:
            ops |= ast.MaskingPolicyRestrictOpDeleteSelect
        case ast.MaskingPolicyRestrictNameCTAS:
            ops |= ast.MaskingPolicyRestrictOpCTAS
        case "NONE", "":
            // No-op.
        default:
            return ast.MaskingPolicyRestrictOpNone, errors.Errorf("unknown masking policy restrict option: %s", token)
        }
    }
    return ops, None
}

// isMaskingPolicyTableNotReady 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn isMaskingPolicyTableNotReady(err: errors::Error) -> bool {
    return ErrTableNotExists.Equal(err)
}

// resetMaskingPolicyCache 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn resetMaskingPolicyCache(is: &mut infoSchema) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.maskingPolicyMutex.Lock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.maskingPolicyMutex.Unlock()
    is.maskingPoliciesLoaded = false
    is.maskingPolicyTableColumnMap = make(map[int64]map[int64]*model.MaskingPolicyInfo)
    is.maskingPoliciesLoadCh = None
}

// AllPlacementPolicies returns all placement policies
// AllPlacementPolicies 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn AllPlacementPolicies(is: &mut infoSchemaMisc) -> Vec<&mut model::PolicyInfo> {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.policyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.policyMutex.RUnlock()
    let mut policies = make([]*model.PolicyInfo, 0, len(is.policyMap))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, policy := range is.policyMap {
        policies = append(policies, policy)
    }
    return policies
}

// ClonePlacementPolicies 对应 Go 同名函数或方法：保留 Go 的快照复制与会话隔离语义，避免共享可变状态泄漏。
pub fn ClonePlacementPolicies(is: &mut infoSchemaMisc) -> HashMap<String, &mut model::PolicyInfo> {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.policyMutex.RLock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.policyMutex.RUnlock()
    return maps.Clone(is.policyMap)
}

// PlacementBundleByPhysicalTableID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn PlacementBundleByPhysicalTableID(is: &mut infoSchemaMisc, id: i64) -> (&mut placement::Bundle, bool) {
    let mut (t, r) = is.ruleBundleMap[id]
    return t, r
}

// AllPlacementBundles 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn AllPlacementBundles(is: &mut infoSchemaMisc) -> Vec<&mut placement::Bundle> {
    let mut bundles = make([]*placement.Bundle, 0, len(is.ruleBundleMap))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, bundle := range is.ruleBundleMap {
        bundles = append(bundles, bundle)
    }
    return bundles
}

// setResourceGroup 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn setResourceGroup(is: &mut infoSchemaMisc, resourceGroup: &mut model::ResourceGroupInfo) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.Lock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.Unlock()
    is.resourceGroupMap[resourceGroup.Name.L] = resourceGroup
}

// deleteResourceGroup 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn deleteResourceGroup(is: &mut infoSchemaMisc, name: String) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.resourceGroupMutex.Lock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.resourceGroupMutex.Unlock()
    delete(is.resourceGroupMap, name)
}

// setPolicy 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn setPolicy(is: &mut infoSchemaMisc, policy: &mut model::PolicyInfo) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.policyMutex.Lock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.policyMutex.Unlock()
    is.policyMap[policy.Name.L] = policy
}

// deletePolicy 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn deletePolicy(is: &mut infoSchemaMisc, name: String) {
    // 该操作是共享缓存的并发边界；原子可见性或锁粒度不得在后续接线时弱化。
    is.policyMutex.Lock()
    // Go defer 负责资源归还或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer is.policyMutex.Unlock()
    delete(is.policyMap, name)
}

// addReferredForeignKeys 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn addReferredForeignKeys(is: &mut infoSchema, schema: ast::CIStr, tbInfo: &mut model::TableInfo) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, fk := range tbInfo.ForeignKeys {
        if fk.Version < model.FKVersion1 {
            continue
        }
        let mut refer = SchemaAndTableName{schema: fk.RefSchema.L, table: fk.RefTable.L}
        let mut referredFKList = is.referredForeignKeyMap[refer]
        let mut found = false
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, referredFK := range referredFKList {
            if referredFK.ChildSchema.L == schema.L && referredFK.ChildTable.L == tbInfo.Name.L && referredFK.ChildFKName.L == fk.Name.L {
                referredFK.Cols = fk.RefCols
                found = true
                break
            }
        }
        if found {
            continue
        }

        let mut newReferredFKList = make([]*model.ReferredFKInfo, 0, len(referredFKList)+1)
        newReferredFKList = append(newReferredFKList, referredFKList...)
        newReferredFKList = append(newReferredFKList, &model.ReferredFKInfo{
            Cols:        fk.RefCols,
            ChildSchema: schema,
            ChildTable:  tbInfo.Name,
            ChildFKName: fk.Name,
        })
        sort.Slice(newReferredFKList, func(i, j int) bool {
            if newReferredFKList[i].ChildSchema.L != newReferredFKList[j].ChildSchema.L {
                return newReferredFKList[i].ChildSchema.L < newReferredFKList[j].ChildSchema.L
            }
            if newReferredFKList[i].ChildTable.L != newReferredFKList[j].ChildTable.L {
                return newReferredFKList[i].ChildTable.L < newReferredFKList[j].ChildTable.L
            }
            return newReferredFKList[i].ChildFKName.L < newReferredFKList[j].ChildFKName.L
        })
        is.referredForeignKeyMap[refer] = newReferredFKList
    }
}

// deleteReferredForeignKeys 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn deleteReferredForeignKeys(is: &mut infoSchema, schema: ast::CIStr, tbInfo: &mut model::TableInfo) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, fk := range tbInfo.ForeignKeys {
        if fk.Version < model.FKVersion1 {
            continue
        }
        let mut refer = SchemaAndTableName{schema: fk.RefSchema.L, table: fk.RefTable.L}
        let mut referredFKList = is.referredForeignKeyMap[refer]
        if len(referredFKList) == 0 {
            continue
        }
        let mut newReferredFKList = make([]*model.ReferredFKInfo, 0, len(referredFKList)-1)
        // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
        for _, referredFK := range referredFKList {
            if referredFK.ChildSchema.L == schema.L && referredFK.ChildTable.L == tbInfo.Name.L && referredFK.ChildFKName.L == fk.Name.L {
                continue
            }
            newReferredFKList = append(newReferredFKList, referredFK)
        }
        is.referredForeignKeyMap[refer] = newReferredFKList
    }
}

// GetTableReferredForeignKeys gets the table's ReferredFKInfo by lowercase schema and table name.
// GetTableReferredForeignKeys 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn GetTableReferredForeignKeys(is: &mut infoSchema, schema: String, table: String) -> Vec<&mut model::ReferredFKInfo> {
    let mut name = SchemaAndTableName{schema: schema, table: table}
    return is.referredForeignKeyMap[name]
}

// GetAutoIDRequirement 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn GetAutoIDRequirement(is: &mut infoSchema) -> autoid::Requirement {
    return is.r
}

// SessionTables store local temporary tables
// SessionTables 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct SessionTables {
    // Session tables can be accessed after the db is dropped, so there needs a way to retain the DBInfo.
    // schemaTables.dbInfo will only be used when the db is dropped and it may be stale after the db is created again.
    // But it's fine because we only need its name.
    pub schemaMap: HashMap<String, &mut schemaTables>,
    pub idx2table: HashMap<i64, table::Table>,
}

// NewSessionTables creates a new NewSessionTables object
// NewSessionTables 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn NewSessionTables() -> &mut SessionTables {
    return &SessionTables{
        schemaMap: make(map[string]*schemaTables),
        idx2table: make(map[int64]table.Table),
    }
}

// TableByName get table by name
// TableByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByName(is: &mut SessionTables, ctx: stdctx::Context, schema: ast::CIStr, table: ast::CIStr) -> (table::Table, bool) {
    if tbNames, ok := is.schemaMap[schema.L]; ok {
        if t, ok := tbNames.tables[table.L]; ok {
            return t, true
        }
    }
    return None, false
}

// TableExists check if table with the name exists
// TableExists 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableExists(is: &mut SessionTables, schema: ast::CIStr, table: ast::CIStr) -> (bool) {
    _, ok = is.TableByName(stdctx.Background(), schema, table)
    return
}

// TableByID get table by table id
// TableByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByID(is: &mut SessionTables, id: i64) -> (table::Table, bool) {
    tbl, ok = is.idx2table[id]
    return
}

// AddTable add a table
// AddTable 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn AddTable(is: &mut SessionTables, db: &mut model::DBInfo, tbl: table::Table) -> errors::Error {
    let mut schemaTables = is.ensureSchema(db)
    let mut tblMeta = tbl.Meta()
    if _, ok := schemaTables.tables[tblMeta.Name.L]; ok {
        return ErrTableExists.GenWithStackByArgs(tblMeta.Name)
    }

    if _, ok := is.idx2table[tblMeta.ID]; ok {
        return ErrTableExists.GenWithStackByArgs(tblMeta.Name)
    }
    intest.Assert(db.ID == tbl.Meta().DBID)

    schemaTables.tables[tblMeta.Name.L] = tbl
    is.idx2table[tblMeta.ID] = tbl

    return None
}

// RemoveTable remove a table
// RemoveTable 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn RemoveTable(is: &mut SessionTables, schema: ast::CIStr, table: ast::CIStr) -> (bool) {
    let mut tbls = is.schemaTables(schema)
    if tbls == None {
        return false
    }

    let mut (oldTable, exist) = tbls.tables[table.L]
    if !exist {
        return false
    }

    delete(tbls.tables, table.L)
    delete(is.idx2table, oldTable.Meta().ID)
    if len(tbls.tables) == 0 {
        delete(is.schemaMap, schema.L)
    }
    return true
}

// Count gets the count of the temporary tables.
// Count 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn Count(is: &mut SessionTables) -> i32 {
    return len(is.idx2table)
}

// SchemaByID get a table's schema from the schema ID.
// SchemaByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByID(is: &mut SessionTables, id: i64) -> (&mut model::DBInfo, bool) {
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, v := range is.schemaMap {
        if v.dbInfo.ID == id {
            return v.dbInfo, true
        }
    }

    return None, false
}

// ensureSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn ensureSchema(is: &mut SessionTables, db: &mut model::DBInfo) -> &mut schemaTables {
    if tbls, ok := is.schemaMap[db.Name.L]; ok {
        return tbls
    }

    let mut tbls = &schemaTables{dbInfo: db, tables: make(map[string]table.Table)}
    is.schemaMap[db.Name.L] = tbls
    return tbls
}

// schemaTables 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn schemaTables(is: &mut SessionTables, schema: ast::CIStr) -> &mut schemaTables {
    if is.schemaMap == None {
        return None
    }

    if tbls, ok := is.schemaMap[schema.L]; ok {
        return tbls
    }

    return None
}

// SessionExtendedInfoSchema implements InfoSchema
// Local temporary table has a loose relationship with database.
// So when a database is dropped, its temporary tables still exist and can be returned by TableByName/TableByID.
// SessionExtendedInfoSchema 对应 Go 同名结构体；字段顺序保留版本、缓存和外部依赖关系。
pub struct SessionExtendedInfoSchema {
    pub InfoSchema: InfoSchema,
    pub LocalTemporaryTablesOnce: sync::Once,
    pub LocalTemporaryTables: &mut SessionTables,
    pub MdlTables: &mut SessionTables,
}

// TableByName implements InfoSchema.TableByName
// TableByName 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByName(ts: &mut SessionExtendedInfoSchema, ctx: stdctx::Context, schema: ast::CIStr, table: ast::CIStr) -> (table::Table, errors::Error) {
    if ts.LocalTemporaryTables != None {
        if tbl, ok := ts.LocalTemporaryTables.TableByName(ctx, schema, table); ok {
            return tbl, None
        }
    }

    if ts.MdlTables != None {
        if tbl, ok := ts.MdlTables.TableByName(ctx, schema, table); ok {
            return tbl, None
        }
    }

    return ts.InfoSchema.TableByName(ctx, schema, table)
}

// TableInfoByName implements InfoSchema.TableInfoByName
// TableInfoByName 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByName(ts: &mut SessionExtendedInfoSchema, schema: ast::CIStr, table: ast::CIStr) -> (&mut model::TableInfo, errors::Error) {
    let mut (tbl, err) = ts.TableByName(stdctx.Background(), schema, table)
    return getTableInfo(tbl), err
}

// TableInfoByID implements InfoSchema.TableInfoByID
// TableInfoByID 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn TableInfoByID(ts: &mut SessionExtendedInfoSchema, id: i64) -> (&mut model::TableInfo, bool) {
    let mut (tbl, ok) = ts.TableByID(stdctx.Background(), id)
    return getTableInfo(tbl), ok
}

// FindTableInfoByPartitionID implements InfoSchema.FindTableInfoByPartitionID
// FindTableInfoByPartitionID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableInfoByPartitionID(ts: &mut SessionExtendedInfoSchema, partitionID: i64) -> (&mut model::TableInfo, &mut model::DBInfo, &mut model::PartitionDefinition) {
    let mut (tbl, db, partDef) = ts.FindTableByPartitionID(partitionID)
    return getTableInfo(tbl), db, partDef
}

// TableByID implements InfoSchema.TableByID
// TableByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn TableByID(ts: &mut SessionExtendedInfoSchema, ctx: stdctx::Context, id: i64) -> (table::Table, bool) {
    if !tableIDIsValid(id) {
        return None, false
    }

    if ts.LocalTemporaryTables != None {
        if tbl, ok := ts.LocalTemporaryTables.TableByID(id); ok {
            return tbl, true
        }
    }

    if ts.MdlTables != None {
        if tbl, ok := ts.MdlTables.TableByID(id); ok {
            return tbl, true
        }
    }

    return ts.InfoSchema.TableByID(ctx, id)
}

// SchemaByID implements InfoSchema.SchemaByID, it returns a stale DBInfo even if it's dropped.
// SchemaByID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn SchemaByID(ts: &mut SessionExtendedInfoSchema, id: i64) -> (&mut model::DBInfo, bool) {
    if ts.LocalTemporaryTables != None {
        if db, ok := ts.LocalTemporaryTables.SchemaByID(id); ok {
            return db, true
        }
    }

    if ts.MdlTables != None {
        if tbl, ok := ts.MdlTables.SchemaByID(id); ok {
            return tbl, true
        }
    }

    let mut (ret, ok) = ts.InfoSchema.SchemaByID(id)
    return ret, ok
}

// UpdateTableInfo implements InfoSchema.SchemaByTable.
// UpdateTableInfo 对应 Go 同名函数或方法：保留内存索引更新顺序以及共享元数据的一致性边界。
pub fn UpdateTableInfo(ts: &mut SessionExtendedInfoSchema, db: &mut model::DBInfo, tableInfo: table::Table) -> errors::Error {
    if ts.MdlTables == None {
        ts.MdlTables = NewSessionTables()
    }
    let mut err = ts.MdlTables.AddTable(db, tableInfo)
    // 错误分支保持 Go 的短路顺序，防止失败后继续发布部分元数据。
    if err != None {
        return err
    }
    return None
}

// HasTemporaryTable returns whether information schema has temporary table
// HasTemporaryTable 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn HasTemporaryTable(ts: &mut SessionExtendedInfoSchema) -> bool {
    return ts.LocalTemporaryTables != None && ts.LocalTemporaryTables.Count() > 0 || ts.InfoSchema.HasTemporaryTable()
}

// DetachTemporaryTableInfoSchema returns a new SessionExtendedInfoSchema without temporary tables
// DetachTemporaryTableInfoSchema 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn DetachTemporaryTableInfoSchema(ts: &mut SessionExtendedInfoSchema) -> &mut SessionExtendedInfoSchema {
    return &SessionExtendedInfoSchema{
        InfoSchema: ts.InfoSchema,
        MdlTables:  ts.MdlTables,
    }
}

// FindTableByTblOrPartID looks for table.Table for the given id in the InfoSchema.
// The id can be either a table id or a partition id.
// If the id is a table id, the corresponding table.Table will be returned, and the second return value is None.
// If the id is a partition id, the corresponding table.Table and PartitionDefinition will be returned.
// If the id is not found in the InfoSchema, None will be returned for both return values.
// FindTableByTblOrPartID 对应 Go 同名函数或方法：保留名称/ID 查找、版本可见性判断和未命中返回语义。
pub fn FindTableByTblOrPartID(is: InfoSchema, id: i64) -> (table::Table, &mut model::PartitionDefinition) {
    let mut (tbl, ok) = is.TableByID(stdctx.Background(), id)
    if ok {
        return tbl, None
    }
    let mut (tbl, _, partDef) = is.FindTableByPartitionID(id)
    return tbl, partDef
}

// getTableInfo 对应 Go 同名函数或方法：保留参数、返回值、关键分支和外部调用形状。
pub fn getTableInfo(tbl: table::Table) -> &mut model::TableInfo {
    if tbl == None {
        return None
    }
    return tbl.Meta()
}

// getTableInfoList 对应 Go 同名函数或方法：保留遍历顺序、过滤短路和返回集合的组装方式。
pub fn getTableInfoList(tables: Vec<table::Table>) -> Vec<&mut model::TableInfo> {
    if tables == None {
        return None
    }

    let mut infoLost = make([]*model.TableInfo, 0, len(tables))
    // 遍历顺序沿用 Go；涉及 map 时调用方不应依赖稳定迭代次序。
    for _, tbl := range tables {
        infoLost = append(infoLost, tbl.Meta())
    }
    return infoLost
}
*/

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use crate::error::ErrTableNotExists;
use astersql_infoschema_context as context_dependency;
use astersql_meta_model as model_dependency;

/// 按表 ID 分桶存放排序表列表时的桶数量。
pub const bucketCount: usize = 512;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 大小写不敏感字符串：保留原文与小写形式，用于库表名比较。
pub struct CiString {
    pub original: String,
    pub lower: String,
}

impl CiString {
    /// 由任意可转 String 的值构造，同时缓存小写形式。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }
}

impl From<&str> for CiString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列的精简缓存索引：ID、名称、是否自增。
pub struct ColumnInfo {
    pub id: i64,
    pub name: CiString,
    pub auto_increment: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引的精简缓存索引：ID 与名称。
pub struct IndexInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区定义的精简表示：物理分区 ID 与名称。
pub struct PartitionDefinition {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的分区信息：分区定义列表。
pub struct PartitionInfo {
    pub definitions: Vec<PartitionDefinition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 外键：名称及引用的父库/父表。
pub struct ForeignKeyInfo {
    pub name: CiString,
    pub ref_schema: CiString,
    pub ref_table: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 被引用外键：记录子表侧的 schema/表/外键名（反向索引）。
pub struct ReferredFKInfo {
    pub child_schema: CiString,
    pub child_table: CiString,
    pub child_fk_name: CiString,
}

#[derive(Clone, Debug, Default)]
/// 表元数据的缓存索引视图；完整 Go 模型可选放在 `model_meta`。
pub struct TableInfo {
    pub id: i64,
    pub db_id: i64,
    pub name: CiString,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<IndexInfo>,
    pub partition: Option<PartitionInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub is_view: bool,
    pub is_sequence: bool,
    /// Complete Go table metadata retained for planner/executor consumers.
    ///
    /// The compact fields above are the cache index used by this migration,
    /// but they cannot represent column defaults, field types, FULLTEXT index
    // / state, or TiFlash replica metadata. Keeping the canonical model avoids
    /// lossy reconstruction at InfoSchema call boundaries.
    pub model_meta: Option<Arc<model_dependency::TableInfo>>,
}

impl PartialEq for TableInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.db_id == other.db_id
            && self.name == other.name
            && self.columns == other.columns
            && self.indices == other.indices
            && self.partition == other.partition
            && self.foreign_keys == other.foreign_keys
            && self.is_view == other.is_view
            && self.is_sequence == other.is_sequence
    }
}

impl Eq for TableInfo {}

#[derive(Clone, Debug)]
/// 共享所有权的表句柄（`Arc<TableInfo>`）。
pub struct Table(pub Arc<TableInfo>);

impl Table {
    /// 包装一张表的元数据。
    pub fn new(info: TableInfo) -> Self {
        Self(Arc::new(info))
    }
    /// 返回内部 `TableInfo` 引用。
    pub fn Meta(&self) -> &TableInfo {
        &self.0
    }

    /// 从完整 meta_model::TableInfo 投影出缓存索引字段，并保留完整模型。
    pub fn from_model(info: model_dependency::TableInfo) -> Self {
        let info = Arc::new(info);
        let auto_increment_id = info.GetAutoIncrementColInfo().map(|column| column.ID);
        let partition = info.Partition.as_ref().map(|partition| PartitionInfo {
            definitions: partition
                .Definitions
                .iter()
                .map(|definition| PartitionDefinition {
                    id: definition.ID,
                    name: CiString::new(definition.Name.O.clone()),
                })
                .collect(),
        });
        Self::new(TableInfo {
            id: info.ID,
            db_id: info.DBID,
            name: CiString::new(info.Name.O.clone()),
            columns: info
                .Columns
                .iter()
                .map(|column| ColumnInfo {
                    id: column.ID,
                    name: CiString::new(column.Name.O.clone()),
                    auto_increment: auto_increment_id == Some(column.ID),
                })
                .collect(),
            indices: info
                .Indices
                .iter()
                .map(|index| IndexInfo {
                    id: index.ID,
                    name: CiString::new(index.Name.O.clone()),
                })
                .collect(),
            partition,
            foreign_keys: info
                .ForeignKeys
                .iter()
                .map(|foreign_key| ForeignKeyInfo {
                    name: CiString::new(foreign_key.Name.O.clone()),
                    ref_schema: CiString::new(foreign_key.RefSchema.O.clone()),
                    ref_table: CiString::new(foreign_key.RefTable.O.clone()),
                })
                .collect(),
            is_view: info.View.is_some(),
            is_sequence: info.Sequence.is_some(),
            model_meta: Some(info),
        })
    }

    /// 取出完整表元数据；缺失时返回 `ErrTableMetadataUnavailable`。
    pub fn ModelMeta(&self) -> Result<Arc<model_dependency::TableInfo>, InfoSchemaError> {
        self.0.model_meta.clone().ok_or_else(|| InfoSchemaError {
            code: "ErrTableMetadataUnavailable",
            message: format!(
                "complete table metadata is unavailable for {}",
                self.0.name.original
            ),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 数据库（schema）元数据：ID、名称、表列表与名→ID 映射。
pub struct DBInfo {
    pub id: i64,
    pub name: CiString,
    pub tables: Vec<Arc<TableInfo>>,
    /// Original-case table name → id map used by schema-cache lazy loading.
    /// Loaded tables are removed (by `Name.O`) during `InitWithDBInfos`.
    pub table_name_2_id: HashMap<String, i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 放置策略（Placement Policy）精简信息。
pub struct PolicyInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 资源组精简信息。
pub struct ResourceGroupInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 物理表对应的 placement rule bundle。
pub struct PlacementBundle {
    pub physical_id: i64,
    pub rules: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略启用状态。
pub enum MaskingPolicyStatus {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略类型；值域与 Go `model.MaskingPolicyType*` 持久化契约一致。
pub enum MaskingPolicyType {
    #[default]
    Full,
    Partial,
    Null,
    Date,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略限制操作位图。
pub struct MaskingPolicyRestrictOps(pub u8);

impl MaskingPolicyRestrictOps {
    pub const NONE: Self = Self(0);
    pub const INSERT_INTO_SELECT: Self = Self(1);
    pub const UPDATE_SELECT: Self = Self(2);
    pub const DELETE_SELECT: Self = Self(4);
    pub const CTAS: Self = Self(8);
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单条脱敏策略：绑定到表列，含表达式与限制操作。
pub struct MaskingPolicyInfo {
    pub id: i64,
    pub name: CiString,
    pub table_id: i64,
    pub column_id: i64,
    pub status: MaskingPolicyStatus,
    pub policy_type: MaskingPolicyType,
    pub restrict_ops: MaskingPolicyRestrictOps,
    pub expression: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 小写 schema+table 名对，用作外键反向索引键。
pub struct SchemaAndTableName {
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的轻量定位信息：所属库名与表名。
pub struct TableItem {
    pub DBName: CiString,
    pub TableName: CiString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// InfoSchema 查询错误：错误码名 + 消息。
pub struct InfoSchemaError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for InfoSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for InfoSchemaError {}

impl InfoSchemaError {
    /// 转为共享错误类型，供跨 crate 传播。
    pub fn into_shared(self) -> astersql_util_dbterror::errors::SharedError {
        astersql_util_dbterror::errors::New(self.to_string())
    }
}

/// InfoSchema 对外查询接口：按名/ID 查库表、分区、特殊属性列表等。
pub trait InfoSchema: Send + Sync {
    fn SchemaMetaVersion(&self) -> i64;
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>>;
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>>;
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError>;
    fn ModelTableInfoByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Arc<model_dependency::TableInfo>, InfoSchemaError> {
        self.TableByName(schema, table)?.ModelMeta()
    }
    fn TableByID(&self, id: i64) -> Option<Table>;
    fn TableItemByID(&self, id: i64) -> Option<TableItem>;
    /// Return all tables in a schema; a missing schema yields an empty list.
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError>;
    /// Whether the snapshot contains a global temporary table.
    fn HasTemporaryTable(&self) -> bool {
        false
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)>;
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>>;
    fn ListTablesWithSpecialAttribute(
        &self,
        filter: context_dependency::SpecialAttributeFilter,
    ) -> Vec<context_dependency::TableInfoResult> {
        self.AllSchemas()
            .into_iter()
            .map(|schema| {
                let table_infos = schema
                    .tables
                    .iter()
                    .filter_map(|table| table.model_meta.clone())
                    .filter(|table| filter(table))
                    .collect();
                context_dependency::TableInfoResult {
                    DBName: astersql_parser_ast::NewCIStr(&schema.name.original),
                    TableInfos: table_infos,
                }
            })
            .collect()
    }
    fn IsV2(&self) -> bool {
        false
    }
    /// Compact table-history records older than `cut_version`.
    ///
    /// InfoSchema V1 has no shared version history, so the default returns
    /// `None`. InfoSchema V2 overrides this with its production data GC.
    fn GCOldVersion(&self, _cut_version: i64) -> Option<(usize, i64)> {
        None
    }
}

#[derive(Clone)]
/// 单个 schema 下的库信息与按小写表名索引的表映射。
struct schemaTables {
    db_info: Arc<DBInfo>,
    tables: HashMap<String, Table>,
}

/// InfoSchema V1 具体实现：分桶表索引、策略/资源组/脱敏缓存与临时表 ID 集合。
pub struct infoSchema {
    schema_meta_version: i64,
    schema_map: HashMap<String, schemaTables>,
    schema_id_to_name: HashMap<i64, String>,
    sorted_table_buckets: Vec<Vec<Table>>,
    referred_foreign_keys: HashMap<SchemaAndTableName, Vec<ReferredFKInfo>>,
    policies: RwLock<HashMap<String, Arc<PolicyInfo>>>,
    resource_groups: RwLock<HashMap<String, Arc<ResourceGroupInfo>>>,
    bundles: HashMap<i64, Arc<PlacementBundle>>,
    temporary_table_ids: HashSet<i64>,
    masking: RwLock<HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>>,
    masking_loaded: Mutex<bool>,
    masking_loader: Option<Arc<dyn MaskingPolicyLoader>>,
    snapshot_ts: u64,
}

impl infoSchema {
    /// 创建指定 schema 元版本的空 InfoSchema。
    pub fn new(schema_meta_version: i64) -> Self {
        Self {
            schema_meta_version,
            schema_map: HashMap::new(),
            schema_id_to_name: HashMap::new(),
            sorted_table_buckets: (0..bucketCount).map(|_| Vec::new()).collect(),
            referred_foreign_keys: HashMap::new(),
            policies: RwLock::new(HashMap::new()),
            resource_groups: RwLock::new(HashMap::new()),
            bundles: HashMap::new(),
            temporary_table_ids: HashSet::new(),
            masking: RwLock::new(HashMap::new()),
            masking_loaded: Mutex::new(false),
            masking_loader: None,
            snapshot_ts: 0,
        }
    }

    /// 挂载脱敏策略加载器与快照时间戳，供惰性加载使用。
    pub fn with_masking_loader(
        mut self,
        loader: Arc<dyn MaskingPolicyLoader>,
        snapshot_ts: u64,
    ) -> Self {
        self.masking_loader = Some(loader);
        self.snapshot_ts = snapshot_ts;
        self
    }

    /// 注册一个 schema 及其表：写入分桶、外键反向索引与名映射。
    pub fn add_schema(&mut self, mut db: DBInfo, tables: Vec<Table>) {
        let mut by_name = HashMap::new();
        db.tables = tables.iter().map(|table| table.0.clone()).collect();
        for table in &tables {
            by_name.insert(table.Meta().name.lower.clone(), table.clone());
            let bucket = tableBucketIdx(table.Meta().id);
            self.sorted_table_buckets[bucket].push(table.clone());
            self.addReferredForeignKeys(&db.name, table.Meta());
        }
        for bucket in &mut self.sorted_table_buckets {
            bucket.sort_by_key(|table| table.Meta().id);
        }
        let db = Arc::new(db);
        self.schema_id_to_name.insert(db.id, db.name.lower.clone());
        self.schema_map.insert(
            db.name.lower.clone(),
            schemaTables {
                db_info: db,
                tables: by_name,
            },
        );
    }

    /// 删除 schema；成功返回 true。
    pub fn del_schema(&mut self, schema: &CiString) -> bool {
        let Some(old) = self.schema_map.remove(&schema.lower) else {
            return false;
        };
        self.schema_id_to_name.remove(&old.db_info.id);
        let ids: HashSet<i64> = old.tables.values().map(|table| table.Meta().id).collect();
        for bucket in &mut self.sorted_table_buckets {
            bucket.retain(|table| !ids.contains(&table.Meta().id));
        }
        true
    }

    /// 判断 schema 是否存在。
    pub fn SchemaExists(&self, schema: &CiString) -> bool {
        self.schema_map.contains_key(&schema.lower)
    }

    /// 判断库表是否存在。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.schema_map
            .get(&schema.lower)
            .is_some_and(|db| db.tables.contains_key(&table.lower))
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

    /// 按分区 ID 找回所属表/库/分区定义。
    pub fn FindTableInfoByPartitionID(
        &self,
        id: i64,
    ) -> Option<(Arc<TableInfo>, Arc<DBInfo>, PartitionDefinition)> {
        self.FindTableByPartitionID(id)
            .map(|(table, db, partition)| (table.0, db, partition))
    }

    /// 列出指定 schema 下全部表的 `TableInfo`。
    pub fn SchemaTableInfos(
        &self,
        schema: &CiString,
    ) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        Ok(self
            .schema_map
            .get(&schema.lower)
            .map(|tables| tables.db_info.tables.clone())
            .unwrap_or_default())
    }

    /// 返回全部 schema 名。
    pub fn AllSchemaNames(&self) -> Vec<CiString> {
        self.AllSchemas()
            .into_iter()
            .map(|db| db.name.clone())
            .collect()
    }
    /// 是否登记了全局临时表。
    pub fn HasTemporaryTable(&self) -> bool {
        !self.temporary_table_ids.is_empty()
    }

    pub(crate) fn set_temporary_table_ids(&mut self, ids: HashSet<i64>) {
        self.temporary_table_ids = ids;
    }

    /// 按名查找放置策略。
    pub fn PolicyByName(&self, name: &CiString) -> Option<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .get(&name.lower)
            .cloned()
    }
    /// 按 ID 查找放置策略。
    pub fn PolicyByID(&self, id: i64) -> Option<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .values()
            .find(|policy| policy.id == id)
            .cloned()
    }
    /// 列出全部放置策略。
    pub fn AllPlacementPolicies(&self) -> Vec<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .values()
            .cloned()
            .collect()
    }
    /// 写入/覆盖一条放置策略。
    pub fn setPolicy(&self, policy: PolicyInfo) {
        self.policies
            .write()
            .expect("policy lock poisoned")
            .insert(policy.name.lower.clone(), Arc::new(policy));
    }
    /// 按名删除放置策略。
    pub fn deletePolicy(&self, name: &str) {
        self.policies
            .write()
            .expect("policy lock poisoned")
            .remove(&name.to_lowercase());
    }

    /// 按名查找资源组。
    pub fn ResourceGroupByName(&self, name: &CiString) -> Option<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .get(&name.lower)
            .cloned()
    }
    /// 按 ID 查找资源组。
    pub fn ResourceGroupByID(&self, id: i64) -> Option<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .values()
            .find(|group| group.id == id)
            .cloned()
    }
    /// 列出全部资源组。
    pub fn AllResourceGroups(&self) -> Vec<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .values()
            .cloned()
            .collect()
    }
    /// 写入/覆盖一个资源组。
    pub fn setResourceGroup(&self, group: ResourceGroupInfo) {
        self.resource_groups
            .write()
            .expect("resource group lock poisoned")
            .insert(group.name.lower.clone(), Arc::new(group));
    }
    /// 按名删除资源组。
    pub fn deleteResourceGroup(&self, name: &str) {
        self.resource_groups
            .write()
            .expect("resource group lock poisoned")
            .remove(&name.to_lowercase());
    }

    /// 按物理表 ID 取 placement bundle。
    pub fn PlacementBundleByPhysicalTableID(&self, id: i64) -> Option<Arc<PlacementBundle>> {
        self.bundles.get(&id).cloned()
    }
    /// 列出全部 placement bundle。
    pub fn AllPlacementBundles(&self) -> Vec<Arc<PlacementBundle>> {
        self.bundles.values().cloned().collect()
    }

    /// 按表 ID + 列 ID 取脱敏策略（必要时触发惰性加载）。
    pub fn MaskingPolicyByTableColumn(
        &self,
        table_id: i64,
        column_id: i64,
    ) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        self.masking
            .read()
            .expect("masking lock poisoned")
            .get(&table_id)?
            .get(&column_id)
            .cloned()
    }
    /// 按策略 ID 查找脱敏策略。
    pub fn MaskingPolicyByID(&self, id: i64) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        self.masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values())
            .find(|policy| policy.id == id)
            .cloned()
    }
    /// 按名查找脱敏策略；重名（歧义）时返回 None。
    pub fn MaskingPolicyByName(&self, name: &CiString) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        let mut found: Option<Arc<MaskingPolicyInfo>> = None;
        for policy in self
            .masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values())
        {
            if policy.name.lower == name.lower {
                if found.is_some() {
                    // Ambiguous name: Go returns (nil, false).
                    return None;
                }
                found = Some(policy.clone());
            }
        }
        found
    }

    /// Whether masking policies have been marked loaded (exported for same-crate tests).
    pub fn masking_policies_loaded(&self) -> bool {
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned")
    }

    /// Inject a masking policy without going through the loader (test / builder copy path).
    pub fn put_masking_policy(&self, policy: MaskingPolicyInfo) {
        let table_id = policy.table_id;
        let column_id = policy.column_id;
        self.masking
            .write()
            .expect("masking lock poisoned")
            .entry(table_id)
            .or_default()
            .insert(column_id, Arc::new(policy));
    }

    /// 测试/迁移辅助：直接设置脱敏策略已加载标志。
    pub fn set_masking_policies_loaded(&self, loaded: bool) {
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned") = loaded;
    }

    /// 克隆表→列→策略的脱敏缓存。
    pub fn clone_masking_policies(&self) -> HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>> {
        self.masking.read().expect("masking lock poisoned").clone()
    }

    /// 用给定映射整体替换脱敏缓存。
    pub fn restore_masking_policies(
        &self,
        policies: HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>,
        loaded: bool,
    ) {
        *self.masking.write().expect("masking lock poisoned") = policies;
        self.set_masking_policies_loaded(loaded);
    }
    /// 列出全部已缓存脱敏策略。
    pub fn AllMaskingPolicies(&self) -> Vec<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        let mut policies: Vec<_> = self
            .masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values().cloned())
            .collect();
        policies.sort_by(|left, right| {
            left.name
                .lower
                .cmp(&right.name.lower)
                .then_with(|| left.id.cmp(&right.id))
        });
        policies
    }
    /// 清空脱敏缓存并重置 loaded 标志。
    pub fn resetMaskingPolicyCache(&self) {
        self.masking.write().expect("masking lock poisoned").clear();
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned") = false;
    }
    /// 若尚未加载则调用 loader；表未就绪则置 loaded 且不再重试。
    fn loadMaskingPoliciesIfNeeded(&self) {
        let mut loaded = self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned");
        if *loaded {
            return;
        }
        let Some(loader) = &self.masking_loader else {
            // Go: factory == nil → mark loaded and skip.
            *loaded = true;
            return;
        };
        match loader.load(&[], self.snapshot_ts) {
            Ok(policies) => {
                let mut target = self.masking.write().expect("masking lock poisoned");
                for policy in policies {
                    target
                        .entry(policy.table_id)
                        .or_default()
                        .insert(policy.column_id, Arc::new(policy));
                }
                *loaded = true;
            }
            Err(err)
                if err.code == ErrTableNotExists.mysql_name || err.code == "ErrNoSuchTable" =>
            {
                // Table not ready: treat as loaded so we do not retry forever.
                // 对应 Go/生产辅助函数 isMaskingPolicyTableNotReady。
                *loaded = true;
            }
            Err(_) => {
                // Generic error: leave unloaded so the next access retries.
            }
        }
    }

    /// 将表的外键登记到父表的反向引用索引。
    fn addReferredForeignKeys(&mut self, schema: &CiString, table: &TableInfo) {
        for foreign_key in &table.foreign_keys {
            self.referred_foreign_keys
                .entry(SchemaAndTableName {
                    schema: foreign_key.ref_schema.lower.clone(),
                    table: foreign_key.ref_table.lower.clone(),
                })
                .or_default()
                .push(ReferredFKInfo {
                    child_schema: schema.clone(),
                    child_table: table.name.clone(),
                    child_fk_name: foreign_key.name.clone(),
                });
        }
    }
    /// 查询引用指定父表的全部子表外键。
    pub fn GetTableReferredForeignKeys(&self, schema: &str, table: &str) -> Vec<ReferredFKInfo> {
        self.referred_foreign_keys
            .get(&SchemaAndTableName {
                schema: schema.to_lowercase(),
                table: table.to_lowercase(),
            })
            .cloned()
            .unwrap_or_default()
    }
}

impl InfoSchema for infoSchema {
    fn SchemaMetaVersion(&self) -> i64 {
        self.schema_meta_version
    }
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>> {
        self.schema_map
            .get(&schema.lower)
            .map(|tables| tables.db_info.clone())
    }
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.schema_id_to_name
            .get(&id)
            .and_then(|name| self.schema_map.get(name))
            .map(|tables| tables.db_info.clone())
    }
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError> {
        self.schema_map
            .get(&schema.lower)
            .and_then(|db| db.tables.get(&table.lower))
            .cloned()
            .ok_or_else(|| InfoSchemaError {
                code: ErrTableNotExists.mysql_name,
                message: format!("{}.{}", schema.original, table.original),
            })
    }
    fn TableByID(&self, id: i64) -> Option<Table> {
        if id <= 0 {
            return None;
        }
        let bucket = &self.sorted_table_buckets[tableBucketIdx(id)];
        bucket
            .binary_search_by_key(&id, |table| table.Meta().id)
            .ok()
            .map(|index| bucket[index].clone())
    }
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        self.SchemaTableInfos(schema)
    }
    fn HasTemporaryTable(&self) -> bool {
        self.HasTemporaryTable()
    }
    fn TableItemByID(&self, id: i64) -> Option<TableItem> {
        let table = self.TableByID(id)?;
        let db = self.SchemaByID(table.Meta().db_id)?;
        Some(TableItem {
            DBName: db.name.clone(),
            TableName: table.Meta().name.clone(),
        })
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)> {
        for db in self.schema_map.values() {
            for table in db.tables.values() {
                if let Some(partition) = &table.Meta().partition {
                    if let Some(definition) = partition
                        .definitions
                        .iter()
                        .find(|definition| definition.id == partition_id)
                    {
                        return Some((table.clone(), db.db_info.clone(), definition.clone()));
                    }
                }
            }
        }
        None
    }
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>> {
        self.schema_map
            .values()
            .map(|tables| tables.db_info.clone())
            .collect()
    }
}

/// 由表 ID 计算分桶下标。
pub fn tableBucketIdx(id: i64) -> usize {
    id.unsigned_abs() as usize % bucketCount
}

/// 用给定表列表构造测试用 InfoSchema（默认库名 test，版本 0）。
pub fn MockInfoSchema(mut table_infos: Vec<TableInfo>) -> Arc<infoSchema> {
    MockInfoSchemaWithSchemaVer(std::mem::take(&mut table_infos), 0)
}

/// 同 `MockInfoSchema`，可指定 schema 元版本。
pub fn MockInfoSchemaWithSchemaVer(
    mut table_infos: Vec<TableInfo>,
    schema_version: i64,
) -> Arc<infoSchema> {
    let mut schema = infoSchema::new(schema_version);
    for table in &mut table_infos {
        table.db_id = 1;
    }
    let mut tables: Vec<Table> = table_infos.into_iter().map(Table::new).collect();
    let system = Table::new(TableInfo {
        id: 9999,
        db_id: 2,
        name: CiString::new("stats_meta"),
        columns: vec![ColumnInfo {
            id: 1,
            name: CiString::new("a"),
            auto_increment: false,
        }],
        ..TableInfo::default()
    });
    schema.add_schema(
        DBInfo {
            id: 1,
            name: CiString::new("test"),
            tables: Vec::new(),
            table_name_2_id: Default::default(),
        },
        std::mem::take(&mut tables),
    );
    schema.add_schema(
        DBInfo {
            id: 2,
            name: CiString::new("mysql"),
            tables: Vec::new(),
            table_name_2_id: Default::default(),
        },
        vec![system],
    );
    Arc::new(schema)
}

/// 判断指定库表是否为视图。
pub fn TableIsView(schema: &dyn InfoSchema, db: &CiString, table: &CiString) -> bool {
    schema
        .TableByName(db, table)
        .is_ok_and(|table| table.Meta().is_view)
}
/// 判断指定库表是否为序列。
pub fn TableIsSequence(schema: &dyn InfoSchema, db: &CiString, table: &CiString) -> bool {
    schema
        .TableByName(db, table)
        .is_ok_and(|table| table.Meta().is_sequence)
}
/// 根据表元数据找回所属 schema。
pub fn SchemaByTable(schema: &dyn InfoSchema, table: &TableInfo) -> Option<Arc<DBInfo>> {
    if table.db_id > 0 {
        return schema.SchemaByID(table.db_id);
    }
    let table = schema.TableByID(table.id)?;
    schema.SchemaByID(table.Meta().db_id)
}
/// 返回全部 schema 的原文名字符串列表。
pub fn AllSchemaNames(schema: &dyn InfoSchema) -> Vec<String> {
    schema
        .AllSchemas()
        .into_iter()
        .map(|db| db.name.original.clone())
        .collect()
}
/// 若表有自增列则返回其名称。
pub fn HasAutoIncrementColumn(table: &TableInfo) -> Option<String> {
    table
        .columns
        .iter()
        .find(|column| column.auto_increment)
        .map(|column| column.name.original.clone())
}

/// 先按表 ID 再按分区 ID 查找表（及可选分区定义）。
pub fn FindTableByTblOrPartID(
    schema: &dyn InfoSchema,
    id: i64,
) -> (Option<Table>, Option<PartitionDefinition>) {
    if let Some(table) = schema.TableByID(id) {
        return (Some(table), None);
    }
    match schema.FindTableByPartitionID(id) {
        Some((table, _, partition)) => (Some(table), Some(partition)),
        None => (None, None),
    }
}

/// 脱敏策略加载器：按表 ID 列表与快照时间戳从存储拉取策略。
pub trait MaskingPolicyLoader: Send + Sync {
    fn load(
        &self,
        table_ids: &[i64],
        snapshot_ts: u64,
    ) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError>;
}

/// 加载全部脱敏策略（无表 ID 过滤）。
pub fn LoadMaskingPolicies(
    loader: &dyn MaskingPolicyLoader,
    snapshot_ts: u64,
) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
    let mut policies = loader.load(&[], snapshot_ts)?;
    policies.sort_by_key(|policy| (policy.table_id, policy.column_id, policy.id));
    Ok(policies)
}

/// 按表 ID 过滤加载脱敏策略。
pub fn loadMaskingPoliciesWithTableIDs(
    loader: &dyn MaskingPolicyLoader,
    table_ids: &[i64],
    snapshot_ts: u64,
) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
    let (normalized, has_filter) = normalizeMaskingPolicyTableIDs(table_ids);
    if !has_filter {
        // No filter → load all policies.
        return LoadMaskingPolicies(loader, snapshot_ts);
    }
    // Has filter with empty positive ids → match nothing.
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    const MAX_BATCH_SIZE: usize = 1024;
    let mut policies = Vec::new();
    for batch in normalized.chunks(MAX_BATCH_SIZE) {
        policies.extend(loader.load(batch, snapshot_ts)?);
    }
    policies.sort_by_key(|policy| (policy.table_id, policy.column_id, policy.id));
    Ok(policies)
}

/// Normalize table-id filters for masking-policy loads.
///
/// Mirrors Go `normalizeMaskingPolicyTableIDs`:
/// - empty / nil input → `(empty, false)` meaning "no filter" (load all);
/// - non-empty input → `(sorted unique positive ids, true)` even when the
// / filtered id list ends up empty (caller must treat that as "match nothing").
/// 规范化表 ID：去非正数、去重排序；返回 (ids, 是否有过滤条件)。
pub fn normalizeMaskingPolicyTableIDs(table_ids: &[i64]) -> (Vec<i64>, bool) {
    if table_ids.is_empty() {
        return (Vec::new(), false);
    }
    let mut ids: Vec<i64> = table_ids.iter().copied().filter(|id| *id > 0).collect();
    ids.sort_unstable();
    ids.dedup();
    (ids, true)
}

/// 判断错误是否表示脱敏系统表尚不存在/未就绪。
pub fn isMaskingPolicyTableNotReady(err: &InfoSchemaError) -> bool {
    err.code == ErrTableNotExists.mysql_name || err.code == "ErrNoSuchTable"
}

/// 构造加载脱敏策略的 SQL 形状与绑定参数（迁移期仅保留形状）。
pub fn buildLoadMaskingPoliciesQuery(table_ids: &[i64]) -> (String, Vec<i64>) {
    let base = "SELECT policy_id, policy_name, db_name, table_name, table_id, column_name, column_id, expression, status, masking_type, restrict_on, created_at, updated_at, created_by\nFROM mysql.tidb_masking_policy";
    if table_ids.is_empty() {
        return (
            format!("{base} ORDER BY table_id, column_id, policy_id"),
            Vec::new(),
        );
    }
    let placeholders = std::iter::repeat_n("%?", table_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    (
        format!(
            "{base} WHERE table_id IN ({placeholders}) ORDER BY table_id, column_id, policy_id"
        ),
        table_ids.to_vec(),
    )
}

/// 解析脱敏状态字符串。
pub fn maskingPolicyStatusFromString(status: &str) -> Result<MaskingPolicyStatus, InfoSchemaError> {
    match status.trim().to_ascii_lowercase().as_str() {
        "enabled" | "enable" => Ok(MaskingPolicyStatus::Enabled),
        "disabled" | "disable" => Ok(MaskingPolicyStatus::Disabled),
        _ => Err(InfoSchemaError {
            code: "ErrInvalidMaskingPolicyStatus",
            message: status.to_owned(),
        }),
    }
}
/// 解析脱敏类型字符串。
pub fn maskingPolicyTypeFromString(
    policy_type: &str,
) -> Result<MaskingPolicyType, InfoSchemaError> {
    match policy_type.trim().to_ascii_uppercase().as_str() {
        "MASK_FULL" => Ok(MaskingPolicyType::Full),
        "MASK_PARTIAL" => Ok(MaskingPolicyType::Partial),
        "MASK_NULL" => Ok(MaskingPolicyType::Null),
        "MASK_DATE" => Ok(MaskingPolicyType::Date),
        "CUSTOM" => Ok(MaskingPolicyType::Custom),
        _ => Err(InfoSchemaError {
            code: "ErrInvalidMaskingPolicyType",
            message: policy_type.to_owned(),
        }),
    }
}
/// 解析限制操作字符串为位图。
pub fn maskingPolicyRestrictOpsFromString(
    value: &str,
) -> Result<MaskingPolicyRestrictOps, InfoSchemaError> {
    let value = value.trim().to_ascii_uppercase();
    if value.is_empty() || value == "NONE" {
        return Ok(MaskingPolicyRestrictOps::NONE);
    }
    let mut result = MaskingPolicyRestrictOps::NONE;
    for operation in value.split(',').map(str::trim).filter(|op| !op.is_empty()) {
        result.0 |= match operation {
            "INSERT_INTO_SELECT" => MaskingPolicyRestrictOps::INSERT_INTO_SELECT.0,
            "UPDATE_SELECT" => MaskingPolicyRestrictOps::UPDATE_SELECT.0,
            "DELETE_SELECT" => MaskingPolicyRestrictOps::DELETE_SELECT.0,
            "CTAS" => MaskingPolicyRestrictOps::CTAS.0,
            "NONE" => 0,
            _ => {
                return Err(InfoSchemaError {
                    code: "ErrInvalidMaskingPolicyRestrictOps",
                    message: operation.to_owned(),
                });
            }
        };
    }
    Ok(result)
}

#[derive(Default)]
/// 会话级本地临时表容器。
pub struct SessionTables {
    schemas: HashMap<String, schemaTables>,
    table_ids: HashMap<i64, Table>,
}

impl SessionTables {
    /// 创建空的会话临时表集合。
    pub fn new() -> Self {
        Self::default()
    }
    /// 按库表名查找会话临时表。
    pub fn TableByName(&self, schema: &CiString, table: &CiString) -> Option<Table> {
        self.schemas
            .get(&schema.lower)?
            .tables
            .get(&table.lower)
            .cloned()
    }
    /// 判断库表是否存在。
    /// 会话临时表是否存在。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.TableByName(schema, table).is_some()
    }
    /// 按表 ID 查找会话临时表。
    pub fn TableByID(&self, id: i64) -> Option<Table> {
        self.table_ids.get(&id).cloned()
    }
    /// 向会话临时表注册一张表。
    pub fn AddTable(&mut self, db: DBInfo, table: Table) -> Result<(), InfoSchemaError> {
        if self.table_ids.contains_key(&table.Meta().id)
            || self.TableExists(&db.name, &table.Meta().name)
        {
            return Err(InfoSchemaError {
                code: "ErrTableExists",
                message: table.Meta().name.original.clone(),
            });
        }
        assert_eq!(db.id, table.Meta().db_id);
        self.table_ids.insert(table.Meta().id, table.clone());
        self.schemas
            .entry(db.name.lower.clone())
            .or_insert_with(|| schemaTables {
                db_info: Arc::new(db),
                tables: HashMap::new(),
            })
            .tables
            .insert(table.Meta().name.lower.clone(), table);
        Ok(())
    }
    /// 移除会话临时表。
    pub fn RemoveTable(&mut self, schema: &CiString, table: &CiString) -> bool {
        let Some(removed) = self
            .schemas
            .get_mut(&schema.lower)
            .and_then(|db| db.tables.remove(&table.lower))
        else {
            return false;
        };
        self.table_ids.remove(&removed.Meta().id);
        if self
            .schemas
            .get(&schema.lower)
            .is_some_and(|db| db.tables.is_empty())
        {
            self.schemas.remove(&schema.lower);
        }
        true
    }
    /// 会话临时表数量。
    pub fn Count(&self) -> usize {
        self.table_ids.len()
    }
    pub fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.schemas
            .values()
            .find(|schema| schema.db_info.id == id)
            .map(|schema| schema.db_info.clone())
    }
}

/// 构造空的 `SessionTables`。
pub fn NewSessionTables() -> SessionTables {
    SessionTables::new()
}

/// 扩展 InfoSchema：在基础视图上叠加会话临时表查找。
pub struct SessionExtendedInfoSchema {
    pub base: Arc<dyn InfoSchema>,
    pub temporary: SessionTables,
}

impl SessionExtendedInfoSchema {
    /// 先查会话临时表，再回落到底层 InfoSchema。
    pub fn TableByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Table, InfoSchemaError> {
        self.temporary
            .TableByName(schema, table)
            .map(Ok)
            .unwrap_or_else(|| self.base.TableByName(schema, table))
    }
    /// 先按 ID 查会话临时表，再回落底层。
    pub fn TableByID(&self, id: i64) -> Option<Table> {
        self.temporary
            .TableByID(id)
            .or_else(|| self.base.TableByID(id))
    }
    /// 先查会话临时 schema，再回落底层。
    pub fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.temporary
            .SchemaByID(id)
            .or_else(|| self.base.SchemaByID(id))
    }
    /// 是否登记了全局临时表。
    pub fn HasTemporaryTable(&self) -> bool {
        self.temporary.Count() != 0
    }
    /// 剥离临时表层，仅返回底层 InfoSchema。
    pub fn DetachTemporaryTableInfoSchema(&self) -> Arc<dyn InfoSchema> {
        self.base.clone()
    }
}
