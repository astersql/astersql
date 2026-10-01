// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// PlacementSettings / PolicyInfo 的单元测试。
//
// 覆盖 String 渲染顺序与引号转义、Clone 深拷贝隔离，以及精简场景下非默认约束输出。
// 文件前半保留 Go 参考源码字符串常量，便于与原测试对照。

use crate::group_3::*;
//

#[test]
/// 验证各设置项 String 输出顺序、零值跳过与 JSON 风格引号转义。
fn test_placement_settings_string() {
    let mut settings = PlacementSettings {
        PrimaryRegion: "us-east-1".into(),
        Regions: "us-east-1,us-east-2".into(),
        Schedule: "EVEN".into(),
        ..Default::default()
    };
    assert_eq!(
        "PRIMARY_REGION=\"us-east-1\" REGIONS=\"us-east-1,us-east-2\" SCHEDULE=\"EVEN\"",
        settings.String()
    );

    settings = PlacementSettings {
        LeaderConstraints: "[+region=bj]".into(),
        ..Default::default()
    };
    assert_eq!("LEADER_CONSTRAINTS=\"[+region=bj]\"", settings.String());

    settings = PlacementSettings {
        Voters: 1,
        VoterConstraints: "[+region=us-east-1]".into(),
        Followers: 2,
        FollowerConstraints: "[+disk=ssd]".into(),
        Learners: 3,
        LearnerConstraints: "[+region=us-east-2]".into(),
        ..Default::default()
    };
    assert_eq!(
        "VOTERS=1 VOTER_CONSTRAINTS=\"[+region=us-east-1]\" FOLLOWERS=2 FOLLOWER_CONSTRAINTS=\"[+disk=ssd]\" LEARNERS=3 LEARNER_CONSTRAINTS=\"[+region=us-east-2]\"",
        settings.String()
    );

    // Constraints 包含 JSON 风格引号，Go 测试确认 String 会对内部双引号转义。
    settings = PlacementSettings {
        Voters: 3,
        Followers: 2,
        Learners: 1,
        Constraints: "{\"+us-east-1\":1,+us-east-2:1}".into(),
        ..Default::default()
    };
    assert_eq!(
        "CONSTRAINTS=\"{\\\"+us-east-1\\\":1,+us-east-2:1}\" VOTERS=3 FOLLOWERS=2 LEARNERS=1",
        settings.String()
    );
}

#[test]
/// 修改 Clone 副本不得污染原 PlacementSettings。
fn test_placement_settings_clone() {
    let settings = PlacementSettings::default();
    let mut cloned_settings = settings.Clone();
    cloned_settings.PrimaryRegion = "r1".into();
    cloned_settings.Regions = "r1,r2".into();
    cloned_settings.Followers = 1;
    cloned_settings.Voters = 2;
    cloned_settings.Followers = 3;
    cloned_settings.Constraints = "[+zone=z1]".into();
    cloned_settings.LearnerConstraints = "[+region=r1]".into();
    cloned_settings.FollowerConstraints = "[+disk=ssd]".into();
    cloned_settings.LeaderConstraints = "[+region=r2]".into();
    cloned_settings.VoterConstraints = "[+zone=z2]".into();
    cloned_settings.Schedule = "even".into();

    // 修改 clone 后，原 settings 必须仍保持全默认值。
    assert!(settings.String().is_empty());
    assert_eq!(0, settings.Voters);
    assert_eq!(0, settings.Followers);
    assert_eq!(0, settings.Learners);
}

#[test]
/// PolicyInfo::Clone 必须深拷贝 PlacementSettings。
fn test_placement_policy_clone() {
    let policy = PolicyInfo {
        PlacementSettings: PlacementSettings::default(),
        ..Default::default()
    };
    let mut cloned_policy = policy.Clone();
    cloned_policy.ID = 100;
    cloned_policy.Name = ast::CIStr {
        O: "p2".into(),
        L: "p2".into(),
    };
    cloned_policy.State = SchemaState::DeleteOnly;
    cloned_policy.PlacementSettings.Followers = 10;

    // PolicyInfo::Clone 必须深拷贝 PlacementSettings，不能让 clone 的修改污染原对象。
    assert_eq!(0, policy.ID);
    assert_eq!(ast::CIStr::default(), policy.Name);
    assert_eq!(SchemaState::None, policy.State);
    assert!(policy.PlacementSettings.String().is_empty());
    assert_eq!(0, policy.PlacementSettings.Followers);
}
use crate::group_3::PlacementSettings;

#[test]
/// 精简断言：非默认 PrimaryRegion/Regions/Schedule 正确拼接，默认值为空串。
fn placement_settings_render_all_non_default_constraints() {
    let settings = PlacementSettings {
        PrimaryRegion: "us-east-1".into(),
        Regions: "us-east-1,us-east-2".into(),
        Schedule: "EVEN".into(),
        ..Default::default()
    };
    assert_eq!(
        settings.String(),
        "PRIMARY_REGION=\"us-east-1\" REGIONS=\"us-east-1,us-east-2\" SCHEDULE=\"EVEN\""
    );
    assert!(PlacementSettings::default().String().is_empty());
}

#[test]
fn crossks_align_policy_decode_go_missing_settings() {
    let policy: PolicyInfo = crate::ast::metadata_json::decode(
        br#"{"id":98001,"name":{"O":"placement","L":"placement"},"state":5}"#,
    )
    .unwrap();
    assert_eq!(policy.ID, 98001);
    assert_eq!(policy.Name.L, "placement");
    assert_eq!(policy.State, crate::group_3::SchemaState::Public);
    assert!(policy.PlacementSettings.String().is_empty());
}

#[test]
fn crossks_align_policy_go_wire_names_and_partial_settings() {
    let policy: PolicyInfo = crate::ast::metadata_json::decode(br#"{"id":7,"name":{"O":"P","L":"p"},"state":5,"primary_region":"east","followers":3,"leader_constraints":"[+zone=east]"}"#).unwrap();
    assert_eq!(policy.PlacementSettings.PrimaryRegion, "east");
    assert_eq!(policy.PlacementSettings.Followers, 3);
    assert_eq!(policy.PlacementSettings.LeaderConstraints, "[+zone=east]");
    let wire = String::from_utf8(crate::ast::metadata_json::encode(&policy).unwrap()).unwrap();
    assert!(wire.contains("\"primary_region\":\"east\""));
    assert!(wire.contains("\"name\":"));
    assert!(!wire.contains("PrimaryRegion"));
    let legacy: PolicyInfo = crate::ast::metadata_json::decode(
        br#"{"id":7,"Name":{"O":"P","L":"p"},"State":5,"PrimaryRegion":"west"}"#,
    )
    .unwrap();
    assert_eq!(legacy.PlacementSettings.PrimaryRegion, "west");
    assert_eq!(legacy.Name.L, "p");
}
