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

// 本文件记录 build/linter/linter.go 的 Go 专属依赖保留约束。
// Go package: linter。
//
// Go imports:
// - _ "github.com/apache/skywalking-eyes/pkg/config"

// Go 的空白导入只让 skywalking-eyes v0.4.0 保留在 go.mod 中，不提供可调用行为。
// Rust 无运行时对应动作，也没有同名 Cargo 依赖，因此这里不虚构函数或初始化副作用。
