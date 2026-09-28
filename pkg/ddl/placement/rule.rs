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

// Placement Rule（放置规则）构建模块。
//
// 将 SQL/DDL 侧声明的副本角色、副本数与 label 约束字符串，转换为 PD（Placement Driver，
// 集群调度中心）可识别的 `Rule` 列表。约束支持 YAML 数组形式 `[constraint, ...]`，
// 以及字典形式 `{constraint: count, ...}`（按约束分组指定副本数）。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::constraints::{NewConstraints, NewConstraintsFromYaml, preCheckDictConstraintStr};
use crate::errors::{
    ErrInvalidConstraintsFormat, ErrInvalidConstraintsMapcnt,
    ErrInvalidConstraintsMappingNoColonFound, ErrInvalidConstraintsMappingWrongSeparator,
    ErrInvalidConstraintsReplicas, Error, wrap,
};
use crate::pd;

/// label 属性前缀；以 `#` 开头表示特殊属性而非普通 label 约束。
pub const attributePrefix: &str = "#";
/// 驱逐 Leader 属性名；用于指示副本不应担任 Region（数据分片）的 Leader。
pub const attributeEvictLeader: &str = "evict-leader";

/// 放置规则的流式构建器，聚合角色、副本数与约束后再生成 `pd::Rule`。
#[derive(Clone, Debug, Default)]
pub struct RuleBuilder {
    /// Peer 角色（如 voter / learner）。
    role: pd::PeerRoleType,
    /// 期望副本总数；为 0 时表示由字典约束自行决定各分组副本数。
    replicasNum: u64,
    /// 为 true 时跳过「显式副本数与规则 Count 之和」一致性校验。
    skipCheckReplicasConsistent: bool,
    /// 原始约束字符串（YAML 数组或字典）。
    constraintStr: String,
}

/// 创建默认配置的 `RuleBuilder`。
pub fn NewRuleBuilder() -> RuleBuilder {
    RuleBuilder::default()
}

impl RuleBuilder {
    /// 设置 Peer 角色。
    pub fn SetRole(&mut self, role: pd::PeerRoleType) -> &mut Self {
        self.role = role;
        self
    }

    /// 设置期望副本总数。
    pub fn SetReplicasNum(&mut self, count: u64) -> &mut Self {
        self.replicasNum = count;
        self
    }

    /// 设置是否跳过副本数一致性校验。
    pub fn SetSkipCheckReplicasConsistent(&mut self, skip: bool) -> &mut Self {
        self.skipCheckReplicasConsistent = skip;
        self
    }

    /// 设置约束字符串。
    pub fn SetConstraintStr(&mut self, constraints: String) -> &mut Self {
        self.constraintStr = constraints;
        self
    }

    /// 仅按字典约束解析并生成规则，忽略数组形式与显式副本数。
    pub fn BuildRulesWithDictConstraintsOnly(&self) -> Result<Vec<Box<pd::Rule>>, Error> {
        newRulesWithDictConstraints(self.role.clone(), &self.constraintStr)
    }

    /// 解析约束并生成规则；可选校验显式副本数与各规则 Count 之和是否一致。
    pub fn BuildRules(&self) -> Result<Vec<Box<pd::Rule>>, Error> {
        let rules = newRules(self.role.clone(), self.replicasNum, &self.constraintStr)?;
        // 显式指定了副本数时，要求与字典约束展开后的 Count 总和一致。
        if !self.skipCheckReplicasConsistent {
            let total: i32 = rules.iter().map(|rule| rule.Count).sum();
            if self.replicasNum != 0 && self.replicasNum != total as u64 {
                return Err(wrap(
                    ErrInvalidConstraintsReplicas,
                    format!(
                        "count of replicas in dict constrains is {total}, but got {}",
                        self.replicasNum
                    ),
                ));
            }
        }
        Ok(rules)
    }
}

/// 根据角色、副本数与已解析的 label 约束构造单条 PD 放置规则。
pub fn NewRule(
    role: pd::PeerRoleType,
    replicas: u64,
    constraints: Vec<pd::LabelConstraint>,
) -> Box<pd::Rule> {
    Box::new(pd::Rule {
        Role: role,
        Count: replicas as i32,
        LabelConstraints: constraints,
        ..Default::default()
    })
}

/// 匹配 YAML 映射中冒号后缺少空格的错误写法（如 `key:1`），用于提前分类错误类型。
static wrongSeparatorRegexp: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[^"':]+:\d"#).expect("constant regex is valid"));

fn is_complete_mapping_with_wrong_separator(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('{') && value.ends_with('}') && wrongSeparatorRegexp.is_match(value)
}

/// 根据约束字符串形态，推断 YAML 映射格式错误（缺冒号或分隔符错误）。
fn getYamlMapFormatError(value: &str) -> Option<Error> {
    if !value.contains(':') {
        Some(Error::new(ErrInvalidConstraintsMappingNoColonFound))
    } else if wrongSeparatorRegexp.is_match(value) {
        Some(Error::new(ErrInvalidConstraintsMappingWrongSeparator))
    } else {
        None
    }
}

/// 优先按 YAML 数组解析约束；失败则回退到字典形式。
fn newRules(
    role: pd::PeerRoleType,
    replicas: u64,
    constraint_string: &str,
) -> Result<Vec<Box<pd::Rule>>, Error> {
    if is_complete_mapping_with_wrong_separator(constraint_string) {
        return Err(Error::new(ErrInvalidConstraintsMappingWrongSeparator));
    }
    match NewConstraintsFromYaml(constraint_string.as_bytes()) {
        Ok(constraints) => {
            // 副本数为 0：仅允许空约束，否则报副本数非法。
            if replicas == 0 {
                if !constraint_string.is_empty() {
                    return Err(wrap(
                        ErrInvalidConstraintsReplicas,
                        format!(
                            "count of replicas should be positive, but got {replicas}, constraint {constraint_string}"
                        ),
                    ));
                }
                return Ok(Vec::new());
            }
            Ok(vec![NewRule(role, replicas, constraints)])
        }
        Err(array_error) => {
            // 数组与字典均无法解析时，合并两边错误信息后返回格式错误。
            if let Err(map_error) = serde_yaml::from_str::<HashMap<String, i32>>(constraint_string)
            {
                return Err(wrap(
                    ErrInvalidConstraintsFormat,
                    format!(
                        "should be [constraint1, ...] (error {array_error}), {{constraint1: cnt1, ...}} (error {map_error}), or any yaml compatible representation"
                    ),
                ));
            }
            newRulesWithDictConstraints(role, constraint_string)
        }
    }
}

/// 将 `{label约束: 副本数}` 字典展开为多条放置规则。
fn newRulesWithDictConstraints(
    role: pd::PeerRoleType,
    constraint_string: &str,
) -> Result<Vec<Box<pd::Rule>>, Error> {
    // yaml.v2 decodes this malformed mapping far enough for Go to classify
    // the missing space; serde_yaml rejects it earlier, so classify it first.
    // 与 Go 侧 yaml.v2 行为对齐：先识别「冒号后缺空格」再交给 serde_yaml。
    if is_complete_mapping_with_wrong_separator(constraint_string) {
        return Err(Error::new(ErrInvalidConstraintsMappingWrongSeparator));
    }
    let constraints: HashMap<String, i32> = serde_yaml::from_str(constraint_string).map_err(|error| {
        wrap(
            ErrInvalidConstraintsFormat,
            format!(
                "should be [constraint1, ...] or {{constraint1: cnt1, ...}}, error {error}, or any yaml compatible representation"
            ),
        )
    })?;
    // 校验每个约束对应的副本数必须为正。
    for (labels, count) in &constraints {
        if *count <= 0 {
            if let Some(error) = getYamlMapFormatError(constraint_string) {
                return Err(error);
            }
            return Err(wrap(
                ErrInvalidConstraintsMapcnt,
                format!("count of labels '{labels}' should be positive, but got {count}"),
            ));
        }
    }
    let mut rules = Vec::with_capacity(constraints.len());
    // 预处理字典键中的特殊属性，再生成 label 约束与规则。
    for (labels, count) in constraints {
        let (labels, override_role) = preCheckDictConstraintStr(&labels, role.clone())?;
        let constraints = NewConstraints(labels)?;
        rules.push(NewRule(override_role, count as u64, constraints));
    }
    Ok(rules)
}
