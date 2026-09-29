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

// 本文件对照 pkg/meta/meta.go 实现元数据键布局、事务读写、遍历和序列化流程。
// kv、structure、model 等依赖由 package 模块接入；聚焦 harness 使用共享内存后端验证相同事务语义。

// TiDB 元数据（meta）核心：键布局、Mutator 事务读写、库表/策略 CRUD 与序列化。
//
// 元数据存放在 KV 的 `m` 前缀下，用类似 Redis 的 string/hash 结构组织：
// - 全局 ID、Schema 版本、Bootstrap 等为 string 键；
// - 每个数据库为 `DB:<id>` hash，field 存表定义与 AutoID；
// - Placement / Masking / ResourceGroup 等策略另有独立 hash。
// Mutator 在一个 kv.Transaction 内完成读写；start_ts 为事务开始时间戳（MVCC 版本）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::meta_autoid::{AutoIdAccessors, AutoIdAccessorsImpl, new_auto_id_accessors};
use crate::{
    ast, bytes, codec, context, errors, helper, json, kerneltype, kv, metadef, metrics, model,
    mysql, partialjson, resourcegroup, rmpb, runtime, schstatus, structure, time,
};

// Go 的三个包级互斥量分别串行化不同 ID 空间，避免同一事务结构上的读改写竞争。
/// 串行化全局 ID 分配的互斥锁。
static GLOBAL_ID_MUTEX: Mutex<()> = Mutex::new(());
/// 串行化 Placement Policy ID 分配。
static POLICY_ID_MUTEX: Mutex<()> = Mutex::new(());
/// 串行化 Masking Policy ID 分配。
static MASKING_POLICY_ID_MUTEX: Mutex<()> = Mutex::new(());

/// meta 键统一前缀字节 `m`。
pub(crate) const META_PREFIX: &[u8] = b"m";
const NEXT_GLOBAL_ID_KEY: &[u8] = b"NextGlobalID";
const SCHEMA_VERSION_KEY: &[u8] = b"SchemaVersionKey";
const DBS: &[u8] = b"DBs";
const DB_PREFIX: &str = "DB";
const TABLE_PREFIX: &str = "Table";
const SEQUENCE_PREFIX: &str = "SID";
const SEQUENCE_CYCLE_PREFIX: &str = "SequenceCycle";
const TABLE_ID_PREFIX: &str = "TID";
const INC_ID_PREFIX: &str = "IID";
const RANDOM_ID_PREFIX: &str = "TARID";
const BOOTSTRAP_KEY: &[u8] = b"BootstrapKey";
const STARTER_BOOTSTRAP_KEY: &[u8] = b"StarterBootstrapKey";
const SCHEMA_DIFF_PREFIX: &str = "Diff";
const POLICIES: &[u8] = b"Policies";
const POLICY_PREFIX: &str = "Policy";
const MASKING_POLICIES: &[u8] = b"MaskingPolicies";
const MASKING_POLICY_PREFIX: &str = "MaskingPolicy";
const RESOURCE_GROUPS: &[u8] = b"ResourceGroups";
const RESOURCE_GROUP_PREFIX: &str = "RG";
const POLICY_GLOBAL_ID: &[u8] = b"PolicyGlobalID";
const MASKING_POLICY_GLOBAL_ID: &[u8] = b"MaskingPolicyGlobalID";
const DDL_TABLE_VERSION_KEY: &[u8] = b"DDLTableVersion";
const BOOT_TABLE_VERSION_KEY: &[u8] = b"BootTableVersion";
const BDR_ROLE_KEY: &[u8] = b"BDRRole";
const METADATA_LOCK_KEY: &[u8] = b"metadataLock";
const SCHEMA_CACHE_SIZE_KEY: &[u8] = b"SchemaCacheSize";
const REQUEST_UNIT_STATS_KEY: &[u8] = b"RequestUnitStats";
const INGEST_MAX_BATCH_SPLIT_RANGES_KEY: &[u8] = b"IngestMaxBatchSplitRanges";
const INGEST_MAX_SPLIT_RANGES_PER_SEC_KEY: &[u8] = b"IngestMaxSplitRangesPerSec";
const INGEST_MAX_INFLIGHT_KEY: &[u8] = b"IngestMaxInflight";
const INGEST_MAX_PER_SEC_KEY: &[u8] = b"IngestMaxReqPerSec";
const DXF_SCHEDULE_TUNE_KEY: &[u8] = b"DXFScheduleTune";
const DDL_JOB_HISTORY_KEY: &[u8] = b"DDLJobHistory";

/// 当前 meta 序列化魔数版本字节。
pub const CURRENT_MAGIC_BYTE_VER: u8 = 0x00;
const TYPE_UNKNOWN: i32 = 0;
const TYPE_JSON: i32 = 1;
const DEFAULT_GROUP_ID: i64 = 1;

// NextGenBootTableVersion 对应 nextgen 系统表 bootstrap 版本。
/// nextgen 系统表 bootstrap 版本枚举。
#[repr(i32)]
pub enum NextGenBootTableVersion {
    Init = 0,
    Base = 1,
    MaskingPolicy = 2,
    StorageClassTransition = 3,
    MaterializedView = 4,
}

// DDLTableVersion 记录并发 DDL、MDL、分布式回填和 notifier 表的演进阶段。
/// DDL 相关系统表演进版本（含 MDL、回填、notifier）。
#[repr(i32)]
pub enum DDLTableVersion {
    Init = 0,
    Base = 1,
    Mdl = 2,
    Backfill = 3,
    DdlNotifier = 4,
}

// Option 对应 Go 的 func(*Mutator)，在构造后按顺序修改事务元数据访问器。
/// Mutator 构造选项：一次性闭包，按顺序应用到新建 Mutator。
pub type OptionFn = Box<dyn FnOnce(&mut Mutator)>;

// Mutator 在一个 kv.Transaction 内读写所有 meta 信息；start_ts 保留事务开始时间戳。
/// 元数据变更器：在单个事务内读写全部 meta。
pub struct Mutator {
    pub txn: structure::TxStructure,
    pub start_ts: u64,
}

// new_mutator 提升事务优先级、允许磁盘接近满载，并用固定 m 前缀创建 TxStructure。
/// 从 kv.Transaction 构造 Mutator，并应用选项。
pub fn new_mutator(mut txn: kv::Transaction, options: Vec<OptionFn>) -> Mutator {
    txn.set_option(kv::Priority::High);
    txn.set_disk_full_option(kv::DiskFullOption::AllowedOnAlmostFull);
    let start_ts = txn.start_ts();
    let mut mutator = Mutator {
        txn: structure::new_structure(txn, META_PREFIX),
        start_ts,
    };
    for option in options {
        option(&mut mutator);
    }
    mutator
}

// 下列键函数保持 Go 的可读十进制编码，解析时先严格校验前缀。
/// 编码数据库 hash 键：`DB:<db_id>`。
pub fn db_key(db_id: i64) -> Vec<u8> {
    format!("{DB_PREFIX}:{db_id}").into_bytes()
}
/// 判断是否为 DB 键前缀。
pub fn is_db_key(key: &[u8]) -> bool {
    key.starts_with(format!("{DB_PREFIX}:").as_bytes())
}
/// 从 DB 键解析 database id。
pub fn parse_db_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !is_db_key(key) {
        return Err(errors::new("fail to parse dbKey"));
    }
    parse_prefixed_id(key, DB_PREFIX)
}
/// 编码表 RowID field：`TID:<table_id>`。
pub fn auto_table_id_key(id: i64) -> Vec<u8> {
    format!("{TABLE_ID_PREFIX}:{id}").into_bytes()
}
/// 判断是否为 TID 键。
pub fn is_auto_table_id_key(key: &[u8]) -> bool {
    key.starts_with(format!("{TABLE_ID_PREFIX}:").as_bytes())
}
/// 从 TID 键解析 table id。
pub fn parse_auto_table_id_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !is_auto_table_id_key(key) {
        return Err(errors::new("fail to parse autoTableKey"));
    }
    parse_prefixed_id(key, TABLE_ID_PREFIX)
}
/// 编码独立 AUTO_INCREMENT field：`IID:<table_id>`。
pub fn auto_increment_id_key(id: i64) -> Vec<u8> {
    format!("{INC_ID_PREFIX}:{id}").into_bytes()
}
/// 判断是否为 IID 键。
pub fn is_auto_increment_id_key(key: &[u8]) -> bool {
    key.starts_with(format!("{INC_ID_PREFIX}:").as_bytes())
}
/// 从 IID 键解析 table id。
pub fn parse_auto_increment_id_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !is_auto_increment_id_key(key) {
        return Err(errors::new("fail to parse autoIncrementKey"));
    }
    parse_prefixed_id(key, INC_ID_PREFIX)
}
/// 编码 AUTO_RANDOM field：`TARID:<table_id>`。
pub fn auto_random_table_id_key(id: i64) -> Vec<u8> {
    format!("{RANDOM_ID_PREFIX}:{id}").into_bytes()
}
/// 判断是否为 TARID 键。
pub fn is_auto_random_table_id_key(key: &[u8]) -> bool {
    key.starts_with(format!("{RANDOM_ID_PREFIX}:").as_bytes())
}
/// 从 TARID 键解析 table id。
pub fn parse_auto_random_table_id_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !is_auto_random_table_id_key(key) {
        return Err(errors::new("fail to parse AutoRandomTableIDKey"));
    }
    parse_prefixed_id(key, RANDOM_ID_PREFIX)
}
/// 编码表定义 field：`Table:<table_id>`。
pub fn table_key(id: i64) -> Vec<u8> {
    format!("{TABLE_PREFIX}:{id}").into_bytes()
}
/// 判断是否为 Table 键。
pub fn is_table_key(key: &[u8]) -> bool {
    key.starts_with(format!("{TABLE_PREFIX}:").as_bytes())
}
/// 从 Table 键解析 table id。
pub fn parse_table_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !key.starts_with(TABLE_PREFIX.as_bytes()) {
        return Err(errors::new("fail to parse tableKey"));
    }
    parse_prefixed_id(key, TABLE_PREFIX)
}
/// 编码 Sequence 值 field：`SID:<table_id>`。
pub fn sequence_key(id: i64) -> Vec<u8> {
    format!("{SEQUENCE_PREFIX}:{id}").into_bytes()
}
/// 判断是否为 SID 键。
pub fn is_sequence_key(key: &[u8]) -> bool {
    key.starts_with(format!("{SEQUENCE_PREFIX}:").as_bytes())
}
/// 从 SID 键解析 table id。
pub fn parse_sequence_key(key: &[u8]) -> Result<i64, errors::Error> {
    if !is_sequence_key(key) {
        return Err(errors::new("fail to parse sequence key"));
    }
    parse_prefixed_id(key, SEQUENCE_PREFIX)
}
/// 去掉 `prefix:` 后解析十进制 id。
fn parse_prefixed_id(key: &[u8], prefix: &str) -> Result<i64, errors::Error> {
    let value = std::str::from_utf8(key).map_err(errors::trace)?;
    value
        .trim_start_matches(&format!("{prefix}:"))
        .parse()
        .map_err(errors::trace)
}

impl Mutator {
    fn policy_key(&self, id: i64) -> Vec<u8> {
        format!("{POLICY_PREFIX}:{id}").into_bytes()
    }
    fn masking_policy_key(&self, id: i64) -> Vec<u8> {
        format!("{MASKING_POLICY_PREFIX}:{id}").into_bytes()
    }
    fn resource_group_key(&self, id: i64) -> Vec<u8> {
        format!("{RESOURCE_GROUP_PREFIX}:{id}").into_bytes()
    }
    fn sequence_cycle_key(&self, id: i64) -> Vec<u8> {
        format!("{SEQUENCE_CYCLE_PREFIX}:{id}").into_bytes()
    }
    fn schema_diff_key(&self, version: i64) -> Vec<u8> {
        format!("{SCHEMA_DIFF_PREFIX}:{version}").into_bytes()
    }

    // gen_global_id 在全局锁内原子自增，并拒绝越过用户全局 ID 上限。
    pub fn gen_global_id(&mut self) -> Result<i64, errors::Error> {
        let _guard = GLOBAL_ID_MUTEX.lock().unwrap();
        let id = self.txn.inc(NEXT_GLOBAL_ID_KEY, 1)?;
        if id > metadef::MAX_USER_GLOBAL_ID {
            return Err(errors::new(format!(
                "global id:{id} exceeds the limit:{}",
                metadef::MAX_USER_GLOBAL_ID
            )));
        }
        Ok(id)
    }
    // advance_global_ids 返回自增前的旧 ID，而 gen_global_ids 展开新分配的连续区间。
    pub fn advance_global_ids(&mut self, n: i32) -> Result<i64, errors::Error> {
        let _guard = GLOBAL_ID_MUTEX.lock().unwrap();
        let id = self.txn.inc(NEXT_GLOBAL_ID_KEY, n as i64)?;
        if id > metadef::MAX_USER_GLOBAL_ID {
            return Err(errors::new("global id exceeds limit"));
        }
        Ok(id - n as i64)
    }
    pub fn gen_global_ids(&mut self, n: i32) -> Result<Vec<i64>, errors::Error> {
        let old = self.advance_global_ids(n)?;
        Ok((old + 1..=old + n as i64).collect())
    }
    pub fn global_id_key(&self) -> kv::Key {
        self.txn.encode_string_data_key(NEXT_GLOBAL_ID_KEY)
    }
    pub fn gen_placement_policy_id(&mut self) -> Result<i64, errors::Error> {
        let _guard = POLICY_ID_MUTEX.lock().unwrap();
        self.txn.inc(POLICY_GLOBAL_ID, 1)
    }
    pub fn gen_masking_policy_id(&mut self) -> Result<i64, errors::Error> {
        let _guard = MASKING_POLICY_ID_MUTEX.lock().unwrap();
        self.txn.inc(MASKING_POLICY_GLOBAL_ID, 1)
    }
    pub fn get_global_id(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(NEXT_GLOBAL_ID_KEY)
    }
    pub fn get_policy_id(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(POLICY_GLOBAL_ID)
    }
    pub fn get_masking_policy_id(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(MASKING_POLICY_GLOBAL_ID)
    }

    // gen_auto_table_id_key_value 保留 structure 层对 hash/field/value 的专用编码。
    pub fn gen_auto_table_id_key_value(
        &self,
        db_id: i64,
        table_id: i64,
        auto_id: i64,
    ) -> (Vec<u8>, Vec<u8>) {
        self.txn.encode_hash_auto_id_key_value(
            &db_key(db_id),
            &auto_table_id_key(table_id),
            auto_id,
        )
    }
    pub fn get_auto_id_accessors(&self, db_id: i64, table_id: i64) -> AutoIdAccessorsImpl<'_> {
        new_auto_id_accessors(self, db_id, table_id)
    }

    // schema 版本读写直接使用字符串整数键；diff 尚未写入时对外退回上一版本以避免 stale read 不一致。
    pub fn get_schema_version_with_non_empty_diff(&self) -> Result<i64, errors::Error> {
        let mut version = self.txn.get_i64(SCHEMA_VERSION_KEY)?;
        if self.get_schema_diff(version)?.is_none() && version > 0 {
            version -= 1;
        }
        Ok(version)
    }
    pub fn encode_schema_diff_key(&self, version: i64) -> kv::Key {
        self.txn
            .encode_string_data_key(&self.schema_diff_key(version))
    }
    pub fn get_schema_version(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(SCHEMA_VERSION_KEY)
    }
    pub fn gen_schema_version(&mut self) -> Result<i64, errors::Error> {
        self.txn.inc(SCHEMA_VERSION_KEY, 1)
    }
    pub fn gen_schema_versions(&mut self, count: i64) -> Result<i64, errors::Error> {
        self.txn.inc(SCHEMA_VERSION_KEY, count)
    }
}

impl Mutator {
    // iter_databases/iter_tables 使用 structure 的流式 hash 迭代，避免一次性加载大量 JSON 导致 OOM。
    pub fn iter_databases<F>(&self, mut visit: F) -> Result<(), errors::Error>
    where
        F: FnMut(model::DbInfo) -> Result<(), errors::Error>,
    {
        self.txn
            .hget_iter(DBS, |pair| visit(json::unmarshal(&pair.value)?))
    }
    pub fn iter_tables<F>(&self, db_id: i64, mut visit: F) -> Result<(), errors::Error>
    where
        F: FnMut(model::TableInfo) -> Result<(), errors::Error>,
    {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        self.txn.hget_iter(&db, |pair| {
            if !pair.field.starts_with(TABLE_PREFIX.as_bytes()) {
                return Ok(());
            }
            let mut table: model::TableInfo = json::unmarshal(&pair.value)?;
            table.db_id = db_id;
            visit(table)
        })
    }
    pub fn get_metas_by_db_id(
        &self,
        db_id: i64,
    ) -> Result<Vec<structure::HashPair>, errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        self.txn.hget_all(&db)
    }
    pub fn list_tables(
        &self,
        ctx: &context::Context,
        db_id: i64,
    ) -> Result<Vec<model::TableInfo>, errors::Error> {
        let mut tables = Vec::new();
        for pair in self.get_metas_by_db_id(db_id)? {
            if !pair.field.starts_with(TABLE_PREFIX.as_bytes()) {
                continue;
            }
            // 长列表遍历每轮检查取消，避免已取消请求仍完成整库反序列化。
            ctx.check_error()?;
            let mut table: model::TableInfo = json::unmarshal(&pair.value)?;
            table.db_id = db_id;
            tables.push(table);
        }
        Ok(tables)
    }
    pub fn list_simple_tables(
        &self,
        db_id: i64,
    ) -> Result<Vec<model::TableNameInfo>, errors::Error> {
        self.get_metas_by_db_id(db_id)?
            .into_iter()
            .filter(|p| p.field.starts_with(TABLE_PREFIX.as_bytes()))
            .map(|p| fast_unmarshal_table_name_info(&p.value))
            .collect()
    }
    pub fn list_databases(&self) -> Result<Vec<model::DbInfo>, errors::Error> {
        self.txn
            .hget_all(DBS)?
            .into_iter()
            .map(|p| json::unmarshal(&p.value))
            .collect()
    }
    pub fn get_database(&self, id: i64) -> Result<Option<model::DbInfo>, errors::Error> {
        self.txn
            .hget(DBS, &db_key(id))?
            .map(|v| json::unmarshal(&v))
            .transpose()
    }
    pub fn get_table(
        &self,
        db_id: i64,
        table_id: i64,
    ) -> Result<Option<model::TableInfo>, errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        self.txn
            .hget(&db, &table_key(table_id))?
            .map(|v| {
                let mut table: model::TableInfo = json::unmarshal(&v)?;
                table.db_id = db_id;
                Ok(table)
            })
            .transpose()
    }
    pub fn check_table_exists_public(
        &self,
        db_id: i64,
        table_id: i64,
    ) -> Result<bool, errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        Ok(self.txn.hget(&db, &table_key(table_id))?.is_some())
    }

    // list/get policy 类方法先剥离 magic byte，再反序列化；default resource group 在缺失时以内建值补齐。
    pub fn list_policies(&self) -> Result<Vec<model::PolicyInfo>, errors::Error> {
        self.txn
            .hget_all(POLICIES)?
            .into_iter()
            .map(|p| json::unmarshal(detach_magic_byte(&p.value)?))
            .collect()
    }
    pub fn list_masking_policies(&self) -> Result<Vec<model::MaskingPolicyInfo>, errors::Error> {
        self.txn
            .hget_all(MASKING_POLICIES)?
            .into_iter()
            .map(|p| json::unmarshal(detach_magic_byte(&p.value)?))
            .collect()
    }
    pub fn get_policy(&self, id: i64) -> Result<model::PolicyInfo, errors::Error> {
        let value =
            self.require_hash_value(POLICIES, &self.policy_key(id), "policy doesn't exist")?;
        json::unmarshal(detach_magic_byte(&value)?)
    }
    pub fn get_masking_policy(&self, id: i64) -> Result<model::MaskingPolicyInfo, errors::Error> {
        let value = self.require_hash_value(
            MASKING_POLICIES,
            &self.masking_policy_key(id),
            "masking policy doesn't exist",
        )?;
        json::unmarshal(detach_magic_byte(&value)?)
    }
    pub fn list_resource_groups(&self) -> Result<Vec<model::ResourceGroupInfo>, errors::Error> {
        let mut groups: Vec<model::ResourceGroupInfo> = self
            .txn
            .hget_all(RESOURCE_GROUPS)?
            .into_iter()
            .map(|p| json::unmarshal(detach_magic_byte(&p.value)?))
            .collect::<Result<_, _>>()?;
        if !groups
            .iter()
            .any(|g| g.name.lower == resourcegroup::DEFAULT_RESOURCE_GROUP_NAME)
        {
            groups.push(default_group_meta());
        }
        Ok(groups)
    }
    pub fn get_resource_group(&self, id: i64) -> Result<model::ResourceGroupInfo, errors::Error> {
        match self
            .txn
            .hget(RESOURCE_GROUPS, &self.resource_group_key(id))?
        {
            Some(value) => json::unmarshal(detach_magic_byte(&value)?),
            None if id == DEFAULT_GROUP_ID => Ok(default_group_meta()),
            None => Err(errors::new("resource group doesn't exist")),
        }
    }
}

// split_range_int64_max 把十进制 DB hash key 空间均分，首段从非补零的 "0" 开始。
pub fn split_range_int64_max(n: i64) -> Vec<(String, String)> {
    let batch = 9_999_999_999_999_999_999u64 / n as u64;
    (0..n)
        .map(|i| {
            let start = if i == 0 {
                "0".to_owned()
            } else {
                format!("{:019}", batch * i as u64)
            };
            (start, format!("{:019}", batch * (i + 1) as u64))
        })
        .collect()
}

// iter_all_tables 最多使用 15 个快照任务扫描 DB key 范围；回调由互斥量串行化，与 Go 行为一致。
pub async fn iter_all_tables<F>(
    ctx: context::Context,
    store: kv::Storage,
    start_ts: u64,
    concurrency: i32,
    visit: F,
) -> Result<(), errors::Error>
where
    F: Fn(model::TableInfo) -> Result<(), errors::Error> + Send + 'static,
{
    let concurrency = concurrency.clamp(1, 15);
    let ranges = split_range_int64_max(concurrency as i64);
    let visit = Arc::new(Mutex::new(visit));
    let mut tasks = Vec::new();
    for (start, end) in ranges {
        let snapshot = store.get_snapshot(start_ts);
        let ctx = ctx.clone();
        let visit = visit.clone();
        tasks.push(runtime::spawn(async move {
            let structure = structure::new_snapshot_structure(snapshot, META_PREFIX);
            structure.iterate_hash_bounded(
                db_scan_key(&start),
                db_scan_key(&end),
                |db, field, value| {
                    ctx.check_error()?;
                    if !field.starts_with(TABLE_PREFIX.as_bytes()) {
                        return Ok(());
                    }
                    let mut table: model::TableInfo = json::unmarshal(value)?;
                    table.db_id = parse_db_key(db)?;
                    visit.lock().unwrap()(table)
                },
            )
        }));
    }
    runtime::try_join_all(tasks).await.map(|_| ())
}

fn db_scan_key(bound: &str) -> Vec<u8> {
    codec::encode_bytes(format!("{DB_PREFIX}:").as_bytes(), bound.as_bytes())
}

const CHECK_FOREIGN_KEY_ATTRIBUTES_NIL: &str = r#""fk_info":null"#;
const CHECK_FOREIGN_KEY_ATTRIBUTES_ZERO: &str = r#""fk_info":[]"#;

// MustLoadFilterAttr 描述 JSON marker 缺失或出现时是否必须完整加载 TableInfo。
pub struct MustLoadFilterAttr {
    pub attr: &'static str,
    pub load_if_missing: bool,
}
pub const CHECK_ATTRIBUTES_IN_ORDER: &[MustLoadFilterAttr] = &[
    MustLoadFilterAttr {
        attr: r#""partition":null"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""Lock":null"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""tiflash_replica":null"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""temp_table_type":0"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""policy_ref_info":null"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""ttl_info":null"#,
        load_if_missing: true,
    },
    MustLoadFilterAttr {
        attr: r#""affinity":{"#,
        load_if_missing: false,
    },
];

// is_table_info_must_load 利用序列化字段顺序逐步缩小搜索切片，避免完整 JSON 解码。
fn is_table_info_must_load(
    mut data: &[u8],
    check_foreign_key: bool,
    filters: &[MustLoadFilterAttr],
) -> bool {
    if check_foreign_key {
        let index = bytes::index(data, CHECK_FOREIGN_KEY_ATTRIBUTES_NIL.as_bytes())
            .or_else(|| bytes::index(data, CHECK_FOREIGN_KEY_ATTRIBUTES_ZERO.as_bytes()));
        let Some(index) = index else { return true };
        data = &data[index..];
    }
    for filter in filters {
        match bytes::index(data, filter.attr.as_bytes()) {
            None if filter.load_if_missing => return true,
            None => continue,
            Some(_) if !filter.load_if_missing => return true,
            Some(index) => data = &data[index..],
        }
    }
    false
}
pub fn is_table_info_must_load_public(data: &[u8]) -> bool {
    is_table_info_must_load(data, true, CHECK_ATTRIBUTES_IN_ORDER)
}
pub const NAME_EXTRACT_REGEXP: &str = r#""O":"([^"\\]*(?:\\.[^"\\]*)*)","#;
pub fn unescape(value: &str) -> String {
    value.replace(r#"\""#, "\"").replace(r#"\\"#, r#"\"#)
}

// get_all_name_to_id_and_must_loaded_table_info 快速提取名称/ID，只对特殊属性表反序列化完整对象。
pub fn get_all_name_to_id_and_must_loaded_table_info(
    mutator: &Mutator,
    db_id: i64,
) -> Result<(HashMap<String, i64>, Vec<model::TableInfo>), errors::Error> {
    let db = db_key(db_id);
    mutator.check_db_exists(&db)?;
    let mut names = HashMap::new();
    let mut tables = Vec::new();
    mutator.txn.iterate_hash(&db, |field, value| {
        if !field.starts_with(TABLE_PREFIX.as_bytes()) {
            return Ok(());
        }
        let (id, name) = partialjson::extract_id_and_original_name(value, NAME_EXTRACT_REGEXP)?;
        names.insert(unescape(&name), id);
        if is_table_info_must_load(value, true, CHECK_ATTRIBUTES_IN_ORDER) {
            let mut table: model::TableInfo = json::unmarshal(value)?;
            table.db_id = db_id;
            tables.push(table);
        }
        Ok(())
    })?;
    Ok((names, tables))
}

pub fn get_table_info_with_attributes(
    mutator: &Mutator,
    db_id: i64,
    filters: &[MustLoadFilterAttr],
) -> Result<Vec<model::TableInfo>, errors::Error> {
    let db = db_key(db_id);
    mutator.check_db_exists(&db)?;
    let mut tables = Vec::new();
    mutator.txn.iterate_hash(&db, |field, value| {
        if field.starts_with(TABLE_PREFIX.as_bytes())
            && is_table_info_must_load(value, false, filters)
        {
            let mut table: model::TableInfo = json::unmarshal(value)?;
            table.db_id = db_id;
            tables.push(table);
        }
        Ok(())
    })?;
    Ok(tables)
}

// fast_unmarshal_table_name_info 只读取顶层 id/name 字段，避免 ListSimpleTables 解码完整 TableInfo。
pub fn fast_unmarshal_table_name_info(data: &[u8]) -> Result<model::TableNameInfo, errors::Error> {
    let members = partialjson::extract_top_level_members(data, &["id", "name"])?;
    let id = members.single_i64("id")?;
    let name = members.case_insensitive_original_name("name")?;
    Ok(model::TableNameInfo {
        id,
        name: ast::CiString::new(name),
    })
}

fn default_group_meta() -> model::ResourceGroupInfo {
    model::ResourceGroupInfo::public_default(
        DEFAULT_GROUP_ID,
        resourcegroup::DEFAULT_RESOURCE_GROUP_NAME,
        i32::MAX,
        -1,
        ast::MEDIUM_PRIORITY_VALUE,
    )
}

pub fn default_group_meta_for_test() -> model::ResourceGroupInfo {
    default_group_meta()
}

// magic byte 的 0x00..0x3f 区间归 JSON handler；当前只接受版本 0。
fn attach_magic_byte(data: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 1);
    out.push(CURRENT_MAGIC_BYTE_VER);
    out.extend(data);
    out
}
fn detach_magic_byte(value: &[u8]) -> Result<&[u8], errors::Error> {
    let (&magic, data) = value
        .split_first()
        .ok_or_else(|| errors::new("empty magic value"))?;
    if which_magic_type(magic) != TYPE_JSON {
        return Err(errors::new("unknown magic type handling module"));
    }
    if magic != CURRENT_MAGIC_BYTE_VER {
        return Err(errors::new("incompatible magic type handling module"));
    }
    Ok(data)
}
fn which_magic_type(byte: u8) -> i32 {
    if byte <= 0x3f {
        TYPE_JSON
    } else {
        TYPE_UNKNOWN
    }
}

impl Mutator {
    fn job_id_key(id: i64) -> [u8; 8] {
        id.to_be_bytes()
    }
    fn add_history_ddl_job(
        &mut self,
        key: &[u8],
        job: &model::Job,
        update_raw_args: bool,
    ) -> Result<(), errors::Error> {
        // Encode 是否更新 RawArgs 由调用方传入，编码成功后再原子写入 history hash。
        let encoded = job.encode(update_raw_args)?;
        self.txn.hset(key, &Self::job_id_key(job.id), &encoded)
    }
    pub fn add_history_ddl_job_public(
        &mut self,
        job: &model::Job,
        update_raw_args: bool,
    ) -> Result<(), errors::Error> {
        self.add_history_ddl_job(DDL_JOB_HISTORY_KEY, job, update_raw_args)
    }
    fn get_history_ddl_job_inner(
        &self,
        key: &[u8],
        id: i64,
    ) -> Result<Option<model::Job>, errors::Error> {
        self.txn
            .hget(key, &Self::job_id_key(id))?
            .map(|value| model::Job::decode(&value))
            .transpose()
    }
    pub fn get_history_ddl_job(&self, id: i64) -> Result<Option<model::Job>, errors::Error> {
        let started = time::now();
        let result = self.get_history_ddl_job_inner(DDL_JOB_HISTORY_KEY, id);
        metrics::META_HISTOGRAM.observe_get_history(result.as_ref(), time::since(started));
        result
    }
    pub fn get_history_ddl_count(&self) -> Result<u64, errors::Error> {
        self.txn.hget_len(DDL_JOB_HISTORY_KEY)
    }

    // ingest 配置使用共用的字符串数值读写辅助，None 对应 Go getter 的 isNull=true。
    fn set_numeric<T: ToString>(&mut self, key: &[u8], value: T) -> Result<(), errors::Error> {
        self.txn.set(key, value.to_string().as_bytes())
    }
    fn get_numeric<T: std::str::FromStr>(&self, key: &[u8]) -> Result<Option<T>, errors::Error>
    where
        errors::Error: From<T::Err>,
    {
        self.txn
            .get(key)?
            .map(|v| {
                std::str::from_utf8(&v)
                    .map_err(errors::from)
                    .and_then(|s| s.parse().map_err(errors::from))
            })
            .transpose()
    }
    pub fn set_ingest_max_batch_split_ranges(&mut self, value: i32) -> Result<(), errors::Error> {
        self.set_numeric(INGEST_MAX_BATCH_SPLIT_RANGES_KEY, value)
    }
    pub fn get_ingest_max_batch_split_ranges(&self) -> Result<Option<i32>, errors::Error> {
        self.get_numeric(INGEST_MAX_BATCH_SPLIT_RANGES_KEY)
    }
    pub fn set_ingest_max_split_ranges_per_sec(&mut self, value: f64) -> Result<(), errors::Error> {
        self.txn.set(
            INGEST_MAX_SPLIT_RANGES_PER_SEC_KEY,
            format!("{value:.2}").as_bytes(),
        )
    }
    pub fn get_ingest_max_split_ranges_per_sec(&self) -> Result<Option<f64>, errors::Error> {
        self.get_numeric(INGEST_MAX_SPLIT_RANGES_PER_SEC_KEY)
    }
    pub fn set_ingest_max_inflight(&mut self, value: i32) -> Result<(), errors::Error> {
        self.set_numeric(INGEST_MAX_INFLIGHT_KEY, value)
    }
    pub fn get_ingest_max_inflight(&self) -> Result<Option<i32>, errors::Error> {
        self.get_numeric(INGEST_MAX_INFLIGHT_KEY)
    }
    pub fn set_ingest_max_per_sec(&mut self, value: f64) -> Result<(), errors::Error> {
        self.txn
            .set(INGEST_MAX_PER_SEC_KEY, format!("{value:.2}").as_bytes())
    }
    pub fn get_ingest_max_per_sec(&self) -> Result<Option<f64>, errors::Error> {
        self.get_numeric(INGEST_MAX_PER_SEC_KEY)
    }

    // history iterator 以大端 job ID 字段倒序扫描，可选 schema/table 名过滤。
    pub fn get_last_history_ddl_jobs_iterator(&self) -> Result<HLastJobIterator, errors::Error> {
        Ok(HLastJobIterator::new(structure::new_hash_reverse_iter(
            &self.txn,
            DDL_JOB_HISTORY_KEY,
        )?))
    }
    pub fn get_last_history_ddl_jobs_iterator_with_filter(
        &self,
        schemas: HashSet<String>,
        tables: HashSet<String>,
    ) -> Result<HLastJobIterator, errors::Error> {
        Ok(HLastJobIterator {
            iter: structure::new_hash_reverse_iter(&self.txn, DDL_JOB_HISTORY_KEY)?,
            schema_names: schemas,
            table_names: tables,
        })
    }
    pub fn get_history_ddl_jobs_iterator(
        &self,
        start_job_id: i64,
    ) -> Result<HLastJobIterator, errors::Error> {
        Ok(HLastJobIterator::new(
            structure::new_hash_reverse_iter_from(
                &self.txn,
                DDL_JOB_HISTORY_KEY,
                &Self::job_id_key(start_job_id),
            )?,
        ))
    }

    // DXF tune factors 按 keyspace 分 field 存储 JSON，缺失时返回 None。
    pub fn set_dxf_schedule_tune_factors(
        &mut self,
        keyspace: &str,
        factors: &schstatus::TtlTuneFactors,
    ) -> Result<(), errors::Error> {
        self.txn.hset(
            DXF_SCHEDULE_TUNE_KEY,
            keyspace.as_bytes(),
            &json::marshal(factors)?,
        )
    }
    pub fn get_dxf_schedule_tune_factors(
        &self,
        keyspace: &str,
    ) -> Result<Option<schstatus::TtlTuneFactors>, errors::Error> {
        self.txn
            .hget(DXF_SCHEDULE_TUNE_KEY, keyspace.as_bytes())?
            .map(|v| json::unmarshal(&v))
            .transpose()
    }

    pub fn get_bootstrap_version(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(BOOTSTRAP_KEY)
    }
    pub fn finish_bootstrap(&mut self, version: i64) -> Result<(), errors::Error> {
        self.txn.set(BOOTSTRAP_KEY, version.to_string().as_bytes())
    }

    pub fn get_starter_bootstrap_version(&self) -> Result<i64, errors::Error> {
        self.txn.get_i64(STARTER_BOOTSTRAP_KEY)
    }
    pub fn finish_starter_bootstrap(&mut self, version: i64) -> Result<(), errors::Error> {
        self.txn
            .set(STARTER_BOOTSTRAP_KEY, version.to_string().as_bytes())
    }

    // schema diff 按版本独立存储；指标覆盖底层 get/set 的耗时与返回标签。
    pub fn get_schema_diff(
        &self,
        version: i64,
    ) -> Result<Option<model::SchemaDiff>, errors::Error> {
        let started = time::now();
        let value = self.txn.get(&self.schema_diff_key(version));
        metrics::META_HISTOGRAM.observe_get_schema_diff(value.as_ref(), time::since(started));
        value?
            .filter(|v| !v.is_empty())
            .map(|v| json::unmarshal(&v))
            .transpose()
    }
    pub fn set_schema_diff(&mut self, diff: &model::SchemaDiff) -> Result<(), errors::Error> {
        let data = json::marshal(diff)?;
        let started = time::now();
        let result = self.txn.set(&self.schema_diff_key(diff.version), &data);
        metrics::META_HISTOGRAM.observe_set_schema_diff(result.as_ref(), time::since(started));
        result
    }

    pub fn get_ru_stats(&self) -> Result<Option<RuStats>, errors::Error> {
        self.txn
            .get(REQUEST_UNIT_STATS_KEY)?
            .map(|v| json::unmarshal(&v))
            .transpose()
    }
    pub fn set_ru_stats(&mut self, stats: &RuStats) -> Result<(), errors::Error> {
        self.txn.set(REQUEST_UNIT_STATS_KEY, &json::marshal(stats)?)
    }
}

// LastJobIterator 对应 Go 接口，可复用调用方提供的 jobs 容量；Rust 返回新 Vec 保留相同结果语义。
pub trait LastJobIterator {
    fn get_last_jobs(&mut self, num: usize) -> Result<Vec<model::Job>, errors::Error>;
}

pub struct HLastJobIterator {
    pub iter: structure::ReverseHashIterator,
    pub schema_names: HashSet<String>,
    pub table_names: HashSet<String>,
}
impl HLastJobIterator {
    fn new(iter: structure::ReverseHashIterator) -> Self {
        Self {
            iter,
            schema_names: HashSet::new(),
            table_names: HashSet::new(),
        }
    }
}

// extract_schema_and_table_name_from_job 快速读取编码 Job 顶层的两个过滤字段。
pub fn extract_schema_and_table_name_from_job(
    data: &[u8],
) -> Result<(String, String), errors::Error> {
    let members = partialjson::extract_top_level_members(data, &["schema_name", "table_name"])?;
    Ok((
        members.single_string("schema_name")?,
        members.single_string("table_name")?,
    ))
}

pub fn is_job_match(
    job: &[u8],
    schemas: &HashSet<String>,
    tables: &HashSet<String>,
) -> Result<bool, errors::Error> {
    if schemas.is_empty() && tables.is_empty() {
        return Ok(true);
    }
    let (schema, table) = extract_schema_and_table_name_from_job(job)?;
    // 显式加括号表达 Go 原条件的优先级：schema 条件和 table 条件必须同时满足。
    Ok((schemas.is_empty() || schemas.contains(&schema))
        && (tables.is_empty() || tables.contains(&table)))
}

impl LastJobIterator for HLastJobIterator {
    fn get_last_jobs(&mut self, num: usize) -> Result<Vec<model::Job>, errors::Error> {
        let mut jobs = Vec::with_capacity(num);
        while self.iter.valid() && jobs.len() < num {
            let value = self.iter.value();
            if is_job_match(value, &self.schema_names, &self.table_names)? {
                jobs.push(model::Job::decode(value)?);
            }
            self.iter.next()?;
        }
        Ok(jobs)
    }
}

// ElementKeyType 的两种 5 字节前缀区分回填 column/index 元素。
pub const COLUMN_ELEMENT_KEY: &[u8; 5] = b"_col_";
pub const INDEX_ELEMENT_KEY: &[u8; 5] = b"_idx_";
const ELEMENT_KEY_LEN: usize = 5;

pub struct Element {
    pub id: i64,
    pub type_key: Vec<u8>,
}
impl std::fmt::Display for Element {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "ID:{},TypeKey:{}",
            self.id,
            String::from_utf8_lossy(&self.type_key)
        )
    }
}
impl Element {
    // encode_element 固定输出 5 字节类型前缀 + 8 字节大端 ID。
    pub fn encode_element(&self) -> Vec<u8> {
        let mut encoded = vec![0; ELEMENT_KEY_LEN + 8];
        let prefix_len = self.type_key.len().min(ELEMENT_KEY_LEN);
        encoded[..prefix_len].copy_from_slice(&self.type_key[..prefix_len]);
        encoded[ELEMENT_KEY_LEN..].copy_from_slice(&self.id.to_be_bytes());
        encoded
    }
}
pub fn decode_element(encoded: &[u8]) -> Result<Element, errors::Error> {
    if encoded.len() < ELEMENT_KEY_LEN + 8 {
        return Err(errors::new(format!(
            "invalid encoded element length {}",
            encoded.len()
        )));
    }
    let type_key = match &encoded[..ELEMENT_KEY_LEN] {
        value if value == INDEX_ELEMENT_KEY => INDEX_ELEMENT_KEY.to_vec(),
        value if value == COLUMN_ELEMENT_KEY => COLUMN_ELEMENT_KEY.to_vec(),
        value => {
            return Err(errors::new(format!(
                "invalid encoded element key prefix {value:?}"
            )));
        }
    };
    let id = i64::from_be_bytes(
        encoded[ELEMENT_KEY_LEN..ELEMENT_KEY_LEN + 8]
            .try_into()
            .unwrap(),
    );
    Ok(Element { id, type_key })
}

// RU 统计结构逐层对应 Go JSON：资源组消费 -> 单日快照 -> 最新/上一次快照。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GroupRuStats {
    pub id: i64,
    pub name: String,
    pub ru_consumption: rmpb::Consumption,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DailyRuStats {
    pub end_time: time::Time,
    pub stats: Vec<GroupRuStats>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RuStats {
    pub latest: Option<DailyRuStats>,
    pub previous: Option<DailyRuStats>,
}

// get_oldest_schema_version 读取 schema version meta key 的 MVCC 最老 write short value。
pub fn get_oldest_schema_version(helper: &helper::Helper) -> Result<i64, errors::Error> {
    let mut encoded = Vec::with_capacity(META_PREFIX.len() + SCHEMA_VERSION_KEY.len() + 24);
    encoded.extend_from_slice(META_PREFIX);
    encoded = codec::encode_bytes(&encoded, SCHEMA_VERSION_KEY);
    encoded = codec::encode_uint(&encoded, structure::STRING_DATA as u64);
    let response = helper.get_mvcc_by_encoded_key_with_ts(&encoded, u64::MAX)?;
    let writes = response
        .and_then(|r| r.info)
        .map(|i| i.writes)
        .filter(|w| !w.is_empty())
        .ok_or_else(|| errors::new("There is no Write MVCC info for the schema version key"))?;
    std::str::from_utf8(&writes.last().unwrap().short_value)?
        .parse()
        .map_err(errors::trace)
}

// ddl_job_history_key 仅用于测试，返回 history hash 中指定 job 的完整数据键。
pub fn ddl_job_history_key(mutator: &Mutator, job_id: i64) -> Vec<u8> {
    mutator
        .txn
        .encode_hash_data_key(DDL_JOB_HISTORY_KEY, &job_id.to_be_bytes())
}

impl Mutator {
    // check_* 系列把“读取成功但值为空/非空”转换为对应的领域错误。
    fn require_hash_value(
        &self,
        hash: &[u8],
        field: &[u8],
        message: &str,
    ) -> Result<Vec<u8>, errors::Error> {
        self.txn
            .hget(hash, field)?
            .ok_or_else(|| errors::new(message))
    }
    fn require_hash_absent(
        &self,
        hash: &[u8],
        field: &[u8],
        message: &str,
    ) -> Result<(), errors::Error> {
        if self.txn.hget(hash, field)?.is_some() {
            Err(errors::new(message))
        } else {
            Ok(())
        }
    }
    fn check_policy_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_value(POLICIES, key, "policy doesn't exist")
            .map(|_| ())
    }
    fn check_policy_not_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_absent(POLICIES, key, "policy already exists")
    }
    fn check_masking_policy_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_value(MASKING_POLICIES, key, "masking policy doesn't exist")
            .map(|_| ())
    }
    fn check_masking_policy_not_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_absent(MASKING_POLICIES, key, "masking policy already exists")
    }
    fn check_resource_group_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_value(RESOURCE_GROUPS, key, "group doesn't exist")
            .map(|_| ())
    }
    fn check_resource_group_not_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_absent(RESOURCE_GROUPS, key, "group already exists")
    }
    fn check_db_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_value(DBS, key, "database doesn't exist")
            .map(|_| ())
    }
    fn check_db_not_exists(&self, key: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_absent(DBS, key, "database already exists")
    }
    fn check_table_exists(&self, db: &[u8], table: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_value(db, table, "table doesn't exist")
            .map(|_| ())
    }
    fn check_table_not_exists(&self, db: &[u8], table: &[u8]) -> Result<(), errors::Error> {
        self.require_hash_absent(db, table, "table already exists")
    }

    // policy/masking policy/resource group 均以 JSON + magic byte 写入各自 hash；创建和更新的存在性要求不同。
    pub fn create_policy(&mut self, policy: &model::PolicyInfo) -> Result<(), errors::Error> {
        if policy.id == 0 {
            return Err(errors::new("policy.ID is invalid"));
        }
        let key = self.policy_key(policy.id);
        self.check_policy_not_exists(&key)?;
        self.txn
            .hset(POLICIES, &key, &attach_magic_byte(json::marshal(policy)?))
    }
    pub fn update_policy(&mut self, policy: &model::PolicyInfo) -> Result<(), errors::Error> {
        let key = self.policy_key(policy.id);
        self.check_policy_exists(&key)?;
        self.txn
            .hset(POLICIES, &key, &attach_magic_byte(json::marshal(policy)?))
    }
    pub fn create_masking_policy(
        &mut self,
        policy: &model::MaskingPolicyInfo,
    ) -> Result<(), errors::Error> {
        if policy.id == 0 {
            return Err(errors::new("masking policy.ID is invalid"));
        }
        let key = self.masking_policy_key(policy.id);
        self.check_masking_policy_not_exists(&key)?;
        self.txn.hset(
            MASKING_POLICIES,
            &key,
            &attach_magic_byte(json::marshal(policy)?),
        )
    }
    pub fn update_masking_policy(
        &mut self,
        policy: &model::MaskingPolicyInfo,
    ) -> Result<(), errors::Error> {
        let key = self.masking_policy_key(policy.id);
        self.check_masking_policy_exists(&key)?;
        self.txn.hset(
            MASKING_POLICIES,
            &key,
            &attach_magic_byte(json::marshal(policy)?),
        )
    }
    pub fn add_resource_group(
        &mut self,
        group: &model::ResourceGroupInfo,
    ) -> Result<(), errors::Error> {
        if group.id == 0 {
            return Err(errors::new("group.ID is invalid"));
        }
        let key = self.resource_group_key(group.id);
        self.check_resource_group_not_exists(&key)?;
        self.txn.hset(
            RESOURCE_GROUPS,
            &key,
            &attach_magic_byte(json::marshal(group)?),
        )
    }
    pub fn update_resource_group(
        &mut self,
        group: &model::ResourceGroupInfo,
    ) -> Result<(), errors::Error> {
        let key = self.resource_group_key(group.id);
        // default group 默认不持久化，因此更新它时不能强制要求旧记录存在。
        if group.id != DEFAULT_GROUP_ID {
            self.check_resource_group_exists(&key)?;
        }
        self.txn.hset(
            RESOURCE_GROUPS,
            &key,
            &attach_magic_byte(json::marshal(group)?),
        )
    }
    pub fn drop_resource_group(&mut self, id: i64) -> Result<(), errors::Error> {
        self.txn.hdel(RESOURCE_GROUPS, &self.resource_group_key(id))
    }
    pub fn drop_policy(&mut self, id: i64) -> Result<(), errors::Error> {
        let key = self.policy_key(id);
        self.txn.hclear(&key)?;
        self.txn.hdel(POLICIES, &key)
    }
    pub fn drop_masking_policy(&mut self, id: i64) -> Result<(), errors::Error> {
        let key = self.masking_policy_key(id);
        self.txn.hclear(&key)?;
        self.txn.hdel(MASKING_POLICIES, &key)
    }

    // database/table CRUD 保留两层 hash 布局：DBs 保存 DBInfo，DB:<id> hash 保存 TableInfo 与各种 ID 字段。
    pub fn create_database(&mut self, db: &model::DbInfo) -> Result<(), errors::Error> {
        let key = db_key(db.id);
        self.check_db_not_exists(&key)?;
        self.txn.hset(DBS, &key, &json::marshal(db)?)
    }
    pub fn is_database_exist(&self, id: i64) -> Result<bool, errors::Error> {
        Ok(self.txn.hget(DBS, &db_key(id))?.is_some())
    }
    pub fn update_database(&mut self, db: &model::DbInfo) -> Result<(), errors::Error> {
        let key = db_key(db.id);
        self.check_db_exists(&key)?;
        self.txn.hset(DBS, &key, &json::marshal(db)?)
    }
    pub fn create_table_or_view(
        &mut self,
        db_id: i64,
        table: &model::TableInfo,
    ) -> Result<(), errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        let key = table_key(table.id);
        self.check_table_not_exists(&db, &key)?;
        self.txn.hset(&db, &key, &json::marshal(table)?)
    }
    pub fn update_table(
        &mut self,
        db_id: i64,
        table: &mut model::TableInfo,
    ) -> Result<(), errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        let key = table_key(table.id);
        self.check_table_exists(&db, &key)?;
        // Revision 每次元数据更新都递增，使缓存观察者能识别同 ID 的新版本。
        table.revision += 1;
        self.txn.hset(&db, &key, &json::marshal(table)?)
    }
    pub fn drop_database(&mut self, id: i64) -> Result<(), errors::Error> {
        let key = db_key(id);
        self.txn.hclear(&key)?;
        self.txn.hdel(DBS, &key)
    }
    pub fn drop_table_or_view(&mut self, db_id: i64, table_id: i64) -> Result<(), errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        let table = table_key(table_id);
        self.check_table_exists(&db, &table)?;
        self.txn.hdel(&db, &table)
    }

    // BDR role、表版本、MDL 和 schema cache 都是简单字符串键；空值由 getter 映射为 is_null。
    pub fn set_bdr_role(&mut self, role: &str) -> Result<(), errors::Error> {
        self.txn.set(BDR_ROLE_KEY, role.as_bytes())
    }
    pub fn get_bdr_role(&self) -> Result<String, errors::Error> {
        Ok(String::from_utf8(
            self.txn.get(BDR_ROLE_KEY)?.unwrap_or_default(),
        )?)
    }
    pub fn clear_bdr_role(&mut self) -> Result<(), errors::Error> {
        self.txn.clear(BDR_ROLE_KEY)
    }
    fn set_table_version(&mut self, key: &[u8], version: i32) -> Result<(), errors::Error> {
        self.txn.set(key, version.to_string().as_bytes())
    }
    fn get_table_version(&self, key: &[u8]) -> Result<i32, errors::Error> {
        match self.txn.get(key)? {
            None => Ok(0),
            Some(value) if value.is_empty() => Ok(0),
            Some(value) => Ok(std::str::from_utf8(&value)?.parse()?),
        }
    }
    pub fn set_ddl_table_version(&mut self, version: DDLTableVersion) -> Result<(), errors::Error> {
        self.set_table_version(DDL_TABLE_VERSION_KEY, version as i32)
    }
    pub fn get_ddl_table_version(&self) -> Result<i32, errors::Error> {
        self.get_table_version(DDL_TABLE_VERSION_KEY)
    }
    pub fn set_nextgen_boot_table_version(
        &mut self,
        version: NextGenBootTableVersion,
    ) -> Result<(), errors::Error> {
        self.set_table_version(BOOT_TABLE_VERSION_KEY, version as i32)
    }
    pub fn get_nextgen_boot_table_version(&self) -> Result<i32, errors::Error> {
        self.get_table_version(BOOT_TABLE_VERSION_KEY)
    }
    pub fn set_metadata_lock(&mut self, enabled: bool) -> Result<(), errors::Error> {
        self.txn
            .set(METADATA_LOCK_KEY, if enabled { b"1" } else { b"0" })
    }
    pub fn get_metadata_lock(&self) -> Result<(bool, bool), errors::Error> {
        match self.txn.get(METADATA_LOCK_KEY)? {
            None => Ok((false, true)),
            Some(v) if v.is_empty() => Ok((false, true)),
            Some(v) => Ok((v == b"1", false)),
        }
    }
    pub fn set_schema_cache_size(&mut self, size: u64) -> Result<(), errors::Error> {
        self.txn
            .set(SCHEMA_CACHE_SIZE_KEY, size.to_string().as_bytes())
    }
    pub fn get_schema_cache_size(&self) -> Result<(u64, bool), errors::Error> {
        match self.txn.get(SCHEMA_CACHE_SIZE_KEY)? {
            None => Ok((0, true)),
            Some(v) if v.is_empty() => Ok((0, true)),
            Some(v) => Ok((std::str::from_utf8(&v)?.parse()?, false)),
        }
    }

    // system DB 构造保留 classic 动态全局 ID 与 nextgen 固定 ID 两条路径。
    pub fn create_mysql_database_if_not_exists(&mut self) -> Result<i64, errors::Error> {
        if kerneltype::is_next_gen() {
            self.create_sys_database_by_id_if_not_exists(
                mysql::SYSTEM_DB,
                metadef::SYSTEM_DATABASE_ID,
            )?;
            return Ok(metadef::SYSTEM_DATABASE_ID);
        }
        let existing = self.get_system_db_id()?;
        if existing != 0 {
            return Ok(existing);
        }
        let id = self.gen_global_id()?;
        self.create_sys_database_by_id(mysql::SYSTEM_DB, id)?;
        Ok(id)
    }
    pub fn create_sys_database_by_id_if_not_exists(
        &mut self,
        name: &str,
        id: i64,
    ) -> Result<(), errors::Error> {
        if self.is_database_exist(id)? {
            Ok(())
        } else {
            self.create_sys_database_by_id(name, id)
        }
    }
    pub fn create_sys_database_by_id(&mut self, name: &str, id: i64) -> Result<(), errors::Error> {
        self.create_database(&model::DbInfo::public_system(
            id,
            name,
            mysql::UTF8MB4_CHARSET,
            mysql::UTF8MB4_DEFAULT_COLLATION,
        ))
    }
    pub fn get_system_db_id(&self) -> Result<i64, errors::Error> {
        Ok(self
            .list_databases()?
            .into_iter()
            .find(|db| db.name.lower == mysql::SYSTEM_DB)
            .map_or(0, |db| db.id))
    }

    // create_table_and_set_auto_id 在建表成功后分别初始化 RowID、AUTO_RANDOM 与独立 AUTO_INCREMENT 字段。
    pub fn create_table_and_set_auto_id(
        &mut self,
        db_id: i64,
        table: &model::TableInfo,
        ids: &model::AutoIdGroup,
    ) -> Result<(), errors::Error> {
        self.create_table_or_view(db_id, table)?;
        let db = db_key(db_id);
        self.txn
            .hinc(&db, &auto_table_id_key(table.id), ids.row_id)?;
        if table.auto_random_bits > 0 {
            self.txn
                .hinc(&db, &auto_random_table_id_key(table.id), ids.random_id)?;
        }
        if table.sep_auto_inc() && table.get_auto_increment_col_info().is_some() {
            self.txn
                .hinc(&db, &auto_increment_id_key(table.id), ids.increment_id)?;
        }
        Ok(())
    }
    pub fn create_sequence_and_set_seq_value(
        &mut self,
        db_id: i64,
        table: &model::TableInfo,
        value: i64,
    ) -> Result<(), errors::Error> {
        self.create_table_or_view(db_id, table)?;
        self.txn
            .hinc(&db_key(db_id), &sequence_key(table.id), value)
            .map(|_| ())
    }
    pub fn restart_sequence_value(
        &mut self,
        db_id: i64,
        table: &model::TableInfo,
        value: i64,
    ) -> Result<(), errors::Error> {
        let db = db_key(db_id);
        self.check_db_exists(&db)?;
        self.check_table_exists(&db, &table_key(table.id))?;
        self.txn
            .hset(&db, &sequence_key(table.id), value.to_string().as_bytes())
    }
    pub fn drop_sequence(&mut self, db_id: i64, table_id: i64) -> Result<(), errors::Error> {
        self.drop_table_or_view(db_id, table_id)?;
        self.get_auto_id_accessors(db_id, table_id).del()?;
        self.txn.hdel(&db_key(db_id), &sequence_key(table_id))
    }
}
