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

// TiDB 副本读策略到 TiKV 策略映射的迁移对照单元测试。
//
// 副本读（replica read）决定读请求发往 Leader、Follower 还是混合路由。
// 本文件锁定：TiDB 七种策略如何折叠为 TiKV 五种，以及枚举判别值与 client-go ABI 一致。

use std::sync::Arc;

use super::{
    GetTiKVReplicaReadType, TiKVReplicaReadType,
    kv::{ReplicaReadType, TransactionSchemaChecker},
};

/// 正式 KV option 共享的 schema checker 应保留本 crate 的成功与错误结果形状。
#[test]
fn transaction_schema_checker_uses_shared_error_contract() {
    let checker = TransactionSchemaChecker(Arc::new(|schema_version| {
        if schema_version == 42 {
            Ok(())
        } else {
            Err("schema changed".to_owned())
        }
    }));

    assert_eq!((checker.0)(42), Ok(()));
    assert_eq!((checker.0)(41), Err("schema changed".to_owned()));
}

/// 断言每种 TiDB 副本读策略都映射到与 Go 侧一致的 TiKV 策略。
///
/// `ReplicaReadClosest` / `ReplicaReadClosestAdaptive` 会折叠为 `ReplicaReadMixed`
///（就近读在 store 层用混合路由近似）。
#[test]
fn maps_every_tidb_replica_read_policy_like_go() {
    use ReplicaReadType::*;
    use TiKVReplicaReadType as Store;

    // 输入为 TiDB 策略，期望为 store 层五策略 ABI。
    let cases = [
        (ReplicaReadLeader, Store::ReplicaReadLeader),
        (ReplicaReadFollower, Store::ReplicaReadFollower),
        (ReplicaReadMixed, Store::ReplicaReadMixed),
        (ReplicaReadClosest, Store::ReplicaReadMixed),
        (ReplicaReadClosestAdaptive, Store::ReplicaReadMixed),
        (ReplicaReadLearner, Store::ReplicaReadLearner),
        (ReplicaReadPreferLeader, Store::ReplicaReadPreferLeader),
    ];

    for (input, expected) in cases {
        assert_eq!(GetTiKVReplicaReadType(input), expected);
    }
}

/// 断言 TiKV 策略枚举的 `repr(u8)` 判别值与 client-go 字节 ABI 对齐，且默认值为 Leader。
#[test]
fn store_policy_discriminants_match_client_go_abi() {
    assert_eq!(TiKVReplicaReadType::ReplicaReadLeader as u8, 0);
    assert_eq!(TiKVReplicaReadType::ReplicaReadFollower as u8, 1);
    assert_eq!(TiKVReplicaReadType::ReplicaReadMixed as u8, 2);
    assert_eq!(TiKVReplicaReadType::ReplicaReadLearner as u8, 3);
    assert_eq!(TiKVReplicaReadType::ReplicaReadPreferLeader as u8, 4);
    assert_eq!(
        TiKVReplicaReadType::default(),
        TiKVReplicaReadType::ReplicaReadLeader
    );
}
