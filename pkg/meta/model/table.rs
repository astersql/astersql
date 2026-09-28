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

// 表级元数据模型：表、分区、外键、视图、序列、TTL 与统计配置等。
//
// `TableInfo` 是 schema 中一张表（或视图/序列）的权威描述，字段顺序与 Go JSON 兼容。
// 本文件与 column.rs、index.rs 共享唯一类型身份；兼容 group 仅做 re-export。

// 本文件由 pkg/meta/model/table.go 迁移而来，按原文件顺序保存表、分区、外键及统计配置元数据。
// 它与 column.rs、index.rs 在同一正式模型 crate 中编译，三者共享唯一的类型身份；兼容 group 只做 re-export。

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::LazyLock;
use std::time::Duration;

/// 执行阶段追加到 schema 中、用于占据行句柄位置的隐藏列 ID。
pub const ExtraHandleID: i64 = -1;
/// 返回物理表/分区来源时使用的隐藏列 ID。
pub const ExtraPhysTblID: i64 = -3;
/// 行校验和隐藏列 ID。
pub const ExtraRowChecksumID: i64 = -4;
/// commit timestamp 隐藏列 ID。
pub const ExtraCommitTSID: i64 = -5;

/// TableInfo 元数据格式版本 0。
pub const TableInfoVersion0: u16 = 0;
/// TableInfo 元数据格式版本 1。
pub const TableInfoVersion1: u16 = 1;
/// TableInfo 元数据格式版本 2。
pub const TableInfoVersion2: u16 = 2;
/// TableInfo 元数据格式版本 3。
pub const TableInfoVersion3: u16 = 3;
/// TableInfo 元数据格式版本 4。
pub const TableInfoVersion4: u16 = 4;
/// V5 起 `_tidb_rowid` 与 AUTO_INCREMENT 可使用独立 allocator。
pub const TableInfoVersion5: u16 = 5;
/// 当前最新 TableInfo 版本号。
pub const CurrLatestTableInfoVersion: u16 = TableInfoVersion5;

// Go 使用包级 CIStr 变量；这里保留构造形状，实际初始化方式由 ast 模块接线决定。
/// 隐式行句柄列名 `_tidb_rowid`。
pub static ExtraHandleName: LazyLock<ast::CIStr> = LazyLock::new(|| ast::NewCIStr("_tidb_rowid"));
/// 物理表/分区来源列名 `_tidb_tid`。
pub static ExtraPhysTblIDName: LazyLock<ast::CIStr> = LazyLock::new(|| ast::NewCIStr("_tidb_tid"));
/// commit timestamp 隐藏列名 `_tidb_commit_ts`。
pub static ExtraCommitTSName: LazyLock<ast::CIStr> =
    LazyLock::new(|| ast::NewCIStr("_tidb_commit_ts"));

/// 向量检索距离虚拟列 ID。
pub const VirtualColVecSearchDistanceID: i64 = -2000;
/// 全文检索得分虚拟列 ID。
pub const VirtualColFTSScoreID: i64 = -2050;

/// TableInfo 对应 Go 的表级元数据；字段顺序与 JSON 结构保持一致。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TableInfo {
    /// 表稳定 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 表名。
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
    /// 默认字符集。
    #[serde(rename = "charset")]
    pub Charset: String,
    /// 默认排序规则。
    #[serde(rename = "collate")]
    pub Collate: String,
    /// Columns 按 schema 中的展示顺序保存，Offset 由变更列逻辑维护。
    #[serde(rename = "cols", alias = "columns")]
    pub Columns: Vec<ColumnInfo>,
    /// 索引列表。
    #[serde(rename = "index_info", alias = "indexes")]
    pub Indices: Vec<IndexInfo>,
    /// CHECK 约束列表。
    #[serde(rename = "constraint_info", alias = "constraints")]
    pub Constraints: Vec<ConstraintInfo>,
    /// 外键列表。
    #[serde(rename = "fk_info")]
    pub ForeignKeys: Vec<FKInfo>,
    /// schema 状态。
    #[serde(rename = "state")]
    pub State: SchemaState,
    /// 整数主键是否同时作为行句柄（handle）。
    #[serde(rename = "pk_is_handle")]
    pub PKIsHandle: bool,
    /// 是否使用 common handle（聚簇非整数主键）。
    #[serde(rename = "is_common_handle")]
    pub IsCommonHandle: bool,
    /// common handle 编码版本。
    #[serde(rename = "common_handle_version")]
    pub CommonHandleVersion: u16,
    /// 表注释。
    #[serde(rename = "comment")]
    pub Comment: String,
    /// 自增 ID 水位。
    #[serde(rename = "auto_inc_id")]
    pub AutoIncID: i64,
    /// BR 在分离 allocator 且非聚簇自增表场景保存额外 `_tidb_rowid`。
    #[serde(rename = "auto_inc_id_extra", skip_serializing_if = "is_zero_i64")]
    pub AutoIncIDExtra: i64,
    /// 自增 ID 缓存大小；为 1 时配合 V5 启用分离 allocator。
    #[serde(rename = "auto_id_cache")]
    pub AutoIDCache: i64,
    /// AUTO_RANDOM 水位。
    #[serde(rename = "auto_rand_id")]
    pub AutoRandID: i64,
    /// 已分配最大列 ID。
    #[serde(rename = "max_col_id")]
    pub MaxColumnID: i64,
    /// 已分配最大索引 ID。
    #[serde(rename = "max_idx_id")]
    pub MaxIndexID: i64,
    /// 已分配最大外键 ID。
    #[serde(rename = "max_fk_id")]
    pub MaxForeignKeyID: i64,
    /// 已分配最大约束 ID。
    #[serde(rename = "max_cst_id")]
    pub MaxConstraintID: i64,
    /// 最近更新时间戳（TSO）。
    #[serde(rename = "update_timestamp")]
    pub UpdateTS: u64,
    /// 跨 schema rename 时保存原 schema ID，保证 auto ID 前缀不变。
    #[serde(rename = "old_schema_id", skip_serializing_if = "is_zero_i64")]
    pub AutoIDSchemaID: i64,
    /// 行 ID 分片位数。
    #[serde(rename = "ShardRowIDBits")]
    pub ShardRowIDBits: u64,
    /// 历史最大分片位数。
    #[serde(rename = "max_shard_row_id_bits")]
    pub MaxShardRowIDBits: u64,
    /// AUTO_RANDOM 随机位数。
    #[serde(rename = "auto_random_bits")]
    pub AutoRandomBits: u64,
    /// AUTO_RANDOM 范围位数。
    #[serde(rename = "auto_random_range_bits")]
    pub AutoRandomRangeBits: u64,
    /// 预分裂 Region 数提示。
    #[serde(rename = "pre_split_regions")]
    pub PreSplitRegions: u64,
    /// 分区配置（可选）。
    #[serde(rename = "partition")]
    pub Partition: Option<PartitionInfo>,
    /// 压缩算法名。
    #[serde(rename = "compression")]
    pub Compression: String,
    /// 视图定义（若本对象是视图）。
    #[serde(rename = "view")]
    pub View: Option<ViewInfo>,
    /// 序列定义（若本对象是序列）。
    #[serde(rename = "sequence")]
    pub Sequence: Option<SequenceInfo>,
    /// 表锁信息。
    #[serde(rename = "Lock")]
    pub Lock: Option<TableLockInfo>,
    /// 元数据格式版本。
    #[serde(rename = "version")]
    pub Version: u16,
    /// TiFlash 副本配置。
    #[serde(rename = "tiflash_replica")]
    pub TiFlashReplica: Option<TiFlashReplicaInfo>,
    /// 是否列存表。
    #[serde(rename = "is_columnar")]
    pub IsColumnar: bool,
    /// 临时表类型。
    #[serde(rename = "temp_table_type")]
    pub TempTableType: TempTableType,
    /// 表缓存状态。
    #[serde(rename = "cache_table_status")]
    pub TableCacheStatusType: TableCacheStatusType,
    /// 放置策略引用。
    #[serde(rename = "policy_ref_info")]
    pub PlacementPolicyRef: Option<PolicyRefInfo>,
    /// 统计信息收集选项。
    #[serde(rename = "stats_options")]
    pub StatsOptions: Option<StatsOptions>,
    /// 交换分区中间信息。
    #[serde(rename = "exchange_partition_info")]
    pub ExchangePartitionInfo: Option<ExchangePartitionInfo>,
    /// TTL（Time To Live，按时间过期删行）配置。
    #[serde(rename = "ttl_info")]
    pub TTLInfo: Option<TTLInfo>,
    /// 是否 active-active 相关标记。
    #[serde(
        rename = "is_active_active",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub IsActiveActive: bool,
    /// 软删除配置。
    #[serde(rename = "softdelete_info", skip_serializing_if = "Option::is_none")]
    pub SoftdeleteInfo: Option<SoftdeleteInfo>,
    /// 表亲和性（affinity）配置。
    #[serde(rename = "affinity", skip_serializing_if = "Option::is_none")]
    pub Affinity: Option<TableAffinityInfo>,
    /// Region 分裂策略。
    #[serde(rename = "table_split_policy", skip_serializing_if = "Option::is_none")]
    pub TableSplitPolicy: Option<RegionSplitPolicy>,
    /// 元数据修订号。
    #[serde(rename = "revision")]
    pub Revision: u64,
    /// DBID 在 Go 中不参与 JSON 序列化。
    #[serde(skip)]
    pub DBID: i64,
    /// 表模式（Normal / Import / Restore）。
    #[serde(rename = "mode", skip_serializing_if = "is_default")]
    pub Mode: TableMode,
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

impl std::fmt::Debug for TableInfo {
    /// Debug 仅输出关键字段，避免完整嵌套结构刷屏。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TableInfo")
            .field("ID", &self.ID)
            .field("Name", &self.Name)
            .field("Columns", &self.Columns)
            .field("Indices", &self.Indices)
            .field("PKIsHandle", &self.PKIsHandle)
            .field("Revision", &self.Revision)
            .finish_non_exhaustive()
    }
}

impl TableInfo {
    /// Hash64 对应 Go HashEquals，只把稳定表 ID 写入 hasher。
    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        h.HashInt64(self.ID);
    }

    /// Equals 保留 Go 对动态类型、nil 接收者和表 ID 的比较语义。
    pub fn Equals(&self, other: &dyn std::any::Any) -> bool {
        other
            .downcast_ref::<TableInfo>()
            .is_some_and(|table| self.ID == table.ID)
    }

    /// V5 且 AutoIDCache==1 时，自增与 rowid 使用分离 allocator。
    pub fn SepAutoInc(&self) -> bool {
        self.Version >= TableInfoVersion5 && self.AutoIDCache == 1
    }

    /// 只有启用的分区配置才对调用者可见。
    pub fn GetPartitionInfo(&self) -> Option<&PartitionInfo> {
        self.Partition.as_ref().filter(|partition| partition.Enable)
    }

    /// 将 UpdateTS（TSO）转换为时间。
    pub fn GetUpdateTime(&self) -> time::Time {
        TSConvert2Time(self.UpdateTS)
    }

    /// Clone 对拥有嵌套切片/可选配置的字段执行深拷贝；Rust Clone 承担 Go 手工复制循环。
    pub fn Clone(&self) -> TableInfo {
        Clone::clone(self)
    }

    /// 返回带 PriKey 标志的列名；无则空 CIStr。
    pub fn GetPkName(&self) -> ast::CIStr {
        self.Columns
            .iter()
            .find(|col| mysql::HasPriKeyFlag(col.GetFlag()))
            .map(|col| col.Name.clone())
            .unwrap_or_default()
    }

    /// 返回带 PriKey 标志的列信息。
    pub fn GetPkColInfo(&self) -> Option<&ColumnInfo> {
        self.Columns
            .iter()
            .find(|col| mysql::HasPriKeyFlag(col.GetFlag()))
    }

    /// 返回带 AUTO_INCREMENT 标志的列。
    pub fn GetAutoIncrementColInfo(&self) -> Option<&ColumnInfo> {
        self.Columns
            .iter()
            .find(|col| mysql::HasAutoIncrementFlag(col.GetFlag()))
    }

    /// 自增列是否为无符号类型。
    pub fn IsAutoIncColUnsigned(&self) -> bool {
        self.GetAutoIncrementColInfo()
            .is_some_and(|col| mysql::HasUnsignedFlag(col.GetFlag()))
    }

    /// 是否配置了 AUTO_RANDOM 位数。
    pub fn ContainsAutoRandomBits(&self) -> bool {
        self.AutoRandomBits != 0
    }

    /// AUTO_RANDOM 主键列是否无符号。
    pub fn IsAutoRandomBitColUnsigned(&self) -> bool {
        self.PKIsHandle
            && self.AutoRandomBits != 0
            && self
                .GetPkColInfo()
                .is_some_and(|col| mysql::HasUnsignedFlag(col.GetFlag()))
    }

    /// Cols 模拟 Go 按 Offset 构造 public columns；非 public 列不会进入返回切片。
    pub fn Cols(&self) -> Vec<Option<&ColumnInfo>> {
        let max_offset = self
            .Columns
            .iter()
            .filter(|c| c.State == StatePublic && c.Offset >= 0)
            .map(|c| c.Offset)
            .max();
        let Some(max_offset) = max_offset else {
            return Vec::new();
        };
        // 按 Offset 槽位放置 public 列；中间空洞保留 None 对齐 Go 切片语义。
        let mut columns: Vec<Option<&ColumnInfo>> = vec![None; max_offset as usize + 1];
        for column in self
            .Columns
            .iter()
            .filter(|c| c.State == StatePublic && c.Offset >= 0)
        {
            columns[column.Offset as usize] = Some(column);
        }
        columns
    }

    /// 按小写名查找索引。
    pub fn FindIndexByName(&self, name: &str) -> Option<&IndexInfo> {
        self.Indices.iter().find(|index| index.Name.L == name)
    }

    /// 按列 ID 查找列。
    pub fn FindColumnByID(&self, id: i64) -> Option<&ColumnInfo> {
        self.Columns.iter().find(|column| column.ID == id)
    }

    /// 按索引 ID 查找索引。
    pub fn FindIndexByID(&self, id: i64) -> Option<&IndexInfo> {
        self.Indices.iter().find(|index| index.ID == id)
    }

    /// 在 public 列中按小写名查找。
    pub fn FindPublicColumnByName(&self, name: &str) -> Option<&ColumnInfo> {
        self.Cols()
            .into_iter()
            .flatten()
            .find(|column| column.Name.L == name)
    }

    /// 是否存在持锁会话。
    pub fn IsLocked(&self) -> bool {
        self.Lock
            .as_ref()
            .is_some_and(|lock| !lock.Sessions.is_empty())
    }

    /// MoveColumnInfo 同步维护列 Offset、索引列 Offset 与变更列依赖 Offset。
    pub fn MoveColumnInfo(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let column = self.Columns.remove(from);
        self.Columns.insert(to, column);
        // 重建 Offset 映射并同步索引列与变更依赖。
        let mut changed = HashMap::new();
        for (new_offset, column) in self.Columns.iter_mut().enumerate() {
            let old_offset = column.Offset;
            column.Offset = new_offset as isize;
            changed.insert(old_offset, new_offset as isize);
        }
        for index in &mut self.Indices {
            for column in index
                .Columns
                .iter_mut()
                .chain(index.AffectColumn.iter_mut().flatten())
            {
                if let Some(offset) = changed.get(&column.Offset) {
                    column.Offset = *offset;
                }
            }
        }
        for column in &mut self.Columns {
            if let Some(change) = &mut column.ChangeStateInfo {
                if let Some(offset) = changed.get(&change.DependencyColumnOffset) {
                    change.DependencyColumnOffset = *offset;
                }
            }
        }
    }

    /// 清除表及分区定义上的 placement 引用。
    pub fn ClearPlacement(&mut self) {
        self.PlacementPolicyRef = None;
        if let Some(partition) = &mut self.Partition {
            for definition in &mut partition.Definitions {
                definition.PlacementPolicyRef = None;
            }
        }
    }

    /// 返回显式主键；否则选择首个所有列均 NOT NULL、非隐藏且完整存在的 UNIQUE 索引。
    pub fn GetPrimaryKey(&self) -> Option<&IndexInfo> {
        let columns = self.Cols();
        let mut implicit = None;
        for key in &self.Indices {
            if key.Primary {
                return Some(key);
            }
            if implicit.is_none() && key.Unique && !key.Columns.is_empty() {
                let valid = key.Columns.iter().all(|index_column| {
                    columns
                        .iter()
                        .flatten()
                        .find(|column| column.Name.L == index_column.Name.L)
                        .is_some_and(|column| {
                            !column.Hidden && mysql::HasNotNullFlag(column.GetFlag())
                        })
                });
                if valid {
                    implicit = Some(key);
                }
            }
        }
        implicit
    }

    /// 判断列是否出现在任一索引中。
    pub fn ColumnIsInIndex(&self, column: &ColumnInfo) -> bool {
        self.Indices
            .iter()
            .any(|index| index.Columns.iter().any(|c| c.Name.L == column.Name.L))
    }

    /// 是否为聚簇索引表（整数 handle 或 common handle）。
    pub fn HasClusteredIndex(&self) -> bool {
        self.PKIsHandle || self.IsCommonHandle
    }
    /// 是否为视图。
    pub fn IsView(&self) -> bool {
        self.View.is_some()
    }
    /// 是否为序列对象。
    pub fn IsSequence(&self) -> bool {
        self.Sequence.is_some()
    }
    /// 是否为普通基表（非视图/序列）。
    pub fn IsBaseTable(&self) -> bool {
        self.Sequence.is_none() && self.View.is_none()
    }

    /// 按约束名（小写）查找 CHECK 约束。
    pub fn FindConstraintInfoByName(&self, name: &str) -> Option<&ConstraintInfo> {
        let lower = name.to_lowercase();
        self.Constraints
            .iter()
            .find(|constraint| constraint.Name.L == lower)
    }

    /// 按索引 ID 返回小写名；未找到返回空串。
    pub fn FindIndexNameByID(&self, id: i64) -> String {
        self.FindIndexByID(id)
            .map(|index| index.Name.L.clone())
            .unwrap_or_default()
    }

    /// 按列 ID 返回小写名；未找到返回空串。
    pub fn FindColumnNameByID(&self, id: i64) -> String {
        self.FindColumnByID(id)
            .map(|column| column.Name.L.clone())
            .unwrap_or_default()
    }

    /// 按 ID 查找 StatePublic 列。
    pub fn GetColumnByID(&self, id: i64) -> Option<&ColumnInfo> {
        self.Columns
            .iter()
            .find(|column| column.State == StatePublic && column.ID == id)
    }

    /// 去除 modify-column 生成的临时列，并用 changing 列覆盖其原始列名。
    pub fn GetNonTempColumns(&self) -> Vec<&ColumnInfo> {
        let mut columns: HashMap<&str, &ColumnInfo> = self
            .Columns
            .iter()
            .filter(|column| !column.IsRemoving())
            .map(|column| (column.Name.L.as_str(), column))
            .collect();
        for column in self
            .Columns
            .iter()
            .filter(|column| !column.IsRemoving() && column.IsChanging())
        {
            columns.remove(column.GetChangingOriginName().as_str());
        }
        // Go map 遍历本身无稳定顺序，因此这里也不承诺返回顺序。
        columns.into_values().collect()
    }
}

/// 按外键名在切片中查找。
pub fn FindFKInfoByName<'a>(foreign_keys: &'a [FKInfo], name: &str) -> Option<&'a FKInfo> {
    foreign_keys
        .iter()
        .find(|foreign_key| foreign_key.Name.L == name)
}

/// modify-column 期间旧/新类型共存，索引列显式要求 changing type 时优先返回它。
pub fn GetIdxChangingFieldType<'a>(
    index_column: &IndexColumn,
    column: &'a ColumnInfo,
) -> &'a types::FieldType {
    if index_column.UseChangingType {
        if let Some(field_type) = &column.ChangingFieldType {
            return field_type;
        }
    }
    &column.FieldType
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
/// 仅含 ID/名称的轻量表引用。
pub struct TableNameInfo {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// 表缓存开关状态包装类型。
pub struct TableCacheStatusType(pub i32);
/// 缓存关闭。
pub const TableCacheStatusDisable: TableCacheStatusType = TableCacheStatusType(0);
/// 缓存开启。
pub const TableCacheStatusEnable: TableCacheStatusType = TableCacheStatusType(1);
/// 缓存切换中。
pub const TableCacheStatusSwitching: TableCacheStatusType = TableCacheStatusType(2);
impl TableCacheStatusType {
    /// 返回缓存状态字符串；未知值为空串。
    pub fn String(self) -> &'static str {
        match self.0 {
            0 => "disable",
            1 => "enable",
            2 => "switching",
            _ => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// 临时表类型包装。
pub struct TempTableType(pub u8);
/// 非临时表。
pub const TempTableNone: TempTableType = TempTableType(0);
/// 全局临时表。
pub const TempTableGlobal: TempTableType = TempTableType(1);
/// 会话级临时表。
pub const TempTableLocal: TempTableType = TempTableType(2);
impl TempTableType {
    /// 返回临时表类型字符串。
    pub fn String(self) -> &'static str {
        match self.0 {
            1 => "global",
            2 => "local",
            _ => "",
        }
    }
}

/// TableLockInfo 保存锁类型、持锁会话、状态和加锁时间戳，不负责实际加锁。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TableLockInfo {
    pub Tp: ast::model::TableLockType,
    pub Sessions: Vec<SessionInfo>,
    pub State: TableLockState,
    pub TS: u64,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
/// 持锁会话标识（服务器 ID + 会话 ID）。
pub struct SessionInfo {
    pub ServerID: String,
    pub SessionID: u64,
}
impl SessionInfo {
    /// 格式化为 `server: X_session: Y`。
    pub fn String(&self) -> String {
        format!("server: {}_session: {}", self.ServerID, self.SessionID)
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
/// 表锁请求目标：schema/table/锁类型三元组。
pub struct TableLockTpInfo {
    pub SchemaID: i64,
    pub TableID: i64,
    pub Tp: ast::model::TableLockType,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// 表锁状态机状态。
pub struct TableLockState(pub u8);
/// 无锁。
pub const TableLockStateNone: TableLockState = TableLockState(0);
/// 预加锁。
pub const TableLockStatePreLock: TableLockState = TableLockState(1);
/// 锁已对公共可见。
pub const TableLockStatePublic: TableLockState = TableLockState(2);
impl TableLockState {
    /// 返回锁状态字符串。
    pub fn String(self) -> &'static str {
        match self.0 {
            1 => "pre-lock",
            2 => "public",
            _ => "none",
        }
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
/// TiFlash 列存副本配置与可用性。
pub struct TiFlashReplicaInfo {
    /// 副本数。
    pub Count: u64,
    /// 位置标签。
    pub LocationLabels: Vec<String>,
    /// 整表是否可用。
    pub Available: bool,
    /// 已可用的分区 ID 列表。
    pub AvailablePartitionIDs: Vec<i64>,
}
impl TiFlashReplicaInfo {
    /// 判断指定分区是否已有可用 TiFlash 副本。
    pub fn IsPartitionAvailable(&self, partition_id: i64) -> bool {
        self.AvailablePartitionIDs.contains(&partition_id)
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
/// 视图元数据：算法、定义者、安全、SELECT 文本与列名。
pub struct ViewInfo {
    #[serde(rename = "view_algorithm")]
    pub Algorithm: ast::ViewAlgorithm,
    #[serde(rename = "view_definer")]
    pub Definer: Option<auth::UserIdentity>,
    #[serde(rename = "view_security")]
    pub Security: ast::ViewSecurity,
    #[serde(rename = "view_select")]
    pub SelectStmt: String,
    #[serde(rename = "view_checkoption")]
    pub CheckOption: ast::ViewCheckOption,
    #[serde(rename = "view_cols")]
    pub Cols: Vec<ast::CIStr>,
}

/// 序列 CACHE 选项默认值。
pub const DefaultSequenceCacheBool: bool = true;
/// 序列 CYCLE 选项默认值。
pub const DefaultSequenceCycleBool: bool = false;
/// 序列 ORDER 选项默认值。
pub const DefaultSequenceOrderBool: bool = false;
/// 序列默认缓存个数。
pub const DefaultSequenceCacheValue: i64 = 1000;
/// 序列默认步长。
pub const DefaultSequenceIncrementValue: i64 = 1;
/// 正序列默认起始值。
pub const DefaultPositiveSequenceStartValue: i64 = 1;
/// 负序列默认起始值。
pub const DefaultNegativeSequenceStartValue: i64 = -1;
/// 正序列默认最小值。
pub const DefaultPositiveSequenceMinValue: i64 = 1;
/// 正序列默认最大值。
pub const DefaultPositiveSequenceMaxValue: i64 = 9_223_372_036_854_775_806;
/// 负序列默认最大值。
pub const DefaultNegativeSequenceMaxValue: i64 = -1;
/// 负序列默认最小值。
pub const DefaultNegativeSequenceMinValue: i64 = -9_223_372_036_854_775_807;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 序列对象参数。
pub struct SequenceInfo {
    #[serde(rename = "sequence_start")]
    pub Start: i64,
    #[serde(rename = "sequence_cache")]
    pub Cache: bool,
    #[serde(rename = "sequence_cycle")]
    pub Cycle: bool,
    #[serde(rename = "sequence_min_value")]
    pub MinValue: i64,
    #[serde(rename = "sequence_max_value")]
    pub MaxValue: i64,
    #[serde(rename = "sequence_increment")]
    pub Increment: i64,
    #[serde(rename = "sequence_cache_value")]
    pub CacheValue: i64,
    #[serde(rename = "sequence_comment")]
    pub Comment: String,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 交换分区 DDL 中间信息。
pub struct ExchangePartitionInfo {
    #[serde(rename = "exchange_partition_id")]
    pub ExchangePartitionTableID: i64,
    #[serde(rename = "exchange_partition_def_id")]
    pub ExchangePartitionDefID: i64,
    /// Go 已标记 deprecated，仅为 JSON 向后兼容保留。
    #[serde(rename = "exchange_partition_flag")]
    pub XXXExchangePartitionFlag: bool,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 分区 DDL 期间索引更新描述。
pub struct UpdateIndexInfo {
    #[serde(rename = "index_name")]
    pub IndexName: String,
    #[serde(rename = "global")]
    pub Global: bool,
}

/// PartitionInfo 保存当前分区定义及 DDL 中间态；方法只变换内存元数据，不执行 DDL。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PartitionInfo {
    #[serde(rename = "type")]
    pub Type: ast::PartitionType,
    /// 分区类型（RANGE/LIST/HASH 等）。
    #[serde(rename = "expr")]
    pub Expr: String,
    /// 分区表达式文本。
    #[serde(rename = "columns")]
    pub Columns: Vec<ast::CIStr>,
    /// 分区键列。
    #[serde(rename = "enable")]
    pub Enable: bool,
    /// 是否启用分区。
    #[serde(rename = "is_empty_columns")]
    pub IsEmptyColumns: bool,
    /// 是否空列分区键标记。
    #[serde(rename = "definitions")]
    pub Definitions: Vec<PartitionDefinition>,
    /// 当前分区定义。
    #[serde(rename = "adding_definitions")]
    pub AddingDefinitions: Vec<PartitionDefinition>,
    /// 正在添加的分区定义。
    #[serde(rename = "dropping_definitions")]
    pub DroppingDefinitions: Vec<PartitionDefinition>,
    /// 正在删除的分区定义。
    #[serde(rename = "new_partition_ids", skip_serializing_if = "Vec::is_empty")]
    pub NewPartitionIDs: Vec<i64>,
    /// truncate 产生的新分区 ID。
    #[serde(
        rename = "original_partition_ids_order",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub OriginalPartitionIDsOrder: Vec<i64>,
    /// 原始分区 ID 顺序快照。
    #[serde(rename = "states")]
    pub States: Vec<PartitionState>,
    /// 各分区 schema 状态。
    #[serde(rename = "num")]
    pub Num: u64,
    /// 分区数量提示。
    #[serde(rename = "ddl_action", skip_serializing_if = "is_default")]
    pub DDLAction: ActionType,
    /// 进行中的分区 DDL Action。
    #[serde(rename = "ddl_state")]
    pub DDLState: SchemaState,
    /// 进行中的分区 DDL schema 状态。
    #[serde(rename = "new_table_id", skip_serializing_if = "is_zero_i64")]
    pub NewTableID: i64,
    /// 重组目标新表 ID。
    #[serde(rename = "ddl_type", skip_serializing_if = "is_default")]
    pub DDLType: ast::PartitionType,
    /// DDL 目标分区类型。
    #[serde(rename = "ddl_expr", skip_serializing_if = "String::is_empty")]
    pub DDLExpr: String,
    /// DDL 目标分区表达式。
    #[serde(rename = "ddl_columns", skip_serializing_if = "Vec::is_empty")]
    pub DDLColumns: Vec<ast::CIStr>,
    /// DDL 目标分区列。
    #[serde(rename = "ddl_update_indexes", skip_serializing_if = "Vec::is_empty")]
    pub DDLUpdateIndexes: Vec<UpdateIndexInfo>,
    /// DDL 期间需更新的索引。
    /// bool 区分 global index 的新副本和待删除旧副本。
    #[serde(
        rename = "ddl_changed_index",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub DDLChangedIndex: HashMap<i64, bool>,
}

impl PartitionInfo {
    /// 深拷贝分区信息。
    pub fn Clone(&self) -> PartitionInfo {
        Clone::clone(self)
    }

    /// 按分区 ID 返回原始名；未找到返回空串。
    pub fn GetNameByID(&self, id: i64) -> String {
        self.Definitions
            .iter()
            .find(|definition| definition.ID == id)
            .map(|definition| definition.Name.O.clone())
            .unwrap_or_default()
    }

    /// 按分区 ID 取 schema 状态；缺失视为 Public。
    pub fn GetStateByID(&self, id: i64) -> SchemaState {
        self.States
            .iter()
            .find(|state| state.ID == id)
            .map(|state| state.State)
            .unwrap_or(StatePublic)
    }

    /// 设置或追加分区 schema 状态。
    pub fn SetStateByID(&mut self, id: i64, state: SchemaState) {
        if let Some(old) = self.States.iter_mut().find(|old| old.ID == id) {
            old.State = state;
        } else {
            self.States.push(PartitionState {
                ID: id,
                State: state,
            });
        }
    }

    /// 清理已不在 Definitions 中的状态，防止 DDL 完成后残留不可达分区 ID。
    pub fn GCPartitionStates(&mut self) {
        self.States.retain(|state| {
            self.Definitions
                .iter()
                .any(|definition| definition.ID == state.ID)
        });
    }

    /// 清理重组中间字段，恢复空闲状态。
    pub fn ClearReorgIntermediateInfo(&mut self) {
        self.DDLAction = ActionNone;
        self.DDLState = StateNone;
        self.DDLType = ast::PartitionTypeNone;
        self.DDLExpr.clear();
        self.DDLColumns.clear();
        self.NewTableID = 0;
        self.DDLChangedIndex.clear();
    }

    /// 按分区名查找定义下标；未找到返回 -1。
    pub fn FindPartitionDefinitionByName(&self, name: &str) -> i32 {
        let lower = name.to_lowercase();
        self.Definitions
            .iter()
            .position(|definition| definition.Name.L == lower)
            .map(|i| i as i32)
            .unwrap_or(-1)
    }

    /// 按分区名返回 ID；未找到返回 -1。
    pub fn GetPartitionIDByName(&self, name: &str) -> i64 {
        let lower = name.to_lowercase();
        self.Definitions
            .iter()
            .find(|definition| definition.Name.L == lower)
            .map(|definition| definition.ID)
            .unwrap_or(-1)
    }

    /// LIST 分区中默认分区下标；非 LIST 或无默认返回 -1。
    pub fn GetDefaultListPartition(&self) -> i32 {
        if self.Type != ast::PartitionTypeList {
            return -1;
        }
        self.Definitions
            .iter()
            .position(|definition| {
                definition.InValues.is_empty()
                    || definition
                        .InValues
                        .iter()
                        .any(|values| values.len() == 1 && values[0] == "DEFAULT")
            })
            .map(|i| i as i32)
            .unwrap_or(-1)
    }

    /// 删除分区处于 WriteOnly 时，读路径允许重叠分区回退。
    pub fn CanHaveOverlappingDroppingPartition(&self) -> bool {
        self.DDLAction == ActionDropTablePartition && self.DDLState == StateWriteOnly
    }

    /// 读路径遇到“正在删除”错误时，尝试改用 DDL 下一状态可见的重叠分区。
    /// 写路径不能调用该修正，因为写入正在删除的范围必须被阻止。
    pub fn ReplaceWithOverlappingPartitionIdx(
        &self,
        mut index: i32,
        mut error: Option<errors::Error>,
    ) -> (i32, Option<errors::Error>) {
        if error.is_some() && index >= 0 {
            index = self.GetOverlappingDroppingPartitionIdx(index);
            if index >= 0 {
                error = None;
            }
        }
        (index, error)
    }

    /// 计算与正在删除分区重叠、可读的替代分区下标。
    pub fn GetOverlappingDroppingPartitionIdx(&self, index: i32) -> i32 {
        if index < 0 || index as usize >= self.Definitions.len() {
            return -1;
        }
        if !self.CanHaveOverlappingDroppingPartition() {
            return index;
        }
        // RANGE：向后找第一个未在删除中的分区；LIST：回退到 DEFAULT（若存在且不同）。
        match self.Type {
            ast::PartitionTypeRange => (index as usize..self.Definitions.len())
                .find(|i| !self.IsDropping(*i as i32))
                .map(|i| i as i32)
                .unwrap_or(-1),
            ast::PartitionTypeList => {
                if !self.IsDropping(index) {
                    return index;
                }
                let default_index = self.GetDefaultListPartition();
                if default_index == index {
                    -1
                } else {
                    default_index
                }
            }
            _ => index,
        }
    }

    /// 判断 Definitions[index] 是否在 DroppingDefinitions 中。
    pub fn IsDropping(&self, index: i32) -> bool {
        let id = self.Definitions[index as usize].ID;
        self.DroppingDefinitions
            .iter()
            .any(|definition| definition.ID == id)
    }

    /// 快照当前 Definitions 的 ID 顺序。
    pub fn SetOriginalPartitionIDs(&mut self) {
        self.OriginalPartitionIDsOrder = self
            .Definitions
            .iter()
            .map(|definition| definition.ID)
            .collect();
    }

    /// 返回当前 schema 版本应隐藏的物理 ID，保持 truncate/drop/add 三类 DDL 的状态机分支。
    pub fn IDsInDDLToIgnore(&self) -> Vec<i64> {
        match self.DDLAction {
            ActionTruncateTablePartition if self.DDLState == StateWriteOnly => {
                self.NewPartitionIDs.clone()
            }
            ActionTruncateTablePartition
                if self.DDLState == StateDeleteOnly
                    || self.DDLState == StateDeleteReorganization =>
            {
                self.DroppingDefinitions
                    .iter()
                    .map(|definition| definition.ID)
                    .collect()
            }
            ActionDropTablePartition => self
                .DroppingDefinitions
                .iter()
                .map(|definition| definition.ID)
                .collect(),
            ActionAddTablePartition => self
                .AddingDefinitions
                .iter()
                .map(|definition| definition.ID)
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 单个分区的 schema 状态条目。
pub struct PartitionState {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "state")]
    pub State: SchemaState,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 单个分区定义：边界值、placement 与注释。
pub struct PartitionDefinition {
    /// 分区物理 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 分区名。
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
    /// RANGE 上界表达式列表。
    #[serde(rename = "less_than")]
    pub LessThan: Vec<String>,
    /// LIST 取值列表。
    #[serde(rename = "in_values")]
    pub InValues: Vec<Vec<String>>,
    /// 分区级 placement 引用。
    #[serde(rename = "policy_ref_info")]
    pub PlacementPolicyRef: Option<PolicyRefInfo>,
    /// 分区注释。
    #[serde(rename = "comment", skip_serializing_if = "String::is_empty")]
    pub Comment: String,
}
impl PartitionDefinition {
    /// 深拷贝分区定义。
    pub fn Clone(&self) -> PartitionDefinition {
        Clone::clone(self)
    }

    /// 对应 Go unsafe.Sizeof 估算，只统计结构体、CIStr、policy 引用和字符串内容。
    pub fn MemoryUsage(&self) -> i64 {
        let mut total =
            size_of::<PartitionState>() as i64 + (self.Name.O.len() + self.Name.L.len()) as i64;
        if let Some(policy) = &self.PlacementPolicyRef {
            total += size_of::<i64>() as i64 + (policy.Name.O.len() + policy.Name.L.len()) as i64;
        }
        total
            + self.LessThan.iter().map(|s| s.len() as i64).sum::<i64>()
            + self
                .InValues
                .iter()
                .flatten()
                .map(|s| s.len() as i64)
                .sum::<i64>()
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// CHECK 约束元数据。
pub struct ConstraintInfo {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "constraint_name", alias = "name")]
    pub Name: ast::CIStr,
    #[serde(rename = "tbl_name")]
    pub Table: ast::CIStr,
    #[serde(rename = "constraint_cols")]
    pub ConstraintCols: Vec<ast::CIStr>,
    #[serde(rename = "enforced")]
    pub Enforced: bool,
    #[serde(rename = "in_column")]
    pub InColumn: bool,
    #[serde(rename = "expr_string")]
    pub ExprString: String,
    #[serde(rename = "state")]
    pub State: SchemaState,
}
impl ConstraintInfo {
    pub fn Clone(&self) -> ConstraintInfo {
        Clone::clone(self)
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 外键元数据。
pub struct FKInfo {
    /// 外键 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "fk_name")]
    pub Name: ast::CIStr,
    #[serde(rename = "ref_schema")]
    pub RefSchema: ast::CIStr,
    #[serde(rename = "ref_table")]
    pub RefTable: ast::CIStr,
    /// 外键名。
    #[serde(rename = "ref_cols")]
    pub RefCols: Vec<ast::CIStr>,
    #[serde(rename = "cols")]
    pub Cols: Vec<ast::CIStr>,
    #[serde(rename = "on_delete")]
    pub OnDelete: i32,
    #[serde(rename = "on_update")]
    pub OnUpdate: i32,
    #[serde(rename = "state")]
    pub State: SchemaState,
    #[serde(rename = "version")]
    pub Version: i32,
}
impl FKInfo {
    /// 按 Go 的 quoting 与 refer option 顺序重建 SHOW CREATE TABLE 使用的外键片段。
    pub fn String(&self, database: &str, table: &str) -> String {
        let columns = self
            .Cols
            .iter()
            .map(|column| format!("`{}`", column.O))
            .collect::<Vec<_>>()
            .join(", ");
        let referenced = self
            .RefCols
            .iter()
            .map(|column| format!("`{}`", column.O))
            .collect::<Vec<_>>()
            .join(", ");
        let reference = if self.RefSchema.L != database {
            format!("`{}`.`{}`", self.RefSchema.L, self.RefTable.L)
        } else {
            format!("`{}`", self.RefTable.L)
        };
        let mut result = format!(
            "`{}`.`{}`, CONSTRAINT `{}` FOREIGN KEY ({}) REFERENCES {} ({})",
            database, table, self.Name.O, columns, reference, referenced
        );
        let refer_option = |value| match value {
            1 => "RESTRICT",
            2 => "CASCADE",
            3 => "SET NULL",
            4 => "NO ACTION",
            5 => "SET DEFAULT",
            _ => "",
        };
        let on_delete = refer_option(self.OnDelete);
        if !on_delete.is_empty() {
            result.push_str(&format!(" ON DELETE {on_delete}"));
        }
        let on_update = refer_option(self.OnUpdate);
        if !on_update.is_empty() {
            result.push_str(&format!(" ON UPDATE {on_update}"));
        }
        result
    }
    /// 深拷贝外键。
    pub fn Clone(&self) -> FKInfo {
        Clone::clone(self)
    }
}

/// 外键元数据版本 0。
pub const FKVersion0: i32 = 0;
/// 外键元数据版本 1。
pub const FKVersion1: i32 = 1;

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 被引用侧记录的子表外键回指信息。
pub struct ReferredFKInfo {
    #[serde(rename = "cols")]
    pub Cols: Vec<ast::CIStr>,
    #[serde(rename = "child_schema")]
    pub ChildSchema: ast::CIStr,
    #[serde(rename = "child_table")]
    pub ChildTable: ast::CIStr,
    #[serde(rename = "child_fk_name")]
    pub ChildFKName: ast::CIStr,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
/// 统计加载项的表内对象 ID（列或索引）。
pub struct TableItemID {
    pub TableID: i64,
    pub ID: i64,
    pub IsIndex: bool,
    pub IsSyncLoadFailed: bool,
}
impl TableItemID {
    /// Key 保持 Go 的 `ID#TableID#IsIndex` 顺序；失败标记不属于唯一键。
    pub fn Key(&self) -> String {
        format!("{}#{}#{}", self.ID, self.TableID, self.IsIndex)
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
/// 统计信息加载请求项。
pub struct StatsLoadItem {
    #[serde(flatten)]
    pub TableItemID: TableItemID,
    pub FullLoad: bool,
}
/// 返回含 FullLoad 标记的唯一键。
impl StatsLoadItem {
    pub fn Key(&self) -> String {
        format!("{}#{}", self.TableItemID.Key(), self.FullLoad)
    }
}

#[derive(Clone, Default)]
/// 表级统计收集选项。
pub struct StatsOptions {
    pub StatsWindowSettings: Option<StatsWindowSettings>,
    pub AutoRecalc: bool,
    pub ColumnChoice: ast::ColumnChoice,
    pub ColumnList: Vec<ast::CIStr>,
    pub SampleNum: u64,
    pub SampleRate: f64,
    pub Buckets: u64,
    pub TopN: u64,
    pub Concurrency: usize,
}

#[derive(serde::Serialize)]
struct StatsOptionsRef<'a> {
    #[serde(rename = "window_start", skip_serializing_if = "Option::is_none")]
    window_start: Option<&'a time::Time>,
    #[serde(rename = "window_end", skip_serializing_if = "Option::is_none")]
    window_end: Option<&'a time::Time>,
    #[serde(rename = "repeat_type", skip_serializing_if = "Option::is_none")]
    repeat_type: Option<WindowRepeatType>,
    #[serde(rename = "repeat_interval", skip_serializing_if = "Option::is_none")]
    repeat_interval: Option<usize>,
    #[serde(rename = "auto_recalc")]
    auto_recalc: bool,
    #[serde(rename = "column_choice")]
    column_choice: ast::ColumnChoice,
    #[serde(rename = "column_list")]
    column_list: &'a [ast::CIStr],
    #[serde(rename = "sample_num")]
    sample_num: u64,
    #[serde(rename = "sample_rate")]
    sample_rate: f64,
    #[serde(rename = "buckets")]
    buckets: u64,
    #[serde(rename = "topn")]
    top_n: u64,
    #[serde(rename = "concurrency")]
    concurrency: usize,
}

impl serde::Serialize for StatsOptions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let window = self.StatsWindowSettings.as_ref();
        StatsOptionsRef {
            window_start: window.map(|settings| &settings.WindowStart),
            window_end: window.map(|settings| &settings.WindowEnd),
            repeat_type: window.map(|settings| settings.RepeatType),
            repeat_interval: window.map(|settings| settings.RepeatInterval),
            auto_recalc: self.AutoRecalc,
            column_choice: self.ColumnChoice,
            column_list: &self.ColumnList,
            sample_num: self.SampleNum,
            sample_rate: self.SampleRate,
            buckets: self.Buckets,
            top_n: self.TopN,
            concurrency: self.Concurrency,
        }
        .serialize(serializer)
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct StatsOptionsOwned {
    #[serde(rename = "window_start")]
    window_start: Option<time::Time>,
    #[serde(rename = "window_end")]
    window_end: Option<time::Time>,
    #[serde(rename = "repeat_type")]
    repeat_type: Option<WindowRepeatType>,
    #[serde(rename = "repeat_interval")]
    repeat_interval: Option<usize>,
    #[serde(rename = "auto_recalc")]
    auto_recalc: bool,
    #[serde(rename = "column_choice")]
    column_choice: ast::ColumnChoice,
    #[serde(rename = "column_list")]
    column_list: Vec<ast::CIStr>,
    #[serde(rename = "sample_num")]
    sample_num: u64,
    #[serde(rename = "sample_rate")]
    sample_rate: f64,
    #[serde(rename = "buckets")]
    buckets: u64,
    #[serde(rename = "topn")]
    top_n: u64,
    #[serde(rename = "concurrency")]
    concurrency: usize,
}

impl<'de> serde::Deserialize<'de> for StatsOptions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = StatsOptionsOwned::deserialize(deserializer)?;
        let has_window = value.window_start.is_some()
            || value.window_end.is_some()
            || value.repeat_type.is_some()
            || value.repeat_interval.is_some();
        Ok(Self {
            StatsWindowSettings: has_window.then(|| StatsWindowSettings {
                WindowStart: value.window_start.unwrap_or_default(),
                WindowEnd: value.window_end.unwrap_or_default(),
                RepeatType: value.repeat_type.unwrap_or_default(),
                RepeatInterval: value.repeat_interval.unwrap_or_default(),
            }),
            AutoRecalc: value.auto_recalc,
            ColumnChoice: value.column_choice,
            ColumnList: value.column_list,
            SampleNum: value.sample_num,
            SampleRate: value.sample_rate,
            Buckets: value.buckets,
            TopN: value.top_n,
            Concurrency: value.concurrency,
        })
    }
}

/// 构造默认统计选项（AutoRecalc=true）。
pub fn NewStatsOptions() -> StatsOptions {
    StatsOptions {
        StatsWindowSettings: None,
        AutoRecalc: true,
        ColumnChoice: ast::DefaultChoice,
        ColumnList: Vec::new(),
        SampleNum: 0,
        SampleRate: 0.0,
        Buckets: 0,
        TopN: 0,
        Concurrency: 0,
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 统计收集时间窗口设置。
pub struct StatsWindowSettings {
    #[serde(rename = "window_start")]
    pub WindowStart: time::Time,
    #[serde(rename = "window_end")]
    pub WindowEnd: time::Time,
    #[serde(rename = "repeat_type")]
    pub RepeatType: WindowRepeatType,
    #[serde(rename = "repeat_interval")]
    pub RepeatInterval: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// 统计窗口重复类型。
pub struct WindowRepeatType(pub u8);
/// 不重复。
pub const Never: WindowRepeatType = WindowRepeatType(0);
/// 按天重复。
pub const Day: WindowRepeatType = WindowRepeatType(1);
/// 按周重复。
pub const Week: WindowRepeatType = WindowRepeatType(2);
/// 按月重复。
pub const Month: WindowRepeatType = WindowRepeatType(3);
impl WindowRepeatType {
    /// 返回重复类型名称。
    pub fn String(self) -> &'static str {
        match self.0 {
            0 => "Never",
            1 => "Day",
            2 => "Week",
            3 => "Month",
            _ => "",
        }
    }
}

/// 当前默认 TTL job 间隔。
pub const DefaultTTLJobInterval: &str = "24h";
/// 旧版默认 TTL job 间隔（升级兼容）。
pub const OldDefaultTTLJobInterval: &str = "1h";

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// TTL 配置：过期列、间隔表达式与调度间隔。
pub struct TTLInfo {
    #[serde(rename = "column")]
    pub ColumnName: ast::CIStr,
    #[serde(rename = "interval_expr")]
    pub IntervalExprStr: String,
    /// 为避免 model 与 ast 循环依赖，Go 将 TimeUnitType 存为 int。
    #[serde(rename = "interval_time_unit")]
    pub IntervalTimeUnit: i32,
    #[serde(rename = "enable")]
    pub Enable: bool,
    #[serde(rename = "job_interval")]
    pub JobInterval: String,
}
impl TTLInfo {
    /// 深拷贝 TTL 配置。
    pub fn Clone(&self) -> TTLInfo {
        Clone::clone(self)
    }

    /// 空值表示从 6.5 时代升级的旧表，此时必须返回旧默认值 1h 保持升级链兼容。
    // / Go failpoint 可覆盖结果；不启用 failpoint，只保留解析错误向上传播。
    pub fn GetJobInterval(&self) -> Result<Duration, errors::Error> {
        let interval = if self.JobInterval.is_empty() {
            OldDefaultTTLJobInterval
        } else {
            &self.JobInterval
        };
        duration::ParseDuration(interval)
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 软删除配置。
pub struct SoftdeleteInfo {
    #[serde(rename = "retention", skip_serializing_if = "String::is_empty")]
    pub Retention: String,
    #[serde(rename = "job_enable", skip_serializing_if = "std::ops::Not::not")]
    pub JobEnable: bool,
    #[serde(rename = "job_interval", skip_serializing_if = "String::is_empty")]
    pub JobInterval: String,
}
/// 深拷贝软删除配置。
impl SoftdeleteInfo {
    pub fn Clone(&self) -> SoftdeleteInfo {
        Clone::clone(self)
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
/// 表亲和性：调度时尽量将相关 Region 放在一起。
pub struct TableAffinityInfo {
    #[serde(rename = "level")]
    pub Level: String,
}

/// 规范化 affinity level；`none`/空字符串映射为 None，非法值保留 Go 的错误分支。
pub fn NewTableAffinityInfoWithLevel(
    level: &str,
) -> Result<Option<TableAffinityInfo>, errors::Error> {
    let normalized = ast::NormalizeTableAffinityLevel(level)
        .ok_or_else(|| errors::Errorf(format!("invalid table affinity level: '{}'", level)))?;
    if normalized == ast::TableAffinityLevelNone {
        return Ok(None);
    }
    Ok(Some(TableAffinityInfo { Level: normalized }))
}

/// 深拷贝亲和性配置。
impl TableAffinityInfo {
    pub fn Clone(&self) -> TableAffinityInfo {
        Clone::clone(self)
    }
}
