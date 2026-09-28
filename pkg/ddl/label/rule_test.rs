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

// Label Rule 单元测试，对齐 Go `rule_test.go` 行为。
//
// 覆盖 AttributesSpec 解析、Default/空属性、Reset 后的 ID/Labels/Index/Data，
// 以及 Classic 与 NextGen Keyspace Codec 下的规则差异。

use super::{NewRule, RestoreRuleID, ast, kerneltype, pd, tablecodec, tikv};

// test_apply_attributes_spec 对应 Go 的 TestApplyAttributesSpec：先验证合法 attributes，再覆盖解析错误。
/// 合法 attributes 写入 Labels；非法字符串必须失败。
#[test]
fn test_apply_attributes_spec() {
    // valid case
    let mut spec = ast::AttributesSpec {
        Attributes: "key=value,key1=value1".to_owned(),
        ..Default::default()
    };
    let mut rule = NewRule();
    let err = rule.ApplyAttributesSpec(&spec);
    assert!(err.is_ok());
    assert_eq!(2, rule.Labels.len());
    assert_eq!("key", rule.Labels[0].Key);
    assert_eq!("value", rule.Labels[0].Value);
    assert_eq!("key1", rule.Labels[1].Key);
    assert_eq!("value1", rule.Labels[1].Value);

    // invalid cases：这些字符串分别覆盖空段、缺少等号、空 value 和空 key。
    let testcases = vec![
        "key=value,,key1=value1",
        "key-value,key1=value1",
        "key=,key1=value1",
        "=value,key1=value1",
    ];

    for testcase in testcases {
        spec = ast::AttributesSpec {
            Attributes: testcase.to_owned(),
            ..Default::default()
        };
        let err = rule.ApplyAttributesSpec(&spec);
        assert!(err.is_err(), "invalid attributes should fail: {}", testcase);
    }
}

// test_default_or_empty 对应 Go 的 TestDefaultOrEmpty：空 attributes 或 Default=true 不生成 label。
/// 空 attributes 或 Default=true 时，Reset 后 Labels 仍为空。
#[test]
fn test_default_or_empty() {
    let specs = vec![
        ast::AttributesSpec {
            Attributes: String::new(),
            ..Default::default()
        },
        ast::AttributesSpec {
            Default: true,
            ..Default::default()
        },
    ];

    for spec in specs {
        let mut rule = NewRule();
        let err = rule.ApplyAttributesSpec(&spec);
        assert!(err.is_ok());

        // Reset 使用 TiKV V1 codec 和 db/table/tableID；Go 断言 default/empty 不附加 labels。
        rule.Reset(
            tikv::NewCodecV1(),
            "db".to_owned(),
            "t".to_owned(),
            String::new(),
            vec![1],
        );
        assert_eq!(0, rule.Labels.len());
    }
}

// test_reset 对应 Go 的 TestReset：验证 rule ID、labels、index 和 table range 数据。
/// 验证表级/分区级 Reset 的 ID、内部标签、Index 与固定 hex 键范围。
#[test]
fn test_reset() {
    let mut spec = ast::AttributesSpec {
        Attributes: "key=value".to_owned(),
        ..Default::default()
    };
    let mut rule = NewRule();
    assert!(rule.ApplyAttributesSpec(&spec).is_ok());

    rule.Reset(
        tikv::NewCodecV1(),
        "db1".to_owned(),
        "t1".to_owned(),
        String::new(),
        vec![1, 2, 3],
    );
    assert_eq!("schema/db1/t1", rule.ID);
    assert_eq!("key-range", rule.RuleType);
    assert_eq!(3, rule.Labels.len());
    assert_eq!("value", rule.Labels[0].Value);
    assert_eq!("db1", rule.Labels[1].Value);
    assert_eq!("t1", rule.Labels[2].Value);
    assert_eq!(2, rule.Index);

    // Go 通过 type assertion 读取 rule.Data 中的 start_key/end_key；这里直接保留期望的十六进制区间。
    let r = &rule.Data[0];
    assert_eq!("7480000000000000ff0100000000000000f8", r["start_key"]);
    assert_eq!("7480000000000000ff0200000000000000f8", r["end_key"]);
    let r = &rule.Data[1];
    assert_eq!("7480000000000000ff0200000000000000f8", r["start_key"]);
    assert_eq!("7480000000000000ff0300000000000000f8", r["end_key"]);
    let r = &rule.Data[2];
    assert_eq!("7480000000000000ff0300000000000000f8", r["start_key"]);
    assert_eq!("7480000000000000ff0400000000000000f8", r["end_key"]);

    let r1 = rule.Clone();
    assert_eq!(*r1, *rule);

    rule.Reset(
        tikv::NewCodecV1(),
        "db2".to_owned(),
        "t2".to_owned(),
        "p2".to_owned(),
        vec![2],
    );
    assert_eq!("schema/db2/t2/p2", rule.ID);
    assert_eq!(4, rule.Labels.len());
    assert_eq!("value", rule.Labels[0].Value);
    assert_eq!("db2", rule.Labels[1].Value);
    assert_eq!("t2", rule.Labels[2].Value);
    assert_eq!("p2", rule.Labels[3].Value);
    assert_eq!(3, rule.Index);

    let r = &rule.Data[0];
    assert_eq!("7480000000000000ff0200000000000000f8", r["start_key"]);
    assert_eq!("7480000000000000ff0300000000000000f8", r["end_key"]);

    // default case：Default=true 时 expected 只带 ID，不保留显式 RegionLabel。
    spec = ast::AttributesSpec {
        Default: true,
        ..Default::default()
    };
    let mut rule = NewRule();
    let mut expected = NewRule();
    expected.ID = "schema/db3/t3/p3".to_owned();
    expected.Labels = vec![];
    assert!(rule.ApplyAttributesSpec(&spec).is_ok());
    rule.Reset(
        tikv::NewCodecV1(),
        "db3".to_owned(),
        "t3".to_owned(),
        "p3".to_owned(),
        vec![3],
    );
    assert_eq!(*rule, *expected);
}

// test_reset_with_keyspace_codec 对应 Go 的 TestResetWithKeyspaceCodec：比较 classic 与 next-gen keyspace 行为。
/// Classic 忽略 V2 Codec；NextGen 生成 keyspace 前缀 ID 与 EncodeRegionRange 键。
#[test]
fn test_reset_with_keyspace_codec() {
    let keyspace_id: u32 = 42;
    let codec_v2 = tikv::NewCodecV2(keyspace_id).expect("valid uint24 keyspace ID");

    let spec = ast::AttributesSpec {
        Attributes: "key=value".to_owned(),
        ..Default::default()
    };
    let mut rule = NewRule();
    assert!(rule.ApplyAttributesSpec(&spec).is_ok());
    rule.Reset(
        tikv::NewCodecV1(),
        "db1".to_owned(),
        "t1".to_owned(),
        String::new(),
        vec![1],
    );
    assert_eq!("schema/db1/t1", rule.ID);
    assert_eq!(3, rule.Labels.len());
    assert_eq!(
        "7480000000000000ff0100000000000000f8",
        rule.Data[0]["start_key"]
    );

    if kerneltype::IsClassic() {
        // Classic 模式即便使用 codecV2，也保持旧版 schema ID 和 V1 range 表现。
        rule.Reset(
            codec_v2,
            "db1".to_owned(),
            "t1".to_owned(),
            String::new(),
            vec![1],
        );
        assert_eq!("schema/db1/t1", rule.ID);
        assert_eq!(3, rule.Labels.len());
        assert_eq!(
            "7480000000000000ff0100000000000000f8",
            rule.Data[0]["start_key"]
        );
        return;
    }

    let mut next_gen_rule = NewRule();
    assert!(next_gen_rule.ApplyAttributesSpec(&spec).is_ok());
    next_gen_rule.Reset(
        codec_v2.clone(),
        "db1".to_owned(),
        "t1".to_owned(),
        String::new(),
        vec![1],
    );
    assert_eq!("keyspace/42/schema/db1/t1", next_gen_rule.ID);
    assert_eq!("schema/db1/t1", RestoreRuleID(&next_gen_rule.ID));
    assert!(next_gen_rule.Labels.contains(&pd::RegionLabel {
        Key: "keyspace".to_owned(),
        Value: "42".to_owned(),
        ..Default::default()
    }));
    assert!(next_gen_rule.Labels.contains(&pd::RegionLabel {
        Key: "db".to_owned(),
        Value: "db1".to_owned(),
        ..Default::default()
    }));
    assert!(next_gen_rule.Labels.contains(&pd::RegionLabel {
        Key: "table".to_owned(),
        Value: "t1".to_owned(),
        ..Default::default()
    }));

    // Go 用 tablecodec.GenTablePrefix 后交给 codecV2 EncodeRegionRange，再 hex 编码后和 rule.Data 对比。
    let (start_key, end_key) = codec_v2.EncodeRegionRange(
        tablecodec::GenTablePrefix(1).0,
        tablecodec::GenTablePrefix(2).0,
    );
    let data = &next_gen_rule.Data[0];
    assert_eq!(hex::encode(start_key), data["start_key"]);
    assert_eq!(hex::encode(end_key), data["end_key"]);

    next_gen_rule.Reset(
        codec_v2,
        "db2".to_owned(),
        "t2".to_owned(),
        "p2".to_owned(),
        vec![2],
    );
    assert_eq!("keyspace/42/schema/db2/t2/p2", next_gen_rule.ID);
    assert_eq!("schema/db2/t2/p2", RestoreRuleID(&next_gen_rule.ID));
    assert!(next_gen_rule.Labels.contains(&pd::RegionLabel {
        Key: "partition".to_owned(),
        Value: "p2".to_owned(),
        ..Default::default()
    }));
}

// Go 的 int64 加法在最大表 ID 处按二补码回绕；Rust 调试构建也必须保持该行为而非 panic。
#[test]
fn test_reset_max_table_id_wraps_end_boundary() {
    let spec = ast::AttributesSpec {
        Attributes: "key=value".to_owned(),
        ..Default::default()
    };
    let mut rule = NewRule();
    assert!(rule.ApplyAttributesSpec(&spec).is_ok());

    rule.Reset(
        tikv::NewCodecV1(),
        "db".to_owned(),
        "t".to_owned(),
        String::new(),
        vec![i64::MAX],
    );

    let expected_end =
        crate::codec::EncodeBytes(Vec::new(), tablecodec::GenTablePrefix(i64::MIN).as_ref());
    assert_eq!(hex::encode(expected_end), rule.Data[0]["end_key"]);
}
