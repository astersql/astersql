// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 pkg/config/kerneltype/type_test.go 迁移而来，保留 Go 测试结构与断言语义。
// Tests Classic/NextGen kernel type classification.
//

// kerneltype 模块的单元测试：验证内核类型（kernel type）的分类逻辑。
//
// 背景说明：数据库内核存在两种形态——
// - Classic（经典架构）：传统的存算一体部署形态；
// - NextGen（下一代架构）：面向云原生的存算分离形态。
// 内核类型在编译期由构建配置决定，运行期只能取二者之一。
// 本测试文件覆盖两个不变量：
// 1. `IsClassic` 与 `IsNextGen` 互斥且互补（恰好一个为真）；
// 2. `IsMatch` 能正确匹配 PD（Placement Driver，集群元信息与调度中心）
//    上报的内核类型名称字符串。

// 允许保留 Go 风格命名与暂未使用的代码，避免机械迁移期间的编译告警。
#![allow(dead_code, non_snake_case)]

// 从父模块引入被测函数：内核类型判定与名称匹配。
use super::{IsClassic, IsMatch, IsNextGen};

// test_kernel_type 对应 Go 的 TestKernelType，验证 IsClassic 与 IsNextGen 始终互为反值。
/// 验证内核类型判定的互斥性：任何时刻 `IsClassic()` 与 `IsNextGen()`
/// 必须互为反值，即实例要么是 Classic、要么是 NextGen，不存在第三种状态。
#[test]
pub fn test_kernel_type() {
    assert_eq!(!IsClassic(), IsNextGen());
    assert_eq!(IsClassic(), !IsNextGen());
}

// test_is_match 对应 Go 的 TestIsMatch，按当前构建内核类型验证 PD kernel type 名称匹配。
/// 验证 `IsMatch` 对内核类型名称字符串的匹配逻辑：
/// 根据当前构建出的内核类型，只有对应的名称（Classic 下的 ""/"Classic"，
/// NextGen 下的 "Next Generation"）才应匹配成功，未知名称一律不匹配。
#[test]
pub fn test_is_match() {
    // 按编译期确定的内核类型分支断言各自合法的名称集合。
    if IsClassic() {
        // 空字符串兼容旧 PD，Go 语义中等价于 Classic。
        assert!(IsMatch(""));
        assert!(IsMatch("Classic"));
    } else if IsNextGen() {
        assert!(IsMatch("Next Generation"));
    }

    // Unknown 在任一内核类型下都不应匹配当前实例。
    assert!(!IsMatch("Unknown"));
}
