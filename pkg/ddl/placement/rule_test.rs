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

// Placement Rule 构建与解析的单元测试。
//
// 覆盖 `RuleBuilder` 在字典约束下的副本数汇总、以及 `#evict-leader` 属性将角色降为
// Follower（跟随者副本，不参与 Leader 选举）的行为。

use super::*;

/// 验证字典约束下副本数汇总为 3，且 `#evict-leader` 使对应规则角色变为 Follower。
fn direct(key: &str, op: pd::LabelConstraintOp, values: &[&str]) -> pd::LabelConstraint {
    NewConstraintDirect(
        key,
        op,
        values.iter().map(|value| (*value).to_owned()).collect(),
    )
}

fn assert_rule_sets(mut actual: Vec<Box<pd::Rule>>, mut expected: Vec<Box<pd::Rule>>, name: &str) {
    let canonical = |rule: &Box<pd::Rule>| {
        (
            format!("{:?}", rule.Role),
            rule.Count,
            format!("{:?}", rule.LabelConstraints),
        )
    };
    actual.sort_by_key(&canonical);
    expected.sort_by_key(&canonical);
    assert_eq!(actual, expected, "{name}");
}

fn build(role: pd::PeerRoleType, replicas: u64, input: &str) -> Result<Vec<Box<pd::Rule>>, Error> {
    let mut builder = NewRuleBuilder();
    builder
        .SetRole(role)
        .SetReplicasNum(replicas)
        .SetConstraintStr(input.into())
        .BuildRules()
}

#[test]
fn rule_clone_is_independent() {
    let original = pd::Rule {
        ID: "434".into(),
        ..Default::default()
    };
    let mut cloned = original.clone();
    cloned.ID = "121".into();
    assert_eq!(original.ID, "434");
    assert_eq!(cloned.ID, "121");
}

#[test]
fn new_rule_and_new_rules_match_go_table() {
    let cases = vec![
        (
            "empty constraints",
            "",
            3,
            Some(vec![NewRule(pd::Voter, 3, vec![])]),
            None,
        ),
        ("zero replicas", "", 0, Some(vec![]), None),
        (
            "normal list constraints",
            r#"["+zone=sh", "+region=sh"]"#,
            3,
            Some(vec![NewRule(
                pd::Voter,
                3,
                vec![
                    direct("zone", pd::In, &["sh"]),
                    direct("region", pd::In, &["sh"]),
                ],
            )]),
            None,
        ),
        (
            "normal dict constraints",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh": 1}"#,
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Voter, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "normal dict constraints, with count",
            "{'+zone=sh,-zone=bj':2, '+zone=sh': 1}",
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Voter, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "zero count in dict constraints",
            r#"{"+zone=sh,-zone=bj":0, "+zone=sh": 1}"#,
            0,
            None,
            Some(ErrInvalidConstraintsMapcnt),
        ),
        (
            "invalid list constraints",
            r#"["ne=sh", "+zone=sh"]"#,
            3,
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "invalid dict syntax",
            r#"{+ne=sh,-zone=bj:1, "+zone=sh": 4"#,
            0,
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "invalid constraint in dict",
            r#"{"nesh,-zone=bj":1, "+zone=sh": 4}"#,
            0,
            None,
            Some(ErrInvalidConstraintFormat),
        ),
        (
            "invalid dict separator",
            "{+region=us-east-2:2}",
            0,
            None,
            Some(ErrInvalidConstraintsMappingWrongSeparator),
        ),
        (
            "evict leader attribute",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,#evict-leader": 1}"#,
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Follower, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "invalid attribute format",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,evict-leader": 1}"#,
            0,
            None,
            Some(ErrInvalidConstraintFormat),
        ),
        (
            "unsupported attribute",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,#reject-follower": 1}"#,
            0,
            None,
            Some(ErrUnsupportedConstraint),
        ),
    ];

    for (name, input, replicas, expected, error_kind) in cases {
        match (build(pd::Voter, replicas, input), expected, error_kind) {
            (Ok(actual), Some(expected), None) => assert_rule_sets(actual, expected, name),
            (Err(error), None, Some(kind)) => assert!(
                error.to_string().starts_with(kind),
                "{name}: expected {kind:?}, got {error}"
            ),
            (actual, expected, error) => {
                panic!(
                    "{name}: unexpected result {actual:?}, expected {expected:?}, error {error:?}"
                )
            }
        }
    }
}
