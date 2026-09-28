// Copyright 2025 PingCAP, Inc.
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

// TiFlash AutoScaler（自动扩缩容器）相关的配置常量与辅助函数。
//
// TiFlash 是 TiDB 的列式存储引擎，用于加速分析型（OLAP）查询；
// AutoScaler 负责在存算分离（disaggregated）架构下按负载自动增减
// TiFlash 计算节点。本模块定义了 AutoScaler 的类型字符串、对应的
// 整数枚举值，以及在两者之间转换与校验的函数。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// MockASStr is string value for mock AutoScaler.
/// mock（模拟）AutoScaler 的字符串配置值，用于测试或本地模拟环境。
pub const MockASStr: &str = "mock";
// AWSASStr is string value for aws AutoScaler.
/// AWS 云环境 AutoScaler 的字符串配置值。
pub const AWSASStr: &str = "aws";
// GCPASStr is string value for gcp AutoScaler.
/// GCP 云环境 AutoScaler 的字符串配置值。
pub const GCPASStr: &str = "gcp";
// TestASStr is string value for test AutoScaler.
/// 本地 tidb 测试用 AutoScaler 的字符串配置值（非用户可用配置）。
pub const TestASStr: &str = "test";
// InvalidASStr is string value for invalid AutoScaler.
/// 非法 AutoScaler 的字符串值，用于表示无法识别的配置。
pub const InvalidASStr: &str = "invalid";

// DefAWSAutoScalerAddr is default address for aws AutoScaler.
/// AWS AutoScaler 的默认服务地址（Kubernetes 集群内部的服务域名与端口）。
pub const DefAWSAutoScalerAddr: &str =
    "tiflash-autoscale-lb.tiflash-autoscale.svc.cluster.local:8081";
// DefASStr is default AutoScaler.
/// 默认 AutoScaler 类型，当前默认为 AWS。
pub const DefASStr: &str = AWSASStr;

// Go iota block：显式写出 int 值，保持 Mock/AWS/GCP/Test/Invalid 的顺序。
// MockASType is int value for mock AutoScaler.
/// mock AutoScaler 对应的整数枚举值。
pub const MockASType: i32 = 0;
// AWSASType is int value for aws AutoScaler.
/// AWS AutoScaler 对应的整数枚举值。
pub const AWSASType: i32 = 1;
// GCPASType is int value for gcp AutoScaler.
/// GCP AutoScaler 对应的整数枚举值。
pub const GCPASType: i32 = 2;
// TestASType is for local tidb test AutoScaler.
/// 本地 tidb 测试 AutoScaler 对应的整数枚举值。
pub const TestASType: i32 = 3;
// InvalidASType is int value for invalid check.
/// 非法类型的整数枚举值，用于校验时表示无法识别的配置。
pub const InvalidASType: i32 = 4;

// IsValidAutoScalerConfig return true if user config of autoscaler type is valid.
// IsValidAutoScalerConfig 对应 Go 的合法性判断，Test/Invalid 不属于用户可用配置。
/// 校验用户配置的 AutoScaler 类型字符串是否合法。
///
/// 仅 mock、aws、gcp 三种类型对用户可用；test 仅供内部测试，
/// 无法识别的字符串会被解析为 Invalid，两者均返回 false。
pub fn IsValidAutoScalerConfig(typ: &str) -> bool {
    // 先转换为整数枚举，再判断是否落在用户可用的三种类型内。
    let t = GetAutoScalerType(typ);
    t == MockASType || t == AWSASType || t == GCPASType
}

// GetAutoScalerType return topo fetcher type.
// GetAutoScalerType 对应 Go switch，将配置字符串转换为内部整数枚举。
/// 将 AutoScaler 配置字符串转换为对应的整数枚举值。
///
/// 无法识别的字符串统一返回 `InvalidASType`。
pub fn GetAutoScalerType(typ: &str) -> i32 {
    match typ {
        MockASStr => MockASType,
        AWSASStr => AWSASType,
        GCPASStr => GCPASType,
        TestASStr => TestASType,
        _ => InvalidASType,
    }
}
