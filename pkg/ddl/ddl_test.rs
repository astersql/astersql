// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 模块单元测试。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等
// 修改数据库对象元信息（schema）的语句。本文件测试 DDL 执行器中的
// 若干纯函数逻辑：
// - 重试间隔策略 `get_interval_from_policy`：DDL job（DDL 任务）轮询
//   等待时按策略取递增间隔，超出策略表长度后沿用最后一个间隔；
// - 建库/建表时的字符集与排序规则（charset/collation）解析，
//   重复且互相冲突的字符集选项需要报错；
// - 标识符长度校验 `check_identifier`：与 Go/TiDB 一致，
//   标识符（表名、列名等）最长 64 个字符。
//

use std::time::Duration;

// 被测函数均来自 DDL 执行器模块：
// - check_identifier: 校验标识符（表名/列名等）长度合法性；
// - get_interval_from_policy: 按重试策略表返回轮询间隔；
// - resolve_charset_collation: 解析字符集与排序规则选项。
use crate::ddl::{ActionType, Job, drop_or_truncate_table_info_from_jobs, recover_snapshot_ts};
use crate::executor::{check_identifier, get_interval_from_policy, resolve_charset_collation};

#[test]
fn recover_snapshot_prefers_real_start_ts_like_go() {
    let mut job = Job::new(1, 2, 3, "drop table t");
    job.start_ts = 10;
    job.real_start_ts = 20;

    assert_eq!(recover_snapshot_ts(&job), 20);
    job.real_start_ts = 0;
    assert_eq!(recover_snapshot_ts(&job), 10);
}

#[test]
fn recover_candidates_filter_actions_validate_gc_and_short_circuit_like_go() {
    let mut create = Job::new(1, 1, 1, "create table t(a int)");
    create.action_type = ActionType::Other;
    create.real_start_ts = 1;
    let mut drop = Job::new(2, 1, 2, "drop table t");
    drop.action_type = ActionType::DropTable;
    drop.start_ts = 20;
    let mut truncate = Job::new(3, 1, 3, "truncate table t");
    truncate.action_type = ActionType::TruncateTable;
    truncate.real_start_ts = 30;

    let mut visited = Vec::new();
    assert!(
        drop_or_truncate_table_info_from_jobs(&[create.clone(), drop, truncate], 10, |job| {
            visited.push(job.id);
            job.id == 3
        })
        .unwrap()
    );
    assert_eq!(visited, vec![2, 3]);

    create.action_type = ActionType::DropTable;
    assert!(drop_or_truncate_table_info_from_jobs(&[create], 10, |_| false).is_err());
}

/// 验证重试间隔策略：索引在策略表范围内时返回对应间隔且标记 `changed=true`；
/// 索引超出策略表后固定返回最后一个间隔并标记 `changed=false`。
/// 该机制用于 DDL job 等待时的退避（backoff）轮询。
#[test]
fn interval_policy_uses_last_value_after_exhaustion() {
    let policy = [Duration::from_secs(1), Duration::from_secs(2)];
    assert_eq!(
        (Duration::from_secs(1), true),
        get_interval_from_policy(&policy, 0)
    );
    assert_eq!(
        (Duration::from_secs(2), true),
        get_interval_from_policy(&policy, 1)
    );
    assert_eq!(
        (Duration::from_secs(2), false),
        get_interval_from_policy(&policy, 2)
    );
    assert_eq!(
        (Duration::from_secs(2), false),
        get_interval_from_policy(&policy, 3)
    );
}

/// 验证字符集/排序规则解析：
/// - 未指定选项时回落到默认排序规则推导出的字符集（utf8mb4/utf8mb4_bin）；
/// - 显式指定字符集时采用该字符集及其默认排序规则；
/// - 同时指定多个互相冲突的字符集（utf8 与 utf8mb4）时必须返回错误，
///   对应 MySQL 中 CREATE DATABASE ... CHARACTER SET 选项冲突的语义。
#[test]
fn schema_charset_options_reject_conflicting_duplicates() {
    assert_eq!(
        ("utf8mb4".into(), "utf8mb4_bin".into()),
        resolve_charset_collation(&[], "utf8mb4_bin").unwrap()
    );
    assert_eq!(
        ("utf8".into(), "utf8_bin".into()),
        resolve_charset_collation(&[(Some("utf8".into()), None)], "utf8mb4_bin").unwrap()
    );
    assert!(
        resolve_charset_collation(
            &[(Some("utf8".into()), None), (Some("utf8mb4".into()), None),],
            "utf8mb4_bin",
        )
        .is_err()
    );
}

/// 验证标识符长度边界与 Go/TiDB 保持一致：
/// 空标识符非法，长度恰为 64 个字符合法，65 个字符则超限报错
/// （对应 MySQL 的最大标识符长度限制）。
#[test]
fn ddl_identifiers_enforce_the_go_length_boundary() {
    assert!(check_identifier("table_name", "table").is_ok());
    assert!(check_identifier("", "table").is_err());
    assert!(check_identifier(&"x".repeat(64), "table").is_ok());
    assert!(check_identifier(&"x".repeat(65), "table").is_err());
}
