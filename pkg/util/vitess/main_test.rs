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
// 对齐 Go `TestMain`：先 `SetupForCommonTest`，再核对泄漏检查忽略列表完整性。

//

// TestMain 对应 Go 的测试进程入口：先设置通用测试环境，再核对完整的泄漏检查白名单。
#[test]
#[allow(non_snake_case)]
pub fn TestMain() {
    testsetup::SetupForCommonTest();
}
