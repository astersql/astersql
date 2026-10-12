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

// 标签约束集合（Constraints）的单元测试。
//
// 覆盖成组构造时的去重、冲突检测、字符串还原与指纹（fingerprint）计算。
// 约束集合描述一组可同时生效的 Store 标签条件，冲突则拒绝追加。

use super::*;

/// 校验约束集合去重、还原格式、冲突拒绝，以及指纹非空。
fn assert_error_kind<T>(result: Result<T, Error>, kind: &str) {
    let error = result.err().expect("expected an error");
    assert!(
        error.to_string().starts_with(kind),
        "expected error kind {kind:?}, got {error}"
    );
}

fn unknown_constraint(op: &str, key: &str) -> pd::LabelConstraint {
    pd::LabelConstraint {
        Op: pd::LabelConstraintOp::Unknown(op.into()),
        Key: key.into(),
        Values: vec![],
    }
}

#[test]
fn new_constraints_matches_go_cases() {
    assert_eq!(NewConstraints(vec![]).unwrap(), vec![]);
    assert_error_kind(
        NewConstraints(vec!["+zonesh".into()]),
        ErrInvalidConstraintFormat,
    );
    assert_error_kind(
        NewConstraints(vec!["+zone=sh".into(), "-zone=sh".into()]),
        ErrConflictingConstraints,
    );
}

#[test]
fn yaml_constraints_and_dictionary_attributes_match_go() {
    assert_eq!(NewConstraintsFromYaml(b"").unwrap(), vec![]);
    assert_eq!(NewConstraintsFromYaml(b"   \n\t").unwrap(), vec![]);
    assert_eq!(
        NewConstraintsFromYaml(br#"[" +zone=sh ", "-rack=r1"]"#).unwrap(),
        vec![
            NewConstraint("+zone=sh").unwrap(),
            NewConstraint("-rack=r1").unwrap(),
        ]
    );
    assert_error_kind(
        NewConstraintsFromYaml(br#"{zone: sh}"#),
        ErrInvalidConstraintsFormat,
    );

    assert_eq!(
        preCheckDictConstraintStr("+zone=sh,-rack=r1", pd::Voter).unwrap(),
        (vec!["+zone=sh".into(), "-rack=r1".into()], pd::Voter)
    );
    assert_eq!(
        preCheckDictConstraintStr("#evict-leader,+zone=sh", pd::Voter).unwrap(),
        (vec!["+zone=sh".into()], pd::Follower)
    );
    assert_eq!(
        preCheckDictConstraintStr("#evict-leader,+zone=sh", pd::Learner).unwrap(),
        (vec!["+zone=sh".into()], pd::Learner)
    );
    assert_error_kind(
        preCheckDictConstraintStr("+zone=sh,#unknown", pd::Voter),
        ErrUnsupportedConstraint,
    );
}

#[test]
fn add_constraint_matches_go_table() {
    let label_in = NewConstraint("+zone=sh").unwrap();
    let label_not_in = NewConstraint("-zone=sh").unwrap();

    let mut labels = vec![label_in.clone()];
    assert_error_kind(
        AddConstraint(&mut labels, label_not_in.clone()),
        ErrConflictingConstraints,
    );

    let mut labels = vec![label_in.clone()];
    AddConstraint(&mut labels, label_in.clone()).unwrap();
    assert_eq!(labels, vec![label_in.clone()]);

    let mut labels = vec![label_in.clone(), label_not_in];
    assert_error_kind(
        AddConstraint(&mut labels, label_in.clone()),
        ErrConflictingConstraints,
    );

    let mut labels = vec![label_in.clone()];
    let invalid_operand = unknown_constraint("[", "");
    AddConstraint(&mut labels, invalid_operand.clone()).unwrap();
    assert_eq!(labels.last(), Some(&invalid_operand));

    let mut labels = vec![unknown_constraint("[", "")];
    AddConstraint(&mut labels, label_in.clone()).unwrap();
    assert_eq!(labels.last(), Some(&label_in));

    let mut labels = vec![unknown_constraint("[", "dc")];
    assert_error_kind(
        AddConstraint(&mut labels, unknown_constraint("]", "dc")),
        ErrConflictingConstraints,
    );

    let mut labels = vec![label_in];
    let compatible = NewConstraint("-zone=bj").unwrap();
    AddConstraint(&mut labels, compatible.clone()).unwrap();
    assert_eq!(labels.last(), Some(&compatible));
}

#[test]
fn restore_constraints_matches_go_table() {
    assert_eq!(RestoreConstraints(&[]).unwrap(), "");
    let constraints = vec![
        NewConstraint("+zone=bj").unwrap(),
        NewConstraint("-zone=sh").unwrap(),
    ];
    assert_eq!(
        RestoreConstraints(&constraints).unwrap(),
        r#""+zone=bj","-zone=sh""#
    );
    assert_error_kind(
        RestoreConstraints(&[pd::LabelConstraint {
            Op: pd::LabelConstraintOp::Unknown("[".into()),
            Key: "dc".into(),
            Values: vec!["dc1".into()],
        }]),
        ErrInvalidConstraintFormat,
    );
}

#[test]
fn constraints_fingerprint_is_order_and_value_order_independent() {
    let first = vec![
        NewConstraintDirect("zone", pd::In, vec!["sh".into(), "bj".into()]),
        NewConstraint("-rack=r1").unwrap(),
    ];
    let second = vec![
        NewConstraint("-rack=r1").unwrap(),
        NewConstraintDirect("zone", pd::In, vec!["bj".into(), "sh".into()]),
    ];
    assert_eq!(
        ConstraintsFingerPrint(&first),
        ConstraintsFingerPrint(&second)
    );
}
