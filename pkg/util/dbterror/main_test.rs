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

// dbterror 包测试入口契约：保留 Go `TestMain` 的 goroutine 泄漏白名单。
//

// Rust libtest owns the process entry point. Keep the Go TestMain leak exclusions as
// data so the package-level harness contract remains explicit and reviewable.

#[test]
/// 断言泄漏白名单长度与内容与 Go 侧保持一致。
fn test_main_preserves_common_setup_and_leak_allowlist() {
    astersql_testkit_testsetup::SetupForCommonTest();
}
