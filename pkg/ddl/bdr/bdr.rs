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

// BDR（Bidirectional Replication，双向复制）场景下的 DDL 拦截规则。
//
// BDR 指两个集群互为主备并双向同步数据的部署形态。为避免双向同步时
// DDL（数据定义语言，如建表、加列等修改表结构的语句）在两侧产生
// 冲突或数据不一致，需要根据节点扮演的 BDR 角色（Primary 主角色 /
// Secondary 从角色 / None 未设置）限制可以执行的 DDL 类型。
// 本模块提供一组判定函数，回答“某个 DDL 在当前 BDR 角色下是否应被拒绝”。

use crate::{ast, model, types};
use std::any::Any;

// IsAddColumnDenied checks whether BDR should reject an
// add-column operation with the given column options. It returns false for
// any role other than ast.BDRRolePrimary without evaluating the options. It
// allows adding a nullable column, or a not-null column with a default value.
/// 判断在 BDR 场景下是否应拒绝“加列”（ADD COLUMN）操作。
///
/// 仅当角色为 Primary（主角色）时才检查列选项；其他角色直接放行。
/// 允许的加列形式：可空列，或“NOT NULL + 默认值”的列——这两种形式
/// 不会导致从集群回放历史数据时因缺少列值而失败。
pub fn IsAddColumnDenied(role: ast::BDRRole, options: &[ast::ColumnOption]) -> bool {
    if role != ast::BDRRole::Primary {
        return false;
    }

    // 扫描列选项，记录是否出现可空、非空、默认值等关键属性；
    // COMMENT 与生成列选项不影响数据兼容性，单独计数以便后面从总数中扣除。
    let mut nullable = false;
    let mut notNull = false;
    let mut defaultValue = false;
    let mut comment = 0;
    let mut generated = 0;
    for option in options {
        match option.Tp {
            ast::ColumnOptionType::DefaultValue => defaultValue = true,
            ast::ColumnOptionType::Comment => comment = 1,
            ast::ColumnOptionType::Generated => generated = 1,
            ast::ColumnOptionType::NotNull => notNull = true,
            ast::ColumnOptionType::Null => nullable = true,
            _ => {}
        }
    }
    // tpLen 为剔除 COMMENT / 生成列后的有效选项个数。
    let tpLen = options.len() - comment - generated;

    // 允许的组合：无有效选项（默认可空）、单独 NULL、
    // 单独 DEFAULT（且未声明 NOT NULL）、NOT NULL + DEFAULT 成对出现。
    if tpLen == 0
        || (tpLen == 1 && nullable)
        || (tpLen == 1 && !notNull && defaultValue)
        || (tpLen == 2 && notNull && defaultValue)
    {
        return false;
    }

    true
}

// IsModifyColumnDenied checks whether BDR should reject a
// modify-column operation with the given field types and column options. It
// returns false for any role other than ast.BDRRolePrimary without evaluating
// the options. It allows changing the default value, or changing the default
// value together with the column comment, when the field type is unchanged.
/// 判断在 BDR 场景下是否应拒绝“改列”（MODIFY COLUMN）操作。
///
/// 仅当角色为 Primary 时才检查；其他角色直接放行。要求字段类型
/// （FieldType，描述列的数据类型、长度、字符集等元信息）保持不变，
/// 且改动仅限于默认值，或默认值加列注释这两种安全组合。
pub fn IsModifyColumnDenied(
    role: ast::BDRRole,
    newFieldType: &types::FieldType,
    oldFieldType: &types::FieldType,
    options: &[ast::ColumnOption],
) -> bool {
    if role != ast::BDRRole::Primary {
        return false;
    }

    // 字段类型发生变化（如改变数据类型或长度）会影响两侧数据兼容性，直接拒绝。
    if !newFieldType.Equal(oldFieldType) {
        return true;
    }

    let mut defaultValue = false;
    let mut comment = false;
    for option in options {
        if option.Tp == ast::ColumnOptionType::DefaultValue {
            defaultValue = true;
        }
        if option.Tp == ast::ColumnOptionType::Comment {
            comment = true;
        }
    }

    // 只修改默认值：安全，放行。
    if options.len() == 1 && defaultValue {
        return false;
    }

    // 同时修改默认值和列注释：也安全，放行。
    if options.len() == 2 && defaultValue && comment {
        return false;
    }

    true
}

// IsDenied checks whether the DDL is denied by BDR.
/// 判断给定 DDL 动作在当前 BDR 角色下是否被拒绝。
///
/// 依据 `model::ActionBDRMap`（DDL 动作到 BDR 安全级别的映射表）分类：
/// - Primary 角色：仅放行 SafeDDL（安全 DDL）与 UnmanagementDDL（不受 BDR 管控的 DDL）；
///   此外加索引/加主键若带唯一约束会被特殊拦截。
/// - Secondary 角色：仅放行 UnmanagementDDL。
/// - None 角色：未启用 BDR，不拦截任何 DDL。
/// 未在映射表中登记的动作一律视为不安全而拒绝。
pub fn IsDenied(role: ast::BDRRole, action: model::ActionType, args: Option<&dyn Any>) -> bool {
    match role {
        ast::BDRRole::Primary => {
            let Some(&ddlType) = model::ActionBDRMap.get(&action) else {
                return true;
            };

            // Can't add unique index on primary role.
            // 主角色不允许新增唯一索引：从集群回放旧数据时可能违反新加的唯一约束。
            // args 以 `dyn Any`（运行时动态类型）传入，此处向下转型为具体的索引参数。
            if let Some(args) = args
                && (action == model::ACTION_ADD_INDEX || action == model::ACTION_ADD_PRIMARY_KEY)
            {
                let args = args
                    .downcast_ref::<model::ModifyIndexArgs>()
                    .expect("add-index BDR arguments must be ModifyIndexArgs");
                if args.IndexArgs[0].Unique {
                    return true;
                }
            }

            if ddlType == model::SafeDDL || ddlType == model::UnmanagementDDL {
                return false;
            }
        }
        ast::BDRRole::Secondary => {
            let Some(&ddlType) = model::ActionBDRMap.get(&action) else {
                return true;
            };
            if ddlType == model::UnmanagementDDL {
                return false;
            }
        }
        ast::BDRRole::None | ast::BDRRole::Unknown => {
            // Go's string-backed role permits unknown values. Any role other
            // than primary/secondary bypasses BDR denial, like an unset role.
            // 未设置或当前版本未知的角色均不拒绝 DDL。
            return false;
        }
    }

    true
}
