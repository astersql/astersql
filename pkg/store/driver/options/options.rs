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

// TiKV store 层副本读策略枚举及其从 TiDB 策略的映射。
//
// 副本读（replica read）控制读请求路由到 Region 的 Leader、Follower、Learner 等副本。
// TiDB 侧有七种策略，client-go / store 层仅暴露五种；就近读类策略在此折叠为 Mixed。

use crate::kv;

/// Replica selection policies understood by the TiKV store layer.
///
/// The Rust TiKV client does not expose client-go's routing policy enum.  Keep
/// the client-go byte ABI here so the store integration can consume the same
/// five policies without weakening TiDB's seven-policy input model.
///
/// TiKV store 层理解的副本选择策略（与 client-go 五策略字节 ABI 对齐）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum TiKVReplicaReadType {
    /// 仅读 Leader 副本（默认）。
    #[default]
    ReplicaReadLeader = 0,
    /// 优先读 Follower 副本。
    ReplicaReadFollower = 1,
    /// 混合路由：Leader / Follower 均可（亦用于近似就近读）。
    ReplicaReadMixed = 2,
    /// 读 Learner 副本（学习副本，通常不参与选举）。
    ReplicaReadLearner = 3,
    /// 优先 Leader，必要时可回落到其他副本。
    ReplicaReadPreferLeader = 4,
}

/// Maps TiDB replica-read policies to the policies supported by TiKV.
///
/// 将 TiDB 七种副本读策略映射为 TiKV 支持的五种；Closest / ClosestAdaptive → Mixed。
pub fn GetTiKVReplicaReadType(t: kv::ReplicaReadType) -> TiKVReplicaReadType {
    match t {
        kv::ReplicaReadType::ReplicaReadLeader => TiKVReplicaReadType::ReplicaReadLeader,
        kv::ReplicaReadType::ReplicaReadFollower => TiKVReplicaReadType::ReplicaReadFollower,
        // 就近读在 store 层无独立 ABI，折叠为 Mixed。
        kv::ReplicaReadType::ReplicaReadMixed
        | kv::ReplicaReadType::ReplicaReadClosest
        | kv::ReplicaReadType::ReplicaReadClosestAdaptive => TiKVReplicaReadType::ReplicaReadMixed,
        kv::ReplicaReadType::ReplicaReadLearner => TiKVReplicaReadType::ReplicaReadLearner,
        kv::ReplicaReadType::ReplicaReadPreferLeader => {
            TiKVReplicaReadType::ReplicaReadPreferLeader
        }
    }
}
