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

// 标签约束集合（Constraints）的构造、合并、还原与指纹计算。
//
// 一组 [`pd::LabelConstraint`] 共同描述副本可落点的 Store 标签条件。
// 追加新约束时会做兼容性检查：重复则跳过，冲突则报错。
// 指纹（fingerprint）用于对约束集合做规范化哈希，便于比较或去重。

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::constraint::{
    ConstraintCompatible, ConstraintCompatibleWith, ConstraintDuplicated, NewConstraint,
    RestoreConstraint,
};
use crate::errors::{
    ErrConflictingConstraints, ErrInvalidConstraintsFormat, ErrUnsupportedConstraint, Error, wrap,
};
use crate::pd;
use crate::rule::{attributeEvictLeader, attributePrefix};

/// 由若干 `{+|-}key=value` 字符串解析并合并为一组无冲突的约束。
pub fn NewConstraints(labels: Vec<String>) -> Result<Vec<pd::LabelConstraint>, Error> {
    let mut constraints = Vec::with_capacity(labels.len());
    for label in labels {
        let parsed = NewConstraint(label.trim())?;
        AddConstraint(&mut constraints, parsed)?;
    }
    Ok(constraints)
}

/// 预处理字典形式的约束字符串：剥离 `#` 属性前缀项，并可能改写 Peer 角色。
///
/// 例如 `#evict-leader` 在原角色为 Voter 时会把角色降为 Follower
/// （驱逐 Leader，让该规则不产生 Leader 副本）。
pub(crate) fn preCheckDictConstraintStr(
    label_string: &str,
    role: pd::PeerRoleType,
) -> Result<(Vec<String>, pd::PeerRoleType), Error> {
    let mut override_role = role.clone();
    let mut labels = Vec::new();
    for label in label_string.split(',') {
        // 以 `#` 开头的是属性而非标签约束。
        if let Some(attribute) = label.strip_prefix(attributePrefix) {
            if attribute == attributeEvictLeader {
                if role == pd::Voter {
                    override_role = pd::Follower;
                }
            } else {
                return Err(wrap(
                    ErrUnsupportedConstraint,
                    format!("unsupported attribute '{label}'"),
                ));
            }
        } else {
            labels.push(label.to_owned());
        }
    }
    Ok((labels, override_role))
}

/// 从 YAML 字节流解析约束列表；全空白输入视为空约束集合。
pub fn NewConstraintsFromYaml(bytes: &[u8]) -> Result<Vec<pd::LabelConstraint>, Error> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let labels: Option<Vec<String>> =
        serde_yaml::from_slice(bytes).map_err(|_| Error::new(ErrInvalidConstraintsFormat))?;
    NewConstraints(labels.unwrap_or_default())
}

/// 直接包装已有约束列表（透传，不做校验）。
pub fn NewConstraintsDirect(constraints: Vec<pd::LabelConstraint>) -> Vec<pd::LabelConstraint> {
    constraints
}

/// 将约束集合还原为逗号分隔的带引号字符串，如 `"+zone=bj","-dc=sh"`。
pub fn RestoreConstraints(constraints: &[pd::LabelConstraint]) -> Result<String, Error> {
    constraints
        .iter()
        .map(|constraint| RestoreConstraint(constraint).map(|value| format!("\"{value}\"")))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| values.join(","))
}

/// 向约束集合追加一条约束：重复则跳过，冲突则返回错误。
pub fn AddConstraint(
    constraints: &mut Vec<pd::LabelConstraint>,
    label: pd::LabelConstraint,
) -> Result<(), Error> {
    let mut should_add = true;
    for constraint in constraints.iter() {
        match ConstraintCompatibleWith(&label, constraint) {
            ConstraintCompatible => {}
            ConstraintDuplicated => should_add = false,
            _ => {
                // 还原为可读字符串，便于错误信息定位冲突双方。
                let left = RestoreConstraint(&label).unwrap_or_else(|error| error.to_string());
                let right = RestoreConstraint(constraint).unwrap_or_else(|error| error.to_string());
                return Err(wrap(
                    ErrConflictingConstraints,
                    format!("'{left}' and '{right}'"),
                ));
            }
        }
    }
    if should_add {
        constraints.push(label);
    }
    Ok(())
}

/// 计算约束集合的规范化指纹：先排序再 SHA-256，结果以 Base64 编码。
pub fn ConstraintsFingerPrint(constraints: &[pd::LabelConstraint]) -> String {
    let mut constraints = constraints.to_vec();
    // 排序保证相同集合无论输入顺序都得到同一指纹。
    constraints.sort_by_key(constraintToString);
    let combined = constraints
        .iter()
        .map(constraintToString)
        .collect::<String>();
    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(combined.as_bytes()))
}

/// 把单条约束序列化为 `key|op|sorted_values`，供排序与哈希使用。
fn constraintToString(constraint: &pd::LabelConstraint) -> String {
    let mut values = constraint.Values.clone();
    values.sort();
    let operation = match &constraint.Op {
        pd::LabelConstraintOp::Empty => "",
        pd::LabelConstraintOp::In => "in",
        pd::LabelConstraintOp::NotIn => "notIn",
        pd::LabelConstraintOp::Exists => "exists",
        pd::LabelConstraintOp::NotExists => "notExists",
        pd::LabelConstraintOp::Unknown(value) => value.as_str(),
    };
    format!("{}|{}|{}", constraint.Key, operation, values.join(","))
}
