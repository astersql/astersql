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

// 磁盘用量 Tracker 类型别名。
//
// 对应 Go：disk 包将 memory 包的 `Tracker`/`NewTracker`/`NewGlobalTracker` 再导出，
// 用于跟踪 spill 等到磁盘的字节占用。

// Tracker is the same type used by the memory package, matching Go's alias.
/// 与 memory 包相同的用量跟踪器类型（Go 侧亦为类型别名）。
pub type Tracker = crate::memory::tracker::Tracker;

// The constructors are function aliases in the Go package as well.
/// 再导出 memory 包的 Tracker 构造函数，语义与 Go 函数别名一致。
pub use crate::memory::tracker::{NewGlobalTracker, NewTracker};
