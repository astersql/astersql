// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// slice 包级测试入口占位。
//
// 对应 Go `TestMain`：Rust 内建测试运行器负责进程生命周期。本包无后台线程，

// Rust 的内建测试运行器负责进程生命周期。本包没有后台线程，因此 Go TestMain
// 中的通用 TiDB 初始化和 goroutine 泄漏白名单在这里没有对应动作。
