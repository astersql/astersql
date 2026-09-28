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

//
// Rust 原生测试无进程级 goroutine 泄漏检测；保留本模块以显式记录平台差异，
// 避免迁移期静默引入假运行时。

// Go's TestMain installs goroutine-leak exclusions. Rust tests have no
// process-wide goroutine harness; keeping this module in the test target makes
// that platform distinction explicit without introducing a fake runtime.
