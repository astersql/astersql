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

// 本文件由 build/linter/constructor/testdata/src/github.com/pingcap/tidb/pkg/util/linter/constructor/constructorflag.go 机械迁移而来。
// 这是 constructor linter 的测试数据草稿，只描述标记类型形状；不会连接数据库、不会执行业务动作，也不会真正运行 Go analyzer。
// Go package: constructor。
//
// Go imports: 无。

// This file is copied from `/util/linter/constructor` to help test

// Constructor is an empty struct to mark the constructor function
// Example:
//
//	type StatementContext struct {
//	    _ constructor.Constructor `ctor:"NewStmtCtx"`
//	}
//
// The linter `constructor` will then ignore all manual construction of the struct in `NewStmtCtx`, and return error
// for all other constructions.
// Constructor 对应 Go 的空结构体标记类型；测试数据依赖它的类型名和 ctor struct tag，而非运行时字段内容。
// 因而这个 fixture 只需要保留符号名可见性，不需要额外字段或行为实现。
pub struct Constructor;
