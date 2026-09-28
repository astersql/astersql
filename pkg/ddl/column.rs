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

// DDL（数据定义语言）列变更模块。
//
// 本模块承载表结构中"列"相关的 DDL 元数据与操作逻辑，包括：
// - 列/索引/表的元信息结构（`ColumnInfo`、`IndexInfo`、`TableInfo` 等）；
// - 加列、删列、移动列位置等操作的辅助函数；
// - 在线 DDL（Online DDL）状态机所需的 Schema 状态（`SchemaState`）。
//
// 背景：分布式数据库通常采用类似 Google F1 的在线 Schema 变更算法，
// 列会依次经历 None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public
// 等中间状态，保证集群中不同节点在相邻两个 Schema 版本间仍能正确读写数据。

use std::collections::HashSet;

/// Schema 对象（列、索引等）在在线 DDL 状态机中的可见性状态。
///
/// 每个状态限定了该对象对 DML（增删改查）操作的可见程度：
/// - `None`：对象刚创建，对任何操作都不可见；
/// - `DeleteOnly`：仅删除操作可见（删除时需要维护该对象）；
/// - `WriteOnly`：写操作（插入/更新/删除）可见，读操作不可见；
/// - `WriteReorganization`：写可见，同时后台在做数据重组（如回填索引）；
/// - `Public`：对象完全公开，读写均可见，DDL 完成。

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub enum SchemaState {
    /// 初始状态：对象不可见。
    #[default]
    None,
    /// 仅删除可见：删除行时需同步维护该对象。
    DeleteOnly,
    /// 仅写可见：所有写操作需维护该对象，但读操作看不到它。
    WriteOnly,
    /// 写重组状态：写可见，后台正在回填/重组数据。
    WriteReorganization,
    /// 公开状态：对象对所有读写操作可见，变更完成。
    Public,
}

/// 新增列在表中的目标位置（对应 SQL 中 `ADD COLUMN ... FIRST/AFTER col`）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ColumnPosition {
    /// 未指定位置：追加到表末尾。
    None,
    /// 放在第一列。
    First,
    /// 放在指定列名之后。
    After(String),
}

/// 列的数据类型种类，对应 MySQL 的常见字段类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnKind {
    Integer,
    Varchar,
    String,
    Bit,
    Year,
    Timestamp,
    DateTime,
    Enum,
    Set,
    TinyBlob,
    Blob,
    MediumBlob,
    LongBlob,
}

/// 字段类型的完整描述：类型种类加上长度、精度、字符集等修饰属性。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldType {
    /// 数据类型种类。
    pub kind: ColumnKind,
    /// 显示长度/最大长度（如 `VARCHAR(255)` 中的 255）。
    pub flen: usize,
    /// 小数位数（用于定点/浮点类型）。
    pub decimal: i32,
    /// 字符集名称（如 utf8mb4）。
    pub charset: String,
    /// 排序规则（collation），决定字符串比较与排序方式。
    pub collation: String,
    /// 是否为二进制类型（BINARY 属性）。
    pub binary: bool,
    /// 是否无符号（UNSIGNED）。
    pub unsigned: bool,
    /// 是否零填充（ZEROFILL，显示时左侧补 0）。
    pub zerofill: bool,
}

impl FieldType {
    /// 构造默认的整数类型（对应 MySQL `INT(11)`，二进制字符集）。
    pub fn integer() -> Self {
        Self {
            kind: ColumnKind::Integer,
            flen: 11,
            decimal: 0,
            charset: "binary".into(),
            collation: "binary".into(),
            binary: false,
            unsigned: false,
            zerofill: false,
        }
    }
}

/// 列的默认值表示（对应 `DEFAULT ...` 子句）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefaultValue {
    /// 默认为 NULL。
    Null,
    /// 整数字面量默认值。
    Integer(i64),
    /// 字符串字面量默认值。
    String(String),
    /// 默认值为当前时间戳（`CURRENT_TIMESTAMP`）。
    CurrentTimestamp,
    /// 默认值为表达式（以字符串形式保存）。
    Expression(String),
}

/// 列的元信息，是 DDL 变更操作的核心数据结构。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnInfo {
    /// 列 ID，表内唯一且单调递增，改类型等操作会分配新 ID。
    pub id: i64,
    /// 列名（存储时统一转为小写，MySQL 列名不区分大小写）。
    pub name: String,
    /// 在线 DDL 状态机中的当前状态。
    pub state: SchemaState,
    /// 列在表中的位置偏移（从 0 开始）。
    pub offset: usize,
    /// 是否为隐藏列（如表达式索引内部生成的虚拟列）。
    pub hidden: bool,
    /// 是否声明为 NOT NULL。
    pub not_null: bool,
    /// 在"修改列为 NOT NULL"的中间阶段阻止插入 NULL 值。
    pub prevent_null_insert: bool,
    /// 是否为自增列（AUTO_INCREMENT）。
    pub auto_increment: bool,
    /// 是否为生成列（Generated Column，值由表达式计算得出）。
    pub generated: bool,
    /// 生成列是否为 STORED（物理存储计算结果，否则为 VIRTUAL）。
    pub generated_stored: bool,
    /// 生成列的表达式文本。
    pub generated_expression: String,
    /// 生成列表达式依赖的其他列名集合。
    pub dependencies: HashSet<String>,
    /// 字段类型描述。
    pub field_type: FieldType,
    /// 当前默认值。
    pub default_value: Option<DefaultValue>,
    /// 原始默认值：改列过程中为旧数据回填保留的默认值。
    pub origin_default_value: Option<DefaultValue>,
    /// 改列（modify column）时记录的依赖列偏移。
    pub change_dependency_offset: Option<usize>,
}

impl ColumnInfo {
    /// 按给定名称与字段类型创建新列，其余属性取默认值。
    /// 列名会被统一转成小写以匹配 MySQL 的大小写不敏感语义。
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            id: 0,
            name: name.into().to_ascii_lowercase(),
            state: SchemaState::None,
            offset: 0,
            hidden: false,
            not_null: false,
            prevent_null_insert: false,
            auto_increment: false,
            generated: false,
            generated_stored: false,
            generated_expression: String::new(),
            dependencies: HashSet::new(),
            field_type,
            default_value: None,
            origin_default_value: None,
            change_dependency_offset: None,
        }
    }
}

/// 索引中引用的单个列。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexColumn {
    /// 被索引的列名。
    pub name: String,
    /// 该列在表中的偏移。
    pub offset: usize,
    /// 前缀索引长度（`INDEX(col(10))` 中的 10）；`None` 表示索引整列。
    pub length: Option<usize>,
    /// 改列过程中是否使用变更后（changing）的新类型。
    pub use_changing_type: bool,
}

/// 索引的元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引 ID，表内唯一。
    pub id: i64,
    /// 索引名。
    pub name: String,
    /// 在线 DDL 状态机中的当前状态。
    pub state: SchemaState,
    /// 索引覆盖的列列表（多列即为联合索引）。
    pub columns: Vec<IndexColumn>,
    /// 是否为主键索引。
    pub primary: bool,
    /// 是否为列存（columnar）索引，服务于分析型查询。
    pub columnar: bool,
}

/// 外键约束的元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForeignKeyInfo {
    /// 外键约束名。
    pub name: String,
    /// 参与外键的本表列名列表。
    pub columns: Vec<String>,
}

/// 表的元信息：包含列、索引、外键等 Schema 定义。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// 表 ID，全局唯一。
    pub id: i64,
    /// 表名。
    pub name: String,
    /// 表级默认字符集。
    pub charset: String,
    /// 表级默认排序规则。
    pub collation: String,
    /// 是否配置了 TiFlash 列存副本（用于 HTAP 分析加速）。
    pub tiflash_replica: bool,
    /// 已分配的最大列 ID，用于为新列分配递增 ID。
    pub max_column_id: i64,
    /// 全部列，按 offset 顺序排列。
    pub columns: Vec<ColumnInfo>,
    /// 全部索引。
    pub indices: Vec<IndexInfo>,
    /// 全部外键约束。
    pub foreign_keys: Vec<ForeignKeyInfo>,
    /// AUTO_RANDOM 的分片位数（0 表示未启用）。
    /// AUTO_RANDOM 通过在主键高位混入随机分片位来打散写入热点。
    pub auto_random_bits: u64,
}

impl TableInfo {
    /// 创建空表元信息，默认使用 utf8mb4 字符集。
    pub fn new(id: i64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            charset: "utf8mb4".into(),
            collation: "utf8mb4_bin".into(),
            tiflash_replica: false,
            max_column_id: 0,
            columns: Vec::new(),
            indices: Vec::new(),
            foreign_keys: Vec::new(),
            auto_random_bits: 0,
        }
    }

    /// 将 `from` 偏移处的列移动到 `to` 偏移处，并重排所有列的 offset。
    ///
    /// 用于 `ALTER TABLE ... MODIFY/CHANGE ... FIRST/AFTER` 调整列顺序。
    pub fn move_column_info(&mut self, from: usize, to: usize) -> Result<(), ColumnError> {
        if from >= self.columns.len() || to >= self.columns.len() {
            return Err(ColumnError::InvalidOffset);
        }
        if from != to {
            let column = self.columns.remove(from);
            self.columns.insert(to, column);
        }
        // 移动后统一按数组下标刷新每列的 offset，保持二者一致。
        for (offset, column) in self.columns.iter_mut().enumerate() {
            column.offset = offset;
        }
        Ok(())
    }
}

/// 列相关 DDL 操作可能产生的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ColumnError {
    /// 同名列已存在。
    ColumnExists(String),
    /// 指定的列不存在。
    ColumnNotFound(String),
    /// 表只剩一列时不允许删除该列。
    CannotDropOnlyColumn(String),
    /// 列被主键、联合索引或列存索引引用，不能直接删除。
    CannotDropIndexedColumn(String),
    /// 列偏移越界。
    InvalidOffset,
    /// 列数超过上限。
    TooManyColumns { count: usize, limit: usize },
    /// AUTO_RANDOM 自增部分的位数不足以容纳当前 ID。
    AutoRandomOverflow,
}

/// 为表分配一个新的列 ID（在 `max_column_id` 上递增）。
pub fn allocate_column_id(table: &mut TableInfo) -> i64 {
    table.max_column_id += 1;
    table.max_column_id
}

/// 初始化新列（分配 ID、置 None 状态、追加到表尾）并加入表中，返回新列 ID。
///
/// 新列从 `SchemaState::None` 开始，随后由 DDL 状态机逐步推进到 Public。
pub fn init_and_add_column_to_table(table: &mut TableInfo, mut column: ColumnInfo) -> i64 {
    column.id = allocate_column_id(table);
    column.state = SchemaState::None;
    column.offset = table.columns.len();
    let id = column.id;
    table.columns.push(column);
    id
}

/// 校验 `AFTER col` 中引用的列在表内存在，否则返回 `ColumnNotFound`。
pub fn check_after_position_exists(
    table: &TableInfo,
    position: &ColumnPosition,
) -> Result<(), ColumnError> {
    if let ColumnPosition::After(name) = position
        && !table.columns.iter().any(|column| column.name == *name)
    {
        return Err(ColumnError::ColumnNotFound(name.clone()));
    }
    Ok(())
}

/// 根据目标位置计算列应移动到的偏移。
///
/// `AFTER name` 仅匹配已 Public 的列；若当前列在目标列之前，
/// 移除当前列后目标列会左移一位，因此目标偏移取值有 +1 的差别。
pub fn locate_offset_to_move(
    current_offset: usize,
    position: &ColumnPosition,
    table: &TableInfo,
) -> Result<usize, ColumnError> {
    match position {
        ColumnPosition::None => Ok(current_offset),
        ColumnPosition::First => Ok(0),
        ColumnPosition::After(name) => {
            let column = table
                .columns
                .iter()
                .find(|column| column.name == *name && column.state == SchemaState::Public)
                .ok_or_else(|| ColumnError::ColumnNotFound(name.clone()))?;
            // 当前列在目标列之前：删除后目标列 offset 左移一位，直接用其 offset；
            // 当前列在目标列之后：需放到目标列的下一个位置。
            if current_offset <= column.offset {
                Ok(column.offset)
            } else {
                Ok(column.offset + 1)
            }
        }
    }
}

/// 改列（modify column）后同步更新索引列的引用信息。
///
/// 将索引列的名称与偏移改为变更后列的值；若新类型不再是变长/定长字符串，
/// 或前缀长度已不小于新类型的总长度，则前缀索引长度失去意义，置为 `None`。
pub fn update_index_column(index_column: &mut IndexColumn, changing: &ColumnInfo) {
    index_column.name.clone_from(&changing.name);
    index_column.offset = changing.offset;
    if !matches!(
        changing.field_type.kind,
        ColumnKind::Varchar
            | ColumnKind::String
            | ColumnKind::TinyBlob
            | ColumnKind::Blob
            | ColumnKind::MediumBlob
            | ColumnKind::LongBlob
    ) || index_column
        .length
        .is_some_and(|length| changing.field_type.flen <= length)
    {
        index_column.length = None;
    }
}

/// 列出仅由指定列单独构成的索引（单列索引），删列时这些索引需要一并删除。
pub fn list_indices_with_column<'a>(name: &str, indices: &'a [IndexInfo]) -> Vec<&'a IndexInfo> {
    indices
        .iter()
        .filter(|index| index.columns.len() == 1 && index.columns[0].name == name)
        .collect()
}

/// 检查列是否允许被删除。
///
/// 两条限制：表至少保留一列；列被主键、列存索引或联合索引引用时
/// 不能直接删除（普通单列索引会随列一起删除，因此不在此列）。
pub fn ensure_column_droppable(table: &TableInfo, name: &str) -> Result<(), ColumnError> {
    if table.columns.len() == 1 {
        return Err(ColumnError::CannotDropOnlyColumn(name.into()));
    }
    for index in &table.indices {
        if (index.primary || index.columnar || index.columns.len() > 1)
            && index.columns.iter().any(|column| column.name == name)
        {
            return Err(ColumnError::CannotDropIndexedColumn(name.into()));
        }
    }
    Ok(())
}

/// 删除指定列及其单列索引，返回被删除的索引 ID 列表。
///
/// 流程：先做可删性检查，再删除仅由该列构成的索引，
/// 最后删除列本身并重排剩余列的 offset。
pub fn remove_column_and_single_indices(
    table: &mut TableInfo,
    column_id: i64,
) -> Result<Vec<i64>, ColumnError> {
    let column = table
        .columns
        .iter()
        .find(|column| column.id == column_id)
        .ok_or_else(|| ColumnError::ColumnNotFound(column_id.to_string()))?;
    ensure_column_droppable(table, &column.name)?;
    let name = column.name.clone();
    let mut removed = Vec::new();
    // 移除单列索引并记录其 ID，供调用方后续清理索引数据。
    table.indices.retain(|index| {
        let remove = index.columns.len() == 1 && index.columns[0].name == name;
        if remove {
            removed.push(index.id);
        }
        !remove
    });
    table.columns.retain(|column| column.id != column_id);
    // 删除列后重排剩余列的 offset，保持与数组下标一致。
    for (offset, column) in table.columns.iter_mut().enumerate() {
        column.offset = offset;
    }
    Ok(removed)
}

/// 构建 DDL 作业的元素列表：列本身加上受影响的索引。
///
/// 元素（element）是 DDL 作业中需要处理的 Schema 对象单元，
/// 以 (ID, 类型标记) 的形式记录，供作业调度与回滚使用。
pub fn build_elements(column: &ColumnInfo, indices: &[IndexInfo]) -> Vec<(i64, &'static str)> {
    let mut elements = Vec::with_capacity(indices.len() + 1);
    elements.push((column.id, "column"));
    elements.extend(indices.iter().map(|index| (index.id, "index")));
    elements
}

/// 检查加列后的列数是否超过上限（对应 MySQL 的 table_column_count_limit）。
pub fn check_add_column_too_many_columns(
    column_count: usize,
    limit: usize,
) -> Result<(), ColumnError> {
    if column_count > limit {
        Err(ColumnError::TooManyColumns {
            count: column_count,
            limit,
        })
    } else {
        Ok(())
    }
}

/// 校验新的 AUTO_RANDOM 位数配置是否能容纳当前已分配的 ID。
///
/// AUTO_RANDOM 主键的 64 位被划分为：分片位（shard_bits，打散写入热点）、
/// 保留位（range_bits）与剩余的自增位。若当前 ID 占用的位数已超过
/// 调整后的自增位数，则新配置会导致 ID 溢出，必须拒绝。
pub fn check_new_auto_random_bits(
    current_id: u64,
    shard_bits: u64,
    range_bits: u64,
) -> Result<(), ColumnError> {
    // 自增部分可用位数 = 64 - 分片位 - 保留位。
    let incremental_bits = 64_u64.saturating_sub(shard_bits).saturating_sub(range_bits);
    // 当前 ID 实际占用的二进制位数（最高有效位的位置）。
    let used_bits = u64::from(64 - current_id.leading_zeros());
    if used_bits > incremental_bits {
        Err(ColumnError::AutoRandomOverflow)
    } else {
        Ok(())
    }
}

/// 查找引用了指定列的第一个外键约束（删列/改列前需检查外键依赖）。
pub fn get_column_foreign_key_info<'a>(
    column_name: &str,
    foreign_keys: &'a [ForeignKeyInfo],
) -> Option<&'a ForeignKeyInfo> {
    foreign_keys
        .iter()
        .find(|foreign_key| foreign_key.columns.iter().any(|name| name == column_name))
}

/// 从表达式索引隐藏列的内部名称还原出原始索引名。
///
/// 表达式索引会生成形如 `_V$_索引名_序号` 的隐藏虚拟列，
/// 此函数剥离 `_V$_` 前缀与末尾的序号后缀，得到原始索引名。
pub fn expression_index_origin_name(original_name: &str) -> String {
    let name = original_name.strip_prefix("_V$_").unwrap_or(original_name);
    name.rsplit_once('_')
        .map_or_else(|| name.to_owned(), |(origin, _)| origin.to_owned())
}
