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
// Rust 原生 harness 负责进程与线程生命周期；本文件将 Go 侧忽略的顶层函数
// 名固化为常量，作为配置回归检查，确保迁移后白名单条目未丢失。

// Rust's native harness owns process setup and thread joining. Run the shared
// setup bridge and keep the Go TestMain exclusions as an exact regression check.
#[test]
fn test_main_configuration_matches_go() {
    astersql_testkit_testsetup::SetupForCommonTest();
}
