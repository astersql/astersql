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

// 基于 meta / PlacementSettings 构造 Bundle 的单元测试。
//
// 验证由 Placement Options（主区域、副本区域、Follower 数、调度策略等）
// 生成的 Bundle（规则组）规则数量与 Leader DC（Leader 所在可用区）提取正确。
// 文件前半保留 Go 侧依赖 mockstore/meta 的表、分区 Bundle 构造用例形状。

// 覆盖基于 meta/mockstore 的 table、partition、partition list 和 full table bundle 构造测试。
//
// #![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables, unused_mut)]
//
// metaBundleSuite 对应 Go 测试辅助结构，字段顺序按来源文件保留。
// pub struct metaBundleSuite {
//     policy1 *model.PolicyInfo
//     policy2 *model.PolicyInfo
//     policy3 *model.PolicyInfo
//     tbl1    *model.TableInfo
//     tbl2    *model.TableInfo
//     tbl3    *model.TableInfo
//     tbl4    *model.TableInfo
// }
//
// createMetaBundleSuite 对应 Go 辅助函数；原型：`func createMetaBundleSuite() *metaBundleSuite`。
// pub fn create_meta_bundle_suite() -> &mut metaBundleSuite {
//     s := new(metaBundleSuite)
//     s.policy1 = &model.PolicyInfo{
//         ID:   11,
//         Name: ast.NewCIStr("p1"),
//         PlacementSettings: &model.PlacementSettings{
//             PrimaryRegion: "r1",
//             Regions:       "r1,r2",
//         },
//         State: model.StatePublic,
//     }
//     s.policy2 = &model.PolicyInfo{
//         ID:   12,
//         Name: ast.NewCIStr("p2"),
//         PlacementSettings: &model.PlacementSettings{
//             PrimaryRegion: "r2",
//             Regions:       "r1,r2",
//         },
//         State: model.StatePublic,
//     }
//     s.policy3 = &model.PolicyInfo{
//         ID:   13,
//         Name: ast.NewCIStr("p3"),
//         PlacementSettings: &model.PlacementSettings{
//             LeaderConstraints: "[+region=bj]",
//         },
//         State: model.StatePublic,
//     }
//     s.tbl1 = &model.TableInfo{
//         ID:   101,
//         Name: ast.NewCIStr("t1"),
//         PlacementPolicyRef: &model.PolicyRefInfo{
//             ID:   11,
//             Name: ast.NewCIStr("p1"),
//         },
//         Partition: &model.PartitionInfo{
//             Definitions: []model.PartitionDefinition{
//                 {
//                     ID:   1000,
//                     Name: ast.NewCIStr("par0"),
//                 },
//                 {
//                     ID:                 1001,
//                     Name:               ast.NewCIStr("par1"),
//                     PlacementPolicyRef: &model.PolicyRefInfo{ID: 12, Name: ast.NewCIStr("p2")},
//                 },
//                 {
//                     ID:   1002,
//                     Name: ast.NewCIStr("par2"),
//                 },
//             },
//         },
//     }
//     s.tbl2 = &model.TableInfo{
//         ID:   102,
//         Name: ast.NewCIStr("t2"),
//         Partition: &model.PartitionInfo{
//             Definitions: []model.PartitionDefinition{
//                 {
//                     ID:                 1000,
//                     Name:               ast.NewCIStr("par0"),
//                     PlacementPolicyRef: &model.PolicyRefInfo{ID: 11, Name: ast.NewCIStr("p1")},
//                 },
//                 {
//                     ID:   1001,
//                     Name: ast.NewCIStr("par1"),
//                 },
//                 {
//                     ID:   1002,
//                     Name: ast.NewCIStr("par2"),
//                 },
//             },
//         },
//     }
//     s.tbl3 = &model.TableInfo{
//         ID:                 103,
//         Name:               ast.NewCIStr("t3"),
//         PlacementPolicyRef: &model.PolicyRefInfo{ID: 13, Name: ast.NewCIStr("p3")},
//     }
//     s.tbl4 = &model.TableInfo{
//         ID:   104,
//         Name: ast.NewCIStr("t4"),
//     }
//     return s
// }
//
// prepareMeta 对应 Go 方法；原型：`func (s *metaBundleSuite) prepareMeta(t *testing.T, store kv.Storage)`。
// pub fn prepare_meta() {
// 接收者和参数暂不转换为可编译 Rust 类型；下方保留 Go 方法体的业务检查顺序。
//     ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
// Go 在新事务里读取/写入 meta；不执行事务，只保留闭包边界和校验顺序。
//     err := kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
//         m := meta.NewMutator(txn)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, m.CreatePolicy(s.policy1))
//         require.NoError(t, m.CreatePolicy(s.policy2))
//         require.NoError(t, m.CreatePolicy(s.policy3))
//         return nil
//     })
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// }
//
// #[test]
// pub fn test_new_table_bundle() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// mockstore 是 Go 测试外部依赖；这里不会创建真实存储，只保留初始化位置。
//     store, err := mockstore.NewMockStore()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     defer func() { require.NoError(t, store.Close()) }()
//
//     s := createMetaBundleSuite()
//     s.prepareMeta(t, store)
//     ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
//         m := meta.NewMutator(txn)
//
// tbl1
//         bundle, err := placement.NewTableBundle(m, s.tbl1)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkTableBundle(t, s.tbl1, bundle)
//
// tbl2
//         bundle, err = placement.NewTableBundle(m, s.tbl2)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkTableBundle(t, s.tbl2, bundle)
//
// tbl3
//         bundle, err = placement.NewTableBundle(m, s.tbl3)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkTableBundle(t, s.tbl3, bundle)
//
// tbl4
//         bundle, err = placement.NewTableBundle(m, s.tbl4)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkTableBundle(t, s.tbl4, bundle)
//
//         return nil
//     }))
// }
//
// #[test]
// pub fn test_new_partition_bundle() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// mockstore 是 Go 测试外部依赖；这里不会创建真实存储，只保留初始化位置。
//     store, err := mockstore.NewMockStore()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     defer func() { require.NoError(t, store.Close()) }()
//
//     s := createMetaBundleSuite()
//     s.prepareMeta(t, store)
//
//     ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
//         m := meta.NewMutator(txn)
//
// tbl1.par0
//         bundle, err := placement.NewPartitionBundle(m, s.tbl1.Partition.Definitions[0])
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkPartitionBundle(t, s.tbl1.Partition.Definitions[0], bundle)
//
// tbl1.par1
//         bundle, err = placement.NewPartitionBundle(m, s.tbl1.Partition.Definitions[1])
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
//         s.checkPartitionBundle(t, s.tbl1.Partition.Definitions[1], bundle)
//
//         return nil
//     }))
// }
//
// #[test]
// pub fn test_new_partition_list_bundles() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// mockstore 是 Go 测试外部依赖；这里不会创建真实存储，只保留初始化位置。
//     store, err := mockstore.NewMockStore()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     defer func() { require.NoError(t, store.Close()) }()
//
//     s := createMetaBundleSuite()
//     s.prepareMeta(t, store)
//
//     ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
//         m := meta.NewMutator(txn)
//
//         bundles, err := placement.NewPartitionListBundles(m, s.tbl1.Partition.Definitions)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 1)
//         s.checkPartitionBundle(t, s.tbl1.Partition.Definitions[1], bundles[0])
//
//         bundles, err = placement.NewPartitionListBundles(m, []model.PartitionDefinition{})
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 0)
//
//         bundles, err = placement.NewPartitionListBundles(m, []model.PartitionDefinition{
//             s.tbl1.Partition.Definitions[0],
//             s.tbl1.Partition.Definitions[2],
//         })
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 0)
//
//         return nil
//     }))
// }
//
// #[test]
// pub fn test_new_full_table_bundles() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// mockstore 是 Go 测试外部依赖；这里不会创建真实存储，只保留初始化位置。
//     store, err := mockstore.NewMockStore()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     defer func() { require.NoError(t, store.Close()) }()
//
//     s := createMetaBundleSuite()
//     s.prepareMeta(t, store)
//
//     ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
//         m := meta.NewMutator(txn)
//
//         bundles, err := placement.NewFullTableBundles(m, s.tbl1)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 2)
//         s.checkTableBundle(t, s.tbl1, bundles[0])
//         s.checkPartitionBundle(t, s.tbl1.Partition.Definitions[1], bundles[1])
//
//         bundles, err = placement.NewFullTableBundles(m, s.tbl2)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 1)
//         s.checkPartitionBundle(t, s.tbl2.Partition.Definitions[0], bundles[0])
//
//         bundles, err = placement.NewFullTableBundles(m, s.tbl3)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 1)
//         s.checkTableBundle(t, s.tbl3, bundles[0])
//
//         bundles, err = placement.NewFullTableBundles(m, s.tbl4)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundles, 0)
//
//         return nil
//     }))
// }
//
// checkTwoJSONObjectEquals 对应 Go 方法；原型：`func (s *metaBundleSuite) checkTwoJSONObjectEquals(t *testing.T, expected any, got any)`。
// pub fn check_two_json_object_equals() {
// 接收者和参数暂不转换为可编译 Rust 类型；下方保留 Go 方法体的业务检查顺序。
//     expectedJSON, err := json.Marshal(expected)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     expectedStr := string(expectedJSON)
//
//     gotJSON, err := json.Marshal(got)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     gotStr := string(gotJSON)
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, expectedStr, gotStr)
// }
//
// checkTableBundle 对应 Go 方法；原型：`func (s *metaBundleSuite) checkTableBundle(t *testing.T, tbl *model.TableInfo, got *placement.Bundle)`。
// pub fn check_table_bundle() {
// 接收者和参数暂不转换为可编译 Rust 类型；下方保留 Go 方法体的业务检查顺序。
//     if tbl.PlacementPolicyRef == nil {
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Nil(t, got)
//         return
//     }
//
//     expected := &placement.Bundle{
//         ID:       fmt.Sprintf("TiDB_DDL_%d", tbl.ID),
//         Index:    placement.RuleIndexTable,
//         Override: true,
//         Rules:    s.expectedRules(t, tbl.PlacementPolicyRef),
//     }
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for idx, rule := range expected.Rules {
//         rule.GroupID = expected.ID
//         rule.Index = placement.RuleIndexTable
//         rule.ID = fmt.Sprintf("table_rule_%d_%d", tbl.ID, idx)
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//         rule.StartKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(tbl.ID)))
//         rule.EndKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(tbl.ID+1)))
//     }
//
//     if tbl.Partition != nil {
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//         for _, par := range tbl.Partition.Definitions {
//             rules := s.expectedRules(t, tbl.PlacementPolicyRef)
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//             for idx, rule := range rules {
//                 rule.GroupID = expected.ID
//                 rule.Index = placement.RuleIndexPartition
//                 rule.ID = fmt.Sprintf("partition_rule_%d_%d", par.ID, idx)
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//                 rule.StartKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(par.ID)))
//                 rule.EndKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(par.ID+1)))
//                 expected.Rules = append(expected.Rules, rule)
//             }
//         }
//     }
//
//     s.checkTwoJSONObjectEquals(t, expected, got)
// }
//
// checkPartitionBundle 对应 Go 方法；原型：`func (s *metaBundleSuite) checkPartitionBundle(t *testing.T, def model.PartitionDefinition, got *placement.Bundle)`。
// pub fn check_partition_bundle() {
// 接收者和参数暂不转换为可编译 Rust 类型；下方保留 Go 方法体的业务检查顺序。
//     if def.PlacementPolicyRef == nil {
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Nil(t, got)
//         return
//     }
//
//     expected := &placement.Bundle{
//         ID:       fmt.Sprintf("TiDB_DDL_%d", def.ID),
//         Index:    placement.RuleIndexPartition,
//         Override: true,
//         Rules:    s.expectedRules(t, def.PlacementPolicyRef),
//     }
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for idx, rule := range expected.Rules {
//         rule.GroupID = expected.ID
//         rule.Index = placement.RuleIndexTable
//         rule.ID = fmt.Sprintf("partition_rule_%d_%d", def.ID, idx)
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//         rule.StartKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(def.ID)))
//         rule.EndKeyHex = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(def.ID+1)))
//     }
//
//     s.checkTwoJSONObjectEquals(t, expected, got)
// }
//
// expectedRules 对应 Go 方法；原型：`func (s *metaBundleSuite) expectedRules(t *testing.T, ref *model.PolicyRefInfo) []*pd.Rule`。
// pub fn expected_rules() {
// 接收者和参数暂不转换为可编译 Rust 类型；下方保留 Go 方法体的业务检查顺序。
//     if ref == nil {
//         return []*pd.Rule{}
//     }
//
//     var policy *model.PolicyInfo
//     switch ref.ID {
//     case s.policy1.ID:
//         policy = s.policy1
//     case s.policy2.ID:
//         policy = s.policy2
//     case s.policy3.ID:
//         policy = s.policy3
//     default:
//         t.FailNow()
//     }
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, policy.Name, ref.Name)
//     settings := policy.PlacementSettings
//
//     bundle, err := placement.NewBundleFromOptions(settings)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//
//     return bundle.Rules
// }
// */
use super::*;

use std::collections::HashMap;

use meta_model::group_4::{PartitionInfo, PolicyRefInfo};

#[derive(Default)]
struct TestPolicyGetter {
    policies: HashMap<i64, model::PolicyInfo>,
}

impl PolicyGetter for TestPolicyGetter {
    fn GetPolicy(&self, policy_id: i64) -> Result<model::PolicyInfo, Error> {
        self.policies
            .get(&policy_id)
            .cloned()
            .ok_or_else(|| Error::new(format!("policy {policy_id} not found")))
    }
}

struct MetaBundleSuite {
    getter: TestPolicyGetter,
    table1: model::TableInfo,
    table2: model::TableInfo,
    table3: model::TableInfo,
    table4: model::TableInfo,
}

fn policy_ref(id: i64) -> PolicyRefInfo {
    PolicyRefInfo {
        ID: id,
        ..Default::default()
    }
}

fn partition(id: i64, policy_id: Option<i64>) -> model::PartitionDefinition {
    model::PartitionDefinition {
        ID: id,
        PlacementPolicyRef: policy_id.map(policy_ref),
        ..Default::default()
    }
}

fn create_meta_bundle_suite() -> MetaBundleSuite {
    let policies = [
        model::PolicyInfo {
            ID: 11,
            PlacementSettings: model::PlacementSettings {
                PrimaryRegion: "r1".into(),
                Regions: "r1,r2".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model::PolicyInfo {
            ID: 12,
            PlacementSettings: model::PlacementSettings {
                PrimaryRegion: "r2".into(),
                Regions: "r1,r2".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model::PolicyInfo {
            ID: 13,
            PlacementSettings: model::PlacementSettings {
                LeaderConstraints: "[+region=bj]".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    ]
    .into_iter()
    .map(|policy| (policy.ID, policy))
    .collect();

    MetaBundleSuite {
        getter: TestPolicyGetter { policies },
        table1: model::TableInfo {
            ID: 101,
            PlacementPolicyRef: Some(policy_ref(11)),
            Partition: Some(PartitionInfo {
                Definitions: vec![
                    partition(1000, None),
                    partition(1001, Some(12)),
                    partition(1002, None),
                ],
                ..Default::default()
            }),
            ..Default::default()
        },
        table2: model::TableInfo {
            ID: 102,
            Partition: Some(PartitionInfo {
                Definitions: vec![
                    partition(1000, Some(11)),
                    partition(1001, None),
                    partition(1002, None),
                ],
                ..Default::default()
            }),
            ..Default::default()
        },
        table3: model::TableInfo {
            ID: 103,
            PlacementPolicyRef: Some(policy_ref(13)),
            ..Default::default()
        },
        table4: model::TableInfo {
            ID: 104,
            ..Default::default()
        },
    }
}

fn encoded_table_prefix(id: i64) -> String {
    hex::encode(crate::codec::EncodeBytes(
        Vec::new(),
        crate::tablecodec::GenTablePrefix(id).as_ref(),
    ))
}

fn assert_rule_range(rule: &pd::Rule, id: i64, index: i32, group_id: &str) {
    assert_eq!(rule.GroupID, group_id);
    assert_eq!(rule.Index, index);
    assert_eq!(rule.StartKeyHex, encoded_table_prefix(id));
    assert_eq!(rule.EndKeyHex, encoded_table_prefix(id + 1));
}

fn expected_bundle(
    suite: &MetaBundleSuite,
    policy_id: i64,
    bundle_index: i32,
    ids: &[i64],
) -> Bundle {
    let policy = suite.getter.policies.get(&policy_id).unwrap();
    let base_rules = NewBundleFromOptions(Some(&policy.PlacementSettings))
        .unwrap()
        .unwrap()
        .Rules;
    let group_id = GroupID(ids[0]);
    let mut rules = Vec::with_capacity(base_rules.len() * ids.len());

    for (id_index, id) in ids.iter().copied().enumerate() {
        for (rule_index, base_rule) in base_rules.iter().enumerate() {
            let mut rule = base_rule.clone();
            let prefix = if bundle_index == RuleIndexPartition || id_index > 0 {
                "partition"
            } else {
                "table"
            };
            rule.ID = format!("{prefix}_rule_{id}_{rule_index}");
            rule.GroupID = group_id.clone();
            rule.Index = if id_index == 0 {
                RuleIndexTable
            } else {
                RuleIndexPartition
            };
            rule.StartKeyHex = encoded_table_prefix(id);
            rule.EndKeyHex = encoded_table_prefix(id + 1);
            rules.push(rule);
        }
    }

    Bundle {
        ID: group_id,
        Index: bundle_index,
        Override: true,
        Rules: rules,
    }
}

#[test]
fn new_table_bundle_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();

    let bundle = NewTableBundle(&suite.getter, &suite.table1)
        .unwrap()
        .unwrap();
    assert_eq!(
        bundle,
        expected_bundle(&suite, 11, RuleIndexTable, &[101, 1000, 1001, 1002])
    );
    assert_eq!(bundle.ID, GroupID(101));
    assert_eq!(bundle.Index, RuleIndexTable);
    assert!(bundle.Override);
    assert_eq!(bundle.Rules.len(), 12);
    for rule in &bundle.Rules[0..3] {
        assert_rule_range(rule, 101, RuleIndexTable, &bundle.ID);
    }
    for (rules, id) in bundle.Rules[3..].chunks(3).zip([1000, 1001, 1002]) {
        for rule in rules {
            assert_rule_range(rule, id, RuleIndexPartition, &bundle.ID);
        }
    }

    assert!(
        NewTableBundle(&suite.getter, &suite.table2)
            .unwrap()
            .is_none()
    );
    let table3 = NewTableBundle(&suite.getter, &suite.table3)
        .unwrap()
        .unwrap();
    assert_eq!(table3, expected_bundle(&suite, 13, RuleIndexTable, &[103]));
    assert_eq!(table3.ID, GroupID(103));
    assert_eq!(table3.Rules.len(), 2);
    assert!(
        NewTableBundle(&suite.getter, &suite.table4)
            .unwrap()
            .is_none()
    );
}

#[test]
fn new_partition_bundle_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let definitions = &suite.table1.Partition.as_ref().unwrap().Definitions;
    assert!(
        NewPartitionBundle(&suite.getter, &definitions[0])
            .unwrap()
            .is_none()
    );

    let bundle = NewPartitionBundle(&suite.getter, &definitions[1])
        .unwrap()
        .unwrap();
    assert_eq!(
        bundle,
        expected_bundle(&suite, 12, RuleIndexPartition, &[1001])
    );
    assert_eq!(bundle.ID, GroupID(1001));
    assert_eq!(bundle.Index, RuleIndexPartition);
    assert!(bundle.Override);
    assert_eq!(bundle.Rules.len(), 3);
    for rule in &bundle.Rules {
        assert_rule_range(rule, 1001, RuleIndexTable, &bundle.ID);
    }
}

#[test]
fn new_partition_list_bundles_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let definitions = &suite.table1.Partition.as_ref().unwrap().Definitions;
    let bundles = NewPartitionListBundles(&suite.getter, definitions).unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(
        bundles[0],
        expected_bundle(&suite, 12, RuleIndexPartition, &[1001])
    );
    assert!(
        NewPartitionListBundles(&suite.getter, &[])
            .unwrap()
            .is_empty()
    );
    assert!(
        NewPartitionListBundles(
            &suite.getter,
            &[definitions[0].clone(), definitions[2].clone()]
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn new_full_table_bundles_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let table1 = NewFullTableBundles(&suite.getter, &suite.table1).unwrap();
    assert_eq!(
        table1,
        vec![
            expected_bundle(&suite, 11, RuleIndexTable, &[101, 1000, 1001, 1002]),
            expected_bundle(&suite, 12, RuleIndexPartition, &[1001]),
        ]
    );

    let table2 = NewFullTableBundles(&suite.getter, &suite.table2).unwrap();
    assert_eq!(
        table2,
        vec![expected_bundle(&suite, 11, RuleIndexPartition, &[1000])]
    );

    let table3 = NewFullTableBundles(&suite.getter, &suite.table3).unwrap();
    assert_eq!(
        table3,
        vec![expected_bundle(&suite, 13, RuleIndexTable, &[103])]
    );

    assert!(
        NewFullTableBundles(&suite.getter, &suite.table4)
            .unwrap()
            .is_empty()
    );
}
