// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 快速随机源：提供无锁 `Uint32`，对应 Go `runtime.cheaprand`。
//
// Go 通过 `//go:linkname` 直连运行时廉价随机；Rust 无等价链接属性，
// 改用线程局部的 `fastrand` crate 生成器，语义仍是高频无锁 uint32。

// 本文件由 pkg/util/fastrand/runtime.go 迁移而来。
//
// Go 的 //go:linkname 没有直接 Rust 等价；fastrand 使用线程局部生成器提供无锁快速随机值。

// Uint32 returns a lock free uint32 value.
//
//go:linkname Uint32 runtime.cheaprand
/// 返回无锁伪随机 `u32`，对应 Go `runtime.cheaprand`。
pub fn Uint32() -> u32 {
    fastrand::u32(..)
}
