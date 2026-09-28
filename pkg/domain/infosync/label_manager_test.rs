// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// `filterRulesByKeyspace` 的单元测试。
//
// 验证未开启 keyspace 感知时返回全部规则，以及开启后仅保留匹配
// `keyspace/<id>/` 前缀的规则。

use crate::{Codec, filterRulesByKeyspace, label};

#[test]
/// 覆盖 keyspace 关闭/开启两种过滤路径。
fn test_filter_label_rules_by_keyspace() {
    let rules = vec![
        label::Rule {
            ID: "schema/test/t1".into(),
            ..Default::default()
        },
        label::Rule {
            ID: "keyspace/42/schema/test/t2".into(),
            ..Default::default()
        },
        label::Rule {
            ID: "keyspace/43/schema/test/t3".into(),
            ..Default::default()
        },
    ];
    // keyspace 感知关闭：不过滤，原样返回。
    assert_eq!(
        filterRulesByKeyspace(rules.clone(), Codec::default()),
        rules
    );
    // keyspace=42 且开启感知：仅保留带该前缀的规则。
    assert_eq!(
        filterRulesByKeyspace(
            rules.clone(),
            Codec {
                keyspace_id: Some(42),
                keyspace_aware_rules: true
            }
        ),
        vec![rules[1].clone()],
    );
}
