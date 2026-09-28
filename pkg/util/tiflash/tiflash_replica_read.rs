// Copyright 2023 PingCAP, Inc.
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

// TiFlash 副本读（replica read）策略：选节点策略、字符串映射与远程读阈值。
//
// 对应 Go `pkg/util/tiflash/tiflash_replica_read.go`。策略决定分析计算时
// 使用全可用节点、同 zone 优先（不足可跨 zone），或仅同 zone（少量跨 zone
// Region 远程读可容忍，超限则报错）。

// 本文件由 pkg/util/tiflash/tiflash_replica_read.go 迁移而来，保留 TiFlash 副本读取策略、
// 字符串映射、未知值回退和远程读阈值的 Go 行为。

// ReplicaRead is the policy to select TiFlash nodes.
// ReplicaRead 对应 Go 里的 `type ReplicaRead int`。
// Go 的 int 在 Rust 中没有完全等价的别名语义；这里用新类型包住 isize，便于保留 Go 方法接收者。
/// TiFlash 节点选择策略；新类型包装 `isize`，对齐 Go `type ReplicaRead int`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplicaRead(pub isize);

// AllReplicas means using all the available nodes to do analytic computing, regardless of local zone or other zones.
// Go iota 从 0 开始递增；这里显式写出数值，避免隐藏常量顺序。
/// 使用全部可用 TiFlash 节点做分析计算，不考虑本 zone 或其他 zone。
#[allow(non_upper_case_globals)]
pub const AllReplicas: ReplicaRead = ReplicaRead(0);
// ClosestAdaptive means using the nodes in the same zone as the entry TiDB. If not all the tiflash data can be accessed, the query will involve the tiflash nodes from other zones.
// ClosestAdaptive 保留 Go 第二个 iota 值，表示优先同 zone，不足时允许跨 zone 访问。
/// 优先入口 TiDB 同 zone；数据不全时可拉入其他 zone 的 TiFlash 节点。
#[allow(non_upper_case_globals)]
pub const ClosestAdaptive: ReplicaRead = ReplicaRead(1);
// ClosestReplicas means using only the nodes in the same zone as the entry TiDB. If not all the tiflash data can be accessed, the query will report an error, and show an error message. Because of the feature of TiFlash remote read, a small number of regions in other zones is acceptable, but performance will be affected. The threshold is fixed, 3 regions per tiflash node.
// ClosestReplicas 保留 Go 第三个 iota 值，表示只使用入口 TiDB 同 zone 的 TiFlash 节点。
/// 仅用同 zone 节点；数据不全则报错。每节点可容忍少量跨 zone Region 远程读（阈值 3）。
#[allow(non_upper_case_globals)]
pub const ClosestReplicas: ReplicaRead = ReplicaRead(2);

#[allow(non_snake_case)]
impl ReplicaRead {
    // IsAllReplicas return whether the policy is AllReplicas.
    // IsAllReplicas 对应 Go 的值接收者方法，只做纯比较，不涉及 IO、并发或外部状态。
    /// 是否为 AllReplicas 策略。
    pub fn IsAllReplicas(&self) -> bool {
        *self == AllReplicas
    }

    // IsClosestReplicas return whether the policy is ClosestReplicas.
    // IsClosestReplicas 保留 Go 方法语义：只有策略值等于 ClosestReplicas 时返回 true。
    /// 是否为 ClosestReplicas 策略。
    pub fn IsClosestReplicas(&self) -> bool {
        *self == ClosestReplicas
    }
}

// GetTiFlashReplicaRead return corresponding policy string in integer.
// GetTiFlashReplicaRead 对应 Go 中把 ReplicaRead 策略值转换为 vardef 字符串的函数。
// default 分支会回退到 all_replicas，这是 Go 对未知整数值的兼容行为。
/// 将策略整型转为 vardef 字符串；未知值回退 `all_replicas`。
#[allow(non_snake_case)]
pub fn GetTiFlashReplicaRead(policy: ReplicaRead) -> &'static str {
    match policy {
        // 关键分支：AllReplicas 返回 vardef.AllReplicaStr，即 "all_replicas"。
        p if p == AllReplicas => vardef::AllReplicaStr,
        // 关键分支：ClosestAdaptive 返回 vardef.ClosestAdaptiveStr，即 "closest_adaptive"。
        p if p == ClosestAdaptive => vardef::ClosestAdaptiveStr,
        // 关键分支：ClosestReplicas 返回 vardef.ClosestReplicasStr，即 "closest_replicas"。
        p if p == ClosestReplicas => vardef::ClosestReplicasStr,
        // Go switch 的 default 分支覆盖未知值，保持向后兼容的 all_replicas 默认策略。
        _ => vardef::AllReplicaStr,
    }
}

// GetTiFlashReplicaReadByStr return corresponding policy in string.
// GetTiFlashReplicaReadByStr 对应 Go 中按字符串解析 TiFlash 副本读策略的函数。
// Parameter `str_` avoids Rust keyword `str`.
/// 按字符串解析策略；未知字符串回退 AllReplicas。参数名 `str_` 避开关键字 `str`。
#[allow(non_snake_case)]
pub fn GetTiFlashReplicaReadByStr(str_: &str) -> ReplicaRead {
    match str_ {
        // 关键分支：匹配 vardef.AllReplicaStr 时返回 AllReplicas。
        s if s == vardef::AllReplicaStr => AllReplicas,
        // 关键分支：匹配 closest_adaptive 字符串时返回 ClosestAdaptive。
        s if s == vardef::ClosestAdaptiveStr => ClosestAdaptive,
        // 关键分支：匹配 closest_replicas 字符串时返回 ClosestReplicas。
        s if s == vardef::ClosestReplicasStr => ClosestReplicas,
        // Go default 分支对未知字符串回退到 AllReplicas，不返回错误。
        _ => AllReplicas,
    }
}

// MaxRemoteReadCountPerNodeForClosestReplicas is the max remote read count per node for "closest_replicas".
// MaxRemoteReadCountPerNodeForClosestReplicas 保留 Go 常量值 3。
// 这个阈值说明 closest_replicas 模式下每个 TiFlash 节点可容忍少量跨 zone region 远程读。
/// closest_replicas 下每 TiFlash 节点允许的最大跨 zone Region 远程读次数（固定为 3）。
#[allow(non_upper_case_globals)]
pub const MaxRemoteReadCountPerNodeForClosestReplicas: isize = 3;
