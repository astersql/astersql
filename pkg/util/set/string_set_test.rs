// Copyright 2019 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `StringSet` 单元测试：去重、Exist、构造与多轮 Intersection。

use super::*;

// TestStringSet 对应 Go 的同名测试：覆盖重复插入去重、Exist 查询以及多轮 Intersection。
/// 覆盖重复插入、Exist、构造初始化与 Intersection 空/非空情形。
#[test]
pub fn TestStringSet() {
    let mut set = NewStringSet(&[]);
    let vals = vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];

    // Go 循环中每个值连续插入 5 次，用来证明 StringSet 只保存唯一 key。
    for val in &vals {
        set.Insert((*val).to_owned());
        set.Insert((*val).to_owned());
        set.Insert((*val).to_owned());
        set.Insert((*val).to_owned());
        set.Insert((*val).to_owned());
    }
    assert_eq!(vals.len(), set.Count() as usize);

    // Go 直接断言 len(set)；Rust 通过 Count 表达同一集合长度语义，避免依赖内部字段可见性。
    assert_eq!(vals.len(), set.Count() as usize);
    for val in &vals {
        assert!(set.Exist(val));
    }

    assert!(!set.Exist("11"));

    set = NewStringSet(&["1", "2", "3", "4", "5", "6"]);
    for i in 1..7 {
        // 对应 Go 的 fmt.Sprintf("%d", i)，逐个查询构造函数传入的字符串成员。
        assert!(set.Exist(&format!("{}", i)));
    }
    assert!(!set.Exist("7"));

    let s1 = NewStringSet(&["1", "2", "3"]);
    let s2 = NewStringSet(&["4", "2", "3"]);
    let s3 = s1.Intersection(&s2);
    assert_eq!(NewStringSet(&["2", "3"]), s3);

    let s4 = NewStringSet(&["4", "5", "3"]);
    assert_eq!(NewStringSet(&["3"]), s3.Intersection(&s4));

    let s5 = NewStringSet(&["4", "5"]);
    assert_eq!(NewStringSet(&[]), s3.Intersection(&s5));

    let s6 = NewStringSet(&[]);
    assert_eq!(NewStringSet(&[]), s3.Intersection(&s6));
}

// Go strings.ToUpper applies simple one-rune mappings and does not expand sharp s to "SS".
#[test]
fn intersection_with_lower_uses_go_simple_unicode_case_mapping() {
    let lowercase = NewStringSet(&["ß"]);
    let rhs = NewStringSet(&["ß"]);

    assert_eq!(
        NewStringSet(&["ß"]),
        lowercase.IntersectionWithLower(&rhs, false)
    );

    let lowercase = NewStringSet(&["i"]);
    let rhs = NewStringSet(&["İ"]);
    assert_eq!(
        NewStringSet(&["İ"]),
        lowercase.IntersectionWithLower(&rhs, true)
    );
}
