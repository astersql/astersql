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

// Registration landing module for extensions maintained outside this repository.
//
// The Go package is intentionally empty: generated registration sources are
// added beside it and package initialization performs their registration.
// Rust generators can add sibling modules here without inventing runtime work
// for the empty package itself.
//
// 仓库外维护的扩展（extension）注册落点模块。
//
// 对应 Go 侧故意留空的 `_import` 包：代码生成器会把注册源文件放到本目录旁，
// 由包初始化完成注册。Rust 侧同样可作为生成模块的挂载点，本文件本身不承载
// 运行时逻辑。
