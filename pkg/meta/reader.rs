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
