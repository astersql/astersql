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

// 事务 driver 包的测试入口设置。
//
// 对应 Go 的 `TestMain` / common test setup：初始化共享测试环境。

#[test]
/// 连续两次调用 SetupForCommonTest，确认共享测试初始化可重复执行且无副作用冲突。
fn TestMainSetupForCommonTestIsIdempotent() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_testkit_testsetup::SetupForCommonTest();
}
