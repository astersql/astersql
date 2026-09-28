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

// UCA 权重表公共常量与 Go generate 指令说明。
//
// 对应 Go `ucadata/data.go`：定义长权重哨兵 `LongRune8`/`LONG_RUNE_8`，
// 以及由 generator 产出 unicode 4.0.0 / 9.0.0 数据文件的 go:generate 约定。

/// 表示该 rune 最多对应 8 个 collation element（排序权重单元）。
///
/// 生成表在 `MapTable4` 的 `u64` 项中写入此哨兵，实际权重改从 `LongRuneMap` 读取。
/// Means the rune has at most 8 collation elements.
///
/// The generated collation tables store this sentinel in `u64` entries.
pub const LONG_RUNE_8: u64 = 0xFFFD;

/// 与 Go `LongRune8` 同名的别名，便于机械迁移对照。
/// Go-compatible name for [`LONG_RUNE_8`].
#[allow(non_upper_case_globals)]
pub const LongRune8: u64 = LONG_RUNE_8;

// Go generate directives from the source file:
// - go run ./generator/ -- unicode_0900_ai_ci_data_generated.go
// - go run ./generator/ -- unicode_ci_data_generated.go
// These directives remain owned by the Go toolchain.
// 上述 go:generate 仍归属 Go 工具链；Rust 侧仅保留语义说明，不在此触发生成。

// Run these commands from the repository root to regenerate the Rust tables:
// - cargo run -p astersql-util-collate-ucadata-generator --bin ucadata-generator -- pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs
// - cargo run -p astersql-util-collate-ucadata-generator --bin ucadata-generator -- pkg/util/collate/ucadata/unicode_ci_data_generated.rs
