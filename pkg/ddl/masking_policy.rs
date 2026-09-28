// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 列脱敏策略（masking policy）DDL 与内存元数据存储。
//
// 脱敏策略把查询结果中的敏感列值改写成遮蔽表达式（如 MASK_FULL）。
// 本模块定义策略元信息、目标表校验、表达式校验、按表/列/库的级联清理，
// 以及列改名时重写表达式中列标识符的词法替换逻辑。

use std::collections::BTreeMap;

use crate::column::{ColumnInfo, ColumnKind, SchemaState, TableInfo};

/// 脱敏策略启用状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskingPolicyStatus {
    /// 策略生效，查询结果按表达式遮蔽。
    Enable,
    /// 策略暂时关闭，查询返回原始列值。
    Disable,
}

/// 内置脱敏函数类型；无法识别的函数名归为 `Custom`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskingPolicyType {
    /// 全量遮蔽（MASK_FULL）。
    Full,
    /// 部分遮蔽（MASK_PARTIAL）。
    Partial,
    /// 返回 NULL（MASK_NULL）。
    Null,
    /// 日期类遮蔽（MASK_DATE）。
    Date,
    /// 自定义 SQL 表达式。
    Custom,
}

/// 限制哪些 DML/DDL 路径禁止读取已脱敏列（位标志集合）。
///
/// 例如 `INSERT INTO ... SELECT`、`UPDATE ... SELECT`、`DELETE ... SELECT`、
/// CTAS（CREATE TABLE AS SELECT）在包含受限标志时会被拦截。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaskingPolicyRestrictOps(u8);

impl MaskingPolicyRestrictOps {
    /// 禁止 INSERT INTO ... SELECT 读取脱敏列。
    pub const INSERT_INTO_SELECT: Self = Self(1);
    /// 禁止 UPDATE ... SELECT 读取脱敏列。
    pub const UPDATE_SELECT: Self = Self(2);
    /// 禁止 DELETE ... SELECT 读取脱敏列。
    pub const DELETE_SELECT: Self = Self(4);
    /// 禁止 CREATE TABLE AS SELECT 读取脱敏列。
    pub const CTAS: Self = Self(8);

    /// 是否包含指定限制标志。
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// 并入新的限制标志。
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// 解析后的脱敏表达式：原始 SQL、顶层函数名及引用的列名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaskingExpression {
    /// 表达式 SQL 文本。
    pub sql: String,
    /// 若为内置 MASK_* 函数调用，则为函数名。
    pub function_name: Option<String>,
    /// 表达式中引用的列名列表（策略仅允许引用目标列）。
    pub referenced_columns: Vec<String>,
}

/// 一条脱敏策略的完整元信息（对应系统表 `mysql.tidb_masking_policy`）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaskingPolicyInfo {
    /// 策略 ID。
    pub id: i64,
    /// 策略名（通常小写存储）。
    pub name: String,
    /// 所属数据库名。
    pub database_name: String,
    /// 所属表名。
    pub table_name: String,
    /// 表 ID。
    pub table_id: i64,
    /// 绑定的列名。
    pub column_name: String,
    /// 绑定的列 ID。
    pub column_id: i64,
    /// 脱敏表达式 SQL。
    pub expression: String,
    /// 启用/禁用状态。
    pub status: MaskingPolicyStatus,
    /// 脱敏类型。
    pub masking_type: MaskingPolicyType,
    /// 受限操作位标志。
    pub restrict_ops: MaskingPolicyRestrictOps,
    /// 创建时间戳。
    pub created_at: u64,
    /// 最近更新时间戳。
    pub updated_at: u64,
    /// 创建者。
    pub created_by: String,
    /// 在线 DDL Schema 状态（None/Public 等）。
    pub state: SchemaState,
}

/// 可作为策略目标的对象种类；当前仅基表（Base）允许挂策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskingTableKind {
    /// 普通基表。
    Base,
    /// 视图。
    View,
    /// 序列对象。
    Sequence,
}

/// 创建/校验策略时的目标表上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaskingTarget<'a> {
    /// 数据库名。
    pub database_name: &'a str,
    /// 是否为系统库（系统表不允许脱敏）。
    pub system_schema: bool,
    /// 是否为临时表（临时表不允许脱敏）。
    pub temporary: bool,
    /// 对象种类。
    pub kind: MaskingTableKind,
    /// 表元信息。
    pub table: &'a TableInfo,
}

/// 脱敏策略校验与存储操作的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaskingPolicyError {
    /// 缺少策略定义。
    MissingPolicy,
    /// 同名策略已存在且未要求 REPLACE。
    PolicyExists,
    /// 同名策略已绑定到其他列。
    PolicyExistsOnAnotherColumn,
    /// 按名查找策略失败。
    PolicyNotFound(String),
    /// 表 ID 与目标不一致或找不到表。
    TableNotFound(i64),
    /// 列 ID/名找不到。
    ColumnNotFound(i64),
    /// 目标不是允许挂策略的对象类型。
    WrongObject,
    /// 目标是临时表。
    TemporaryTable,
    /// 目标位于系统库。
    SystemTable,
    /// 生成列不允许挂策略。
    GeneratedColumn,
    /// 列类型不在支持列表中。
    UnsupportedColumnType,
    /// 表达式引用了非目标列。
    InvalidExpressionColumn(String),
    /// 表达式为空或非法。
    InvalidExpression,
    /// 无法识别的状态字符串。
    UnknownStatus(String),
    /// 无法识别的 restrict 操作名。
    UnknownRestrictOperation(String),
    /// 系统表行数据非法。
    InvalidRow,
}

/// 判断列类型是否支持挂脱敏策略（整型、字符串/BLOB、时间类等）。
pub fn is_masking_policy_supported_type(column: &ColumnInfo) -> bool {
    matches!(
        column.field_type.kind,
        ColumnKind::Integer
            | ColumnKind::Varchar
            | ColumnKind::String
            | ColumnKind::TinyBlob
            | ColumnKind::Blob
            | ColumnKind::MediumBlob
            | ColumnKind::LongBlob
            | ColumnKind::Timestamp
            | ColumnKind::DateTime
            | ColumnKind::Year
    )
}

/// 校验列可否挂策略：拒绝生成列与不支持的类型。
pub fn check_masking_policy_column(column: &ColumnInfo) -> Result<(), MaskingPolicyError> {
    if column.generated {
        return Err(MaskingPolicyError::GeneratedColumn);
    }
    if !is_masking_policy_supported_type(column) {
        return Err(MaskingPolicyError::UnsupportedColumnType);
    }
    Ok(())
}

/// 校验策略目标表合法，并把规范化后的库/表/列名写回 `policy`。
pub fn validate_masking_policy_target(
    target: &MaskingTarget<'_>,
    policy: &mut MaskingPolicyInfo,
) -> Result<(), MaskingPolicyError> {
    // 仅普通基表可挂策略；临时表与系统库一律拒绝。
    if target.kind != MaskingTableKind::Base {
        return Err(MaskingPolicyError::WrongObject);
    }
    if target.temporary {
        return Err(MaskingPolicyError::TemporaryTable);
    }
    if target.system_schema {
        return Err(MaskingPolicyError::SystemTable);
    }
    if target.table.id != policy.table_id {
        return Err(MaskingPolicyError::TableNotFound(policy.table_id));
    }
    let column = target
        .table
        .columns
        .iter()
        .find(|column| column.id == policy.column_id)
        .ok_or(MaskingPolicyError::ColumnNotFound(policy.column_id))?;
    check_masking_policy_column(column)?;
    // 用目标表上的规范名称回填策略元数据。
    policy.database_name = target.database_name.to_owned();
    policy.table_name.clone_from(&target.table.name);
    policy.column_name.clone_from(&column.name);
    Ok(())
}

/// 校验表达式非空，且引用的列只能是目标列本身。
pub fn validate_masking_policy_expression(
    target_column: &ColumnInfo,
    expression: &MaskingExpression,
) -> Result<(), MaskingPolicyError> {
    if expression.sql.trim().is_empty() {
        return Err(MaskingPolicyError::InvalidExpression);
    }
    // 任一非目标列引用都会使策略非法。
    if let Some(column) = expression
        .referenced_columns
        .iter()
        .find(|column| !column.eq_ignore_ascii_case(&target_column.name))
    {
        return Err(MaskingPolicyError::InvalidExpressionColumn(column.clone()));
    }
    Ok(())
}

/// 根据表达式顶层函数名推断内置脱敏类型。
pub fn masking_policy_type_from_expression(expression: &MaskingExpression) -> MaskingPolicyType {
    match expression
        .function_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mask_full" => MaskingPolicyType::Full,
        "mask_partial" => MaskingPolicyType::Partial,
        "mask_null" => MaskingPolicyType::Null,
        "mask_date" => MaskingPolicyType::Date,
        _ => MaskingPolicyType::Custom,
    }
}

/// 组装一条待写入的 `MaskingPolicyInfo`（ID 暂为 0，由 Store 分配）。
#[allow(clippy::too_many_arguments)]
pub fn build_masking_policy_info(
    target: &MaskingTarget<'_>,
    policy_name: &str,
    column_name: &str,
    expression: &MaskingExpression,
    restrict_ops: MaskingPolicyRestrictOps,
    explicitly_disabled: bool,
    created_by: &str,
    now: u64,
) -> Result<MaskingPolicyInfo, MaskingPolicyError> {
    let column = target
        .table
        .columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(column_name))
        .ok_or(MaskingPolicyError::ColumnNotFound(0))?;
    check_masking_policy_column(column)?;
    validate_masking_policy_expression(column, expression)?;
    // 填充策略字段；状态默认 Enable，除非显式 Disable。
    let mut policy = MaskingPolicyInfo {
        id: 0,
        name: policy_name.to_ascii_lowercase(),
        database_name: target.database_name.to_owned(),
        table_name: target.table.name.clone(),
        table_id: target.table.id,
        column_name: column.name.clone(),
        column_id: column.id,
        expression: expression.sql.clone(),
        status: if explicitly_disabled {
            MaskingPolicyStatus::Disable
        } else {
            MaskingPolicyStatus::Enable
        },
        masking_type: masking_policy_type_from_expression(expression),
        restrict_ops,
        created_at: now,
        updated_at: now,
        created_by: created_by.into(),
        state: SchemaState::None,
    };
    validate_masking_policy_target(target, &mut policy)?;
    Ok(policy)
}

/// 内存中的脱敏策略仓库：分配 ID、维护 schema_version，并提供级联更新。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaskingPolicyStore {
    /// 下一个可分配的策略 ID。
    next_id: i64,
    /// 按策略 ID 索引的策略集合。
    policies: BTreeMap<i64, MaskingPolicyInfo>,
    /// 策略元数据变更后递增的 Schema 版本号。
    pub schema_version: i64,
}

impl MaskingPolicyStore {
    /// 创建策略；`replace_on_exist` 为真时覆盖同名同列策略（OR REPLACE）。
    pub fn create(
        &mut self,
        mut policy: MaskingPolicyInfo,
        replace_on_exist: bool,
    ) -> Result<i64, MaskingPolicyError> {
        // 同名冲突：不同列报错；同列且允许 REPLACE 则保留原 ID/创建信息。
        if let Some(existing) = self
            .policies
            .values()
            .find(|existing| {
                existing.table_id == policy.table_id
                    && existing.name.eq_ignore_ascii_case(&policy.name)
            })
            .cloned()
        {
            if existing.column_id != policy.column_id {
                return Err(MaskingPolicyError::PolicyExistsOnAnotherColumn);
            }
            if !replace_on_exist {
                return Err(MaskingPolicyError::PolicyExists);
            }
            policy.id = existing.id;
            policy.created_at = existing.created_at;
            policy.created_by = existing.created_by;
            policy.state = SchemaState::Public;
            self.policies.insert(policy.id, policy);
            self.schema_version += 1;
            return Ok(existing.id);
        }
        self.next_id += 1;
        policy.id = self.next_id;
        policy.state = SchemaState::Public;
        self.policies.insert(policy.id, policy);
        self.schema_version += 1;
        Ok(self.next_id)
    }

    /// 修改已有策略的表达式、状态、类型与限制操作。
    pub fn alter(
        &mut self,
        policy_id: i64,
        changes: &MaskingPolicyInfo,
    ) -> Result<(), MaskingPolicyError> {
        let existing = self
            .policies
            .get_mut(&policy_id)
            .ok_or_else(|| MaskingPolicyError::PolicyNotFound(changes.name.clone()))?;
        existing.expression.clone_from(&changes.expression);
        existing.status = changes.status;
        existing.masking_type = changes.masking_type;
        existing.restrict_ops = changes.restrict_ops;
        existing.updated_at = changes.updated_at;
        self.schema_version += 1;
        Ok(())
    }

    /// 按 ID 删除策略；找不到时用策略名构造错误。
    pub fn drop(&mut self, policy_id: i64, name: &str) -> Result<(), MaskingPolicyError> {
        self.policies
            .remove(&policy_id)
            .ok_or_else(|| MaskingPolicyError::PolicyNotFound(name.into()))?;
        self.schema_version += 1;
        Ok(())
    }

    /// 按策略 ID 查找。
    pub fn by_id(&self, policy_id: i64) -> Option<&MaskingPolicyInfo> {
        self.policies.get(&policy_id)
    }

    /// 按策略名（忽略大小写）查找。
    pub fn by_name(&self, name: &str) -> Option<&MaskingPolicyInfo> {
        self.policies
            .values()
            .find(|policy| policy.name.eq_ignore_ascii_case(name))
    }

    /// 列出绑定到指定表的全部策略。
    pub fn by_table(&self, table_id: i64) -> Vec<&MaskingPolicyInfo> {
        self.policies
            .values()
            .filter(|policy| policy.table_id == table_id)
            .collect()
    }

    /// DROP TABLE 时清理该表上的策略。
    pub fn drop_on_table(&mut self, table_id: i64) {
        self.policies
            .retain(|_, policy| policy.table_id != table_id);
    }

    /// DROP DATABASE 时清理该库下全部策略。
    pub fn drop_by_database_name(&mut self, database_name: &str) {
        self.policies
            .retain(|_, policy| !policy.database_name.eq_ignore_ascii_case(database_name));
    }

    /// DROP COLUMN 时清理绑定到该列的策略。
    pub fn drop_on_column(&mut self, table_id: i64, column_id: i64) {
        self.policies
            .retain(|_, policy| policy.table_id != table_id || policy.column_id != column_id);
    }

    /// TRUNCATE TABLE 会换新 table_id；策略保留但需改写绑定的表 ID。
    pub fn update_table_id_after_truncate(
        &mut self,
        old_table_id: i64,
        new_table_id: i64,
        now: u64,
    ) {
        for policy in self
            .policies
            .values_mut()
            .filter(|policy| policy.table_id == old_table_id)
        {
            policy.table_id = new_table_id;
            policy.updated_at = now;
        }
    }

    /// RENAME TABLE（含跨库）后同步策略中的库名与表名。
    pub fn update_names_after_rename(
        &mut self,
        table_id: i64,
        database_name: &str,
        table_name: &str,
        now: u64,
    ) {
        for policy in self
            .policies
            .values_mut()
            .filter(|policy| policy.table_id == table_id)
        {
            policy.database_name = database_name.into();
            policy.table_name = table_name.into();
            policy.updated_at = now;
        }
    }

    /// 列修改/改名后：校验新列类型，并同步策略的列 ID/名与表达式中的标识符。
    pub fn sync_modified_column(
        &mut self,
        table: &TableInfo,
        old_column: &ColumnInfo,
        new_column: &ColumnInfo,
        now: u64,
    ) -> Result<(), MaskingPolicyError> {
        check_masking_policy_column(new_column)?;
        for policy in self.policies.values_mut().filter(|policy| {
            policy.table_id == table.id
                && (policy.column_id == old_column.id
                    || policy.column_name.eq_ignore_ascii_case(&old_column.name)
                    || policy.column_name.eq_ignore_ascii_case(&new_column.name))
        }) {
            // 列名变化时重写表达式里的标识符（保留反引号风格）。
            if !policy.column_name.eq_ignore_ascii_case(&new_column.name) {
                policy.expression = rewrite_masking_policy_expression_column_name(
                    &policy.expression,
                    &policy.column_name,
                    &new_column.name,
                )?;
            }
            policy.table_name.clone_from(&table.name);
            policy.column_id = new_column.id;
            policy.column_name.clone_from(&new_column.name);
            policy.updated_at = now;
        }
        Ok(())
    }
}

/// 将表达式中的旧列标识符（含可选反引号）替换为新列名。
///
/// 采用简易词法扫描：识别 `` `id` `` 或裸标识符，忽略大小写比较后替换。
pub fn rewrite_masking_policy_expression_column_name(
    expression: &str,
    old_column: &str,
    new_column: &str,
) -> Result<String, MaskingPolicyError> {
    if old_column.eq_ignore_ascii_case(new_column) {
        return Ok(expression.into());
    }
    if expression.trim().is_empty() {
        return Err(MaskingPolicyError::InvalidExpression);
    }
    let mut output = String::with_capacity(expression.len());
    let chars: Vec<char> = expression.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\'' || chars[index] == '"' {
            let quote = chars[index];
            output.push(quote);
            index += 1;
            let mut closed = false;
            while index < chars.len() {
                let current = chars[index];
                output.push(current);
                index += 1;
                if current == '\\' && index < chars.len() {
                    output.push(chars[index]);
                    index += 1;
                    continue;
                }
                if current == quote {
                    if index < chars.len() && chars[index] == quote {
                        output.push(chars[index]);
                        index += 1;
                    } else {
                        closed = true;
                        break;
                    }
                }
            }
            if !closed {
                return Err(MaskingPolicyError::InvalidExpression);
            }
            continue;
        }
        let quoted = chars[index] == '`';
        // 反引号标识符或以字母/_ 开头的裸标识符。
        if quoted || chars[index].is_ascii_alphabetic() || chars[index] == '_' {
            let start = index;
            if quoted {
                index += 1;
                while index < chars.len() && chars[index] != '`' {
                    index += 1;
                }
                if index == chars.len() {
                    return Err(MaskingPolicyError::InvalidExpression);
                }
                index += 1;
            } else {
                index += 1;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                {
                    index += 1;
                }
            }
            let token: String = chars[start..index].iter().collect();
            let identifier = token.trim_matches('`');
            if identifier.eq_ignore_ascii_case(old_column) {
                if quoted {
                    output.push('`');
                    output.push_str(new_column);
                    output.push('`');
                } else {
                    output.push_str(new_column);
                }
            } else {
                output.push_str(&token);
            }
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    Ok(output)
}

/// 解析 ENABLE/DISABLE（及 ENABLED/DISABLED）状态字符串。
pub fn masking_policy_status_from_string(
    status: &str,
) -> Result<MaskingPolicyStatus, MaskingPolicyError> {
    match status.trim().to_ascii_uppercase().as_str() {
        "ENABLE" | "ENABLED" => Ok(MaskingPolicyStatus::Enable),
        "DISABLE" | "DISABLED" => Ok(MaskingPolicyStatus::Disable),
        _ => Err(MaskingPolicyError::UnknownStatus(status.into())),
    }
}

/// 解析 FULL/PARTIAL/NULL/DATE 类型字符串，其余归为 Custom。
pub fn masking_policy_type_from_string(value: &str) -> MaskingPolicyType {
    match value.trim().to_ascii_uppercase().as_str() {
        "FULL" => MaskingPolicyType::Full,
        "PARTIAL" => MaskingPolicyType::Partial,
        "NULL" => MaskingPolicyType::Null,
        "DATE" => MaskingPolicyType::Date,
        _ => MaskingPolicyType::Custom,
    }
}

/// 将限制标志位集合序列化为逗号分隔字符串；空集输出 `NONE`。
pub fn masking_policy_restrict_ops_to_string(ops: MaskingPolicyRestrictOps) -> String {
    let mut values = Vec::new();
    for (flag, name) in [
        (
            MaskingPolicyRestrictOps::INSERT_INTO_SELECT,
            "INSERT_INTO_SELECT",
        ),
        (MaskingPolicyRestrictOps::UPDATE_SELECT, "UPDATE_SELECT"),
        (MaskingPolicyRestrictOps::DELETE_SELECT, "DELETE_SELECT"),
        (MaskingPolicyRestrictOps::CTAS, "CTAS"),
    ] {
        if ops.contains(flag) {
            values.push(name);
        }
    }
    if values.is_empty() {
        "NONE".into()
    } else {
        values.join(",")
    }
}

/// 从逗号分隔字符串解析限制标志；空串或 `NONE` 表示无限制。
pub fn masking_policy_restrict_ops_from_string(
    value: &str,
) -> Result<MaskingPolicyRestrictOps, MaskingPolicyError> {
    let value = value.trim().to_ascii_uppercase();
    if value.is_empty() || value == "NONE" {
        return Ok(MaskingPolicyRestrictOps::default());
    }
    let mut ops = MaskingPolicyRestrictOps::default();
    // 逐段解析已知标志名，未知名报错。
    for token in value.split(',').map(str::trim) {
        match token {
            "INSERT_INTO_SELECT" => ops.insert(MaskingPolicyRestrictOps::INSERT_INTO_SELECT),
            "UPDATE_SELECT" => ops.insert(MaskingPolicyRestrictOps::UPDATE_SELECT),
            "DELETE_SELECT" => ops.insert(MaskingPolicyRestrictOps::DELETE_SELECT),
            "CTAS" => ops.insert(MaskingPolicyRestrictOps::CTAS),
            "NONE" | "" => {}
            _ => return Err(MaskingPolicyError::UnknownRestrictOperation(token.into())),
        }
    }
    Ok(ops)
}
