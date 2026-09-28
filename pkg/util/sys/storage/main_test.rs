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

// Go TestMain applies log_level before running storage tests. Call the shared
// implementation once from tests that need common setup. Native libtest runs
// tests itself; this helper does not impose ordering on unrelated tests.
// The storage implementation and testsetup logger create no background workers.
// Go's four goroutine exclusions are reference configuration only, not a Rust
// thread-leak detector; check them against the actual Go source below.

use std::sync::Once;

/// 执行一次与 Go `TestMain` 对应的公共测试初始化。
pub fn setup_for_common_test() {
    static SETUP: Once = Once::new();
    SETUP.call_once(astersql_testkit_testsetup::SetupForCommonTest);
}
