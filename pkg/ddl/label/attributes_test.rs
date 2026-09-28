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

// Region Label 属性模块的单元测试。
//
// 对应 Go 侧 NewLabel / RestoreLabel / NewLabels / AddLabels / RestoreLabels 用例，
// 覆盖解析裁剪空白、去重、冲突报错，以及还原时过滤内部标签与 keyspace 分支。

use super::{
    Add, NewLabel, NewLabels, RestoreRegionLabel, RestoreRegionLabels, kerneltype, pd::RegionLabel,
};

// test_new_label 对应 Go 的 TestNewLabel：验证单个 key=value 字符串可解析并裁剪空白。
/// 验证 NewLabel 解析与空白裁剪。
#[test]
fn test_new_label() {
    struct TestCase {
        name: &'static str,
        input: &'static str,
        label: RegionLabel,
    }

    let tests = vec![
        TestCase {
            name: "normal",
            input: "merge_option=allow",
            label: RegionLabel {
                Key: "merge_option".to_owned(),
                Value: "allow".to_owned(),
                ..Default::default()
            },
        },
        TestCase {
            name: "normal with space",
            input: " merge_option=allow ",
            label: RegionLabel {
                Key: "merge_option".to_owned(),
                Value: "allow".to_owned(),
                ..Default::default()
            },
        },
    ];

    for test in tests {
        let label = NewLabel(test.input).expect("Go require.NoError: NewLabel should succeed");
        assert_eq!(test.label, label, "case {}", test.name);
    }
}

// test_restore_label 对应 Go 的 TestRestoreLabel：验证 RegionLabel 可以恢复成规范 key=value 文本。
/// 验证 RestoreRegionLabel 输出规范文本。
#[test]
fn test_restore_label() {
    struct TestCase {
        name: &'static str,
        input: RegionLabel,
        output: &'static str,
    }

    let input = NewLabel("merge_option=allow").expect("Go require.NoError");
    let input1 = NewLabel(" merge_option=allow  ").expect("Go require.NoError");

    let tests = vec![
        TestCase {
            name: "normal",
            input,
            output: "merge_option=allow",
        },
        TestCase {
            name: "normal with spaces",
            input: input1,
            output: "merge_option=allow",
        },
    ];

    for test in tests {
        let output = RestoreRegionLabel(&test.input);
        assert_eq!(test.output, output, "case {}", test.name);
    }
}

// test_new_labels 对应 Go 的 TestNewLabels：覆盖空输入、多属性和重复属性去重。
/// 验证 NewLabels：空列表、多标签顺序与重复去重。
#[test]
fn test_new_labels() {
    let labels = NewLabels(vec![]).expect("nil slice should be accepted");
    assert_eq!(0, labels.len());

    let labels = NewLabels(vec![]).expect("empty slice should be accepted");
    assert_eq!(0, labels.len());

    let labels =
        NewLabels(vec!["merge_option=allow".to_owned()]).expect("single label should parse");
    assert_eq!(1, labels.len());
    assert_eq!("merge_option", labels[0].Key);
    assert_eq!("allow", labels[0].Value);

    // 对应 Go 注释：测试多个 attributes 的顺序和字段值。
    let labels = NewLabels(vec![
        "merge_option=allow".to_owned(),
        "key=value".to_owned(),
    ])
    .expect("multiple labels should parse");
    assert_eq!(2, labels.len());
    assert_eq!("merge_option", labels[0].Key);
    assert_eq!("allow", labels[0].Value);
    assert_eq!("key", labels[1].Key);
    assert_eq!("value", labels[1].Value);

    // 对应 Go 注释：重复 attributes 被跳过，只保留一份。
    let labels = NewLabels(vec![
        "merge_option=allow".to_owned(),
        "merge_option=allow".to_owned(),
    ])
    .expect("duplicated labels should be skipped");
    assert_eq!(1, labels.len());
    assert_eq!("merge_option", labels[0].Key);
    assert_eq!("allow", labels[0].Value);
}

// test_add_labels 对应 Go 的 TestAddLabels：验证新增、重复跳过和冲突报错。
/// 验证 Add：正常追加、重复跳过、同键冲突报错。
#[test]
fn test_add_labels() {
    struct TestCase {
        name: &'static str,
        labels: Vec<RegionLabel>,
        label: RegionLabel,
        err: bool,
    }

    let labels = NewLabels(vec!["merge_option=allow".to_owned()]).expect("fixture labels");
    let label = NewLabel("somethingelse=true").expect("fixture label");
    let l1 = NewLabels(vec!["key=value".to_owned()]).expect("fixture labels");
    let l2 = NewLabel("key=value").expect("fixture label");
    let l3 = NewLabels(vec!["key=value1".to_owned()]).expect("fixture labels");

    let tests = vec![
        TestCase {
            name: "normal",
            labels: labels.clone(),
            label: label.clone(),
            err: false,
        },
        TestCase {
            name: "duplicated attributes, skip",
            labels: l1,
            label: l2.clone(),
            err: false,
        },
        TestCase {
            name: "duplicated attributes, skip",
            labels: {
                let mut labels = labels.clone();
                labels.push(RegionLabel {
                    Key: "merge_option".to_owned(),
                    Value: "allow".to_owned(),
                    ..Default::default()
                });
                labels
            },
            label: label.clone(),
            err: false,
        },
        TestCase {
            name: "conflict attributes",
            labels: l3,
            label: l2,
            err: true,
        },
    ];

    for mut test in tests {
        // Go 传入 &test.labels 并允许 Add 就地追加；这里用可变 Vec 保留同样的指针修改语义。
        let err = Add(&mut test.labels, test.label.clone());
        if test.err {
            assert!(err.is_err(), "case {} should return error", test.name);
        } else {
            assert!(err.is_ok(), "case {} should succeed", test.name);
            assert_eq!(Some(&test.label), test.labels.last());
        }
    }
}

// test_restore_labels 对应 Go 的 TestRestoreLabels：只恢复可输出的 label，db/table/partition 等内部 label 被过滤。
/// 验证 RestoreRegionLabels 过滤内部标签，并按内核类型处理 keyspace。
#[test]
fn test_restore_labels() {
    struct TestCase {
        name: &'static str,
        input: Vec<RegionLabel>,
        output: &'static str,
    }

    let input1 = NewLabel("merge_option=allow").expect("fixture label");
    let input2 = NewLabel("key=value").expect("fixture label");
    let input3 = NewLabel("db=d1").expect("fixture label");
    let input4 = NewLabel("table=t1").expect("fixture label");
    let input5 = NewLabel("partition=p1").expect("fixture label");
    let input6 = NewLabel("keyspace=42").expect("fixture label");

    let tests = vec![
        TestCase {
            name: "normal1",
            input: vec![],
            output: "",
        },
        TestCase {
            name: "normal2",
            input: vec![input1.clone(), input2.clone()],
            output: r#""merge_option=allow","key=value""#,
        },
        TestCase {
            name: "normal3",
            input: vec![input3.clone(), input4.clone(), input5.clone()],
            output: "",
        },
        TestCase {
            name: "normal4",
            input: vec![input1.clone(), input2.clone(), input3],
            output: r#""merge_option=allow","key=value""#,
        },
    ];

    for test in tests {
        let output = RestoreRegionLabels(&test.input);
        assert_eq!(test.output, output, "case {}", test.name);
    }

    // kerneltype.IsNextGen 分支会过滤 keyspace 标签；Classic 分支保留 keyspace 输出。
    if kerneltype::IsNextGen() {
        let output = RestoreRegionLabels(&vec![input1.clone(), input6.clone()]);
        assert_eq!(r#""merge_option=allow""#, output);
        let output = RestoreRegionLabels(&vec![input6, input1]);
        assert_eq!(r#""merge_option=allow""#, output);
    } else {
        let output = RestoreRegionLabels(&vec![input1.clone(), input6.clone()]);
        assert_eq!(r#""merge_option=allow","keyspace=42""#, output);
        let output = RestoreRegionLabels(&vec![input6, input1]);
        assert_eq!(r#""keyspace=42","merge_option=allow""#, output);
    }
}
