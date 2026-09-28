// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 单条 LabelConstraint（标签约束）的解析、还原与兼容性判断。
//
// LabelConstraint 描述 Store（存储节点）标签必须满足的条件，例如
// `+zone=bj` 表示副本只能落在 zone=bj 的节点，`-engine=tiflash` 表示
// 排除 TiFlash 引擎节点。约束最终交给 PD（Placement Driver）的
// placement rule 调度使用。
//
// 字符串格式：`{+|-}key=value[,value...]`，`+` 对应 In，`-` 对应 NotIn。

use crate::common::{EngineLabelKey, EngineLabelTiFlash};
use crate::errors::{ErrInvalidConstraintFormat, ErrUnsupportedConstraint, Error, wrap};
use crate::pd;

/// 从 `{+|-}key=value` 字符串解析出一条 [`pd::LabelConstraint`]。
///
/// 禁止 `+engine=tiflash`（TiFlash 副本由独立规则组管理，不能通过普通约束强制）。
pub fn NewConstraint(label: &str) -> Result<pd::LabelConstraint, Error> {
    // 最短合法形式至少 4 字符，例如 `+a=b`。
    if label.len() < 4 {
        return Err(wrap(ErrInvalidConstraintFormat, label));
    }
    // 首字符决定操作符：+ 为 In，- 为 NotIn。
    let op = match label.as_bytes()[0] {
        b'+' => pd::In,
        b'-' => pd::NotIn,
        _ => return Err(wrap(ErrInvalidConstraintFormat, label)),
    };
    let parts = label[1..].split('=').collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(wrap(ErrInvalidConstraintFormat, label));
    }
    let key = parts[0].trim();
    let value = parts[1].trim();
    if key.is_empty() || value.is_empty() {
        return Err(wrap(ErrInvalidConstraintFormat, label));
    }
    // TiFlash 引擎不能通过正向约束强制加入普通规则。
    if op == pd::In && key == EngineLabelKey && value.eq_ignore_ascii_case(EngineLabelTiFlash) {
        return Err(wrap(ErrUnsupportedConstraint, label));
    }
    Ok(pd::LabelConstraint {
        Key: key.to_owned(),
        Op: op,
        Values: value.split(',').map(str::to_owned).collect(),
    })
}

/// 直接用 key/op/values 构造约束，跳过字符串解析。
pub fn NewConstraintDirect(
    key: impl Into<String>,
    op: pd::LabelConstraintOp,
    values: Vec<String>,
) -> pd::LabelConstraint {
    pd::LabelConstraint {
        Key: key.into(),
        Op: op,
        Values: values,
    }
}

/// 将约束还原为 `{+|-}key=value` 字符串；仅支持单 value 的 In/NotIn。
pub fn RestoreConstraint(constraint: &pd::LabelConstraint) -> Result<String, Error> {
    if constraint.Values.len() != 1 {
        return Err(wrap(
            ErrInvalidConstraintFormat,
            format!(
                "constraint should have exactly one label value, got {:?}",
                constraint.Values
            ),
        ));
    }
    let prefix = match constraint.Op {
        pd::LabelConstraintOp::In => '+',
        pd::LabelConstraintOp::NotIn => '-',
        _ => {
            return Err(wrap(
                ErrInvalidConstraintFormat,
                format!("disallowed operation '{:?}'", constraint.Op),
            ));
        }
    };
    Ok(format!(
        "{prefix}{}={}",
        constraint.Key, constraint.Values[0]
    ))
}

/// 两条约束之间的兼容性结果编码。
pub type ConstraintCompatibility = u8;
/// 兼容：可并存于同一约束集合。
pub const ConstraintCompatible: ConstraintCompatibility = 0;
/// 不兼容：同 key 下操作或取值互相矛盾。
pub const ConstraintIncompatible: ConstraintCompatibility = 1;
/// 重复：同 key、同操作、同取值，追加时应跳过。
pub const ConstraintDuplicated: ConstraintCompatibility = 2;

/// 判断两条约束是否兼容、重复或不兼容。
///
/// 不同 key 恒为兼容；同 key 时按操作符与取值逐项比较。
pub fn ConstraintCompatibleWith(
    constraint: &pd::LabelConstraint,
    other: &pd::LabelConstraint,
) -> ConstraintCompatibility {
    if constraint.Key != other.Key {
        return ConstraintCompatible;
    }
    let same_op = constraint.Op == other.Op;
    let mut same_value = true;
    // 按索引对齐比较 Values；任一位不同则视为取值不同。
    for (index, value) in constraint.Values.iter().enumerate() {
        if index < other.Values.len() && value != &other.Values[index] {
            same_value = false;
            break;
        }
    }
    // 同操作同值 → 重复；反向操作同值，或同为 In 但取值不同 → 不兼容。
    if same_op && same_value {
        ConstraintDuplicated
    } else if (!same_op && same_value) || (same_op && !same_value && constraint.Op == pd::In) {
        ConstraintIncompatible
    } else {
        ConstraintCompatible
    }
}
