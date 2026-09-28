// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! MaxStoreConcurrency in the future, num of tikv may extend to a large number,
//! this is limitation of connection pool to tikv per our knowledge; in present,
//! 128 may be good enough.
//!
//! 单进程对 TiKV 的最大并发连接上限；集群扩容后仍以此为连接池化经验天花板。
//! 对齐 Go `br/pkg/common` 常量 MaxStoreConcurrency=128。

/// 每 BR 进程对 store 的并发连接上限（经验值 128）。
///
/// 使用 `usize` 以对应 Go 中该无类型常量与 `len(...)` 共同作为并发计数的用法。
pub const MaxStoreConcurrency: usize = 128;
