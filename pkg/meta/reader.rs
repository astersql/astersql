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

// 元数据只读 Reader 接口及其在 Mutator 上的委托实现。
//
// Reader 统一读取数据库、表、DDL 历史、SchemaDiff、放置策略、脱敏策略、资源组等
// 元数据；`new_reader` 在调用方传入的 KV snapshot（一致性快照）上构造只读 Mutator。

// 本文件对照 pkg/meta/reader.go 定义读取数据库、表、DDL 历史与策略元数据的统一接口。
// new_reader 只包装调用方传入的 snapshot，并在实际 Reader 方法调用时执行元数据 IO。

use std::collections::HashMap;

use crate::context;
use crate::errors;
use crate::kv;
use crate::meta::{LastJobIterator, META_PREFIX, Mutator, RuStats};
use crate::meta_autoid::AutoIdAccessorsImpl;
use crate::model;
use crate::structure;

/// Reader 对应 Go interface；所有错误仍通过 Result 原样交给调用者处理。
/// 迭代方法保留“回调返回错误即停止”的控制流，未引入异步或并发执行。
pub trait Reader {
    /// 按库 ID 读取单个数据库元数据（DbInfo）。
    fn get_database(&self, db_id: i64) -> Result<Option<model::DbInfo>, errors::Error>;
    /// 列出全部数据库元数据。
    fn list_databases(&self) -> Result<Vec<model::DbInfo>, errors::Error>;
    /// 按库 ID 与表 ID 读取单表 TableInfo。
    fn get_table(
        &self,
        db_id: i64,
        table_id: i64,
    ) -> Result<Option<model::TableInfo>, errors::Error>;

    /// context 只传递取消/超时语义，具体 snapshot IO 由实现负责。
    fn list_tables(
        &self,
        ctx: &context::Context,
        db_id: i64,
    ) -> Result<Vec<model::TableInfo>, errors::Error>;
    /// 仅返回表名等轻量信息，避免完整 TableInfo 反序列化开销。
    fn list_simple_tables(&self, db_id: i64) -> Result<Vec<model::TableNameInfo>, errors::Error>;
    /// 遍历全部数据库；visitor 返回 Err 时立即停止。
    fn iter_databases(
        &self,
        visitor: &mut dyn FnMut(model::DbInfo) -> Result<(), errors::Error>,
    ) -> Result<(), errors::Error>;
    /// 遍历指定库下全部表；visitor 返回 Err 时立即停止。
    fn iter_tables(
        &self,
        db_id: i64,
        visitor: &mut dyn FnMut(model::TableInfo) -> Result<(), errors::Error>,
    ) -> Result<(), errors::Error>;
    /// 获取表的 AutoID（自增 ID）访问器，用于读写自增水位。
    fn get_auto_id_accessors(&self, db_id: i64, table_id: i64) -> AutoIdAccessorsImpl<'_>;

    /// 同时返回名称到 ID 的映射，以及 schema load 时必须完整载入的表信息。
    fn get_all_name_to_id_and_the_must_loaded_table_info(
        &self,
        db_id: i64,
    ) -> Result<(HashMap<String, i64>, Vec<model::TableInfo>), errors::Error>;

    /// 第二个 bool 对应 Go isNull，用于区分 false 与元数据键不存在。
    fn get_metadata_lock(&self) -> Result<(bool, bool), errors::Error>;
    /// 按 job ID 读取历史 DDL job（已完成的 DDL 任务记录）。
    fn get_history_ddl_job(&self, id: i64) -> Result<Option<model::Job>, errors::Error>;
    /// 历史 DDL job 总条数。
    fn get_history_ddl_count(&self) -> Result<u64, errors::Error>;
    /// 从最新历史 DDL 起的迭代器。
    fn get_last_history_ddl_jobs_iterator(&self)
    -> Result<Box<dyn LastJobIterator>, errors::Error>;
    /// 从指定 start_job_id 起的历史 DDL 迭代器。
    fn get_history_ddl_jobs_iterator(
        &self,
        start_job_id: i64,
    ) -> Result<Box<dyn LastJobIterator>, errors::Error>;

    /// 当前 schema 版本号（DDL 变更递增）。
    fn get_schema_version(&self) -> Result<i64, errors::Error>;
    /// EncodeSchemaDiffKey 只编码内部 key，不访问 snapshot。
    fn encode_schema_diff_key(&self, schema_version: i64) -> kv::Key;
    /// 读取指定版本的 SchemaDiff（两次 schema 版本之间的变更摘要）。
    fn get_schema_diff(
        &self,
        schema_version: i64,
    ) -> Result<Option<model::SchemaDiff>, errors::Error>;
    /// 找到带有非空 SchemaDiff 的最新 schema 版本。
    fn get_schema_version_with_non_empty_diff(&self) -> Result<i64, errors::Error>;

    /// 下一个放置策略（placement policy）可用 ID。
    fn get_policy_id(&self) -> Result<i64, errors::Error>;
    /// 按 ID 读取放置策略。
    fn get_policy(&self, policy_id: i64) -> Result<model::PolicyInfo, errors::Error>;
    /// 列出全部放置策略。
    fn list_policies(&self) -> Result<Vec<model::PolicyInfo>, errors::Error>;
    /// 下一个脱敏策略（masking policy）可用 ID。
    fn get_masking_policy_id(&self) -> Result<i64, errors::Error>;
    /// 按 ID 读取脱敏策略。
    fn get_masking_policy(&self, policy_id: i64)
    -> Result<model::MaskingPolicyInfo, errors::Error>;
    /// 列出全部脱敏策略。
    fn list_masking_policies(&self) -> Result<Vec<model::MaskingPolicyInfo>, errors::Error>;

    /// 读取 RU（Request Unit，资源计量单位）统计。
    fn get_ru_stats(&self) -> Result<Option<RuStats>, errors::Error>;
    /// 按 ID 读取资源组（resource group）。
    fn get_resource_group(&self, group_id: i64) -> Result<model::ResourceGroupInfo, errors::Error>;
    /// 列出全部资源组。
    fn list_resource_groups(&self) -> Result<Vec<model::ResourceGroupInfo>, errors::Error>;

    /// 按库 ID 读取 structure 层 hash 元数据键值对。
    fn get_metas_by_db_id(&self, db_id: i64) -> Result<Vec<structure::HashPair>, errors::Error>;
    /// 全局对象 ID 分配水位。
    fn get_global_id(&self) -> Result<i64, errors::Error>;
    /// BDR（双向复制）角色字符串。
    fn get_bdr_role(&self) -> Result<String, errors::Error>;
    /// 系统库（如 mysql）的数据库 ID。
    fn get_system_db_id(&self) -> Result<i64, errors::Error>;
    /// bool 保留 Go isNull，避免把缺失的 cache-size 错当成数值 0。
    fn get_schema_cache_size(&self) -> Result<(u64, bool), errors::Error>;
    /// 集群 bootstrap（初始化）版本号。
    fn get_bootstrap_version(&self) -> Result<i64, errors::Error>;
    /// 已完成的 starter bootstrap 版本号。
    fn get_starter_bootstrap_version(&self) -> Result<i64, errors::Error>;
}

/// Mutator 同时实现只读 Reader：各方法直接委托到同名固有方法，保持与 Go 一致的调用面。
impl Reader for Mutator {
    fn get_database(&self, db_id: i64) -> Result<Option<model::DbInfo>, errors::Error> {
        Mutator::get_database(self, db_id)
    }

    fn list_databases(&self) -> Result<Vec<model::DbInfo>, errors::Error> {
        Mutator::list_databases(self)
    }

    fn get_table(
        &self,
        db_id: i64,
        table_id: i64,
    ) -> Result<Option<model::TableInfo>, errors::Error> {
        Mutator::get_table(self, db_id, table_id)
    }

    fn list_tables(
        &self,
        ctx: &context::Context,
        db_id: i64,
    ) -> Result<Vec<model::TableInfo>, errors::Error> {
        Mutator::list_tables(self, ctx, db_id)
    }

    fn list_simple_tables(&self, db_id: i64) -> Result<Vec<model::TableNameInfo>, errors::Error> {
        Mutator::list_simple_tables(self, db_id)
    }

    fn iter_databases(
        &self,
        visitor: &mut dyn FnMut(model::DbInfo) -> Result<(), errors::Error>,
    ) -> Result<(), errors::Error> {
        Mutator::iter_databases(self, visitor)
    }

    fn iter_tables(
        &self,
        db_id: i64,
        visitor: &mut dyn FnMut(model::TableInfo) -> Result<(), errors::Error>,
    ) -> Result<(), errors::Error> {
        Mutator::iter_tables(self, db_id, visitor)
    }

    fn get_auto_id_accessors(&self, db_id: i64, table_id: i64) -> AutoIdAccessorsImpl<'_> {
        Mutator::get_auto_id_accessors(self, db_id, table_id)
    }

    fn get_all_name_to_id_and_the_must_loaded_table_info(
        &self,
        db_id: i64,
    ) -> Result<(HashMap<String, i64>, Vec<model::TableInfo>), errors::Error> {
        crate::meta::get_all_name_to_id_and_must_loaded_table_info(self, db_id)
    }

    fn get_metadata_lock(&self) -> Result<(bool, bool), errors::Error> {
        Mutator::get_metadata_lock(self)
    }

    fn get_history_ddl_job(&self, id: i64) -> Result<Option<model::Job>, errors::Error> {
        Mutator::get_history_ddl_job(self, id)
    }

    fn get_history_ddl_count(&self) -> Result<u64, errors::Error> {
        Mutator::get_history_ddl_count(self)
    }

    fn get_last_history_ddl_jobs_iterator(
        &self,
    ) -> Result<Box<dyn LastJobIterator>, errors::Error> {
        Ok(Box::new(Mutator::get_last_history_ddl_jobs_iterator(self)?))
    }

    fn get_history_ddl_jobs_iterator(
        &self,
        start_job_id: i64,
    ) -> Result<Box<dyn LastJobIterator>, errors::Error> {
        Ok(Box::new(Mutator::get_history_ddl_jobs_iterator(
            self,
            start_job_id,
        )?))
    }

    fn get_schema_version(&self) -> Result<i64, errors::Error> {
        Mutator::get_schema_version(self)
    }

    fn encode_schema_diff_key(&self, schema_version: i64) -> kv::Key {
        Mutator::encode_schema_diff_key(self, schema_version)
    }

    fn get_schema_diff(
        &self,
        schema_version: i64,
    ) -> Result<Option<model::SchemaDiff>, errors::Error> {
        Mutator::get_schema_diff(self, schema_version)
    }

    fn get_schema_version_with_non_empty_diff(&self) -> Result<i64, errors::Error> {
        Mutator::get_schema_version_with_non_empty_diff(self)
    }

    fn get_policy_id(&self) -> Result<i64, errors::Error> {
        Mutator::get_policy_id(self)
    }
    fn get_policy(&self, policy_id: i64) -> Result<model::PolicyInfo, errors::Error> {
        Mutator::get_policy(self, policy_id)
    }
    fn list_policies(&self) -> Result<Vec<model::PolicyInfo>, errors::Error> {
        Mutator::list_policies(self)
    }
    fn get_masking_policy_id(&self) -> Result<i64, errors::Error> {
        Mutator::get_masking_policy_id(self)
    }
    fn get_masking_policy(
        &self,
        policy_id: i64,
    ) -> Result<model::MaskingPolicyInfo, errors::Error> {
        Mutator::get_masking_policy(self, policy_id)
    }
    fn list_masking_policies(&self) -> Result<Vec<model::MaskingPolicyInfo>, errors::Error> {
        Mutator::list_masking_policies(self)
    }
    fn get_ru_stats(&self) -> Result<Option<RuStats>, errors::Error> {
        Mutator::get_ru_stats(self)
    }
    fn get_resource_group(&self, group_id: i64) -> Result<model::ResourceGroupInfo, errors::Error> {
        Mutator::get_resource_group(self, group_id)
    }
    fn list_resource_groups(&self) -> Result<Vec<model::ResourceGroupInfo>, errors::Error> {
        Mutator::list_resource_groups(self)
    }
    fn get_metas_by_db_id(&self, db_id: i64) -> Result<Vec<structure::HashPair>, errors::Error> {
        Mutator::get_metas_by_db_id(self, db_id)
    }
    fn get_global_id(&self) -> Result<i64, errors::Error> {
        Mutator::get_global_id(self)
    }
    fn get_bdr_role(&self) -> Result<String, errors::Error> {
        Mutator::get_bdr_role(self)
    }
    fn get_system_db_id(&self) -> Result<i64, errors::Error> {
        Mutator::get_system_db_id(self)
    }
    fn get_schema_cache_size(&self) -> Result<(u64, bool), errors::Error> {
        Mutator::get_schema_cache_size(self)
    }
    fn get_bootstrap_version(&self) -> Result<i64, errors::Error> {
        Mutator::get_bootstrap_version(self)
    }
    fn get_starter_bootstrap_version(&self) -> Result<i64, errors::Error> {
        Mutator::get_starter_bootstrap_version(self)
    }
}

/// NewReader 在调用方提供的只读 snapshot 上标记内部 meta 请求来源，再用 mMetaPrefix 构造 Mutator。
/// SetOption 只改变 snapshot 请求选项；网络/磁盘读取要等具体 Reader 方法被调用才发生。
pub fn new_reader(mut snapshot: kv::Snapshot) -> Box<dyn Reader> {
    snapshot.set_option(kv::RequestSourceInternal, true);
    snapshot.set_option(kv::RequestSourceType, kv::InternalTxnMeta);
    let transaction = structure::new_snapshot_structure(snapshot, META_PREFIX);
    Box::new(Mutator {
        txn: transaction,
        start_ts: 0,
    })
}

/// Go metadata read from a real immutable KV snapshot. Unlike the harness
/// Reader, this reader retains the target storage's MVCC and error semantics.
pub struct SnapshotReader {
    snapshot: Box<dyn astersql_kv::Snapshot>,
}
impl SnapshotReader {
    /// Mark the provided real MVCC snapshot as internal metadata traffic.
    pub fn new(mut snapshot: Box<dyn astersql_kv::Snapshot>) -> Self {
        snapshot.SetOption(astersql_kv::RequestSourceInternal, Some(Box::new(true)));
        snapshot.SetOption(
            astersql_kv::RequestSourceType,
            Some(Box::new(astersql_kv::InternalTxnMeta.to_string())),
        );
        snapshot.SetOption(astersql_kv::TiKVClientReadTimeout, Some(Box::new(3000_u64)));
        Self { snapshot }
    }
    fn hash_get(&self, hash: &[u8], field: &[u8]) -> Result<Option<Vec<u8>>, errors::Error> {
        let key = transaction_meta_hash_key(hash, field);
        match self
            .snapshot
            .Get(&astersql_kv::Context::default(), key, &[])
        {
            Ok(value) => Ok(Some(value.Value)),
            Err(error) if astersql_kv::IsErrNotFound(&error) => Ok(None),
            Err(error) => Err(errors::new(error)),
        }
    }
    /// Recovery reads the newest committed version with an actual diff, as Go.
    pub fn get_schema_version_with_non_empty_diff(&self) -> Result<i64, String> {
        let ctx = astersql_kv::Context::default();
        let version =
            match self
                .snapshot
                .Get(&ctx, transaction_meta_string_key(b"SchemaVersionKey"), &[])
            {
                Ok(raw) => std::str::from_utf8(&raw.Value)
                    .map_err(|e| e.to_string())?
                    .parse::<i64>()
                    .map_err(|e| e.to_string())?,
                Err(e) if astersql_kv::IsErrNotFound(&e) => 0,
                Err(e) => return Err(e.to_string()),
            };
        if version > 0 {
            match self.snapshot.Get(
                &ctx,
                transaction_meta_string_key(format!("Diff:{version}").as_bytes()),
                &[],
            ) {
                Ok(raw) if !raw.Value.is_empty() => return Ok(version),
                Ok(_) => return Ok(version - 1),
                Err(e) if astersql_kv::IsErrNotFound(&e) => return Ok(version - 1),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(version)
    }
    /// Read Go DBs/DB:<id> metadata from this snapshot.
    pub fn get_database(
        &self,
        id: i64,
    ) -> Result<Option<astersql_meta_model::DBInfo>, errors::Error> {
        self.hash_get(b"DBs", format!("DB:{id}").as_bytes())?
            .map(|raw| astersql_meta_model::DecodeDBInfo(&raw).map_err(errors::new))
            .transpose()
    }
    /// Read a table in the requested database without consulting InfoSchema.
    pub fn get_table(
        &self,
        db: i64,
        id: i64,
    ) -> Result<Option<astersql_meta_model::TableInfo>, errors::Error> {
        if self.get_database(db)?.is_none() {
            return Err(errors::new(format!("database {db} not found")));
        }
        self.hash_get(
            format!("DB:{db}").as_bytes(),
            format!("Table:{id}").as_bytes(),
        )?
        .map(|raw| astersql_meta_model::DecodeTableInfo(&raw).map_err(errors::new))
        .transpose()
    }
    /// Read Go DDLJobHistory, keyed by the big-endian job ID.
    pub fn get_history_ddl_job(
        &self,
        id: i64,
    ) -> Result<Option<astersql_meta_model::group_3::Job>, errors::Error> {
        self.hash_get(b"DDLJobHistory", &id.to_be_bytes())?
            .map(|raw| decode_go_history_job(&raw))
            .transpose()
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct GoHistoryError {
    class: i64,
    code: i64,
    message: String,
    rfccode: String,
}
impl GoHistoryError {
    fn display(self) -> String {
        // github.com/pingcap/errors compatible_shim.go, pinned by go.mod.
        let class = match self.class {
            1 => "autoid",
            2 => "ddl",
            3 => "domain",
            4 => "evaluator",
            5 => "executor",
            6 => "expression",
            7 => "admin",
            8 => "kv",
            9 => "meta",
            10 => "planner",
            11 => "parser",
            12 => "perfschema",
            13 => "privilege",
            14 => "schema",
            15 => "server",
            16 => "struct",
            17 => "variable",
            18 => "xeval",
            19 => "table",
            20 => "types",
            21 => "global",
            22 => "mocktikv",
            23 => "json",
            24 => "tikv",
            25 => "session",
            26 => "plugin",
            27 => "util",
            _ => "",
        };
        let code = if self.rfccode.is_empty() && self.class > 0 {
            format!("{class}:{}", self.code)
        } else {
            self.rfccode
        };
        format!("[{code}]{}", self.message)
    }
}
pub fn decode_go_history_job(
    raw: &[u8],
) -> Result<astersql_meta_model::group_3::Job, errors::Error> {
    // The current Job error ABI is a display string. Adapt Go's structured
    // terror JSON to Error() at the read boundary without rewriting stored
    // history or dropping fields from the complete Job model.
    let mut value: serde_json::Value = serde_json::from_slice(raw)?;
    for field in ["err", "warning"] {
        if let Some(error) = value.get_mut(field)
            && error.is_object()
        {
            let wire: GoHistoryError = serde_json::from_value(error.clone())?;
            *error = serde_json::Value::String(wire.display());
        }
    }
    astersql_meta_model::group_3::Job::decode(&serde_json::to_vec(&value)?).map_err(errors::new)
}

/// Go structure hash key shared by real metadata readers and writers.
pub fn transaction_meta_hash_key(hash: &[u8], field: &[u8]) -> astersql_kv::Key {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    astersql_kv::Key(EncodeBytes(
        EncodeUint(EncodeBytes(vec![b'm'], hash), b'h' as u64),
        field,
    ))
}
pub fn transaction_meta_string_key(key: &[u8]) -> astersql_kv::Key {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    astersql_kv::Key(EncodeUint(EncodeBytes(vec![b'm'], key), b's' as u64))
}
/// Borrow the existing SQL transaction; never opens or commits another Store.
/// The key layouts follow structure/type.go and meta/meta.go.
pub struct TransactionMutator<'a> {
    txn: &'a mut dyn astersql_kv::Transaction,
}
impl<'a> TransactionMutator<'a> {
    pub fn new(txn: &'a mut dyn astersql_kv::Transaction) -> Self {
        txn.SetOption(
            astersql_kv::Priority,
            Some(Box::new(astersql_kv::PriorityHigh)),
        );
        txn.SetDiskFullOpt(astersql_kv::kvrpcpb::DiskFullOpt::AllowedOnAlmostFull);
        Self { txn }
    }
    pub fn start_ts(&self) -> u64 {
        self.txn.StartTS()
    }
    fn get(&self, key: astersql_kv::Key) -> Result<Option<Vec<u8>>, String> {
        match self.txn.Get(&astersql_kv::Context::default(), key, &[]) {
            Ok(entry) => Ok(Some(entry.Value)),
            Err(e) if astersql_kv::IsErrNotFound(&e) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn get_database(&self, id: i64) -> Result<Option<astersql_meta_model::DBInfo>, String> {
        self.get(transaction_meta_hash_key(
            b"DBs",
            format!("DB:{id}").as_bytes(),
        ))?
        .map(|raw| astersql_meta_model::DecodeDBInfo(&raw))
        .transpose()
    }
    pub fn get_table(
        &self,
        db: i64,
        id: i64,
    ) -> Result<Option<astersql_meta_model::TableInfo>, String> {
        self.get(transaction_meta_hash_key(
            format!("DB:{db}").as_bytes(),
            format!("Table:{id}").as_bytes(),
        ))?
        .map(|raw| astersql_meta_model::DecodeTableInfo(&raw))
        .transpose()
    }
    /// Inspect Go's numeric mode before decoding the Rust enum. Go retains
    /// unknown mode values so the DDL handler can cancel them explicitly.
    pub fn get_table_mode_value(&self, db: i64, id: i64) -> Result<Option<i64>, String> {
        self.get(transaction_meta_hash_key(
            format!("DB:{db}").as_bytes(),
            format!("Table:{id}").as_bytes(),
        ))?
        .map(|raw| {
            let value: serde_json::Value =
                serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
            Ok(value.get("mode").and_then(|m| m.as_i64()).unwrap_or(0))
        })
        .transpose()
    }
    pub fn update_table(
        &mut self,
        db: i64,
        table: &mut astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        if self.get_database(db)?.is_none() || self.get_table(db, table.ID)?.is_none() {
            return Err("table metadata disappeared".into());
        }
        table.Revision = table.Revision.wrapping_add(1);
        if table.State == astersql_meta_model::SchemaState::Public {
            table.UpdateTS = self.start_ts()
        }
        self.txn
            .Set(
                transaction_meta_hash_key(
                    format!("DB:{db}").as_bytes(),
                    format!("Table:{}", table.ID).as_bytes(),
                ),
                astersql_meta_model::EncodeTableInfo(table)?,
            )
            .map_err(|e| e.to_string())
    }
    pub fn gen_schema_version(&mut self) -> Result<i64, String> {
        astersql_kv::IncInt64(
            self.txn,
            &transaction_meta_string_key(b"SchemaVersionKey"),
            1,
        )
        .map_err(|e| e.to_string())
    }
    /// The Go default diff for a single metadata-only action.
    pub fn set_table_schema_diff(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        version: i64,
    ) -> Result<(), String> {
        let diff = serde_json::json!({"version":version,"type":job.tp,"schema_id":job.schema_id,"table_id":job.table_id,"old_table_id":0,"old_schema_id":0,"regenerate_schema_map":false,"affected_options":null});
        self.txn
            .Set(
                transaction_meta_string_key(format!("Diff:{version}").as_bytes()),
                serde_json::to_vec(&diff).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
    }
    pub fn add_history_ddl_job(
        &mut self,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        self.txn
            .Set(
                transaction_meta_hash_key(b"DDLJobHistory", &job.id.to_be_bytes()),
                encode_go_ddl_job(job, false)?,
            )
            .map_err(|e| e.to_string())
    }
}
/// Convert the existing display-string error ABI at the durable Go wire boundary.
/// Go jobs use structured terror errors; plain strings would fail Go decoding.
pub fn encode_go_ddl_job(
    job: &mut astersql_meta_model::group_3::Job,
    update_args: bool,
) -> Result<Vec<u8>, String> {
    let raw = job.encode(update_args).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    for field in ["err", "warning"] {
        if let Some(serde_json::Value::String(message)) = value.get(field) {
            let mut class = 2;
            let mut code = 1105;
            let mut text = message.as_str();
            let mut rfc = "ddl:1105".to_owned();
            if let Some((prefix, rest)) = message.strip_prefix('[').and_then(|m| m.split_once(']'))
            {
                if let Some((name, n)) = prefix.split_once(':') {
                    if let Ok(n) = n.parse::<i64>() {
                        class = match name {
                            "schema" => 14,
                            "ddl" => 2,
                            _ => 2,
                        };
                        code = n;
                        text = rest;
                        rfc = prefix.to_owned();
                    }
                }
            }
            value[field] =
                serde_json::json!({"class":class,"code":code,"message":text,"rfccode":rfc});
        }
    }
    serde_json::to_vec(&value).map_err(|e| e.to_string())
}

/// SQL history timestamp from Go's physical TSO milliseconds, in UTC.
pub fn tso_history_datetime(ts: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis((ts >> 18) as i64)
        .expect("u64 TSO physical milliseconds fit chrono range")
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}
