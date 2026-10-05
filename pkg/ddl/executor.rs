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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// DDL（Data Definition Language，数据定义语言）执行器模块。
//
// 本模块是 DDL 语句（CREATE/ALTER/DROP 等）的执行入口，职责包括：
// - 定义 DDL 相关的元数据结构：库（Schema）、表（Table）、列（Column）、
//   索引（Index）、外键（Foreign Key）、分区（Partition）等信息；
// - 定义 DDL 作业（Job）模型：每条 DDL 语句会被封装为一个 `DdlJob`，
//   提交到作业队列后由后台异步执行，执行器轮询作业状态直至完成；
// - 提供 `Executor` 类型，实现建库、建表、加列、加索引、分区管理、
//   表锁、TiFlash（列存副本）设置等具体 DDL 操作的校验与元数据变更；
// - 提供标识符、字符集/排序规则、表定义等通用校验函数，以及
//   `ADMIN REPAIR TABLE`（修复损坏表元数据）相关的辅助能力。
//
// 术语说明：DDL 在分布式数据库中通常采用"在线异步变更"（Online DDL）模型，
// 即元数据按状态机多阶段推进（见 `ObjectState`），避免长时间锁表。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

/// 表达式索引（对表达式建立的索引）隐藏列名的前缀。
pub const EXPRESSION_INDEX_PREFIX: &str = "_V$";
/// 表示"表不存在"的特殊表 ID。
pub const TABLE_NOT_EXIST: i64 = -1;
/// 默认放置策略（Placement Policy，控制数据副本放置位置）的名称。
pub const DEFAULT_PLACEMENT_POLICY_NAME: &str = "default";
/// 等待 TiFlash 副本同步的待处理表数量上限。
pub const TIFLASH_PENDING_TABLE_LIMIT: u32 = 100;
/// TiFlash 待处理表检查的重试次数。
pub const TIFLASH_PENDING_TABLE_RETRY: u32 = 7;

const COLUMNAR_STORE_TYPE_OVERRIDE: &str = "cse.columnar-store-type";

fn columnar_store_type(session: &SessionContext) -> String {
    session
        .system_vars
        .get(COLUMNAR_STORE_TYPE_OVERRIDE)
        .cloned()
        .unwrap_or_else(|| {
            astersql_config::get_global_config()
                .cse
                .columnar_store_type
                .clone()
        })
}

fn check_columnar_storage_enabled(session: &SessionContext) -> Result<(), ExecutorError> {
    let store_type = columnar_store_type(session);
    let columnar_enabled = matches!(store_type.as_str(), "columnar" | "both");
    if !columnar_enabled {
        return Ok(());
    }
    let value = session
        .system_vars
        .get(astersql_sessionctx_vardef::TiDBColumnarStorageEnabled)
        .map(String::as_str)
        .unwrap_or(astersql_sessionctx_vardef::On);
    if astersql_sessionctx_variable::TiDBOptOn(value) {
        Ok(())
    } else {
        Err(ExecutorError::Unsupported(format!(
            "`set TiFlash replica` because Columnar Storage is not enabled for cluster default (tidb_columnar_storage_enabled={value:?})"
        )))
    }
}

fn check_columnar_storage_for_replica(
    session: &SessionContext,
    count: u64,
    skip_gate: bool,
) -> Result<(), ExecutorError> {
    if count == 0 || skip_gate {
        Ok(())
    } else {
        check_columnar_storage_enabled(session)
    }
}

fn wrap_columnar_index_gate(error: ExecutorError) -> ExecutorError {
    match error {
        ExecutorError::Unsupported(message)
            if message.contains("Columnar Storage is not enabled") =>
        {
            ExecutorError::Unsupported(
                "Unsupported add columnar index: Columnar Storage is not enabled".into(),
            )
        }
        error => error,
    }
}

fn check_columnar_storage_for_job(
    session: &SessionContext,
    action: &DdlAction,
    args: &BTreeMap<String, String>,
) -> Result<(), ExecutorError> {
    match action {
        DdlAction::SetTiFlashReplica => check_columnar_storage_for_replica(
            session,
            args.get("replica_count")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            args.get("skip_columnar_storage_gate")
                .is_some_and(|value| value == "true"),
        ),
        DdlAction::CreateTable => check_columnar_storage_for_replica(
            session,
            args.get("tiflash_replica_count")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            false,
        )
        .map_err(|error| {
            if args
                .get("columnar_index")
                .is_some_and(|value| value == "true")
            {
                wrap_columnar_index_gate(error)
            } else {
                error
            }
        }),
        DdlAction::AddIndex
            if args
                .get("columnar_index")
                .is_some_and(|value| value == "true") =>
        {
            check_columnar_storage_enabled(session).map_err(wrap_columnar_index_gate)
        }
        _ => Ok(()),
    }
}

/// 限定表名标识符，由"库名 + 表名"组成，例如 `db.tbl`。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ident {
    /// 库（schema/database）名。
    pub schema: String,
    /// 表名。
    pub table: String,
}

impl Ident {
    /// 构造一个新的库表标识符。
    pub fn new(schema: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            table: table.into(),
        }
    }

    /// 返回大小写不敏感的唯一键（`库名.表名` 全小写），用于查找与去重。
    pub fn key(&self) -> String {
        format!(
            "{}.{}",
            self.schema.to_ascii_lowercase(),
            self.table.to_ascii_lowercase()
        )
    }
}

/// 创建对象时若目标已存在的处理策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExist {
    /// 报错（默认行为）。
    Error,
    /// 忽略，相当于 `IF NOT EXISTS`。
    Ignore,
    /// 替换已有对象，相当于 `OR REPLACE`。
    Replace,
}

/// 模式对象（列、索引等）在 Online DDL 状态机中的可见性状态。
///
/// Online DDL 通过多阶段状态推进（None -> DeleteOnly -> WriteOnly ->
/// WriteReorganization -> Public）保证集群中不同节点在相邻两个
/// 元数据版本之间仍能正确读写，避免全局锁表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectState {
    /// 对象尚不存在或刚创建，不可见。
    None,
    /// 仅删除可见：只对删除操作生效。
    DeleteOnly,
    /// 删除重组阶段：正在清理已有数据。
    DeleteReorganization,
    /// 仅写入可见：写操作会维护该对象，但读不可见。
    WriteOnly,
    /// 写重组阶段：正在回填（backfill）历史数据。
    WriteReorganization,
    /// 公开状态：对所有读写完全可见。
    Public,
    /// 副本准备阶段：分区已建立但副本尚未全部就绪。
    ReplicaOnly,
}

/// DDL 作业（Job）的生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    /// 初始状态，尚未入队。
    None,
    /// 已入队等待执行。
    Queueing,
    /// 正在执行。
    Running,
    /// 正在暂停中。
    Pausing,
    /// 已暂停。
    Paused,
    /// 正在回滚（例如作业被取消后撤销已做的变更）。
    RollingBack,
    /// 回滚完成。
    RollbackDone,
    /// 已取消。
    Cancelled,
    /// 已完成且元数据变更已同步到全集群（终态）。
    Synced,
}

/// 列的数据类型分类（简化后的类型枚举）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    /// 有符号整数。
    Integer,
    /// 无符号整数。
    UnsignedInteger,
    /// 字符串类型（CHAR/VARCHAR 等）。
    String,
    /// 二进制大对象（BLOB/TEXT 等）。
    Blob,
    /// 日期。
    Date,
    /// 日期时间。
    DateTime,
    /// 时间戳。
    Timestamp,
    /// 定点小数。
    Decimal,
    /// JSON 类型。
    Json,
    /// 向量类型（用于向量检索）。
    Vector,
}

/// 列的元数据信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    /// 列 ID（表内唯一，0 表示尚未分配）。
    pub id: i64,
    /// 列名。
    pub name: String,
    /// 列的数据类型分类。
    pub kind: ColumnKind,
    /// 字符集，例如 utf8mb4。
    pub charset: String,
    /// 排序规则（collation），决定字符串比较与排序方式。
    pub collation: String,
    /// 是否允许 NULL。
    pub nullable: bool,
    /// 是否为隐藏列（如表达式索引生成的内部列）。
    pub hidden: bool,
    /// 生成列（generated column）表达式所依赖的其他列名集合。
    pub generated_dependencies: BTreeSet<String>,
    /// 绑定的脱敏策略（masking policy）ID，用于查询时对敏感数据脱敏。
    pub masking_policy: Option<i64>,
}

impl ColumnInfo {
    /// 快捷构造一个可空的整数列，字符集/排序规则为 binary。
    pub fn integer(name: impl Into<String>) -> Self {
        Self {
            id: 0,
            name: name.into(),
            kind: ColumnKind::Integer,
            charset: "binary".into(),
            collation: "binary".into(),
            nullable: true,
            hidden: false,
            generated_dependencies: BTreeSet::new(),
            masking_policy: None,
        }
    }
}

/// 索引的元数据信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    /// 索引 ID（0 表示尚未分配）。
    pub id: i64,
    /// 索引名。
    pub name: String,
    /// 索引覆盖的列名列表（有序）。
    pub columns: Vec<String>,
    /// 是否为唯一索引。
    pub unique: bool,
    /// 是否为主键索引。
    pub primary: bool,
    /// 是否为不可见索引（优化器不使用，但仍维护）。
    pub invisible: bool,
    /// 是否为全局索引（跨分区的索引，仅分区表可用）。
    pub global: bool,
    /// 是否为向量索引（用于近似最近邻检索）。
    pub vector: bool,
    /// 索引在 Online DDL 状态机中的当前状态。
    pub state: ObjectState,
    /// Region（数据分片调度单元）切分策略，用于避免写热点。
    pub split_policy: Option<String>,
}

impl IndexInfo {
    /// 构造一个普通（非唯一、非主键）索引定义。
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            id: 0,
            name: name.into(),
            columns,
            unique: false,
            primary: false,
            invisible: false,
            global: false,
            vector: false,
            state: ObjectState::None,
            split_policy: None,
        }
    }
}

/// 外键（Foreign Key）约束的元数据，描述本表列到被引用表列的引用关系。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeyInfo {
    /// 外键约束名。
    pub name: String,
    /// 本表参与外键的列名列表。
    pub columns: Vec<String>,
    /// 被引用（父）表的标识符。
    pub referenced: Ident,
    /// 被引用表中对应的列名列表。
    pub referenced_columns: Vec<String>,
}

impl ForeignKeyInfo {
    /// 构造一个外键约束定义。
    pub fn new(
        name: impl Into<String>,
        columns: Vec<String>,
        referenced: Ident,
        referenced_columns: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            columns,
            referenced,
            referenced_columns,
        }
    }
}

/// 分区定义。分区表将数据按规则拆分到多个物理分区，便于管理与裁剪。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionDefinition {
    /// 分区的物理 ID（0 表示尚未分配）。
    pub id: i64,
    /// 分区名。
    pub name: String,
    /// RANGE 分区的上界表达式列表（`VALUES LESS THAN (...)`）。
    pub less_than: Vec<String>,
    /// 分区级别的放置策略（覆盖表级设置）。
    pub placement_policy: Option<String>,
}

impl PartitionDefinition {
    /// 构造一个 RANGE 分区定义。
    pub fn new(name: impl Into<String>, less_than: Vec<String>) -> Self {
        Self {
            id: 0,
            name: name.into(),
            less_than,
            placement_policy: None,
        }
    }
}

/// 表的完整元数据信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    /// 表 ID（全局唯一，0 表示尚未分配）。
    pub id: i64,
    /// 所属库的 ID。
    pub schema_id: i64,
    /// 表名。
    pub name: String,
    /// 表默认字符集。
    pub charset: String,
    /// 表默认排序规则。
    pub collation: String,
    /// 列定义列表。
    pub columns: Vec<ColumnInfo>,
    /// 索引定义列表。
    pub indexes: Vec<IndexInfo>,
    /// 外键约束列表。
    pub foreign_keys: Vec<ForeignKeyInfo>,
    /// 分区定义列表（空表示非分区表）。
    pub partitions: Vec<PartitionDefinition>,
    /// 自增 ID 的当前基值（AUTO_INCREMENT）。
    pub auto_increment: i64,
    /// AUTO_RANDOM 的随机位数（主键高位随机化打散写热点）。
    pub auto_random_bits: u8,
    /// SHARD_ROW_ID_BITS：隐式行 ID 的分片位数，用于打散写入。
    pub shard_row_id_bits: u8,
    /// 历史上使用过的最大分片位数（只能增大不能减小）。
    pub max_shard_row_id_bits: u8,
    /// 表注释。
    pub comment: String,
    /// 是否为临时表。
    pub temporary: bool,
    /// 是否为视图。
    pub view: bool,
    /// 是否为序列（SEQUENCE 对象）。
    pub sequence: bool,
    /// 是否开启表缓存（整表缓存到内存以加速读取）。
    pub cached: bool,
    /// TiFlash 列存副本数量（0 表示无列存副本）。
    pub tiflash_replica_count: u64,
    /// 已完成 TiFlash 同步的物理表/分区 ID 集合。
    pub tiflash_available_ids: BTreeSet<i64>,
    /// 表级放置策略名。
    pub placement_policy: Option<String>,
    /// 亲和性（affinity）级别，控制数据在存储节点上的聚集方式。
    pub affinity: Option<String>,
    /// TTL（Time To Live，数据过期自动删除）依据的时间列名。
    pub ttl_column: Option<String>,
    /// 当前持有的表锁类型（`LOCK TABLES` 语句设置）。
    pub table_lock: Option<TableLockType>,
}

impl TableInfo {
    /// 用给定列构造一张默认配置的表（utf8mb4 字符集、无索引/分区）。
    pub fn new(name: impl Into<String>, columns: Vec<ColumnInfo>) -> Self {
        Self {
            id: 0,
            schema_id: 0,
            name: name.into(),
            charset: "utf8mb4".into(),
            collation: "utf8mb4_bin".into(),
            columns,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            partitions: Vec::new(),
            auto_increment: 0,
            auto_random_bits: 0,
            shard_row_id_bits: 0,
            max_shard_row_id_bits: 0,
            comment: String::new(),
            temporary: false,
            view: false,
            sequence: false,
            cached: false,
            tiflash_replica_count: 0,
            tiflash_available_ids: BTreeSet::new(),
            placement_policy: None,
            affinity: None,
            ttl_column: None,
            table_lock: None,
        }
    }
}

/// 库（Schema/Database）的元数据信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaInfo {
    /// 库 ID。
    pub id: i64,
    /// 库名。
    pub name: String,
    /// 库默认字符集。
    pub charset: String,
    /// 库默认排序规则。
    pub collation: String,
    /// 库级放置策略名。
    pub placement_policy: Option<String>,
    /// 库内的表集合，键为小写表名。
    pub tables: BTreeMap<String, TableInfo>,
}

/// 表锁类型（对应 `LOCK TABLES ... READ/WRITE`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableLockType {
    /// 读锁：多个会话可同时持有，均只能读。
    Read,
    /// 写锁：独占，持有者可读写，其他会话不可访问。
    Write,
    /// 本地写锁：持有者可写，其他会话仍可读。
    WriteLocal,
}

/// DDL 作业的操作类型，枚举了执行器支持的所有 DDL 动作。
///
/// 大致分类：库操作、表/视图操作、列操作、索引操作、外键操作、
/// 分区操作、表属性调整（自增基值、字符集、TiFlash、TTL、放置策略等）、
/// 序列/策略/资源组管理，以及多模式变更（一条 ALTER 含多个子操作）等。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlAction {
    CreateSchema,
    ModifySchemaCharset,
    ModifySchemaPlacement,
    SetSchemaTiFlash,
    DropSchema,
    RecoverSchema,
    CreateTable,
    CreateView,
    DropTable,
    DropView,
    RecoverTable,
    TruncateTable,
    RenameTable,
    AddColumn,
    DropColumn,
    ModifyColumn,
    SetDefaultValue,
    AddIndex,
    AddPrimaryKey,
    DropIndex,
    DropPrimaryKey,
    RenameIndex,
    AlterIndexVisibility,
    AddForeignKey,
    DropForeignKey,
    AddPartition,
    DropPartition,
    TruncatePartition,
    ReorganizePartition,
    CoalescePartition,
    ExchangePartition,
    AlterPartitioning,
    RemovePartitioning,
    RebaseAutoId,
    ShardRowId,
    ModifyComment,
    ModifyCharset,
    SetTiFlashReplica,
    UpdateTiFlashReplicaStatus,
    SetTtl,
    RemoveTtl,
    SetAffinity,
    SetPlacement,
    SetAttributes,
    CacheTable,
    NoCacheTable,
    LockTable,
    UnlockTable,
    RepairTable,
    CreateSequence,
    AlterSequence,
    DropSequence,
    CreatePlacementPolicy,
    AlterPlacementPolicy,
    DropPlacementPolicy,
    CreateResourceGroup,
    AlterResourceGroup,
    DropResourceGroup,
    CreateMaskingPolicy,
    AlterMaskingPolicy,
    DropMaskingPolicy,
    AddCheckConstraint,
    DropCheckConstraint,
    AlterCheckConstraint,
    FlashbackCluster,
    RefreshMeta,
    MultiSchemaChange,
    SetRegionSplitPolicy,
}

/// 一个 DDL 作业。每条 DDL 语句都会被封装为作业提交给后端异步执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdlJob {
    /// 作业 ID（由后端分配，0 表示尚未分配）。
    pub id: i64,
    /// 涉及的库 ID。
    pub schema_id: i64,
    /// 涉及的表 ID。
    pub table_id: i64,
    /// 作业执行的 DDL 操作类型。
    pub action: DdlAction,
    /// 作业当前状态。
    pub state: JobState,
    /// 作业当前的模式对象状态；drop 类作业据此判断能否回滚。
    pub schema_state: ObjectState,
    /// 多模式变更是否仍处于可回滚阶段。
    pub multi_schema_revertible: bool,
    /// 触发该作业的原始 SQL 语句。
    pub query: String,
    /// 作业失败时的错误信息。
    pub error: Option<String>,
    /// 执行过程产生的警告：错误码 -> (消息, 出现次数)。
    pub warnings: BTreeMap<u16, (String, u64)>,
    /// 作业完成后对应的元数据版本号（schema version）。
    pub schema_version: u64,
    /// 作业涉及的其他库表对象（用于依赖检查与冲突控制）。
    pub involving_schema: Vec<Ident>,
    /// 附加参数（键值对形式，如截断表时的新表 ID）。
    pub args: BTreeMap<String, String>,
}

impl DdlJob {
    /// 判断作业当前是否可以回滚（即取消后能撤销已做的变更）。
    pub fn is_rollbackable(&self) -> bool {
        match self.action {
            DdlAction::DropIndex | DdlAction::DropPrimaryKey => !matches!(
                self.schema_state,
                ObjectState::DeleteOnly
                    | ObjectState::DeleteReorganization
                    | ObjectState::WriteOnly
            ),
            DdlAction::ModifyColumn => self.schema_state != ObjectState::Public,
            DdlAction::AddPartition => matches!(
                self.schema_state,
                ObjectState::None | ObjectState::ReplicaOnly
            ),
            DdlAction::DropColumn
            | DdlAction::DropSchema
            | DdlAction::DropTable
            | DdlAction::DropSequence
            | DdlAction::DropForeignKey
            | DdlAction::DropPartition => self.schema_state == ObjectState::Public,
            DdlAction::TruncatePartition => matches!(
                self.schema_state,
                ObjectState::Public | ObjectState::WriteOnly
            ),
            DdlAction::RebaseAutoId
            | DdlAction::ShardRowId
            | DdlAction::TruncateTable
            | DdlAction::AddForeignKey
            | DdlAction::RenameTable
            | DdlAction::ModifyCharset
            | DdlAction::ModifySchemaCharset
            | DdlAction::RepairTable
            | DdlAction::ModifySchemaPlacement
            | DdlAction::DropCheckConstraint => self.schema_state == ObjectState::None,
            DdlAction::MultiSchemaChange => self.multi_schema_revertible,
            DdlAction::FlashbackCluster => !matches!(
                self.schema_state,
                ObjectState::WriteReorganization | ObjectState::WriteOnly
            ),
            DdlAction::ReorganizePartition
            | DdlAction::RemovePartitioning
            | DdlAction::AlterPartitioning => self.schema_state != ObjectState::Public,
            _ => true,
        }
    }
}

/// DDL 作业队列：按作业 ID 有序存放待处理的作业。
#[derive(Default)]
pub struct DdlJobQueue {
    /// 作业 ID -> 作业，BTreeMap 保证按 ID 升序遍历。
    jobs: BTreeMap<i64, DdlJob>,
}

impl DdlJobQueue {
    /// 向队列添加作业；作业 ID 重复时返回错误。
    pub fn add(&mut self, job: DdlJob) -> Result<(), ExecutorError> {
        if self.jobs.contains_key(&job.id) {
            return Err(ExecutorError::JobSubmit(format!(
                "DDL job {} already exists",
                job.id
            )));
        }
        self.jobs.insert(job.id, job);
        Ok(())
    }

    /// 返回队列中所有作业的引用（按 ID 升序）。
    pub fn all(&self) -> Vec<&DdlJob> {
        self.jobs.values().collect()
    }

    /// 按 ID 升序遍历作业，访问器返回 true 时提前终止。
    pub fn iter_until<F>(&self, mut visitor: F)
    where
        F: FnMut(&DdlJob) -> bool,
    {
        for job in self.jobs.values() {
            if visitor(job) {
                break;
            }
        }
    }
}

/// 作业提交结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitResult {
    /// 后端分配的作业 ID。
    pub job_id: i64,
    /// 是否被合并进了已有作业（批量提交优化）。
    pub merged: bool,
}

/// DDL 执行器可能产生的错误类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutorError {
    SchemaExists(String),
    SchemaNotFound(String),
    TableExists(String),
    TableNotFound(String),
    ColumnExists(String),
    ColumnNotFound(String),
    IndexExists(String),
    IndexNotFound(String),
    ForeignKeyExists(String),
    ForeignKeyNotFound(String),
    PartitionNotFound(String),
    InvalidIdentifier(String),
    InvalidCharsetCollation,
    InvalidTableDefinition(String),
    InvalidPartition(String),
    InvalidAutoId,
    InvalidShardBits,
    InvisiblePrimaryKey,
    GlobalIndexNeedsPartition,
    Unsupported(String),
    /// 作业提交到后端失败。
    JobSubmit(String),
    /// 作业执行失败（后端返回错误）。
    JobFailed(String),
    /// 作业被系统自动暂停（如磁盘空间不足）。
    JobAutoPaused {
        id: i64,
        reason: String,
    },
    /// 作业被用户取消。
    Cancelled,
    /// 当前 SQL 被 KILL QUERY 中断（MySQL 1317）。
    QueryInterrupted,
    /// 等待作业完成超时。
    Timeout,
    /// 列数超过上限（1017）。
    TooManyColumns,
    /// 不能删除表中最后一个可见列。
    LastVisibleColumn,
    /// 对象被其他对象依赖（索引、生成列、TTL、外键等）无法删除。
    Dependency(String),
    /// 表锁或只读模式冲突。
    LockConflict,
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ExecutorError {}

/// DDL 作业后端抽象：负责作业的提交、状态查询与取消。
///
/// 真实实现通常将作业持久化并由后台 worker 异步执行；
/// 执行器通过轮询 `history_job`/`current_job` 感知作业进度。
pub trait JobBackend {
    /// 提交作业，返回分配的作业 ID 等结果。
    fn submit(&mut self, job: &mut DdlJob) -> Result<SubmitResult, String>;
    /// 查询已完成（进入历史）的作业。
    fn history_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String>;
    /// 查询仍在执行中的作业。
    fn current_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String>;
    /// 取消指定作业。
    fn cancel(&mut self, job_id: i64) -> Result<(), CancelJobError>;
}

/// 系统取消 DDL 作业时的逐作业结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancelJobError {
    /// 作业已经完成，取消命令无需重试。
    Finished,
    /// 作业已经进入不可取消阶段，取消命令无需重试。
    CannotCancel,
    /// 作业已经不在当前队列中，取消命令无需重试。
    NotFound,
    /// 临时错误；保留会话中的作业 ID 并重试取消。
    Temporary(String),
}

/// 与 Go `isRetryableDDLCancelErr` 一致：三个明确终态不重试，其余错误重试。
pub fn is_retryable_ddl_cancel_err(error: &CancelJobError) -> bool {
    !matches!(
        error,
        CancelJobError::Finished | CancelJobError::CannotCancel | CancelJobError::NotFound
    )
}

/// 内存版作业后端：提交后立即同步完成，主要用于测试。
#[derive(Default)]
pub struct MemoryJobBackend {
    /// 作业 ID 分配器。
    next_job_id: i64,
    /// 执行中的作业。
    current: BTreeMap<i64, DdlJob>,
    /// 已完成的历史作业。
    history: BTreeMap<i64, DdlJob>,
}

impl MemoryJobBackend {
    /// 返回全部历史作业（按 ID 升序）。
    pub fn history(&self) -> Vec<&DdlJob> {
        self.history.values().collect()
    }
}

impl JobBackend for MemoryJobBackend {
    // 内存实现：分配 ID 后立刻把作业标记为 Synced 并移入历史。
    fn submit(&mut self, job: &mut DdlJob) -> Result<SubmitResult, String> {
        self.next_job_id += 1;
        job.id = self.next_job_id;
        self.current.insert(job.id, job.clone());
        let mut finished = job.clone();
        finished.state = JobState::Synced;
        finished.schema_version = job.id as u64;
        self.current.remove(&job.id);
        self.history.insert(job.id, finished);
        Ok(SubmitResult {
            job_id: job.id,
            merged: false,
        })
    }

    fn history_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String> {
        Ok(self.history.get(&job_id).cloned())
    }

    fn current_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String> {
        Ok(self.current.get(&job_id).cloned())
    }

    fn cancel(&mut self, job_id: i64) -> Result<(), CancelJobError> {
        let Some(mut job) = self.current.remove(&job_id) else {
            return Ok(());
        };
        job.state = JobState::Cancelled;
        self.history.insert(job_id, job);
        Ok(())
    }
}

/// 会话上下文：保存执行 DDL 时所需的会话级状态。
#[derive(Clone, Debug)]
pub struct SessionContext {
    /// 当前正在执行的 SQL 语句文本。
    pub query: String,
    /// 客户端连接 ID。
    pub connection_id: u64,
    /// 会话是否已被 kill（用于中断等待中的 DDL）。
    pub killed: bool,
    /// 测试 failpoint 对应的批量 TiFlash 提前结束；与 KILL 不同，不返回错误。
    pub batch_tiflash_abort: bool,
    /// 服务是否正在关闭。
    pub shutting_down: bool,
    /// 当前会话正在等待的 DDL 作业 ID。
    pub ddl_job_id: Option<i64>,
    /// 累积的警告信息。
    pub warnings: Vec<String>,
    /// 累积的提示（note）信息。
    pub notes: Vec<String>,
    /// 会话可见的系统变量（如 sql_mode、time_zone）。
    pub system_vars: BTreeMap<String, String>,
    /// 会话持有的表锁：表 ID -> 锁类型。
    pub locked_tables: BTreeMap<i64, TableLockType>,
    /// 若处于多模式变更（一条 ALTER 多个子操作）收集阶段，
    /// 子操作会先记录到这里而不立即提交作业。
    pub multi_schema_actions: Option<Vec<DdlAction>>,
    /// 最近一次完成的 DDL 语句文本。
    pub last_ddl_query: String,
    /// 最近一次完成的 DDL 对应的元数据版本号。
    pub last_ddl_sequence: u64,
}

impl Default for SessionContext {
    fn default() -> Self {
        Self {
            query: String::new(),
            connection_id: 0,
            killed: false,
            batch_tiflash_abort: false,
            shutting_down: false,
            ddl_job_id: None,
            warnings: Vec::new(),
            notes: Vec::new(),
            system_vars: BTreeMap::new(),
            locked_tables: BTreeMap::new(),
            multi_schema_actions: None,
            last_ddl_query: String::new(),
            last_ddl_sequence: 0,
        }
    }
}

/// DDL 执行器：维护库表元数据，并把 DDL 变更封装为作业提交到后端。
///
/// 泛型参数 `B` 是作业后端实现（测试中常用 `MemoryJobBackend`）。
pub struct Executor<B: JobBackend> {
    /// 作业后端。
    backend: B,
    /// 全部库的元数据，键为小写库名。
    pub schemas: BTreeMap<String, SchemaInfo>,
    /// 全局 ID 分配器（库/表/索引/分区共用）。
    next_id: i64,
    /// 元数据租约（lease）：节点缓存元数据的有效期，
    /// 用于限制轮询作业状态的最大等待间隔。
    lease: Duration,
    /// 正在等待 TiFlash 副本同步的表数量。
    pub pending_tiflash_tables: u32,
    /// 当前已知的最新元数据版本号。
    pub schema_version: u64,
    /// 已完成作业的通知队列（作业 ID）。
    done_notifications: VecDeque<i64>,
}

impl<B: JobBackend> Executor<B> {
    /// 用给定后端与租约时长创建执行器。
    pub fn new(backend: B, lease: Duration) -> Self {
        Self {
            backend,
            schemas: BTreeMap::new(),
            next_id: 0,
            lease,
            pending_tiflash_tables: 0,
            schema_version: 0,
            done_notifications: VecDeque::new(),
        }
    }

    /// 返回作业后端的只读引用。
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// 分配一个新的全局 ID（从 1 开始，饱和递增防止溢出）。
    fn alloc_id(&mut self) -> i64 {
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.next_id
    }

    /// 按名称（大小写不敏感）查找库，不存在则报错。
    fn schema(&self, name: &str) -> Result<&SchemaInfo, ExecutorError> {
        self.schemas
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| ExecutorError::SchemaNotFound(name.to_string()))
    }
    /// `schema` 的可变引用版本。
    fn schema_mut(&mut self, name: &str) -> Result<&mut SchemaInfo, ExecutorError> {
        self.schemas
            .get_mut(&name.to_ascii_lowercase())
            .ok_or_else(|| ExecutorError::SchemaNotFound(name.to_string()))
    }

    /// 创建库（CREATE DATABASE）。
    ///
    /// `charset_options` 是语句中出现的多组字符集/排序规则选项；
    /// `on_exist` 决定库已存在时报错还是忽略。返回新库的 ID。
    pub fn create_schema(
        &mut self,
        session: &mut SessionContext,
        name: &str,
        charset_options: &[(Option<String>, Option<String>)],
        placement_policy: Option<String>,
        on_exist: OnExist,
    ) -> Result<i64, ExecutorError> {
        check_identifier(name, "schema")?;
        let key = name.to_ascii_lowercase();
        // 库已存在：按 on_exist 策略处理。
        if self.schemas.contains_key(&key) {
            return match on_exist {
                OnExist::Ignore => Ok(self.schemas[&key].id),
                OnExist::Replace => Err(ExecutorError::Unsupported("replace schema".into())),
                OnExist::Error => Err(ExecutorError::SchemaExists(name.into())),
            };
        }
        // utf8mb4 的默认排序规则可由系统变量覆盖。
        let default_collation = session
            .system_vars
            .get("default_collation_for_utf8mb4")
            .map(String::as_str)
            .unwrap_or("utf8mb4_bin");
        let (charset, collation) = resolve_charset_collation(charset_options, default_collation)?;
        let id = self.alloc_id();
        let info = SchemaInfo {
            id,
            name: name.to_string(),
            charset,
            collation,
            placement_policy,
            tables: BTreeMap::new(),
        };
        self.schemas.insert(key, info);
        self.submit_simple_job(session, DdlAction::CreateSchema, id, 0, BTreeMap::new())?;
        Ok(id)
    }

    /// 修改库的默认字符集与排序规则（ALTER DATABASE ... CHARACTER SET）。
    pub fn alter_schema_charset(
        &mut self,
        session: &mut SessionContext,
        name: &str,
        charset: &str,
        collation: &str,
    ) -> Result<(), ExecutorError> {
        check_charset_and_collation(charset, collation)?;
        let schema = self.schema_mut(name)?;
        // 与现值相同则无需提交作业。
        if schema.charset.eq_ignore_ascii_case(charset)
            && schema.collation.eq_ignore_ascii_case(collation)
        {
            return Ok(());
        }
        schema.charset = charset.to_ascii_lowercase();
        schema.collation = collation.to_ascii_lowercase();
        let id = schema.id;
        self.submit_simple_job(
            session,
            DdlAction::ModifySchemaCharset,
            id,
            0,
            BTreeMap::new(),
        )
    }

    /// 修改库的放置策略（ALTER DATABASE ... PLACEMENT POLICY）。
    ///
    /// `ignore` 为 true 时（如某些兼容模式下）忽略放置设置并记录提示。
    pub fn alter_schema_placement(
        &mut self,
        session: &mut SessionContext,
        name: &str,
        policy: Option<String>,
        ignore: bool,
    ) -> Result<(), ExecutorError> {
        let schema = self.schema_mut(name)?;
        // 指定 default 策略等价于清除策略。
        schema.placement_policy = if ignore
            || policy
                .as_ref()
                .is_some_and(|name| name.eq_ignore_ascii_case(DEFAULT_PLACEMENT_POLICY_NAME))
        {
            None
        } else {
            policy
        };
        if ignore {
            session.notes.push("placement is ignored".into());
        }
        let id = schema.id;
        self.submit_simple_job(
            session,
            DdlAction::ModifySchemaPlacement,
            id,
            0,
            BTreeMap::new(),
        )
    }

    /// 删除库（DROP DATABASE）。`if_exists` 对应 `IF EXISTS` 修饰。
    pub fn drop_schema(
        &mut self,
        session: &mut SessionContext,
        name: &str,
        if_exists: bool,
    ) -> Result<(), ExecutorError> {
        let key = name.to_ascii_lowercase();
        let Some(schema) = self.schemas.remove(&key) else {
            return if if_exists {
                session.notes.push(format!("schema {name} does not exist"));
                Ok(())
            } else {
                Err(ExecutorError::SchemaNotFound(name.into()))
            };
        };
        self.submit_simple_job(
            session,
            DdlAction::DropSchema,
            schema.id,
            0,
            BTreeMap::new(),
        )
    }

    /// 恢复被删除的库（FLASHBACK/RECOVER DATABASE），直接放回给定的元数据。
    pub fn recover_schema(
        &mut self,
        session: &mut SessionContext,
        schema: SchemaInfo,
    ) -> Result<(), ExecutorError> {
        let key = schema.name.to_ascii_lowercase();
        if self.schemas.contains_key(&key) {
            return Err(ExecutorError::SchemaExists(schema.name));
        }
        let id = schema.id;
        self.schemas.insert(key, schema);
        self.submit_simple_job(session, DdlAction::RecoverSchema, id, 0, BTreeMap::new())
    }

    /// 创建表（CREATE TABLE）。校验表定义后写入元数据并提交作业，返回表 ID。
    pub fn create_table(
        &mut self,
        session: &mut SessionContext,
        schema_name: &str,
        mut table: TableInfo,
        on_exist: OnExist,
    ) -> Result<i64, ExecutorError> {
        check_identifier(&table.name, "table")?;
        validate_table_definition(&table)?;
        let has_columnar_index = table.indexes.iter().any(|index| index.vector);
        if has_columnar_index && table.tiflash_replica_count == 0 {
            table.tiflash_replica_count = 1;
        }
        if table.tiflash_replica_count > 0 {
            check_columnar_storage_for_replica(session, table.tiflash_replica_count, false)
                .map_err(|error| {
                    if has_columnar_index {
                        wrap_columnar_index_gate(error)
                    } else {
                        error
                    }
                })?;
        }
        let key = table.name.to_ascii_lowercase();
        if self.schema(schema_name)?.tables.contains_key(&key) {
            return match on_exist {
                OnExist::Ignore => Ok(self.schema(schema_name)?.tables[&key].id),
                _ => Err(ExecutorError::TableExists(table.name)),
            };
        }
        if table.id == 0 {
            table.id = self.alloc_id();
        }
        table.schema_id = self.schema(schema_name)?.id;
        let id = table.id;
        let schema_id = table.schema_id;
        let tiflash_replica_count = table.tiflash_replica_count;
        self.schema_mut(schema_name)?.tables.insert(key, table);
        self.submit_simple_job(
            session,
            DdlAction::CreateTable,
            schema_id,
            id,
            BTreeMap::from([
                (
                    "tiflash_replica_count".into(),
                    tiflash_replica_count.to_string(),
                ),
                ("columnar_index".into(), has_columnar_index.to_string()),
            ]),
        )?;
        Ok(id)
    }

    /// 批量创建表：先整体校验（重名、定义合法性），再逐个创建。
    pub fn batch_create_tables(
        &mut self,
        session: &mut SessionContext,
        schema: &str,
        tables: Vec<TableInfo>,
        on_exist: OnExist,
    ) -> Result<Vec<i64>, ExecutorError> {
        let mut created = Vec::new();
        let mut names = BTreeSet::new();
        // 预检查：批内表名不得重复，且每个表定义必须合法。
        for table in &tables {
            if !names.insert(table.name.to_ascii_lowercase()) {
                return Err(ExecutorError::TableExists(table.name.clone()));
            }
            validate_table_definition(table)?;
        }
        for table in tables {
            created.push(self.create_table(session, schema, table, on_exist)?);
        }
        Ok(created)
    }

    /// 删除表或视图（DROP TABLE/VIEW）。
    ///
    /// `view_only` 为 true 表示 DROP VIEW，此时目标必须是视图；
    /// 返回被删除的表元数据（可用于后续 RECOVER）。
    pub fn drop_table(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        if_exists: bool,
        view_only: bool,
    ) -> Result<TableInfo, ExecutorError> {
        // 系统关键表禁止删除。
        if is_undroppable_table(&ident.schema, &ident.table) {
            return Err(ExecutorError::Unsupported(
                "system table cannot be dropped".into(),
            ));
        }
        let schema = self.schema_mut(&ident.schema)?;
        let key = ident.table.to_ascii_lowercase();
        let Some(table) = schema.tables.remove(&key) else {
            return if if_exists {
                Err(ExecutorError::TableNotFound(ident.key()))
            } else {
                Err(ExecutorError::TableNotFound(ident.key()))
            };
        };
        // 对象类型不匹配（对表执行 DROP VIEW 或反之）：放回元数据并报错。
        if view_only != table.view {
            schema.tables.insert(key, table);
            return Err(ExecutorError::Unsupported("wrong object type".into()));
        }
        let action = if view_only {
            DdlAction::DropView
        } else {
            DdlAction::DropTable
        };
        let schema_id = schema.id;
        let table_id = table.id;
        self.submit_simple_job(session, action, schema_id, table_id, BTreeMap::new())?;
        Ok(table)
    }

    /// 截断表（TRUNCATE TABLE）：通过给表换一个新 ID 来"逻辑清空"数据，
    /// 旧 ID 对应的数据由后台垃圾回收。返回新表 ID。
    pub fn truncate_table(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
    ) -> Result<i64, ExecutorError> {
        let new_id = self.alloc_id();
        let schema = self.schema_mut(&ident.schema)?;
        let table = schema
            .tables
            .get_mut(&ident.table.to_ascii_lowercase())
            .ok_or_else(|| ExecutorError::TableNotFound(ident.key()))?;
        let old_id = table.id;
        table.id = new_id;
        table.auto_increment = 0;
        table.tiflash_available_ids.clear();
        let schema_id = schema.id;
        // 表锁需要跟随新表 ID 迁移：提交前先复制锁，结束后按结果清理。
        handle_lock_on_submit(session, old_id, new_id);
        let result = self.submit_simple_job(
            session,
            DdlAction::TruncateTable,
            schema_id,
            old_id,
            BTreeMap::from([("new_table_id".into(), new_id.to_string())]),
        );
        handle_lock_on_finish(session, old_id, new_id, result.is_ok());
        result.map(|_| new_id)
    }

    /// 批量重命名表（RENAME TABLE a TO b, ...），支持跨库移动。
    pub fn rename_tables(
        &mut self,
        session: &mut SessionContext,
        renames: &[(Ident, Ident)],
    ) -> Result<(), ExecutorError> {
        // 目标名不得在本批内重复。
        let mut targets = BTreeSet::new();
        for (_, target) in renames {
            check_identifier(&target.table, "table")?;
            if !targets.insert(target.key()) {
                return Err(ExecutorError::TableExists(target.key()));
            }
        }
        // 目标名若已存在且不是本批的某个源表（链式改名场景），则冲突。
        let source_keys: BTreeSet<String> =
            renames.iter().map(|(source, _)| source.key()).collect();
        for (_, target) in renames {
            if !source_keys.contains(&target.key()) && self.table_exists(target) {
                return Err(ExecutorError::TableExists(target.key()));
            }
        }
        // 先整体摘除所有源表，再统一放到目标位置，保证批内原子性。
        let mut moved = Vec::new();
        for (source, target) in renames {
            let schema = self.schema_mut(&source.schema)?;
            let table = schema
                .tables
                .remove(&source.table.to_ascii_lowercase())
                .ok_or_else(|| ExecutorError::TableNotFound(source.key()))?;
            moved.push((table, source.clone(), target.clone()));
        }
        for (mut table, source, target) in moved {
            table.schema_id = self.schema(&target.schema)?.id;
            table.name = target.table.clone();
            // 全库扫描：更新所有指向旧名字的外键引用。
            for schema in self.schemas.values_mut() {
                for dependent in schema.tables.values_mut() {
                    for fk in &mut dependent.foreign_keys {
                        if fk.referenced == source {
                            fk.referenced = target.clone();
                        }
                    }
                }
            }
            self.schema_mut(&target.schema)?
                .tables
                .insert(target.table.to_ascii_lowercase(), table);
        }
        self.submit_simple_job(session, DdlAction::RenameTable, 0, 0, BTreeMap::new())
    }

    /// 判断指定库表是否存在（大小写不敏感）。
    pub fn table_exists(&self, ident: &Ident) -> bool {
        self.schemas
            .get(&ident.schema.to_ascii_lowercase())
            .is_some_and(|schema| {
                schema
                    .tables
                    .contains_key(&ident.table.to_ascii_lowercase())
            })
    }

    /// 添加列（ALTER TABLE ... ADD COLUMN）。
    ///
    /// `after` 指定插入到某列之后，None 表示追加到末尾。
    pub fn add_column(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        column: ColumnInfo,
        after: Option<&str>,
        if_not_exists: bool,
    ) -> Result<(), ExecutorError> {
        check_identifier(&column.name, "column")?;
        let table = self.table_mut(ident)?;
        if table
            .columns
            .iter()
            .any(|old| old.name.eq_ignore_ascii_case(&column.name))
        {
            return if if_not_exists {
                Ok(())
            } else {
                Err(ExecutorError::ColumnExists(column.name))
            };
        }
        // 计算插入位置：未指定 AFTER 时追加到末尾。
        let position = match after {
            None => table.columns.len(),
            Some(name) => table
                .columns
                .iter()
                .position(|old| old.name.eq_ignore_ascii_case(name))
                .map(|index| index + 1)
                .ok_or_else(|| ExecutorError::ColumnNotFound(name.into()))?,
        };
        table.columns.insert(position, column);
        let (schema_id, table_id) = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::AddColumn,
            schema_id,
            table_id,
            BTreeMap::new(),
        )
    }

    /// 删除列（ALTER TABLE ... DROP COLUMN）。
    ///
    /// 会拒绝删除最后一个可见列，以及被索引、生成列或 TTL 依赖的列。
    pub fn drop_column(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        name: &str,
        if_exists: bool,
    ) -> Result<(), ExecutorError> {
        let table = self.table_mut(ident)?;
        let Some(index) = table
            .columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
        else {
            return if if_exists {
                Ok(())
            } else {
                Err(ExecutorError::ColumnNotFound(name.into()))
            };
        };
        // 表至少要保留一个可见列。
        if table.columns.iter().filter(|column| !column.hidden).count() <= 1
            && !table.columns[index].hidden
        {
            return Err(ExecutorError::LastVisibleColumn);
        }
        // 依赖检查：被索引引用、被生成列依赖或作为 TTL 列时不能删除。
        if table.indexes.iter().any(|idx| {
            idx.columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case(name))
        }) || table.columns.iter().any(|column| {
            column
                .generated_dependencies
                .iter()
                .any(|dep| dep.eq_ignore_ascii_case(name))
        }) || table
            .ttl_column
            .as_ref()
            .is_some_and(|ttl| ttl.eq_ignore_ascii_case(name))
        {
            return Err(ExecutorError::Dependency(name.into()));
        }
        table.columns.remove(index);
        let (schema_id, table_id) = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::DropColumn,
            schema_id,
            table_id,
            BTreeMap::new(),
        )
    }

    /// 修改列定义（ALTER TABLE ... MODIFY/CHANGE COLUMN），可同时改名。
    pub fn modify_column(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        old_name: &str,
        mut new_column: ColumnInfo,
    ) -> Result<(), ExecutorError> {
        let table = self.table_mut(ident)?;
        let index = table
            .columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(old_name))
            .ok_or_else(|| ExecutorError::ColumnNotFound(old_name.into()))?;
        if !old_name.eq_ignore_ascii_case(&new_column.name)
            && table
                .columns
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(&new_column.name))
        {
            return Err(ExecutorError::ColumnExists(new_column.name));
        }
        // 绑定了脱敏策略的列不允许更改数据类型。
        if table.columns[index].masking_policy.is_some()
            && table.columns[index].kind != new_column.kind
        {
            return Err(ExecutorError::Dependency("masking policy".into()));
        }
        // 保留原列 ID，保证底层数据仍然可以按 ID 关联。
        new_column.id = table.columns[index].id;
        table.columns[index] = new_column;
        let (schema_id, table_id) = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::ModifyColumn,
            schema_id,
            table_id,
            BTreeMap::new(),
        )
    }

    /// 创建索引（CREATE INDEX / ADD PRIMARY KEY），返回新索引 ID。
    pub fn create_index(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        mut index: IndexInfo,
        if_not_exists: bool,
    ) -> Result<i64, ExecutorError> {
        check_identifier(&index.name, "index")?;
        if index.vector {
            check_columnar_storage_enabled(session).map_err(wrap_columnar_index_gate)?;
        }
        let new_id = self.alloc_id();
        let table = self.table_mut(ident)?;
        if table
            .indexes
            .iter()
            .any(|old| old.name.eq_ignore_ascii_case(&index.name))
        {
            return if if_not_exists {
                Ok(0)
            } else {
                Err(ExecutorError::IndexExists(index.name))
            };
        }
        // 索引列必须都存在于表中。
        for name in &index.columns {
            if !table
                .columns
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(name))
            {
                return Err(ExecutorError::ColumnNotFound(name.clone()));
            }
        }
        // 主键不允许设为不可见索引。
        if index.primary && index.invisible {
            return Err(ExecutorError::InvisiblePrimaryKey);
        }
        check_create_global_index(table, &index)?;
        index.id = new_id;
        index.state = ObjectState::None;
        let action = if index.primary {
            DdlAction::AddPrimaryKey
        } else {
            DdlAction::AddIndex
        };
        let columnar = index.vector;
        table.indexes.push(index);
        if columnar && table.tiflash_replica_count == 0 {
            table.tiflash_replica_count = 1;
        }
        let (schema_id, table_id) = (table.schema_id, table.id);
        self.submit_simple_job_with_schema_state(
            session,
            action,
            schema_id,
            table_id,
            ObjectState::Public,
            BTreeMap::from([("columnar_index".into(), columnar.to_string())]),
        )?;
        self.table_mut(ident)?
            .indexes
            .iter_mut()
            .find(|created| created.id == new_id)
            .expect("newly created index must remain in table metadata")
            .state = ObjectState::Public;
        Ok(new_id)
    }

    /// 删除索引（DROP INDEX / DROP PRIMARY KEY）。
    ///
    /// 若某外键依赖该索引且没有其他索引可以替代它，则拒绝删除
    /// （外键需要一个前缀匹配其列的索引来加速引用检查）。
    pub fn drop_index(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        name: &str,
        if_exists: bool,
    ) -> Result<(), ExecutorError> {
        let table = self.table_mut(ident)?;
        let Some(position) = table
            .indexes
            .iter()
            .position(|index| index.name.eq_ignore_ascii_case(name))
        else {
            return if if_exists {
                Ok(())
            } else {
                Err(ExecutorError::IndexNotFound(name.into()))
            };
        };
        let index = &table.indexes[position];
        // 检查每个外键：若待删索引正好覆盖外键列前缀，
        // 必须存在另一个同样覆盖前缀的索引，否则报依赖错误。
        for foreign_key in &table.foreign_keys {
            if index.columns.len() >= foreign_key.columns.len()
                && index
                    .columns
                    .iter()
                    .zip(&foreign_key.columns)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
            {
                let alternate = table.indexes.iter().enumerate().any(|(i, other)| {
                    i != position
                        && other.columns.len() >= foreign_key.columns.len()
                        && other
                            .columns
                            .iter()
                            .zip(&foreign_key.columns)
                            .all(|(left, right)| left.eq_ignore_ascii_case(right))
                });
                if !alternate {
                    return Err(ExecutorError::Dependency(foreign_key.name.clone()));
                }
            }
        }
        let primary = index.primary;
        table.indexes.remove(position);
        let (schema_id, table_id) = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            if primary {
                DdlAction::DropPrimaryKey
            } else {
                DdlAction::DropIndex
            },
            schema_id,
            table_id,
            BTreeMap::new(),
        )
    }

    /// 重命名索引（ALTER TABLE ... RENAME INDEX）。
    pub fn rename_index(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), ExecutorError> {
        check_identifier(new_name, "index")?;
        let table = self.table_mut(ident)?;
        let source = table
            .indexes
            .iter()
            .position(|index| index.name.eq_ignore_ascii_case(old_name))
            .ok_or_else(|| ExecutorError::IndexNotFound(old_name.into()))?;
        if old_name == new_name {
            return Ok(());
        }
        if table.indexes.iter().enumerate().any(|(position, index)| {
            position != source && index.name.eq_ignore_ascii_case(new_name)
        }) {
            return Err(ExecutorError::IndexExists(new_name.into()));
        }
        table.indexes[source].name = new_name.into();
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::RenameIndex,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )
    }

    /// 添加外键约束（ALTER TABLE ... ADD FOREIGN KEY）。
    pub fn add_foreign_key(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        foreign_key: ForeignKeyInfo,
    ) -> Result<(), ExecutorError> {
        check_identifier(&foreign_key.name, "foreign key")?;
        // 被引用（父）表必须存在。
        if !self.table_exists(&foreign_key.referenced) {
            return Err(ExecutorError::TableNotFound(foreign_key.referenced.key()));
        }
        let table = self.table_mut(ident)?;
        if table
            .foreign_keys
            .iter()
            .any(|old| old.name.eq_ignore_ascii_case(&foreign_key.name))
        {
            return Err(ExecutorError::ForeignKeyExists(foreign_key.name));
        }
        for name in &foreign_key.columns {
            if !table
                .columns
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(name))
            {
                return Err(ExecutorError::ColumnNotFound(name.clone()));
            }
        }
        table.foreign_keys.push(foreign_key);
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::AddForeignKey,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )
    }

    /// 删除外键约束（ALTER TABLE ... DROP FOREIGN KEY）。
    pub fn drop_foreign_key(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        name: &str,
    ) -> Result<(), ExecutorError> {
        let table = self.table_mut(ident)?;
        let index = table
            .foreign_keys
            .iter()
            .position(|foreign_key| foreign_key.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| ExecutorError::ForeignKeyNotFound(name.into()))?;
        table.foreign_keys.remove(index);
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::DropForeignKey,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )
    }

    /// 添加分区（ALTER TABLE ... ADD PARTITION）。
    pub fn add_partitions(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        mut definitions: Vec<PartitionDefinition>,
    ) -> Result<(), ExecutorError> {
        // 为每个新分区分配物理 ID。
        let mut new_ids = Vec::new();
        for definition in &mut definitions {
            check_identifier(&definition.name, "partition")?;
            if definition.id == 0 {
                definition.id = self.alloc_id();
            }
            new_ids.push(definition.id);
        }
        let table = self.table_mut(ident)?;
        let existing: BTreeSet<String> = table
            .partitions
            .iter()
            .map(|part| part.name.to_ascii_lowercase())
            .collect();
        if definitions
            .iter()
            .any(|part| existing.contains(&part.name.to_ascii_lowercase()))
        {
            return Err(ExecutorError::InvalidPartition(
                "duplicate partition".into(),
            ));
        }
        table.partitions.extend(definitions);
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::AddPartition,
            ids.0,
            ids.1,
            BTreeMap::from([("new_ids".into(), format!("{new_ids:?}"))]),
        )
    }

    /// 删除分区（ALTER TABLE ... DROP PARTITION），返回被删分区的 ID 列表。
    pub fn drop_partitions(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        names: &[String],
    ) -> Result<Vec<i64>, ExecutorError> {
        let table = self.table_mut(ident)?;
        let wanted: BTreeSet<String> = names.iter().map(|name| name.to_ascii_lowercase()).collect();
        // 至少要保留一个分区。
        if wanted.len() >= table.partitions.len() {
            return Err(ExecutorError::InvalidPartition(
                "cannot drop all partitions".into(),
            ));
        }
        if wanted.iter().any(|name| {
            !table
                .partitions
                .iter()
                .any(|part| part.name.eq_ignore_ascii_case(name))
        }) {
            return Err(ExecutorError::PartitionNotFound(names.join(",")));
        }
        let removed: Vec<i64> = table
            .partitions
            .iter()
            .filter(|part| wanted.contains(&part.name.to_ascii_lowercase()))
            .map(|part| part.id)
            .collect();
        table
            .partitions
            .retain(|part| !wanted.contains(&part.name.to_ascii_lowercase()));
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::DropPartition,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )?;
        Ok(removed)
    }

    /// 交换分区（ALTER TABLE ... EXCHANGE PARTITION）：
    /// 将分区表的某个分区与一张普通表的数据互换，
    /// 实现上只需互换两者的物理 ID，无需搬移数据。
    pub fn exchange_partition(
        &mut self,
        session: &mut SessionContext,
        partitioned: &Ident,
        partition_name: &str,
        normal: &Ident,
    ) -> Result<(), ExecutorError> {
        let partitioned_table = self.table(partitioned)?.clone();
        let normal_table = self.table(normal)?.clone();
        // 两表的列定义、索引数量等必须完全兼容。
        check_table_def_compatible(&partitioned_table, &normal_table)?;
        let partition = partitioned_table
            .partitions
            .iter()
            .find(|part| part.name.eq_ignore_ascii_case(partition_name))
            .ok_or_else(|| ExecutorError::PartitionNotFound(partition_name.into()))?;
        let normal_id = normal_table.id;
        let partition_id = partition.id;
        // 互换物理 ID：分区拿普通表的 ID，普通表拿分区的 ID。
        self.table_mut(partitioned)?
            .partitions
            .iter_mut()
            .find(|part| part.id == partition_id)
            .unwrap()
            .id = normal_id;
        self.table_mut(normal)?.id = partition_id;
        self.submit_simple_job(
            session,
            DdlAction::ExchangePartition,
            partitioned_table.schema_id,
            partitioned_table.id,
            BTreeMap::new(),
        )
    }

    /// 重设自增基值（ALTER TABLE ... AUTO_INCREMENT = n）。
    ///
    /// 默认只允许调大；`force` 为 true 时允许强制回退。返回实际生效的基值。
    pub fn rebase_auto_id(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        new_base: i64,
        force: bool,
    ) -> Result<i64, ExecutorError> {
        if new_base < 0 {
            return Err(ExecutorError::InvalidAutoId);
        }
        let table = self.table_mut(ident)?;
        // 非强制模式下不允许比当前值小，避免自增 ID 冲突。
        let adjusted = if force {
            new_base
        } else {
            new_base.max(table.auto_increment)
        };
        table.auto_increment = adjusted;
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::RebaseAutoId,
            ids.0,
            ids.1,
            BTreeMap::from([
                ("force".into(), force.to_string()),
                ("base".into(), adjusted.to_string()),
            ]),
        )?;
        Ok(adjusted)
    }

    /// 设置 SHARD_ROW_ID_BITS：对隐式行 ID 高位做随机分片以打散写热点。
    pub fn shard_row_id(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        bits: u8,
    ) -> Result<(), ExecutorError> {
        // 分片位数上限 15。
        if bits > 15 {
            return Err(ExecutorError::InvalidShardBits);
        }
        let table = self.table_mut(ident)?;
        // 只能增大，不能小于历史最大值，否则旧数据行 ID 可能冲突。
        if bits < table.max_shard_row_id_bits {
            return Err(ExecutorError::InvalidShardBits);
        }
        table.shard_row_id_bits = bits;
        table.max_shard_row_id_bits = table.max_shard_row_id_bits.max(bits);
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::ShardRowId,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )
    }

    /// 修改表的字符集与排序规则（ALTER TABLE ... CONVERT TO CHARACTER SET）。
    ///
    /// `overwrite_columns` 为 true 时同步覆盖所有字符串/文本列的字符集。
    pub fn alter_table_charset(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        charset: &str,
        collation: &str,
        overwrite_columns: bool,
    ) -> Result<(), ExecutorError> {
        check_charset_and_collation(charset, collation)?;
        let table = self.table_mut(ident)?;
        // 与现值相同则无需提交作业。
        if table.charset.eq_ignore_ascii_case(charset)
            && table.collation.eq_ignore_ascii_case(collation)
        {
            return Ok(());
        }
        if overwrite_columns {
            for column in &mut table.columns {
                if matches!(column.kind, ColumnKind::String | ColumnKind::Blob) {
                    column.charset = charset.into();
                    column.collation = collation.into();
                }
            }
        }
        table.charset = charset.into();
        table.collation = collation.into();
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::ModifyCharset,
            ids.0,
            ids.1,
            BTreeMap::new(),
        )
    }

    /// 设置 TiFlash 列存副本数（ALTER TABLE ... SET TIFLASH REPLICA n）。
    ///
    /// TiFlash 是列式存储引擎，通过为表增加列存副本加速分析型查询。
    pub fn set_tiflash_replica(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        count: u64,
        available_stores: u64,
    ) -> Result<(), ExecutorError> {
        self.set_tiflash_replica_with_options(session, ident, count, available_stores, false)
    }

    /// Internal variant used by placement-rule repair. The bypass only skips
    /// the columnar-storage switch; all table-kind and store-count checks stay.
    pub fn set_tiflash_replica_with_options(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        count: u64,
        available_stores: u64,
        skip_columnar_storage_gate: bool,
    ) -> Result<(), ExecutorError> {
        let current_count = self.table_mut(ident)?.tiflash_replica_count;
        if current_count == count {
            return Ok(());
        }
        let store_type = columnar_store_type(session);
        if matches!(store_type.as_str(), "tiflash" | "both") && count > available_stores {
            return Err(ExecutorError::Unsupported(
                "TiFlash replica count exceeds stores".into(),
            ));
        }
        check_columnar_storage_for_replica(session, count, skip_columnar_storage_gate)?;
        let table = self.table_mut(ident)?;
        // 临时表、视图、序列不支持 TiFlash 副本。
        if table.temporary || table.view || table.sequence {
            return Err(ExecutorError::Unsupported(
                "TiFlash unsupported table type".into(),
            ));
        }
        table.tiflash_replica_count = count;
        if count == 0 {
            table.tiflash_available_ids.clear();
        }
        let ids = (table.schema_id, table.id);
        self.submit_simple_job(
            session,
            DdlAction::SetTiFlashReplica,
            ids.0,
            ids.1,
            BTreeMap::from([
                ("replica_count".into(), count.to_string()),
                (
                    "skip_columnar_storage_gate".into(),
                    skip_columnar_storage_gate.to_string(),
                ),
            ]),
        )
    }

    /// 为数据库中的普通表批量设置 TiFlash 副本数。
    ///
    /// KILL QUERY 必须以 QueryInterrupted 返回；仅由 failpoint 触发的提前结束仍成功。
    /// 单表 DDL 返回取消/中断时立即停止，不继续处理剩余表。
    pub fn set_schema_tiflash_replica(
        &mut self,
        session: &mut SessionContext,
        schema_name: &str,
        count: u64,
        available_stores: u64,
        pending_threshold: u32,
    ) -> Result<(), ExecutorError> {
        if count > available_stores {
            return Err(ExecutorError::Unsupported(
                "TiFlash replica count exceeds stores".into(),
            ));
        }
        let table_names = self
            .schema(schema_name)?
            .tables
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if table_names.is_empty() {
            return Err(ExecutorError::Unsupported("empty database".into()));
        }

        for table_name in table_names {
            if session.killed {
                return Err(ExecutorError::QueryInterrupted);
            }
            if session.batch_tiflash_abort {
                return Ok(());
            }
            // The Go path waits while pending replicas reach the configured threshold.
            // This in-memory executor has no asynchronous schema cache, so it can only
            // re-check the session abort signals before issuing the next DDL job.
            if pending_threshold > 0 && self.pending_tiflash_tables >= pending_threshold {
                if session.killed {
                    return Err(ExecutorError::QueryInterrupted);
                }
                if session.batch_tiflash_abort {
                    return Ok(());
                }
            }
            let ident = Ident::new(schema_name, table_name);
            match self.set_tiflash_replica(session, &ident, count, available_stores) {
                Err(error @ (ExecutorError::Cancelled | ExecutorError::QueryInterrupted)) => {
                    return Err(error);
                }
                other => other?,
            }
        }
        Ok(())
    }

    /// 更新某个物理表/分区的 TiFlash 副本同步状态（由存储层回调触发）。
    pub fn update_replica_status(
        &mut self,
        session: &mut SessionContext,
        physical_id: i64,
        available: bool,
    ) -> Result<(), ExecutorError> {
        let mut target = None;
        // 全库查找该物理 ID 归属的表（可能是表本身或其某个分区）。
        for schema in self.schemas.values_mut() {
            for table in schema.tables.values_mut() {
                if table.id == physical_id
                    || table.partitions.iter().any(|part| part.id == physical_id)
                {
                    if available {
                        table.tiflash_available_ids.insert(physical_id);
                    } else {
                        table.tiflash_available_ids.remove(&physical_id);
                    }
                    target = Some((table.schema_id, table.id));
                    break;
                }
            }
        }
        let (schema_id, table_id) =
            target.ok_or_else(|| ExecutorError::TableNotFound(physical_id.to_string()))?;
        self.submit_simple_job(
            session,
            DdlAction::UpdateTiFlashReplicaStatus,
            schema_id,
            table_id,
            BTreeMap::new(),
        )
    }

    /// 批量设置表选项（注释、放置策略、亲和性、表缓存、TTL 列），
    /// 每设置一项就提交一个对应类型的作业。
    pub fn set_table_options(
        &mut self,
        session: &mut SessionContext,
        ident: &Ident,
        comment: Option<String>,
        placement: Option<String>,
        affinity: Option<String>,
        cached: Option<bool>,
        ttl_column: Option<String>,
    ) -> Result<(), ExecutorError> {
        let table = self.table_mut(ident)?;
        let mut actions = Vec::new();
        if let Some(comment) = comment {
            table.comment = validate_comment_length(&comment, 2048, &mut session.warnings);
            actions.push(DdlAction::ModifyComment);
        }
        if let Some(policy) = placement {
            table.placement_policy =
                (!policy.eq_ignore_ascii_case(DEFAULT_PLACEMENT_POLICY_NAME)).then_some(policy);
            actions.push(DdlAction::SetPlacement);
        }
        if let Some(level) = affinity {
            if !matches!(
                level.to_ascii_uppercase().as_str(),
                "TABLE" | "PARTITION" | ""
            ) {
                return Err(ExecutorError::Unsupported("invalid affinity".into()));
            }
            table.affinity = (!level.is_empty()).then_some(level);
            actions.push(DdlAction::SetAffinity);
        }
        if let Some(cached) = cached {
            table.cached = cached;
            actions.push(if cached {
                DdlAction::CacheTable
            } else {
                DdlAction::NoCacheTable
            });
        }
        if let Some(column) = ttl_column {
            if !table
                .columns
                .iter()
                .any(|info| info.name.eq_ignore_ascii_case(&column))
            {
                return Err(ExecutorError::ColumnNotFound(column));
            }
            table.ttl_column = Some(column);
            actions.push(DdlAction::SetTtl);
        }
        let ids = (table.schema_id, table.id);
        for action in actions {
            self.submit_simple_job(session, action, ids.0, ids.1, BTreeMap::new())?;
        }
        Ok(())
    }

    /// 加表锁（LOCK TABLES），已被其他会话锁定时报冲突。
    pub fn lock_tables(
        &mut self,
        session: &mut SessionContext,
        locks: &[(Ident, TableLockType)],
    ) -> Result<(), ExecutorError> {
        for (ident, lock_type) in locks {
            let table = self.table_mut(ident)?;
            if table.table_lock.is_some() {
                return Err(ExecutorError::LockConflict);
            }
            table.table_lock = Some(*lock_type);
            session.locked_tables.insert(table.id, *lock_type);
        }
        self.submit_simple_job(session, DdlAction::LockTable, 0, 0, BTreeMap::new())
    }

    /// 释放当前会话持有的全部表锁（UNLOCK TABLES）。
    pub fn unlock_tables(&mut self, session: &mut SessionContext) -> Result<(), ExecutorError> {
        let ids: Vec<i64> = session.locked_tables.keys().copied().collect();
        for schema in self.schemas.values_mut() {
            for table in schema.tables.values_mut() {
                if ids.contains(&table.id) {
                    table.table_lock = None;
                }
            }
        }
        session.locked_tables.clear();
        self.submit_simple_job(session, DdlAction::UnlockTable, 0, 0, BTreeMap::new())
    }

    /// 构造并提交一个简单的 DDL 作业，同步等待其完成。
    pub fn submit_simple_job(
        &mut self,
        session: &mut SessionContext,
        action: DdlAction,
        schema_id: i64,
        table_id: i64,
        args: BTreeMap<String, String>,
    ) -> Result<(), ExecutorError> {
        self.submit_simple_job_with_schema_state(
            session,
            action,
            schema_id,
            table_id,
            ObjectState::None,
            args,
        )
    }

    /// 构造并提交一个带指定最终模式状态的简单 DDL 作业。
    fn submit_simple_job_with_schema_state(
        &mut self,
        session: &mut SessionContext,
        action: DdlAction,
        schema_id: i64,
        table_id: i64,
        schema_state: ObjectState,
        args: BTreeMap<String, String>,
    ) -> Result<(), ExecutorError> {
        // Owner/job-side recheck: the global switch may change after SQL
        // precheck but before the job reaches its first metadata transition.
        check_columnar_storage_for_job(session, &action, &args)?;
        // 多模式变更收集阶段：只记录子操作，不立即提交。
        if let Some(actions) = session.multi_schema_actions.as_mut() {
            actions.push(action);
            return Ok(());
        }
        let mut job = DdlJob {
            id: 0,
            schema_id,
            table_id,
            action,
            state: JobState::None,
            schema_state,
            multi_schema_revertible: false,
            query: session.query.clone(),
            error: None,
            warnings: BTreeMap::new(),
            schema_version: 0,
            involving_schema: Vec::new(),
            args,
        };
        self.do_ddl_job_wrapper(session, &mut job)
    }

    /// 提交作业并轮询等待其完成的核心流程。
    ///
    /// 流程：校验涉及对象 -> 设置作业 SQL -> 提交到后端 ->
    /// 循环检查历史/当前作业状态，直到成功、失败、取消或超时。
    pub fn do_ddl_job_wrapper(
        &mut self,
        session: &mut SessionContext,
        job: &mut DdlJob,
    ) -> Result<(), ExecutorError> {
        validate_involving_schema(&job.involving_schema)?;
        set_ddl_job_query(session, job);
        let result = self.backend.submit(job).map_err(ExecutorError::JobSubmit)?;
        job.id = result.job_id;
        session.ddl_job_id = Some(result.job_id);
        // 截断表失败时需要把表锁还原到旧表 ID，先记录旧锁。
        let old_lock = if job.action == DdlAction::TruncateTable {
            session.locked_tables.get(&job.table_id).copied()
        } else {
            None
        };
        let mut attempt = 0_usize;
        loop {
            // 会话被 kill：正常情况下尝试取消作业；若在关机则直接退出。
            if session.killed {
                if session.shutting_down {
                    return Err(ExecutorError::Cancelled);
                }
                match self.backend.cancel(job.id) {
                    Ok(()) => session.ddl_job_id = None,
                    Err(error) if is_retryable_ddl_cancel_err(&error) => continue,
                    Err(_) => session.ddl_job_id = None,
                }
            }
            // 作业进入历史即代表已到达终态（完成/失败/取消）。
            let history = self
                .backend
                .history_job(job.id)
                .map_err(ExecutorError::JobSubmit)?;
            if let Some(history) = history {
                session.last_ddl_query = history.query.clone();
                session.last_ddl_sequence = history.schema_version;
                if history.state == JobState::Synced {
                    // 成功：合并警告、推进本地元数据版本并记录完成通知。
                    append_job_warnings(session, &history);
                    self.schema_version = self.schema_version.max(history.schema_version);
                    session.ddl_job_id = None;
                    self.done_notifications.push_back(job.id);
                    return Ok(());
                }
                if let Some(error) = history.error {
                    session.ddl_job_id = None;
                    // 截断表失败：把表锁还原到旧表 ID。
                    if let Some(lock_type) = old_lock {
                        session.locked_tables.insert(job.table_id, lock_type);
                    }
                    return Err(ExecutorError::JobFailed(error));
                }
                return Err(ExecutorError::JobFailed(
                    "terminal DDL job has no error".into(),
                ));
            }
            // 尚未完成：检查是否因磁盘等原因被系统自动暂停。
            if let Some(current) = self
                .backend
                .current_job(job.id)
                .map_err(ExecutorError::JobSubmit)?
            {
                if current.state == JobState::Paused
                    && current
                        .error
                        .as_ref()
                        .is_some_and(|error| error.contains("disk"))
                {
                    return Err(ExecutorError::JobAutoPaused {
                        id: job.id,
                        reason: current.error.unwrap_or_default(),
                    });
                }
            }
            attempt += 1;
            if attempt > 10_000 {
                return Err(ExecutorError::Timeout);
            }
            // 按操作类型选取下一次轮询间隔，并以 10 倍租约为上限。
            let _interval = get_job_check_interval(job.action.clone(), attempt)
                .0
                .min(self.lease.saturating_mul(10));
        }
    }

    /// 按标识符查找表，不存在则报错。
    fn table(&self, ident: &Ident) -> Result<&TableInfo, ExecutorError> {
        self.schema(&ident.schema)?
            .tables
            .get(&ident.table.to_ascii_lowercase())
            .ok_or_else(|| ExecutorError::TableNotFound(ident.key()))
    }
    /// `table` 的可变引用版本。
    fn table_mut(&mut self, ident: &Ident) -> Result<&mut TableInfo, ExecutorError> {
        self.schema_mut(&ident.schema)?
            .tables
            .get_mut(&ident.table.to_ascii_lowercase())
            .ok_or_else(|| ExecutorError::TableNotFound(ident.key()))
    }
}

/// 校验标识符合法性：非空且长度不超过 64 个字符（MySQL 标识符上限）。
pub fn check_identifier(name: &str, kind: &str) -> Result<(), ExecutorError> {
    if name.is_empty() || name.chars().count() > 64 {
        Err(ExecutorError::InvalidIdentifier(format!("{kind}: {name}")))
    } else {
        Ok(())
    }
}

/// 返回字符集的默认排序规则；utf8mb4 的默认值由参数指定（可配置）。
pub fn default_collation(charset: &str, default_utf8mb4: &str) -> Result<String, ExecutorError> {
    Ok(match charset.to_ascii_lowercase().as_str() {
        "utf8mb4" => default_utf8mb4.to_string(),
        "utf8" => "utf8_bin".into(),
        "latin1" => "latin1_bin".into(),
        "ascii" => "ascii_bin".into(),
        "binary" => "binary".into(),
        _ => return Err(ExecutorError::InvalidCharsetCollation),
    })
}

/// 解析语句中出现的多组字符集/排序规则选项，归并为最终的 (charset, collation)。
///
/// 多次出现且互相矛盾时报错；缺省的一方按另一方推导：
/// 排序规则名的前缀即其字符集，字符集则取其默认排序规则。
pub fn resolve_charset_collation(
    options: &[(Option<String>, Option<String>)],
    default_utf8mb4: &str,
) -> Result<(String, String), ExecutorError> {
    let mut charset: Option<String> = None;
    let mut collation: Option<String> = None;
    // 逐项归并，出现互相矛盾的重复指定即报错。
    for (new_charset, new_collation) in options {
        if let Some(value) = new_charset {
            if charset
                .as_ref()
                .is_some_and(|old| !old.eq_ignore_ascii_case(value))
            {
                return Err(ExecutorError::InvalidCharsetCollation);
            }
            charset = Some(value.to_ascii_lowercase());
        }
        if let Some(value) = new_collation {
            if collation
                .as_ref()
                .is_some_and(|old| !old.eq_ignore_ascii_case(value))
            {
                return Err(ExecutorError::InvalidCharsetCollation);
            }
            collation = Some(value.to_ascii_lowercase());
        }
    }
    // 未指定字符集时从排序规则名前缀推导（如 utf8mb4_bin -> utf8mb4）。
    let charset = charset.unwrap_or_else(|| {
        collation
            .as_deref()
            .and_then(|value| value.split('_').next())
            .unwrap_or("utf8mb4")
            .to_string()
    });
    let collation = collation.unwrap_or(default_collation(&charset, default_utf8mb4)?);
    check_charset_and_collation(&charset, &collation)?;
    Ok((charset, collation))
}

/// 校验字符集与排序规则是否匹配：排序规则名必须以字符集名为前缀
/// （binary 除外，其排序规则也叫 binary）。
pub fn check_charset_and_collation(charset: &str, collation: &str) -> Result<(), ExecutorError> {
    let charset = charset.to_ascii_lowercase();
    let collation = collation.to_ascii_lowercase();
    let valid = if charset == "binary" {
        collation == "binary"
    } else {
        collation.starts_with(&(charset + "_"))
    };
    if valid {
        Ok(())
    } else {
        Err(ExecutorError::InvalidCharsetCollation)
    }
}

/// 校验完整表定义：字符集匹配、列/索引/分区名不重复、
/// 列数不超上限、主键可见、全局索引约束满足等。
pub fn validate_table_definition(table: &TableInfo) -> Result<(), ExecutorError> {
    check_charset_and_collation(&table.charset, &table.collation)?;
    // 普通表必须至少有一列（视图与序列除外）。
    if table.columns.is_empty() && !table.view && !table.sequence {
        return Err(ExecutorError::InvalidTableDefinition(
            "table has no columns".into(),
        ));
    }
    let mut columns = BTreeSet::new();
    for column in &table.columns {
        check_identifier(&column.name, "column")?;
        if !columns.insert(column.name.to_ascii_lowercase()) {
            return Err(ExecutorError::ColumnExists(column.name.clone()));
        }
    }
    if table.columns.len() > 1017 {
        return Err(ExecutorError::TooManyColumns);
    }
    let mut indexes = BTreeSet::new();
    for index in &table.indexes {
        if !indexes.insert(index.name.to_ascii_lowercase()) {
            return Err(ExecutorError::IndexExists(index.name.clone()));
        }
        if index.primary && index.invisible {
            return Err(ExecutorError::InvisiblePrimaryKey);
        }
        check_create_global_index(table, index)?;
    }
    let mut partitions = BTreeSet::new();
    for partition in &table.partitions {
        if !partitions.insert(partition.name.to_ascii_lowercase()) {
            return Err(ExecutorError::InvalidPartition(partition.name.clone()));
        }
    }
    Ok(())
}

/// 校验全局索引的创建条件：只能建在分区表上，且不能覆盖生成列。
pub fn check_create_global_index(
    table: &TableInfo,
    index: &IndexInfo,
) -> Result<(), ExecutorError> {
    if index.global && table.partitions.is_empty() {
        return Err(ExecutorError::GlobalIndexNeedsPartition);
    }
    // 全局索引不支持包含生成列（有依赖表达式的列）。
    if index.global
        && index.columns.iter().any(|name| {
            table
                .columns
                .iter()
                .find(|column| column.name.eq_ignore_ascii_case(name))
                .is_some_and(|column| !column.generated_dependencies.is_empty())
        })
    {
        return Err(ExecutorError::Unsupported(
            "global index on generated column".into(),
        ));
    }
    Ok(())
}

/// 校验两张表定义是否兼容（用于交换分区）：
/// 列数、各列类型/字符集/可空性、索引数、TiFlash 副本数均须一致。
pub fn check_table_def_compatible(
    source: &TableInfo,
    target: &TableInfo,
) -> Result<(), ExecutorError> {
    if source.temporary
        || target.temporary
        || target.view
        || target.sequence
        || source.columns.len() != target.columns.len()
        || source.indexes.len() != target.indexes.len()
    {
        return Err(ExecutorError::InvalidTableDefinition(
            "exchange partition definitions differ".into(),
        ));
    }
    for (left, right) in source.columns.iter().zip(&target.columns) {
        if left.kind != right.kind
            || left.charset != right.charset
            || left.collation != right.collation
            || left.nullable != right.nullable
        {
            return Err(ExecutorError::InvalidTableDefinition(format!(
                "column {} incompatible",
                left.name
            )));
        }
    }
    if source.tiflash_replica_count != target.tiflash_replica_count {
        return Err(ExecutorError::InvalidTableDefinition(
            "TiFlash replica differs".into(),
        ));
    }
    Ok(())
}

/// 为未命名索引生成默认名：以首列名（向量索引用 "vector_index"）为前缀，
/// 冲突时追加 `_2`、`_3` 等序号直到唯一。
pub fn get_name_for_anonymous_index(table: &TableInfo, column_name: &str, vector: bool) -> String {
    let prefix = if vector { "vector_index" } else { column_name };
    let existing: BTreeSet<String> = table
        .indexes
        .iter()
        .map(|index| index.name.to_ascii_lowercase())
        .collect();
    if !existing.contains(&prefix.to_ascii_lowercase()) {
        return prefix.into();
    }
    for suffix in 2.. {
        let candidate = format!("{prefix}_{suffix}");
        if !existing.contains(&candidate.to_ascii_lowercase()) {
            return candidate;
        }
    }
    unreachable!()
}

/// 校验注释长度：超过 `max_bytes` 字节时截断并记录警告。
pub fn validate_comment_length(
    comment: &str,
    max_bytes: usize,
    warnings: &mut Vec<String>,
) -> String {
    if comment.len() <= max_bytes {
        return comment.into();
    }
    // 回退到 UTF-8 字符边界，避免截断出非法字节序列。
    let mut end = max_bytes;
    while !comment.is_char_boundary(end) {
        end -= 1;
    }
    warnings.push("comment is too long and was truncated".into());
    comment[..end].into()
}

/// 判断某表是否禁止删除（不校验表 ID 的简化版本）。
pub fn is_undroppable_table(schema: &str, table: &str) -> bool {
    is_undroppable_table_with_id(schema, table, 0)
}

/// 判断某表是否禁止删除：保留 ID 段内的表、workload_schema 下的表，
/// 以及 mysql 库中若干与 GC（垃圾回收）相关的系统关键表。
pub fn is_undroppable_table_with_id(schema: &str, table: &str, table_id: i64) -> bool {
    const RESERVED_GLOBAL_ID_UPPER_BOUND: i64 = 0x0000_FFFF_FFFF_FFFF;
    const RESERVED_GLOBAL_ID_LOWER_BOUND: i64 = RESERVED_GLOBAL_ID_UPPER_BOUND - 1000;

    // 保留的全局 ID 区间用于系统内部表。
    if table_id > RESERVED_GLOBAL_ID_LOWER_BOUND && table_id <= RESERVED_GLOBAL_ID_UPPER_BOUND {
        return true;
    }
    if schema.eq_ignore_ascii_case("workload_schema") {
        return true;
    }
    schema.eq_ignore_ascii_case("mysql")
        && matches!(
            table.to_ascii_lowercase().as_str(),
            "tidb" | "gc_delete_range" | "gc_delete_range_done"
        )
}

/// `OPTIMIZE TABLE` 不受支持，统一返回错误。
pub fn validate_optimize_table() -> Result<(), ExecutorError> {
    Err(ExecutorError::Unsupported(
        "OPTIMIZE TABLE is not supported".into(),
    ))
}

/// 作业提交时的表锁迁移：把旧表 ID 上的锁复制一份到新表 ID。
///
/// 截断表等操作会为表分配新的物理 ID，提交前先把锁"预迁移"到新 ID，
/// 以便作业成功后新表仍持有原来的锁。
pub fn handle_lock_on_submit(session: &mut SessionContext, old_table_id: i64, new_table_id: i64) {
    if let Some(lock_type) = session.locked_tables.get(&old_table_id).copied() {
        session.locked_tables.insert(new_table_id, lock_type);
    }
}
/// 作业结束后清理表锁：成功则丢弃旧 ID 的锁（锁已随新 ID 生效），
/// 失败则丢弃新 ID 的锁（回退，保留旧 ID 上的锁）。
pub fn handle_lock_on_finish(
    session: &mut SessionContext,
    old_table_id: i64,
    new_table_id: i64,
    success: bool,
) {
    if success {
        session.locked_tables.remove(&old_table_id);
    } else {
        session.locked_tables.remove(&new_table_id);
    }
}

/// 为作业设置其原始 SQL 文本。
///
/// 副本状态更新与解锁表这类系统内部动作不对外记录 SQL，故置为空串。
pub fn set_ddl_job_query(session: &SessionContext, job: &mut DdlJob) {
    job.query = if matches!(
        job.action,
        DdlAction::UpdateTiFlashReplicaStatus | DdlAction::UnlockTable
    ) {
        String::new()
    } else {
        session.query.clone()
    };
}

/// 把作业执行期间产生的警告合并到会话警告列表中。
///
/// 同一警告出现多次时汇总为"N warnings, first warning: ..."的形式。
pub fn append_job_warnings(session: &mut SessionContext, job: &DdlJob) {
    for (message, count) in job.warnings.values() {
        session.warnings.push(if *count == 1 {
            message.clone()
        } else {
            format!("{count} warnings, first warning: {message}")
        });
    }
}

/// 校验作业涉及的库表对象列表中不存在重复项（用于依赖冲突控制）。
pub fn validate_involving_schema(involving: &[Ident]) -> Result<(), ExecutorError> {
    let mut seen = BTreeSet::new();
    for ident in involving {
        if !seen.insert(ident.key()) {
            return Err(ExecutorError::InvalidTableDefinition(
                "duplicate involving schema object".into(),
            ));
        }
    }
    Ok(())
}

/// 根据重试策略数组取第 `attempt` 次的等待间隔。
///
/// 返回值第二项表示是否仍在策略数组范围内；超出范围则复用最后一个间隔。
pub fn get_interval_from_policy(policy: &[Duration], attempt: usize) -> (Duration, bool) {
    if attempt < policy.len() {
        (policy[attempt], true)
    } else {
        (*policy.last().unwrap_or(&Duration::from_millis(500)), false)
    }
}

/// 按 DDL 操作类型选择轮询作业状态的退避间隔。
///
/// 加索引、修改列、分区重组等需要回填大量数据的重操作采用较慢的退避（SLOW），
/// 建表/建库等轻操作用快退避（FAST），其余用普通退避（NORMAL）。
pub fn get_job_check_interval(action: DdlAction, attempt: usize) -> (Duration, bool) {
    const FAST: &[Duration] = &[Duration::from_millis(500)];
    const NORMAL: &[Duration] = &[
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(1),
    ];
    const SLOW: &[Duration] = &[
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(3),
    ];
    match action {
        DdlAction::AddIndex
        | DdlAction::AddPrimaryKey
        | DdlAction::ModifyColumn
        | DdlAction::ReorganizePartition
        | DdlAction::RemovePartitioning
        | DdlAction::AlterPartitioning => get_interval_from_policy(SLOW, attempt),
        DdlAction::CreateTable | DdlAction::CreateSchema => get_interval_from_policy(FAST, attempt),
        _ => get_interval_from_policy(NORMAL, attempt),
    }
}

/// DDL 数据回填（reorganization）阶段的元数据快照。
///
/// 加索引、修改列等操作需要遍历历史数据并写入新结构（reorg/backfill）。
/// 由于该过程异步执行，需固化提交时的会话环境（SQL 模式、时区、
/// 资源组、排序规则版本等），保证回填结果与在线写入一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdlReorgMeta {
    /// 提交时的 SQL 模式（sql_mode）位标志。
    pub sql_mode: u64,
    /// 回填过程收集的警告：错误码 -> 消息。
    pub warnings: BTreeMap<u16, String>,
    /// 每个警告码出现的次数。
    pub warning_counts: BTreeMap<u16, u64>,
    /// 时区名称（如 UTC）。
    pub timezone_name: String,
    /// 时区相对 UTC 的偏移量（秒）。
    pub timezone_offset: i32,
    /// 执行回填所使用的资源组（Resource Group，用于资源隔离与限流）。
    pub resource_group: String,
    /// 元数据格式版本号。
    pub version: u64,
    /// 是否启用新的排序规则（new collation）语义。
    pub use_new_collation: bool,
}

/// 从会话的系统变量中提取并构造回填元数据，缺省项使用合理默认值。
pub fn new_ddl_reorg_meta(session: &SessionContext) -> DdlReorgMeta {
    DdlReorgMeta {
        sql_mode: session
            .system_vars
            .get("sql_mode")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        warnings: BTreeMap::new(),
        warning_counts: BTreeMap::new(),
        timezone_name: session
            .system_vars
            .get("time_zone")
            .cloned()
            .unwrap_or_else(|| "UTC".into()),
        timezone_offset: session
            .system_vars
            .get("time_zone_offset")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        resource_group: session
            .system_vars
            .get("resource_group")
            .cloned()
            .unwrap_or_default(),
        version: 1,
        use_new_collation: session
            .system_vars
            .get("new_collation")
            .is_some_and(|value| value == "ON"),
    }
}

/// 判断某个 ALTER 子句是否可忽略。
///
/// ALGORITHM 与 LOCK 只是变更执行方式的提示，不产生实际元数据变更，
/// 在判定"是否为多模式变更"时应予以剔除。
pub fn is_ignorable_alter_spec(spec: &str) -> bool {
    matches!(spec.to_ascii_uppercase().as_str(), "ALGORITHM" | "LOCK")
}

/// 过滤掉可忽略的 ALTER 子句，返回真正生效的子句列表。
pub fn resolve_alter_specs(specs: &[String]) -> Vec<String> {
    specs
        .iter()
        .filter(|spec| !is_ignorable_alter_spec(spec))
        .cloned()
        .collect()
}

/// 判断一条 ALTER 语句是否包含多个子操作（多模式变更）。
///
/// 多模式变更（MultiSchemaChange）会把多个子操作打包进同一个作业原子执行。
pub fn is_multi_schema_change(specs: &[String]) -> bool {
    resolve_alter_specs(specs).len() > 1
}

/// 返回表实际用于打散写入的分片位数，取行 ID 分片位与 AUTO_RANDOM 位的较大者。
pub fn sharding_bits(table: &TableInfo) -> u64 {
    table.shard_row_id_bits.max(table.auto_random_bits) as u64
}

/// 判断给定列是否为 AUTO_RANDOM 列（即启用了随机主键且为表的首列）。
pub fn is_auto_random_column_id(table: &TableInfo, column_id: i64) -> bool {
    table.auto_random_bits > 0
        && table
            .columns
            .first()
            .is_some_and(|column| column.id == column_id)
}

/// 当表已有索引配置了 Region 切分策略时，提示为新索引也添加切分策略。
///
/// Region 是数据的分片调度单元；新索引若不预先切分，其起始区间容易集中写入
/// 形成热点（write hotspot），因此给出建议性警告。
pub fn warn_missing_region_split_policy(
    table: &TableInfo,
    new_index_name: &str,
    warnings: &mut Vec<String>,
) {
    if table
        .indexes
        .iter()
        .any(|index| index.split_policy.is_some())
    {
        warnings.push(format!("It is recommended to add a region split strategy to the new index '{new_index_name}' to avoid write hotspots"));
    }
}

/// 表访问控制器：统一管理全局只读模式与按连接持有的表锁。
#[derive(Default)]
pub struct TableAccessController {
    /// 是否处于全局只读模式。
    read_only: bool,
    /// 连接 ID -> 该连接持有的锁类型。
    locks: BTreeMap<u64, TableLockType>,
}

impl TableAccessController {
    /// 设置只读模式；存在活跃锁时无法开启只读，返回状态是否发生变化。
    pub fn set_read_only(&mut self, read_only: bool) -> Result<bool, ExecutorError> {
        if read_only && !self.locks.is_empty() {
            return Err(ExecutorError::LockConflict);
        }
        let changed = self.read_only != read_only;
        self.read_only = read_only;
        Ok(changed)
    }

    /// 检查当前是否允许写入；只读模式下返回锁冲突错误。
    pub fn check_write(&self) -> Result<(), ExecutorError> {
        if self.read_only {
            Err(ExecutorError::LockConflict)
        } else {
            Ok(())
        }
    }

    /// 为某连接加表锁。读锁可与其他读锁共存；写锁要求当前无任何锁。
    pub fn lock(
        &mut self,
        connection_id: u64,
        lock_type: TableLockType,
    ) -> Result<(), ExecutorError> {
        if self.read_only {
            return Err(ExecutorError::LockConflict);
        }
        // LOCK TABLES replaces the lock already held by the same connection.
        // Only locks owned by other connections participate in compatibility.
        let other_locks = self
            .locks
            .iter()
            .filter(|(owner, _)| **owner != connection_id)
            .map(|(_, lock_type)| lock_type);
        let compatible = match lock_type {
            TableLockType::Read => other_locks
                .into_iter()
                .all(|existing| *existing == TableLockType::Read),
            TableLockType::Write | TableLockType::WriteLocal => other_locks.count() == 0,
        };
        if !compatible {
            return Err(ExecutorError::LockConflict);
        }
        self.locks.insert(connection_id, lock_type);
        Ok(())
    }

    /// 释放某连接持有的锁，返回是否确有锁被移除。
    pub fn unlock(&mut self, connection_id: u64) -> bool {
        self.locks.remove(&connection_id).is_some()
    }

    /// 清空所有锁并退出只读模式（用于重置控制器状态）。
    pub fn cleanup(&mut self) {
        self.read_only = false;
        self.locks.clear();
    }

    /// 返回当前持有的锁数量。
    pub fn lock_count(&self) -> usize {
        self.locks.len()
    }
}

/// Rebuilds a table definition for `ADMIN REPAIR TABLE` while retaining every
// / physical identifier that owns existing data. New definitions may omit old
/// columns, indexes, or range partitions, but may not invent an object whose
/// name/type cannot be matched to the damaged table.
///
/// 为 `ADMIN REPAIR TABLE`（修复损坏表元数据）重建表定义：
/// 沿用旧定义中所有承载已有数据的物理标识符（表/列/索引/分区 ID）。
/// 新定义可以省略部分旧列、索引或 RANGE 分区，但不能凭空引入
/// 无法与受损表按名称/类型匹配上的对象。
/// `exact_partition_count` 为 true 时要求分区数量与旧表完全一致（用于哈希分区）。
pub fn repair_table_definition(
    old: &TableInfo,
    mut replacement: TableInfo,
    exact_partition_count: bool,
) -> Result<TableInfo, ExecutorError> {
    replacement.id = old.id;
    replacement.schema_id = old.schema_id;
    replacement.auto_increment = old.auto_increment;

    // 列必须按名称一一对上旧定义，并沿用原列 ID，避免底层已落盘数据失配。
    for column in &mut replacement.columns {
        let original = old
            .columns
            .iter()
            .find(|candidate| candidate.name.eq_ignore_ascii_case(&column.name))
            .ok_or_else(|| {
                ExecutorError::InvalidTableDefinition(format!("Column {} has lost", column.name))
            })?;
        if column.kind != original.kind {
            return Err(ExecutorError::InvalidTableDefinition(format!(
                "Column {} type should be the same",
                column.name
            )));
        }
        column.id = original.id;
    }

    // 索引除名称外，还要求列序与索引类型一致，才能复用原索引 ID。
    for index in &mut replacement.indexes {
        let original = old
            .indexes
            .iter()
            .find(|candidate| {
                candidate.name.eq_ignore_ascii_case(&index.name)
                    && candidate.columns.len() == index.columns.len()
                    && candidate
                        .columns
                        .iter()
                        .zip(&index.columns)
                        .all(|(left, right)| left.eq_ignore_ascii_case(right))
            })
            .ok_or_else(|| {
                ExecutorError::InvalidTableDefinition(format!("Index {} has lost", index.name))
            })?;
        if index.unique != original.unique
            || index.primary != original.primary
            || index.vector != original.vector
        {
            return Err(ExecutorError::InvalidTableDefinition(format!(
                "Index {} type should be the same",
                index.name
            )));
        }
        index.id = original.id;
    }

    // 哈希分区这类依赖分区数量的布局，要求新旧定义的分区数完全一致。
    if exact_partition_count && replacement.partitions.len() != old.partitions.len() {
        return Err(ExecutorError::InvalidPartition(
            "Hash partition num should be the same".into(),
        ));
    }
    // RANGE 分区按分区名与边界同时匹配，成功后复用原分区 ID。
    for partition in &mut replacement.partitions {
        let original = old
            .partitions
            .iter()
            .find(|candidate| {
                candidate.name.eq_ignore_ascii_case(&partition.name)
                    && candidate.less_than == partition.less_than
            })
            .ok_or_else(|| {
                ExecutorError::InvalidPartition(format!("Partition {} has lost", partition.name))
            })?;
        partition.id = original.id;
    }

    validate_table_definition(&replacement)?;
    Ok(replacement)
}

/// 修复表注册表：管理 REPAIR MODE（修复模式）下待修复的表集合。
///
/// 开启修复模式后，注册表中的表对普通查询不可见，直到执行修复。
#[derive(Default)]
pub struct RepairTableRegistry {
    /// 是否处于修复模式。
    enabled: bool,
    /// 是否已从存储层拉取过待修复表清单。
    fetched: bool,
    /// 待修复的表：键为 `库.表` 小写标识。
    tables: BTreeMap<String, TableInfo>,
}

impl RepairTableRegistry {
    /// 开启/关闭修复模式；关闭时清空已拉取的待修复清单。
    pub fn set_mode(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.fetched = false;
            self.tables.clear();
        }
    }

    /// 载入待修复的表清单（通常来自存储层扫描结果）。
    pub fn fetch_tables<I>(&mut self, tables: I)
    where
        I: IntoIterator<Item = (Ident, TableInfo)>,
    {
        self.tables = tables
            .into_iter()
            .map(|(ident, table)| (ident.key(), table))
            .collect();
        self.fetched = true;
    }

    /// 判断某表对普通访问是否可见：处于修复模式且在待修复清单中时不可见。
    pub fn is_visible(&self, ident: &Ident) -> bool {
        !(self.enabled && self.tables.contains_key(&ident.key()))
    }

    /// 执行表修复：校验修复模式与前置条件，用新定义修复受损表元数据。
    ///
    /// 系统库（mysql、information_schema 等）不允许修复；修复失败时
    /// 会把旧定义放回清单以便重试。
    pub fn repair(
        &mut self,
        ident: &Ident,
        replacement: TableInfo,
        exact_partition_count: bool,
    ) -> Result<TableInfo, ExecutorError> {
        if !self.enabled {
            return Err(ExecutorError::Unsupported(
                "TiDB is not in REPAIR MODE".into(),
            ));
        }
        if !self.fetched {
            return Err(ExecutorError::Unsupported("repair list is empty".into()));
        }
        if matches!(
            ident.schema.to_ascii_lowercase().as_str(),
            "mysql" | "information_schema" | "performance_schema" | "metrics_schema"
        ) {
            return Err(ExecutorError::Unsupported(
                "memory or system database is not for repair".into(),
            ));
        }
        let old = self.tables.remove(&ident.key()).ok_or_else(|| {
            ExecutorError::TableNotFound(format!("{} is not in repair", ident.key()))
        })?;
        match repair_table_definition(&old, replacement, exact_partition_count) {
            Ok(table) => Ok(table),
            Err(error) => {
                self.tables.insert(ident.key(), old);
                Err(error)
            }
        }
    }
}
