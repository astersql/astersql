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

// `InTest` 状态的启用变体：测试或 `intest` feature 下初始为 `true`。
//
// 对应 Go `intest` 带 `intest` build tag 的文件；与 `not_in_unittest` 互斥编译。

// InTest is initially true for the `intest` build variant.
/// 当前内部测试状态；允许与 Go 一样在运行时覆盖，并安全地跨线程读取。
pub static InTest: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
