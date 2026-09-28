// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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
// 对应 Go `pkg/util/codec/main_test.go`。Rust 无 goroutine TestMain，
// 在原生测试入口前执行公共初始化；Go goroutine 白名单仅作为迁移元数据保留。

// 本文件由 pkg/util/codec/main_test.go 迁移而来，保留 Go TestMain 初始化与白名单。
//

// libtest 没有稳定的 TestMain 钩子。原生加载器在测试线程创建和参数筛选前
// 调用此入口，保证无匹配测试、--list 和并行测试也执行 Go 的公共初始化。
// SAFETY: 入口无参数且只调用进程级日志初始化；静态函数指针存活整个进程。
#[used]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
#[cfg_attr(
    all(not(target_vendor = "apple"), not(target_os = "windows")),
    unsafe(link_section = ".init_array")
)]
static INITIALIZE_COMMON_TEST: extern "C" fn() = {
    extern "C" fn initialize() {
        testsetup::SetupForCommonTest();
    }
    initialize
};
