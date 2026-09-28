// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 build/debug-linter/main.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 debug linter 的单 analyzer 入口；当前不会启动真实 Go analysis，
// 不会扫描源码，也不会修改构建产物，bootstrap 与 singlechecker 均为占位依赖。
// Go package: main。
//
// Go imports:
// - github.com/pingcap/tidb/build/linter/bootstrap
// - golang.org/x/tools/go/analysis/singlechecker

// THIS IS FOR DEBUG PURPOSES ONLY.
// main 对应 Go 的 debug 入口：把当前选定 analyzer 交给 singlechecker 运行。
// 这里刻意保持“手工切换一个 analyzer 再运行”的极简流程，便于迁移阶段逐个比对诊断输出。
pub fn main() {
    // just change the linter here.
    // Go 这里要求调试时手动替换 bootstrap.Analyzer；Rust 草稿保留同一入口形状。
    singlechecker::Main(bootstrap::Analyzer);
}
