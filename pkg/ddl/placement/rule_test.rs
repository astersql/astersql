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

// Placement Rule 构建与解析的单元测试。
//
// 覆盖 `RuleBuilder` 在字典约束下的副本数汇总、以及 `#evict-leader` 属性将角色降为
// Follower（跟随者副本，不参与 Leader 选举）的行为。文件前半保留 Go 侧测试草稿注释，
// 便于对照迁移用例（克隆、列表/字典约束、错误码匹配）。

// 职责：placement 规则解析测试，覆盖 Rule 克隆、约束列表/映射解析和错误匹配。
// 中文注释按测试入口、辅助函数、断言、资源收尾、事务、failpoint 和外部 IO 位置补充，供后续人工迁移使用。
//
// #![allow(dead_code, unused_variables, non_snake_case)]
//
// go_step 用于承载原 Go 语句文本，避免误执行数据库、事务或外部依赖动作。
// #[allow(dead_code)]
// fn go_step(_source: &str) {}
//
// TestClone 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestClone(t *testing.T) {
// 参数语义: t *testing.T
// #[test]
// #[allow(non_snake_case, unused_variables, dead_code)]
// fn test_clone_go_draft() {

// 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
// Go: rule := &pd.Rule{ID: "434"}
// Go: newRule := rule.Clone()
// Go: newRule.ID = "121"
// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.Equal(t, &pd.Rule{ID: "434"}, rule)
// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.Equal(t, &pd.Rule{ID: "121"}, newRule)
// }
//
// matchRules 对应 Go 测试辅助函数；参数和返回值语义见下方 Go 签名。
// Go 签名: func matchRules(t1, t2 []*pd.Rule, prefix string, t *testing.T) {
// 参数语义: t1, t2 []*pd.Rule, prefix string, t *testing.T
// #[allow(non_snake_case, unused_variables, dead_code)]
// fn match_rules_go_draft() {

// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.Equal(t, len(t2), len(t1), prefix)
// 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
// Go: for i := range t1 {
// Go: found := false
// 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
// Go: for j := range t2 {
// Go: ok := reflect.DeepEqual(t2[j], t1[i])
// 条件分支保留 Go 的错误处理或状态判断语义。
// Go: if ok {
// Go: found = true
// Go: break
// Go: }
// Go: }
// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.True(t, found, "%s\n\ncan not found %d rule\n%+v\n%+v", prefix, i, t1[i], t2)
// Go: }
// }
// TestNewRuleAndNewRules 对应 Go 测试函数；按原顺序保留 testkit、require 和外部依赖调用。
// Go 签名: func TestNewRuleAndNewRules(t *testing.T) {
// 参数语义: t *testing.T
// #[test]
// #[allow(non_snake_case, unused_variables, dead_code)]
// fn test_new_rule_and_new_rules_go_draft() {

// 测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
// Go: type TestCase struct {
// Go: name string
// Go: input string
// Go: replicas uint64
// Go: output []*pd.Rule
// Go: err error
// Go: }
// Go: var tests []TestCase
// Go: tests = append(tests, TestCase{
// Go: name: "empty constraints",
// Go: input: "",
// Go: replicas: 3,
// Go: output: []*pd.Rule{
// Go: NewRule(pd.Voter, 3, NewConstraintsDirect()),
// Go: },
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "zero replicas",
// Go: input: "",
// Go: replicas: 0,
// Go: output: nil,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "normal list constraints",
// Go: input: `["+zone=sh", "+region=sh"]`,
// Go: replicas: 3,
// Go: output: []*pd.Rule{
// Go: NewRule(pd.Voter, 3, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: NewConstraintDirect("region", pd.In, "sh"),
// Go: )),
// Go: },
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "normal dict constraints",
// Go: input: `{"+zone=sh,-zone=bj":2, "+zone=sh": 1}`,
// Go: output: []*pd.Rule{
// Go: NewRule(pd.Voter, 2, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: NewConstraintDirect("zone", pd.NotIn, "bj"),
// Go: )),
// Go: NewRule(pd.Voter, 1, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: )),
// Go: },
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "normal dict constraints, with count",
// Go: input: "{'+zone=sh,-zone=bj':2, '+zone=sh': 1}",
// Go: output: []*pd.Rule{
// Go: NewRule(pd.Voter, 2, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: NewConstraintDirect("zone", pd.NotIn, "bj"),
// Go: )),
// Go: NewRule(pd.Voter, 1, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: )),
// Go: },
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "zero count in dict constraints",
// Go: input: `{"+zone=sh,-zone=bj":0, "+zone=sh": 1}`,
// Go: err: ErrInvalidConstraintsMapcnt,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid list constraints",
// Go: input: `["ne=sh", "+zone=sh"]`,
// Go: replicas: 3,
// Go: err: ErrInvalidConstraintsFormat,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid dict constraints",
// Go: input: `{+ne=sh,-zone=bj:1, "+zone=sh": 4`,
// Go: err: ErrInvalidConstraintsFormat,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid dict constraints",
// Go: input: `{"nesh,-zone=bj":1, "+zone=sh": 4}`,
// Go: err: ErrInvalidConstraintFormat,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid dict separator",
// Go: input: `{+region=us-east-2:2}`,
// Go: err: ErrInvalidConstraintsMappingWrongSeparator,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "normal dict constraint with evict leader attribute",
// Go: input: `{"+zone=sh,-zone=bj":2, "+zone=sh,#evict-leader": 1}`,
// Go: output: []*pd.Rule{
// Go: NewRule(pd.Voter, 2, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: NewConstraintDirect("zone", pd.NotIn, "bj"),
// Go: )),
// Go: NewRule(pd.Follower, 1, NewConstraintsDirect(
// Go: NewConstraintDirect("zone", pd.In, "sh"),
// Go: )),
// Go: },
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid constraints with invalid format",
// Go: input: `{"+zone=sh,-zone=bj":2, "+zone=sh,evict-leader": 1}`,
// Go: err: ErrInvalidConstraintFormat,
// Go: })
// Go: tests = append(tests, TestCase{
// Go: name: "invalid constraints with undetermined attribute",
// Go: input: `{"+zone=sh,-zone=bj":2, "+zone=sh,#reject-follower": 1}`,
// Go: err: ErrUnsupportedConstraint,
// Go: })
// 循环保持 Go 的遍历/批量构造语义，尤其是测试用例、分区或批量 INSERT。
// Go: for _, tt := range tests {
// Go: comment := fmt.Sprintf("[%s]", tt.name)
// Go: output, err := newRules(pd.Voter, tt.replicas, tt.input)
// 条件分支保留 Go 的错误处理或状态判断语义。
// Go: if tt.err == nil {
// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.NoError(t, err, comment)
// Go: matchRules(tt.output, output, comment, t)
// Go: } else {
// require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
// Go: require.True(t, errors.Is(err, tt.err), "[%s]\n%s\n%s\n", tt.name, err, tt.err)
// Go: }
// Go: }
// }
// */
use super::*;

/// 验证字典约束下副本数汇总为 3，且 `#evict-leader` 使对应规则角色变为 Follower。
fn direct(key: &str, op: pd::LabelConstraintOp, values: &[&str]) -> pd::LabelConstraint {
    NewConstraintDirect(
        key,
        op,
        values.iter().map(|value| (*value).to_owned()).collect(),
    )
}

fn assert_rule_sets(mut actual: Vec<Box<pd::Rule>>, mut expected: Vec<Box<pd::Rule>>, name: &str) {
    let canonical = |rule: &Box<pd::Rule>| {
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

fn build(role: pd::PeerRoleType, replicas: u64, input: &str) -> Result<Vec<Box<pd::Rule>>, Error> {
    let mut builder = NewRuleBuilder();
    builder
        .SetRole(role)
        .SetReplicasNum(replicas)
        .SetConstraintStr(input.into())
        .BuildRules()
}

#[test]
fn rule_clone_is_independent() {
    let original = pd::Rule {
        ID: "434".into(),
        ..Default::default()
    };
    let mut cloned = original.clone();
    cloned.ID = "121".into();
    assert_eq!(original.ID, "434");
    assert_eq!(cloned.ID, "121");
}

#[test]
fn new_rule_and_new_rules_match_go_table() {
    let cases = vec![
        (
            "empty constraints",
            "",
            3,
            Some(vec![NewRule(pd::Voter, 3, vec![])]),
            None,
        ),
        ("zero replicas", "", 0, Some(vec![]), None),
        (
            "normal list constraints",
            r#"["+zone=sh", "+region=sh"]"#,
            3,
            Some(vec![NewRule(
                pd::Voter,
                3,
                vec![
                    direct("zone", pd::In, &["sh"]),
                    direct("region", pd::In, &["sh"]),
                ],
            )]),
            None,
        ),
        (
            "normal dict constraints",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh": 1}"#,
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Voter, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "normal dict constraints, with count",
            "{'+zone=sh,-zone=bj':2, '+zone=sh': 1}",
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Voter, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "zero count in dict constraints",
            r#"{"+zone=sh,-zone=bj":0, "+zone=sh": 1}"#,
            0,
            None,
            Some(ErrInvalidConstraintsMapcnt),
        ),
        (
            "invalid list constraints",
            r#"["ne=sh", "+zone=sh"]"#,
            3,
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "invalid dict syntax",
            r#"{+ne=sh,-zone=bj:1, "+zone=sh": 4"#,
            0,
            None,
            Some(ErrInvalidConstraintsFormat),
        ),
        (
            "invalid constraint in dict",
            r#"{"nesh,-zone=bj":1, "+zone=sh": 4}"#,
            0,
            None,
            Some(ErrInvalidConstraintFormat),
        ),
        (
            "invalid dict separator",
            "{+region=us-east-2:2}",
            0,
            None,
            Some(ErrInvalidConstraintsMappingWrongSeparator),
        ),
        (
            "evict leader attribute",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,#evict-leader": 1}"#,
            0,
            Some(vec![
                NewRule(
                    pd::Voter,
                    2,
                    vec![
                        direct("zone", pd::In, &["sh"]),
                        direct("zone", pd::NotIn, &["bj"]),
                    ],
                ),
                NewRule(pd::Follower, 1, vec![direct("zone", pd::In, &["sh"])]),
            ]),
            None,
        ),
        (
            "invalid attribute format",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,evict-leader": 1}"#,
            0,
            None,
            Some(ErrInvalidConstraintFormat),
        ),
        (
            "unsupported attribute",
            r#"{"+zone=sh,-zone=bj":2, "+zone=sh,#reject-follower": 1}"#,
            0,
            None,
            Some(ErrUnsupportedConstraint),
        ),
    ];

    for (name, input, replicas, expected, error_kind) in cases {
        match (build(pd::Voter, replicas, input), expected, error_kind) {
            (Ok(actual), Some(expected), None) => assert_rule_sets(actual, expected, name),
            (Err(error), None, Some(kind)) => assert!(
                error.to_string().starts_with(kind),
                "{name}: expected {kind:?}, got {error}"
            ),
            (actual, expected, error) => {
                panic!(
                    "{name}: unexpected result {actual:?}, expected {expected:?}, error {error:?}"
                )
            }
        }
    }
}
