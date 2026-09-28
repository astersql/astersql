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

// Go 1.26 构建下 map ABI 测试用的类型/常量别名。
//
// 对应 Go `map_abi_test_type_go126_test.go`。Go 1.26 将 table 命名为
// `mapTable`，测试经此别名与 Go 1.25 套件共用断言逻辑。

// testMapTable 对应 Go 的 `type testMapTable = mapTable`。
// Go 1.26 生产这里把 runtime table 名称改为 mapTable，测试通过该别名复用同一套断言。
/// 测试侧 table 类型别名，指向 Go 1.26 的 `mapTable`。
pub type testMapTable = super::map_abi_go126::mapTable;

// testMapGroupSlots 对应 Go 的 `const testMapGroupSlots = mapGroupSlots`。
// map_abi_test.rs 会用该常量校验每个 runtime group 的槽位容量。
/// 测试侧每 group 槽位数，指向 Go 1.26 的 `mapGroupSlots`。
pub const testMapGroupSlots: u64 = super::map_abi_go126::mapGroupSlots;
