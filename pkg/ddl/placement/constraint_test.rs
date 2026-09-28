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

// 单条 LabelConstraint（标签约束）的单元测试。
//
// 覆盖字符串解析、还原（Restore）与兼容性判断（Compatible / Duplicated /
// Incompatible）。标签约束用于限定 Region（数据分片）副本可落在哪些
// 带特定 Store 标签的节点上；测试保持 Go 表驱动用例的覆盖范围。

use super::*;

fn assert_error_kind<T>(result: Result<T, Error>, kind: &str) {
    let error = result.err().expect("expected an error");
    assert!(
        error.to_string().starts_with(kind),
        "expected error kind {kind:?}, got {error}"
    );
}

#[test]
fn new_constraints_from_yaml_matches_go_cases() {
    assert!(NewConstraintsFromYaml(b"[]").is_ok());
    assert_error_kind(NewConstraintsFromYaml(b"]"), ErrInvalidConstraintsFormat);
}

#[test]
fn new_constraint_matches_go_table() {
    let valid = [
        (
            "normal",
            "+zone=bj",
            pd::LabelConstraint {
                Key: "zone".into(),
                Op: pd::In,
                Values: vec!["bj".into()],
            },
        ),
        (
            "normal with spaces",
            "-  dc  =  sh  ",
            pd::LabelConstraint {
                Key: "dc".into(),
                Op: pd::NotIn,
                Values: vec!["sh".into()],
            },
        ),
        (
            "not tiflash",
            "-engine  =  tiflash  ",
            pd::LabelConstraint {
                Key: "engine".into(),
                Op: pd::NotIn,
                Values: vec!["tiflash".into()],
            },
        ),
        (
            "not tiflash_compute",
            "-engine  =  tiflash_compute  ",
            pd::LabelConstraint {
                Key: "engine".into(),
                Op: pd::NotIn,
                Values: vec!["tiflash_compute".into()],
            },
        ),
    ];
    for (name, input, expected) in valid {
        assert_eq!(NewConstraint(input).unwrap(), expected, "{name}");
    }

    assert_error_kind(NewConstraint("+engine=Tiflash"), ErrUnsupportedConstraint);
    for (name, input) in [
        ("invalid length", ",,,"),
        ("invalid, lack = 1", "+    "),
        ("invalid, lack = 2", "+000"),
        ("invalid op", "0000"),
        ("empty key 1", "+ =zone1"),
        ("empty key 2", "+  =   z"),
        ("empty value 1", "+zone="),
        ("empty value 2", "+z  =   "),
    ] {
        let error = NewConstraint(input).unwrap_err();
        assert!(
            error.to_string().starts_with(ErrInvalidConstraintFormat),
            "{name}: {error}"
        );
    }
}

#[test]
fn restore_constraint_matches_go_table() {
    for (input, expected) in [
        ("+zone=bj", "+zone=bj"),
        ("+  zone = bj  ", "+zone=bj"),
        ("-  zone = bj  ", "-zone=bj"),
    ] {
        assert_eq!(
            RestoreConstraint(&NewConstraint(input).unwrap()).unwrap(),
            expected
        );
    }

    for constraint in [
        pd::LabelConstraint {
            Op: pd::In,
            Key: "dc".into(),
            Values: vec![],
        },
        pd::LabelConstraint {
            Op: pd::In,
            Key: "dc".into(),
            Values: vec!["dc1".into(), "dc2".into()],
        },
        pd::LabelConstraint {
            Op: pd::LabelConstraintOp::Unknown("[".into()),
            Key: "dc".into(),
            Values: vec![],
        },
    ] {
        assert_error_kind(RestoreConstraint(&constraint), ErrInvalidConstraintFormat);
    }
}

#[test]
fn constraint_compatibility_matches_go_table() {
    for (name, left, right, expected) in [
        ("case 2", "+zone=sh", "-zone=sh", ConstraintIncompatible),
        ("case 3", "+zone=bj", "+zone=sh", ConstraintIncompatible),
        ("case 1", "+zone=sh", "+zone=sh", ConstraintDuplicated),
        ("normal 1", "+zone=sh", "+dc=sh", ConstraintCompatible),
        ("normal 2", "-zone=sh", "-zone=bj", ConstraintCompatible),
    ] {
        assert_eq!(
            ConstraintCompatibleWith(
                &NewConstraint(left).unwrap(),
                &NewConstraint(right).unwrap()
            ),
            expected,
            "{name}"
        );
    }
}
