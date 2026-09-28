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

// Placement Bundle 相关单元测试（对齐 Go 行为）。
//
// Bundle 是 PD（Placement Driver）侧的一组放置规则（placement rule）集合，
// 用于描述表/分区/策略的副本分布。本文件校验约束解析、糖语法选项、
// Tidy 合并、范围重置与序列化等与 Go 一致的行为。

use super::*;

/// Constraint / RuleBuilder：解析、去重、拒绝 tiflash 引擎约束，以及字典约束分隔符校验。
#[test]
fn constraint_and_rule_builders_match_go_behavior() {
    let constraint = NewConstraint("+zone=sh").unwrap();
    assert_eq!(constraint.Key, "zone");
    assert_eq!(constraint.Op, pd::In);
    assert_eq!(constraint.Values, ["sh"]);
    assert_eq!(RestoreConstraint(&constraint).unwrap(), "+zone=sh");
    assert!(
        NewConstraint("+engine=tiflash")
            .unwrap_err()
            .to_string()
            .contains(ErrUnsupportedConstraint)
    );

    let mut constraints = NewConstraints(vec!["+zone=sh".into(), "+zone=sh".into()]).unwrap();
    assert_eq!(constraints.len(), 1);
    assert!(AddConstraint(&mut constraints, NewConstraint("+zone=bj").unwrap()).is_err());

    let mut builder = NewRuleBuilder();
    let rules = builder
        .SetRole(pd::Voter)
        .SetReplicasNum(3)
        .SetConstraintStr("{'+zone=sh': 2, '#evict-leader,+zone=bj': 1}".into())
        .BuildRules()
        .unwrap();
    assert_eq!(rules.iter().map(|rule| rule.Count).sum::<i32>(), 3);
    assert!(rules.iter().any(|rule| rule.Role == pd::Follower));

    let mut invalid = NewRuleBuilder();
    let error = invalid
        .SetRole(pd::Voter)
        .SetConstraintStr("{+region=us-east-2:2}".into())
        .BuildRules()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(ErrInvalidConstraintsMappingWrongSeparator)
    );
}

/// 糖语法 PlacementSettings 生成 Bundle；Tidy 将 Leader+Voter 合并为 Voter 计数。
#[test]
fn sugar_options_and_tidy_match_go_behavior() {
    let options = model::PlacementSettings {
        PrimaryRegion: "sh".into(),
        Regions: "bj, sh".into(),
        Followers: 3,
        Schedule: "even".into(),
        SurvivalPreferences: "[zone, rack]".into(),
        ..Default::default()
    };
    let bundle = NewBundleFromOptions(Some(&options)).unwrap().unwrap();
    assert_eq!(bundle.Rules.iter().map(|rule| rule.Count).sum::<i32>(), 4);
    assert_eq!(bundle.Rules[0].LocationLabels, ["zone", "rack"]);
    assert!(bundle.Rules.iter().all(|rule| !rule.ID.is_empty()));
    assert_eq!(bundle.GetLeaderDC("region"), ("sh".to_owned(), true));

    let mut merge = Bundle {
        Rules: vec![
            *NewRule(pd::Leader, 1, NewConstraintsDirect(vec![])),
            *NewRule(pd::Voter, 2, NewConstraintsDirect(vec![])),
            *NewRule(pd::Learner, 1, NewConstraintsDirect(vec![])),
        ],
        ..Default::default()
    };
    merge.Tidy().unwrap();
    assert_eq!(merge.Rules.len(), 2);
    assert!(
        merge
            .Rules
            .iter()
            .any(|rule| rule.Role == pd::Voter && rule.Count == 3)
    );
    assert!(
        merge
            .Rules
            .iter()
            .any(|rule| rule.Role == pd::Learner && rule.Count == 1)
    );
}

/// Bundle ID/ObjectID、Reset 按表与分区复制规则、RebuildForRange 与 JSON 序列化。
#[test]
fn bundle_identity_range_and_reset_match_go_behavior() {
    let mut bundle = NewBundle(42);
    assert_eq!(bundle.ID, "TiDB_DDL_42");
    assert_eq!(bundle.ObjectID().unwrap(), 42);
    assert!(
        Bundle {
            ID: "pd".into(),
            ..Default::default()
        }
        .ObjectID()
        .is_err()
    );

    bundle.Rules.push(*NewRule(pd::Voter, 3, vec![]));
    bundle.Reset(RuleIndexTable, &[42, 43]);
    assert_eq!(bundle.Rules.len(), 2);
    assert_eq!(bundle.Rules[0].Index, RuleIndexTable);
    assert_eq!(bundle.Rules[1].Index, RuleIndexPartition);
    assert_ne!(bundle.Rules[0].StartKeyHex, bundle.Rules[1].StartKeyHex);

    let mut cloned = bundle.Clone();
    let rebuilt = cloned.RebuildForRange(KeyRangeMeta, "PolicyA");
    assert_eq!(rebuilt.ID, TiDBBundleRangePrefixForMeta);
    assert!(
        rebuilt
            .Rules
            .iter()
            .all(|rule| rule.GroupID == TiDBBundleRangePrefixForMeta)
    );
    assert!(
        rebuilt
            .Rules
            .iter()
            .all(|rule| rule.ID.starts_with("policya_rule_"))
    );
    let (start, end) = GetRangeStartAndEndKeyHex(TiDBBundleRangePrefixForMeta);
    assert!(!start.is_empty() && !end.is_empty());

    let serialized = NewBundle(42).String();
    assert_eq!(
        serialized,
        r#"{"group_id":"TiDB_DDL_42","group_index":0,"group_override":false,"rules":[]}"#
    );
}
