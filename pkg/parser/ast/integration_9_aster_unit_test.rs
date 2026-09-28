// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `integration` 模块的 Aster 迁移单元测试。
//
// 校验 CIStr、LEADING 展平、脱敏策略位掩码，以及索引提示类型/作用域
// 常量与 Go 语义一致。

use super::*;

/// cistrandhintpayloadsusegofieldsemantics。
#[test]
fn cistr_and_hint_payloads_use_go_field_semantics() {
    let name = NewCIStr("MiXeD");
    assert_eq!(name.O, "MiXeD");
    assert_eq!(name.L, "mixed");
    let from_string: CIStr = serde_json::from_str(r#""TeSt""#).unwrap();
    let from_object: CIStr = serde_json::from_str(r#"{"O":"X","L":"x"}"#).unwrap();
    let legacy_object: CIStr = serde_json::from_str(r#"{"O":"Mixed","L":"legacy"}"#).unwrap();
    assert_eq!(from_string, NewCIStr("TeSt"));
    assert_eq!(from_object, NewCIStr("X"));
    assert_eq!(legacy_object.O, "Mixed");
    assert_eq!(legacy_object.L, "legacy");

    assert!(FlattenLeadingList(&LeadingList::default()).is_empty());

    let nested = LeadingList {
        Items: vec![
            LeadingItem::Table(HintTable {
                TableName: NewCIStr("t1"),
                ..Default::default()
            }),
            LeadingItem::List(LeadingList {
                Items: vec![LeadingItem::Table(HintTable {
                    DBName: NewCIStr("db"),
                    TableName: NewCIStr("t2"),
                    QBName: NewCIStr("qb"),
                    PartitionList: vec![NewCIStr("p0")],
                    ..Default::default()
                })],
            }),
        ],
    };
    let flattened = FlattenLeadingList(&nested);
    assert_eq!(
        flattened
            .iter()
            .map(|table| table.TableName.O.as_str())
            .collect::<Vec<_>>(),
        ["t1", "t2"]
    );
    assert_eq!(flattened[1].DBName, NewCIStr("db"));
    assert_eq!(flattened[1].QBName, NewCIStr("qb"));
    assert_eq!(flattened[1].PartitionList, [NewCIStr("p0")]);
}

/// maskingpolicyrestrictbitsmatchgo。
#[test]
fn masking_policy_restrict_bits_match_go() {
    assert_eq!(MaskingPolicyRestrictOpNone, 0);
    assert_eq!(
        MaskingPolicyRestrictOpInsertIntoSelect
            | MaskingPolicyRestrictOpUpdateSelect
            | MaskingPolicyRestrictOpDeleteSelect
            | MaskingPolicyRestrictOpCTAS,
        15,
    );
}

/// indexhinttypesandscopescoverthegoconstants。
#[test]
fn index_hint_types_and_scopes_cover_the_go_constants() {
    assert_eq!(HintUse, IndexHintType::Use);
    assert_eq!(HintIgnore, IndexHintType::Ignore);
    assert_eq!(HintForce, IndexHintType::Force);
    assert_eq!(HintOrderIndex, IndexHintType::OrderIndex);
    assert_eq!(HintNoOrderIndex, IndexHintType::NoOrderIndex);
    assert_eq!(HintUse as i32, 1);
    assert_eq!(HintIgnore as i32, 2);
    assert_eq!(HintForce as i32, 3);
    assert_eq!(HintOrderIndex as i32, 4);
    assert_eq!(HintNoOrderIndex as i32, 5);

    assert_eq!(HintForScan, IndexHintScope::Scan);
    assert_eq!(HintForJoin, IndexHintScope::Join);
    assert_eq!(HintForOrderBy, IndexHintScope::OrderBy);
    assert_eq!(HintForGroupBy, IndexHintScope::GroupBy);
    assert_eq!(HintForScan as i32, 1);
    assert_eq!(HintForJoin as i32, 2);
    assert_eq!(HintForOrderBy as i32, 3);
    assert_eq!(HintForGroupBy as i32, 4);
}
