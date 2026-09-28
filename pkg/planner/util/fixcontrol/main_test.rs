// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// fix-control 包级测试隔离性检查。
//
// Go 的 TestMain 会初始化进程级测试脚手架；Rust 单测由 Cargo 隔离运行，
// 此处用两次独立 ParseToMap 确认解析结果互不污染，等价验证用例隔离。

use super::ParseToMap;

// Go's TestMain initializes TiDB's process-wide test harness. Rust unit tests
// use Cargo's isolated harness, so the package-level equivalent is verifying
// that independent parses do not retain state between cases.
/// 连续两次解析不同输入时，各自 map 仅含本用例键值。
#[test]
fn TestHarnessCasesAreIsolated() {
    let (first, _) = ParseToMap("1:on").expect("first case");
    let (second, _) = ParseToMap("2:off").expect("second case");
    assert_eq!(Some("on"), first.get(&1).map(String::as_str));
    assert!(!first.contains_key(&2));
    assert_eq!(Some("off"), second.get(&2).map(String::as_str));
    assert!(!second.contains_key(&1));
}
