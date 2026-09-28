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

// texttree 核心 API 的字节级单元测试。
//
// 对应 Go `texttree` 测试：用 `as_bytes` 比较，确保盒线 Unicode 与空白/制表符
// 在 UTF-8 下与期望完全一致。

use super::{Indent4Child, PrettyIdentifier};

/// 覆盖 PrettyIdentifier：空缩进、中间/末孩子、空格与制表符缩进。
#[test]
fn test_pretty_identifier() {
    assert_bytes("test", PrettyIdentifier("test", "", false));
    assert_bytes("  ├ ─test", PrettyIdentifier("test", "  │  ", false));
    assert_bytes("\t\t├\t─test", PrettyIdentifier("test", "\t\t│\t\t", false));
    assert_bytes("  └ ─test", PrettyIdentifier("test", "  │  ", true));
    assert_bytes("\t\t└\t─test", PrettyIdentifier("test", "\t\t│\t\t", true));
}

/// 覆盖 Indent4Child：非末孩子追加树干，末孩子关闭最近分支后再追加。
#[test]
fn test_indent4_child() {
    assert_bytes("    │ ", Indent4Child("    ", false));
    assert_bytes("    │ ", Indent4Child("    ", true));
    assert_bytes("     │ ", Indent4Child("   │ ", true));
}

/// 按字节比较期望与实际字符串，避免可视化空白差异掩盖编码问题。
fn assert_bytes(expected: &str, actual: String) {
    assert_eq!(expected.as_bytes(), actual.as_bytes());
}
