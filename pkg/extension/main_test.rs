// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 扩展框架测试运行时复位检查。
//
// 全局 registry 为进程级单例；测试前后需 `Reset`，避免污染其他用例。
// 使用 `serial` 保证与同类测试串行执行。

use serial_test::serial;

/// 验证 Reset 后 GetExtensions 为空，并再次 Reset 收尾。
#[test]
#[serial]
fn canonical_extension_test_runtime_resets_registry_before_and_after_test() {
    // 清空全局注册表，确保起始状态干净。
    crate::Reset();
    assert!(crate::GetExtensions().unwrap().is_none());
    // 测试结束后再次 Reset，避免残留影响后续用例。
    crate::Reset();
}
