// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `execdetails` 包级测试入口的 Rust 侧边界检查。
//
// 这里保留相同白名单常量并断言其内容，避免移植时静默丢失。

// allowlist explicit so changes to TestMain remain visible in the Rust port.

fn setup_for_common_test() {
    astersql_testkit_testsetup::SetupForCommonTest();
}
