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

// Bundle（规则组）相关单元测试。
//
// Bundle 是交给 PD（Placement Driver）的一组放置规则（Rule）的容器，
// 对应表或分区的 Placement Policy。本文件提供可运行的 Rust 回归：
// 校验 Bundle 创建、Reset（按对象 ID 重置
// key 范围）、ObjectID 解析与 JSON 序列化的规范化行为。

use super::*;

/// 校验 Bundle 经 Reset 后：ObjectID、按表 ID 展开的多段 Rule key 范围、以及 JSON 中的 group_id 均规范正确。
fn constraint(key: &str, op: pd::LabelConstraintOp, values: &[&str]) -> pd::LabelConstraint {
    NewConstraintDirect(
        key,
        op,
        values.iter().map(|value| (*value).to_owned()).collect(),
    )
}

fn rule(role: pd::PeerRoleType, count: u64, constraints: Vec<pd::LabelConstraint>) -> pd::Rule {
    *NewRule(role, count, constraints)
}

fn assert_error_kind<T>(result: Result<T, Error>, kind: &str, name: &str) {
    let error = result.err().expect("expected an error");
    assert!(
        error.to_string().starts_with(kind),
        "{name}: expected {kind:?}, got {error}"
    );
}

fn assert_rule_sets(mut actual: Vec<pd::Rule>, mut expected: Vec<pd::Rule>, name: &str) {
    let canonical = |rule: &pd::Rule| {
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

fn expect_options(
    name: &str,
    settings: Option<&model::PlacementSettings>,
    expected: Option<Vec<pd::Rule>>,
    error_kind: Option<&str>,
) {
    match (newBundleFromOptions(settings), expected, error_kind) {
        (Ok(Some(bundle)), Some(expected), None) => assert_rule_sets(bundle.Rules, expected, name),
        (Err(error), None, Some(kind)) => assert!(
            error.to_string().starts_with(kind),
            "{name}: expected {kind:?}, got {error}"
        ),
        (actual, expected, error) => {
            panic!("{name}: unexpected result {actual:?}, expected {expected:?}, error {error:?}")
        }
    }
}

#[test]
fn empty_clone_and_object_id_match_go_cases() {
    assert!(
        Bundle {
            ID: GroupID(1),
            ..Default::default()
        }
        .IsEmpty()
    );
    for bundle in [
        Bundle {
            ID: GroupID(1),
            Index: 1,
            ..Default::default()
        },
        Bundle {
            ID: GroupID(1),
            Override: true,
            ..Default::default()
        },
        Bundle {
            ID: GroupID(1),
            Rules: vec![pd::Rule {
                ID: "434".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        Bundle {
            ID: GroupID(1),
            Index: 1,
            Override: true,
            ..Default::default()
        },
    ] {
        assert!(!bundle.IsEmpty());
    }

    let original = Bundle {
        ID: GroupID(1),
        Rules: vec![pd::Rule {
            ID: "434".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut cloned = original.Clone();
    cloned.ID = GroupID(2);
    cloned.Rules[0].ID = "121".into();
    assert_eq!(original.ID, GroupID(1));
    assert_eq!(original.Rules[0].ID, "434");
    assert_eq!(cloned.ID, GroupID(2));
    assert_eq!(cloned.Rules[0].ID, "121");

    for (name, id, kind) in [
        ("non tidb bundle", "pd", ErrInvalidBundleIDFormat),
        ("id of words", "TiDB_DDL_foo", ErrInvalidBundleID),
        ("id of words and nums", "TiDB_DDL_3x", ErrInvalidBundleID),
        ("id of floats", "TiDB_DDL_3.0", ErrInvalidBundleID),
        ("id of negatives", "TiDB_DDL_-10", ErrInvalidBundleID),
    ] {
        assert_error_kind(
            Bundle {
                ID: id.into(),
                ..Default::default()
            }
            .ObjectID(),
            kind,
            name,
        );
    }
    assert_eq!(
        Bundle {
            ID: "TiDB_DDL_10".into(),
            ..Default::default()
        }
        .ObjectID()
        .unwrap(),
        10
    );
}

#[test]
fn get_leader_dc_matches_go_table() {
    let leader_rule = |key: &str, op: pd::LabelConstraintOp, values: &[&str], count| pd::Rule {
        Role: pd::Leader,
        Count: count,
        LabelConstraints: vec![constraint(key, op, values)],
        ..Default::default()
    };
    let cases = vec![
        (
            "only leader",
            vec![leader_rule("zone", pd::In, &["bj"], 1)],
            "bj",
        ),
        (
            "no leader",
            vec![pd::Rule {
                Role: pd::Voter,
                Count: 3,
                LabelConstraints: vec![constraint("zone", pd::In, &["bj"])],
                ..Default::default()
            }],
            "",
        ),
        (
            "voter and leader",
            vec![
                leader_rule("zone", pd::In, &["sh"], 1),
                pd::Rule {
                    Role: pd::Voter,
                    Count: 3,
                    LabelConstraints: vec![constraint("zone", pd::In, &["bj"])],
                    ..Default::default()
                },
            ],
            "sh",
        ),
        (
            "wrong label key",
            vec![leader_rule("fake", pd::In, &["sh"], 1)],
            "",
        ),
        (
            "wrong operator",
            vec![leader_rule("zone", pd::NotIn, &["sh"], 1)],
            "",
        ),
        (
            "leader has multiple values",
            vec![leader_rule("zone", pd::In, &["sh", "bj"], 1)],
            "",
        ),
        (
            "irrelevant rules",
            vec![
                leader_rule(EngineLabelKey, pd::NotIn, &[EngineLabelTiFlash], 1),
                leader_rule("disk", pd::NotIn, &["ssd", "hdd"], 1),
                leader_rule("zone", pd::In, &["bj"], 1),
            ],
            "bj",
        ),
        (
            "multiple leader count",
            vec![leader_rule("zone", pd::In, &["sh"], 2)],
            "",
        ),
        (
            "multiple leader rules",
            vec![
                leader_rule("zone", pd::In, &["sh"], 1),
                leader_rule("zone", pd::In, &["bj"], 1),
            ],
            "sh",
        ),
    ];
    for (name, rules, expected) in cases {
        let result = Bundle {
            ID: GroupID(1),
            Rules: rules,
            ..Default::default()
        }
        .GetLeaderDC("zone");
        assert_eq!(
            result,
            (expected.to_owned(), !expected.is_empty()),
            "{name}"
        );
    }
}

#[test]
fn string_and_new_bundle_match_go_cases() {
    let mut bundle = Bundle {
        ID: GroupID(1),
        Rules: vec![
            rule(pd::Voter, 3, vec![constraint("zone", pd::In, &["sh"])]),
            rule(
                pd::Voter,
                4,
                vec![
                    constraint("zone", pd::NotIn, &["sh"]),
                    constraint("zone", pd::In, &["bj"]),
                ],
            ),
            rule(
                pd::Voter,
                3,
                vec![
                    constraint("engine", pd::NotIn, &["tiflash"]),
                    constraint("engine", pd::NotIn, &["tiflash_compute"]),
                ],
            ),
        ],
        ..Default::default()
    };
    assert_eq!(
        bundle.String(),
        "{\"group_id\":\"TiDB_DDL_1\",\"group_index\":0,\"group_override\":false,\"rules\":[{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":3,\"label_constraints\":[{\"key\":\"zone\",\"op\":\"in\",\"values\":[\"sh\"]}]},{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":4,\"label_constraints\":[{\"key\":\"zone\",\"op\":\"notIn\",\"values\":[\"sh\"]},{\"key\":\"zone\",\"op\":\"in\",\"values\":[\"bj\"]}]},{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":3,\"label_constraints\":[{\"key\":\"engine\",\"op\":\"notIn\",\"values\":[\"tiflash\"]},{\"key\":\"engine\",\"op\":\"notIn\",\"values\":[\"tiflash_compute\"]}]}]}"
    );
    bundle.Rules.clear();

    assert_eq!(
        NewBundle(3),
        Bundle {
            ID: GroupID(3),
            ..Default::default()
        }
    );
    assert_eq!(
        NewBundle(-1),
        Bundle {
            ID: GroupID(-1),
            ..Default::default()
        }
    );
    assert_error_kind(
        NewBundleFromConstraintsOptions(None),
        ErrInvalidPlacementOptions,
        "nil constraints options",
    );
    assert_error_kind(
        NewBundleFromSugarOptions(None),
        ErrInvalidPlacementOptions,
        "nil sugar options",
    );
    assert_error_kind(
        NewBundleFromOptions(None),
        ErrInvalidPlacementOptions,
        "nil options",
    );
}

#[test]
fn new_bundle_from_options_matches_go_table() {
    let region = |value: &str| constraint("region", pd::In, &[value]);
    let empty = model::PlacementSettings::default();
    expect_options(
        "empty 1",
        Some(&empty),
        Some(vec![rule(pd::Voter, 3, vec![])]),
        None,
    );
    expect_options("empty 2", None, None, Some(ErrInvalidPlacementOptions));

    let cases: Vec<(
        &str,
        model::PlacementSettings,
        Option<Vec<pd::Rule>>,
        Option<&str>,
    )> = vec![
        (
            "empty 3",
            model::PlacementSettings {
                LearnerConstraints: "[+region=us]".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsReplicas),
        ),
        (
            "sugar normal one region",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "us".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 2, vec![region("us")]),
            ]),
            None,
        ),
        (
            "sugar majority one region",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "us".into(),
                Schedule: "majority_in_primary".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 1, vec![region("us")]),
                rule(pd::Voter, 1, vec![]),
            ]),
            None,
        ),
        (
            "sugar few followers",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "bj,sh,us".into(),
                Followers: 1,
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(
                    pd::Voter,
                    1,
                    vec![constraint("region", pd::In, &["bj", "sh"])],
                ),
            ]),
            None,
        ),
        (
            "sugar omit regions even",
            model::PlacementSettings {
                Followers: 2,
                Schedule: "even".into(),
                ..Default::default()
            },
            Some(vec![rule(pd::Voter, 3, vec![])]),
            None,
        ),
        (
            "sugar omit regions majority",
            model::PlacementSettings {
                Followers: 2,
                Schedule: "majority_in_primary".into(),
                ..Default::default()
            },
            Some(vec![rule(pd::Voter, 3, vec![])]),
            None,
        ),
        (
            "sugar wrong schedule",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "us".into(),
                Schedule: "wrong".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidPlacementOptions),
        ),
        (
            "sugar invalid region 1",
            model::PlacementSettings {
                PrimaryRegion: ",=,".into(),
                Regions: ",=,".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidPlacementOptions),
        ),
        (
            "sugar invalid region 2",
            model::PlacementSettings {
                PrimaryRegion: "f".into(),
                Regions: ",=".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidPlacementOptions),
        ),
        (
            "sugar invalid missing primary",
            model::PlacementSettings {
                Regions: "g".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidPlacementOptions),
        ),
        (
            "sugar even multiple regions",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "sh,us".into(),
                Followers: 5,
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 2, vec![region("us")]),
                rule(pd::Voter, 3, vec![region("sh")]),
            ]),
            None,
        ),
        (
            "sugar explicit even",
            model::PlacementSettings {
                PrimaryRegion: "us".into(),
                Regions: "sh,us".into(),
                Followers: 5,
                Schedule: "even".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 2, vec![region("us")]),
                rule(pd::Voter, 3, vec![region("sh")]),
            ]),
            None,
        ),
        (
            "sugar majority",
            model::PlacementSettings {
                PrimaryRegion: "sh".into(),
                Regions: "bj,sh".into(),
                Followers: 4,
                Schedule: "majority_in_primary".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("sh")]),
                rule(pd::Voter, 2, vec![region("sh")]),
                rule(pd::Voter, 2, vec![region("bj")]),
            ]),
            None,
        ),
        (
            "direct common constraints",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 2, vec![region("us")]),
            ]),
            None,
        ),
        (
            "direct common with learners",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                Followers: 2,
                Learners: 2,
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("us")]),
                rule(pd::Voter, 2, vec![region("us")]),
                rule(pd::Learner, 2, vec![region("us")]),
            ]),
            None,
        ),
        (
            "direct only leader constraints",
            model::PlacementSettings {
                LeaderConstraints: "[+region=as]".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("as")]),
                rule(pd::Voter, 2, vec![]),
            ]),
            None,
        ),
        (
            "direct only leader with followers",
            model::PlacementSettings {
                LeaderConstraints: "[+region=as]".into(),
                Followers: 4,
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("as")]),
                rule(pd::Voter, 4, vec![]),
            ]),
            None,
        ),
        (
            "direct leader and follower dict",
            model::PlacementSettings {
                LeaderConstraints: "[+region=as]".into(),
                FollowerConstraints: r#"{"+region=us": 2}"#.into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("as")]),
                rule(pd::Voter, 2, vec![region("us")]),
            ]),
            None,
        ),
        (
            "direct follower default count",
            model::PlacementSettings {
                LeaderConstraints: "[+region=as]".into(),
                FollowerConstraints: "[-region=us]".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![region("as")]),
                rule(pd::Voter, 2, vec![constraint("region", pd::NotIn, &["us"])]),
            ]),
            None,
        ),
        (
            "direct learner lacks count",
            model::PlacementSettings {
                LeaderConstraints: "[+region=as]".into(),
                LearnerConstraints: "[-region=us]".into(),
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsReplicas),
        ),
        (
            "direct omit leader",
            model::PlacementSettings {
                Followers: 2,
                FollowerConstraints: "[+region=bj]".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![]),
                rule(pd::Voter, 2, vec![region("bj")]),
            ]),
            None,
        ),
        (
            "direct leader conflict",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                LeaderConstraints: "[-region=us]".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrConflictingConstraints),
        ),
        (
            "direct follower conflict",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                FollowerConstraints: "[-region=us]".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrConflictingConstraints),
        ),
        (
            "direct learner conflict",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                LearnerConstraints: "[-region=us]".into(),
                Followers: 2,
                Learners: 2,
                ..Default::default()
            },
            None,
            Some(ErrConflictingConstraints),
        ),
        (
            "direct invalid leader",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                LeaderConstraints: "-region=us]".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "direct invalid common",
            model::PlacementSettings {
                Constraints: "+region=us]".into(),
                LeaderConstraints: "[-region=us]".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "direct invalid follower",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                FollowerConstraints: "-region=us]".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "direct invalid leader with learners",
            model::PlacementSettings {
                Constraints: "[+region=us]".into(),
                LeaderConstraints: "-region=us]".into(),
                Learners: 2,
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "direct follower dict",
            model::PlacementSettings {
                FollowerConstraints: "{+disk=ssd: 1}".into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![]),
                rule(pd::Voter, 1, vec![constraint("disk", pd::In, &["ssd"])]),
            ]),
            None,
        ),
        (
            "direct invalid follower dict count",
            model::PlacementSettings {
                FollowerConstraints: "{+disk=ssd: 1}".into(),
                Followers: 2,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsReplicas),
        ),
        (
            "direct learner dict",
            model::PlacementSettings {
                LearnerConstraints: r#"{"+region=us": 2}"#.into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Leader, 1, vec![]),
                rule(pd::Voter, 2, vec![]),
                rule(pd::Learner, 2, vec![region("us")]),
            ]),
            None,
        ),
        (
            "direct invalid learner dict count",
            model::PlacementSettings {
                LearnerConstraints: r#"{"+region=us": 2}"#.into(),
                Learners: 4,
                ..Default::default()
            },
            None,
            Some(ErrInvalidConstraintsReplicas),
        ),
        (
            "direct normal dict",
            model::PlacementSettings {
                Constraints: r#"{"+region=us": 3}"#.into(),
                ..Default::default()
            },
            Some(vec![rule(pd::Voter, 3, vec![region("us")])]),
            None,
        ),
        (
            "direct dict 2 2 1",
            model::PlacementSettings {
                Constraints:
                    r#"{ "+region=us-east-1":2, "+region=us-east-2": 2, "+region=us-west-1": 1}"#
                        .into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Voter, 2, vec![region("us-east-1")]),
                rule(pd::Voter, 2, vec![region("us-east-2")]),
                rule(pd::Voter, 1, vec![region("us-west-1")]),
            ]),
            None,
        ),
        (
            "direct common and learner dict",
            model::PlacementSettings {
                Constraints: r#"{"+region=us-east": 3}"#.into(),
                LearnerConstraints: r#"{"+region=us-west": 1}"#.into(),
                ..Default::default()
            },
            Some(vec![
                rule(pd::Voter, 3, vec![region("us-east")]),
                rule(pd::Learner, 1, vec![region("us-west")]),
            ]),
            None,
        ),
    ];

    for (name, settings, expected, error) in &cases {
        expect_options(name, Some(settings), expected.clone(), *error);
    }
}

fn encoded_table_prefix(id: i64) -> String {
    hex::encode(codec::EncodeBytes(
        Vec::new(),
        tablecodec::GenTablePrefix(id).as_ref(),
    ))
}

#[test]
fn reset_bundle_matches_go_single_and_multi_rule_cases() {
    let mut single = Bundle {
        ID: GroupID(1),
        Rules: vec![rule(
            pd::Voter,
            3,
            vec![constraint("zone", pd::In, &["sh"])],
        )],
        ..Default::default()
    };
    single.Reset(RuleIndexTable, &[3]);
    assert_eq!(single.ID, GroupID(3));
    assert_eq!(single.Index, RuleIndexTable);
    assert!(single.Override);
    assert_eq!(single.Rules.len(), 1);
    assert_eq!(single.Rules[0].GroupID, single.ID);
    assert_eq!(single.Rules[0].StartKeyHex, encoded_table_prefix(3));
    assert_eq!(single.Rules[0].EndKeyHex, encoded_table_prefix(4));

    let options = model::PlacementSettings {
        LeaderConstraints: r#"["+zone=bj"]"#.into(),
        Followers: 2,
        FollowerConstraints: r#"["+zone=hz"]"#.into(),
        Learners: 1,
        LearnerConstraints: r#"["+zone=cd"]"#.into(),
        Constraints: r#"["+disk=ssd"]"#.into(),
        ..Default::default()
    };
    let mut bundle = NewBundleFromOptions(Some(&options)).unwrap().unwrap();
    assert_eq!(bundle.Rules.len(), 3);
    bundle.Reset(RuleIndexTable, &[1, 2, 3]);
    assert_eq!(bundle.Rules.len(), 9);
    for (chunk, id) in bundle.Rules.chunks(3).zip([1, 2, 3]) {
        assert!(
            chunk
                .iter()
                .all(|item| item.StartKeyHex == encoded_table_prefix(id))
        );
        assert!(
            chunk
                .iter()
                .all(|item| item.EndKeyHex == encoded_table_prefix(id + 1))
        );
    }

    bundle.Reset(RuleIndexTable, &[1, 3, 4, 5]);
    assert_eq!(bundle.Rules.len(), 12);
    for (chunk, id) in bundle.Rules.chunks(3).zip([1, 3, 4, 5]) {
        assert!(
            chunk
                .iter()
                .all(|item| item.StartKeyHex == encoded_table_prefix(id))
        );
        assert!(
            chunk
                .iter()
                .all(|item| item.EndKeyHex == encoded_table_prefix(id + 1))
        );
    }
}

fn rack_rule(id: &str, role: pd::PeerRoleType, count: u64, rack: &str) -> pd::Rule {
    pd::Rule {
        ID: id.into(),
        Role: role,
        Count: count as i32,
        LabelConstraints: vec![constraint("rack", pd::In, &[rack])],
        LocationLabels: vec!["region".into()],
        ..Default::default()
    }
}

fn unconstrained_rule(id: &str, role: pd::PeerRoleType, count: u64) -> pd::Rule {
    pd::Rule {
        ID: id.into(),
        Role: role,
        Count: count as i32,
        LocationLabels: vec!["region".into()],
        ..Default::default()
    }
}

#[test]
fn tidy_matches_go_merge_matrix_and_is_stable() {
    let cases = vec![
        ("empty", vec![], vec![]),
        (
            "empty constraints leader voter",
            vec![
                unconstrained_rule("1", pd::Leader, 1),
                unconstrained_rule("2", pd::Voter, 2),
            ],
            vec![unconstrained_rule("0", pd::Voter, 3)],
        ),
        (
            "leader follower",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 2, "1"),
            ],
            vec![rack_rule("0", pd::Voter, 3, "1")],
        ),
        (
            "leader voter",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Voter, 2, "1"),
            ],
            vec![rack_rule("0", pd::Voter, 3, "1")],
        ),
        (
            "leader follower voter",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 1, "1"),
                rack_rule("3", pd::Voter, 1, "1"),
            ],
            vec![rack_rule("0", pd::Voter, 3, "1")],
        ),
        (
            "leader follower voter learner",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 1, "1"),
                rack_rule("3", pd::Voter, 1, "1"),
                rack_rule("4", pd::Learner, 2, "1"),
            ],
            vec![
                rack_rule("0", pd::Voter, 3, "1"),
                rack_rule("3", pd::Learner, 2, "1"),
            ],
        ),
        (
            "leader follower learner plus separate follower",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 1, "1"),
                rack_rule("3", pd::Learner, 1, "1"),
                rack_rule("4", pd::Follower, 1, "2"),
            ],
            vec![
                rack_rule("0", pd::Voter, 2, "1"),
                rack_rule("2", pd::Learner, 1, "1"),
                rack_rule("3", pd::Follower, 1, "2"),
            ],
        ),
        (
            "leader follower learner plus separate voter",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 1, "1"),
                rack_rule("3", pd::Learner, 1, "1"),
                rack_rule("4", pd::Voter, 1, "2"),
            ],
            vec![
                rack_rule("0", pd::Leader, 1, "1"),
                rack_rule("1", pd::Follower, 1, "1"),
                rack_rule("2", pd::Learner, 1, "1"),
                rack_rule("3", pd::Voter, 1, "2"),
            ],
        ),
        (
            "different constraints stay separate",
            vec![
                rack_rule("1", pd::Leader, 1, "1"),
                rack_rule("2", pd::Follower, 1, "2"),
            ],
            vec![
                rack_rule("0", pd::Leader, 1, "1"),
                rack_rule("1", pd::Follower, 1, "2"),
            ],
        ),
    ];

    for (name, rules, expected) in cases {
        let mut bundle = Bundle {
            Rules: rules,
            ..Default::default()
        };
        bundle.Tidy().unwrap();
        assert_eq!(bundle.Rules, expected, "{name}");
    }

    let mut pruned = Bundle {
        Rules: vec![
            rule(pd::Voter, 0, vec![constraint("zone", pd::In, &["sh"])]),
            rule(
                pd::Voter,
                4,
                vec![
                    constraint("zone", pd::NotIn, &["sh"]),
                    constraint("zone", pd::In, &["bj"]),
                ],
            ),
            rule(
                pd::Voter,
                4,
                vec![
                    constraint("zone", pd::NotIn, &["sh"]),
                    constraint("zone", pd::In, &["bj"]),
                ],
            ),
        ],
        ..Default::default()
    };
    pruned.Tidy().unwrap();
    assert_eq!(pruned.Rules.len(), 1);
    assert_eq!(pruned.Rules[0].Count, 8);
    assert_eq!(pruned.Rules[0].ID, "0");

    let mut stable = Bundle {
        Rules: vec![
            rule(
                pd::Voter,
                8,
                vec![
                    constraint("zone", pd::NotIn, &["sh"]),
                    constraint("zone", pd::In, &["bj"]),
                ],
            ),
            pd::Rule {
                Role: pd::Follower,
                Count: 9,
                LocationLabels: vec!["zone".into(), "host".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    stable.Tidy().unwrap();
    let once = stable.Clone();
    stable.Tidy().unwrap();
    assert_eq!(
        stable, once,
        "Go TestTidy requires this result to be stable"
    );
}

#[test]
fn range_keys_and_rebuild_match_go_cases() {
    let (start, end) = GetRangeStartAndEndKeyHex(TiDBBundleRangePrefixForMeta);
    assert_eq!(
        hex::decode(start).unwrap(),
        codec::EncodeBytes(Vec::new(), metaPrefix)
    );
    assert_eq!(
        hex::decode(end).unwrap(),
        codec::EncodeBytes(Vec::new(), tablecodec::GenTablePrefix(0).as_ref())
    );
    assert_eq!(
        GetRangeStartAndEndKeyHex("other"),
        (String::new(), String::new())
    );

    let mut bundle = Bundle {
        Rules: vec![rule(pd::Voter, 3, vec![])],
        ..Default::default()
    };
    bundle.RebuildForRange(KeyRangeMeta, "PolicyA");
    assert_eq!(bundle.ID, TiDBBundleRangePrefixForMeta);
    assert_eq!(bundle.Index, RuleIndexKeyRangeForMeta);
    assert!(bundle.Override);
    assert_eq!(bundle.Rules[0].ID, "policya_rule_0");
    assert_eq!(bundle.Rules[0].GroupID, TiDBBundleRangePrefixForMeta);
}
