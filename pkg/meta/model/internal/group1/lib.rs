// Copyright 2026 AsterSQL.

// meta model group1 库根：聚合 AST/类型依赖、DDL Action 常量、Schema 状态，
// 并再导出 column / index / db / bdr / flags / table 等元数据子模块。
//
// 本 crate 是表模型身份（`TableInfo` 等）的正式定义边界。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 解析器 AST 与分区/引用选项等别名常量。
pub mod ast {
    pub use parser_ast::model;
    pub use parser_ast::model::{
        ColumnChoice, IndexType, PartitionType, ReferOptionType, RunawayActionType,
        RunawayWatchType, ViewAlgorithm, ViewCheckOption, ViewSecurity,
    };
    pub use parser_ast::model::{
        MediumPriorityValue, PriorityValueToName, RunawayActionCooldown, RunawayActionDryRun,
        RunawayActionKill, RunawayActionNone, RunawayActionSwitchGroup, WatchExact, WatchNone,
        WatchPlan, WatchSimilar,
    };
    pub use parser_ast::{
        CIStr, ExprKind, ExprNode, MaskingPolicyRestrictOps, NewCIStr, SelectStmt, metadata_json,
    };

    /// 无分区。
    pub const PartitionTypeNone: PartitionType = model::PartitionTypeNone;
    /// RANGE 分区。
    pub const PartitionTypeRange: PartitionType = model::PartitionTypeRange;
    /// LIST 分区。
    pub const PartitionTypeList: PartitionType = model::PartitionTypeList;
    /// HASH 分区。
    pub const PartitionTypeHash: PartitionType = model::PartitionTypeHash;
    /// 外键无引用动作。
    pub const ReferOptionNoOption: ReferOptionType = model::ReferOptionNoOption;
    /// 外键 RESTRICT。
    pub const ReferOptionRestrict: ReferOptionType = model::ReferOptionRestrict;
    /// 外键 CASCADE。
    pub const ReferOptionCascade: ReferOptionType = model::ReferOptionCascade;
    /// 列生成默认选择。
    pub const DefaultChoice: ColumnChoice = model::DefaultChoice;
    /// 表亲和级别：无。
    pub const TableAffinityLevelNone: &str = "none";

    /// 规范化表亲和级别字符串；非法值返回 `None`。
    pub fn NormalizeTableAffinityLevel(value: &str) -> Option<String> {
        let lower = value.to_lowercase();
        match lower.as_str() {
            "" | "none" => Some(TableAffinityLevelNone.to_owned()),
            "table" | "partition" => Some(lower),
            _ => None,
        }
    }
}
/// 认证相关类型再导出。
pub mod auth {
    pub use parser_auth::auth::*;
}
/// 规划器基础类型（如 Hasher）再导出。
pub mod base {
    pub use planner_base::base::*;
}
/// 字符集与二进制校对常量。
pub mod charset {
    pub use parser_charset::charset::{CharsetBin, CollationBin};
}
/// 内核类型（classic / next-gen）探测。
pub mod kerneltype {
    pub use kerneltype::*;
}
/// MySQL 常量、类型码与工具函数。
pub mod mysql {
    pub use parser_mysql::r#const::*;
    pub use parser_mysql::r#type::*;
    pub use parser_mysql::util::*;
}
/// SQL 解析器入口。
pub mod parser {
    pub use ::parser::*;
}
/// 字段类型等解析器类型系统。
pub mod types {
    pub use parser_types::types::*;
}
/// 时间类型别名（UTC DateTime 与 Duration）。
pub mod time {
    pub use chrono::{DateTime, Utc};
    pub use std::time::Duration;
    pub type Time = DateTime<Utc>;
}
/// 共享错误类型与格式化构造。
pub mod errors {
    pub use astersql_errors::SharedError as Error;

    /// 用消息字符串构造共享错误。
    pub fn Errorf(message: String) -> Error {
        astersql_errors::New(message)
    }
}
/// 人类可读时长解析（如 `1h` → `Duration`）。
pub mod duration {
    use super::errors;

    fn read_float(value: &str) -> Result<(f64, &str), errors::Error> {
        let end = value
            .char_indices()
            .find_map(|(position, character)| {
                (!character.is_numeric() && character != '.').then_some(position)
            })
            .filter(|position| *position > 0)
            .ok_or_else(|| errors::Errorf("fail to read an integer".to_owned()))?;
        let number = &value[..end];
        let parsed = number
            .parse::<f64>()
            .map_err(|error| errors::Errorf(format!("invalid float {number:?}: {error}")))?;
        Ok((parsed, &value[end..]))
    }

    /// 解析时长字符串；失败时包装为共享错误。
    pub fn ParseDuration(mut value: &str) -> Result<std::time::Duration, errors::Error> {
        if value == "0" {
            return Ok(std::time::Duration::ZERO);
        }

        let mut duration_nanos = 0_u64;
        while !value.is_empty() {
            let (number, rest) = read_float(value)?;
            let unit = rest
                .as_bytes()
                .first()
                .copied()
                .ok_or_else(|| errors::Errorf("duration unit is missing".to_owned()))?;
            let unit_nanos = match unit {
                b'd' => 24.0 * 60.0 * 60.0 * 1_000_000_000.0,
                b'h' => 60.0 * 60.0 * 1_000_000_000.0,
                b'm' => 60.0 * 1_000_000_000.0,
                _ => return Err(errors::Errorf(format!("unknown unit {}", unit as char))),
            };
            duration_nanos = duration_nanos.saturating_add((number * unit_nanos) as u64);
            value = &rest[1..];
        }

        Ok(std::time::Duration::from_nanos(duration_nanos))
    }
}

/// DDL 作业动作类型（持久化数值协议）。
pub type ActionType = u8;
/// 创建 schema（库）。
pub const ACTION_CREATE_SCHEMA: ActionType = 1;
/// 删除 schema。
pub const ACTION_DROP_SCHEMA: ActionType = 2;
/// 创建表。
pub const ACTION_CREATE_TABLE: ActionType = 3;
/// 删除表。
pub const ACTION_DROP_TABLE: ActionType = 4;
/// 加列。
pub const ACTION_ADD_COLUMN: ActionType = 5;
/// 删列。
pub const ACTION_DROP_COLUMN: ActionType = 6;
/// 加索引。
pub const ACTION_ADD_INDEX: ActionType = 7;
/// 删索引。
pub const ACTION_DROP_INDEX: ActionType = 8;
/// 加外键。
pub const ACTION_ADD_FOREIGN_KEY: ActionType = 9;
/// 删外键。
pub const ACTION_DROP_FOREIGN_KEY: ActionType = 10;
/// 清空表。
pub const ACTION_TRUNCATE_TABLE: ActionType = 11;
/// 修改列。
pub const ACTION_MODIFY_COLUMN: ActionType = 12;
/// 重置自增 ID 基线。
pub const ACTION_REBASE_AUTO_ID: ActionType = 13;
/// 重命名表。
pub const ACTION_RENAME_TABLE: ActionType = 14;
/// 设置默认值。
pub const ACTION_SET_DEFAULT_VALUE: ActionType = 15;
/// 分片行 ID。
pub const ACTION_SHARD_ROW_ID: ActionType = 16;
/// 修改表注释。
pub const ACTION_MODIFY_TABLE_COMMENT: ActionType = 17;
/// 重命名索引。
pub const ACTION_RENAME_INDEX: ActionType = 18;
/// 添加表分区。
pub const ACTION_ADD_TABLE_PARTITION: ActionType = 19;
/// 删除表分区。
pub const ACTION_DROP_TABLE_PARTITION: ActionType = 20;
/// 创建视图。
pub const ACTION_CREATE_VIEW: ActionType = 21;
/// 修改表字符集与校对。
pub const ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE: ActionType = 22;
/// 清空表分区。
pub const ACTION_TRUNCATE_TABLE_PARTITION: ActionType = 23;
/// 删除视图。
pub const ACTION_DROP_VIEW: ActionType = 24;
/// 恢复表。
pub const ACTION_RECOVER_TABLE: ActionType = 25;
/// 修改 schema 字符集与校对。
pub const ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE: ActionType = 26;
/// 锁表。
pub const ACTION_LOCK_TABLE: ActionType = 27;
/// 解锁表。
pub const ACTION_UNLOCK_TABLE: ActionType = 28;
/// 修复表。
pub const ACTION_REPAIR_TABLE: ActionType = 29;
/// 设置 TiFlash 副本。
pub const ACTION_SET_TIFLASH_REPLICA: ActionType = 30;
/// 更新 TiFlash 副本状态。
pub const ACTION_UPDATE_TIFLASH_REPLICA_STATUS: ActionType = 31;
/// 加主键。
pub const ACTION_ADD_PRIMARY_KEY: ActionType = 32;
/// 删主键。
pub const ACTION_DROP_PRIMARY_KEY: ActionType = 33;
/// 创建序列。
pub const ACTION_CREATE_SEQUENCE: ActionType = 34;
/// 修改序列。
pub const ACTION_ALTER_SEQUENCE: ActionType = 35;
/// 删除序列。
pub const ACTION_DROP_SEQUENCE: ActionType = 36;
/// 已废弃：批量加列；保留持久化编号兼容性。
pub const DEPRECATED_ACTION_ADD_COLUMNS: ActionType = 37;
/// 已废弃：批量删列；保留持久化编号兼容性。
pub const DEPRECATED_ACTION_DROP_COLUMNS: ActionType = 38;
/// 修改表自增缓存。
pub const ACTION_MODIFY_TABLE_AUTO_ID_CACHE: ActionType = 39;
/// 重置 AUTO_RANDOM 基线。
pub const ACTION_REBASE_AUTO_RANDOM_BASE: ActionType = 40;
/// 修改索引可见性。
pub const ACTION_ALTER_INDEX_VISIBILITY: ActionType = 41;
/// 交换表分区。
pub const ACTION_EXCHANGE_TABLE_PARTITION: ActionType = 42;
/// 加 CHECK 约束。
pub const ACTION_ADD_CHECK_CONSTRAINT: ActionType = 43;
/// 删 CHECK 约束。
pub const ACTION_DROP_CHECK_CONSTRAINT: ActionType = 44;
/// 修改 CHECK 约束。
pub const ACTION_ALTER_CHECK_CONSTRAINT: ActionType = 45;
/// 已废弃：修改表分区定义。
pub const DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION: ActionType = 46;
/// 批量重命名表。
pub const ACTION_RENAME_TABLES: ActionType = 47;
/// 已废弃：批量删索引；保留持久化编号兼容性。
pub const DEPRECATED_ACTION_DROP_INDEXES: ActionType = 48;
/// 修改表属性。
pub const ACTION_ALTER_TABLE_ATTRIBUTES: ActionType = 49;
/// 修改表分区属性。
pub const ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES: ActionType = 50;
/// 创建放置策略。
pub const ACTION_CREATE_PLACEMENT_POLICY: ActionType = 51;
/// 修改放置策略。
pub const ACTION_ALTER_PLACEMENT_POLICY: ActionType = 52;
/// 删除放置策略。
pub const ACTION_DROP_PLACEMENT_POLICY: ActionType = 53;
/// 修改表分区放置。
pub const ACTION_ALTER_TABLE_PARTITION_PLACEMENT: ActionType = 54;
/// 修改 schema 默认放置。
pub const ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT: ActionType = 55;
/// 修改表放置。
pub const ACTION_ALTER_TABLE_PLACEMENT: ActionType = 56;
/// 设为缓存表。
pub const ACTION_ALTER_CACHE_TABLE: ActionType = 57;
/// 修改表统计选项。
pub const ACTION_ALTER_TABLE_STATS_OPTIONS: ActionType = 58;
/// 取消缓存表。
pub const ACTION_ALTER_NO_CACHE_TABLE: ActionType = 59;
/// 批量建表。
pub const ACTION_CREATE_TABLES: ActionType = 60;
/// 多 schema 变更事务。
pub const ACTION_MULTI_SCHEMA_CHANGE: ActionType = 61;
/// 集群闪回。
pub const ACTION_FLASHBACK_CLUSTER: ActionType = 62;
/// 恢复 schema。
pub const ACTION_RECOVER_SCHEMA: ActionType = 63;
/// 重组分区。
pub const ACTION_REORGANIZE_PARTITION: ActionType = 64;
/// 修改 TTL 信息。
pub const ACTION_ALTER_TTL_INFO: ActionType = 65;
/// 移除 TTL。
pub const ACTION_ALTER_TTL_REMOVE: ActionType = 67;
/// 创建资源组。
pub const ACTION_CREATE_RESOURCE_GROUP: ActionType = 68;
/// 修改资源组。
pub const ACTION_ALTER_RESOURCE_GROUP: ActionType = 69;
/// 删除资源组。
pub const ACTION_DROP_RESOURCE_GROUP: ActionType = 70;
/// 修改表分区方案。
pub const ACTION_ALTER_TABLE_PARTITIONING: ActionType = 71;
/// 移除分区。
pub const ACTION_REMOVE_PARTITIONING: ActionType = 72;
/// 添加列存索引。
pub const ACTION_ADD_COLUMNAR_INDEX: ActionType = 73;
/// 修改引擎属性。
pub const ACTION_MODIFY_ENGINE_ATTRIBUTE: ActionType = 74;
/// 修改表模式。
pub const ACTION_ALTER_TABLE_MODE: ActionType = 75;
/// 刷新元数据。
pub const ACTION_REFRESH_META: ActionType = 76;
/// 修改 schema 只读。
pub const ACTION_MODIFY_SCHEMA_READ_ONLY: ActionType = 77;
/// 修改表亲和。
pub const ACTION_ALTER_TABLE_AFFINITY: ActionType = 78;
/// 修改表软删除信息。
pub const ACTION_ALTER_TABLE_SOFT_DELETE_INFO: ActionType = 79;
/// 修改 schema 软删除与双活。
pub const ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE: ActionType = 80;
/// 创建脱敏策略。
pub const ACTION_CREATE_MASKING_POLICY: ActionType = 81;
/// 修改脱敏策略。
pub const ACTION_ALTER_MASKING_POLICY: ActionType = 82;
/// 删除脱敏策略。
pub const ACTION_DROP_MASKING_POLICY: ActionType = 83;
/// 设置表 Region 拆分策略。
pub const ACTION_ALTER_TABLE_SET_REGION_SPLIT_POLICY: ActionType = 84;
/// Go DDL ActionType 85.
pub const ACTION_CREATE_MATERIALIZED_VIEW_LOG: ActionType = 85;
/// Go DDL ActionType 86.
pub const ACTION_CREATE_MATERIALIZED_VIEW: ActionType = 86;
/// Go DDL ActionType 87.
pub const ACTION_DROP_MATERIALIZED_VIEW_LOG: ActionType = 87;
/// Go DDL ActionType 88.
pub const ACTION_DROP_MATERIALIZED_VIEW: ActionType = 88;
/// Go DDL ActionType 89.
pub const ACTION_ALTER_MATERIALIZED_VIEW_REFRESH: ActionType = 89;
/// Go DDL ActionType 90.
pub const ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE: ActionType = 90;
/// Go DDL ActionType 91.
pub const ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES: ActionType = 91;
/// Go DDL ActionType 92.
pub const ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER: ActionType = 92;
/// Go DDL ActionType 93.
pub const ACTION_CREATE_MATERIALIZED_VIEW_SHADOW: ActionType = 93;
/// Go DDL ActionType 94.
pub const ACTION_DROP_MATERIALIZED_VIEW_SHADOW: ActionType = 94;

/// Schema 对象状态机（DDL 在线变更各阶段）。
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SchemaState(pub u8);

impl Default for SchemaState {
    fn default() -> Self {
        Self::None
    }
}

impl SchemaState {
    pub const None: Self = Self(0);
    pub const DeleteOnly: Self = Self(1);
    pub const WriteOnly: Self = Self(2);
    pub const WriteReorganization: Self = Self(3);
    pub const DeleteReorganization: Self = Self(4);
    pub const Public: Self = Self(5);
    pub const ReplicaOnly: Self = Self(6);
    pub const GlobalTxnOnly: Self = Self(7);

    pub fn String(self) -> &'static str {
        match self {
            Self::DeleteOnly => "delete only",
            Self::WriteOnly => "write only",
            Self::WriteReorganization => "write reorganization",
            Self::DeleteReorganization => "delete reorganization",
            Self::Public => "public",
            Self::ReplicaOnly => "replica only",
            Self::GlobalTxnOnly => "global txn only",
            _ => "none",
        }
    }
}

impl std::fmt::Display for SchemaState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.String())
    }
}
/// `SchemaState::Public` 别名。
pub const StatePublic: SchemaState = SchemaState::Public;
/// `SchemaState::None` 别名。
pub const StateNone: SchemaState = SchemaState::None;
/// `SchemaState::DeleteOnly` 别名。
pub const StateDeleteOnly: SchemaState = SchemaState::DeleteOnly;
/// `SchemaState::WriteOnly` 别名。
pub const StateWriteOnly: SchemaState = SchemaState::WriteOnly;
/// `SchemaState::WriteReorganization` 别名。
pub const StateWriteReorganization: SchemaState = SchemaState::WriteReorganization;
/// `SchemaState::DeleteReorganization` 别名。
pub const StateDeleteReorganization: SchemaState = SchemaState::DeleteReorganization;
/// `SchemaState::ReplicaOnly` 别名。
pub const StateReplicaOnly: SchemaState = SchemaState::ReplicaOnly;
/// `SchemaState::GlobalTxnOnly` 别名。
pub const StateGlobalTxnOnly: SchemaState = SchemaState::GlobalTxnOnly;

/// 索引回填（backfill）状态。
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BackfillState(pub u8);

impl Default for BackfillState {
    fn default() -> Self {
        Self::Inapplicable
    }
}

impl BackfillState {
    pub const Inapplicable: Self = Self(0);
    pub const Running: Self = Self(1);
    pub const ReadyToMerge: Self = Self(2);
    pub const Merging: Self = Self(3);

    pub fn String(self) -> &'static str {
        match self {
            Self::Running => "backfill state running",
            Self::ReadyToMerge => "backfill state ready to merge",
            Self::Merging => "backfill state merging",
            Self::Inapplicable => "backfill state inapplicable",
            _ => "backfill state unknown",
        }
    }
}

pub const BackfillStateInapplicable: BackfillState = BackfillState::Inapplicable;
pub const BackfillStateRunning: BackfillState = BackfillState::Running;
pub const BackfillStateReadyToMerge: BackfillState = BackfillState::ReadyToMerge;
pub const BackfillStateMerging: BackfillState = BackfillState::Merging;

/// 放置策略引用：策略 ID 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PolicyRefInfo {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
}

/// 无动作。
pub const ActionNone: ActionType = 0;
/// `ACTION_ADD_TABLE_PARTITION` 的 PascalCase 别名。
pub const ActionAddTablePartition: ActionType = ACTION_ADD_TABLE_PARTITION;
/// `ACTION_DROP_TABLE_PARTITION` 的 PascalCase 别名。
pub const ActionDropTablePartition: ActionType = ACTION_DROP_TABLE_PARTITION;
/// `ACTION_TRUNCATE_TABLE_PARTITION` 的 PascalCase 别名。
pub const ActionTruncateTablePartition: ActionType = ACTION_TRUNCATE_TABLE_PARTITION;

#[path = "../../column.rs"]
mod column;
pub use column::*;
#[path = "../../index.rs"]
mod index;
pub use index::*;
#[path = "../../metadata_codec.rs"]
mod metadata_codec;
pub use metadata_codec::*;

include!("../../table_mode.rs");
include!("../../table.rs");

#[path = "../../bdr.rs"]
mod bdr;
pub use bdr::*;
#[path = "../../db.rs"]
mod db;
pub use db::*;
#[path = "../../flags.rs"]
mod flags;
pub use flags::*;

#[cfg(test)]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "../../table_4_aster_unit_test.rs"]
mod table_4_aster_unit_test;
