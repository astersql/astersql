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
// Rust 测试框架接管进程启动后，仍调用公共 TiDB 测试初始化；

/// Rust's test harness owns process startup, so the executable part of Go's
/// allow-list remains above verbatim: its entries name Go goroutines and have
/// no Rust-thread equivalent to register.
/// 对应 Go TestMain：执行公共测试初始化。
#[allow(non_snake_case)]
fn TestMain() {
    testsetup::SetupForCommonTest();
}
