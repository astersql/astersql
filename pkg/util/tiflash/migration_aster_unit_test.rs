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

// TiFlash 副本读策略迁移补充单元测试。
//
// 校验 iota 数值、谓词、整型/字符串双向映射的默认回退，以及
// closest_replicas 模式下每节点远程读（remote read）阈值与 Go 一致。

#![allow(non_snake_case, non_upper_case_globals)]

use super::{
    AllReplicas, ClosestAdaptive, ClosestReplicas, GetTiFlashReplicaRead,
    GetTiFlashReplicaReadByStr, MaxRemoteReadCountPerNodeForClosestReplicas, ReplicaRead,
};
use crate::sessionctx::vardef;

/// Go 零值默认应为 AllReplicas（Default derive 与 iota 0 对齐）。
#[test]
fn go_zero_value_defaults_to_all_replicas() {
    assert_eq!(ReplicaRead::default(), AllReplicas);
}

/// 策略常量数值与 IsAllReplicas / IsClosestReplicas 谓词与 Go iota 一致。
#[test]
fn replica_values_and_predicates_match_go_iota() {
    assert_eq!(AllReplicas.0, 0);
    assert_eq!(ClosestAdaptive.0, 1);
    assert_eq!(ClosestReplicas.0, 2);

    for (policy, is_all, is_closest) in [
        (AllReplicas, true, false),
        (ClosestAdaptive, false, false),
        (ClosestReplicas, false, true),
        (ReplicaRead(99), false, false),
    ] {
        assert_eq!(policy.IsAllReplicas(), is_all);
        assert_eq!(policy.IsClosestReplicas(), is_closest);
    }
}

/// 整型策略转字符串：未知值回退到 all_replicas（Go switch default）。
#[test]
fn policy_to_string_matches_go_switch_and_default() {
    for (policy, expected) in [
        (AllReplicas, vardef::AllReplicaStr),
        (ClosestAdaptive, vardef::ClosestAdaptiveStr),
        (ClosestReplicas, vardef::ClosestReplicasStr),
        (ReplicaRead(-1), vardef::AllReplicaStr),
        (ReplicaRead(99), vardef::AllReplicaStr),
    ] {
        assert_eq!(GetTiFlashReplicaRead(policy), expected);
    }
}

/// 字符串解析策略：空串/大小写变体/未知值均回退 AllReplicas。
#[test]
fn string_to_policy_matches_go_switch_and_default() {
    for (value, expected) in [
        (vardef::AllReplicaStr, AllReplicas),
        (vardef::ClosestAdaptiveStr, ClosestAdaptive),
        (vardef::ClosestReplicasStr, ClosestReplicas),
        ("", AllReplicas),
        ("ALL_REPLICAS", AllReplicas),
        ("unknown", AllReplicas),
    ] {
        assert_eq!(GetTiFlashReplicaReadByStr(value), expected);
    }
}

/// closest_replicas 每节点可容忍的跨 zone Region 远程读上限为 3。
#[test]
fn closest_replicas_remote_read_threshold_matches_go() {
    assert_eq!(MaxRemoteReadCountPerNodeForClosestReplicas, 3);
}
