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

// BDR（Bidirectional Replication，双向复制）相关 DDL 安全分类。
//
// 将各类 DDL `ActionType` 归入 Safe / Unsafe / Unmanagement / Unknown，
// 供双向复制场景判断哪些 schema 变更可在对端安全重放。
// 同时提供 TSO（Timestamp Oracle，时间戳 oracle）物理部分到 UTC 时间的转换。

use super::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::LazyLock;

/// DDL 在 BDR 语义下的安全类别标签（静态字符串包装）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DDLBDRType(pub &'static str);

/// 不安全 DDL：通常含破坏性/不可逆变更，BDR 下需额外管控。
pub const UnsafeDDL: DDLBDRType = DDLBDRType("unsafe DDL");
/// 安全 DDL：一般可在双向复制中重放且风险较低。
pub const SafeDDL: DDLBDRType = DDLBDRType("safe DDL");
/// 非管理类 DDL：如放置策略、资源组等集群级对象，不纳入常规 BDR 管理。
pub const UnmanagementDDL: DDLBDRType = DDLBDRType("unmanagement DDL");
/// 未知/已废弃 DDL：无法归入上述类别的动作。
pub const UnknownDDL: DDLBDRType = DDLBDRType("unknown DDL");

/// 正向映射：BDR 类别 → 属于该类别的 DDL Action 列表。
pub static BDRActionMap: LazyLock<HashMap<DDLBDRType, Vec<ActionType>>> = LazyLock::new(|| {
    HashMap::from([
        (
            SafeDDL,
            vec![
                ACTION_CREATE_SCHEMA,
                ACTION_CREATE_TABLE,
                ACTION_CREATE_MATERIALIZED_VIEW_LOG,
                ACTION_CREATE_MATERIALIZED_VIEW,
                ACTION_ALTER_MATERIALIZED_VIEW_REFRESH,
                ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES,
                ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE,
                ACTION_ADD_COLUMN,
                ACTION_ADD_INDEX,
                ACTION_DROP_INDEX,
                ACTION_MODIFY_COLUMN,
                ACTION_SET_DEFAULT_VALUE,
                ACTION_MODIFY_TABLE_COMMENT,
                ACTION_RENAME_INDEX,
                ACTION_ADD_TABLE_PARTITION,
                ACTION_DROP_PRIMARY_KEY,
                ACTION_ALTER_INDEX_VISIBILITY,
                ACTION_CREATE_TABLES,
                ACTION_ALTER_TTL_INFO,
                ACTION_ALTER_TTL_REMOVE,
                ACTION_CREATE_VIEW,
                ACTION_DROP_VIEW,
                ACTION_ALTER_TABLE_AFFINITY,
            ],
        ),
        (
            UnsafeDDL,
            vec![
                ACTION_DROP_SCHEMA,
                ACTION_DROP_TABLE,
                ACTION_DROP_MATERIALIZED_VIEW,
                ACTION_DROP_MATERIALIZED_VIEW_LOG,
                ACTION_DROP_MATERIALIZED_VIEW_SHADOW,
                ACTION_DROP_COLUMN,
                ACTION_ADD_FOREIGN_KEY,
                ACTION_DROP_FOREIGN_KEY,
                ACTION_TRUNCATE_TABLE,
                ACTION_REBASE_AUTO_ID,
                ACTION_RENAME_TABLE,
                ACTION_SHARD_ROW_ID,
                ACTION_DROP_TABLE_PARTITION,
                ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE,
                ACTION_TRUNCATE_TABLE_PARTITION,
                ACTION_RECOVER_TABLE,
                ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE,
                ACTION_LOCK_TABLE,
                ACTION_UNLOCK_TABLE,
                ACTION_REPAIR_TABLE,
                ACTION_SET_TIFLASH_REPLICA,
                ACTION_UPDATE_TIFLASH_REPLICA_STATUS,
                ACTION_ADD_PRIMARY_KEY,
                ACTION_CREATE_SEQUENCE,
                ACTION_ALTER_SEQUENCE,
                ACTION_DROP_SEQUENCE,
                ACTION_MODIFY_TABLE_AUTO_ID_CACHE,
                ACTION_REBASE_AUTO_RANDOM_BASE,
                ACTION_EXCHANGE_TABLE_PARTITION,
                ACTION_ADD_CHECK_CONSTRAINT,
                ACTION_DROP_CHECK_CONSTRAINT,
                ACTION_ALTER_CHECK_CONSTRAINT,
                ACTION_RENAME_TABLES,
                ACTION_ALTER_TABLE_ATTRIBUTES,
                ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES,
                ACTION_ALTER_TABLE_PARTITION_PLACEMENT,
                ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT,
                ACTION_ALTER_TABLE_PLACEMENT,
                ACTION_ALTER_CACHE_TABLE,
                ACTION_ALTER_TABLE_STATS_OPTIONS,
                ACTION_ALTER_NO_CACHE_TABLE,
                ACTION_MULTI_SCHEMA_CHANGE,
                ACTION_FLASHBACK_CLUSTER,
                ACTION_RECOVER_SCHEMA,
                ACTION_REORGANIZE_PARTITION,
                ACTION_ALTER_TABLE_PARTITIONING,
                ACTION_REMOVE_PARTITIONING,
                ACTION_ADD_COLUMNAR_INDEX,
                ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER,
                ACTION_CREATE_MATERIALIZED_VIEW_SHADOW,
                ACTION_MODIFY_ENGINE_ATTRIBUTE,
                ACTION_ALTER_TABLE_MODE,
                ACTION_REFRESH_META,
                ACTION_MODIFY_SCHEMA_READ_ONLY,
                ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE,
                ACTION_ALTER_TABLE_SOFT_DELETE_INFO,
                ACTION_ALTER_TABLE_SET_REGION_SPLIT_POLICY,
            ],
        ),
        (
            UnmanagementDDL,
            vec![
                ACTION_CREATE_PLACEMENT_POLICY,
                ACTION_ALTER_PLACEMENT_POLICY,
                ACTION_DROP_PLACEMENT_POLICY,
                ACTION_CREATE_MASKING_POLICY,
                ACTION_ALTER_MASKING_POLICY,
                ACTION_DROP_MASKING_POLICY,
                ACTION_CREATE_RESOURCE_GROUP,
                ACTION_ALTER_RESOURCE_GROUP,
                ACTION_DROP_RESOURCE_GROUP,
            ],
        ),
        (
            UnknownDDL,
            vec![DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION],
        ),
    ])
});

/// 反向映射：单个 DDL Action → 其 BDR 安全类别；由 `BDRActionMap` 展平得到。
pub static ActionBDRMap: LazyLock<HashMap<ActionType, DDLBDRType>> = LazyLock::new(|| {
    // 将「类别 → 动作列表」展平为「动作 → 类别」，便于按 ActionType 查询。
    BDRActionMap
        .iter()
        .flat_map(|(kind, actions)| actions.iter().copied().map(|action| (action, *kind)))
        .collect()
});

/// 将 TSO 时间戳的物理毫秒部分转为 UTC `DateTime`。
///
/// TiDB/PD 的 TSO 高 46 位为物理时间（毫秒），右移 18 位即可取出。
pub fn TSConvert2Time(ts: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis((ts >> 18) as i64)
        .expect("TSO physical milliseconds must be a valid UTC timestamp")
}
