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

// MODIFY / CHANGE COLUMN 在线 DDL 核心逻辑。
//
// 负责判定列类型变更是否需要重组（reorg，重写行数据或索引）、构造数据
// 合法性检查 SQL、校验分区键/AUTO_RANDOM 约束，并推进在线 Schema 状态机
// （None → DeleteOnly → WriteOnly → WriteReorganization → Public）。

use std::collections::BTreeSet;

use crate::column::{
    ColumnError, ColumnInfo, ColumnKind, ColumnPosition, DefaultValue, IndexInfo, SchemaState,
    TableInfo, locate_offset_to_move, update_index_column,
};
use crate::generated_column::{GeneratedColumnError, has_dependent_generated_column};

/// 列修改采用的物理变更路径。
///
/// - `None`：尚未判定；
/// - `Precheck`：先做数据预检查（如 VARCHAR→CHAR 尾部空格）；
/// - `NoReorg`：仅改元数据，无需扫表；
/// - `NoReorgWithCheck`：改元数据但需校验存量数据（如 NULL→NOT NULL）；
/// - `IndexReorg`：仅重建相关索引；
/// - `Reorg`：需要行数据重组（重写列值）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModifyColumnType {
    #[default]
    /// 尚未确定修改类型。
    None,
    /// 需要数据预检查后再决定后续路径。
    Precheck,
    /// 无需重组，直接更新元数据。
    NoReorg,
    /// 无需重组，但必须校验存量数据合法。
    NoReorgWithCheck,
    /// 仅索引需要重组。
    IndexReorg,
    /// 行数据需要重组。
    Reorg,
}

/// 表分区类型（决定分区键列可做哪些兼容变更）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionType {
    /// KEY 分区。
    Key,
    /// RANGE 分区。
    Range,
    /// LIST 分区。
    List,
    /// HASH 分区。
    Hash,
}

/// 分区表达式对目标列的使用方式分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PartitionExpressionUsage {
    /// 直接引用列，无函数包裹。
    NoFunction,
    /// 使用 `TO_DAYS(col)`。
    ToDays,
    /// 使用 `EXTRACT(... FROM col)`。
    Extract,
    /// 其他尚不支持放宽变更的函数用法。
    Unsupported,
}

/// 分区定义摘要：类型、分区列与表达式文本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionInfo {
    /// 分区类型。
    pub partition_type: PartitionType,
    /// 分区列名列表（列分区时非空）。
    pub columns: Vec<String>,
    /// 分区表达式原文。
    pub expression: String,
}

/// 一次 MODIFY COLUMN Job 携带的参数与中间状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModifyColumnArgs {
    /// 修改前的列名。
    pub old_column_name: String,
    /// 修改前的列 ID（解析后回填）。
    pub old_column_id: i64,
    /// 目标列定义。
    pub column: ColumnInfo,
    /// 列在表中的新位置。
    pub position: ColumnPosition,
    /// 判定出的修改路径类型。
    pub modify_type: ModifyColumnType,
    /// 在线变更时临时“changing”列的 ID。
    pub changing_column_id: Option<i64>,
    /// 临时 changing 索引的 ID 列表。
    pub changing_index_ids: Vec<i64>,
    /// 可丢弃的冗余索引 ID。
    pub redundant_index_ids: Vec<i64>,
    /// 旧 ENUM/SET 元素列表。
    pub old_elements: Vec<String>,
    /// 新 ENUM/SET 元素列表。
    pub new_elements: Vec<String>,
    /// 新的 AUTO_RANDOM 分片位数。
    pub new_shard_bits: u64,
    /// 新的 AUTO_RANDOM 范围位数。
    pub new_range_bits: u64,
}

/// 判定修改路径与校验时依赖的会话/表级上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModifyColumnContext {
    /// 是否处于严格 SQL Mode（影响可否走无损优化）。
    pub strict_sql_mode: bool,
    /// 若表有分区，附带分区信息以做额外限制。
    pub partition: Option<PartitionInfo>,
    /// 关闭“可能丢数据但仍可优化”的路径，强制 Reorg。
    pub disable_lossy_optimization: bool,
    /// AUTO_RANDOM range bits 的默认值。
    pub auto_random_range_bits_default: u64,
    /// AUTO_RANDOM shard bits 允许的最大值。
    pub auto_random_shard_bits_max: u64,
}

/// MODIFY COLUMN 校验与状态推进过程中的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModifyColumnError {
    /// 找不到待修改列。
    ColumnNotFound(String),
    /// 新列名与已有列冲突。
    ColumnExists(String),
    /// 不支持的类型变更（如生成列类型改动）。
    UnsupportedTypeChange,
    /// 不支持的字符集/排序规则变更。
    UnsupportedCharsetChange,
    /// 主键相关限制触发。
    PrimaryKey,
    /// 列式索引相关限制触发。
    ColumnarIndex,
    /// 存在依赖该列的生成列。
    DependentGeneratedColumn(String),
    /// 存在依赖该列的函数索引（隐藏生成列）。
    DependentFunctionalIndex(String),
    /// 分区键列不允许改名。
    PartitionColumnRename,
    /// 分区键列不允许的类型/属性变更。
    PartitionTypeChange,
    /// NULL→NOT NULL 时存量数据含 NULL。
    InvalidNull,
    /// 存量数据无法装入新类型（截断等）。
    DataTruncated,
    /// 索引前缀长度非法。
    IndexPrefixTooLong,
    /// 列位置参数非法。
    InvalidPosition,
    /// 在线 DDL 状态机遇到非法状态。
    InvalidState(SchemaState),
    /// AUTO_RANDOM 参数非法。
    InvalidAutoRandom(String),
    /// 生成列子系统错误。
    Generated(GeneratedColumnError),
}

impl From<GeneratedColumnError> for ModifyColumnError {
    fn from(value: GeneratedColumnError) -> Self {
        Self::Generated(value)
    }
}

/// 列是否仍带有在线修改中间标志（依赖偏移或禁止插入 NULL）。
pub fn has_modify_flag(column: &ColumnInfo) -> bool {
    column.change_dependency_offset.is_some() || column.prevent_null_insert
}

/// 是否为可空列改为 NOT NULL。
pub fn is_null_to_not_null_change(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    !old.not_null && new.not_null
}

/// 新旧列是否均为整型（可走整型专用优化判定）。
pub fn is_integer_change(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    old.field_type.kind == ColumnKind::Integer && new.field_type.kind == ColumnKind::Integer
}

/// 新旧列是否均为字符类型（CHAR/VARCHAR）。
pub fn is_character_change(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    matches!(
        old.field_type.kind,
        ColumnKind::String | ColumnKind::Varchar
    ) && matches!(
        new.field_type.kind,
        ColumnKind::String | ColumnKind::Varchar
    )
}

/// ENUM/SET 元素是否发生缩减或同位改写（相对旧列表）。
pub fn elements_changed(old_elements: &[String], new_elements: &[String]) -> bool {
    old_elements.len() > new_elements.len()
        || old_elements
            .iter()
            .zip(new_elements)
            .any(|(old, new)| old != new)
}

/// 严格模式下判断类型变更是否“纯扩大”、可免于数据重组。
///
/// 例如同符号整型变宽、同字符集/排序规则下字符串变长、ENUM/SET 未删改元素等。
pub fn no_reorg_data_strict(
    old: &ColumnInfo,
    new: &ColumnInfo,
    old_elements: &[String],
    new_elements: &[String],
) -> bool {
    if old.field_type.kind == new.field_type.kind {
        return match old.field_type.kind {
            ColumnKind::Integer => {
                old.field_type.unsigned == new.field_type.unsigned
                    && new.field_type.flen >= old.field_type.flen
            }
            ColumnKind::Varchar | ColumnKind::String => {
                old.field_type.charset == new.field_type.charset
                    && old.field_type.collation == new.field_type.collation
                    && new.field_type.flen >= old.field_type.flen
            }
            ColumnKind::Enum | ColumnKind::Set => !elements_changed(old_elements, new_elements),
            _ => {
                new.field_type.flen >= old.field_type.flen
                    && new.field_type.decimal >= old.field_type.decimal
            }
        };
    }
    matches!(
        (old.field_type.kind, new.field_type.kind),
        (ColumnKind::String, ColumnKind::Varchar) | (ColumnKind::Varchar, ColumnKind::String)
    ) && old.field_type.charset == new.field_type.charset
        && old.field_type.collation == new.field_type.collation
        && new.field_type.flen >= old.field_type.flen
}

/// 是否需要重写行数据：非整型变更默认需要；字符类型仅在涉及 binary 字符集时需要。
pub fn need_row_reorganization(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    if is_integer_change(old, new) {
        return false;
    }
    if !is_character_change(old, new) {
        return true;
    }
    old.field_type.charset == "binary" || new.field_type.charset == "binary"
}

/// 是否需要重建索引：整型看有无符号变化；字符类型看排序规则或 CHAR/VARCHAR 互换。
pub fn need_index_reorganization(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    if is_integer_change(old, new) {
        return old.field_type.unsigned != new.field_type.unsigned;
    }
    debug_assert!(is_character_change(old, new));
    old.field_type.collation != new.field_type.collation
        || old.field_type.kind != new.field_type.kind
}

/// 综合类型兼容性、分区/副本/SQL Mode 与索引情况，选定 `ModifyColumnType`。
pub fn get_modify_column_type(
    table: &TableInfo,
    args: &ModifyColumnArgs,
    old: &ColumnInfo,
    context: &ModifyColumnContext,
) -> ModifyColumnType {
    let new = &args.column;
    // 纯扩大类型：最多做 NULL→NOT NULL 的数据检查。
    if no_reorg_data_strict(old, new, &args.old_elements, &args.new_elements) {
        return if is_null_to_not_null_change(old, new) {
            ModifyColumnType::NoReorgWithCheck
        } else {
            ModifyColumnType::NoReorg
        };
    }
    // 分区表、TiFlash 副本、关闭无损优化或非严格模式：直接走完整 Reorg。
    if context.partition.is_some()
        || table.tiflash_replica
        || context.disable_lossy_optimization
        || !context.strict_sql_mode
    {
        return ModifyColumnType::Reorg;
    }
    if is_integer_change(old, new) && old.field_type.unsigned != new.field_type.unsigned {
        return ModifyColumnType::Reorg;
    }
    if is_character_change(old, new) && old.field_type.collation != new.field_type.collation {
        return ModifyColumnType::Reorg;
    }
    if need_row_reorganization(old, new) {
        return ModifyColumnType::Reorg;
    }
    let indexed = table
        .indices
        .iter()
        .any(|index| index.columns.iter().any(|column| column.name == old.name));
    // 有索引且索引键表示变化 → IndexReorg；否则只做数据检查。
    if !indexed || !need_index_reorganization(old, new) {
        ModifyColumnType::NoReorgWithCheck
    } else {
        ModifyColumnType::IndexReorg
    }
}

/// 改名时检查新列名是否与其它列冲突。
pub fn check_column_already_exists(
    table: &TableInfo,
    old_name: &str,
    new_name: &str,
) -> Result<(), ModifyColumnError> {
    if !old_name.eq_ignore_ascii_case(new_name)
        && table
            .columns
            .iter()
            .any(|column| column.name.eq_ignore_ascii_case(new_name))
    {
        return Err(ModifyColumnError::ColumnExists(new_name.into()));
    }
    Ok(())
}

/// 校验类型变更是否允许：拒绝生成列变更；GBK 改字符集、索引列改排序规则等受限。
pub fn check_modify_types(
    old: &ColumnInfo,
    new: &ColumnInfo,
    indexed: bool,
) -> Result<(), ModifyColumnError> {
    if old.generated || new.generated {
        return Err(ModifyColumnError::UnsupportedTypeChange);
    }
    // GBK 与其它字符集之间不可互换。
    if old.field_type.charset == "gbk" || new.field_type.charset == "gbk" {
        if old.field_type.charset != new.field_type.charset {
            return Err(ModifyColumnError::UnsupportedCharsetChange);
        }
    }
    // 已建索引的字符列不允许改 collation（会影响键序）。
    if indexed
        && is_character_change(old, new)
        && old.field_type.collation != new.field_type.collation
    {
        return Err(ModifyColumnError::UnsupportedCharsetChange);
    }
    Ok(())
}

/// 构造“整型值是否超出目标位宽/符号范围”的 WHERE 子句片段。
pub fn build_check_range_for_integer(column: &ColumnInfo, changing: &ColumnInfo) -> String {
    let name = format!("`{}`", column.name);
    let bits = changing.field_type.flen.clamp(1, 64) as u32;
    if changing.field_type.unsigned {
        let upper = if bits == 64 {
            u64::MAX
        } else {
            (1_u64 << bits) - 1
        };
        format!("({name} < 0 OR {name} > {upper})")
    } else {
        let upper = if bits == 64 {
            i64::MAX
        } else {
            (1_i64 << (bits - 1)) - 1
        };
        let lower = if bits == 64 {
            i64::MIN
        } else {
            -(1_i64 << (bits - 1))
        };
        format!("({name} < {lower} OR {name} > {upper})")
    }
}

/// 生成探测“是否存在不兼容存量行”的 SELECT ... LIMIT 1 SQL；无需检查时返回空串。
pub fn build_check_sql_from_modify_column(
    database_name: &str,
    table_name: &str,
    old: &ColumnInfo,
    changing: &ColumnInfo,
    check_value_range: bool,
) -> String {
    let name = format!("`{}`", old.name);
    let mut conditions = Vec::new();
    if check_value_range {
        if is_integer_change(old, changing) {
            conditions.push(build_check_range_for_integer(old, changing));
        } else {
            // 字符串过长，或 VARCHAR→CHAR 时尾部空格会导致截断。
            conditions.push(format!("LENGTH({name}) > {}", changing.field_type.flen));
            if old.field_type.kind == ColumnKind::Varchar
                && changing.field_type.kind == ColumnKind::String
            {
                conditions.push(format!("{name} LIKE '% '"));
            }
        }
    }
    // NULL→NOT NULL（非“改成 Timestamp”特例）需探测存量 NULL。
    if is_null_to_not_null_change(old, changing)
        && !(old.field_type.kind != ColumnKind::Timestamp
            && changing.field_type.kind == ColumnKind::Timestamp)
    {
        conditions.push(format!("{name} IS NULL"));
    }
    if conditions.is_empty() {
        String::new()
    } else {
        format!(
            "SELECT {name} FROM `{database_name}`.`{table_name}` WHERE {} LIMIT 1",
            conditions.join(" OR ")
        )
    }
}

/// 分析分区表达式中目标列的用法（无函数 / TO_DAYS / EXTRACT / 其它）。
pub fn collect_partition_expression_usage(
    expression: &str,
    target_column: &str,
) -> BTreeSet<PartitionExpressionUsage> {
    let mut usages = BTreeSet::new();
    let bytes = expression.as_bytes();
    let target = target_column.as_bytes();
    let mut parentheses: Vec<Option<String>> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == quote {
                        if index + 1 < bytes.len() && bytes[index + 1] == quote {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'(' => {
                let mut end = index;
                while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                    end -= 1;
                }
                let mut start = end;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                let function = (start < end)
                    .then(|| String::from_utf8_lossy(&bytes[start..end]).to_ascii_lowercase());
                parentheses.push(function);
                index += 1;
            }
            b')' => {
                parentheses.pop();
                index += 1;
            }
            b'`' => {
                let start = index + 1;
                index = start;
                while index < bytes.len() && bytes[index] != b'`' {
                    index += 1;
                }
                record_partition_column_usage(
                    &bytes[start..index],
                    target,
                    &parentheses,
                    &mut usages,
                );
                index += usize::from(index < bytes.len());
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let mut next = index;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                if next == bytes.len() || bytes[next] != b'(' {
                    record_partition_column_usage(
                        &bytes[start..index],
                        target,
                        &parentheses,
                        &mut usages,
                    );
                }
            }
            _ => index += 1,
        }
    }
    usages
}

fn record_partition_column_usage(
    identifier: &[u8],
    target: &[u8],
    parentheses: &[Option<String>],
    usages: &mut BTreeSet<PartitionExpressionUsage>,
) {
    if !identifier.eq_ignore_ascii_case(target) {
        return;
    }
    if parentheses
        .iter()
        .flatten()
        .any(|function| !matches!(function.as_str(), "to_days" | "extract"))
    {
        usages.insert(PartitionExpressionUsage::Unsupported);
        return;
    }
    let usage = parentheses.iter().rev().flatten().next().map_or(
        PartitionExpressionUsage::NoFunction,
        |function| {
            if function == "to_days" {
                PartitionExpressionUsage::ToDays
            } else {
                PartitionExpressionUsage::Extract
            }
        },
    );
    usages.insert(usage);
}

/// 整型显示宽度/位宽变大（分区键允许的兼容变更之一）。
fn integer_type_widening(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    is_integer_change(old, new) && new.field_type.flen > old.field_type.flen
}

/// 非 binary 字符类型长度变大。
fn string_length_extension(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    old.field_type.kind == new.field_type.kind
        && matches!(
            old.field_type.kind,
            ColumnKind::String | ColumnKind::Varchar
        )
        && old.field_type.charset != "binary"
        && new.field_type.charset != "binary"
        && new.field_type.flen > old.field_type.flen
}

/// 时间类型小数秒精度变高。
fn time_precision_extension(old: &ColumnInfo, new: &ColumnInfo) -> bool {
    old.field_type.kind == new.field_type.kind
        && matches!(old.field_type.kind, ColumnKind::DateTime)
        && new.field_type.decimal > old.field_type.decimal
}

/// 若列参与分区，限制只能做兼容的“扩大”类变更，禁止改名与危险属性变化。
pub fn check_partition_column_modifiable(
    partition: &PartitionInfo,
    old: &ColumnInfo,
    new: &ColumnInfo,
    old_elements: &[String],
    new_elements: &[String],
) -> Result<(), ModifyColumnError> {
    let used = partition
        .columns
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&old.name))
        || !collect_partition_expression_usage(&partition.expression, &old.name).is_empty();
    if !used {
        return Ok(());
    }
    if !old.name.eq_ignore_ascii_case(&new.name) {
        return Err(ModifyColumnError::PartitionColumnRename);
    }
    // 字符集/排序规则变化，或可空→NOT NULL，对分区键不安全。
    if old.field_type.charset != new.field_type.charset
        || old.field_type.collation != new.field_type.collation
        || (old.not_null != new.not_null && !old.not_null)
    {
        return Err(ModifyColumnError::PartitionTypeChange);
    }
    // 按分区类型与表达式用法判断是否允许扩大变更。
    let allowed = match partition.partition_type {
        PartitionType::Key => {
            integer_type_widening(old, new)
                || string_length_extension(old, new)
                || (matches!(old.field_type.kind, ColumnKind::Enum | ColumnKind::Set)
                    && old.field_type.kind == new.field_type.kind
                    && !elements_changed(old_elements, new_elements))
        }
        PartitionType::Range | PartitionType::List if !partition.columns.is_empty() => {
            integer_type_widening(old, new)
                || string_length_extension(old, new)
                || time_precision_extension(old, new)
        }
        PartitionType::Hash if !partition.columns.is_empty() => false,
        PartitionType::Range | PartitionType::List | PartitionType::Hash => {
            let usages = collect_partition_expression_usage(&partition.expression, &old.name);
            !usages.contains(&PartitionExpressionUsage::Unsupported)
                && usages.iter().all(|usage| match usage {
                    PartitionExpressionUsage::NoFunction => integer_type_widening(old, new),
                    PartitionExpressionUsage::ToDays => {
                        old.field_type.kind == ColumnKind::DateTime
                            && time_precision_extension(old, new)
                    }
                    PartitionExpressionUsage::Extract => time_precision_extension(old, new),
                    PartitionExpressionUsage::Unsupported => false,
                })
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(ModifyColumnError::PartitionTypeChange)
    }
}

/// 比较 AUTO_RANDOM range bits：0 表示使用默认值后再比是否变化。
pub fn range_bits_changed(old_bits: u64, new_bits: u64, default_bits: u64) -> bool {
    let old_bits = if old_bits == 0 {
        default_bits
    } else {
        old_bits
    };
    let new_bits = if new_bits == 0 {
        default_bits
    } else {
        new_bits
    };
    old_bits != new_bits
}

/// 校验 AUTO_RANDOM（自动随机主键）分片/范围位数变更是否合法，成功则返回新 shard bits。
///
/// 不允许减少或移除 shard bits、超出上限、改类型/加自增/加默认值，也不允许改 range bits。
#[allow(clippy::too_many_arguments)]
pub fn check_auto_random(
    old_shard_bits: u64,
    old_range_bits: u64,
    new_shard_bits: u64,
    new_range_bits: u64,
    old_column: &ColumnInfo,
    new_column: &ColumnInfo,
    has_default: bool,
    context: &ModifyColumnContext,
) -> Result<u64, ModifyColumnError> {
    if old_shard_bits > new_shard_bits {
        return Err(ModifyColumnError::InvalidAutoRandom(
            if new_shard_bits == 0 {
                "remove"
            } else {
                "decrease"
            }
            .into(),
        ));
    }
    if new_shard_bits > context.auto_random_shard_bits_max {
        return Err(ModifyColumnError::InvalidAutoRandom("overflow".into()));
    }
    // 启用 AUTO_RANDOM 时列必须保持整型，且不能同时自增或带默认值。
    if old_shard_bits > 0 || new_shard_bits > 0 {
        if old_column.field_type.kind != new_column.field_type.kind
            || old_column.field_type.kind != ColumnKind::Integer
            || new_column.auto_increment
            || has_default
        {
            return Err(ModifyColumnError::InvalidAutoRandom(
                "incompatible column".into(),
            ));
        }
    }
    if range_bits_changed(
        old_range_bits,
        new_range_bits,
        context.auto_random_range_bits_default,
    ) {
        return Err(ModifyColumnError::InvalidAutoRandom("range bits".into()));
    }
    Ok(new_shard_bits)
}

/// 返回包含指定列名的索引在 `table.indices` 中的下标列表。
fn related_indices(table: &TableInfo, column_name: &str) -> Vec<usize> {
    table
        .indices
        .iter()
        .enumerate()
        .filter_map(|(offset, index)| {
            index
                .columns
                .iter()
                .any(|column| column.name == column_name)
                .then_some(offset)
        })
        .collect()
}

fn unique_changing_column_name(table: &TableInfo, old_name: &str) -> String {
    (0..)
        .map(|suffix| format!("_col$_{old_name}_{suffix}"))
        .find(|candidate| {
            table
                .columns
                .iter()
                .all(|column| !column.name.eq_ignore_ascii_case(candidate))
        })
        .expect("the changing-column suffix space is unbounded")
}

fn unique_changing_index_name(table: &TableInfo, old_name: &str) -> String {
    (0..)
        .map(|suffix| format!("_idx$_{old_name}_{suffix}"))
        .find(|candidate| {
            table
                .indices
                .iter()
                .all(|index| !index.name.eq_ignore_ascii_case(candidate))
        })
        .expect("the changing-index suffix space is unbounded")
}

/// 将最终列定义写回表：保留原列 ID、同步索引/外键列名，并按需移动列位置。
fn apply_modified_column(
    table: &mut TableInfo,
    old_id: i64,
    mut new_column: ColumnInfo,
    position: &ColumnPosition,
) -> Result<(), ModifyColumnError> {
    let old_offset = table
        .columns
        .iter()
        .position(|column| column.id == old_id)
        .ok_or_else(|| ModifyColumnError::ColumnNotFound(old_id.to_string()))?;
    let old_name = table.columns[old_offset].name.clone();
    let destination = locate_offset_to_move(old_offset, position, table)
        .map_err(|_| ModifyColumnError::InvalidPosition)?;
    // 在线变更完成：清除中间标志，状态置 Public。
    new_column.id = old_id;
    new_column.offset = old_offset;
    new_column.state = SchemaState::Public;
    new_column.prevent_null_insert = false;
    new_column.change_dependency_offset = None;
    table.columns[old_offset] = new_column.clone();
    for index in &mut table.indices {
        for index_column in &mut index.columns {
            if index_column.name == old_name {
                update_index_column(index_column, &new_column);
            }
        }
    }
    for foreign_key in &mut table.foreign_keys {
        for name in &mut foreign_key.columns {
            if *name == old_name {
                name.clone_from(&new_column.name);
            }
        }
    }
    table
        .move_column_info(old_offset, destination)
        .map_err(|_| ModifyColumnError::InvalidPosition)
}

/// 首次进入重组路径时创建隐藏的 changing 列与对应 changing 索引（在线 DDL 双对象）。
fn initialize_changing_objects(
    table: &mut TableInfo,
    args: &mut ModifyColumnArgs,
    old: &ColumnInfo,
) {
    if args.changing_column_id.is_some() {
        return;
    }
    let max_index_id = table
        .indices
        .iter()
        .map(|index| index.id)
        .max()
        .unwrap_or(0);
    let mut next_index_id = max_index_id;
    let mut changing = args.column.clone();
    // 分配新列 ID，挂到表尾，并记录对旧列的依赖偏移。
    table.max_column_id += 1;
    changing.id = table.max_column_id;
    changing.name = unique_changing_column_name(table, &old.name);
    changing.offset = table.columns.len();
    changing.state = SchemaState::None;
    changing.change_dependency_offset = Some(old.offset);
    args.changing_column_id = Some(changing.id);
    table.columns.push(changing.clone());

    // 为每个相关索引克隆一份 changing 索引，指向新列类型。
    for index_offset in related_indices(table, &old.name) {
        let mut changing_index: IndexInfo = table.indices[index_offset].clone();
        next_index_id += 1;
        changing_index.id = next_index_id;
        changing_index.name = unique_changing_index_name(table, &changing_index.name);
        changing_index.state = SchemaState::None;
        for index_column in &mut changing_index.columns {
            if index_column.name == old.name {
                index_column.use_changing_type = true;
                update_index_column(index_column, &changing);
            }
        }
        args.changing_index_ids.push(changing_index.id);
        table.indices.push(changing_index);
    }
}

/// 回滚时删除 changing 列/索引，并清理所有列上的在线修改中间标志。
fn rollback_changing_objects(table: &mut TableInfo, args: &ModifyColumnArgs) {
    if let Some(column_id) = args.changing_column_id {
        table.columns.retain(|column| column.id != column_id);
    }
    table
        .indices
        .retain(|index| !args.changing_index_ids.contains(&index.id));
    for (offset, column) in table.columns.iter_mut().enumerate() {
        column.offset = offset;
        column.prevent_null_insert = false;
        column.change_dependency_offset = None;
    }
}

/// 一次状态推进的结果：当前 Schema 状态、版本号以及是否已结束/已回滚。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModifyColumnOutcome {
    /// 当前（changing）对象所处 Schema 状态。
    pub schema_state: SchemaState,
    /// 推进后的 Schema 版本。
    pub schema_version: i64,
    /// Job 是否已完成。
    pub finished: bool,
    /// 是否完成了回滚清理。
    pub rollback_done: bool,
}

/// 推进一次 MODIFY COLUMN 的在线 DDL 状态；可完成、继续等待或执行回滚。
///
/// `data_is_valid` 由外部检查 SQL 的结果注入；`rolling_back` 为真时清理 changing 对象。
#[allow(clippy::too_many_arguments)]
pub fn advance_modify_column(
    table: &mut TableInfo,
    args: &mut ModifyColumnArgs,
    context: &ModifyColumnContext,
    data_is_valid: bool,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<ModifyColumnOutcome, ModifyColumnError> {
    // 按列 ID 或列名（含 removing 前缀）定位旧列。
    let old_offset = if args.old_column_id > 0 {
        table
            .columns
            .iter()
            .position(|column| column.id == args.old_column_id)
    } else {
        table.columns.iter().position(|column| {
            column.name.eq_ignore_ascii_case(&args.old_column_name)
                || column.name == format!("_tidb_removing_{}", args.old_column_name)
        })
    }
    .ok_or_else(|| ModifyColumnError::ColumnNotFound(args.old_column_name.clone()))?;
    args.old_column_id = table.columns[old_offset].id;
    let old = table.columns[old_offset].clone();
    check_column_already_exists(table, &old.name, &args.column.name)?;
    if let Some((dependent, hidden)) = has_dependent_generated_column(table, &old.name) {
        return Err(if hidden {
            ModifyColumnError::DependentFunctionalIndex(dependent.into())
        } else {
            ModifyColumnError::DependentGeneratedColumn(dependent.into())
        });
    }
    let indexed = !related_indices(table, &old.name).is_empty();
    check_modify_types(&old, &args.column, indexed)?;
    if let Some(partition) = &context.partition {
        check_partition_column_modifiable(
            partition,
            &old,
            &args.column,
            &args.old_elements,
            &args.new_elements,
        )?;
    }
    // 首次进入：选定修改路径；VARCHAR→CHAR 可能先走 Precheck。
    if args.modify_type == ModifyColumnType::None {
        args.modify_type = get_modify_column_type(table, args, &old, context);
        if old.field_type.kind == ColumnKind::Varchar
            && args.column.field_type.kind == ColumnKind::String
            && matches!(
                args.modify_type,
                ModifyColumnType::NoReorgWithCheck | ModifyColumnType::IndexReorg
            )
        {
            args.modify_type = ModifyColumnType::Precheck;
        }
    }
    if rolling_back {
        rollback_changing_objects(table, args);
        *schema_version += 1;
        return Ok(ModifyColumnOutcome {
            schema_state: SchemaState::None,
            schema_version: *schema_version,
            finished: true,
            rollback_done: true,
        });
    }

    if args.modify_type == ModifyColumnType::Precheck {
        if !data_is_valid {
            return Err(ModifyColumnError::DataTruncated);
        }
        args.modify_type = if indexed {
            ModifyColumnType::IndexReorg
        } else {
            ModifyColumnType::NoReorgWithCheck
        };
    }
    // 无需重组：校验通过后直接写回最终列定义。
    if matches!(
        args.modify_type,
        ModifyColumnType::NoReorg | ModifyColumnType::NoReorgWithCheck
    ) {
        if args.modify_type == ModifyColumnType::NoReorgWithCheck && !data_is_valid {
            return Err(if is_null_to_not_null_change(&old, &args.column) {
                ModifyColumnError::InvalidNull
            } else {
                ModifyColumnError::DataTruncated
            });
        }
        apply_modified_column(table, old.id, args.column.clone(), &args.position)?;
        *schema_version += 1;
        return Ok(ModifyColumnOutcome {
            schema_state: SchemaState::Public,
            schema_version: *schema_version,
            finished: true,
            rollback_done: false,
        });
    }

    // 重组路径：创建/推进 changing 对象的 Schema 状态机。
    initialize_changing_objects(table, args, &old);
    if is_null_to_not_null_change(&old, &args.column) {
        table.columns[old_offset].prevent_null_insert = true;
    }
    let changing_id = args
        .changing_column_id
        .ok_or_else(|| ModifyColumnError::ColumnNotFound("changing column".into()))?;
    let changing_offset = table
        .columns
        .iter()
        .position(|column| column.id == changing_id)
        .ok_or_else(|| ModifyColumnError::ColumnNotFound(changing_id.to_string()))?;
    let current = table.columns[changing_offset].state;
    let next = match current {
        SchemaState::None => SchemaState::DeleteOnly,
        SchemaState::DeleteOnly => SchemaState::WriteOnly,
        SchemaState::WriteOnly => SchemaState::WriteReorganization,
        SchemaState::WriteReorganization => SchemaState::Public,
        state => return Err(ModifyColumnError::InvalidState(state)),
    };
    table.columns[changing_offset].state = next;
    for index in table
        .indices
        .iter_mut()
        .filter(|index| args.changing_index_ids.contains(&index.id))
    {
        index.state = next;
    }
    *schema_version += 1;
    if next != SchemaState::Public {
        return Ok(ModifyColumnOutcome {
            schema_state: next,
            schema_version: *schema_version,
            finished: false,
            rollback_done: false,
        });
    }
    // Public：把目标定义落到原列 ID，并删除临时 changing 列/索引。
    let mut final_column = args.column.clone();
    final_column.id = old.id;
    apply_modified_column(table, old.id, final_column, &args.position)?;
    table.columns.retain(|column| column.id != changing_id);
    let changing_ids = args.changing_index_ids.clone();
    table
        .indices
        .retain(|index| !changing_ids.contains(&index.id));
    for (offset, column) in table.columns.iter_mut().enumerate() {
        column.offset = offset;
    }
    Ok(ModifyColumnOutcome {
        schema_state: SchemaState::Public,
        schema_version: *schema_version,
        finished: true,
        rollback_done: false,
    })
}

/// 是否为 CHAR ↔ VARCHAR 之间的类型互换。
pub fn convert_between_char_and_varchar(old: ColumnKind, new: ColumnKind) -> bool {
    matches!(
        (old, new),
        (ColumnKind::String, ColumnKind::Varchar) | (ColumnKind::Varchar, ColumnKind::String)
    )
}

/// 同步写入修改后列的默认值与原始默认值字段。
pub fn set_default_for_modified_column(column: &mut ColumnInfo, value: Option<DefaultValue>) {
    column.default_value.clone_from(&value);
    column.origin_default_value = value;
}

impl From<ColumnError> for ModifyColumnError {
    fn from(_: ColumnError) -> Self {
        Self::InvalidPosition
    }
}
