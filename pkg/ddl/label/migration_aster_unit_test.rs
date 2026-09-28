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

// Label 核心逻辑的 Aster 迁移单元测试。
//
// 覆盖属性解析严格性、Labels 去重与冲突、AttributesSpec 应用、
// Keyspace 感知的 Rule Reset（重置规则 ID/标签/键范围）以及
// Clone/JSON/Patch 与 PD 形状一致性，对齐 Go 侧行为。

use super::{
    Add, NewLabel, NewLabels, NewRule, NewRuleID, NewRulePatch, RestoreRegionLabels, RestoreRuleID,
    UseKeyspaceAwareRules, ast::AttributesSpec, pd::RegionLabel, tablecodec, tikv,
};

/// 合法 label 会 trim 空白；非法格式（缺等号、多余段等）必须报错。
#[test]
fn label_parsing_is_strict_and_trims_like_go() {
    let label = NewLabel(" merge_option=allow ").expect("valid label");
    assert_eq!("merge_option", label.Key);
    assert_eq!("allow", label.Value);

    for invalid in ["", "key", "=value", "key=", "a=b=c"] {
        let error = NewLabel(invalid).expect_err("invalid label must fail");
        assert!(
            error
                .to_string()
                .contains("attributes should be in format 'key=value'"),
            "unexpected error for {invalid:?}: {error}"
        );
    }
}

/// 重复 key=value 去重；同 key 不同 value 冲突且不修改原集合；Restore 输出可见形式。
#[test]
fn label_add_deduplicates_conflicts_and_restores_visible_labels() {
    let mut labels = NewLabels(vec![
        "merge_option=allow".to_owned(),
        "merge_option=allow".to_owned(),
        "db=d1".to_owned(),
        "table=t1".to_owned(),
        "partition=p1".to_owned(),
        "keyspace=42".to_owned(),
    ])
    .expect("labels");
    assert_eq!(5, labels.len(), "duplicate must be ignored");
    let expected = if kerneltype_dependency::IsNextGen() {
        "\"merge_option=allow\""
    } else {
        "\"merge_option=allow\",\"keyspace=42\""
    };
    assert_eq!(expected, RestoreRegionLabels(&labels));

    let before = labels.clone();
    let conflict = Add(
        &mut labels,
        RegionLabel {
            Key: "merge_option".to_owned(),
            Value: "deny".to_owned(),
            ..Default::default()
        },
    )
    .expect_err("same key with a different value must conflict");
    assert_eq!(before, labels, "conflict must not mutate the labels");
    assert_eq!(
        "'merge_option=deny' and 'merge_option=allow' are conflicted",
        conflict.to_string()
    );
}

/// AttributesSpec 走严格 YAML 解析；Default=true 时清空 Labels。
#[test]
fn attributes_spec_uses_strict_yaml_and_default_clears_labels() {
    let mut rule = NewRule();
    rule.ApplyAttributesSpec(&AttributesSpec {
        Attributes: "key=value,key1=value1".to_owned(),
        Default: false,
    })
    .expect("valid attributes");
    assert_eq!(2, rule.Labels.len());

    for invalid in [
        "key=value,,key1=value1",
        "key-value,key1=value1",
        "key=,key1=value1",
        "=value,key1=value1",
    ] {
        assert!(
            rule.ApplyAttributesSpec(&AttributesSpec {
                Attributes: invalid.to_owned(),
                Default: false,
            })
            .is_err(),
            "invalid attributes must fail: {invalid}"
        );
    }

    rule.ApplyAttributesSpec(&AttributesSpec {
        Attributes: "ignored=value".to_owned(),
        Default: true,
    })
    .expect("default attributes");
    assert!(rule.Labels.is_empty());
}

/// Reset 在 V2 Codec 下注入 keyspace/db/table/partition，并按 table ID 排序生成键范围。
#[test]
fn reset_matches_go_for_keyspace_ids_labels_and_sorted_ranges() {
    let codec_v2 = tikv::NewCodecV2(42).expect("valid uint24 keyspace ID");
    let nextgen = kerneltype_dependency::IsNextGen();
    assert_eq!(nextgen, UseKeyspaceAwareRules(codec_v2.clone()));
    let expected_id = if nextgen {
        "keyspace/42/schema/db1/t1"
    } else {
        "schema/db1/t1"
    };
    assert!(!UseKeyspaceAwareRules(tikv::NewCodecV1()));
    assert!(!UseKeyspaceAwareRules(tikv::NilCodec()));
    assert_eq!(
        expected_id,
        NewRuleID(
            codec_v2.clone(),
            "db1".to_owned(),
            "t1".to_owned(),
            String::new()
        )
    );

    let mut rule = NewRule();
    rule.ApplyAttributesSpec(&AttributesSpec {
        Attributes: "key=value".to_owned(),
        Default: false,
    })
    .expect("attributes");
    rule.Reset(
        codec_v2.clone(),
        "db1".to_owned(),
        "t1".to_owned(),
        String::new(),
        vec![3, 1, 2],
    );

    assert_eq!(expected_id, rule.ID);
    assert_eq!("schema/db1/t1", RestoreRuleID(&rule.ID));
    assert_eq!(2, rule.Index);
    assert_eq!("key-range", rule.RuleType);
    assert_eq!(
        nextgen,
        rule.Labels
            .iter()
            .any(|label| label.Key == "keyspace" && label.Value == "42")
    );
    for expected in [("db", "db1"), ("table", "t1")] {
        assert!(
            rule.Labels
                .iter()
                .any(|label| label.Key == expected.0 && label.Value == expected.1),
            "missing label {expected:?}"
        );
    }

    // 期望的 Data 与 Go 一致：按 ID 排序后用 EncodeRegionRange 生成 hex 起止键。
    let expected_ranges: Vec<_> = [1_i64, 2, 3]
        .into_iter()
        .map(|id| {
            let start_prefix = tablecodec::GenTablePrefix(id);
            let end_prefix = tablecodec::GenTablePrefix(id + 1);
            let codec = if nextgen {
                codec_v2.clone()
            } else {
                tikv::NewCodecV1()
            };
            let (start, end) = codec.EncodeRegionRange(start_prefix.0, end_prefix.0);
            serde_json::json!({
                "start_key": hex::encode(start),
                "end_key": hex::encode(end),
            })
        })
        .collect();
    assert_eq!(serde_json::Value::Array(expected_ranges), rule.Data);

    rule.Reset(
        codec_v2,
        "db2".to_owned(),
        "t2".to_owned(),
        "p2".to_owned(),
        vec![2],
    );
    assert_eq!(
        if nextgen {
            "keyspace/42/schema/db2/t2/p2"
        } else {
            "schema/db2/t2/p2"
        },
        rule.ID
    );
    assert_eq!(3, rule.Index);
    assert!(
        rule.Labels
            .iter()
            .any(|label| label.Key == "partition" && label.Value == "p2")
    );
}

/// Clone、JSON 序列化与 RulePatch 保持 PD 侧字段形状。
#[test]
fn rule_clone_json_and_patch_preserve_pd_shape() {
    let mut rule = NewRule();
    rule.ID = "schema/db/table".to_owned();
    rule.Labels = vec![NewLabel("key=value").expect("label")];
    let cloned = rule.Clone();
    assert_eq!(*rule, *cloned);

    let json: serde_json::Value = serde_json::from_str(&rule.String()).expect("rule JSON");
    assert_eq!("schema/db/table", json["id"]);
    assert_eq!("key", json["labels"][0]["key"]);

    let patch = NewRulePatch(vec![rule], vec!["schema/old/table".to_owned()]);
    assert_eq!(1, patch.SetRules.len());
    assert_eq!(vec!["schema/old/table"], patch.DeleteRules);
    assert_eq!(
        serde_json::json!({
            "sets": [{
                "id": "schema/db/table",
                "index": 0,
                "labels": [{"key": "key", "value": "value"}],
                "rule_type": "",
                "data": null
            }],
            "deletes": ["schema/old/table"]
        }),
        serde_json::to_value(&*patch).expect("PD label rule patch JSON")
    );
}

/// PD omits empty TTL/start_at fields, so decoding the common minimal shape must succeed.
#[test]
fn pd_region_label_accepts_omitted_optional_fields() {
    let label: RegionLabel =
        serde_json::from_value(serde_json::json!({"key": "db", "value": "test"}))
            .expect("PD omits empty optional RegionLabel fields");
    assert_eq!("db", label.Key);
    assert_eq!("test", label.Value);
    assert!(label.TTL.is_empty());
    assert!(label.StartAt.is_empty());
}

/// client-go preserves an unbounded V1 end and maps an unbounded V2 end to the next keyspace.
#[test]
fn codec_unbounded_region_ranges_match_client_go() {
    let (_, v1_end) = tikv::NewCodecV1().EncodeRegionRange(vec![b't'], Vec::new());
    assert!(v1_end.is_empty());

    let (_, v2_end) = tikv::NewCodecV2(42)
        .expect("valid uint24 keyspace ID")
        .EncodeRegionRange(vec![b't'], Vec::new());
    assert_eq!("7800002b00000000fb", hex::encode(v2_end));

    assert!(
        tikv::NewCodecV2(0x0100_0000).is_err(),
        "client-go rejects keyspace IDs wider than uint24"
    );
}
