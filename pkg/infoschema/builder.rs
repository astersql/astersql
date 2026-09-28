// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// InfoSchema Builder：根据 DDL SchemaDiff 增量或全量构建元数据快照。
//
// SchemaDiff 描述一次 DDL 对元数据的变更；Builder 将其应用到内存中的库表、
// Placement Policy 与 Resource Group，最后 `Build` 出 `InfoSchema`（v1 或 v2）。
// 上方机械草稿保留与 Go 完整路径对照的注释与依赖形状。

// 当前 Rust 2024 实现不会读取 KV、开启事务、连接数据库或真正修改在线元数据；
// meta、model、autoid、table、metrics 等名字保留 Go 外部依赖形状，供后续模块化迁移对照。

/* Mechanical draft retained for migration history.
use std::collections::{HashMap, HashSet};

// Builder 对应 Go 聚合构建器；dirtyDB 记录写时复制过的库，避免一个构建周期重复复制。
/// InfoSchema 构建器：累积库表与策略，支持 v1 内存结构或 v2 Data。
pub struct Builder {
    pub enableV2: bool,
    pub infoschemaV2: infoschemaV2,
    pub dirtyDB: HashMap<String, bool>,
    pub Requirement: autoid::Requirement,
    pub factory: ResourceFactory,
    pub bundleInfoBuilder: bundleInfoBuilder,
    pub infoData: Data,
    pub store: Option<kv::Storage>,
    pub crossKS: bool,
}

pub type ResourceFactory = fn() -> Result<pools::Resource, Error>;

impl Builder {
    // SetSchemaVersion 对应 Go setter，后续 diff 分派均以此版本写入缓存。
    pub fn SetSchemaVersion(&mut self, ver: i64) {
        self.infoschemaV2.infoSchema.schemaMetaVersion = ver;
    }

    // ApplyDiff 对应 Go 总分派器，按 DDL ActionType 选择专门更新路径。
    pub fn ApplyDiff(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
        self.SetSchemaVersion(diff.Version);
        match diff.Type {
            model::ActionCreateSchema => { self.applyCreateSchema(m, diff)?; Ok(vec![]) }
            model::ActionDropSchema => Ok(self.applyDropSchema(diff)),
            model::ActionRecoverSchema => self.applyRecoverSchema(m, diff),
            model::ActionModifySchemaCharsetAndCollate => { self.applyModifySchemaCharsetAndCollate(m, diff)?; Ok(vec![]) }
            model::ActionModifySchemaDefaultPlacement => { self.applyModifySchemaDefaultPlacement(m, diff)?; Ok(vec![]) }
            model::ActionCreatePlacementPolicy => { applyCreatePolicy(self, m, diff)?; Ok(vec![]) }
            model::ActionDropPlacementPolicy => Ok(applyDropPolicy(self, diff.SchemaID)),
            model::ActionAlterPlacementPolicy => applyAlterPolicy(self, m, diff),
            model::ActionCreateResourceGroup | model::ActionAlterResourceGroup => {
                applyCreateOrAlterResourceGroup(self, m, diff)?; Ok(vec![])
            }
            model::ActionDropResourceGroup => Ok(applyDropResourceGroup(self, m, diff)),
            model::ActionCreateMaskingPolicy | model::ActionAlterMaskingPolicy | model::ActionDropMaskingPolicy => applyMaskingPolicyChange(self),
            model::ActionTruncateTablePartition | model::ActionTruncateTable => applyTruncateTableOrPartition(self, m, diff),
            model::ActionDropTable | model::ActionDropTablePartition => applyDropTableOrPartition(self, m, diff),
            model::ActionRecoverTable => applyRecoverTable(self, m, diff),
            model::ActionCreateTables => self.applyCreateTables(m, diff),
            model::ActionReorganizePartition | model::ActionRemovePartitioning | model::ActionAlterTablePartitioning => applyReorganizePartition(self, m, diff),
            model::ActionExchangeTablePartition => applyExchangeTablePartition(self, m, diff),
            model::ActionFlashbackCluster => Ok(vec![-1]),
            model::ActionRefreshMeta => applyRefreshMeta(self, m, diff),
            _ => applyDefaultAction(self, m, diff),
        }
    }

    fn applyCreateTables(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
        self.applyAffectedOpts(m, Vec::with_capacity(diff.AffectedOpts.len()), diff, model::ActionCreateTable)
    }

    // applyAffectedOpts 为每个 AffectedOption 合成独立 diff，并递归走统一分派器。
    fn applyAffectedOpts(&mut self, m: &dyn meta::Reader, mut ids: Vec<i64>, diff: &model::SchemaDiff, tp: model::ActionType) -> Result<Vec<i64>, Error> {
        for opt in &diff.AffectedOpts {
            let affected = model::SchemaDiff::from_affected(diff.Version, tp, opt);
            ids.extend(self.ApplyDiff(m, &affected)?);
        }
        Ok(ids)
    }

    // getTableIDs 保留 Go 对建表、删表、截表及普通表更新的旧/新 ID 选择规则。
    fn getTableIDs(&self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<(i64, i64), Error> {
        match diff.Type {
            model::ActionCreateSequence | model::ActionRecoverTable => Ok((0, diff.TableID)),
            model::ActionCreateTable => Ok((diff.OldTableID, diff.TableID)),
            model::ActionDropTable | model::ActionDropView | model::ActionDropSequence => {
                if diff.IsRefreshMeta { return Ok((diff.TableID, 0)); }
                // 外键级联在 StateNone 前仍需旧表留在 InfoSchema，因此重新读取状态。
                let keep = m.GetTable(diff.SchemaID, diff.TableID)?
                    .is_some_and(|table| table.State != model::StateNone);
                Ok((diff.TableID, if keep { diff.TableID } else { 0 }))
            }
            model::ActionTruncateTable | model::ActionCreateView | model::ActionExchangeTablePartition
            | model::ActionAlterTablePartitioning | model::ActionRemovePartitioning => Ok((diff.OldTableID, diff.TableID)),
            _ => Ok((diff.TableID, diff.TableID)),
        }
    }

    fn updateBundleForTableUpdate(&mut self, diff: &model::SchemaDiff, new_id: i64, old_id: i64) {
        match diff.Type {
            model::ActionCreateTable | model::ActionAddTablePartition | model::ActionRecoverTable
            | model::ActionAlterTablePlacement | model::ActionAlterTablePartitionPlacement => self.markTableBundleShouldUpdate(new_id),
            model::ActionDropTable => self.deleteBundle(old_id),
            model::ActionTruncateTable => { self.deleteBundle(old_id); self.markTableBundleShouldUpdate(new_id); }
            _ => {}
        }
    }

    // applyTableUpdate 是表级 diff 的共同主线：写时复制、删除旧表、复用 allocator、创建新表。
    fn applyTableUpdate(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
        let ro = self.infoSchema().SchemaByID(diff.SchemaID).ok_or_else(|| Error::database_not_exists(diff.SchemaID))?;
        let db = self.getSchemaAndCopyIfNecessary(&ro.Name.L);
        let (old_id, new_id) = self.getTableIDs(m, diff)?;
        self.updateBundleForTableUpdate(diff, new_id, old_id);
        self.copySortedTables(old_id, new_id);
        let (mut ids, allocs) = dropTableForUpdate(self, new_id, old_id, db, diff)?;
        if tableIDIsValid(new_id) {
            ids = applyCreateTable(self, m, db, new_id, allocs, diff.Type, ids, diff.Version)?;
        }
        if needRefreshMaskingPoliciesForTableDiff(diff.Type) {
            refreshMaskingPoliciesForTableIDs(self, &[old_id, new_id])?;
        }
        Ok(ids)
    }

    fn applyCreateSchema(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<(), Error> {
        let db = m.GetDatabase(diff.SchemaID)?.ok_or_else(|| Error::database_not_exists(diff.SchemaID))?;
        self.addDB(diff.Version, db.clone(), schemaTables::empty(db));
        Ok(())
    }

    fn applyModifySchemaCharsetAndCollate(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<(), Error> {
        let current = m.GetDatabase(diff.SchemaID)?.ok_or_else(|| Error::database_not_exists(diff.SchemaID))?;
        let db = self.getSchemaAndCopyIfNecessary(&current.Name.L);
        db.Charset = current.Charset;
        db.Collate = current.Collate;
        Ok(())
    }

    fn applyModifySchemaDefaultPlacement(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<(), Error> {
        let current = m.GetDatabase(diff.SchemaID)?.ok_or_else(|| Error::database_not_exists(diff.SchemaID))?;
        self.getSchemaAndCopyIfNecessary(&current.Name.L).PlacementPolicyRef = current.PlacementPolicyRef;
        Ok(())
    }

    // applyDropSchema 先复制受影响 bucket，再删除表、分区及 placement bundle。
    fn applyDropSchema(&mut self, diff: &model::SchemaDiff) -> Vec<i64> {
        let Some(db) = self.infoSchema().SchemaByID(diff.SchemaID).cloned() else { return vec![]; };
        self.infoSchema_mut().delSchema(&db);
        let mut ids = Vec::new();
        let mut buckets = HashSet::new();
        for table in &db.Deprecated.Tables {
            buckets.insert(tableBucketIdx(table.ID));
            ids = appendAffectedIDs(ids, table);
        }
        for bucket in buckets { self.copySortedTablesBucket(bucket); }
        for id in ids.clone() { self.deleteBundle(id); self.applyDropTable(diff, &mut db.clone(), id, vec![]); }
        ids
    }

    fn applyRecoverSchema(&mut self, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
        if self.infoSchema().SchemaByID(diff.SchemaID).is_some() { return Err(Error::database_exists(diff.SchemaID)); }
        let db = m.GetDatabase(diff.SchemaID)?.ok_or_else(|| Error::database_not_exists(diff.SchemaID))?;
        self.infoSchema_mut().addSchema(schemaTables::with_capacity(db, diff.AffectedOpts.len()));
        self.applyCreateTables(m, diff)
    }

    fn copySortedTables(&mut self, old_id: i64, new_id: i64) {
        if tableIDIsValid(old_id) { self.copySortedTablesBucket(tableBucketIdx(old_id)); }
        if tableIDIsValid(new_id) && new_id != old_id { self.copySortedTablesBucket(tableBucketIdx(new_id)); }
    }

    fn copySortedTablesBucket(&mut self, bucket: usize) {
        // Rust clone 对应 Go copy，确保旧 InfoSchema 持有的 slice 仍只读。
        self.infoSchema_mut().sortedTablesBuckets[bucket] = self.infoSchema().sortedTablesBuckets[bucket].clone();
    }

    fn buildAllocsForCreateTable(&self, tp: model::ActionType, db: &model::DBInfo, table: &model::TableInfo, mut allocs: autoid::Allocators) -> autoid::Allocators {
        if allocs.Allocs.is_empty() { return autoid::NewAllocatorsFromTblInfo(&self.Requirement, db.ID, table); }
        match tp {
            model::ActionRebaseAutoID | model::ActionModifyTableAutoIDCache => {
                for kind in [autoid::AutoIncrementType, autoid::RowIDAllocType] {
                    allocs.Append(autoid::NewAllocator(&self.Requirement, db.ID, table.ID, table.IsAutoIncColUnsigned(), kind));
                }
            }
            model::ActionRebaseAutoRandomBase => allocs.Append(autoid::NewAllocator(&self.Requirement, db.ID, table.ID, table.IsAutoRandomBitColUnsigned(), autoid::AutoRandomType)),
            model::ActionModifyColumn if table.ContainsAutoRandomBits() && allocs.Get(autoid::AutoRandomType).is_none() => {
                allocs = allocs.Filter(|a| !matches!(a.GetType(), autoid::AutoIncrementType | autoid::RowIDAllocType));
                allocs.Append(autoid::NewAllocator(&self.Requirement, db.ID, table.ID, table.IsAutoRandomBitColUnsigned(), autoid::AutoRandomType));
            }
            _ => {}
        }
        allocs
    }

    // applyDropTable 同步名字映射、ID bucket、临时表集合和外键反向索引。
    fn applyDropTable(&mut self, _diff: &model::SchemaDiff, db: &mut model::DBInfo, table_id: i64, mut affected: Vec<i64>) -> Vec<i64> {
        let bucket = tableBucketIdx(table_id);
        let Some(index) = self.infoSchema().sortedTablesBuckets[bucket].searchTable(table_id) else { return affected; };
        let table = self.infoSchema().sortedTablesBuckets[bucket][index].Meta().clone();
        if let Some(names) = self.infoSchema_mut().schemaMap.get_mut(&db.Name.L) {
            names.tables.remove(&table.Name.L);
            affected = appendAffectedIDs(affected, &table);
        }
        self.infoSchema_mut().sortedTablesBuckets[bucket].remove(index);
        self.infoSchema_mut().temporaryTableIDs.remove(&table_id);
        self.deleteReferredForeignKeys(db, table_id);
        affected
    }

    fn deleteReferredForeignKeys(&mut self, db: &mut model::DBInfo, table_id: i64) {
        if let Some(index) = db.Deprecated.Tables.iter().position(|table| table.ID == table_id) {
            let table = db.Deprecated.Tables.remove(index);
            self.infoSchema_mut().deleteReferredForeignKeys(&db.Name, &table);
        }
    }

    // Build 写入快照 TS 并收束 bundle 更新；返回 V1 或 V2 接口对象。
    pub fn Build(&mut self, schemaTS: u64) -> &dyn InfoSchema {
        if self.enableV2 {
            self.infoschemaV2.ts = schemaTS;
            self.infoschemaV2.infoSchema.ts = schemaTS;
            updateInfoSchemaBundles(self);
            return &self.infoschemaV2;
        }
        self.infoschemaV2.infoSchema.ts = schemaTS;
        updateInfoSchemaBundles(self);
        &self.infoschemaV2.infoSchema
    }

    // InitWithOldInfoSchema 对应增量构建初始化；映射和 bucket 采用浅层写时复制。
    pub fn InitWithOldInfoSchema(&mut self, old: &dyn InfoSchema) -> Result<(), Error> {
        let is_v2 = IsV2(old);
        if self.enableV2 != is_v2 { return Err(Error::infoschema_version_mismatch(self.enableV2)); }
        let base = old.base();
        self.initBundleInfoBuilder();
        self.infoschemaV2.infoSchema.clone_metadata_from(base);
        // Go 在读锁内复制 masking-policy 二级 map，并清除正在加载的 channel。
        self.infoschemaV2.infoSchema.clone_masking_policies_from(base);
        self.infoschemaV2.infoSchema.sortedTablesBuckets.clone_from(&base.sortedTablesBuckets);
        Ok(())
    }

    fn getSchemaAndCopyIfNecessary(&mut self, db_name: &str) -> &mut model::DBInfo {
        if !self.dirtyDB.get(db_name).copied().unwrap_or(false) {
            self.dirtyDB.insert(db_name.to_owned(), true);
            let copied = self.infoSchema().schemaMap[db_name].deep_clone();
            self.infoSchema_mut().addSchema(copied);
        }
        &mut self.infoSchema_mut().schemaMap.get_mut(db_name).unwrap().dbInfo
    }

    fn initVirtualTables(&mut self, schema_version: i64) -> Result<(), Error> {
        if self.crossKS { return Ok(()); }
        // 跨 keyspace 会话禁止访问虚拟表；普通构建逐个调用已注册 driver。
        for driver in virtual_table_drivers() {
            self.createSchemaTablesForDB(driver.DBInfo.clone(), driver.TableFromMeta, schema_version)?;
        }
        Ok(())
    }

    fn sortAllTablesByID(&mut self) {
        for bucket in &mut self.infoSchema_mut().sortedTablesBuckets {
            bucket.sort_by_key(|table| table.Meta().ID);
        }
    }

    // InitWithDBInfos 对应全量加载：先实体库表，再杂项元数据、虚拟表和 ID 排序。
    pub fn InitWithDBInfos(&mut self, dbs: Vec<model::DBInfo>, policies: Vec<model::PolicyInfo>, groups: Vec<model::ResourceGroupInfo>, masking: Vec<model::MaskingPolicyInfo>, version: i64) -> Result<(), Error> {
        self.infoschemaV2.infoSchema.schemaMetaVersion = version;
        if self.enableV2 { self.infoData.resetBeforeFullLoad(version); }
        self.initBundleInfoBuilder();
        for db in dbs { self.createSchemaTablesForDB(db, tableFromMeta, version)?; }
        self.initMisc(policies, groups, masking);
        self.initVirtualTables(version)?;
        self.sortAllTablesByID();
        Ok(())
    }

    fn createSchemaTablesForDB(&mut self, mut db: model::DBInfo, convert: tableFromMetaFunc, version: i64) -> Result<(), Error> {
        let mut schema = schemaTables::with_capacity(db.clone(), db.Deprecated.Tables.len());
        for info in db.Deprecated.Tables.clone() {
            let allocs = autoid::NewAllocatorsFromTblInfo(&self.Requirement, db.ID, &info);
            let table = convert(allocs, self.factory, &info)?;
            schema.tables.insert(info.Name.L.clone(), table.clone());
            self.addTable(version, &db, &info, table);
            db.TableName2ID.remove(&info.Name.O);
        }
        // V2 还需保留未加载表的 name→ID 项，供惰性 table cache 查找。
        if self.enableV2 { for (name, id) in &db.TableName2ID { self.infoData.add_name_mapping(&db, name, *id, version); } }
        self.addDB(version, db, schema);
        Ok(())
    }

    fn addDB(&mut self, version: i64, db: model::DBInfo, schema: schemaTables) {
        if self.enableV2 {
            if IsSpecialDB(&db.Name.L) { self.infoData.addSpecialDB(db, schema); } else { self.infoData.addDB(version, db); }
        } else { self.infoSchema_mut().addSchema(schema); }
    }

    fn addTable(&mut self, version: i64, db: &model::DBInfo, info: &model::TableInfo, table: table::Table) {
        if self.enableV2 {
            self.infoData.addReferredForeignKeys(&db.Name, info, version);
            self.infoData.add(tableItem::new(db, info, version), table);
        } else {
            self.infoSchema_mut().sortedTablesBuckets[tableBucketIdx(info.ID)].push(table);
            self.infoSchema_mut().addReferredForeignKeys(&db.Name, info);
        }
    }

    pub fn WithStore(mut self, store: kv::Storage) -> Self { self.store = Some(store); self }
    pub fn WithCrossKS(mut self, cross: bool) -> Self { self.crossKS = cross; self }

    fn schemaByID(&self, id: i64) -> Option<&model::DBInfo> {
        if self.enableV2 { self.infoschemaV2.SchemaByID(id) } else { self.infoSchema().SchemaByID(id) }
    }

    fn infoSchema(&self) -> &infoSchema { &self.infoschemaV2.infoSchema }
    fn infoSchema_mut(&mut self) -> &mut infoSchema { &mut self.infoschemaV2.infoSchema }
}

// equalPlacementPolicy 对应 Go 的 nil 安全比较，ID 和小写名称均需一致。
fn equalPlacementPolicy(a: Option<&model::PolicyRefInfo>, b: Option<&model::PolicyRefInfo>) -> bool {
    match (a, b) { (None, None) => true, (Some(a), Some(b)) => a.ID == b.ID && a.Name.L == b.Name.L, _ => false }
}

// applyRefreshMeta 对应 PITR 刷新：BR 必须按删表、删库、建库、建表的顺序投递。
fn applyRefreshMeta(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    if diff.TableID == 0 {
        match m.GetDatabase(diff.SchemaID)? {
            None => return Ok(b.applyDropSchema(&diff.as_action(model::ActionDropSchema))),
            Some(db) if b.schemaByID(diff.SchemaID).is_none() => b.applyCreateSchema(m, &diff.as_action(model::ActionCreateSchema))?,
            Some(db) => {
                let current = b.schemaByID(diff.SchemaID).unwrap();
                if current.Charset != db.Charset || current.Collate != db.Collate { b.applyModifySchemaCharsetAndCollate(m, diff)?; }
                if !equalPlacementPolicy(current.PlacementPolicyRef.as_ref(), db.PlacementPolicyRef.as_ref()) { b.applyModifySchemaDefaultPlacement(m, diff)?; }
            }
        }
        return Ok(vec![]);
    }
    if b.schemaByID(diff.SchemaID).is_none() { return Ok(vec![]); }
    match m.GetTable(diff.SchemaID, diff.TableID)? {
        None => applyDropTableOrPartition(b, m, &diff.as_action(model::ActionDropTable)),
        Some(_) => applyDefaultAction(b, m, &diff.as_action(model::ActionCreateTable)),
    }
}

fn applyTruncateTableOrPartition(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    let mut ids = b.applyTableUpdate(m, diff)?;
    if diff.Type == model::ActionTruncateTable { b.deleteBundle(diff.OldTableID); }
    b.markTableBundleShouldUpdate(diff.TableID);
    for opt in &diff.AffectedOpts {
        if diff.Type == model::ActionTruncateTablePartition { ids.push(opt.OldTableID); }
        b.deleteBundle(opt.OldTableID);
    }
    Ok(ids)
}

fn applyDropTableOrPartition(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    let ids = b.applyTableUpdate(m, diff)?;
    b.markTableBundleShouldUpdate(diff.TableID);
    for opt in &diff.AffectedOpts { b.deleteBundle(opt.OldTableID); }
    Ok(ids)
}

fn applyReorganizePartition(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    let ids = b.applyTableUpdate(m, diff)?;
    if diff.TableID != diff.OldTableID && diff.OldTableID != 0 { b.deleteBundle(diff.OldTableID); }
    b.markTableBundleShouldUpdate(diff.TableID);
    for opt in &diff.AffectedOpts { if opt.OldTableID != 0 { b.deleteBundle(opt.OldTableID); } }
    Ok(ids)
}

// applyExchangeTablePartition 保留先更新普通表、再重载分区表、最后对齐 auto ID 的顺序。
fn applyExchangeTablePartition(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    if diff.OldTableID == diff.TableID && diff.OldSchemaID == diff.SchemaID {
        let mut ids = b.applyTableUpdate(m, diff)?;
        if let Some(opt) = diff.AffectedOpts.first().filter(|opt| opt.OldSchemaID != 0) {
            ids.splice(0..0, b.applyTableUpdate(m, &model::SchemaDiff::reload_partition(diff, opt))?);
        }
        return Ok(ids);
    }
    let opt = diff.AffectedOpts.first();
    let partitioned_id = opt.map_or(diff.TableID, |v| v.TableID);
    let partitioned_schema = opt.filter(|v| v.SchemaID != 0).map_or(diff.SchemaID, |v| v.SchemaID);
    let mut normal_ids = b.applyTableUpdate(m, &model::SchemaDiff::normal_table_for_exchange(diff, partitioned_id))?;
    b.markTableBundleShouldUpdate(diff.TableID);
    b.markTableBundleShouldUpdate(partitioned_id);
    let mut partition_ids = b.applyTableUpdate(m, &model::SchemaDiff::partitioned_table_for_exchange(diff, partitioned_schema, partitioned_id))?;
    updateAutoIDForExchangePartition(b.store.as_ref().unwrap(), partitioned_schema, partitioned_id, diff.OldSchemaID, diff.OldTableID)?;
    partition_ids.append(&mut normal_ids);
    Ok(partition_ids)
}

fn applyRecoverTable(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    let ids = b.applyTableUpdate(m, diff)?;
    for opt in &diff.AffectedOpts { b.markTableBundleShouldUpdate(opt.TableID); }
    Ok(ids)
}

fn applyMaskingPolicyChange(b: &mut Builder) -> Result<Vec<i64>, Error> { b.infoSchema_mut().resetMaskingPolicyCache(); Ok(vec![]) }

// updateAutoIDForExchangePartition 对应 Go 新事务：两张表都写入三类 auto ID 的逐项最大值。
fn updateAutoIDForExchangePartition(store: &kv::Storage, ps: i64, pt: i64, ns: i64, nt: i64) -> Result<(), Error> {
    kv::RunInNewTxn(store, true, |txn| {
        let mut meta = meta::NewMutator(txn);
        let partition = meta.GetAutoIDAccessors(ps, pt).Get()?;
        let normal = meta.GetAutoIDAccessors(ns, nt).Get()?;
        let merged = model::AutoIDGroup::component_max(partition, normal);
        meta.GetAutoIDAccessors(ps, pt).Put(merged.clone())?;
        meta.GetAutoIDAccessors(ns, nt).Put(merged)
    })
}

fn applyDefaultAction(b: &mut Builder, m: &dyn meta::Reader, diff: &model::SchemaDiff) -> Result<Vec<i64>, Error> {
    let ids = b.applyTableUpdate(m, diff)?;
    b.applyAffectedOpts(m, ids, diff, diff.Type)
}

fn dropTableForUpdate(b: &mut Builder, new_id: i64, old_id: i64, db: &mut model::DBInfo, diff: &model::SchemaDiff) -> Result<(Vec<i64>, autoid::Allocators), Error> {
    let mut ids = vec![];
    let mut kept = autoid::Allocators::default();
    if tableIDIsValid(old_id) {
        if old_id == new_id && !matches!(diff.Type, model::ActionRepairTable | model::ActionAlterSequence | model::ActionExchangeTablePartition) {
            kept = getKeptAllocators(diff, allocByID(b, old_id).unwrap_or_default());
        }
        let dropped = if matches!(diff.Type, model::ActionRenameTable | model::ActionRenameTables) && diff.OldSchemaID != diff.SchemaID {
            let old_db = oldSchemaInfo(b, diff).ok_or_else(|| Error::database_not_exists(diff.OldSchemaID))?;
            b.applyDropTable(diff, old_db, old_id, vec![])
        } else { b.applyDropTable(diff, db, old_id, vec![]) };
        if old_id != new_id { ids = dropped; }
    }
    Ok((ids, kept))
}

fn needRefreshMaskingPoliciesForTableDiff(tp: model::ActionType) -> bool {
    matches!(tp, model::ActionCreateMaskingPolicy | model::ActionAlterMaskingPolicy | model::ActionDropMaskingPolicy
        | model::ActionDropTable | model::ActionDropColumn | model::ActionModifyColumn | model::ActionRenameTable
        | model::ActionRenameTables | model::ActionTruncateTable | model::ActionDropSchema)
}

fn refreshMaskingPoliciesForTableIDs(b: &mut Builder, _ids: &[i64]) -> Result<(), Error> { b.infoSchema_mut().resetMaskingPolicyCache(); Ok(()) }

// getKeptAllocators 只丢弃被当前 DDL 改变的 allocator，其余缓存继续复用。
fn getKeptAllocators(diff: &model::SchemaDiff, old: autoid::Allocators) -> autoid::Allocators {
    let auto_id = matches!(diff.Type, model::ActionRebaseAutoID | model::ActionModifyTableAutoIDCache)
        || diff.SubActionTypes.iter().any(|t| matches!(t, model::ActionRebaseAutoID | model::ActionModifyTableAutoIDCache));
    let auto_random = diff.Type == model::ActionRebaseAutoRandomBase
        || diff.SubActionTypes.contains(&model::ActionRebaseAutoRandomBase);
    if auto_id { old.Filter(|a| !matches!(a.GetType(), autoid::RowIDAllocType | autoid::AutoIncrementType)) }
    else if auto_random { old.Filter(|a| a.GetType() != autoid::AutoRandomType) }
    else { old }
}

fn appendAffectedIDs(mut affected: Vec<i64>, table: &model::TableInfo) -> Vec<i64> {
    affected.push(table.ID);
    if let Some(partitions) = table.GetPartitionInfo() { affected.extend(partitions.Definitions.iter().map(|part| part.ID)); }
    affected
}

// applyCreateTable 读取元数据、转移 allocator、创建表对象，并维护指标、bucket 与临时表集合。
fn applyCreateTable(b: &mut Builder, m: &dyn meta::Reader, db: &mut model::DBInfo, id: i64, mut allocs: autoid::Allocators, tp: model::ActionType, mut affected: Vec<i64>, version: i64) -> Result<Vec<i64>, Error> {
    let mut info = m.GetTable(db.ID, id)?.ok_or_else(|| Error::table_not_exists(db.ID, id))?;
    if tp != model::ActionTruncateTablePartition { affected = appendAffectedIDs(affected, &info); }
    ConvertCharsetCollateToLowerCaseIfNeed(&mut info);
    ConvertOldVersionUTF8ToUTF8MB4IfNeed(&mut info);
    for alloc in &mut allocs.Allocs { alloc.Transfer(db.ID, id)?; }
    allocs = b.buildAllocsForCreateTable(tp, db, &info, allocs);
    let table = tableFromMeta(allocs, b.factory, &info)?;
    if info.Indices.iter().all(|index| index.State == model::StatePublic) { metrics::DDLResetTempIndexWrite(info.ID); }
    if info.Columns.iter().all(|col| col.State == model::StatePublic) && info.Indices.iter().all(|idx| idx.State == model::StatePublic) && metrics::DDLHasBackfillMetrics() {
        metrics::DDLClearBackfillMetrics(info.ID);
        if let Some(parts) = &info.Partition { for part in &parts.Definitions { metrics::DDLClearBackfillMetrics(part.ID); } }
    }
    if !b.enableV2 { b.infoSchema_mut().schemaMap.get_mut(&db.Name.L).unwrap().tables.insert(info.Name.L.clone(), table.clone()); }
    b.addTable(version, db, &info, table);
    b.infoSchema_mut().sortedTablesBuckets[tableBucketIdx(id)].sort_by_key(|table| table.Meta().ID);
    if info.TempTableType != model::TempTableNone { b.addTemporaryTable(id); }
    if let Some(new_table) = b.infoSchema().TableByID(id) { db.Deprecated.Tables.push(new_table.Meta().clone()); }
    Ok(affected)
}

// ConvertCharsetCollateToLowerCaseIfNeed 兼容 TableInfoVersion3 之前的大小写元数据。
pub fn ConvertCharsetCollateToLowerCaseIfNeed(table: &mut model::TableInfo) {
    if table.Version >= model::TableInfoVersion3 { return; }
    table.Charset.make_ascii_lowercase(); table.Collate.make_ascii_lowercase();
    for col in &mut table.Columns { col.SetCharset(col.GetCharset().to_lowercase()); col.SetCollate(col.GetCollate().to_lowercase()); }
}

// ConvertOldVersionUTF8ToUTF8MB4IfNeed 按全局兼容开关升级旧表及旧列字符集。
pub fn ConvertOldVersionUTF8ToUTF8MB4IfNeed(table: &mut model::TableInfo) {
    if table.Version >= model::TableInfoVersion2 || !config::GetGlobalConfig().TreatOldVersionUTF8AsUTF8MB4 { return; }
    if table.Charset == charset::CharsetUTF8 { table.Charset = charset::CharsetUTF8MB4.into(); table.Collate = charset::CollationUTF8MB4.into(); }
    for col in &mut table.Columns {
        if col.Version < model::ColumnInfoVersion2 && col.GetCharset() == charset::CharsetUTF8 {
            col.SetCharset(charset::CharsetUTF8MB4.into()); col.SetCollate(charset::CollationUTF8MB4.into());
        }
    }
}

fn tableFromMeta(allocs: autoid::Allocators, factory: ResourceFactory, info: &model::TableInfo) -> Result<table::Table, Error> {
    let mut table = tables::TableFromMeta(allocs, info)?;
    // CachedTable 需要从 session resource 取得 SQLExecutor；这里只保留初始化调用形状。
    if let Some(cached) = table.as_cached_mut() { let resource = factory()?; cached.Init(resource.GetSQLExecutor())?; }
    Ok(table)
}

pub type tableFromMetaFunc = fn(autoid::Allocators, ResourceFactory, &model::TableInfo) -> Result<table::Table, Error>;

pub struct virtualTableDriver { pub DBInfo: model::DBInfo, pub TableFromMeta: tableFromMetaFunc }

static DRIVERS: std::sync::Mutex<Vec<virtualTableDriver>> = std::sync::Mutex::new(Vec::new());
fn virtual_table_drivers() -> std::sync::MutexGuard<'static, Vec<virtualTableDriver>> { DRIVERS.lock().unwrap() }

// RegisterVirtualTable 对应 Go 全局注册表；互斥锁补足 Rust 静态可变状态的同步要求。
pub fn RegisterVirtualTable(db: model::DBInfo, convert: tableFromMetaFunc) { DRIVERS.lock().unwrap().push(virtualTableDriver { DBInfo: db, TableFromMeta: convert }); }

// NewBuilder 创建 V2 基础对象并设置 table cache 容量；不会触发任何元数据 IO。
pub fn NewBuilder(requirement: autoid::Requirement, cache_size: u64, factory: ResourceFactory, mut data: Data, use_v2: bool) -> Builder {
    let v2 = NewInfoSchemaV2(&requirement, factory, &data);
    data.tableCache.SetCapacity(cache_size);
    Builder { enableV2: use_v2, infoschemaV2: v2, dirtyDB: HashMap::new(), Requirement: requirement,
        factory, bundleInfoBuilder: bundleInfoBuilder::default(), infoData: data, store: None, crossKS: false }
}

fn tableBucketIdx(table_id: i64) -> usize { assert!(table_id > 0); table_id as usize % bucketCount }
fn tableIDIsValid(table_id: i64) -> bool { table_id > 0 }
*/

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::infoschema::{
    DBInfo, InfoSchema, PolicyInfo, ResourceGroupInfo, Table, TableInfo, infoSchema,
};
use crate::infoschema_v2::{Data, infoschemaV2};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// DDL 动作类型，对应 Go `model.ActionType`，驱动 `ApplyDiff` 分派。
pub enum ActionType {
    #[default]
    None,
    CreateSchema,
    DropSchema,
    RecoverSchema,
    ModifySchemaCharsetAndCollate,
    ModifySchemaDefaultPlacement,
    CreateTable,
    CreateTables,
    DropTable,
    TruncateTable,
    RecoverTable,
    RenameTable,
    RenameTables,
    AddTablePartition,
    DropTablePartition,
    TruncateTablePartition,
    ReorganizePartition,
    ExchangeTablePartition,
    AlterTablePartitioning,
    RemovePartitioning,
    CreatePlacementPolicy,
    AlterPlacementPolicy,
    DropPlacementPolicy,
    CreateResourceGroup,
    AlterResourceGroup,
    DropResourceGroup,
    CreateMaskingPolicy,
    AlterMaskingPolicy,
    DropMaskingPolicy,
    RefreshMeta,
    // Allocator-related actions used by getKeptAllocators (Go model.Action*).
    RebaseAutoID,
    ModifyTableAutoIDCache,
    RebaseAutoRandomBase,
    MultiSchemaChange,
    AddColumn,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次 diff 中额外受影响的库表 ID 选项（如批量建表、重命名）。
pub struct AffectedOption {
    pub schema_id: i64,
    pub old_schema_id: i64,
    pub table_id: i64,
    pub old_table_id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单次 schema 变更描述：版本、动作、主库表 ID 及附属选项。
pub struct SchemaDiff {
    pub version: i64,
    pub action_type: ActionType,
    pub schema_id: i64,
    pub table_id: i64,
    pub old_schema_id: i64,
    pub old_table_id: i64,
    pub affected_options: Vec<AffectedOption>,
    /// Sub-actions for MultiSchemaChange (Go `SchemaDiff.SubActionTypes`).
    pub sub_action_types: Vec<ActionType>,
}

/// 从元存储读取库/表/策略/资源组的抽象；当前多为后续接线占位。
pub trait MetadataReader {
    fn database(&self, id: i64) -> Result<Option<DBInfo>, String>;
    fn table(&self, schema_id: i64, table_id: i64) -> Result<Option<TableInfo>, String>;
    fn policy(&self, id: i64) -> Result<Option<PolicyInfo>, String> {
        let _ = id;
        Ok(None)
    }
    fn resource_group(&self, id: i64) -> Result<Option<ResourceGroupInfo>, String> {
        let _ = id;
        Ok(None)
    }
}

#[derive(Clone)]
/// Builder 内部的库状态：库信息与表 ID → Table 映射。
struct DatabaseState {
    info: DBInfo,
    tables: HashMap<i64, Table>,
}

pub struct Builder {
    /// 是否写入 infoschema v2 的 `Data` 后端。
    enable_v2: bool,
    /// 当前构建对应的 schema 元数据版本。
    schema_version: i64,
    /// Build 时写入的 schema 时间戳。
    schema_ts: u64,
    /// 库 ID → 库内表集合。
    databases: HashMap<i64, DatabaseState>,
    /// Placement Policy 缓存。
    policies: HashMap<i64, PolicyInfo>,
    /// Resource Group 缓存。
    resource_groups: HashMap<i64, ResourceGroupInfo>,
    /// 临时表 ID 集合。
    temporary_table_ids: HashSet<i64>,
    /// v2 路径共享的 Data。
    info_data: Arc<Data>,
    /// 是否跨 keyspace（多租户键空间）构建。
    cross_keyspace: bool,
}

impl Builder {
    /// 创建空 Builder。
    pub fn new(info_data: Arc<Data>, use_v2: bool) -> Self {
        Self {
            enable_v2: use_v2,
            schema_version: 0,
            schema_ts: 0,
            databases: HashMap::new(),
            policies: HashMap::new(),
            resource_groups: HashMap::new(),
            temporary_table_ids: HashSet::new(),
            info_data,
            cross_keyspace: false,
        }
    }
    /// 设置后续 diff 应用使用的 schema 版本。
    pub fn SetSchemaVersion(&mut self, version: i64) {
        self.schema_version = version;
    }
    /// 占位：Go 侧绑定 KV Storage，此处恒等返回。
    pub fn WithStore(self) -> Self {
        self
    }
    /// 设置跨 keyspace 标志。
    pub fn WithCrossKS(mut self, cross_keyspace: bool) -> Self {
        self.cross_keyspace = cross_keyspace;
        self
    }

    /// 按 `action_type` 分派 DDL 变更，返回受影响的表/分区 ID 列表。
    pub fn ApplyDiff(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        // 先对齐版本，再按动作类型更新内存元数据。
        self.SetSchemaVersion(diff.version);
        let mut affected = Vec::new();
        match diff.action_type {
            ActionType::CreateSchema => self.applyCreateSchema(metadata, diff)?,
            ActionType::DropSchema => affected.extend(self.applyDropSchema(diff)),
            ActionType::RecoverSchema => affected.extend(self.applyRecoverSchema(metadata, diff)?),
            ActionType::ModifySchemaCharsetAndCollate
            | ActionType::ModifySchemaDefaultPlacement => {
                self.refresh_schema(metadata, diff.schema_id)?
            }
            ActionType::CreateTable | ActionType::RecoverTable => {
                affected.extend(self.applyTableUpdate(metadata, diff)?)
            }
            // 主表更新后，再处理 AffectedOptions 中的其余表。
            ActionType::CreateTables => {
                for option in &diff.affected_options {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        0,
                    )?);
                }
            }
            ActionType::DropTable => {
                affected.extend(self.applyDropTable(diff.schema_id, diff.table_id))
            }
            // 截断/重命名/分区变更等：更新主表并处理附属 old/new 表 ID。
            ActionType::TruncateTable
            | ActionType::RenameTable
            | ActionType::RenameTables
            | ActionType::AddTablePartition
            | ActionType::DropTablePartition
            | ActionType::TruncateTablePartition
            | ActionType::ReorganizePartition
            | ActionType::ExchangeTablePartition
            | ActionType::AlterTablePartitioning
            | ActionType::RemovePartitioning => {
                affected.extend(self.applyTableUpdate(metadata, diff)?);
                for option in &diff.affected_options {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        option.old_table_id,
                    )?);
                }
            }
            ActionType::RefreshMeta => affected.extend(self.applyRefreshMeta(metadata, diff)?),
            ActionType::CreatePlacementPolicy => self.applyCreatePolicy(metadata, diff.table_id)?,
            ActionType::AlterPlacementPolicy => {
                self.applyCreatePolicy(metadata, diff.table_id)?;
                affected.extend(self.tables_referencing_policy(diff.table_id));
            }
            ActionType::DropPlacementPolicy => {
                self.policies.remove(&diff.table_id);
                affected.extend(self.tables_referencing_policy(diff.table_id));
            }
            ActionType::CreateResourceGroup | ActionType::AlterResourceGroup => {
                self.applyResourceGroup(metadata, diff.table_id)?
            }
            ActionType::DropResourceGroup => {
                self.resource_groups.remove(&diff.table_id);
            }
            // Masking / 仅分配器相关动作在此路径暂不修改结构。
            ActionType::CreateMaskingPolicy
            | ActionType::AlterMaskingPolicy
            | ActionType::DropMaskingPolicy
            | ActionType::None
            | ActionType::RebaseAutoID
            | ActionType::ModifyTableAutoIDCache
            | ActionType::RebaseAutoRandomBase
            | ActionType::MultiSchemaChange
            | ActionType::AddColumn => {}
        }
        // 去重排序后返回，便于调用方稳定比较。
        affected.sort_unstable();
        affected.dedup();
        Ok(affected)
    }

    /// 从 MetadataReader 加载库并插入；v2 同步 addDB。
    fn applyCreateSchema(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<(), String> {
        let db = metadata
            .database(diff.schema_id)?
            .ok_or_else(|| format!("database {} not found", diff.schema_id))?;
        self.databases.insert(
            db.id,
            DatabaseState {
                info: db.clone(),
                tables: HashMap::new(),
            },
        );
        // v2：直接封装共享 Data；v1：把库表与策略填入 infoSchema。
        if self.enable_v2 {
            self.info_data.addDB(diff.version, db);
        }
        Ok(())
    }
    /// 删除库及其表，返回受影响表 ID。
    fn applyDropSchema(&mut self, diff: &SchemaDiff) -> Vec<i64> {
        let Some(db) = self.databases.remove(&diff.schema_id) else {
            return Vec::new();
        };
        let affected: Vec<i64> = db.tables.keys().copied().collect();
        if self.enable_v2 {
            self.info_data.deleteDB(db.info, diff.version);
        }
        affected
    }
    /// 恢复库后，按 affected_options 逐表恢复。
    fn applyRecoverSchema(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        self.applyCreateSchema(metadata, diff)?;
        let table_ids: Vec<i64> = diff
            .affected_options
            .iter()
            .map(|option| option.table_id)
            .collect();
        let mut affected = Vec::new();
        for table_id in table_ids {
            affected.extend(self.apply_table_ids(metadata, diff.schema_id, table_id, 0)?);
        }
        Ok(affected)
    }
    /// 刷新库级属性（字符集/默认 placement 等）。
    fn refresh_schema(
        &mut self,
        metadata: &dyn MetadataReader,
        schema_id: i64,
    ) -> Result<(), String> {
        let db = metadata
            .database(schema_id)?
            .ok_or_else(|| format!("database {schema_id} not found"))?;
        if let Some(state) = self.databases.get_mut(&schema_id) {
            state.info = db.clone();
        } else {
            self.databases.insert(
                schema_id,
                DatabaseState {
                    info: db.clone(),
                    tables: HashMap::new(),
                },
            );
        }
        if self.enable_v2 {
            self.info_data.addDB(self.schema_version, db);
        }
        Ok(())
    }
    /// PITR 元数据刷新：库级 diff 创建/更新/删除库，表级 diff 创建/更新/删除表。
    fn applyRefreshMeta(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        if diff.table_id == 0 {
            if metadata.database(diff.schema_id)?.is_none() {
                return Ok(self.applyDropSchema(diff));
            }
            if self.databases.contains_key(&diff.schema_id) {
                self.refresh_schema(metadata, diff.schema_id)?;
            } else {
                self.applyCreateSchema(metadata, diff)?;
            }
            return Ok(Vec::new());
        }

        // 与 Go 一致：库已不存在时，其表必然也已从一致快照中移除。
        if !self.databases.contains_key(&diff.schema_id) {
            return Ok(Vec::new());
        }
        if metadata.table(diff.schema_id, diff.table_id)?.is_none() {
            return Ok(self.applyDropTable(diff.schema_id, diff.table_id));
        }
        self.applyTableUpdate(metadata, diff)
    }
    /// 表级更新：若存在 old_table_id 则先删旧再写新。
    fn applyTableUpdate(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        let old_id = if diff.old_table_id != 0 {
            diff.old_table_id
        } else {
            diff.table_id
        };
        self.apply_table_ids(metadata, diff.schema_id, diff.table_id, old_id)
    }
    /// 写入新表元数据，必要时删除旧表，并收集分区等受影响 ID。
    fn apply_table_ids(
        &mut self,
        metadata: &dyn MetadataReader,
        schema_id: i64,
        new_table_id: i64,
        old_table_id: i64,
    ) -> Result<Vec<i64>, String> {
        let mut affected = Vec::new();
        if old_table_id > 0 && old_table_id != new_table_id {
            affected.extend(self.applyDropTable(schema_id, old_table_id));
        }
        let mut table_info = metadata
            .table(schema_id, new_table_id)?
            .ok_or_else(|| format!("table {schema_id}/{new_table_id} not found"))?;
        table_info.db_id = schema_id;
        // 规范化字符集/排序规则大小写，并处理历史 UTF8→UTF8MB4。
        ConvertCharsetCollateToLowerCaseIfNeed(&mut table_info);
        ConvertOldVersionUTF8ToUTF8MB4IfNeed(&mut table_info);
        let table = Table::new(table_info.clone());
        let db = self
            .databases
            .get_mut(&schema_id)
            .ok_or_else(|| format!("database {schema_id} not loaded"))?;
        db.tables.insert(new_table_id, table.clone());
        if self.enable_v2 {
            self.info_data.add(&db.info, table, self.schema_version);
        }
        affected.extend(appendAffectedIDs(Vec::new(), &table_info));
        Ok(affected)
    }
    /// 从库中移除表；v2 同步 remove。
    fn applyDropTable(&mut self, schema_id: i64, table_id: i64) -> Vec<i64> {
        let Some(db) = self.databases.get_mut(&schema_id) else {
            return Vec::new();
        };
        let Some(table) = db.tables.remove(&table_id) else {
            return Vec::new();
        };
        self.temporary_table_ids.remove(&table_id);
        self.info_data.removeTemporaryTable(table_id);
        if self.enable_v2 {
            self.info_data.remove(
                db.info.name.clone(),
                db.info.id,
                table.Meta().name.clone(),
                table_id,
                self.schema_version,
            );
        }
        appendAffectedIDs(Vec::new(), table.Meta())
    }
    /// 加载并缓存 Placement Policy（创建与修改共用）。
    fn applyCreatePolicy(
        &mut self,
        metadata: &dyn MetadataReader,
        policy_id: i64,
    ) -> Result<(), String> {
        let policy = metadata
            .policy(policy_id)?
            .ok_or_else(|| format!("policy {policy_id} not found"))?;
        self.policies.insert(policy.id, policy);
        Ok(())
    }
    /// 加载并缓存 Resource Group。
    fn applyResourceGroup(
        &mut self,
        metadata: &dyn MetadataReader,
        group_id: i64,
    ) -> Result<(), String> {
        let group = metadata
            .resource_group(group_id)?
            .ok_or_else(|| format!("resource group {group_id} not found"))?;
        self.resource_groups.insert(group.id, group);
        Ok(())
    }
    /// Go 侧会扫描引用该 policy 的表；此处暂返回空以保持接口。
    fn tables_referencing_policy(&self, _policy_id: i64) -> Vec<i64> {
        Vec::new()
    }
    /// 登记临时表 ID。
    pub fn addTemporaryTable(&mut self, table_id: i64) {
        self.temporary_table_ids.insert(table_id);
        self.info_data.addTemporaryTable(table_id);
    }
    /// 批量注入 policy 与 resource group。
    pub fn initMisc(&mut self, policies: Vec<PolicyInfo>, resource_groups: Vec<ResourceGroupInfo>) {
        self.policies
            .extend(policies.into_iter().map(|policy| (policy.id, policy)));
        self.resource_groups
            .extend(resource_groups.into_iter().map(|group| (group.id, group)));
    }

    /// 用完整 DBInfo 列表全量初始化；已加载表会从 TableName2ID 按原名移除。
    pub fn InitWithDBInfos(
        &mut self,
        db_infos: &mut [DBInfo],
        policies: Vec<PolicyInfo>,
        resource_groups: Vec<ResourceGroupInfo>,
        schema_version: i64,
    ) {
        self.schema_version = schema_version;
        self.databases.clear();
        if self.enable_v2 {
            self.info_data.resetBeforeFullLoad(schema_version);
        }
        for db in db_infos.iter_mut() {
            let tables = std::mem::take(&mut db.tables);
            // Go 按 Name.O（原始大小写）从 TableName2ID 删除已加载表，保留未加载项供惰性加载。
            // Go deletes loaded tables from TableName2ID by Name.O (original case).
            if !db.table_name_2_id.is_empty() {
                for table in &tables {
                    db.table_name_2_id.remove(&table.name.original);
                }
            }
            let mut state = DatabaseState {
                info: db.clone(),
                tables: HashMap::new(),
            };
            if self.enable_v2 {
                self.info_data.addDB(schema_version, db.clone());
            }
            for table in tables {
                let table = Table(table);
                if self.enable_v2 {
                    self.info_data.add(db, table.clone(), schema_version);
                }
                state.tables.insert(table.Meta().id, table);
            }
            self.databases.insert(db.id, state);
        }
        self.initMisc(policies, resource_groups);
    }

    /// 从已有 InfoSchema 拷贝库表到 Builder，继承其 schema 版本。
    pub fn InitWithOldInfoSchema(&mut self, old: &dyn InfoSchema) {
        let schemas = old.AllSchemas();
        self.databases.clear();
        for db in schemas {
            let tables = db
                .tables
                .iter()
                .cloned()
                .map(Table)
                .map(|table| (table.Meta().id, table))
                .collect();
            self.databases.insert(
                db.id,
                DatabaseState {
                    info: (*db).clone(),
                    tables,
                },
            );
        }
        self.schema_version = old.SchemaMetaVersion();
    }

    /// 消费 Builder，产出 v2 `infoschemaV2` 或 v1 `infoSchema`。
    pub fn Build(mut self, schema_ts: u64) -> Arc<dyn InfoSchema> {
        self.schema_ts = schema_ts;
        if self.enable_v2 {
            return Arc::new(infoschemaV2::new(
                self.info_data,
                self.schema_version,
                schema_ts,
            ));
        }
        let mut schema = infoSchema::new(self.schema_version);
        schema.set_temporary_table_ids(self.temporary_table_ids.clone());
        for (_, state) in self.databases {
            schema.add_schema(state.info, state.tables.into_values().collect());
        }
        for (_, policy) in self.policies {
            schema.setPolicy(policy);
        }
        for (_, group) in self.resource_groups {
            schema.setResourceGroup(group);
        }
        Arc::new(schema)
    }
}

/// 将表 ID 及其分区定义 ID 追加到受影响列表。
pub fn appendAffectedIDs(mut affected: Vec<i64>, table: &TableInfo) -> Vec<i64> {
    affected.push(table.id);
    if let Some(partitions) = &table.partition {
        affected.extend(partitions.definitions.iter().map(|partition| partition.id));
    }
    affected
}
/// 将表/列名的 lower 字段同步为 original 的小写形式。
pub fn ConvertCharsetCollateToLowerCaseIfNeed(table: &mut TableInfo) {
    table.name.lower = table.name.original.to_lowercase();
    for column in &mut table.columns {
        column.name.lower = column.name.original.to_lowercase();
    }
}
/// 历史 UTF8 升级占位；完整逻辑待与 Go 对齐后接入。
pub fn ConvertOldVersionUTF8ToUTF8MB4IfNeed(_table: &mut TableInfo) {}
/// 比较两个可选 Placement Policy ID 是否相同。
pub fn equalPlacementPolicy(left: Option<i64>, right: Option<i64>) -> bool {
    left == right
}
/// 按表 ID 映射到 infoschema 分桶下标。
pub fn tableBucketIdx(table_id: i64) -> usize {
    assert!(table_id > 0);
    table_id as usize % crate::infoschema::bucketCount
}
/// 表 ID 是否为正数（有效）。
pub fn tableIDIsValid(table_id: i64) -> bool {
    table_id > 0
}

/// 从 TableInfo 构造虚拟表 Table 的函数类型。
pub type tableFromMetaFunc = fn(TableInfo) -> Result<Table, String>;
#[derive(Clone)]
/// 虚拟表驱动：绑定系统库信息与 TableFromMeta 转换函数。
pub struct virtualTableDriver {
    pub DBInfo: DBInfo,
    pub TableFromMeta: tableFromMetaFunc,
}
static DRIVERS: OnceLock<Mutex<Vec<virtualTableDriver>>> = OnceLock::new();
/// 进程内全局虚拟表驱动列表。
fn virtual_table_drivers() -> &'static Mutex<Vec<virtualTableDriver>> {
    DRIVERS.get_or_init(|| Mutex::new(Vec::new()))
}
/// 注册虚拟表驱动，供 information_schema 等系统表使用。
pub fn RegisterVirtualTable(db: DBInfo, convert: tableFromMetaFunc) {
    virtual_table_drivers()
        .lock()
        .expect("virtual table driver lock poisoned")
        .push(virtualTableDriver {
            DBInfo: db,
            TableFromMeta: convert,
        });
}
/// 设置 Data 缓存容量并创建 Builder。
pub fn NewBuilder(schema_cache_size: u64, info_data: Arc<Data>, use_v2: bool) -> Builder {
    info_data.SetCacheCapacity(schema_cache_size);
    Builder::new(info_data, use_v2)
}

/// 从已有 v1 infoSchema 拷贝 masking policy 缓存，对齐 Go InitWithOldInfoSchema 语义。
/// Copy masking-policy cache from an existing v1 `infoSchema`, matching Go
/// `InitWithOldInfoSchema` masking semantics.
pub fn apply_masking_copy(
    target: &crate::infoschema::infoSchema,
    old: &crate::infoschema::infoSchema,
    load_in_progress: bool,
) {
    let policies = old.clone_masking_policies();
    let mut loaded = old.masking_policies_loaded();
    if load_in_progress {
        loaded = false;
    }
    target.restore_masking_policies(policies, loaded);
}

/// 按 SchemaDiff 过滤仍有效的自动分配器（AutoID / AutoRandom 等）。
///
/// 对应 Go `getKeptAllocators`：
/// - RebaseAutoID / ModifyTableAutoIDCache → 丢弃 RowID 与 AutoIncrement；
/// - RebaseAutoRandomBase → 丢弃 AutoRandom；
/// - MultiSchemaChange 检查 `sub_action_types`；
/// - 其余动作保留全部。
/// Keep allocators that were not invalidated by the current schema diff.
///
/// Mirrors Go `getKeptAllocators`:
/// - RebaseAutoID / ModifyTableAutoIDCache → drop RowID + AutoIncrement;
/// - RebaseAutoRandomBase → drop AutoRandom;
/// - MultiSchemaChange inspects `sub_action_types` the same way;
/// - otherwise keep all.
pub fn getKeptAllocators(
    diff: &SchemaDiff,
    old: &astersql_meta_autoid::Allocators,
) -> astersql_meta_autoid::Allocators {
    use astersql_meta_autoid::AllocatorType;

    // 扫描主动作与子动作，标记哪些分配器类型已失效。
    let mut auto_id_changed = false;
    let mut auto_random_changed = false;
    match diff.action_type {
        ActionType::RebaseAutoID | ActionType::ModifyTableAutoIDCache => {
            auto_id_changed = true;
        }
        ActionType::RebaseAutoRandomBase => {
            auto_random_changed = true;
        }
        ActionType::MultiSchemaChange => {
            for sub in &diff.sub_action_types {
                match sub {
                    ActionType::RebaseAutoID | ActionType::ModifyTableAutoIDCache => {
                        auto_id_changed = true;
                    }
                    ActionType::RebaseAutoRandomBase => {
                        auto_random_changed = true;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if auto_id_changed {
        old.filter(|a| {
            let tp = a.get_type();
            tp != AllocatorType::RowId && tp != AllocatorType::AutoIncrement
        })
    } else if auto_random_changed {
        old.filter(|a| a.get_type() != AllocatorType::AutoRandom)
    } else {
        old.clone()
    }
}
