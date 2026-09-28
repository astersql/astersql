// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Race 检测已启用时的构建变体（对应 Go `//go:build race`）。
//
// 与 `norace.rs` 成对：通过 Cargo feature `race` 选择导出哪个 `RaceEnabled`，
// 供运行时按是否开启竞态检测调整行为（如更严格的同步断言）。

// This file mirrors pkg/util/israce/israce.go.
// Go build tag: race。

// RaceEnabled checks if race is enabled.
// RaceEnabled 对应 Go 在 race 构建条件下的常量值。
/// 在启用 race feature 时为 `true`，表示当前构建开启了竞态检测。
#[cfg(feature = "race")]
pub const RaceEnabled: bool = true;
