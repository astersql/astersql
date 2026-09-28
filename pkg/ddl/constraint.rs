// Copyright 2026 AsterSQL.
// Copyright 2023-2023 PingCAP, Inc.
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

// CHECK 约束的 DDL（数据定义语言）状态机模块。
//
// 本模块实现表级 CHECK 约束的添加、删除与修改（ALTER）流程。
// 由于分布式数据库中 schema 变更需要在多个节点间逐步生效，DDL 采用
// 类似 Google F1 的"在线 schema 变更"协议：约束状态按
// `None -> WriteOnly -> WriteReorganization -> Public` 逐级推进，
// 每前进一步都会递增 schema 版本号，保证任意时刻集群内各节点
// 看到的 schema 至多相差一个版本，从而不阻塞在线读写。
//
// 主要内容：
// - [`ConstraintInfo`]：单个 CHECK 约束的元信息；
// - [`ConstraintTableState`]：某张表上全部约束的集合状态；
// - [`advance_add_check_constraint`] / [`advance_drop_check_constraint`] /
//   [`advance_alter_check_constraint`]：三种 DDL 操作的状态推进函数；
// - 列删除 / 重命名前的约束依赖检查辅助函数。

use std::collections::{BTreeSet, HashSet};

use crate::column::{SchemaState, TableInfo};
use crate::generated_column::{ExpressionNode, find_column_names_in_expr};

/// 约束标识符（名称）的最大长度，与 MySQL 的 64 字符标识符上限保持一致。
pub const MAX_CONSTRAINT_IDENTIFIER_LENGTH: usize = 64;

/// 单个 CHECK 约束的元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstraintInfo {
    /// 约束在表内的唯一 ID，由 [`ConstraintTableState::max_constraint_id`] 分配。
    pub id: i64,
    /// 约束名称，统一存为小写以支持大小写不敏感的比较。
    pub name: String,
    /// 约束所属表的表名。
    pub table_name: String,
    /// 约束表达式所依赖的列名列表（均为小写）。
    pub columns: Vec<String>,
    /// CHECK 约束表达式的文本形式。
    pub expression: String,
    /// 是否强制执行（对应 SQL 的 ENFORCED/NOT ENFORCED 子句）；
    /// 未强制的约束仅作为元数据保存，不校验数据。
    pub enforced: bool,
    /// 是否是定义在列上的约束（列级 CHECK），而非表级 CHECK。
    pub in_column: bool,
    /// 约束当前的 schema 状态（在线 schema 变更协议中的阶段）。
    pub state: SchemaState,
}

/// 一张表上全部 CHECK 约束的集合状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConstraintTableState {
    /// 已分配过的最大约束 ID，用于给新约束生成单调递增的 ID。
    pub max_constraint_id: i64,
    /// 表上当前存在的所有约束（含尚未进入 Public 状态的约束）。
    pub constraints: Vec<ConstraintInfo>,
}

/// 约束相关 DDL 操作可能产生的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConstraintError {
    /// 约束名超过最大标识符长度。
    NameTooLong(String),
    /// 同名约束已存在（且已公开可见）。
    ConstraintExists(String),
    /// 按名称查找约束失败。
    ConstraintNotFound(String),
    /// 约束表达式引用了表中不存在（或不可见）的列。
    UnknownColumn(String),
    /// 存量数据不满足约束表达式，约束校验失败。
    Violated(String),
    /// 约束处于状态机不允许的非法状态。
    InvalidState(SchemaState),
    /// 列被某个约束依赖，禁止删除或重命名该列。
    ColumnNeededByConstraint { constraint: String, column: String },
}

/// 校验约束名长度是否超过标识符上限（按小写形式的字节长度计算）。
pub fn check_constraint_name(name: &str) -> Result<(), ConstraintError> {
    if name.to_ascii_lowercase().len() > MAX_CONSTRAINT_IDENTIFIER_LENGTH {
        Err(ConstraintError::NameTooLong(name.into()))
    } else {
        Ok(())
    }
}

/// 提取约束表达式中引用的所有列名，返回去重且有序的集合。
pub fn find_dependent_columns(expression: &ExpressionNode) -> BTreeSet<String> {
    find_column_names_in_expr(expression).into_iter().collect()
}

/// 根据表信息与表达式构造一个新的 [`ConstraintInfo`]。
///
/// 名称与依赖列会统一转为小写存储；此处不分配约束 ID（保持为 0），
/// ID 由后续的 [`advance_add_check_constraint`] 在真正加入表状态时分配。
pub fn build_constraint_info(
    table: &TableInfo,
    name: &str,
    dependent_columns: impl IntoIterator<Item = String>,
    expression: &str,
    enforced: bool,
    in_column: bool,
    state: SchemaState,
) -> Result<ConstraintInfo, ConstraintError> {
    check_constraint_name(name)?;
    Ok(ConstraintInfo {
        id: 0,
        name: name.to_ascii_lowercase(),
        table_name: table.name.clone(),
        columns: dependent_columns
            .into_iter()
            .map(|column| column.to_ascii_lowercase())
            .collect(),
        expression: expression.into(),
        enforced,
        in_column,
        state,
    })
}

/// 为未命名的约束自动生成名称，格式为 `<表名小写>_chk_<序号>`。
///
/// 通过 `existing_names` 保证生成的名称不与已有名称冲突；
/// 序号从 1 开始递增，遇到冲突则继续尝试下一个序号。
pub fn set_names_for_constraints(
    table_lower_name: &str,
    existing_names: &mut HashSet<String>,
    constraints: &mut [ConstraintInfo],
) {
    let mut sequence = 1;
    let prefix = format!("{table_lower_name}_chk_");
    for constraint in constraints
        .iter_mut()
        .filter(|constraint| constraint.name.is_empty())
    {
        loop {
            let candidate = format!("{prefix}{sequence}");
            sequence += 1;
            if existing_names.insert(candidate.clone()) {
                constraint.name = candidate;
                break;
            }
        }
    }
}

/// 校验约束依赖的列在表中都存在且已处于 Public（对外可见）状态。
fn validate_dependencies(
    table: &TableInfo,
    constraint: &ConstraintInfo,
) -> Result<(), ConstraintError> {
    for name in &constraint.columns {
        if !table
            .columns
            .iter()
            .any(|column| column.name == *name && column.state == SchemaState::Public)
        {
            return Err(ConstraintError::UnknownColumn(name.clone()));
        }
    }
    Ok(())
}

/// 一次约束 DDL 状态推进后的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConstraintOutcome {
    /// 推进后约束所处的 schema 状态。
    pub schema_state: SchemaState,
    /// 推进后的 schema 版本号。
    pub schema_version: i64,
    /// DDL 是否已完成（约束进入终态）。
    pub finished: bool,
    /// 是否是因回滚而结束。
    pub rollback_done: bool,
}

/// 推进"添加 CHECK 约束"DDL 的状态机，每次调用前进一个阶段。
///
/// 正常路径：`None -> WriteOnly -> WriteReorganization -> Public`。
/// - WriteOnly：新写入的数据需满足约束，但存量数据尚未校验；
/// - WriteReorganization：后台扫描并校验存量数据（`remaining_records_valid`
///   表示该校验的结果）；
/// - Public：约束对所有请求完全生效。
///
/// 若 `rolling_back` 为真，则直接从表状态中移除该约束并结束。
/// 未强制（NOT ENFORCED）的约束跳过数据校验，直接置为 Public。
pub fn advance_add_check_constraint(
    table: &TableInfo,
    state: &mut ConstraintTableState,
    incoming: &mut ConstraintInfo,
    remaining_records_valid: bool,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<ConstraintOutcome, ConstraintError> {
    if rolling_back {
        // Go rollingBackAddConstraint only removes the constraint found by name.
        state
            .constraints
            .retain(|constraint| !constraint.name.eq_ignore_ascii_case(&incoming.name));
        *schema_version += 1;
        return Ok(ConstraintOutcome {
            schema_state: SchemaState::None,
            schema_version: *schema_version,
            finished: true,
            rollback_done: true,
        });
    }

    // 查找是否已存在同名约束：存在且已 Public 视为重复；
    // 存在但未 Public 说明是本 DDL 前几步插入的，继续推进其状态。
    let existing_offset = state
        .constraints
        .iter()
        .position(|constraint| constraint.name.eq_ignore_ascii_case(&incoming.name));
    let offset = if let Some(offset) = existing_offset {
        if state.constraints[offset].state == SchemaState::Public {
            return Err(ConstraintError::ConstraintExists(incoming.name.clone()));
        }
        offset
    } else {
        // 首次进入：校验依赖列、必要时自动命名、分配 ID 并加入表状态。
        validate_dependencies(table, incoming)?;
        let mut names: HashSet<String> = state
            .constraints
            .iter()
            .map(|constraint| constraint.name.clone())
            .collect();
        set_names_for_constraints(
            &table.name.to_ascii_lowercase(),
            &mut names,
            std::slice::from_mut(incoming),
        );
        state.max_constraint_id += 1;
        incoming.id = state.max_constraint_id;
        state.constraints.push(incoming.clone());
        state.constraints.len() - 1
    };

    let constraint = &mut state.constraints[offset];
    // 未强制的约束不校验存量数据，直接进入 Public 终态。
    if !constraint.enforced {
        constraint.state = SchemaState::Public;
        *schema_version += 1;
        return Ok(ConstraintOutcome {
            schema_state: SchemaState::Public,
            schema_version: *schema_version,
            finished: true,
            rollback_done: false,
        });
    }
    // 按在线 schema 变更协议逐级推进状态；
    // 只有存量数据校验通过后才能从 WriteReorganization 进入 Public。
    let next = match constraint.state {
        SchemaState::None => SchemaState::WriteOnly,
        SchemaState::WriteOnly => SchemaState::WriteReorganization,
        SchemaState::WriteReorganization => {
            if !remaining_records_valid {
                return Err(ConstraintError::Violated(constraint.name.clone()));
            }
            SchemaState::Public
        }
        state => return Err(ConstraintError::InvalidState(state)),
    };
    constraint.state = next;
    *schema_version += 1;
    Ok(ConstraintOutcome {
        schema_state: next,
        schema_version: *schema_version,
        finished: next == SchemaState::Public,
        rollback_done: false,
    })
}

/// 推进"删除 CHECK 约束"DDL 的状态机。
///
/// 删除按 `Public -> WriteOnly -> 移除` 两步走：先降级为 WriteOnly
/// （对读请求不再可见），再从表状态中真正移除，避免各节点 schema
/// 版本不一致时出现读写行为分歧。
pub fn advance_drop_check_constraint(
    state: &mut ConstraintTableState,
    name: &str,
    schema_version: &mut i64,
) -> Result<ConstraintOutcome, ConstraintError> {
    let offset = state
        .constraints
        .iter()
        .position(|constraint| constraint.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| ConstraintError::ConstraintNotFound(name.into()))?;
    match state.constraints[offset].state {
        // 第一步：Public 降级为 WriteOnly。
        SchemaState::Public => state.constraints[offset].state = SchemaState::WriteOnly,
        // 第二步：从表状态中彻底移除，DDL 完成。
        SchemaState::WriteOnly => {
            state.constraints.remove(offset);
            *schema_version += 1;
            return Ok(ConstraintOutcome {
                schema_state: SchemaState::None,
                schema_version: *schema_version,
                finished: true,
                rollback_done: false,
            });
        }
        invalid => return Err(ConstraintError::InvalidState(invalid)),
    }
    *schema_version += 1;
    Ok(ConstraintOutcome {
        schema_state: SchemaState::WriteOnly,
        schema_version: *schema_version,
        finished: false,
        rollback_done: false,
    })
}

/// 推进"修改 CHECK 约束"（ALTER，切换 ENFORCED 属性）DDL 的状态机。
///
/// - 关闭强制（NOT ENFORCED）：无需校验数据，一步到位回到 Public；
/// - 打开强制（ENFORCED）：需重新校验存量数据，路径为
///   `Public -> WriteReorganization -> WriteOnly -> Public`；
/// - `rolling_back` 为真时恢复原来的 enforced 取值并回到 Public。
pub fn advance_alter_check_constraint(
    state: &mut ConstraintTableState,
    name: &str,
    enforced: bool,
    remaining_records_valid: bool,
    schema_version: &mut i64,
    rolling_back: bool,
) -> Result<ConstraintOutcome, ConstraintError> {
    let constraint = state
        .constraints
        .iter_mut()
        .find(|constraint| constraint.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| ConstraintError::ConstraintNotFound(name.into()))?;
    if rolling_back {
        // 回滚：恢复为修改前的 enforced 值并直接回到 Public 状态。
        constraint.enforced = !enforced;
        constraint.state = SchemaState::Public;
        *schema_version += 1;
        return Ok(ConstraintOutcome {
            schema_state: SchemaState::Public,
            schema_version: *schema_version,
            finished: true,
            rollback_done: true,
        });
    }
    // 目标属性与当前一致且已是 Public，属于空操作，直接返回完成。
    if constraint.state == SchemaState::Public && constraint.enforced == enforced {
        return Ok(ConstraintOutcome {
            schema_state: SchemaState::Public,
            schema_version: *schema_version,
            finished: true,
            rollback_done: false,
        });
    }
    // 关闭强制无需校验任何数据，可一步完成。
    if !enforced {
        constraint.enforced = false;
        constraint.state = SchemaState::Public;
        *schema_version += 1;
        return Ok(ConstraintOutcome {
            schema_state: SchemaState::Public,
            schema_version: *schema_version,
            finished: true,
            rollback_done: false,
        });
    }
    // 打开强制：先进入 WriteReorganization 触发存量数据重扫，
    // 再经 WriteOnly 校验通过后回到 Public。
    let next = match constraint.state {
        SchemaState::Public => {
            constraint.enforced = true;
            SchemaState::WriteReorganization
        }
        SchemaState::WriteReorganization => SchemaState::WriteOnly,
        SchemaState::WriteOnly => {
            if !remaining_records_valid {
                return Err(ConstraintError::Violated(constraint.name.clone()));
            }
            SchemaState::Public
        }
        invalid => return Err(ConstraintError::InvalidState(invalid)),
    };
    constraint.state = next;
    *schema_version += 1;
    Ok(ConstraintOutcome {
        schema_state: next,
        schema_version: *schema_version,
        finished: next == SchemaState::Public,
        rollback_done: false,
    })
}

/// 检查删除某列是否会破坏多列 CHECK 约束。
///
/// 仅当该列被"依赖多个列"的约束引用时才拒绝删除；
/// 若约束只依赖这一列，删除列时可连同约束一起删除，故不报错。
pub fn ensure_column_droppable_with_check_constraint(
    column: &str,
    state: &ConstraintTableState,
) -> Result<(), ConstraintError> {
    for constraint in &state.constraints {
        if constraint.columns.len() > 1 && constraint.columns.iter().any(|name| name == column) {
            return Err(ConstraintError::ColumnNeededByConstraint {
                constraint: constraint.name.clone(),
                column: column.into(),
            });
        }
    }
    Ok(())
}

/// 检查重命名某列是否被 CHECK 约束阻止。
///
/// 与删除不同，重命名只要该列被任意约束引用即拒绝，
/// 因为约束表达式文本中记录的是旧列名，改名会使表达式失效。
pub fn ensure_column_renameable_with_check_constraint(
    column: &str,
    state: &ConstraintTableState,
) -> Result<(), ConstraintError> {
    if let Some(constraint) = state
        .constraints
        .iter()
        .find(|constraint| constraint.columns.iter().any(|name| name == column))
    {
        Err(ConstraintError::ColumnNeededByConstraint {
            constraint: constraint.name.clone(),
            column: column.into(),
        })
    } else {
        Ok(())
    }
}
