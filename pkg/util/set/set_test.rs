// Copyright 2024 PingCAP, Inc.
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

// 泛型 Set 单测：基础 CRUD、并/交/差与 CombSet 组合枚举。
//
// 对应 Go `TestSetBasic`、`TestSetOperation`、`TestSetCombination`。

use super::*;

// item 对应 Go 测试中的结构体，Text 同时作为显示内容和 Set key。
/// 测试元素：`Text` 既是显示内容也是 `Key()`。
#[derive(Clone, Debug, Eq, PartialEq)]
struct item {
    Text: String,
}

impl Key for item {
    // Key 对应 Go 的 `func (i item) Key() string { return i.Text }`。
    fn Key(&self) -> String {
        self.Text.clone()
    }
}

// item_text 是 Rust 测试中的小辅助，避免每个断言里重复 String 构造，语义仍对应 Go 的 item{Text: "..."}。
/// 由字面量构造 `item`，减少测试样板代码。
fn item_text(text: &str) -> item {
    item {
        Text: text.to_string(),
    }
}

// TestSetBasic 对应 Go 的基础集合测试：Add、Contains、Size、ToList、Remove 和 Clone。
/// 验证 Add/Contains/Size/ToList/Remove 与 Clone 独立性。
#[test]
fn TestSetBasic() {
    let mut s = NewSet::<item>();
    s.Add(&[item_text("q1"), item_text("q2"), item_text("q3")]);

    assert!(s.Contains(&item_text("q1")));
    assert!(s.Contains(&item_text("q2")));
    assert!(s.Contains(&item_text("q3")));
    assert!(!s.Contains(&item_text("q4")));
    assert_eq!(3, s.Size());
    assert_eq!(
        vec![item_text("q1"), item_text("q2"), item_text("q3")],
        s.ToList()
    );

    s.Remove(&item_text("q2"));
    assert!(!s.Contains(&item_text("q2")));
    assert_eq!(2, s.Size());

    // Clone 后修改原集合不应影响 clonedS，保留 Go 接口返回新集合的语义检查。
    let clonedS = s.Clone();
    assert!(clonedS.Contains(&item_text("q1")));
    s.Remove(&item_text("q1"));
    assert!(!s.Contains(&item_text("q1")));
    assert!(clonedS.Contains(&item_text("q1")));
    assert_eq!(2, clonedS.Size());
}

// TestSetOperation 对应 Go 的集合运算测试：并集、交集和两个方向的差集。
/// 验证 UnionSet、AndSet 与双向 DiffSet。
#[test]
fn TestSetOperation() {
    let mut s1 = NewSet::<item>();
    s1.Add(&[item_text("q1"), item_text("q2"), item_text("q3")]);
    let mut s2 = NewSet::<item>();
    s2.Add(&[item_text("q2"), item_text("q3"), item_text("q4")]);

    let unionSet = UnionSet(&[s1.as_ref(), s2.as_ref()]);
    assert_eq!(
        vec![
            item_text("q1"),
            item_text("q2"),
            item_text("q3"),
            item_text("q4")
        ],
        unionSet.ToList(),
    );

    let andSet = AndSet(&[s1.as_ref(), s2.as_ref()]);
    assert_eq!(vec![item_text("q2"), item_text("q3")], andSet.ToList());

    let mut diffSet = DiffSet(s1.as_ref(), s2.as_ref());
    assert_eq!(vec![item_text("q1")], diffSet.ToList());
    diffSet = DiffSet(s2.as_ref(), s1.as_ref());
    assert_eq!(vec![item_text("q4")], diffSet.ToList());
}

// TestSetCombination 对应 Go 的组合枚举测试：验证 CombSet 对 1 到 5 个元素的输出顺序与内容。
/// 验证 CombSet 在 k=1..4 的稳定输出，以及 k 超出大小时为空。
#[test]
fn TestSetCombination() {
    let mut s = NewSet::<item>();
    s.Add(&[
        item_text("q1"),
        item_text("q2"),
        item_text("q3"),
        item_text("q4"),
    ]);

    // 将组合列表字符串化，便于与 Go strings.Join 期望值直接比对。
    let setListStr = |setList: Vec<Box<dyn Set<item>>>| -> String {
        let mut tmp = Vec::new();
        for set in setList {
            // Go 闭包逐个调用 set.String() 后 strings.Join；这里保留相同的字符串化聚合流程。
            tmp.push(set.String());
        }
        tmp.join(", ")
    };

    let s1 = CombSet(s.as_ref(), 1);
    assert_eq!("{q1}, {q2}, {q3}, {q4}", setListStr(s1));

    let s2 = CombSet(s.as_ref(), 2);
    assert_eq!(
        "{q1, q2}, {q1, q3}, {q1, q4}, {q2, q3}, {q2, q4}, {q3, q4}",
        setListStr(s2),
    );

    let s3 = CombSet(s.as_ref(), 3);
    assert_eq!(
        "{q1, q2, q3}, {q1, q2, q4}, {q1, q3, q4}, {q2, q3, q4}",
        setListStr(s3),
    );

    let s4 = CombSet(s.as_ref(), 4);
    assert_eq!("{q1, q2, q3, q4}", setListStr(s4));

    // 请求 5 个元素超过集合大小，Go 返回 nil/空切片，Join 后得到空字符串。
    let s5 = CombSet(s.as_ref(), 5);
    assert_eq!("", setListStr(s5));
}
