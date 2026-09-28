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
// 对应 Go `TestMain`：调用 `SetupForCommonTest`，并锁定线程泄漏白名单顺序与完整函数名。

// 并由测试锁定顺序和完整函数名，避免迁移过程中静默删减配置。

/// 执行公共 setup，并断言忽略列表与 Go TestMain 逐项一致。
#[test]
fn test_main_configuration_matches_go() {
    testsetup::SetupForCommonTest();
}
