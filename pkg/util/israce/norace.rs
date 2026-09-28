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

// 未启用 Race 检测时的构建变体（对应 Go `//go:build !race`）。
//
// 与 `israce.rs` 成对：默认构建（无 `race` feature）导出本文件的 `RaceEnabled = false`。

// This file mirrors pkg/util/israce/norace.go.
// Go build tag: !race。

// RaceEnabled checks if race is enabled.
// RaceEnabled 对应 Go 在非 race 构建条件下的常量值。
/// 在未启用 race feature 时为 `false`，表示当前构建未开启竞态检测。
#[cfg(not(feature = "race"))]
pub const RaceEnabled: bool = false;
