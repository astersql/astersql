// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Go 1.25 构建下 map ABI 测试用的类型/常量别名。
//
// 对应 Go `map_abi_test_type_go125_test.go`。通过别名屏蔽版本差异，
// 让 `map_abi_test` 在不同 Go runtime 命名下复用同一套断言。

// testMapTable 对应 Go 的 `type testMapTable = swissMapTable`。
// 测试文件通过这个别名屏蔽 Go 版本差异：Go 1.25 仍使用 swissMapTable。
/// 测试侧 table 类型别名，指向 Go 1.25 的 `swissMapTable`。
pub type testMapTable = super::map_abi::swissMapTable;

// testMapGroupSlots 对应 Go 的 `const testMapGroupSlots = swissMapGroupSlots`。
// map_abi_test.rs 会用该常量校验每个 runtime group 的槽位容量。
/// 测试侧每 group 槽位数，指向 Go 1.25 的 `swissMapGroupSlots`。
pub const testMapGroupSlots: u64 = super::map_abi::swissMapGroupSlots;
