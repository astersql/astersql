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

// Schema Validator（元数据租约校验器）迁移单元测试。
//
// 对照 Go 行为核对：生命周期与租约（lease）过期、schema delta 与 MDL
// （Metadata Lock，元数据锁）语义、enqueue 压缩与容量上限，以及 action type
// 参与压缩时的保留结果。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::validator::{
    DeltaSchemaInfo, RelatedSchemaChange, Result, Validator, contain_in, system_time_to_ts,
    unix_seconds,
};

/// 捕获并在 Drop 时还原 vardef 中与本测试相关的全局开关，避免污染并行用例。
struct VardefRestore {
    max_delta_count: i64,
    mdl_enabled: bool,
}

impl VardefRestore {
    /// 记录当前 `MaxDeltaSchemaCount` 与 MDL 开关。
    fn capture() -> Self {
        Self {
            max_delta_count: super::vardef::GetMaxDeltaSchemaCount(),
            mdl_enabled: super::vardef::IsMDLEnabled(),
        }
    }
}

impl Drop for VardefRestore {
    fn drop(&mut self) {
        super::vardef::SetMaxDeltaSchemaCount(self.max_delta_count);
        super::vardef::SetEnableMDL(self.mdl_enabled);
    }
}

/// 构造一条 RelatedSchemaChange 载荷：物理表 ID 与 DDL action 一一对应。
fn change(ids: &[i64], actions: &[u64]) -> RelatedSchemaChange {
    RelatedSchemaChange {
        phy_tbl_ids: ids.to_vec(),
        action_types: actions.to_vec(),
    }
}

/// 生命周期：启动、租约内成功、超出租约 Unknown、stop 后拒绝更新、restart 后旧版本 Fail、reset 清空。
#[test]
fn lifecycle_and_lease_checks_match_go() {
    // 编译期确认 Validator 实现 validatorapi::Validator。
    fn assert_validator_api<
        T: super::validatorapi::Validator<RelatedSchemaChange = RelatedSchemaChange>,
    >() {
    }
    assert_validator_api::<Validator>();

    let lease = Duration::from_millis(20);
    let validator = Validator::new(lease);
    assert!(validator.is_started());

    let grant_time = SystemTime::now();
    let grant_ts = system_time_to_ts(grant_time);
    // 授予租约并刷新到 schema ver 0；租约未过期时应 ResultSucc。
    validator.update(grant_ts, -1, 0, None);
    assert_eq!(
        validator.check(grant_ts, 0, Some(&[]), true).1,
        Result::ResultSucc
    );

    // 事务时间戳超过 latest_schema_expire（约 2 个 lease）后结果为 Unknown。
    let after_two_leases = system_time_to_ts(grant_time + lease * 2);
    assert_eq!(
        validator.check(after_two_leases, 0, Some(&[]), true).1,
        Result::ResultUnknown
    );

    // stop 后 update 被忽略；check 返回 Unknown。
    validator.stop();
    assert!(!validator.is_started());
    validator.update(grant_ts, 0, 9, Some(&change(&[9], &[9])));
    assert_eq!(validator.snapshot().latest_schema_ver, 0);
    assert_eq!(
        validator.check(grant_ts, 0, Some(&[]), true).1,
        Result::ResultUnknown
    );

    // restart 记录 restart_schema_ver；更旧的 schema_ver 在提交前判定 Fail。
    validator.restart(3);
    assert!(validator.is_started());
    assert_eq!(
        validator.check(grant_ts, 2, Some(&[]), true).1,
        Result::ResultFail
    );

    validator.reset();
    let snapshot = validator.snapshot();
    assert!(snapshot.is_started);
    assert_eq!(snapshot.latest_schema_ver, 0);
    assert_eq!(snapshot.restart_schema_ver, 0);
    assert!(snapshot.delta_schema_infos.is_empty());
}

/// 默认 Go 构建不启用 intest.Assert，因此零租约构造不会 panic。
#[test]
fn zero_lease_is_accepted_without_intest_assertions() {
    let validator = Validator::new(Duration::ZERO);
    assert!(validator.is_started());
}

/// Go time.Time.Unix 对 epoch 前的亚秒时间向负无穷取整。
#[test]
fn unix_seconds_floors_subsecond_times_before_epoch() {
    assert_eq!(unix_seconds(UNIX_EPOCH - Duration::from_micros(1)), -1);
    assert_eq!(unix_seconds(UNIX_EPOCH - Duration::from_secs(1)), -1);
}

/// 关闭 MDL 时按 delta 判定相关表变更；开启 MDL 且不要求 delta 检查时可 Succ。
#[test]
fn schema_delta_checks_preserve_nil_empty_and_mdl_semantics() {
    let _lock = crate::VARDEF_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = VardefRestore::capture();
    super::vardef::SetMaxDeltaSchemaCount(10);
    super::vardef::SetEnableMDL(false);

    let validator = Validator::new(Duration::from_millis(20));
    let grant_ts = system_time_to_ts(SystemTime::now());
    // 连续两个版本：表 10 再表 20。
    validator.update(grant_ts, 0, 1, Some(&change(&[10], &[1])));
    validator.update(grant_ts, 1, 2, Some(&change(&[20], &[2])));

    // 事务仍用 ver=1：只关心表 10 时成功；关心表 20 / nil / 空切片时失败。
    assert_eq!(
        validator.check(grant_ts, 1, Some(&[10]), true).1,
        Result::ResultSucc
    );
    assert_eq!(
        validator.check(grant_ts, 1, Some(&[20]), true).1,
        Result::ResultFail
    );
    assert_eq!(
        validator.check(grant_ts, 1, None, true).1,
        Result::ResultFail
    );
    assert_eq!(
        validator.check(grant_ts, 0, Some(&[]), true).1,
        Result::ResultFail
    );

    // MDL 开启且 need_check_schema_by_delta=false 时跳过 delta 相关表检查。
    super::vardef::SetEnableMDL(true);
    assert_eq!(
        validator.check(grant_ts, 1, Some(&[20]), false).1,
        Result::ResultSucc
    );
    assert!(!validator.is_related_tables_changed(1, &[10]));
    assert!(validator.is_related_tables_changed(1, &[20]));
    // curr_ver=-1 比队列中任何版本都旧，视为可能丢失历史，判定已变更。
    assert!(validator.is_related_tables_changed(-1, &[]));
}

/// enqueue：max_count=0 丢弃；相同表集合可压缩；超长队列从队头淘汰。
#[test]
fn enqueue_compression_and_capacity_match_go() {
    let _lock = crate::VARDEF_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = VardefRestore::capture();
    super::vardef::SetMaxDeltaSchemaCount(0);

    let validator = Validator::new(Duration::from_millis(10));
    validator.enqueue(1, Some(&change(&[11], &[11])));
    assert!(validator.snapshot().delta_schema_infos.is_empty());

    super::vardef::SetMaxDeltaSchemaCount(10);
    // 与 Go 单测相同的 delta 序列，验证 contain_in 压缩后的版本号集合。
    let deltas = [
        (0, vec![1], vec![1]),
        (1, vec![1], vec![1]),
        (2, vec![1], vec![1]),
        (3, vec![2, 2], vec![2, 2]),
        (4, vec![2], vec![2]),
        (5, vec![1, 4], vec![1, 4]),
        (6, vec![1, 4], vec![1, 4]),
        (7, vec![3, 1, 3], vec![3, 1, 3]),
        (8, vec![1, 2, 3], vec![1, 2, 3]),
        (9, vec![1, 2, 3], vec![1, 2, 3]),
    ];
    for (version, ids, actions) in deltas {
        validator.enqueue(version, Some(&change(&ids, &actions)));
    }
    validator.enqueue(10, Some(&change(&[1], &[1])));
    assert_eq!(
        validator
            .snapshot()
            .delta_schema_infos
            .iter()
            .map(|delta| delta.schema_version)
            .collect::<Vec<_>>(),
        vec![0, 2, 3, 4, 6, 9, 10]
    );

    // 后写入的超集可覆盖队列末尾项（不合并队首）。
    validator.enqueue(11, Some(&change(&[1, 2, 3, 4], &[1, 2, 3, 4])));
    validator.enqueue(12, Some(&change(&[4, 1, 2, 3, 1], &[4, 1, 2, 3, 1])));
    validator.enqueue(13, Some(&change(&[4, 1, 3, 2, 5], &[4, 1, 3, 2, 5])));
    let expected = vec![
        DeltaSchemaInfo {
            schema_version: 0,
            related_ids: vec![1],
            related_actions: vec![1],
        },
        DeltaSchemaInfo {
            schema_version: 2,
            related_ids: vec![1],
            related_actions: vec![1],
        },
        DeltaSchemaInfo {
            schema_version: 3,
            related_ids: vec![2, 2],
            related_actions: vec![2, 2],
        },
        DeltaSchemaInfo {
            schema_version: 4,
            related_ids: vec![2],
            related_actions: vec![2],
        },
        DeltaSchemaInfo {
            schema_version: 6,
            related_ids: vec![1, 4],
            related_actions: vec![1, 4],
        },
        DeltaSchemaInfo {
            schema_version: 9,
            related_ids: vec![1, 2, 3],
            related_actions: vec![1, 2, 3],
        },
        DeltaSchemaInfo {
            schema_version: 13,
            related_ids: vec![4, 1, 3, 2, 5],
            related_actions: vec![4, 1, 3, 2, 5],
        },
    ];
    assert_eq!(validator.snapshot().delta_schema_infos, expected);

    // 再入队 4 项使长度超过 max=10，队首旧版本被移除。
    for version in 14..=17 {
        validator.enqueue(version, Some(&change(&[version], &[version as u64])));
    }
    let snapshot = validator.snapshot();
    let mut expected_after_eviction = expected;
    for version in 14..=17 {
        expected_after_eviction.push(DeltaSchemaInfo {
            schema_version: version,
            related_ids: vec![version],
            related_actions: vec![version as u64],
        });
    }
    assert_eq!(snapshot.delta_schema_infos, expected_after_eviction[1..]);

    // contain_in：last 的每个 (table, action) 对都能在 current 中找到。
    assert!(contain_in(
        &DeltaSchemaInfo {
            schema_version: 1,
            related_ids: vec![1, 2],
            related_actions: vec![3, 4],
        },
        &DeltaSchemaInfo {
            schema_version: 2,
            related_ids: vec![2, 1, 5],
            related_actions: vec![4, 3, 6],
        }
    ));
}

/// action type 不同时即使表 ID 相同也不可压缩合并。
#[test]
fn enqueue_action_type_compression_matches_go() {
    let _lock = crate::VARDEF_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = VardefRestore::capture();
    super::vardef::SetMaxDeltaSchemaCount(10);

    let validator = Validator::new(Duration::from_millis(10));
    // ver 8 与 9 表 ID 相同但 action[2] 分别为 3 与 4，必须各自保留。
    let deltas = [
        (0, vec![1], vec![1]),
        (1, vec![1], vec![1]),
        (2, vec![1], vec![1]),
        (3, vec![2, 2], vec![2, 2]),
        (4, vec![2], vec![2]),
        (5, vec![1, 4], vec![1, 4]),
        (6, vec![1, 4], vec![1, 4]),
        (7, vec![3, 1, 3], vec![3, 1, 3]),
        (8, vec![1, 2, 3], vec![1, 2, 3]),
        (9, vec![1, 2, 3], vec![1, 2, 4]),
    ];
    for (version, ids, actions) in deltas {
        validator.enqueue(version, Some(&change(&ids, &actions)));
    }
    validator.enqueue(10, Some(&change(&[1], &[15])));

    let expected = vec![
        DeltaSchemaInfo {
            schema_version: 0,
            related_ids: vec![1],
            related_actions: vec![1],
        },
        DeltaSchemaInfo {
            schema_version: 2,
            related_ids: vec![1],
            related_actions: vec![1],
        },
        DeltaSchemaInfo {
            schema_version: 3,
            related_ids: vec![2, 2],
            related_actions: vec![2, 2],
        },
        DeltaSchemaInfo {
            schema_version: 4,
            related_ids: vec![2],
            related_actions: vec![2],
        },
        DeltaSchemaInfo {
            schema_version: 6,
            related_ids: vec![1, 4],
            related_actions: vec![1, 4],
        },
        DeltaSchemaInfo {
            schema_version: 8,
            related_ids: vec![1, 2, 3],
            related_actions: vec![1, 2, 3],
        },
        DeltaSchemaInfo {
            schema_version: 9,
            related_ids: vec![1, 2, 3],
            related_actions: vec![1, 2, 4],
        },
        DeltaSchemaInfo {
            schema_version: 10,
            related_ids: vec![1],
            related_actions: vec![15],
        },
    ];
    assert_eq!(validator.snapshot().delta_schema_infos, expected);
    assert!(validator.is_related_tables_changed(5, &[1, 2, 3, 4]));
}
