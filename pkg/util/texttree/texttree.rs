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

// 文本树（text tree）绘制工具：用 Unicode 盒线字符拼出算子/计划树缩进。
//
// 对应 Go `pkg/util/texttree`。按 rune（`char`）而非字节处理 '│'/'├'/'└'/'─'，
// 供执行计划（execution plan）等层次结构的文本展示。

#![allow(non_snake_case, non_upper_case_globals)]

// This module mirrors pkg/util/texttree/texttree.go and preserves its
// rune-oriented handling of Unicode tree symbols.

// TreeBody indicates the current operator sub-tree is not finished, still
// has child operators to be attached on.
// TreeBody 对应 Go 里的 rune 常量 '│'，表示当前子树还没有结束。
/// 竖向树干字符 '│'：当前子树尚未结束，后续仍可挂接孩子。
pub const TreeBody: char = '│';

// TreeMiddleNode indicates this operator is not the last child of the
// current sub-tree rooted by its parent.
// TreeMiddleNode 对应 Go 里的 rune 常量 '├'，表示当前节点不是父节点下的最后一个孩子。
/// 中间孩子分支字符 '├'：当前节点不是父节点下的最后一个孩子。
pub const TreeMiddleNode: char = '├';

// TreeLastNode indicates this operator is the last child of the current
// sub-tree rooted by its parent.
// TreeLastNode 对应 Go 里的 rune 常量 '└'，表示当前节点是父节点下的最后一个孩子。
/// 末孩子分支字符 '└'：当前节点是父节点下的最后一个孩子。
pub const TreeLastNode: char = '└';

// TreeGap is used to represent the gap between the branches of the tree.
// TreeGap 对应 Go 里的 rune 常量空格，用来填充树枝之间的空白。
/// 树枝间隙空格：填充已结束祖先分支留下的空白。
pub const TreeGap: char = ' ';

// TreeNodeIdentifier is used to replace the treeGap once we need to attach
// a node to a sub-tree.
// TreeNodeIdentifier 对应 Go 里的 rune 常量 '─'，用来把缩进末尾空白改成节点连接横线。
/// 节点连接横线 '─'：把缩进末尾间隙改成挂接节点的标识。
pub const TreeNodeIdentifier: char = '─';

// Indent4Child appends more blank to the `indent` string
// Indent4Child 在已有缩进后追加下一层子节点所需的树干和空白。
// Go 版本接收 string 并返回新 string；用 &str 输入并构造新的 String，避免修改调用方数据。
/// 为下一层子节点追加缩进：非末孩子直接加树干；末孩子先关闭最近 TreeBody。
pub fn Indent4Child(indent: &str, isLastChild: bool) -> String {
    if !isLastChild {
        // Go 代码用 append([]rune(indent), TreeBody, TreeGap)；这里显式收集为 char，
        // 保持对 Unicode 盒线字符按 rune/char 而不是字节处理。
        let mut indent_chars: Vec<char> = indent.chars().collect();
        indent_chars.push(TreeBody);
        indent_chars.push(TreeGap);
        return indent_chars.into_iter().collect();
    }

    // If the current node is the last node of the current operator tree, we
    // need to end this sub-tree by changing the closest treeBody to a treeGap.
    // 如果当前节点是该层最后一个孩子，需要把最近的 TreeBody 改成 TreeGap，
    // 这样后续文本树不会继续画出已经结束的祖先分支。
    let mut indent_chars: Vec<char> = indent.chars().collect();
    for i in (0..indent_chars.len()).rev() {
        if indent_chars[i] == TreeBody {
            indent_chars[i] = TreeGap;
            break;
        }
    }

    // 与 Go 版本一样，无论是否找到可替换的 TreeBody，都会为下一层追加 TreeBody 和 TreeGap。
    indent_chars.push(TreeBody);
    indent_chars.push(TreeGap);
    indent_chars.into_iter().collect()
}

// PrettyIdentifier returns a pretty identifier which contains indent and tree node hierarchy indicator
// PrettyIdentifier 把节点 id 和当前缩进组合成带树形层级标识的展示字符串。
// Go 语义是在缩进为空时直接返回 id；缩进非空时改写最近的 TreeBody 和末尾 TreeGap。
/// 组合缩进与节点 id：空缩进直接返回 id；否则改写最近分支与末尾连接符。
pub fn PrettyIdentifier(id: &str, indent: &str, isLastChild: bool) -> String {
    if indent.is_empty() {
        return id.to_string();
    }

    // 对应 Go 的 []rune(indent)。这里继续使用 Vec<char>，确保 '│'、'├'、'└'、'─'
    // 这类多字节 Unicode 字符按单个文本树符号处理。
    let mut indent_chars: Vec<char> = indent.chars().collect();
    for i in (0..indent_chars.len()).rev() {
        if indent_chars[i] != TreeBody {
            continue;
        }

        // Here we attach a new node to the current sub-tree by changing
        // the closest treeBody to a:
        // 1. treeLastNode, if this operator is the last child.
        // 2. treeMiddleNode, if this operator is not the last child..
        // 这里保持 Go 的最近 TreeBody 替换规则：最后一个孩子画 '└'，否则画 '├'。
        if isLastChild {
            indent_chars[i] = TreeLastNode;
        } else {
            indent_chars[i] = TreeMiddleNode;
        }
        break;
    }

    // Replace the treeGap between the treeBody and the node to a
    // treeNodeIdentifier.
    // Go 代码直接改写最后一个 rune；由于前面已经排除了空缩进，这里可以安全访问末尾元素。
    let last_index = indent_chars.len() - 1;
    indent_chars[last_index] = TreeNodeIdentifier;

    let mut result: String = indent_chars.into_iter().collect();
    result.push_str(id);
    result
}
