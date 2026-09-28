// Copyright 2023 PingCAP, Inc.
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

// 普通同步互斥锁 / 读写锁封装（对应 Go 默认 `sync` 实现，无死锁检测）。
//
// 再导出 `parking_lot` 的 Mutex/RWMutex；`EnableDeadlock` 恒为 false。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

/// 再导出普通互斥锁与读写锁类型别名（无死锁检测包装）。
pub use parking_lot::{Mutex, RwLock as RWMutex};

// EnableDeadlock is a flag to enable deadlock detection.
// EnableDeadlock 对应 Go 常量，普通 sync 实现下固定为 false，表示不启用死锁检测包装。
// Go-compatible exported symbol names retained for legacy callers.
/// 死锁检测开关：本变体固定为 false。
pub const EnableDeadlock: bool = false;
