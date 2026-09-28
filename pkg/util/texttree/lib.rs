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

// 纯文本树形缩进与节点标识符工具 crate 入口。
//
// 对应 Go `pkg/util/texttree`：为执行计划/调试输出等生成 `|`/`└─`/`├─`
// 风格的树线与缩进；实现位于 `texttree` 子模块。

#![allow(non_snake_case, non_upper_case_globals)]

/// 树线字符常量与 PrettyIdentifier 等实现。
mod texttree;

/// 再导出树缩进与节点标识相关公共 API。
pub use texttree::{
    Indent4Child, PrettyIdentifier, TreeBody, TreeGap, TreeLastNode, TreeMiddleNode,
    TreeNodeIdentifier,
};

/// 对照 Go TestMain 的包级测试配置契约。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// texttree 核心行为单测。
#[cfg(test)]
#[path = "texttree_test.rs"]
mod texttree_test;
