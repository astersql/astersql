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

// DDL 索引模块：负责索引（Index）元数据的构建与维护。
//
// 索引是数据库中加速数据检索的辅助结构；DDL（Data Definition Language，
// 数据定义语言）指 CREATE/ALTER/DROP 等修改表结构的语句。本模块涵盖：
// - 全文索引（FULLTEXT）的规范化构建入口 `BuildCanonicalFullTextIndex`；
// - 索引列校验与前缀长度计算（如 BLOB/TEXT 列必须指定前缀长度）；
// - 索引元数据（`IndexInfo`）的创建、重命名、可见性切换与删除；
// - 索引回填（backfill，即为已有数据补建索引条目）策略选择、
//   重试错误判定以及分布式任务键（task key）的生成等辅助逻辑。

use std::collections::{BTreeMap, BTreeSet};

/// Select the on-disk global-index format using the same compatibility rules as Go.
///
/// The version is reset first so callers that turn a global index into a local one
/// cannot retain stale metadata. V1 is used only when the cluster advertises support,
/// the table is non-clustered, and the index key must carry a partition ID: every
/// non-unique global index, plus unique global indexes containing a nullable column.
pub fn set_global_index_version(
    table: &astersql_meta_model::TableInfo,
    index: &mut astersql_meta_model::IndexInfo,
) {
    use astersql_meta_model::{GetGlobalIndexV1Supported, GlobalIndexVersionV1};
    use astersql_parser_mysql::r#type::{HasNotNullFlag, HasPreventNullInsertFlag};

    index.GlobalIndexVersion = 0;
    if !GetGlobalIndexV1Supported() || !index.Global || table.HasClusteredIndex() {
        return;
    }

    let needs_partition_in_key = !index.Unique
        || index.Columns.iter().any(|index_column| {
            table
                .Columns
                .iter()
                .find(|column| column.Name.L == index_column.Name.L)
                .is_some_and(|column| {
                    !HasNotNullFlag(column.GetFlag()) || HasPreventNullInsertFlag(column.GetFlag())
                })
        });
    if needs_partition_in_key {
        index.GlobalIndexVersion = GlobalIndexVersionV1;
    }
}

/// Appends the canonical metadata for `ALTER TABLE ... ADD FULLTEXT`.
/// Validation and ID allocation follow the CREATE TABLE metadata path, while
/// the deployment gate remains inside DDL so non-session callers cannot bypass
/// the Starter-only contract.
///
/// 为 `ALTER TABLE ... ADD FULLTEXT` 语句构建规范化的全文索引元数据。
/// 校验与索引 ID 分配复用 CREATE TABLE 的元数据路径；部署模式检查
/// （仅 Starter 部署模式支持全文索引）放在 DDL 内部，保证非会话调用方
/// 也无法绕过该限制。全文索引（FULLTEXT）用于对文本列做分词检索。
pub fn BuildCanonicalFullTextIndex(
    table: &mut astersql_meta_model::TableInfo,
    constraint: &astersql_parser_ast::Constraint,
) -> Result<astersql_meta_model::IndexInfo, astersql_parser::errors::Error> {
    use astersql_meta_model as model;
    use astersql_parser_ast as ast;

    let error = |message: &str| astersql_parser::errors::New(message);
    // 约束类型必须是 FULLTEXT，否则拒绝。
    if constraint.Tp != ast::ConstraintType::Fulltext {
        return Err(error("constraint is not FULLTEXT"));
    }
    // 全文索引仅在 Starter 部署模式下可用。
    if !astersql_config_deploymode::IsStarter() {
        return Err(error(
            "FULLTEXT index is only supported in starter deployment mode",
        ));
    }
    // 全文索引只允许恰好一个升序、且不指定前缀长度的完整列。
    if constraint.Keys.len() != 1
        || constraint.Keys[0].Length != model::types::UnspecifiedLength
        || constraint.Keys[0].Desc
    {
        return Err(error(
            "FULLTEXT index requires exactly one whole ascending column",
        ));
    }
    // 取出被索引的列名；表达式索引（基于表达式而非列的索引）不支持 FULLTEXT。
    let column_name = constraint.Keys[0]
        .Column
        .as_ref()
        .map(|column| column.Name.clone())
        .ok_or_else(|| error("FULLTEXT expression index is not supported"))?;
    // 在表的列定义中定位该列，并要求其求值类型为字符串。
    let offset = table
        .Columns
        .iter()
        .position(|column| column.Name.L == column_name.L)
        .ok_or_else(|| {
            astersql_parser::errors::New(format!("key column '{}' does not exist", column_name.O))
        })?;
    if table.Columns[offset].FieldType.EvalType() != model::types::ETString {
        return Err(error("FULLTEXT index requires a string column"));
    }
    // 索引名缺省时沿用列名，并检查是否与已有索引重名。
    let index_name = if constraint.Name.is_empty() {
        column_name.clone()
    } else {
        ast::NewCIStr(&constraint.Name)
    };
    if table
        .Indices
        .iter()
        .any(|index| index.Name.L == index_name.L)
    {
        return Err(astersql_parser::errors::New(format!(
            "duplicate key name '{}'",
            index_name.O
        )));
    }
    // 解析 WITH PARSER 指定的分词器类型，缺省使用标准分词器 StandardV1。
    let parser_type = constraint
        .Option
        .as_ref()
        .filter(|option| !option.ParserName.L.is_empty())
        .map(|option| model::GetFullTextParserTypeBySQLName(&option.ParserName.L))
        .unwrap_or_else(|| model::FullTextParserTypeStandardV1.clone());
    if parser_type == model::FullTextParserTypeInvalid {
        return Err(error("invalid FULLTEXT parser"));
    }

    // 分配新的索引 ID 并组装 IndexInfo 元数据，状态直接置为 Public（对外可见）。
    table.MaxIndexID += 1;
    let option = constraint.Option.as_ref();
    let index = model::IndexInfo {
        ID: table.MaxIndexID,
        Name: index_name,
        Table: table.Name.clone(),
        Columns: vec![model::IndexColumn {
            Name: column_name,
            Offset: offset as isize,
            Length: model::types::UnspecifiedLength,
            UseChangingType: false,
        }],
        State: model::StatePublic,
        Tp: match option
            .map(|option| option.Tp)
            .filter(|index_type| *index_type != ast::IndexType::Invalid)
            .unwrap_or(ast::IndexType::Btree)
        {
            ast::IndexType::Invalid => model::ast::IndexType::Invalid,
            ast::IndexType::Btree => model::ast::IndexType::Btree,
            ast::IndexType::Hash => model::ast::IndexType::Hash,
            ast::IndexType::Rtree => model::ast::IndexType::Rtree,
            ast::IndexType::Hypo => model::ast::IndexType::Hypo,
            ast::IndexType::HNSW => model::ast::IndexType::HNSW,
            ast::IndexType::Inverted => model::ast::IndexType::Inverted,
        },
        Invisible: option
            .is_some_and(|option| option.Visibility == ast::IndexVisibility::Invisible),
        Global: option.is_some_and(|option| option.Global),
        Comment: option
            .map(|option| option.Comment.clone())
            .unwrap_or_default(),
        FullTextInfo: Some(model::FullTextIndexInfo {
            ParserType: parser_type,
        }),
        ..Default::default()
    };
    // 给列打上 MultipleKeyFlag（表示该列参与了非唯一索引），并登记索引。
    table.Columns[offset].AddFlag(model::mysql::MultipleKeyFlag);
    table.Indices.push(index.clone());
    Ok(index)
}

/// 索引校验所用的简化列类型枚举，覆盖 MySQL 常见数据类型。
/// 各变体携带计算索引键长度所需的参数（如精度、位数、字符数）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ColumnType {
    TinyInt,
    SmallInt,
    MediumInt,
    Int,
    BigInt,
    Float,
    Double,
    Decimal { precision: usize },
    Date,
    DateTime,
    Timestamp,
    Duration,
    Year,
    Bit(usize),
    Char(usize),
    VarChar(usize),
    Binary(usize),
    VarBinary(usize),
    Blob,
    Text,
    Json,
    Enum,
    Set,
    Vector,
}

/// 索引构建过程中使用的列元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnInfo {
    /// 列 ID。
    pub id: i64,
    /// 列名。
    pub name: String,
    /// 列的数据类型。
    pub column_type: ColumnType,
    /// 该列所用字符集单个字符的最大字节数（如 utf8mb4 为 4）。
    pub charset_max_bytes: usize,
    /// 是否为生成列（generated column，值由表达式计算得出）。
    pub generated: bool,
    /// 生成列是否为 STORED（物化存储）而非 VIRTUAL（读取时计算）。
    pub stored: bool,
    /// 是否为隐藏列（如表达式索引内部生成的辅助列）。
    pub hidden: bool,
    /// 是否允许 NULL。
    pub nullable: bool,
    /// 是否属于主键。
    pub primary_key: bool,
    /// 引用该列的索引数量计数，用于维护列上的索引标记。
    pub index_flags: usize,
    /// 生成列表达式依赖的其他列名集合（小写）。
    pub generated_dependencies: BTreeSet<String>,
}

/// 索引中的一个键列：记录列名、在表列中的偏移以及可选的前缀长度。
/// 前缀索引（prefix index）只对列值的前 N 个字符/字节建索引。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexColumn {
    /// 列名。
    pub name: String,
    /// 该列在表列数组中的下标。
    pub offset: usize,
    /// 前缀长度；`None` 表示索引整列。
    pub length: Option<usize>,
}

/// 列存索引类型。列存（columnar）索引由列式存储引擎承载，
/// 与常规行存 B 树索引不同，不占用行存键空间。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColumnarIndexType {
    /// 非列存索引（普通行存索引）。
    #[default]
    None,
    /// 向量索引，用于向量近似最近邻检索。
    Vector,
    /// 倒排索引，按值到行的映射加速过滤。
    Inverted,
    /// 全文索引，用于文本分词检索。
    FullText,
}

/// 索引的种类（对应 SQL 中的 USING/索引类型语法）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IndexKind {
    /// B 树索引，默认类型，支持范围查询。
    #[default]
    BTree,
    /// 哈希索引，仅支持等值查询。
    Hash,
    /// 全文索引。
    FullText,
    /// 向量索引。
    Vector,
    /// 倒排索引。
    Inverted,
}

/// 索引元数据：描述一个索引的名称、键列、唯一性、可见性等属性。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引 ID，表内唯一。
    pub id: i64,
    /// 索引名。
    pub name: String,
    /// 索引包含的键列（有序）。
    pub columns: Vec<IndexColumn>,
    /// 是否唯一索引（主键隐含唯一）。
    pub unique: bool,
    /// 是否主键索引。
    pub primary: bool,
    /// 是否不可见索引（优化器忽略但仍维护）。
    pub invisible: bool,
    /// 是否全局索引（分区表上跨所有分区的索引）。
    pub global: bool,
    /// 索引种类。
    pub kind: IndexKind,
    /// 索引注释。
    pub comment: String,
    /// 部分索引（partial index）的过滤条件表达式文本。
    pub condition: Option<String>,
    /// 索引元数据版本号。
    pub version: u32,
}

/// 表元数据的简化表示，仅保留索引 DDL 所需字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 表 ID。
    pub id: i64,
    /// 表的列定义。
    pub columns: Vec<ColumnInfo>,
    /// 表上已有的索引。
    pub indices: Vec<IndexInfo>,
    /// 已分配的最大索引 ID，用于单调分配新 ID。
    pub max_index_id: i64,
    /// 是否分区表。
    pub partitioned: bool,
}

/// 索引 DDL 过程中可能出现的各种校验错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexError {
    /// 指定的列不存在。
    ColumnNotFound(String),
    /// 键列数量超过上限。
    TooManyKeyParts { actual: usize, maximum: usize },
    /// 同一索引中列重复出现。
    DuplicateColumn(String),
    /// 主键定义在生成列上（不允许）。
    GeneratedPrimaryKey(String),
    /// 前缀长度非法（如为 0）。
    InvalidPrefix(String),
    /// BLOB/TEXT 列建索引必须指定前缀长度。
    BlobNeedsPrefix(String),
    /// 前缀长度超过整列长度。
    PrefixTooLong(String),
    /// 索引键总长度超出上限。
    KeyTooLong { actual: usize, maximum: usize },
    /// 索引名与已有索引重复。
    DuplicateName(String),
    /// 主键索引不允许不可见。
    PrimaryIndexInvisible,
    /// 该列类型或索引类型不受支持。
    UnsupportedIndexType,
    /// 向量索引列约束不满足。
    InvalidVectorIndex,
    /// 倒排索引列约束不满足。
    InvalidInvertedIndex,
    /// 全文索引列约束不满足。
    InvalidFullTextIndex,
    /// 部分索引条件非法。
    InvalidCondition,
    /// 找不到指定名称的索引。
    IndexNotFound(String),
}

/// 根据列规格（列名与可选前缀长度）构建索引键列列表。
/// 校验键列数量上限、列是否存在、是否重复以及类型是否可索引。
/// 返回键列数组与是否引用隐藏列（表示存在表达式索引）的标记。
pub fn build_index_columns(
    columns: &[ColumnInfo],
    specifications: &[(String, Option<usize>)],
    columnar: ColumnarIndexType,
) -> Result<(Vec<IndexColumn>, bool), IndexError> {
    // MySQL 兼容限制：一个索引最多 16 个键列。
    const MAX_KEY_PARTS: usize = 16;
    if specifications.len() > MAX_KEY_PARTS {
        return Err(IndexError::TooManyKeyParts {
            actual: specifications.len(),
            maximum: MAX_KEY_PARTS,
        });
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(specifications.len());
    let mut has_expression = false;
    for (name, length) in specifications {
        // 列名不区分大小写，重复出现视为错误。
        let lower = name.to_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(IndexError::DuplicateColumn(name.clone()));
        }
        let offset = columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| IndexError::ColumnNotFound(name.clone()))?;
        // 隐藏列意味着这是表达式索引生成的辅助列。
        if columns[offset].hidden {
            has_expression = true;
        }
        check_index_column(&columns[offset], *length, columnar)?;
        result.push(IndexColumn {
            name: columns[offset].name.clone(),
            offset,
            length: *length,
        });
    }
    Ok((result, has_expression))
}

/// 校验主键列不能是生成列（MySQL 不允许在生成列上定义主键）。
pub fn check_primary_key_on_generated_column(
    columns: &[ColumnInfo],
    specifications: &[(String, Option<usize>)],
) -> Result<(), IndexError> {
    for (name, _) in specifications {
        let column = columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| IndexError::ColumnNotFound(name.clone()))?;
        if column.generated && !column.stored {
            return Err(IndexError::GeneratedPrimaryKey(name.clone()));
        }
    }
    Ok(())
}

/// 累加各键列的索引键长度并校验不超过上限（如 3072 字节）。
/// 返回索引键的总字节长度。
pub fn check_index_prefix_length(
    columns: &[ColumnInfo],
    index_columns: &[IndexColumn],
    columnar: ColumnarIndexType,
    maximum: usize,
) -> Result<usize, IndexError> {
    let mut total = 0;
    for index_column in index_columns {
        let column = columns
            .get(index_column.offset)
            .ok_or_else(|| IndexError::ColumnNotFound(index_column.name.clone()))?;
        total += get_index_column_length(column, index_column.length, columnar)?;
    }
    if total > maximum {
        Err(IndexError::KeyTooLong {
            actual: total,
            maximum,
        })
    } else {
        Ok(total)
    }
}

/// 校验单个索引列的合法性：前缀长度不能为 0；
/// BLOB/TEXT 列在行存索引中必须指定前缀长度；JSON 列不能建行存索引。
/// 列存索引不受这些行存键长度限制。
fn check_index_column(
    column: &ColumnInfo,
    length: Option<usize>,
    columnar: ColumnarIndexType,
) -> Result<(), IndexError> {
    if length == Some(0) {
        return Err(IndexError::InvalidPrefix(column.name.clone()));
    }
    if length.is_some()
        && !matches!(
            column.column_type,
            ColumnType::Char(_)
                | ColumnType::VarChar(_)
                | ColumnType::Binary(_)
                | ColumnType::VarBinary(_)
                | ColumnType::Blob
                | ColumnType::Text
        )
    {
        return Err(IndexError::UnsupportedIndexType);
    }
    if matches!(column.column_type, ColumnType::Blob | ColumnType::Text)
        && length.is_none()
        && columnar == ColumnarIndexType::None
    {
        return Err(IndexError::BlobNeedsPrefix(column.name.clone()));
    }
    if matches!(column.column_type, ColumnType::Json) && columnar == ColumnarIndexType::None {
        return Err(IndexError::UnsupportedIndexType);
    }
    Ok(())
}

/// 计算某列在索引键中占用的字节数。
/// 列存索引不占行存键空间，直接返回 0；字符类型按
/// 字符数乘以字符集最大字节数计算；指定前缀时取前缀与整列长度的较小值。
pub fn get_index_column_length(
    column: &ColumnInfo,
    prefix: Option<usize>,
    columnar: ColumnarIndexType,
) -> Result<usize, IndexError> {
    if columnar != ColumnarIndexType::None {
        // Go uses one as the smallest non-zero length because a zero length can
        // break downstream size and concurrency calculations.
        return Ok(1);
    }
    // 各类型索引整列时的字节长度。
    let full = match column.column_type {
        ColumnType::TinyInt => 1,
        ColumnType::SmallInt => 2,
        ColumnType::MediumInt => 3,
        ColumnType::Int | ColumnType::Float => 4,
        ColumnType::BigInt | ColumnType::Double => 8,
        ColumnType::Decimal { precision } => calc_bytes_length_for_decimal(precision),
        ColumnType::Date | ColumnType::Duration => 3,
        ColumnType::DateTime => 8,
        ColumnType::Timestamp => 4,
        ColumnType::Year => 1,
        ColumnType::Bit(bits) => bits.div_ceil(8),
        ColumnType::Char(chars) | ColumnType::VarChar(chars) => {
            chars.saturating_mul(column.charset_max_bytes.max(1))
        }
        ColumnType::Binary(bytes) | ColumnType::VarBinary(bytes) => bytes,
        ColumnType::Blob | ColumnType::Text => prefix
            .ok_or_else(|| IndexError::BlobNeedsPrefix(column.name.clone()))?
            .saturating_mul(column.charset_max_bytes.max(1)),
        ColumnType::Enum => 2,
        ColumnType::Set => 8,
        ColumnType::Json | ColumnType::Vector => return Err(IndexError::UnsupportedIndexType),
    };
    if let Some(prefix) = prefix {
        // 字符类型的前缀以字符为单位，需换算为字节。
        let requested = if matches!(
            column.column_type,
            ColumnType::Char(_) | ColumnType::VarChar(_) | ColumnType::Text
        ) {
            prefix.saturating_mul(column.charset_max_bytes.max(1))
        } else {
            prefix
        };
        // 前缀超过整列长度视为错误（BLOB/TEXT 长度不定，允许例外）。
        if requested > full && !matches!(column.column_type, ColumnType::Blob | ColumnType::Text) {
            return Err(IndexError::PrefixTooLong(column.name.clone()));
        }
        Ok(requested.min(full))
    } else {
        Ok(full)
    }
}

/// 计算 DECIMAL 类型按精度存储所需的字节数：
/// 每 9 个十进制数字压缩为 4 字节（MySQL 的 decimal 存储约定）。
pub const fn calc_bytes_length_for_decimal(precision: usize) -> usize {
    (precision / 9 * 4) + ((precision % 9) + 1) / 2
}

/// 构建索引时的可选属性集合（唯一性、可见性、注释、部分索引条件等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexOptions {
    /// 是否唯一索引。
    pub unique: bool,
    /// 是否主键索引。
    pub primary: bool,
    /// 是否不可见索引。
    pub invisible: bool,
    /// 是否请求创建全局索引。
    pub global: bool,
    /// 索引种类。
    pub kind: IndexKind,
    /// SQL 中声明的索引注释。
    pub comment: String,
    /// 部分索引的过滤条件文本。
    pub condition: Option<String>,
}

/// 在表上构建新索引（默认非 Starter 部署，即不允许 FULLTEXT）。
pub fn build_index_info(
    table: &mut TableInfo,
    name: &str,
    specifications: &[(String, Option<usize>)],
    options: IndexOptions,
) -> Result<IndexInfo, IndexError> {
    build_index_info_for_deploy_mode(table, name, specifications, options, false)
}

/// Builds an index using the deployment capability supplied by the DDL
/// executor. FULLTEXT is a columnar index and is unavailable outside Starter.
///
/// 按 DDL 执行器提供的部署能力构建索引：完成重名检查、主键约束校验、
/// 键列构建、键长度校验、特殊索引（向量/倒排/全文）校验、部分索引条件
/// 校验后分配索引 ID 并登记到表元数据。FULLTEXT 属于列存索引，
/// 仅在 Starter 部署模式（`starter_deployment` 为 true）下可用。
pub fn build_index_info_for_deploy_mode(
    table: &mut TableInfo,
    name: &str,
    specifications: &[(String, Option<usize>)],
    options: IndexOptions,
    starter_deployment: bool,
) -> Result<IndexInfo, IndexError> {
    if table
        .indices
        .iter()
        .any(|index| index.name.eq_ignore_ascii_case(name))
    {
        return Err(IndexError::DuplicateName(name.to_owned()));
    }
    if options.primary {
        check_primary_key_on_generated_column(&table.columns, specifications)?;
    }
    // 主键索引不允许设置为不可见。
    if options.primary && options.invisible {
        return Err(IndexError::PrimaryIndexInvisible);
    }
    // 由索引种类推导对应的列存索引类型；FULLTEXT 受部署模式门控。
    let columnar = match options.kind {
        IndexKind::Vector => ColumnarIndexType::Vector,
        IndexKind::Inverted => ColumnarIndexType::Inverted,
        IndexKind::FullText => {
            if !starter_deployment {
                return Err(IndexError::UnsupportedIndexType);
            }
            ColumnarIndexType::FullText
        }
        _ => ColumnarIndexType::None,
    };
    let (columns, _) = build_index_columns(&table.columns, specifications, columnar)?;
    // 3072 字节是 InnoDB 兼容的索引键长度上限。
    check_index_prefix_length(&table.columns, &columns, columnar, 3072)?;
    validate_special_index(&table.columns, &columns, options.kind)?;
    // 部分索引条件若为空白字符串则视为非法。
    if options
        .condition
        .as_ref()
        .is_some_and(|condition| condition.trim().is_empty())
    {
        return Err(IndexError::InvalidCondition);
    }
    let id = allocate_index_id(table);
    let info = IndexInfo {
        id,
        name: name.to_owned(),
        columns,
        unique: options.unique || options.primary,
        primary: options.primary,
        invisible: options.invisible,
        // 全局索引仅对分区表生效。
        global: options.global && table.partitioned,
        kind: options.kind,
        comment: options.comment,
        condition: options.condition,
        // 全局索引使用版本 2，普通索引为版本 1。
        version: u32::from(options.global && table.partitioned) + 1,
    };
    add_index_column_flag(table, &info);
    table.indices.push(info.clone());
    Ok(info)
}

/// 校验特殊索引的列约束：
/// - 向量索引必须恰好一个 Vector 类型列；
/// - 倒排索引至少一个列；
/// - 全文索引必须恰好一个字符串类型列（CHAR/VARCHAR/TEXT）。
fn validate_special_index(
    columns: &[ColumnInfo],
    index_columns: &[IndexColumn],
    kind: IndexKind,
) -> Result<(), IndexError> {
    // 特殊索引不只校验列数，还要校验列的物理类型是否匹配具体引擎能力。
    match kind {
        IndexKind::Vector
            if index_columns.len() != 1
                || !matches!(
                    columns[index_columns[0].offset].column_type,
                    ColumnType::Vector
                ) =>
        {
            Err(IndexError::InvalidVectorIndex)
        }
        IndexKind::Inverted
            if index_columns.is_empty()
                || index_columns.iter().any(|index| {
                    !matches!(
                        columns[index.offset].column_type,
                        ColumnType::TinyInt
                            | ColumnType::SmallInt
                            | ColumnType::MediumInt
                            | ColumnType::Int
                            | ColumnType::BigInt
                            | ColumnType::Year
                            | ColumnType::Date
                            | ColumnType::DateTime
                            | ColumnType::Timestamp
                            | ColumnType::Duration
                    )
                }) =>
        {
            Err(IndexError::InvalidInvertedIndex)
        }
        IndexKind::FullText
            if index_columns.len() != 1
                || index_columns.iter().any(|index| {
                    !matches!(
                        columns[index.offset].column_type,
                        ColumnType::Char(_) | ColumnType::VarChar(_) | ColumnType::Text
                    )
                }) =>
        {
            Err(IndexError::InvalidFullTextIndex)
        }
        _ => Ok(()),
    }
}

/// 为索引引用的各列增加索引引用计数。
pub fn add_index_column_flag(table: &mut TableInfo, index: &IndexInfo) {
    for column in &index.columns {
        table.columns[column.offset].index_flags += 1;
    }
}
/// 为索引引用的各列减少索引引用计数（饱和减，避免下溢）。
pub fn drop_index_column_flag(table: &mut TableInfo, index: &IndexInfo) {
    for column in &index.columns {
        table.columns[column.offset].index_flags =
            table.columns[column.offset].index_flags.saturating_sub(1);
    }
}

/// 校验索引重命名的合法性：源索引必须存在且目标名不能与其他索引冲突。
/// 返回 `true` 表示新旧同名，无需实际操作。
pub fn validate_rename_index(from: &str, to: &str, table: &TableInfo) -> Result<bool, IndexError> {
    if !table
        .indices
        .iter()
        .any(|index| index.name.eq_ignore_ascii_case(from))
    {
        return Err(IndexError::IndexNotFound(from.to_owned()));
    }
    if from == to {
        return Ok(true);
    }
    if !from.eq_ignore_ascii_case(to)
        && table
            .indices
            .iter()
            .any(|index| index.name.eq_ignore_ascii_case(to))
    {
        return Err(IndexError::DuplicateName(to.to_owned()));
    }
    Ok(false)
}

/// 执行索引重命名（`ALTER TABLE ... RENAME INDEX`），先校验后改名。
pub fn rename_index(table: &mut TableInfo, from: &str, to: &str) -> Result<(), IndexError> {
    if validate_rename_index(from, to, table)? {
        return Ok(());
    }
    table
        .indices
        .iter_mut()
        .find(|index| index.name.eq_ignore_ascii_case(from))
        .expect("validated index")
        .name = to.to_owned();
    Ok(())
}

/// 设置索引可见性（`ALTER INDEX ... VISIBLE/INVISIBLE`）；
/// 主键索引不允许被设为不可见。
pub fn set_index_visibility(
    table: &mut TableInfo,
    name: &str,
    invisible: bool,
) -> Result<(), IndexError> {
    let index = table
        .indices
        .iter_mut()
        .find(|index| index.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| IndexError::IndexNotFound(name.to_owned()))?;
    if index.primary && invisible {
        return Err(IndexError::PrimaryIndexInvisible);
    }
    index.invisible = invisible;
    Ok(())
}

/// 从表元数据中删除索引（DROP INDEX）：
/// 移除索引记录、递减列引用计数并清理仅被该索引使用的隐藏列。
pub fn remove_index_info(table: &mut TableInfo, name: &str) -> Result<IndexInfo, IndexError> {
    let position = table
        .indices
        .iter()
        .position(|index| index.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| IndexError::IndexNotFound(name.to_owned()))?;
    let removed = table.indices.remove(position);
    drop_index_column_flag(table, &removed);
    remove_dependent_hidden_columns(table, &removed);
    Ok(removed)
}

/// 清理表达式索引专用的隐藏列：被删索引引用的隐藏列若不再被
/// 其他索引使用，则一并从表中移除。
pub fn remove_dependent_hidden_columns(table: &mut TableInfo, index: &IndexInfo) {
    // 收集被删索引引用的隐藏列名（小写）。
    let hidden_names: BTreeSet<String> = index
        .columns
        .iter()
        .filter_map(|column| table.columns.get(column.offset))
        .filter(|column| column.hidden)
        .map(|column| column.name.to_lowercase())
        .collect();
    table
        .columns
        .retain(|column| !hidden_names.contains(&column.name.to_lowercase()));
    let offsets: BTreeMap<String, usize> = table
        .columns
        .iter()
        .enumerate()
        .map(|(offset, column)| (column.name.to_lowercase(), offset))
        .collect();
    for other in &mut table.indices {
        for column in &mut other.columns {
            if let Some(offset) = offsets.get(&column.name.to_lowercase()) {
                column.offset = *offset;
            }
        }
    }
}

/// 建索引后自动 ANALYZE（收集统计信息以供优化器使用）任务的状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalyzeStatus {
    /// 仍在运行。
    Running,
    /// 已成功完成。
    Finished,
    /// 执行失败。
    Failed,
    /// 未找到对应任务记录。
    NotFound,
}

/// 根据 ANALYZE 状态与是否超时给出决策，返回四元组：
/// (是否完成, 是否超时, 是否失败, 是否应在本地启动 ANALYZE)。
pub fn analyze_status_decision(status: AnalyzeStatus, timed_out: bool) -> (bool, bool, bool, bool) {
    match status {
        AnalyzeStatus::Finished => (true, false, false, false),
        AnalyzeStatus::Failed => (true, false, true, false),
        AnalyzeStatus::Running if timed_out => (false, true, false, false),
        AnalyzeStatus::Running => (false, false, false, false),
        AnalyzeStatus::NotFound => (false, false, false, true),
    }
}

/// 索引回填（reorg/backfill，为存量数据补建索引条目）的执行方式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReorgType {
    /// 通过普通事务逐批写入索引条目。
    Transactional,
    /// 本地 ingest：先在本地构建 SST 文件再批量导入存储层，速度更快。
    LocalIngest,
    /// 分布式执行框架（DXF）并行回填。
    Distributed,
}

/// 依据配置选择回填方式：优先分布式（除非需要合并临时索引），
/// 其次本地 ingest，最后回退到事务写入。
pub fn pick_backfill_type(
    distributed: bool,
    ingest: bool,
    temporary_index_merge: bool,
) -> ReorgType {
    if distributed && !temporary_index_merge {
        ReorgType::Distributed
    } else if ingest {
        ReorgType::LocalIngest
    } else {
        ReorgType::Transactional
    }
}

/// DDL 作业执行中的错误分类，用于判定能否重试。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobErrorKind {
    /// 写冲突：乐观事务提交时发现数据已被并发修改。
    WriteConflict,
    /// Region（存储层的数据分片单元）暂不可用，如正在调度迁移。
    RegionUnavailable,
    /// 操作超时。
    Timeout,
    /// 唯一键冲突（数据违反唯一约束，不可重试）。
    DuplicateKey,
    /// 作业被用户取消。
    Cancelled,
    /// 未知错误。
    Unknown,
}
/// 判断错误是否可重试：写冲突、Region 不可用、超时属于瞬时错误可重试；
/// 未知错误由 `retry_unknown` 决定。
pub fn is_retryable_error(error: JobErrorKind, retry_unknown: bool) -> bool {
    matches!(
        error,
        JobErrorKind::WriteConflict | JobErrorKind::RegionUnavailable | JobErrorKind::Timeout
    ) || (error == JobErrorKind::Unknown && retry_unknown)
}
/// 判断作业级错误是否可重试：累计错误次数达到 5 次即放弃。
pub fn is_retryable_job_error(error: JobErrorKind, error_count: i64) -> bool {
    error_count + 1 < 5 && is_retryable_error(error, true)
}

/// 分布式回填任务键（task key）的构建器。
/// 任务键唯一标识一次分布式任务，可附带"合并临时索引"阶段标记
/// 与多 schema 变更（multi-schema change）的子任务序号。
#[derive(Clone, Debug, Default)]
pub struct TaskKeyBuilder {
    merge_temporary_index: bool,
    multi_schema_sequence: Option<i64>,
}
impl TaskKeyBuilder {
    /// 创建默认构建器。
    pub fn new() -> Self {
        Self::default()
    }
    /// 标记该任务处于"合并临时索引"阶段（把回填期间写入临时
    /// 索引的增量数据合并进正式索引）。
    pub fn set_merge_temporary_index(mut self, value: bool) -> Self {
        self.merge_temporary_index = value;
        self
    }
    /// 设置多 schema 变更中的子任务序号。
    pub fn set_multi_schema(mut self, sequence: Option<i64>) -> Self {
        self.multi_schema_sequence = sequence.filter(|sequence| *sequence >= 0);
        self
    }
    /// 生成任务键：以作业 ID 开头，按需拼接阶段标记与序号。
    pub fn build(&self, job_id: i64) -> String {
        let mut key = format!("ddl/backfill/{job_id}");
        if let Some(sequence) = self.multi_schema_sequence {
            key.push('/');
            key.push_str(&sequence.to_string());
        }
        if self.merge_temporary_index {
            key.push_str("/merge");
        }
        key
    }
}
/// 便捷函数：由作业 ID 和临时索引合并标记直接生成任务键。
pub fn task_key(job_id: i64, merge_temporary_index: bool) -> String {
    TaskKeyBuilder::new()
        .set_merge_temporary_index(merge_temporary_index)
        .build(job_id)
}

/// 调整回填并发度：取请求 worker 数与节点可用槽位数的较小值。
pub fn adjust_concurrency(worker_count: usize, available_slots: usize) -> usize {
    worker_count.min(available_slots)
}

/// 估算表的平均行大小（字节）：有行数统计时用表大小除以行数，
/// 否则退回到基于 Region 的估计值。
pub fn estimate_table_row_size(table_size: i64, row_count: i64, region_estimate: i64) -> usize {
    if row_count > 0 {
        usize::try_from((table_size.max(0) / row_count).max(1)).unwrap_or(usize::MAX)
    } else {
        usize::try_from(region_estimate.max(0)).unwrap_or(usize::MAX)
    }
}

/// 分区定义的简化表示：分区表按规则把数据拆分到多个物理分区。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    /// 分区 ID。
    pub id: i64,
    /// 该分区正在被删除（DDL 进行中）。
    pub dropping: bool,
    /// 该分区正在被添加（DDL 进行中）。
    pub adding: bool,
}
/// 返回当前分区之后的下一个分区 ID，用于回填时按分区顺序推进。
pub fn find_next_partition_id(current: i64, definitions: &[PartitionDefinition]) -> Option<i64> {
    definitions
        .iter()
        .skip_while(|definition| definition.id != current)
        .skip(1)
        .map(|definition| definition.id)
        .next()
}
/// 返回 Definitions 中当前分区之后第一个不在 DroppingDefinitions 中的分区 ID。
pub fn find_next_non_touched_partition_id(
    current: i64,
    definitions: &[PartitionDefinition],
) -> Option<i64> {
    definitions
        .iter()
        .skip_while(|definition| definition.id != current)
        .skip(1)
        .find(|definition| !definition.dropping)
        .map(|definition| definition.id)
}

/// Advance the canonical reorg cursor through Definitions minus DroppingDefinitions.
/// An unknown current ID terminates traversal, matching Go's warning-only fallback.
pub fn next_non_touched_partition_id(
    current: i64,
    partition: &astersql_meta_model::PartitionInfo,
) -> i64 {
    if !partition
        .Definitions
        .iter()
        .any(|definition| definition.ID == current)
    {
        astersql_util_logutil::log::BgLogger().warn(format!(
            "current partition not found in the table definitions: partitionID={current}"
        ));
        return 0;
    }
    let definitions = partition
        .Definitions
        .iter()
        .map(|definition| PartitionDefinition {
            id: definition.ID,
            dropping: partition
                .DroppingDefinitions
                .iter()
                .any(|dropped| dropped.ID == definition.ID),
            adding: false,
        })
        .collect::<Vec<_>>();
    find_next_non_touched_partition_id(current, &definitions).unwrap_or(0)
}

/// Go getNextPartitionInfo's recreated-index branch. Absence from AddingDefinitions
/// selects the non-touched phase; it is not an error in that phase.
pub fn next_recreated_index_partition_id(
    current: i64,
    partition: &astersql_meta_model::PartitionInfo,
) -> i64 {
    if let Some(position) = partition
        .AddingDefinitions
        .iter()
        .position(|p| p.ID == current)
    {
        return partition
            .AddingDefinitions
            .get(position + 1)
            .map_or(0, |p| p.ID);
    }
    next_non_touched_partition_id(current, partition)
}

/// 分配新的索引 ID：单调递增，保证表内唯一。
pub fn allocate_index_id(table: &mut TableInfo) -> i64 {
    table.max_index_id += 1;
    table.max_index_id
}
/// 比较两组索引键列是否等价；与 Go 修复表逻辑一致，仅比较规范化列名。
pub fn index_column_slice_equal(left: &[IndexColumn], right: &[IndexColumn]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.name.eq_ignore_ascii_case(&right.name))
}

/// 查找引用了指定列的所有索引，返回 (索引 ID, 列在索引中的位置) 列表。
/// 修改列类型（如 MODIFY COLUMN）时需据此同步更新相关索引。
pub fn find_related_indexes_to_change(table: &TableInfo, column_name: &str) -> Vec<(i64, usize)> {
    table
        .indices
        .iter()
        .flat_map(|index| {
            index
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column.name.eq_ignore_ascii_case(column_name))
                .map(move |(offset, _)| (index.id, offset))
        })
        .collect()
}

/// 重命名表达式索引的隐藏列：同步更新所有索引键列中的旧名，
/// 以及生成列表达式对旧名的依赖记录。
pub fn rename_expression_index_columns(table: &mut TableInfo, from: &str, to: &str) {
    for index in &mut table.indices {
        for column in &mut index.columns {
            // 表达式索引的隐藏列名也出现在索引键列元数据里，需要同步替换。
            if column.name.eq_ignore_ascii_case(from) {
                column.name = to.to_owned();
            }
        }
    }
    // 生成列的依赖集合中也要替换旧列名。
    for column in &mut table.columns {
        if column.generated_dependencies.remove(&from.to_lowercase()) {
            column.generated_dependencies.insert(to.to_lowercase());
        }
    }
}

/// 校验并构建部分索引的条件表达式文本：
/// 条件不能为空白；引用的列必须存在且不能是虚拟（非 STORED）生成列。
pub fn check_and_build_index_condition_string(
    table: &TableInfo,
    referenced_columns: &[String],
    restored: &str,
) -> Result<String, IndexError> {
    if restored.trim().is_empty() {
        return Err(IndexError::InvalidCondition);
    }
    for name in referenced_columns {
        let column = table
            .columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| IndexError::ColumnNotFound(name.clone()))?;
        if column.generated {
            return Err(IndexError::InvalidCondition);
        }
    }
    Ok(restored.to_owned())
}

/// 构建部分索引条件的行级检查闭包：
/// 给定一行（列名到可空整数值的映射），返回该行是否满足索引条件、
/// 即是否需要为该行写入索引条目。无条件时恒为满足。
pub fn build_index_condition_checker<'a>(
    condition: Option<&'a str>,
) -> impl Fn(&BTreeMap<String, Option<i64>>) -> Result<bool, IndexError> + 'a {
    move |row| {
        // 没有条件时，表示所有行都应写入该索引。
        let Some(condition) = condition else {
            return Ok(true);
        };
        // 简化语义：条件即列名，取该列值并判断其大于 0。
        let value = row.get(condition).ok_or(IndexError::InvalidCondition)?;
        Ok(value.is_some_and(|value| value > 0))
    }
}

/// Owner-local services used by Go's shared add-index/modify-column initializer.
/// The URI is cached by job ID by the owning execution context; disk probing is
/// deliberately separate so cloud jobs never touch the local ingest directory.
pub trait ReorgIndexEnvironment {
    fn load_cloud_storage_uri(&mut self, job_id: i64) -> Result<String, String>;
    fn after_load_cloud_storage_uri(&mut self, _job: &mut astersql_meta_model::group_3::Job) {}
    fn ingest_initialized(&self) -> bool;
    fn pre_check_ingest_disk(&mut self) -> Result<(), String>;
}

/// Select and persist the Go backfill type without changing a started job.
pub fn pick_job_backfill_type(
    environment: &mut dyn ReorgIndexEnvironment,
    job: &mut astersql_meta_model::group_3::Job,
) -> Result<astersql_meta_model::group_3::ReorgType, String> {
    use astersql_meta_model::group_3::ReorgType::*;
    let meta = job
        .reorg_meta
        .as_mut()
        .ok_or("DDL reorg metadata missing")?;
    if meta.ReorgTp != ReorgTypeNone {
        return Ok(meta.ReorgTp);
    }
    let selected = if !meta.IsFastReorg {
        ReorgTypeTxn
    } else if environment.ingest_initialized() {
        if !meta.UseCloudStorage {
            environment.pre_check_ingest_disk()?;
        }
        ReorgTypeIngest
    } else {
        ReorgTypeTxnMerge
    };
    meta.ReorgTp = selected;
    Ok(selected)
}

/// Go initForReorgIndexes, shared by index creation and changing-index reorg.
pub fn init_for_reorg_indexes(
    environment: &mut dyn ReorgIndexEnvironment,
    job: &mut astersql_meta_model::group_3::Job,
    indexes: &mut [astersql_meta_model::IndexInfo],
) -> Result<(), String> {
    if indexes.is_empty() {
        return Ok(());
    }
    let uri = environment.load_cloud_storage_uri(job.id)?;
    let meta = job
        .reorg_meta
        .as_mut()
        .ok_or("DDL reorg metadata missing")?;
    meta.UseCloudStorage = !uri.is_empty() && meta.IsDistReorg;
    environment.after_load_cloud_storage_uri(job);
    let selected = pick_job_backfill_type(environment, job)?;
    if matches!(
        selected,
        astersql_meta_model::group_3::ReorgType::ReorgTypeTxn
            | astersql_meta_model::group_3::ReorgType::ReorgTypeTxnMerge
    ) && indexes
        .iter()
        .any(|index| !index.ConditionExprString.is_empty())
    {
        return Err(astersql_util_dbterror::ErrUnsupportedAddPartialIndex
            .GenWithStackByArgs(&["add partial index without fast reorg is not supported".into()])
            .to_string());
    }
    if selected.NeedMergeProcess() {
        astersql_metrics::telemetry::InitTelemetryMetrics()
            .map_err(|error| error.to_string())?
            .add_index_ingest
            .inc();
        for index in indexes {
            index.BackfillState = astersql_meta_model::BackfillStateRunning;
        }
    }
    Ok(())
}

/// Restore the owner-local cloud-storage URI for a durable add-index job.
///
/// `UseCloudStorage` is persisted in the job while the URI itself lives in the
/// previous owner's reorg context. A new owner must therefore reload and cache
/// the configured URI before submitting a replacement distributed task. Merge
/// tasks and local-sort jobs intentionally bypass cloud storage.
pub fn resolve_cloud_storage_uri_after_owner_failover(
    job_id: i64,
    use_cloud_storage: bool,
    merge_temp_index: bool,
    cached_uri: &mut String,
    load_configured_uri: impl FnOnce() -> String,
) -> Result<String, String> {
    if merge_temp_index || !use_cloud_storage || !cached_uri.is_empty() {
        return Ok(cached_uri.clone());
    }

    let configured_uri = load_configured_uri();
    if configured_uri.is_empty() {
        return Err(format!(
            "cloud storage URI is empty for add-index job {job_id} with cloud storage enabled"
        ));
    }
    cached_uri.clone_from(&configured_uri);
    Ok(configured_uri)
}

pub use astersql_ddl_ingest::env::{
    init_global_lightning_env, initialized_disk_root, replace_global_lightning_env_for_test,
};
pub use astersql_dxf_framework_handle::resolve_cloud_storage_uri;
