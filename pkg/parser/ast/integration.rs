// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// crate 根部规范 AST 的兼容视图。
//
// Go 版本只公开一套 `pkg/parser/ast` 类型。这里直接重新导出同一批类型，
// 让历史 `integration` 路径保持源码兼容，同时避免编译出另一套可能逐渐偏离的 AST。

pub use crate::*;
