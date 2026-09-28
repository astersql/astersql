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
// 对应表或分区的 Placement Policy。本文件保留从 Go 迁移的表驱动用例形状，
// 并提供可运行的 Rust 回归：校验 Bundle 创建、Reset（按对象 ID 重置
// key 范围）、ObjectID 解析与 JSON 序列化的规范化行为。

// 覆盖 Bundle 判空、克隆、对象 ID、leader DC 提取、字符串化、placement options 构造、Reset/Tidy 以及 range key 编解码测试。
//
// #![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables, unused_mut)]
//
// #[test]
// pub fn test_empty() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     bundle := &Bundle{ID: GroupID(1)}
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.True(t, bundle.IsEmpty())
//
//     bundle = &Bundle{ID: GroupID(1), Index: 1}
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.False(t, bundle.IsEmpty())
//
//     bundle = &Bundle{ID: GroupID(1), Override: true}
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.False(t, bundle.IsEmpty())
//
//     bundle = &Bundle{ID: GroupID(1), Rules: []*pd.Rule{{ID: "434"}}}
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.False(t, bundle.IsEmpty())
//
//     bundle = &Bundle{ID: GroupID(1), Index: 1, Override: true}
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.False(t, bundle.IsEmpty())
// }
//
// #[test]
// pub fn test_clone_bundle() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     bundle := &Bundle{ID: GroupID(1), Rules: []*pd.Rule{{ID: "434"}}}
//
//     newBundle := bundle.Clone()
//     newBundle.ID = GroupID(2)
//     newBundle.Rules[0] = &pd.Rule{ID: "121"}
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, &Bundle{ID: GroupID(1), Rules: []*pd.Rule{{ID: "434"}}}, bundle)
//     require.Equal(t, &Bundle{ID: GroupID(2), Rules: []*pd.Rule{{ID: "121"}}}, newBundle)
// }
//
// #[test]
// pub fn test_object_id() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// TestCase 对应 Go 测试辅助结构，字段顺序按来源文件保留。
// pub struct TestCase {
//         name       string
//         bundleID   string
//         expectedID int64
//         err        error
//     }
// 表驱动用例保留 Go 的输入、期望输出和期望错误，方便后续逐项恢复为可运行 Rust 测试。
//     tests := []TestCase{
//         {"non tidb bundle", "pd", 0, ErrInvalidBundleIDFormat},
//         {"id of words", "TiDB_DDL_foo", 0, ErrInvalidBundleID},
//         {"id of words and nums", "TiDB_DDL_3x", 0, ErrInvalidBundleID},
//         {"id of floats", "TiDB_DDL_3.0", 0, ErrInvalidBundleID},
//         {"id of negatives", "TiDB_DDL_-10", 0, ErrInvalidBundleID},
//         {"id of positive integer", "TiDB_DDL_10", 10, nil},
//     }
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for _, test := range tests {
//         bundle := Bundle{ID: test.bundleID}
//         id, err := bundle.ObjectID()
// Go 表驱动按期望错误分支拆断言；Rust 版本应对应 Result 的 Ok/Err 分支。
//         if test.err == nil {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//             require.NoError(t, err, test.name)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//             require.Equal(t, test.expectedID, id, test.name)
//         } else {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//             require.ErrorIs(t, err, test.err, test.name)
//         }
//     }
// }
//
// #[test]
// pub fn test_get_leader_dc_by_bundle() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// 表驱动用例保留 Go 的输入、期望输出和期望错误，方便后续逐项恢复为可运行 Rust 测试。
//     testcases := []struct {
//         name       string
//         bundle     *Bundle
//         expectedDC string
//     }{
//         {
//             name: "only leader",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "12",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"bj"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "bj",
//         },
//         {
//             name: "no leader",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "12",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"bj"},
//                             },
//                         },
//                         Count: 3,
//                     },
//                 },
//             },
//             expectedDC: "",
//         },
//         {
//             name: "voter and leader",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "11",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"sh"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                     {
//                         ID:   "12",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"bj"},
//                             },
//                         },
//                         Count: 3,
//                     },
//                 },
//             },
//             expectedDC: "sh",
//         },
//         {
//             name: "wrong label key",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "11",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "fake",
//                                 Op:     pd.In,
//                                 Values: []string{"sh"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "",
//         },
//         {
//             name: "wrong operator",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "11",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.NotIn,
//                                 Values: []string{"sh"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "",
//         },
//         {
//             name: "leader have multi values",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "11",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"sh", "bj"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "",
//         },
//         {
//             name: "irrelvant rules",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "15",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    EngineLabelKey,
//                                 Op:     pd.NotIn,
//                                 Values: []string{EngineLabelTiFlash},
//                             },
//                         },
//                         Count: 1,
//                     },
//                     {
//                         ID:   "14",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "disk",
//                                 Op:     pd.NotIn,
//                                 Values: []string{"ssd", "hdd"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                     {
//                         ID:   "13",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"bj"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "bj",
//         },
//         {
//             name: "multi leaders 1",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "16",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"sh"},
//                             },
//                         },
//                         Count: 2,
//                     },
//                 },
//             },
//             expectedDC: "",
//         },
//         {
//             name: "multi leaders 2",
//             bundle: &Bundle{
//                 ID: GroupID(1),
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "17",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"sh"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                     {
//                         ID:   "18",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {
//                                 Key:    "zone",
//                                 Op:     pd.In,
//                                 Values: []string{"bj"},
//                             },
//                         },
//                         Count: 1,
//                     },
//                 },
//             },
//             expectedDC: "sh",
//         },
//     }
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for _, testcase := range testcases {
//         result, ok := testcase.bundle.GetLeaderDC("zone")
//         if len(testcase.expectedDC) > 0 {
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//             require.True(t, ok, testcase.name)
//         } else {
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//             require.False(t, ok, testcase.name)
//         }
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Equal(t, testcase.expectedDC, result, testcase.name)
//     }
// }
//
// #[test]
// pub fn test_string() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     bundle := &Bundle{
//         ID: GroupID(1),
//     }
//
//     rules1, err := newRules(pd.Voter, 3, `["+zone=sh", "+zone=sh"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     rules2, err := newRules(pd.Voter, 4, `["-zone=sh", "+zone=bj"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     rules3, err := newRules(pd.Voter, 3, `["-engine=tiflash", "-engine=tiflash_compute"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     bundle.Rules = append(append(rules1, rules2...), rules3...)
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, "{\"group_id\":\"TiDB_DDL_1\",\"group_index\":0,\"group_override\":false,\"rules\":[{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":3,\"label_constraints\":[{\"key\":\"zone\",\"op\":\"in\",\"values\":[\"sh\"]}]},{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":4,\"label_constraints\":[{\"key\":\"zone\",\"op\":\"notIn\",\"values\":[\"sh\"]},{\"key\":\"zone\",\"op\":\"in\",\"values\":[\"bj\"]}]},{\"group_id\":\"\",\"id\":\"\",\"start_key\":\"\",\"end_key\":\"\",\"role\":\"voter\",\"is_witness\":false,\"count\":3,\"label_constraints\":[{\"key\":\"engine\",\"op\":\"notIn\",\"values\":[\"tiflash\"]},{\"key\":\"engine\",\"op\":\"notIn\",\"values\":[\"tiflash_compute\"]}]}]}", bundle.String())
//
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/placement/MockMarshalFailure", `return(true)`))
// defer 用于资源收尾；Rust 接线时应改成 Drop/作用域或显式 close 校验。
//     defer func() {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/placement/MockMarshalFailure"))
//     }()
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, "", bundle.String())
// }
//
// #[test]
// pub fn test_new_bundle() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, &Bundle{ID: GroupID(3)}, NewBundle(3))
//     require.Equal(t, &Bundle{ID: GroupID(-1)}, NewBundle(-1))
//     _, err := NewBundleFromConstraintsOptions(nil)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.Error(t, err)
//     _, err = NewBundleFromSugarOptions(nil)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.Error(t, err)
//     _, err = NewBundleFromOptions(nil)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.Error(t, err)
// }
//
// #[test]
// pub fn test_new_bundle_from_options() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// TestCase 对应 Go 测试辅助结构，字段顺序按来源文件保留。
// pub struct TestCase {
//         name   string
//         input  *model.PlacementSettings
//         output []*pd.Rule
//         err    error
//     }
// 表驱动用例保留 Go 的输入、期望输出和期望错误，方便后续逐项恢复为可运行 Rust 测试。
//     var tests []TestCase
//
//     tests = append(tests, TestCase{
//         name:  "empty 1",
//         input: &model.PlacementSettings{},
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 3, NewConstraintsDirect()),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name:  "empty 2",
//         input: nil,
//         err:   ErrInvalidPlacementOptions,
//     })
//
//     tests = append(tests, TestCase{
//         name: "empty 3",
//         input: &model.PlacementSettings{
//             LearnerConstraints: "[+region=us]",
//         },
//         err: ErrInvalidConstraintsReplicas,
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: normal case 1",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "us",
//             Regions:       "us",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: normal case 2",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "us",
//             Regions:       "us",
//             Schedule:      "majority_in_primary",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 1, NewConstraintsDirect()),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: few followers",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "us",
//             Regions:       "bj,sh,us",
//             Followers:     1,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "bj", "sh"),
//             )),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: omit regions 1",
//         input: &model.PlacementSettings{
//             Followers: 2,
//             Schedule:  "even",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 3, NewConstraintsDirect()),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: omit regions 2",
//         input: &model.PlacementSettings{
//             Followers: 2,
//             Schedule:  "majority_in_primary",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 3, NewConstraintsDirect()),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: wrong schedule prop",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "us",
//             Regions:       "us",
//             Schedule:      "wrong",
//         },
//         err: ErrInvalidPlacementOptions,
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: invalid region name 1",
//         input: &model.PlacementSettings{
//             PrimaryRegion: ",=,",
//             Regions:       ",=,",
//         },
//         err: ErrInvalidPlacementOptions,
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: invalid region name 2",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "f",
//             Regions:       ",=",
//         },
//         err: ErrInvalidPlacementOptions,
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: invalid region name 4",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "",
//             Regions:       "g",
//         },
//         err: ErrInvalidPlacementOptions,
//     })
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: normal case 2",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "us",
//             Regions:       "sh,us",
//             Followers:     5,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 3, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "sh"),
//             )),
//         },
//     })
//     tests = append(tests, tests[len(tests)-1])
//     tests[len(tests)-1].name = "sugar syntax: explicit schedule"
//     tests[len(tests)-1].input.Schedule = "even"
//
//     tests = append(tests, TestCase{
//         name: "sugar syntax: majority schedule",
//         input: &model.PlacementSettings{
//             PrimaryRegion: "sh",
//             Regions:       "bj,sh",
//             Followers:     4,
//             Schedule:      "majority_in_primary",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "sh"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "sh"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "bj"),
//             )),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: normal case 1",
//         input: &model.PlacementSettings{
//             Constraints: "[+region=us]",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: normal case 3",
//         input: &model.PlacementSettings{
//             Constraints: "[+region=us]",
//             Followers:   2,
//             Learners:    2,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//             NewRule(pd.Learner, 2, NewConstraintsDirect(
//                 NewConstraintDirect("region", pd.In, "us"),
//             )),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: only leader constraints",
//         input: &model.PlacementSettings{
//             LeaderConstraints: "[+region=as]",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "as"))),
//             NewRule(pd.Voter, 2, NewConstraintsDirect()),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: only leader constraints",
//         input: &model.PlacementSettings{
//             LeaderConstraints: "[+region=as]",
//             Followers:         4,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "as"))),
//             NewRule(pd.Voter, 4, NewConstraintsDirect()),
//         },
//     })
//     tests = append(tests, TestCase{
//         name: "direct syntax: leader and follower constraints",
//         input: &model.PlacementSettings{
//             LeaderConstraints:   "[+region=as]",
//             FollowerConstraints: `{"+region=us": 2}`,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "as"))),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: lack count 1",
//         input: &model.PlacementSettings{
//             LeaderConstraints:   "[+region=as]",
//             FollowerConstraints: "[-region=us]",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "as"))),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.NotIn, "us"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: lack count 2",
//         input: &model.PlacementSettings{
//             LeaderConstraints:  "[+region=as]",
//             LearnerConstraints: "[-region=us]",
//         },
//         err: ErrInvalidConstraintsReplicas,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: omit leader",
//         input: &model.PlacementSettings{
//             Followers:           2,
//             FollowerConstraints: "[+region=bj]",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect()),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "bj"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: conflicts 1",
//         input: &model.PlacementSettings{
//             Constraints:       "[+region=us]",
//             LeaderConstraints: "[-region=us]",
//             Followers:         2,
//         },
//         err: ErrConflictingConstraints,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: conflicts 3",
//         input: &model.PlacementSettings{
//             Constraints:         "[+region=us]",
//             FollowerConstraints: "[-region=us]",
//             Followers:           2,
//         },
//         err: ErrConflictingConstraints,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: conflicts 4",
//         input: &model.PlacementSettings{
//             Constraints:        "[+region=us]",
//             LearnerConstraints: "[-region=us]",
//             Followers:          2,
//             Learners:           2,
//         },
//         err: ErrConflictingConstraints,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: invalid format 1",
//         input: &model.PlacementSettings{
//             Constraints:       "[+region=us]",
//             LeaderConstraints: "-region=us]",
//             Followers:         2,
//         },
//         err: ErrInvalidConstraintsFormat,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: invalid format 2",
//         input: &model.PlacementSettings{
//             Constraints:       "+region=us]",
//             LeaderConstraints: "[-region=us]",
//             Followers:         2,
//         },
//         err: ErrInvalidConstraintsFormat,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: invalid format 4",
//         input: &model.PlacementSettings{
//             Constraints:         "[+region=us]",
//             FollowerConstraints: "-region=us]",
//             Followers:           2,
//         },
//         err: ErrInvalidConstraintsFormat,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: invalid format 5",
//         input: &model.PlacementSettings{
//             Constraints:       "[+region=us]",
//             LeaderConstraints: "-region=us]",
//             Learners:          2,
//             Followers:         2,
//         },
//         err: ErrInvalidConstraintsFormat,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: follower dict constraints",
//         input: &model.PlacementSettings{
//             FollowerConstraints: "{+disk=ssd: 1}",
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect()),
//             NewRule(pd.Voter, 1, NewConstraintsDirect(NewConstraintDirect("disk", pd.In, "ssd"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: invalid follower dict constraints",
//         input: &model.PlacementSettings{
//             FollowerConstraints: "{+disk=ssd: 1}",
//             Followers:           2,
//         },
//         err: ErrInvalidConstraintsReplicas,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: learner dict constraints",
//         input: &model.PlacementSettings{
//             LearnerConstraints: `{"+region=us": 2}`,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Leader, 1, NewConstraintsDirect()),
//             NewRule(pd.Voter, 2, NewConstraintsDirect()),
//             NewRule(pd.Learner, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: learner dict constraints, with count",
//         input: &model.PlacementSettings{
//             LearnerConstraints: `{"+region=us": 2}`,
//             Learners:           4,
//         },
//         err: ErrInvalidConstraintsReplicas,
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: dict constraints",
//         input: &model.PlacementSettings{
//             Constraints: `{"+region=us": 3}`,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 3, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: dict constraints, 2:2:1",
//         input: &model.PlacementSettings{
//             Constraints: `{ "+region=us-east-1":2, "+region=us-east-2": 2, "+region=us-west-1": 1}`,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us-east-1"))),
//             NewRule(pd.Voter, 2, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us-east-2"))),
//             NewRule(pd.Voter, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us-west-1"))),
//         },
//     })
//
//     tests = append(tests, TestCase{
//         name: "direct syntax: dict constraints",
//         input: &model.PlacementSettings{
//             Constraints:        `{"+region=us-east": 3}`,
//             LearnerConstraints: `{"+region=us-west": 1}`,
//         },
//         output: []*pd.Rule{
//             NewRule(pd.Voter, 3, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us-east"))),
//             NewRule(pd.Learner, 1, NewConstraintsDirect(NewConstraintDirect("region", pd.In, "us-west"))),
//         },
//     })
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for _, test := range tests {
//         bundle, err := newBundleFromOptions(test.input)
//         comment := fmt.Sprintf("[%s]\nerr1 %s\nerr2 %s", test.name, err, test.err)
// Go 表驱动按期望错误分支拆断言；Rust 版本应对应 Result 的 Ok/Err 分支。
//         if test.err != nil {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//             require.ErrorIs(t, err, test.err, comment)
//         } else {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//             require.NoError(t, err, comment)
//             matchRules(test.output, bundle.Rules, comment, t)
//         }
//     }
// }
//
// #[test]
// pub fn test_reset_bundle_with_single_rule() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     bundle := &Bundle{
//         ID: GroupID(1),
//     }
//
//     rules, err := newRules(pd.Voter, 3, `["+zone=sh", "+zone=sh"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     bundle.Rules = rules
//
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     bundle.Reset(RuleIndexTable, []int64{3})
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, GroupID(3), bundle.ID)
//     require.Equal(t, true, bundle.Override)
//     require.Equal(t, RuleIndexTable, bundle.Index)
//     require.Len(t, bundle.Rules, 1)
//     require.Equal(t, bundle.ID, bundle.Rules[0].GroupID)
//
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey := hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(3)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[0].StartKeyHex)
//
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     endKey := hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(4)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, endKey, bundle.Rules[0].EndKeyHex)
// }
//
// #[test]
// pub fn test_reset_bundle_with_multi_rules() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// build a bundle with three rules.
//     bundle, err := NewBundleFromOptions(&model.PlacementSettings{
//         LeaderConstraints:   `["+zone=bj"]`,
//         Followers:           2,
//         FollowerConstraints: `["+zone=hz"]`,
//         Learners:            1,
//         LearnerConstraints:  `["+zone=cd"]`,
//         Constraints:         `["+disk=ssd"]`,
//     })
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, 3, len(bundle.Rules))
//
// test if all the three rules are basic rules even the start key are not set.
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     bundle.Reset(RuleIndexTable, []int64{1, 2, 3})
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, GroupID(1), bundle.ID)
//     require.Equal(t, RuleIndexTable, bundle.Index)
//     require.Equal(t, true, bundle.Override)
//     require.Equal(t, 3*3, len(bundle.Rules))
// for id 1.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey := hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(1)))
//     endKey := hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(2)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[0].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[0].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[1].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[1].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[2].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[2].EndKeyHex)
// for id 2.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(2)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(3)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[3].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[3].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[4].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[4].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[5].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[5].EndKeyHex)
// for id 3.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(3)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(4)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[6].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[6].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[7].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[7].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[8].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[8].EndKeyHex)
//
// test if bundle has redundant rules.
// for now, the bundle has 9 rules, each table id or partition id has the three with them.
// once we reset this bundle for another ids, for example, adding partitions. we should
// extend the basic rules(3 of them) to the new partition id.
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     bundle.Reset(RuleIndexTable, []int64{1, 3, 4, 5})
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, GroupID(1), bundle.ID)
//     require.Equal(t, RuleIndexTable, bundle.Index)
//     require.Equal(t, true, bundle.Override)
//     require.Equal(t, 3*4, len(bundle.Rules))
// for id 1.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(1)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(2)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[0].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[0].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[1].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[1].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[2].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[2].EndKeyHex)
// for id 3.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(3)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(4)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[3].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[3].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[4].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[4].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[5].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[5].EndKeyHex)
// for id 4.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(4)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(5)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[6].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[6].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[7].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[7].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[8].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[8].EndKeyHex)
// for id 5.
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     startKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(5)))
//     endKey = hex.EncodeToString(codec.EncodeBytes(nil, tablecodec.GenTablePrefix(6)))
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, startKey, bundle.Rules[9].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[9].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[10].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[10].EndKeyHex)
//     require.Equal(t, startKey, bundle.Rules[11].StartKeyHex)
//     require.Equal(t, endKey, bundle.Rules[11].EndKeyHex)
// }
//
// #[test]
// pub fn test_tidy() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     bundle := &Bundle{
//         ID: GroupID(1),
//     }
//
//     rules0, err := newRules(pd.Voter, 1, `["+zone=sh", "+zone=sh"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, rules0, 1)
//     rules0[0].Count = 0 // test prune useless rules
//
//     rules1, err := newRules(pd.Voter, 4, `["-zone=sh", "+zone=bj"]`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, rules1, 1)
//     rules2, err := newRules(pd.Voter, 0, `{"-zone=sh,+zone=bj": 4}}`)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//     bundle.Rules = append(bundle.Rules, rules0...)
//     bundle.Rules = append(bundle.Rules, rules1...)
//     bundle.Rules = append(bundle.Rules, rules2...)
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, bundle.Rules, 3)
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     err = bundle.Tidy()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, bundle.Rules, 1)
//     require.Equal(t, "0", bundle.Rules[0].ID)
//     require.Len(t, bundle.Rules[0].LabelConstraints, 2)
//
// merge
//     rules3, err := newRules(pd.Follower, 4, "")
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, rules3, 1)
//
//     rules4, err := newRules(pd.Follower, 5, "")
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Len(t, rules4, 1)
//
//     rules0[0].Role = pd.Voter
//     bundle.Rules = append(bundle.Rules, rules0...)
//     bundle.Rules = append(bundle.Rules, rules3...)
//     bundle.Rules = append(bundle.Rules, rules4...)
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for _, r := range bundle.Rules {
//         r.LocationLabels = []string{"zone", "host"}
//     }
//     chkfunc := func() {
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//         require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//         require.Len(t, bundle.Rules, 2)
//         require.Equal(t, "0", bundle.Rules[0].ID)
//         require.Equal(t, "1", bundle.Rules[1].ID)
//         require.Equal(t, 9, bundle.Rules[1].Count)
//         require.Equal(t, 0, len(bundle.Rules[1].LabelConstraints))
//         require.Equal(t, []string{"zone", "host"}, bundle.Rules[1].LocationLabels)
//     }
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     err = bundle.Tidy()
//     chkfunc()
//
// tidy again
// it should be stable
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//     err = bundle.Tidy()
//     chkfunc()
//
// tidy again
// it should be stable
//     bundle2 := bundle.Clone()
//     err = bundle2.Tidy()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.Equal(t, bundle, bundle2)
// }
//
// #[test]
// pub fn test_tidy2() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
// 表驱动用例保留 Go 的输入、期望输出和期望错误，方便后续逐项恢复为可运行 Rust 测试。
//     tests := []struct {
//         name     string
//         bundle   Bundle
//         expected Bundle
//     }{
//         {
//             name: "Empty bundle",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{},
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{},
//             },
//         },
//         {
//             name: "Rules with empty constraints are merged",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:               "1",
//                         Role:             pd.Leader,
//                         Count:            1,
//                         LabelConstraints: []pd.LabelConstraint{},
//                         LocationLabels:   []string{"region"},
//                     },
//                     {
//                         ID:               "2",
//                         Role:             pd.Voter,
//                         Count:            2,
//                         LabelConstraints: []pd.LabelConstraint{},
//                         LocationLabels:   []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:               "0",
//                         Role:             pd.Voter,
//                         Count:            3,
//                         LabelConstraints: []pd.LabelConstraint{},
//                         LocationLabels:   []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints are merged, Leader + Follower",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          2,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          3,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints are merged, Leader + Voter",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          2,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          3,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints and role are merged,  Leader + Follower + Voter",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          3,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints and role are merged,  Leader + Follower + Voter + Learner",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "4",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          2,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          3,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          2,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints and role are merged,  Leader + Follower + Learner | Follower",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "4",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          2,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with same constraints and role are merged,  Leader + Follower + Learner | Voter",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "4",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "1",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Learner,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "3",
//                         Role: pd.Voter,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//         {
//             name: "Rules with different constraints are kept separate",
//             bundle: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "1",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "2",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//             expected: Bundle{
//                 Rules: []*pd.Rule{
//                     {
//                         ID:   "0",
//                         Role: pd.Leader,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"1"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                     {
//                         ID:   "1",
//                         Role: pd.Follower,
//                         LabelConstraints: []pd.LabelConstraint{
//                             {Op: pd.In, Key: "rack", Values: []string{"2"}},
//                         },
//                         Count:          1,
//                         LocationLabels: []string{"region"},
//                     },
//                 },
//             },
//         },
//     }
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//     for _, tt := range tests {
//         t.Run(tt.name, func(t *testing.T) {
// 这里验证 Bundle 规则重写/整理的副作用；保留调用位置和后续断言。
//             err := tt.bundle.Tidy()
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//             require.NoError(t, err)
//
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//             require.Equal(t, len(tt.expected.Rules), len(tt.bundle.Rules))
//
// Go range 循环保留为形状；后续接线时需要改成 Rust iterator/enumerate。
//             for i, rule := range tt.bundle.Rules {
//                 expectedRule := tt.expected.Rules[i]
//                 if !reflect.DeepEqual(rule, expectedRule) {
//                     t.Errorf("unexpected rule at index %d:\nactual=%#v,\nexpected=%#v\n", i, rule, expectedRule)
//                 }
//             }
//         })
//     }
// }
//
// #[test]
// pub fn test_get_range_start_and_end_key_hex() {
// Go 的 t 参数由测试框架传入；这里保留 require/assert 调用形状作为人工迁移参考。
//     startKey, endKey := GetRangeStartAndEndKeyHex(TiDBBundleRangePrefixForMeta)
//
// Check that startKey is properly encoded in table mode
//     startKeyBytes, err := hex.DecodeString(startKey)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
//
// Both keys should be valid codec encoded bytes
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     _, startKeyDecoded, err := codec.DecodeBytes(startKeyBytes, nil)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.True(t, bytes.Equal(metaPrefix, startKeyDecoded), "metaPrefix and startKeyDecoded should have the same content")
//
//     endKeyBytes, err := hex.DecodeString(endKey)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// tablecodec/codec 编解码决定 rule key 边界；迁移时需保持字节前缀与十六进制编码一致。
//     _, endKeyDecoded, err := codec.DecodeBytes(endKeyBytes, nil)
// require 断言保留 Go 测试语义：成功路径校验无错，错误路径校验具体哨兵错误。
//     require.NoError(t, err)
// require 断言保留期望值比较；当前不替换为 Rust assert_eq!/assert!。
//     require.True(t, bytes.Equal(tablecodec.GenTablePrefix(0), endKeyDecoded), "tablePrefix and endKeyDecoded should have the same content")
// }
// */
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
