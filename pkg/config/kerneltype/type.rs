// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 本文件由 pkg/config/kerneltype/type.go 迁移而来，保留 Go 实现结构。
// TiDB kernel type name classification.

// 内核类型（kernel type）名称判定模块。
//
// 数据库内核存在两种形态：Classic（经典架构）与 Next Generation（存算分离
// 的下一代架构）。编译期通过 `nextgen` feature 决定当前实例属于哪一种，
// 运行期本模块负责：
// - 提供两种内核类型的规范名称常量；
// - 返回当前实例的内核类型名称（[`Name`]）；
// - 校验 PD（Placement Driver，集群元数据与调度中心组件）上报的内核类型
//   是否与当前实例一致（[`IsMatch`]），避免混合部署不同内核形态的节点。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// 依据编译 feature 选择 IsNextGen 的实现来源：
// 未启用 nextgen 时使用 classic 模块（恒为 false），启用时使用 nextgen 模块（恒为 true）。
#[cfg(not(feature = "nextgen"))]
use super::classic::IsNextGen;
#[cfg(feature = "nextgen")]
use super::nextgen::IsNextGen;

// Go const block：保留 PD 与 TiDB 共同使用的内核类型字符串。
/// 经典（Classic）内核类型的规范名称，与 PD 侧定义保持一致。
pub const classicKernelName: &str = "Classic";
/// 下一代（Next Generation）内核类型的规范名称，与 PD 侧定义保持一致。
pub const nextgenKernelName: &str = "Next Generation";

// IsMatch checks if the given PD kernel type matches current instance.
// PD defines the kernel type name in the same wey as TiDB does, might we can unify
// them in the future. see:
// https://github.com/tikv/pd/blob/29ead019cd0982a3120bc79d4a4d19199dab2279/pkg/versioninfo/kerneltype/nextgen.go#L25
// https://github.com/tikv/pd/blob/29ead019cd0982a3120bc79d4a4d19199dab2279/pkg/versioninfo/kerneltype/classic.go#L25
// IsMatch 对应 Go 的同名函数：空 PD kernel type 兼容旧 PD，按 Classic 处理。
/// 判断 PD 上报的内核类型是否与当前实例匹配。
///
/// PD（Placement Driver）是集群的元数据管理与调度组件，其内核类型命名
/// 规则与本模块一致。参数 `pdKernelType` 为 PD 返回的内核类型字符串；
/// 为空表示旧版本 PD（尚无该字段），此时按 Classic 处理以保持向后兼容。
pub fn IsMatch(pdKernelType: &str) -> bool {
    if pdKernelType.is_empty() {
        // 旧版本 PD 没有 kernel type 字段；Go 代码把它视为 Classic 后再和当前实例名比较。
        return classicKernelName == Name();
    }
    pdKernelType == Name()
}

// Name returns the name of the current kernel type.
// Name 对应 Go 的当前内核类型名称选择；IsNextGen 由当前构建选择的模块提供。
/// 返回当前实例的内核类型名称。
///
/// 结果由编译期 feature 决定：启用 `nextgen` 时返回 [`nextgenKernelName`]，
/// 否则返回 [`classicKernelName`]。
pub fn Name() -> &'static str {
    if IsNextGen() {
        return nextgenKernelName;
    }
    classicKernelName
}
