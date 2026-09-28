// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 构造函数标记类型：供 `constructor` linter 识别允许的构造入口。
//
// 对应 Go `pkg/util/linter/constructor/constructorflag.go`。Go 侧通过嵌入空结构体
// 与 `ctor` tag 声明白名单构造函数；Rust 侧保留零大小标记类型以对齐语义。

// 本文件由 pkg/util/linter/constructor/constructorflag.go 迁移而来，保留 Go 实现结构。
// 只是让人工读者看到 Go 侧通过嵌入字段和 `ctor` tag 标记构造函数白名单的语义。

// Constructor is an empty struct to mark the constructor function
// Example:
//
//	type StatementContext struct {
//	    _ constructor.Constructor `ctor:"NewStmtCtx"`
//	}
//
// The linter `constructor` will then ignore all manual construction of the struct in `NewStmtCtx`, and return error
// for all other constructions.
// Constructor 对应 Go 的空结构体标记；Rust 实现保持零字段结构，不增加运行时状态。
// Go 里的 `ctor:"NewStmtCtx"` tag 没有直接 Rust 字段 tag 等价物，后续 linter 迁移需要单独处理。
/// 空构造函数标记结构体；零大小，可 Copy，语义对齐 Go 嵌入字段。
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct Constructor {}
