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

// cophandler 包级测试入口的公共初始化。

#![allow(non_snake_case)]

fn setup_for_common_test() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

#[test]
fn TestMainSetupForCommonTestIsIdempotent() {
    setup_for_common_test();
    setup_for_common_test();
}
