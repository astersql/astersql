// Copyright 2026 AsterSQL.
//
// Copyright 2016 PingCAP, Inc.
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

// Schema Validator 的 Go 风格集成测试端口。
//
// 用后台线程模拟“远端”租约授予服务，覆盖通用生命周期、enqueue 压缩与
// action type 参与压缩的语义，子用例顺序与 Go `TestSchemaValidator` 一致。

use std::thread;
use std::time::{Duration, SystemTime};

use crossbeam_channel::{Receiver, Sender, bounded, select};

use crate::validator::{
    DeltaSchemaInfo, RelatedSchemaChange, Result, Validator, system_time_to_ts,
};

/// Drop 时还原 `MaxDeltaSchemaCount`，避免污染其它用例。
struct MaxDeltaSchemaCountRestore(i64);

impl MaxDeltaSchemaCountRestore {
    /// 记录当前全局 max-delta 配置。
    fn capture() -> Self {
        Self(crate::vardef::GetMaxDeltaSchemaCount())
    }
}

impl Drop for MaxDeltaSchemaCountRestore {
    fn drop(&mut self) {
        crate::vardef::SetMaxDeltaSchemaCount(self.0);
    }
}

/// 构造 RelatedSchemaChange 测试载荷。
fn change(ids: &[i64], actions: &[u64]) -> RelatedSchemaChange {
    RelatedSchemaChange {
        phy_tbl_ids: ids.to_vec(),
        action_types: actions.to_vec(),
    }
}

/// 构造一条期望的 DeltaSchemaInfo。
fn delta(schema_version: i64, ids: &[i64], actions: &[u64]) -> DeltaSchemaInfo {
    DeltaSchemaInfo {
        schema_version,
        related_ids: ids.to_vec(),
        related_actions: actions.to_vec(),
    }
}

// TestSchemaValidator maps to Go's batched test and preserves its subtest order.
/// 入口：串行跑 general / enqueue / enqueue_action_type 三个子用例。
#[test]
fn test_schema_validator() {
    let _lock = crate::VARDEF_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = MaxDeltaSchemaCountRestore::capture();
    // Parallel packages may leave a zero max-delta; restore a usable default first.
    // 并行包可能把 max-delta 留成 0，先恢复可用默认值。
    crate::vardef::SetMaxDeltaSchemaCount(crate::vardef::DefTiDBMaxDeltaSchemaCount);
    sub_test_schema_validator_general();
    sub_test_enqueue();
    sub_test_enqueue_action_type();
}

// subTestSchemaValidatorGeneral is batched in TestSchemaValidator.
/// 通用路径：租约授予、stop/restart、版本推进、相关表变更与租约过期。
fn sub_test_schema_validator_general() {
    let lease = Duration::from_millis(10);
    let (lease_grant_tx, lease_grant_rx) = bounded(0);
    let (exit_tx, exit_rx) = bounded(0);
    let server = thread::spawn(move || server_func(lease_grant_tx, exit_rx));

    let validator = Validator::new(lease);
    assert!(validator.is_started());

    for _ in 0..3 {
        // Reload can run arbitrarily, at any time.
        // 模拟任意时刻的 lease reload。
        let item = lease_grant_rx.recv().expect("lease server stopped early");
        validator.update(item.lease_grant_ts, item.old_ver, item.schema_ver, None);
    }

    // Take a lease, check it's valid.
    // 取得租约并确认当前 schema 下 check 成功。
    let item = lease_grant_rx.recv().expect("lease server stopped early");
    validator.update(
        item.lease_grant_ts,
        item.old_ver,
        item.schema_ver,
        Some(&change(&[10], &[10])),
    );
    assert_eq!(
        validator
            .check(item.lease_grant_ts, item.schema_ver, Some(&[10]), true)
            .1,
        Result::ResultSucc
    );

    // Stop the validator, validator's items value is nil.
    // stop 后 delta 清空，相关表判定与 check 均失败/未知。
    validator.stop();
    assert!(!validator.is_started());
    assert!(validator.is_related_tables_changed(item.schema_ver, &[10]));
    assert_eq!(
        validator
            .check(item.lease_grant_ts, item.schema_ver, Some(&[10]), true)
            .1,
        Result::ResultUnknown
    );
    validator.restart(validator.snapshot().restart_schema_ver);

    // Increase the current time by 2 leases, check schema is invalid.
    // 事务时间戳推到约 2 个 lease 之后，应得 Unknown。
    let after_two_leases = SystemTime::now() + lease * 2;
    let mut ts = system_time_to_ts(after_two_leases);
    assert_eq!(
        validator.check(ts, item.schema_ver, Some(&[10]), true).1,
        Result::ResultUnknown,
        "validator snapshot {:?}, item {:?}, ts {ts}",
        validator.snapshot(),
        item
    );

    // Make sure newItem's version is greater than item.schema.
    // 推进到更大 schema 版本后：nil / 无关表 ID 在版本落后时应 Fail。
    let mut new_item = get_greater_version_item(&lease_grant_rx, item.schema_ver);
    let curr_ver = new_item.schema_ver;
    validator.update(new_item.lease_grant_ts, new_item.old_ver, curr_ver, None);
    assert_eq!(
        validator.check(ts, item.schema_ver, None, true).1,
        Result::ResultFail,
        "currVer {curr_ver}, item {item:?}"
    );
    assert_eq!(
        validator.check(ts, item.schema_ver, Some(&[0]), true).1,
        Result::ResultFail,
        "currVer {curr_ver}, item {item:?}"
    );

    // Check the latest schema version must changed.
    assert!(item.schema_ver < validator.snapshot().latest_schema_ver);

    // Make sure newItem's version is greater than currVer.
    new_item = get_greater_version_item(&lease_grant_rx, curr_ver);
    // Update current schema version to newItem's version and the delta table IDs is 1, 2, 3.
    validator.update(
        ts,
        curr_ver,
        new_item.schema_ver,
        Some(&change(&[1, 2, 3], &[1, 2, 3])),
    );
    // Make sure the updated table IDs don't be covered with the same schema version.
    // 同版本再次 update（无变化）不得冲掉已记录的表 ID。
    validator.update(ts, new_item.schema_ver, new_item.schema_ver, None);
    assert!(!validator.is_related_tables_changed(curr_ver, &[]));
    assert!(
        validator.is_related_tables_changed(curr_ver, &[2]),
        "currVer {curr_ver}, newItem {new_item:?}"
    );
    // The current schema version is older than the oldest schema version.
    // 比队列最旧版本还旧：无法证明安全。
    assert!(
        validator.is_related_tables_changed(-1, &[]),
        "currVer {curr_ver}, newItem {new_item:?}"
    );

    // All schema versions is expired.
    // 再推两个 lease，即使版本已最新也因租约过期返回 Unknown。
    ts = system_time_to_ts(after_two_leases + lease * 2);
    assert_eq!(
        validator.check(ts, new_item.schema_ver, None, true).1,
        Result::ResultUnknown,
        "schemaVer {}, validator {:?}",
        new_item.schema_ver,
        validator.snapshot()
    );

    exit_tx.send(()).expect("lease server stopped early");
    server.join().expect("lease server panicked");
}

// subTestEnqueue verifies delta compression, ordering, and maximum capacity.
/// 验证 enqueue 压缩、顺序与最大容量淘汰。
fn sub_test_enqueue() {
    let _restore = MaxDeltaSchemaCountRestore::capture();
    let validator = Validator::new(Duration::from_millis(10));
    assert!(validator.is_started());

    // maxCnt is 0.
    crate::vardef::SetMaxDeltaSchemaCount(0);
    validator.enqueue(1, Some(&change(&[11], &[11])));
    assert!(validator.snapshot().delta_schema_infos.is_empty());

    // maxCnt is 10.
    crate::vardef::SetMaxDeltaSchemaCount(10);
    let deltas = vec![
        delta(0, &[1], &[1]),
        delta(1, &[1], &[1]),
        delta(2, &[1], &[1]),
        delta(3, &[2, 2], &[2, 2]),
        delta(4, &[2], &[2]),
        delta(5, &[1, 4], &[1, 4]),
        delta(6, &[1, 4], &[1, 4]),
        delta(7, &[3, 1, 3], &[3, 1, 3]),
        delta(8, &[1, 2, 3], &[1, 2, 3]),
        delta(9, &[1, 2, 3], &[1, 2, 3]),
    ];
    for item in &deltas {
        validator.enqueue(
            item.schema_version,
            Some(&change(&item.related_ids, &item.related_actions)),
        );
    }
    validator.enqueue(10, Some(&change(&[1], &[1])));
    let mut expected = vec![
        delta(0, &[1], &[1]),
        delta(2, &[1], &[1]),
        delta(3, &[2, 2], &[2, 2]),
        delta(4, &[2], &[2]),
        delta(6, &[1, 4], &[1, 4]),
        delta(9, &[1, 2, 3], &[1, 2, 3]),
        delta(10, &[1], &[1]),
    ];
    assert_eq!(expected, validator.snapshot().delta_schema_infos);

    // The Items' relatedTableIDs have different order.
    // 表 ID 顺序不同但集合可覆盖时，末尾项被超集替换。
    validator.enqueue(11, Some(&change(&[1, 2, 3, 4], &[1, 2, 3, 4])));
    validator.enqueue(12, Some(&change(&[4, 1, 2, 3, 1], &[4, 1, 2, 3, 1])));
    validator.enqueue(13, Some(&change(&[4, 1, 3, 2, 5], &[4, 1, 3, 2, 5])));
    *expected.last_mut().expect("expected queue is not empty") =
        delta(13, &[4, 1, 3, 2, 5], &[4, 1, 3, 2, 5]);
    assert_eq!(expected, validator.snapshot().delta_schema_infos);

    // The length of deltaSchemaInfos is greater then maxCnt.
    // 超过 maxCnt 后队首被淘汰，快照等于 expected[1..]。
    for version in 14..=17 {
        validator.enqueue(version, Some(&change(&[version], &[version as u64])));
        expected.push(delta(version, &[version], &[version as u64]));
    }
    assert_eq!(expected[1..], validator.snapshot().delta_schema_infos);
}

// subTestEnqueueActionType also verifies that action types participate in compression.
/// 验证 action type 也参与 contain_in 压缩判定。
fn sub_test_enqueue_action_type() {
    let _restore = MaxDeltaSchemaCountRestore::capture();
    let validator = Validator::new(Duration::from_millis(10));
    assert!(validator.is_started());

    // maxCnt is 0.
    crate::vardef::SetMaxDeltaSchemaCount(0);
    validator.enqueue(1, Some(&change(&[11], &[11])));
    assert!(validator.snapshot().delta_schema_infos.is_empty());

    // maxCnt is 10.
    crate::vardef::SetMaxDeltaSchemaCount(10);
    let deltas = vec![
        delta(0, &[1], &[1]),
        delta(1, &[1], &[1]),
        delta(2, &[1], &[1]),
        delta(3, &[2, 2], &[2, 2]),
        delta(4, &[2], &[2]),
        delta(5, &[1, 4], &[1, 4]),
        delta(6, &[1, 4], &[1, 4]),
        delta(7, &[3, 1, 3], &[3, 1, 3]),
        delta(8, &[1, 2, 3], &[1, 2, 3]),
        delta(9, &[1, 2, 3], &[1, 2, 4]),
    ];
    for item in &deltas {
        validator.enqueue(
            item.schema_version,
            Some(&change(&item.related_ids, &item.related_actions)),
        );
    }
    validator.enqueue(10, Some(&change(&[1], &[15])));
    let expected = vec![
        delta(0, &[1], &[1]),
        delta(2, &[1], &[1]),
        delta(3, &[2, 2], &[2, 2]),
        delta(4, &[2], &[2]),
        delta(6, &[1, 4], &[1, 4]),
        delta(8, &[1, 2, 3], &[1, 2, 3]),
        delta(9, &[1, 2, 3], &[1, 2, 4]),
        delta(10, &[1], &[15]),
    ];
    assert_eq!(expected, validator.snapshot().delta_schema_infos);

    // tableID 3 has action flags in successive schema versions; the table is related.
    // 连续版本上表 3 有 action，判定为相关变更。
    assert!(validator.is_related_tables_changed(5, &[1, 2, 3, 4]));
}

/// 模拟租约服务器一次授予：时间戳 + 旧/新 schema 版本。
#[derive(Clone, Debug)]
struct LeaseGrantItem {
    lease_grant_ts: u64,
    old_ver: i64,
    schema_ver: i64,
}

/// 阻塞接收直到拿到 schema_ver 严格大于 `curr_ver` 的授予项。
fn get_greater_version_item(
    lease_grant_rx: &Receiver<LeaseGrantItem>,
    curr_ver: i64,
) -> LeaseGrantItem {
    let new_item = lease_grant_rx.recv().expect("lease server stopped early");
    assert!(
        new_item.schema_ver > curr_ver,
        "currVer {curr_ver}, newItem {new_item:?}"
    );
    new_item
}

// serverFunc plays the role as a remote server, runs in a separate goroutine.
// It can grant lease and provide timestamp oracle.
// Caller should communicate with it through channel to mock network.
/// 模拟远端租约/TSO 服务：经 channel 授予递增 schema 版本，收到 exit 退出。
fn server_func(require_lease: Sender<LeaseGrantItem>, exit: Receiver<()>) {
    let mut version = 0_i64;
    let mut lease_ts = system_time_to_ts(SystemTime::now());
    loop {
        let item = LeaseGrantItem {
            lease_grant_ts: lease_ts,
            old_ver: version - 1,
            schema_ver: version,
        };
        select! {
            send(require_lease, item) -> sent => {
                if sent.is_err() {
                    return;
                }
                version += 1;
                lease_ts = system_time_to_ts(SystemTime::now());
            }
            recv(exit) -> _ => return,
        }
    }
}
