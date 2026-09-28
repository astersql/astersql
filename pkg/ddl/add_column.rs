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

// ADD COLUMN（添加列）DDL 逻辑模块。
//
// 本模块实现在线添加列的核心校验与状态推进流程，对应 TiDB 的
// `ddl/add_column` 逻辑。在线 DDL（Online Schema Change）采用
// F1/Google 提出的多阶段模式演进协议：新列会依次经历
// `None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public`
// 等模式状态（SchemaState），保证集群中不同节点在相邻两个版本的
// 模式（schema）下并发读写仍然一致。
//
// 主要职责：
// - 校验列定义的约束、字符集、默认值、生成列（Generated Column）表达式；
// - 根据类型标志调整字段类型（如 BLOB 长度归一化、二进制排序规则）；
// - 创建新列并加入表元数据（`TableInfo`）；
// - 按状态机推进添加列任务，或在回滚时移除列。

use std::collections::HashSet;

use crate::column::{
    ColumnInfo, ColumnKind, ColumnPosition, DefaultValue, FieldType, SchemaState, TableInfo,
    check_add_column_too_many_columns, init_and_add_column_to_table, locate_offset_to_move,
};
use crate::generated_column::{
    ExpressionNode, GeneratedColumnError, GenerationType, check_auto_increment_reference,
    check_depended_columns_exist, check_illegal_function_for_generated,
    verify_column_generation_single,
};

/// TINYBLOB 类型的最大字节长度（2^8 - 1）。
const TINY_BLOB_MAX_LENGTH: usize = 255;
/// BLOB 类型的最大字节长度（2^16 - 1）。
const BLOB_MAX_LENGTH: usize = 65_535;
/// MEDIUMBLOB 类型的最大字节长度（2^24 - 1）。
const MEDIUM_BLOB_MAX_LENGTH: usize = 16_777_215;
/// LONGBLOB 类型的最大字节长度（2^32 - 1）。
const LONG_BLOB_MAX_LENGTH: usize = 4_294_967_295;

/// 列级约束类型，对应 SQL 列定义中可携带的各种约束选项。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnConstraint {
    /// NOT NULL：列值不允许为空。
    NotNull,
    /// NULL：显式声明列值允许为空。
    Null,
    /// AUTO_INCREMENT：自增列，插入时自动分配递增值。
    AutoIncrement,
    /// PRIMARY KEY：主键约束。
    PrimaryKey,
    /// UNIQUE KEY：唯一键约束。
    UniqueKey,
    /// AUTO_RANDOM：TiDB 扩展的随机自增主键，用于打散写入热点。
    AutoRandom,
    /// BINARY：以二进制方式比较/存储字符串。
    Binary,
    /// ON UPDATE CURRENT_TIMESTAMP：行更新时自动刷新为当前时间戳。
    OnUpdateCurrentTimestamp,
}

/// 生成列（Generated Column）的定义。
///
/// 生成列的值由表达式基于同一行其他列计算得出，分为虚拟（VIRTUAL，
/// 读取时计算）与存储（STORED，写入时计算并落盘）两种。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedDefinition {
    /// 生成表达式的原始 SQL 文本。
    pub expression_sql: String,
    /// 解析后的表达式语法树节点。
    pub expression: ExpressionNode,
    /// 表达式依赖的其他列名集合。
    pub dependencies: HashSet<String>,
    /// 是否为存储生成列（STORED）；false 表示虚拟生成列。
    pub stored: bool,
}

/// ADD COLUMN 语句中的新列定义。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnDefinition {
    /// 列名。
    pub name: String,
    /// 字段类型（含类型种类、长度、字符集、排序规则等）。
    pub field_type: FieldType,
    /// 列上声明的约束列表。
    pub constraints: Vec<ColumnConstraint>,
    /// 默认值；None 表示未指定默认值。
    pub default_value: Option<DefaultValue>,
    /// 列注释（COMMENT 子句内容）。
    pub comment: String,
    /// 生成列定义；None 表示普通列。
    pub generated: Option<GeneratedDefinition>,
}

/// 添加列过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddColumnError {
    /// 同名列已存在。
    ColumnExists(String),
    /// ADD COLUMN 不支持的约束（如自增、主键等）。
    UnsupportedConstraint(ColumnConstraint),
    /// 表带有 TiFlash（列式存储副本）副本时不支持的字符集。
    UnsupportedTiFlashCharset(String),
    /// 不支持在线添加存储生成列（STORED generated column）。
    StoredGeneratedColumn,
    /// 生成列校验错误的包装。
    Generated(GeneratedColumnError),
    /// 默认值使用了不安全（非确定性）的函数。
    UnsafeDefaultFunction(String),
    /// 默认值非法（如 NOT NULL 列默认为 NULL）。
    InvalidDefault(String),
    /// 引用了不存在的列。
    UnknownColumn(String),
    /// 表的列数超过上限。
    TooManyColumns,
    /// 遇到了状态机中不合法的模式状态。
    InvalidState(SchemaState),
}

/// 允许用 `?` 将生成列错误自动转换为添加列错误。
impl From<GeneratedColumnError> for AddColumnError {
    fn from(value: GeneratedColumnError) -> Self {
        Self::Generated(value)
    }
}

/// 检查列定义中是否包含 ADD COLUMN 不支持的约束。
///
/// 自增、主键、唯一键、AUTO_RANDOM 都需要重建索引或改变行编码，
/// 无法通过在线添加列实现，因此直接拒绝。
pub fn check_unsupported_column_constraint(
    definition: &ColumnDefinition,
) -> Result<(), AddColumnError> {
    for constraint in &definition.constraints {
        if matches!(
            constraint,
            ColumnConstraint::AutoIncrement
                | ColumnConstraint::PrimaryKey
                | ColumnConstraint::UniqueKey
                | ColumnConstraint::AutoRandom
        ) {
            return Err(AddColumnError::UnsupportedConstraint(*constraint));
        }
    }
    Ok(())
}

/// 当表带有 TiFlash 副本时，校验新列字符集是否受支持。
///
/// TiFlash 是 TiDB 的列式存储引擎，仅支持有限的字符集集合；
/// 若表配置了 TiFlash 副本且新列字符集不在白名单内则报错。
pub fn check_unsupported_charset_for_tiflash(
    table: &TableInfo,
    definition: &ColumnDefinition,
) -> Result<(), AddColumnError> {
    // 仅当表存在 TiFlash 副本时才做字符集白名单检查。
    if table.tiflash_replica
        && !matches!(
            definition.field_type.charset.as_str(),
            "binary" | "ascii" | "latin1" | "utf8" | "utf8mb4"
        )
    {
        return Err(AddColumnError::UnsupportedTiFlashCharset(
            definition.field_type.charset.clone(),
        ));
    }
    Ok(())
}

/// 若列声明了 BINARY 标志，则将排序规则改写为对应字符集的 `_bin` 版本。
///
/// 排序规则（collation）决定字符串比较与排序方式；`_bin` 后缀表示
/// 按字节二进制比较。仅对字符串类字段（VARCHAR/CHAR/ENUM/SET）生效。
pub fn overwrite_collation_with_binary_flag(field_type: &mut FieldType) {
    if field_type.binary
        && matches!(
            field_type.kind,
            ColumnKind::Varchar | ColumnKind::String | ColumnKind::Enum | ColumnKind::Set
        )
        && !field_type.charset.is_empty()
    {
        field_type.collation = format!("{}_bin", field_type.charset);
    }
}

/// 按 MySQL 兼容规则规范化字段类型标志。
///
/// - BIT 列：强制无符号，且不参与二进制比较；
/// - YEAR 列：强制补零显示（zerofill）；
/// - zerofill 隐含 unsigned（MySQL 语义）。
pub fn process_column_flags(field_type: &mut FieldType) {
    if matches!(
        field_type.kind,
        ColumnKind::Varchar
            | ColumnKind::String
            | ColumnKind::Enum
            | ColumnKind::Set
            | ColumnKind::TinyBlob
            | ColumnKind::Blob
            | ColumnKind::MediumBlob
            | ColumnKind::LongBlob
    ) {
        field_type.binary = field_type.charset == "binary";
    }
    if matches!(field_type.kind, ColumnKind::Bit) {
        field_type.binary = false;
        field_type.unsigned = true;
    }
    if matches!(field_type.kind, ColumnKind::Year) {
        field_type.binary = false;
        field_type.zerofill = true;
    }
    if field_type.zerofill {
        field_type.unsigned = true;
    }
}

/// 根据声明长度与字符集单字符最大字节数，把 BLOB 归一化为合适的子类型。
///
/// 例如 `BLOB(100)` 实际所需字节数不超过 255 时会降级为 TINYBLOB，
/// 超过 65535 时升级为 MEDIUMBLOB/LONGBLOB，与 MySQL 的行为一致。
pub fn adjust_blob_type_length(
    field_type: &mut FieldType,
    charset_max_length: usize,
) -> Result<(), AddColumnError> {
    if field_type.kind != ColumnKind::Blob {
        return Ok(());
    }
    // 所需字节数 = 声明长度 × 字符集单字符最大字节数，乘法溢出视为非法。
    let required = field_type
        .flen
        .checked_mul(charset_max_length)
        .ok_or_else(|| AddColumnError::InvalidDefault("blob length overflow".into()))?;
    // 按所需字节数选取能容纳它的最小 BLOB 子类型，并将 flen 设为该类型上限。
    match required {
        0..=TINY_BLOB_MAX_LENGTH => {
            field_type.kind = ColumnKind::TinyBlob;
            field_type.flen = TINY_BLOB_MAX_LENGTH;
        }
        ..=BLOB_MAX_LENGTH => field_type.flen = BLOB_MAX_LENGTH,
        ..=MEDIUM_BLOB_MAX_LENGTH => {
            field_type.kind = ColumnKind::MediumBlob;
            field_type.flen = MEDIUM_BLOB_MAX_LENGTH;
        }
        ..=LONG_BLOB_MAX_LENGTH => {
            field_type.kind = ColumnKind::LongBlob;
            field_type.flen = LONG_BLOB_MAX_LENGTH;
        }
        _ => {
            return Err(AddColumnError::InvalidDefault(
                "blob length is too large".into(),
            ));
        }
    }
    Ok(())
}

/// 校验列的默认值定义并返回可用的默认值。
///
/// 两条规则：
/// 1. 默认值表达式不允许使用非确定性/不安全函数（如 rand、uuid），
///    否则在线添加列时无法为存量行回填一致的默认值；
/// 2. NOT NULL 列不允许显式指定 DEFAULT NULL。
fn check_default_value(
    definition: &ColumnDefinition,
) -> Result<Option<DefaultValue>, AddColumnError> {
    if let Some(DefaultValue::Expression(expression)) = &definition.default_value {
        // 取表达式左括号前的部分作为函数名，转小写后比对黑名单。
        let function = expression
            .split_once('(')
            .map_or(expression.as_str(), |(name, _)| name)
            .to_ascii_lowercase();
        if matches!(
            function.as_str(),
            "nextval" | "rand" | "uuid" | "uuid_to_bin" | "replace" | "upper"
        ) {
            return Err(AddColumnError::UnsafeDefaultFunction(function));
        }
    }
    if definition.constraints.contains(&ColumnConstraint::NotNull)
        && matches!(definition.default_value, Some(DefaultValue::Null))
    {
        return Err(AddColumnError::InvalidDefault(definition.name.clone()));
    }
    Ok(definition.default_value.clone())
}

/// 校验列定义并构造新列的元数据（`ColumnInfo`）。
///
/// 依次执行：约束检查、TiFlash 字符集检查、保留列名检查、重名检查、
/// 生成列表达式校验、字段类型规范化、默认值校验，最终生成列元数据。
///
/// 参数说明：
/// - `position`：新列插入位置（FIRST / AFTER xxx / 末尾）；
/// - `enable_auto_increment_in_generated`：是否允许生成列引用自增列；
/// - `expression_index_enabled`：是否启用表达式索引特性开关。
pub fn create_new_column(
    table: &TableInfo,
    definition: &ColumnDefinition,
    position: &ColumnPosition,
    enable_auto_increment_in_generated: bool,
    expression_index_enabled: bool,
) -> Result<ColumnInfo, AddColumnError> {
    check_unsupported_column_constraint(definition)?;
    check_unsupported_charset_for_tiflash(table, definition)?;
    // `_tidb_rowid` 是内部隐藏行 ID 列名，禁止用户占用。
    if definition.name.eq_ignore_ascii_case("_tidb_rowid") {
        return Err(AddColumnError::InvalidDefault(definition.name.clone()));
    }
    if table
        .columns
        .iter()
        .any(|column| column.name.eq_ignore_ascii_case(&definition.name))
    {
        return Err(AddColumnError::ColumnExists(definition.name.clone()));
    }

    // 生成列的额外校验：仅支持虚拟生成列，且需检查表达式合法性、
    // 依赖列存在性与自增列引用限制。
    if let Some(generated) = &definition.generated {
        if generated.stored {
            return Err(AddColumnError::StoredGeneratedColumn);
        }
        check_illegal_function_for_generated(
            &definition.name,
            GenerationType::Column,
            &generated.expression,
            expression_index_enabled,
        )?;
        let mut missing = generated.dependencies.clone();
        check_depended_columns_exist(&mut missing, &table.columns)?;
        verify_column_generation_single(&generated.dependencies, &table.columns, position)?;
        if !enable_auto_increment_in_generated {
            check_auto_increment_reference(&definition.name, &generated.dependencies, table)?;
        }
    }

    // 规范化字段类型标志与排序规则，然后校验默认值。
    let mut field_type = definition.field_type.clone();
    overwrite_collation_with_binary_flag(&mut field_type);
    process_column_flags(&mut field_type);
    let default_value = check_default_value(definition)?;
    let mut column = ColumnInfo::new(&definition.name, field_type);
    column.not_null = definition.constraints.contains(&ColumnConstraint::NotNull);
    column.default_value.clone_from(&default_value);
    column.origin_default_value = default_value;
    if let Some(generated) = &definition.generated {
        column.generated = true;
        column.generated_stored = generated.stored;
        column
            .generated_expression
            .clone_from(&generated.expression_sql);
        column.dependencies.clone_from(&generated.dependencies);
    }
    Ok(column)
}

/// 启动添加列流程：创建列、检查列数上限并把列加入表元数据。
///
/// 返回新列分配到的列 ID；新列此时处于初始状态（尚未 Public），
/// 后续由 [`advance_add_column`] 按状态机逐步推进。
pub fn start_add_column(
    table: &mut TableInfo,
    definition: &ColumnDefinition,
    position: &ColumnPosition,
    column_limit: usize,
    enable_auto_increment_in_generated: bool,
    expression_index_enabled: bool,
) -> Result<i64, AddColumnError> {
    let column = create_new_column(
        table,
        definition,
        position,
        enable_auto_increment_in_generated,
        expression_index_enabled,
    )?;
    check_add_column_too_many_columns(table.columns.len() + 1, column_limit)
        .map_err(|_| AddColumnError::TooManyColumns)?;
    Ok(init_and_add_column_to_table(table, column))
}

/// 一次状态推进后的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddColumnOutcome {
    /// 推进后新列所处的模式状态。
    pub schema_state: SchemaState,
    /// 推进后的模式版本号（每次模式变更单调递增）。
    pub schema_version: i64,
    /// 添加列任务是否结束（到达 Public 或回滚完成）。
    pub finished: bool,
}

/// 将添加列任务向前推进一个模式状态，或在回滚时移除新列。
///
/// 正常路径按在线 DDL 状态机推进：
/// `None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public`。
/// 每次状态变更都会递增模式版本号（schema version），供其他节点
/// 感知模式演进；到达 Public 时把新列移动到用户指定的位置。
pub fn advance_add_column(
    table: &mut TableInfo,
    column_id: i64,
    position: &ColumnPosition,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<AddColumnOutcome, AddColumnError> {
    let index = table
        .columns
        .iter()
        .position(|column| column.id == column_id)
        .ok_or_else(|| AddColumnError::UnknownColumn(column_id.to_string()))?;
    // 回滚路径：直接移除新列并重排剩余列的偏移量。
    if rolling_back {
        table.columns.remove(index);
        for (offset, column) in table.columns.iter_mut().enumerate() {
            column.offset = offset;
        }
        *schema_version += 1;
        return Ok(AddColumnOutcome {
            schema_state: SchemaState::None,
            schema_version: *schema_version,
            finished: true,
        });
    }

    // 正常路径：由当前状态计算下一个状态，非法状态直接报错。
    let current = table.columns[index].state;
    let next = match current {
        SchemaState::None => SchemaState::DeleteOnly,
        SchemaState::DeleteOnly => SchemaState::WriteOnly,
        SchemaState::WriteOnly => SchemaState::WriteReorganization,
        SchemaState::WriteReorganization => SchemaState::Public,
        state => return Err(AddColumnError::InvalidState(state)),
    };
    // 进入 Public（对外可见）时，把列移动到 FIRST/AFTER 指定的位置。
    if next == SchemaState::Public {
        let destination = locate_offset_to_move(index, position, table)
            .map_err(|_| AddColumnError::UnknownColumn(format!("{position:?}")))?;
        table.columns[index].state = next;
        table
            .move_column_info(index, destination)
            .map_err(|_| AddColumnError::UnknownColumn(column_id.to_string()))?;
    } else {
        table.columns[index].state = next;
    }
    *schema_version += 1;
    Ok(AddColumnOutcome {
        schema_state: next,
        schema_version: *schema_version,
        finished: next == SchemaState::Public,
    })
}
