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

// DDL（数据定义语言）相关 AST 节点与 Restore（还原为 SQL 文本）辅助。
//
// 覆盖库选项、索引选项、放置策略（Placement Policy，控制 Region 副本分布）、
// 资源组、外键引用、DROP/TRUNCATE/ALTER DATABASE、序列与表级选项等结构的序列化。
/// 用反引号引用标识符，内部反引号加倍转义。
fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// 用单引号引用字符串，转义反斜杠与单引号。
fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 字符集/排序规则选项对。
pub struct CharsetOpt {
    pub chs: String,
    pub col: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 可区分“空串”与“未设置”的字符串包装（empty 为真表示空值语义）。
pub struct NullString {
    pub string: String,
    pub empty: bool,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 数据库级选项类型：字符集、排序规则、加密、TiFlash 副本、放置策略等。
pub enum DatabaseOptionType {
    #[default]
    None = 0,
    Charset = 1,
    Collate = 2,
    Encryption = 3,
    SetTiFlashReplica = 4,
    PlacementPolicy = 0x300c,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TiFlash（列存引擎）副本规格：副本数与 LOCATION LABELS。
pub struct TiFlashReplicaSpec {
    pub count: u64,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单条数据库选项及其载荷。
pub struct DatabaseOption {
    pub tp: DatabaseOptionType,
    pub value: String,
    pub uint_value: u64,
    pub tiflash_replica: Option<TiFlashReplicaSpec>,
}

impl DatabaseOption {
    /// 构造 CHARACTER SET 选项。
    pub fn charset(value: impl Into<String>) -> Self {
        Self {
            tp: DatabaseOptionType::Charset,
            value: value.into(),
            ..Self::default()
        }
    }

    /// 构造 COLLATE 选项。
    pub fn collate(value: impl Into<String>) -> Self {
        Self {
            tp: DatabaseOptionType::Collate,
            value: value.into(),
            ..Self::default()
        }
    }

    /// 构造 ENCRYPTION 选项。
    pub fn encryption(value: impl Into<String>) -> Self {
        Self {
            tp: DatabaseOptionType::Encryption,
            value: value.into(),
            ..Self::default()
        }
    }

    /// 构造 PLACEMENT POLICY 选项。
    pub fn placement_policy(value: impl Into<String>) -> Self {
        Self {
            tp: DatabaseOptionType::PlacementPolicy,
            value: value.into(),
            ..Self::default()
        }
    }

    /// 构造 SET TIFLASH REPLICA 选项。
    pub fn tiflash_replica(count: u64, labels: Vec<String>) -> Self {
        Self {
            tp: DatabaseOptionType::SetTiFlashReplica,
            tiflash_replica: Some(TiFlashReplicaSpec { count, labels }),
            ..Self::default()
        }
    }

    /// 尝试还原为 SQL 片段；无效类型返回错误。
    pub fn try_restore(&self) -> Result<String, String> {
        Ok(match self.tp {
            DatabaseOptionType::Charset => format!("CHARACTER SET = {}", self.value),
            DatabaseOptionType::Collate => format!("COLLATE = {}", self.value),
            DatabaseOptionType::Encryption => format!("ENCRYPTION = {}", quote_string(&self.value)),
            DatabaseOptionType::PlacementPolicy => {
                format!("PLACEMENT POLICY = {}", quote_name(&self.value))
            }
            // TiFlash 副本：可选 LOCATION LABELS 列表。
            DatabaseOptionType::SetTiFlashReplica => {
                let replica = self.tiflash_replica.as_ref().ok_or_else(|| {
                    "SET TIFLASH REPLICA requires a replica specification".to_owned()
                })?;
                let mut sql = format!("SET TIFLASH REPLICA {}", replica.count);
                if !replica.labels.is_empty() {
                    sql.push_str(" LOCATION LABELS ");
                    sql.push_str(
                        &replica
                            .labels
                            .iter()
                            .map(|label| quote_string(label))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                sql
            }
            DatabaseOptionType::None => return Err("invalid DatabaseOptionType: 0".to_owned()),
        })
    }

    /// 还原为 SQL 片段；假定选项合法。
    pub fn restore(&self) -> String {
        self.try_restore().expect("valid database option")
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 外键 ON DELETE/UPDATE 动作：RESTRICT/CASCADE/SET NULL 等。
pub enum ReferOptionType {
    #[default]
    NoOption = 0,
    Restrict,
    Cascade,
    SetNull,
    NoAction,
    SetDefault,
}

impl ReferOptionType {
    /// 还原为 SQL 关键字；NoOption 为空串。
    pub fn restore(self) -> &'static str {
        match self {
            Self::NoOption => "",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::NoAction => "NO ACTION",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 索引算法/类型：BTree、Hash、向量、全文等。
pub enum IndexType {
    #[default]
    Invalid,
    Btree,
    Hash,
    Rtree,
    Hypo,
    Vector,
    Inverted,
    Hnsw,
    Fulltext,
}

impl IndexType {
    /// 还原 USING 子句中的索引类型名。
    fn restore(self) -> &'static str {
        match self {
            Self::Invalid => "",
            Self::Btree => "BTREE",
            Self::Hash => "HASH",
            Self::Rtree => "RTREE",
            Self::Hypo => "HYPO",
            Self::Vector => "VECTOR",
            Self::Inverted => "INVERTED",
            Self::Hnsw => "HNSW",
            Self::Fulltext => "FULLTEXT",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 索引可见性：VISIBLE / INVISIBLE。
pub enum IndexVisibility {
    #[default]
    Default,
    Visible,
    Invisible,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 主键聚簇类型：CLUSTERED / NONCLUSTERED。
pub enum PrimaryKeyType {
    #[default]
    Default,
    Clustered,
    NonClustered,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引附加选项集合（块大小、注释、可见性、条件索引 WHERE 等）。
pub struct IndexOption {
    pub key_block_size: u64,
    pub tp: IndexType,
    pub comment: String,
    pub parser_name: String,
    pub visibility: IndexVisibility,
    pub primary_key_tp: PrimaryKeyType,
    pub global: bool,
    pub secondary_engine_attr: String,
    pub add_columnar_replica_on_demand: i32,
    pub condition: Option<String>,
}

impl IndexOption {
    /// 是否为全默认（无任何选项需输出）。
    pub fn is_empty(&self) -> bool {
        self.primary_key_tp == PrimaryKeyType::Default
            && self.key_block_size == 0
            && self.tp == IndexType::Invalid
            && self.parser_name.is_empty()
            && self.comment.is_empty()
            && !self.global
            && self.visibility == IndexVisibility::Default
            && self.secondary_engine_attr.is_empty()
            && self.condition.is_none()
    }

    /// 按固定顺序拼接非空索引选项为 SQL 片段。
    pub fn restore(&self) -> String {
        let mut options = Vec::new();
        if self.add_columnar_replica_on_demand > 0 {
            options.push("ADD_COLUMNAR_REPLICA_ON_DEMAND".to_owned());
        }
        match self.primary_key_tp {
            PrimaryKeyType::Clustered => options.push("CLUSTERED".to_owned()),
            PrimaryKeyType::NonClustered => options.push("NONCLUSTERED".to_owned()),
            PrimaryKeyType::Default => {}
        }
        if self.key_block_size > 0 {
            options.push(format!("KEY_BLOCK_SIZE={}", self.key_block_size));
        }
        if self.tp != IndexType::Invalid {
            options.push(format!("USING {}", self.tp.restore()));
        }
        if !self.parser_name.is_empty() {
            options.push(format!("WITH PARSER {}", quote_name(&self.parser_name)));
        }
        if !self.comment.is_empty() {
            options.push(format!("COMMENT {}", quote_string(&self.comment)));
        }
        if self.global {
            options.push("GLOBAL".to_owned());
        }
        match self.visibility {
            IndexVisibility::Visible => options.push("VISIBLE".to_owned()),
            IndexVisibility::Invisible => options.push("INVISIBLE".to_owned()),
            IndexVisibility::Default => {}
        }
        if !self.secondary_engine_attr.is_empty() {
            options.push(format!(
                "SECONDARY_ENGINE_ATTRIBUTE = {}",
                quote_string(&self.secondary_engine_attr)
            ));
        }
        if let Some(condition) = &self.condition {
            options.push(format!("WHERE {condition}"));
        }
        options.join(" ")
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 放置策略选项类型：主 Region、副本数、约束、策略名等。
pub enum PlacementOptionType {
    PrimaryRegion = 0x3000,
    Regions,
    FollowerCount,
    VoterCount,
    LearnerCount,
    Schedule,
    Constraints,
    LeaderConstraints,
    LearnerConstraints,
    FollowerConstraints,
    VoterConstraints,
    SurvivalPreferences,
    Policy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 放置调度策略：均匀或主 Region 多数派。
pub enum PlacementSchedule {
    Even,
    MajorityInPrimary,
}

impl PlacementSchedule {
    /// 还原 SCHEDULE 取值字符串。
    fn as_str(self) -> &'static str {
        match self {
            Self::Even => "EVEN",
            Self::MajorityInPrimary => "MAJORITY_IN_PRIMARY",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条放置策略选项（字符串或数值载荷）。
pub struct PlacementOption {
    pub tp: PlacementOptionType,
    pub str_value: String,
    pub uint_value: u64,
}

impl PlacementOption {
    /// 构造 PRIMARY_REGION 选项。
    pub fn primary_region(value: impl Into<String>) -> Self {
        Self::string(PlacementOptionType::PrimaryRegion, value)
    }
    /// 构造 REGIONS 选项。
    pub fn regions(value: impl Into<String>) -> Self {
        Self::string(PlacementOptionType::Regions, value)
    }
    /// 构造 FOLLOWERS 数量选项。
    pub fn followers(value: u64) -> Self {
        Self::number(PlacementOptionType::FollowerCount, value)
    }
    /// 构造 SCHEDULE 选项。
    pub fn schedule(value: PlacementSchedule) -> Self {
        Self::string(PlacementOptionType::Schedule, value.as_str())
    }
    /// 构造 PLACEMENT POLICY 引用。
    pub fn policy(value: impl Into<String>) -> Self {
        Self::string(PlacementOptionType::Policy, value)
    }

    /// 构造字符串型放置选项。
    pub fn string(tp: PlacementOptionType, value: impl Into<String>) -> Self {
        Self {
            tp,
            str_value: value.into(),
            uint_value: 0,
        }
    }

    /// 构造数值型放置选项。
    pub fn number(tp: PlacementOptionType, value: u64) -> Self {
        Self {
            tp,
            str_value: String::new(),
            uint_value: value,
        }
    }

    /// 还原单条放置选项为 `KEY = value` 形式。
    pub fn restore(&self) -> String {
        let string_keyword = match self.tp {
            PlacementOptionType::PrimaryRegion => Some("PRIMARY_REGION"),
            PlacementOptionType::Regions => Some("REGIONS"),
            PlacementOptionType::Schedule => Some("SCHEDULE"),
            PlacementOptionType::Constraints => Some("CONSTRAINTS"),
            PlacementOptionType::LeaderConstraints => Some("LEADER_CONSTRAINTS"),
            PlacementOptionType::LearnerConstraints => Some("LEARNER_CONSTRAINTS"),
            PlacementOptionType::FollowerConstraints => Some("FOLLOWER_CONSTRAINTS"),
            PlacementOptionType::VoterConstraints => Some("VOTER_CONSTRAINTS"),
            PlacementOptionType::SurvivalPreferences => Some("SURVIVAL_PREFERENCES"),
            _ => None,
        };
        if let Some(keyword) = string_keyword {
            return format!("{keyword} = {}", quote_string(&self.str_value));
        }
        match self.tp {
            PlacementOptionType::FollowerCount => format!("FOLLOWERS = {}", self.uint_value),
            PlacementOptionType::VoterCount => format!("VOTERS = {}", self.uint_value),
            PlacementOptionType::LearnerCount => format!("LEARNERS = {}", self.uint_value),
            PlacementOptionType::Policy => {
                format!("PLACEMENT POLICY = {}", quote_name(&self.str_value))
            }
            _ => unreachable!(),
        }
    }
}

/// 将多条放置选项以空格拼接还原。
pub fn restore_placement_options(options: &[PlacementOption]) -> String {
    options
        .iter()
        .map(PlacementOption::restore)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 资源组突发（burstable）模式：关闭/适度/无限。
pub enum BurstableType {
    #[default]
    Disable,
    Moderated,
    Unlimited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 资源组优先级：Low/Medium/High。
pub enum ResourceGroupPriority {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 资源组配额项类型：RU 速率、优先级、CPU、IO 带宽、突发等。
/// RU（Request Unit）是标准化请求资源计量单位。
pub enum ResourceUnitType {
    RuRate,
    Priority,
    Cpu,
    IoReadBandwidth,
    IoWriteBandwidth,
    Burstable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条资源组选项。
pub struct ResourceGroupOption {
    pub tp: ResourceUnitType,
    pub str_value: String,
    pub uint_value: u64,
    pub burstable: BurstableType,
}

impl ResourceGroupOption {
    /// 构造 RU_PER_SEC 配额。
    pub fn ru_per_sec(value: u64) -> Self {
        Self {
            tp: ResourceUnitType::RuRate,
            str_value: String::new(),
            uint_value: value,
            burstable: BurstableType::Disable,
        }
    }
    /// 构造 PRIORITY；内部用 1/8/16 编码 Low/Medium/High。
    pub fn priority(value: ResourceGroupPriority) -> Self {
        Self {
            tp: ResourceUnitType::Priority,
            str_value: String::new(),
            uint_value: match value {
                ResourceGroupPriority::Low => 1,
                ResourceGroupPriority::Medium => 8,
                ResourceGroupPriority::High => 16,
            },
            burstable: BurstableType::Disable,
        }
    }
    /// 构造 BURSTABLE 模式。
    pub fn burstable(value: BurstableType) -> Self {
        Self {
            tp: ResourceUnitType::Burstable,
            str_value: String::new(),
            uint_value: 0,
            burstable: value,
        }
    }

    /// 还原资源组选项为 SQL 赋值片段。
    pub fn restore(&self) -> String {
        match self.tp {
            ResourceUnitType::RuRate if self.burstable == BurstableType::Unlimited => {
                "RU_PER_SEC = UNLIMITED".to_owned()
            }
            ResourceUnitType::RuRate => format!("RU_PER_SEC = {}", self.uint_value),
            ResourceUnitType::Priority => format!(
                "PRIORITY = {}",
                match self.uint_value {
                    16 => "HIGH",
                    8 => "MEDIUM",
                    _ => "LOW",
                }
            ),
            ResourceUnitType::Cpu => format!("CPU = {}", quote_string(&self.str_value)),
            ResourceUnitType::IoReadBandwidth => {
                format!("IO_READ_BANDWIDTH = {}", quote_string(&self.str_value))
            }
            ResourceUnitType::IoWriteBandwidth => {
                format!("IO_WRITE_BANDWIDTH = {}", quote_string(&self.str_value))
            }
            ResourceUnitType::Burstable => format!(
                "BURSTABLE = {}",
                match self.burstable {
                    BurstableType::Disable => "OFF",
                    BurstableType::Moderated => "MODERATED",
                    BurstableType::Unlimited => "UNLIMITED",
                }
            ),
        }
    }
}

/// 将多条资源组选项以逗号拼接还原。
pub fn restore_resource_group_options(options: &[ResourceGroupOption]) -> String {
    options
        .iter()
        .map(ResourceGroupOption::restore)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表名：可选 schema 限定。
pub struct TableName {
    pub schema: String,
    pub name: String,
}

impl TableName {
    /// 仅表名（无 schema）。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: String::new(),
            name: name.into(),
        }
    }
    /// schema.table 限定名。
    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }
    /// 还原为反引号引用的表名。
    pub fn restore(&self) -> String {
        if self.schema.is_empty() {
            quote_name(&self.name)
        } else {
            format!("{}.{}", quote_name(&self.schema), quote_name(&self.name))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// RENAME 中的 old TO new 表名对。
pub struct TableToTable {
    pub old_table: TableName,
    pub new_table: TableName,
}

impl TableToTable {
    /// 还原为 `old TO new`。
    pub fn restore(&self) -> String {
        format!(
            "{} TO {}",
            self.old_table.restore(),
            self.new_table.restore()
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引列/表达式部件：列名、前缀长度、降序或表达式。
pub struct IndexPartSpecification {
    pub column: Option<TableName>,
    pub expression: Option<String>,
    pub length: Option<u64>,
    pub descending: bool,
}

impl IndexPartSpecification {
    /// 按列构造索引部件。
    pub fn column(name: impl Into<String>, length: Option<u64>, descending: bool) -> Self {
        Self {
            column: Some(TableName::new(name)),
            expression: None,
            length,
            descending,
        }
    }
    /// 按表达式构造索引部件。
    pub fn expression(expression: impl Into<String>, descending: bool) -> Self {
        Self {
            column: None,
            expression: Some(expression.into()),
            length: None,
            descending,
        }
    }
    /// 还原索引部件（含可选长度与 DESC）。
    pub fn restore(&self) -> String {
        let mut sql = if let Some(column) = &self.column {
            let mut column = column.restore();
            if let Some(length) = self.length {
                column.push_str(&format!("({length})"));
            }
            column
        } else {
            format!("({})", self.expression.as_deref().unwrap_or_default())
        };
        if self.descending {
            sql.push_str(" DESC");
        }
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 外键 REFERENCES 定义：引用表、列、MATCH 与 ON DELETE/UPDATE。
pub struct ReferenceDef {
    pub table: TableName,
    pub columns: Vec<IndexPartSpecification>,
    pub match_type: Option<&'static str>,
    pub on_delete: ReferOptionType,
    pub on_update: ReferOptionType,
}

impl ReferenceDef {
    /// 还原 REFERENCES 子句。
    pub fn restore(&self) -> String {
        let mut sql = format!(
            "REFERENCES {}({})",
            self.table.restore(),
            self.columns
                .iter()
                .map(IndexPartSpecification::restore)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(match_type) = self.match_type {
            sql.push_str(&format!(" MATCH {match_type}"));
        }
        if self.on_delete != ReferOptionType::NoOption {
            sql.push_str(&format!(" ON DELETE {}", self.on_delete.restore()));
        }
        if self.on_update != ReferOptionType::NoOption {
            sql.push_str(&format!(" ON UPDATE {}", self.on_update.restore()));
        }
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DROP TABLE/VIEW 语句节点。
pub struct DropTableStatement {
    pub temporary: bool,
    pub is_view: bool,
    pub if_exists: bool,
    pub tables: Vec<TableName>,
}

impl DropTableStatement {
    /// 还原 DROP TABLE/VIEW 语句。
    pub fn restore(&self) -> String {
        let mut sql = "DROP ".to_owned();
        if self.temporary {
            sql.push_str("TEMPORARY ");
        }
        sql.push_str(if self.is_view { "VIEW " } else { "TABLE " });
        if self.if_exists {
            sql.push_str("IF EXISTS ");
        }
        sql.push_str(
            &self
                .tables
                .iter()
                .map(TableName::restore)
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TRUNCATE TABLE 语句节点。
pub struct TruncateTableStatement {
    pub table: TableName,
}
impl TruncateTableStatement {
    /// 还原 TRUNCATE TABLE 语句。
    pub fn restore(&self) -> String {
        format!("TRUNCATE TABLE {}", self.table.restore())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ALTER 时列位置：FIRST 或 AFTER col。
pub struct ColumnPosition {
    pub first: bool,
    pub after: Option<String>,
}
impl ColumnPosition {
    /// 还原列位置子句；未指定则为空。
    pub fn restore(&self) -> String {
        if self.first {
            "FIRST".to_owned()
        } else if let Some(after) = &self.after {
            format!("AFTER {}", quote_name(after))
        } else {
            String::new()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// ALTER DATABASE 语句节点。
pub struct AlterDatabaseStatement {
    pub name: String,
    pub alter_default_database: bool,
    pub options: Vec<DatabaseOption>,
}
impl AlterDatabaseStatement {
    /// 还原 ALTER DATABASE；可省略库名表示当前库。
    pub fn restore(&self) -> String {
        let mut sql = "ALTER DATABASE".to_owned();
        if !self.alter_default_database {
            sql.push(' ');
            sql.push_str(&quote_name(&self.name));
        }
        for option in &self.options {
            sql.push(' ');
            sql.push_str(&option.restore());
        }
        sql
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 放置策略 DDL 动作：CREATE 或 ALTER。
pub enum PlacementPolicyAction {
    Create,
    Alter,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// CREATE/ALTER PLACEMENT POLICY 语句。
pub struct PlacementPolicyStatement {
    pub action: PlacementPolicyAction,
    pub if_not_exists: bool,
    pub name: String,
    pub options: Vec<PlacementOption>,
}
impl PlacementPolicyStatement {
    /// 还原放置策略 DDL。
    pub fn restore(&self) -> String {
        let mut sql = format!(
            "{} PLACEMENT POLICY ",
            match self.action {
                PlacementPolicyAction::Create => "CREATE",
                PlacementPolicyAction::Alter => "ALTER",
            }
        );
        if self.if_not_exists && self.action == PlacementPolicyAction::Create {
            sql.push_str("IF NOT EXISTS ");
        }
        sql.push_str(&quote_name(&self.name));
        if !self.options.is_empty() {
            sql.push(' ');
            sql.push_str(&restore_placement_options(&self.options));
        }
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DROP PLACEMENT POLICY 语句。
pub struct DropPlacementPolicyStatement {
    pub if_exists: bool,
    pub name: String,
}
impl DropPlacementPolicyStatement {
    /// 还原 DROP PLACEMENT POLICY。
    pub fn restore(&self) -> String {
        format!(
            "DROP PLACEMENT POLICY {}{}",
            if self.if_exists { "IF EXISTS " } else { "" },
            quote_name(&self.name)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// FLASHBACK DATABASE 语句（按名称恢复，可选 TO 新名）。
pub struct FlashBackDatabaseStatement {
    pub name: String,
    pub new_name: Option<String>,
}
impl FlashBackDatabaseStatement {
    /// 还原 FLASHBACK DATABASE。
    pub fn restore(&self) -> String {
        let mut sql = format!("FLASHBACK DATABASE {}", quote_name(&self.name));
        if let Some(new_name) = &self.new_name {
            sql.push_str(&format!(" TO {}", quote_name(new_name)));
        }
        sql
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 资源组 DDL 动作：CREATE 或 ALTER。
pub enum ResourceGroupAction {
    Create,
    Alter,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// CREATE/ALTER RESOURCE GROUP 语句。
pub struct ResourceGroupStatement {
    pub action: ResourceGroupAction,
    pub if_not_exists: bool,
    pub name: String,
    pub options: Vec<ResourceGroupOption>,
}
impl ResourceGroupStatement {
    /// 还原资源组 DDL。
    pub fn restore(&self) -> String {
        let mut sql = format!(
            "{} RESOURCE GROUP ",
            match self.action {
                ResourceGroupAction::Create => "CREATE",
                ResourceGroupAction::Alter => "ALTER",
            }
        );
        if self.if_not_exists && self.action == ResourceGroupAction::Create {
            sql.push_str("IF NOT EXISTS ");
        }
        sql.push_str(&quote_name(&self.name));
        if !self.options.is_empty() {
            sql.push(' ');
            sql.push_str(&restore_resource_group_options(&self.options));
        }
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 序列（SEQUENCE）选项：增量、起止、缓存、CYCLE 等。
pub enum SequenceOption {
    Increment(i64),
    Start(i64),
    MinValue(i64),
    NoMinValue,
    MaxValue(i64),
    NoMaxValue,
    Cache(i64),
    NoCache,
    Cycle,
    NoCycle,
}
impl SequenceOption {
    /// 还原单条序列选项。
    pub fn restore(&self) -> String {
        match self {
            Self::Increment(value) => format!("INCREMENT BY {value}"),
            Self::Start(value) => format!("START WITH {value}"),
            Self::MinValue(value) => format!("MINVALUE {value}"),
            Self::NoMinValue => "NO MINVALUE".to_owned(),
            Self::MaxValue(value) => format!("MAXVALUE {value}"),
            Self::NoMaxValue => "NO MAXVALUE".to_owned(),
            Self::Cache(value) => format!("CACHE {value}"),
            Self::NoCache => "NOCACHE".to_owned(),
            Self::Cycle => "CYCLE".to_owned(),
            Self::NoCycle => "NOCYCLE".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// CREATE/ALTER SEQUENCE 语句。
pub struct SequenceStatement {
    pub create: bool,
    pub if_not_exists: bool,
    pub if_exists: bool,
    pub name: TableName,
    pub options: Vec<SequenceOption>,
}
impl SequenceStatement {
    /// 还原序列 DDL。
    pub fn restore(&self) -> String {
        let mut sql = format!("{} SEQUENCE ", if self.create { "CREATE" } else { "ALTER" });
        if self.create && self.if_not_exists {
            sql.push_str("IF NOT EXISTS ");
        }
        if !self.create && self.if_exists {
            sql.push_str("IF EXISTS ");
        }
        sql.push_str(&self.name.restore());
        for option in &self.options {
            sql.push(' ');
            sql.push_str(&option.restore());
        }
        sql
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表级选项：放置策略、TTL、预分裂 Region 等。
/// TTL（Time To Live）按表达式自动过期行；PreSplitRegions 预创建 Region 分片。
pub enum TableOption {
    PlacementPolicy(String),
    Ttl(String),
    TtlEnable(bool),
    PreSplitRegions(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 表选项还原控制标志：特殊注释包装、跳过放置、强制关闭 TTL。
pub struct TableRestoreFlags {
    pub special_comments: bool,
    pub skip_placement: bool,
    pub force_ttl_enable_off: bool,
}

impl TableOption {
    /// 按 flags 还原表选项；可包在 `/*T![...]*/` 特殊注释中。
    pub fn restore(&self, flags: TableRestoreFlags) -> String {
        match self {
            Self::PlacementPolicy(_) if flags.skip_placement => String::new(),
            Self::PlacementPolicy(policy) => {
                let body = format!("PLACEMENT POLICY = {}", quote_name(policy));
                if flags.special_comments {
                    format!("/*T![placement] {body} */")
                } else {
                    body
                }
            }
            Self::Ttl(expression) => {
                let body = format!("TTL = {expression}");
                if flags.special_comments {
                    format!("/*T![ttl] {body} */")
                } else {
                    body
                }
            }
            Self::TtlEnable(value) => {
                let enabled = if flags.force_ttl_enable_off {
                    false
                } else {
                    *value
                };
                let body = format!("TTL_ENABLE = '{}'", if enabled { "ON" } else { "OFF" });
                if flags.special_comments {
                    format!("/*T![ttl] {body} */")
                } else {
                    body
                }
            }
            Self::PreSplitRegions(value) => {
                let body = format!("PRE_SPLIT_REGIONS = {value}");
                if flags.special_comments {
                    format!("/*T![pre_split] {body} */")
                } else {
                    body
                }
            }
        }
    }
}
