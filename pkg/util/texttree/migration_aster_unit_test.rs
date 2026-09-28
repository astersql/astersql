// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// texttree 迁移补充单元测试。
//
// 校验盒线字符常量、PrettyIdentifier / Indent4Child 与 Go 示例一致，
// 并覆盖 Unicode 节点名及无活动分支时按 rune 改写末尾字符的行为。

use super::{
    Indent4Child, PrettyIdentifier, TreeBody, TreeGap, TreeLastNode, TreeMiddleNode,
    TreeNodeIdentifier,
};

/// 断言树形绘制所用 rune 常量与 Go `texttree` 一致。
#[test]
fn constants_match_go_tree_runes() {
    assert_eq!(TreeBody, '│');
    assert_eq!(TreeMiddleNode, '├');
    assert_eq!(TreeLastNode, '└');
    assert_eq!(TreeGap, ' ');
    assert_eq!(TreeNodeIdentifier, '─');
}

/// 对照 Go 示例：空缩进、中间/末尾孩子、空白与制表符缩进。
#[test]
fn pretty_identifier_matches_go_examples() {
    let cases = [
        ("test", "", false, "test"),
        ("test", "  │  ", false, "  ├ ─test"),
        ("test", "\t\t│\t\t", false, "\t\t├\t─test"),
        ("test", "  │  ", true, "  └ ─test"),
        ("test", "\t\t│\t\t", true, "\t\t└\t─test"),
    ];

    for (id, indent, is_last_child, expected) in cases {
        assert_eq!(PrettyIdentifier(id, indent, is_last_child), expected);
    }
}

/// 对照 Go 示例：为子层追加树干，或在末孩子时关闭最近分支。
#[test]
fn indent_for_child_matches_go_examples() {
    let cases = [
        ("    ", false, "    │ "),
        ("    ", true, "    │ "),
        ("   │ ", true, "     │ "),
    ];

    for (indent, is_last_child, expected) in cases {
        assert_eq!(Indent4Child(indent, is_last_child), expected);
    }
}

/// 按 rune 处理多字节盒线符与中文节点名；无 TreeBody 时仍改写末尾字符。
#[test]
fn nearest_branch_and_unicode_text_are_handled_as_runes() {
    let indent = "│ │ ";
    assert_eq!(PrettyIdentifier("节点", indent, false), "│ ├─节点");
    assert_eq!(PrettyIdentifier("节点", indent, true), "│ └─节点");
    assert_eq!(Indent4Child(indent, true), "│   │ ");

    // Go replaces the final rune even when the indent has no active branch.
    // 即使缩进中没有活动分支，Go 仍会替换最后一个 rune。
    assert_eq!(PrettyIdentifier("node", "abc", false), "ab─node");
}
