// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 存储容量查询单测：校验当前目录可用容量至少为 1 字节。
//
// 对应 Go `TestGetTargetDirectoryCapacity`；可选后续用 `df` 核对精确值。

/// 对 `"."` 调用 `GetTargetDirectoryCapacity`，断言返回容量 ≥ 1。
#[test]
fn test_get_target_directory_capacity() {
    // 与 Go TestMain / setup 对齐的公共测试初始化。
    super::main_test::setup_for_common_test();

    let capacity =
        super::GetTargetDirectoryCapacity(".").expect("couldn't get target directory capacity");
    assert!(capacity >= 1, "couldn't get capacity");

    // TODO: check the value of r with `df` in linux
}
