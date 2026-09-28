// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// `RunningJobs`（运行中 DDL 作业冲突表）的单元测试。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等修改
// 库表结构的语句。为了允许多条 DDL 并发执行，调度器需要维护一张
// "运行中作业涉及对象" 的记录表：每个作业声明它涉及的数据库/表、
// Placement Policy（放置策略，控制数据副本分布位置的规则）以及
// Resource Group（资源组，用于限制作业可用的 CPU/IO 资源），
// 并区分独占（exclusive）与共享（shared）两种占用模式。
// 新作业只有在与所有运行中作业、以及先到的 pending（等待中）作业
// 都不冲突时才允许运行，从而保证公平性（先来的作业不会被饿死）。
//
// 本文件测试的核心规则：
// - 同一张表的独占占用互斥；`db.*` 与 `*.*` 通配符会与更细粒度的对象冲突；
// - 共享模式之间可以并存，但与独占模式互斥；
// - 一旦某对象上出现 pending 作业，后续到达的作业（即使是共享模式）
//   也必须排队，直到 `reset_all_pending` 开启新一轮调度。
//
// 文件开头保留了 Go(TiDB) 原测试的机械翻译版本（已整体注释掉），
// 其后的活动代码是等价语义的 Rust 重写测试。

/*
//

pub fn mkJob(id: i64, schema_table_names: &[&str]) -> (i64, Vec<model::InvolvingSchemaInfo>) {
    let mut schema_infos = Vec::with_capacity(schema_table_names.len());
    for schema_table_name in schema_table_names {
        let ss: Vec<&str> = schema_table_name.split('.').collect();
        schema_infos.push(model::InvolvingSchemaInfo {
            Database: ss[0].to_string(),
            Table: ss[1].to_string(),
            ..Default::default()
        });
    }
    (id, schema_infos)
}

pub fn checkInvariants(j: &runningJobs) {
    // Go 测试逐个检查 exclusive/shared/pending 的计数映射均为正数。
    for checking_obj in [&j.exclusive, &j.shared, &j.pending] {
        for tables in checking_obj.schemas.values() {
            require::Greater(t, tables.len(), 0);
            for v in tables.values() {
                require::Greater(t, *v, 0);
            }
        }
        for v in checking_obj.placementPolicies.values() {
            require::Greater(t, *v, 0);
        }
        for v in checking_obj.resourceGroups.values() {
            require::Greater(t, *v, 0);
        }
    }
}

pub fn orderedAllIDs(ids: &str) -> String {
    if ids.is_empty() {
        return String::new();
    }

    let mut ssid: Vec<i32> = ids
        .split(',')
        .map(|s| s.parse::<i32>().unwrap_or_default())
        .collect();
    ssid.sort();
    ssid.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
}

#[test]
fn test_running_jobs() {
    let mut j = newRunningJobs();
    require::Equal(t, "", j.allIDs());
    checkInvariants(j);

    let mut runnable = j.checkRunnable(mkJob(0, &["db1.t1"]));
    require::True(t, runnable);

    let (job_id1, involves1) = mkJob(1, &["db1.t1", "db1.t2"]);
    runnable = j.checkRunnable(job_id1, involves1.clone());
    require::True(t, runnable);
    j.addRunning(job_id1, involves1.clone());
    let (job_id2, involves2) = mkJob(2, &["db2.t3"]);
    runnable = j.checkRunnable(job_id2, involves2.clone());
    require::True(t, runnable);
    j.addRunning(job_id2, involves2.clone());
    require::Equal(t, "1,2", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    require::False(t, j.checkRunnable(mkJob(0, &["db1.t1"])));
    require::False(t, j.checkRunnable(mkJob(0, &["db1.t2"])));
    require::False(t, j.checkRunnable(mkJob(0, &["db3.t4", "db1.t1"])));
    require::True(t, j.checkRunnable(mkJob(0, &["db3.t4", "db4.t5"])));

    let (job_id3, involves3) = mkJob(3, &["db1.*"]);
    require::False(t, j.checkRunnable(job_id3, involves3.clone()));
    j.removeRunning(job_id1, involves1.clone());
    require::True(t, j.checkRunnable(job_id3, involves3.clone()));
    j.addRunning(job_id3, involves3.clone());
    require::Equal(t, "2,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    require::False(t, j.checkRunnable(mkJob(0, &["db1.t100"])));

    let (job_id4, involves4) = mkJob(4, &["db4.t100", "db2.t6"]);
    require::True(t, j.checkRunnable(job_id4, involves4.clone()));
    j.addRunning(job_id4, involves4.clone());
    require::Equal(t, "2,3,4", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    let (job_id5, involves5) = mkJob(5, &["*.*"]);
    require::False(t, j.checkRunnable(job_id5, involves5.clone()));

    j.removeRunning(job_id2, involves2.clone());
    j.removeRunning(job_id3, involves3.clone());
    j.removeRunning(job_id4, involves4.clone());
    require::Equal(t, "", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    require::True(t, j.checkRunnable(job_id5, involves5.clone()));
    j.addRunning(job_id5, involves5.clone());
    require::Equal(t, "5", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    require::False(t, j.checkRunnable(mkJob(0, &["db1.t1"])));
}

#[test]
fn test_schema_policy_and_resource_group() {
    let mut j = newRunningJobs();

    let (job_id1, involves1) = mkJob(1, &["db1.t1", "db1.t2"]);
    require::True(t, j.checkRunnable(job_id1, involves1.clone()));
    j.addRunning(job_id1, involves1.clone());

    let mut failed_involves = vec![
        model::InvolvingSchemaInfo { Policy: "p0".into(), ..Default::default() },
        model::InvolvingSchemaInfo {
            Database: "db1".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
    ];
    require::False(t, j.checkRunnable(0, failed_involves.clone()));

    failed_involves = vec![
        model::InvolvingSchemaInfo {
            Database: model::InvolvingAll.into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo { ResourceGroup: "g0".into(), ..Default::default() },
    ];
    require::False(t, j.checkRunnable(0, failed_involves.clone()));

    let job_id2 = 2_i64;
    let involves2 = vec![
        model::InvolvingSchemaInfo {
            Database: "db2".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo { Policy: "p0".into(), ..Default::default() },
        model::InvolvingSchemaInfo { ResourceGroup: "g0".into(), ..Default::default() },
    ];
    require::True(t, j.checkRunnable(job_id2, involves2.clone()));
    j.addRunning(job_id2, involves2.clone());

    let job_id3 = 3_i64;
    let involves3 = vec![
        model::InvolvingSchemaInfo { Policy: "p1".into(), ..Default::default() },
        model::InvolvingSchemaInfo { ResourceGroup: "g1".into(), ..Default::default() },
    ];
    require::True(t, j.checkRunnable(job_id3, involves3.clone()));
    j.addRunning(job_id3, involves3.clone());
    require::Equal(t, "1,2,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    failed_involves = vec![model::InvolvingSchemaInfo { ResourceGroup: "g0".into(), ..Default::default() }];
    require::False(t, j.checkRunnable(0, failed_involves.clone()));

    j.removeRunning(job_id2, involves2.clone());
    require::Equal(t, "1,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    let job_id4 = 4_i64;
    let involves4 = vec![
        model::InvolvingSchemaInfo { Policy: "p0".into(), ..Default::default() },
        model::InvolvingSchemaInfo {
            Database: "db3".into(),
            Table: "t3".into(),
            ..Default::default()
        },
    ];
    require::True(t, j.checkRunnable(job_id4, involves4.clone()));
    j.addRunning(job_id4, involves4.clone());
    require::Equal(t, "1,3,4", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    failed_involves = vec![model::InvolvingSchemaInfo {
        Database: "db3".into(),
        Table: "t3".into(),
        ..Default::default()
    }];
    require::False(t, j.checkRunnable(0, failed_involves.clone()));
    failed_involves = vec![model::InvolvingSchemaInfo { Policy: "p1".into(), ..Default::default() }];
    require::False(t, j.checkRunnable(0, failed_involves.clone()));
}

#[test]
fn test_exclusive_shared() {
    let mut j = newRunningJobs();

    let (job_id1, involves1) = mkJob(1, &["db1.t1", "db1.t2"]);
    require::True(t, j.checkRunnable(job_id1, involves1.clone()));
    j.addRunning(job_id1, involves1.clone());

    let failed_involves = vec![
        model::InvolvingSchemaInfo {
            Database: "db2".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            Database: "db1".into(),
            Table: "t1".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::False(t, j.checkRunnable(0, failed_involves));

    let job_id2 = 2_i64;
    let involves2 = vec![
        model::InvolvingSchemaInfo {
            Database: "db3".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            Database: "db2".into(),
            Table: "t2".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::True(t, j.checkRunnable(job_id2, involves2.clone()));
    j.addRunning(job_id2, involves2.clone());

    let job_id3 = 3_i64;
    let involves3 = vec![
        model::InvolvingSchemaInfo {
            Database: "db4".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            Database: "db2".into(),
            Table: "t2".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::True(t, j.checkRunnable(job_id3, involves3.clone()));
    j.addRunning(job_id3, involves3.clone());
    require::Equal(t, "1,2,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    let mut pending_involves = vec![model::InvolvingSchemaInfo {
        Database: "db2".into(),
        Table: "t2".into(),
        ..Default::default()
    }];
    require::False(t, j.checkRunnable(0, pending_involves.clone()));
    j.addPending(pending_involves.clone());
    require::Equal(t, "1,2,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    // 因为 db2.t2 上已经有 pending job，后续同对象的 shared job 也应被阻塞。
    let job_id4 = 4_i64;
    let involves4 = vec![
        model::InvolvingSchemaInfo {
            Database: "db100".into(),
            Table: model::InvolvingAll.into(),
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            Database: "db2".into(),
            Table: "t2".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::False(t, j.checkRunnable(job_id4, involves4.clone()));
    require::Equal(t, "1,2,3", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    // 模拟一轮 running job 完成后重新扫描 pending 队列。
    j.resetAllPending();
    j.removeRunning(job_id1, involves1.clone());
    j.removeRunning(job_id2, involves2.clone());
    j.removeRunning(job_id3, involves3.clone());
    checkInvariants(j);
    require::True(t, j.checkRunnable(0, pending_involves.clone()));

    let job_id5 = 5_i64;
    let involves5 = vec![
        model::InvolvingSchemaInfo {
            Policy: "p1".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            Policy: "p2".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::True(t, j.checkRunnable(job_id5, involves5.clone()));
    j.addRunning(job_id5, involves5.clone());

    let job_id6 = 6_i64;
    let involves6 = vec![
        model::InvolvingSchemaInfo {
            Policy: "p1".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
        model::InvolvingSchemaInfo {
            ResourceGroup: "g1".into(),
            Mode: model::SharedInvolving,
            ..Default::default()
        },
    ];
    require::True(t, j.checkRunnable(job_id6, involves6.clone()));
    j.addRunning(job_id6, involves6.clone());
    require::Equal(t, "5,6", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    pending_involves = vec![
        model::InvolvingSchemaInfo { Policy: "p1".into(), ..Default::default() },
        model::InvolvingSchemaInfo { ResourceGroup: "g2".into(), ..Default::default() },
    ];
    require::False(t, j.checkRunnable(0, pending_involves.clone()));
    j.addPending(pending_involves.clone());

    let second_pending_involves = vec![
        model::InvolvingSchemaInfo { ResourceGroup: "g2".into(), ..Default::default() },
        model::InvolvingSchemaInfo { ResourceGroup: "g3".into(), ..Default::default() },
    ];
    require::False(t, j.checkRunnable(0, second_pending_involves.clone()));
    j.addPending(second_pending_involves.clone());

    // 一个 shared p1 完成后进入下一轮，pending 仍然应维持阻塞关系。
    j.removeRunning(job_id6, involves6.clone());
    require::Equal(t, "5", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);
    j.resetAllPending();

    require::False(t, j.checkRunnable(0, pending_involves.clone()));
    j.addPending(pending_involves.clone());
    require::False(t, j.checkRunnable(0, second_pending_involves.clone()));
    j.addPending(second_pending_involves.clone());

    j.removeRunning(job_id5, involves5.clone());
    require::Equal(t, "", orderedAllIDs(&j.allIDs()));
    checkInvariants(j);

    let third_pending_involves = vec![model::InvolvingSchemaInfo { Policy: "p1".into(), ..Default::default() }];
    require::False(t, j.checkRunnable(0, third_pending_involves.clone()));
    j.addPending(third_pending_involves);

    // 新一轮开始后，最早的 pending job 可以运行。
    j.resetAllPending();
    require::True(t, j.checkRunnable(0, pending_involves));
}
*/

use crate::ddl_running_jobs::{INVOLVING_ALL, InvolvingSchemaInfo, RunningJobs};

/// 构造一个测试作业：把 `"db.table"` 形式的字符串列表解析为
/// `InvolvingSchemaInfo`（作业涉及的库表对象描述）集合，返回 (作业 ID, 涉及对象列表)。
fn job(id: i64, names: &[&str]) -> (i64, Vec<InvolvingSchemaInfo>) {
    let involves = names
        .iter()
        .map(|name| {
            let (database, table) = name
                .split_once('.')
                .expect("schema object must use database.table form");
            InvolvingSchemaInfo::schema(database, table)
        })
        .collect();
    (id, involves)
}

/// 断言 `RunningJobs` 内部不变量成立：
/// exclusive/shared/pending 三张计数表中的引用计数必须全部为正数
/// （计数归零的条目应被及时清除，否则会产生虚假冲突）。
fn assert_invariants(jobs: &RunningJobs) {
    assert!(jobs.invariants_hold());
}

/// 验证库表级独占冲突规则与 Go 版本一致：
/// 精确表名互斥、`db.*` 与该库下所有表冲突、`*.*` 与一切对象冲突。
#[test]
fn running_schema_jobs_follow_go_conflict_rules() {
    let mut jobs = RunningJobs::default();
    // 空表时任何作业都可运行。
    assert_eq!("", jobs.all_ids());
    assert_invariants(&jobs);
    assert!(jobs.check_runnable(0, &job(0, &["db1.t1"]).1));

    let (job_id1, involves1) = job(1, &["db1.t1", "db1.t2"]);
    assert!(jobs.check_runnable(job_id1, &involves1));
    jobs.add_running(job_id1, involves1);
    let (job_id2, involves2) = job(2, &["db2.t3"]);
    assert!(jobs.check_runnable(job_id2, &involves2));
    jobs.add_running(job_id2, involves2);
    assert_eq!("1,2", jobs.all_ids());
    assert_invariants(&jobs);

    // 与运行中作业涉及的表重叠即冲突；只要有一个对象冲突，整个作业就不可运行。
    assert!(!jobs.check_runnable(0, &job(0, &["db1.t1"]).1));
    assert!(!jobs.check_runnable(0, &job(0, &["db1.t2"]).1));
    assert!(!jobs.check_runnable(0, &job(0, &["db3.t4", "db1.t1"]).1));
    assert!(jobs.check_runnable(0, &job(0, &["db3.t4", "db4.t5"]).1));

    // `db1.*` 表示涉及 db1 库下的所有表（如 DROP DATABASE），
    // 必须等 db1 上的表级作业结束后才能运行。
    let (job_id3, involves3) = job(3, &["db1.*"]);
    assert!(!jobs.check_runnable(job_id3, &involves3));
    jobs.remove_running(job_id1);
    assert!(jobs.check_runnable(job_id3, &involves3));
    jobs.add_running(job_id3, involves3);
    assert_eq!("2,3", jobs.all_ids());
    assert!(!jobs.check_runnable(0, &job(0, &["db1.t100"]).1));

    let (job_id4, involves4) = job(4, &["db4.t100", "db2.t6"]);
    assert!(jobs.check_runnable(job_id4, &involves4));
    jobs.add_running(job_id4, involves4);
    assert_eq!("2,3,4", jobs.all_ids());

    // `*.*` 表示涉及全部库表（如 FLASHBACK CLUSTER），与任何运行中作业都冲突。
    let (job_id5, involves5) = job(5, &["*.*"]);
    assert!(!jobs.check_runnable(job_id5, &involves5));
    jobs.remove_running(job_id2);
    jobs.remove_running(job_id3);
    jobs.remove_running(job_id4);
    assert_eq!("", jobs.all_ids());
    assert_invariants(&jobs);

    assert!(jobs.check_runnable(job_id5, &involves5));
    jobs.add_running(job_id5, involves5);
    assert_eq!("5", jobs.all_ids());
    assert!(!jobs.check_runnable(0, &job(0, &["db1.t1"]).1));
}

/// 验证 Placement Policy 与 Resource Group 也参与冲突检测：
/// 涉及同一策略/资源组的两个独占作业互斥，且 `*.*` 通配符会与其一起冲突。
#[test]
fn schema_policy_and_resource_group_conflicts_match_go() {
    let mut jobs = RunningJobs::default();
    let (job_id1, involves1) = job(1, &["db1.t1", "db1.t2"]);
    jobs.add_running(job_id1, involves1);

    // 虽然策略 p0 不冲突，但 `db1.*` 与运行中的 db1.t1/db1.t2 冲突，整体不可运行。
    let failed = vec![
        InvolvingSchemaInfo::policy("p0"),
        InvolvingSchemaInfo::schema("db1", INVOLVING_ALL),
    ];
    assert!(!jobs.check_runnable(0, &failed));
    let failed = vec![
        InvolvingSchemaInfo::schema(INVOLVING_ALL, INVOLVING_ALL),
        InvolvingSchemaInfo::resource_group("g0"),
    ];
    assert!(!jobs.check_runnable(0, &failed));

    let involves2 = vec![
        InvolvingSchemaInfo::schema("db2", INVOLVING_ALL),
        InvolvingSchemaInfo::policy("p0"),
        InvolvingSchemaInfo::resource_group("g0"),
    ];
    assert!(jobs.check_runnable(2, &involves2));
    jobs.add_running(2, involves2);
    let involves3 = vec![
        InvolvingSchemaInfo::policy("p1"),
        InvolvingSchemaInfo::resource_group("g1"),
    ];
    assert!(jobs.check_runnable(3, &involves3));
    jobs.add_running(3, involves3);
    assert_eq!("1,2,3", jobs.all_ids());
    // 资源组 g0 已被作业 2 独占，涉及它的新作业必须等待。
    assert!(!jobs.check_runnable(0, &[InvolvingSchemaInfo::resource_group("g0")]));

    // 移除作业 2 后其占用的 db2.*、p0、g0 均被释放。
    jobs.remove_running(2);
    assert_eq!("1,3", jobs.all_ids());
    let involves4 = vec![
        InvolvingSchemaInfo::policy("p0"),
        InvolvingSchemaInfo::schema("db3", "t3"),
    ];
    assert!(jobs.check_runnable(4, &involves4));
    jobs.add_running(4, involves4);
    assert_eq!("1,3,4", jobs.all_ids());
    assert!(!jobs.check_runnable(0, &[InvolvingSchemaInfo::schema("db3", "t3")]));
    assert!(!jobs.check_runnable(0, &[InvolvingSchemaInfo::policy("p1")]));
    assert_invariants(&jobs);
}

/// 验证独占/共享模式与 pending 队列的公平性：
/// 共享占用之间可并存；独占与任何占用互斥；
/// 一旦某对象上有 pending 作业，后来者（含共享作业）必须排队，
/// 直到 `reset_all_pending` 开启新一轮调度才重新判定。
#[test]
fn exclusive_shared_and_pending_jobs_are_fair() {
    let mut jobs = RunningJobs::default();
    let (job_id1, involves1) = job(1, &["db1.t1", "db1.t2"]);
    jobs.add_running(job_id1, involves1);

    // db1.t1 已被作业 1 独占，即使以共享模式申请也会冲突。
    let failed = vec![
        InvolvingSchemaInfo::schema("db2", INVOLVING_ALL),
        InvolvingSchemaInfo::schema("db1", "t1").shared(),
    ];
    assert!(!jobs.check_runnable(0, &failed));

    // 作业 2 与作业 3 都以共享模式占用 db2.t2，共享之间可以并存。
    let involves2 = vec![
        InvolvingSchemaInfo::schema("db3", INVOLVING_ALL),
        InvolvingSchemaInfo::schema("db2", "t2").shared(),
    ];
    assert!(jobs.check_runnable(2, &involves2));
    jobs.add_running(2, involves2);
    let involves3 = vec![
        InvolvingSchemaInfo::schema("db4", INVOLVING_ALL),
        InvolvingSchemaInfo::schema("db2", "t2").shared(),
    ];
    assert!(jobs.check_runnable(3, &involves3));
    jobs.add_running(3, involves3);

    // 独占申请 db2.t2 与共享占用冲突，被记入 pending 队列。
    let pending = vec![InvolvingSchemaInfo::schema("db2", "t2")];
    assert!(!jobs.check_runnable(0, &pending));
    jobs.add_pending(pending.clone());
    // db2.t2 上已有 pending 作业，后到的共享作业也必须排队，防止独占作业被饿死。
    let shared_after_pending = vec![
        InvolvingSchemaInfo::schema("db100", INVOLVING_ALL),
        InvolvingSchemaInfo::schema("db2", "t2").shared(),
    ];
    assert!(!jobs.check_runnable(4, &shared_after_pending));

    // 新一轮调度：清空 pending 记录并移除已完成的作业后，原 pending 作业可运行。
    jobs.reset_all_pending();
    jobs.remove_running(1);
    jobs.remove_running(2);
    jobs.remove_running(3);
    assert!(jobs.check_runnable(0, &pending));

    // 共享模式同样适用于放置策略与资源组：p1 可被作业 5、6 同时共享。
    let involves5 = vec![
        InvolvingSchemaInfo::policy("p1").shared(),
        InvolvingSchemaInfo::policy("p2").shared(),
    ];
    jobs.add_running(5, involves5);
    let involves6 = vec![
        InvolvingSchemaInfo::policy("p1").shared(),
        InvolvingSchemaInfo::resource_group("g1").shared(),
    ];
    assert!(jobs.check_runnable(6, &involves6));
    jobs.add_running(6, involves6);

    // 独占申请 p1 与共享占用冲突，进入 pending；其涉及的 g2 也被标记为 pending。
    let pending = vec![
        InvolvingSchemaInfo::policy("p1"),
        InvolvingSchemaInfo::resource_group("g2"),
    ];
    assert!(!jobs.check_runnable(0, &pending));
    jobs.add_pending(pending.clone());
    // g2 上已有 pending，涉及 g2 的第二个作业也要排队。
    let second_pending = vec![
        InvolvingSchemaInfo::resource_group("g2"),
        InvolvingSchemaInfo::resource_group("g3"),
    ];
    assert!(!jobs.check_runnable(0, &second_pending));
    jobs.add_pending(second_pending.clone());

    // 作业 6 完成后开启新一轮，但作业 5 仍共享 p1，pending 作业依旧被阻塞。
    jobs.remove_running(6);
    jobs.reset_all_pending();
    assert!(!jobs.check_runnable(0, &pending));
    jobs.add_pending(pending.clone());
    assert!(!jobs.check_runnable(0, &second_pending));
    jobs.add_pending(second_pending);
    // 作业 5 完成后，本轮内新作业仍受 pending 记录阻塞，保证先到者优先；
    // 再次 reset 开启新一轮后，最早的 pending 作业终于可以运行。
    jobs.remove_running(5);
    assert!(!jobs.check_runnable(0, &[InvolvingSchemaInfo::policy("p1")]));
    jobs.reset_all_pending();
    assert!(jobs.check_runnable(0, &pending));
    assert_invariants(&jobs);
}

/// 验证 `finish_or_pend_job` 的原子性：作业结束时一次性完成
/// "从运行表移除 + 将其涉及对象转入 pending" 两个动作，
/// 避免中间状态让其他作业趁虚而入。
#[test]
fn finish_or_pend_updates_counts_atomically() {
    let mut jobs = RunningJobs::default();
    let involves = vec![InvolvingSchemaInfo::schema("db", "table")];
    jobs.add_running(7, involves.clone());
    // 作业 7 结束并把 db.table 标记为 pending：运行表清空，但同对象新作业仍被阻塞。
    jobs.finish_or_pend_job(7, involves.clone(), true);
    assert_eq!("", jobs.all_ids());
    assert!(!jobs.check_runnable(8, &involves));
    jobs.reset_all_pending();
    assert!(jobs.check_runnable(8, &involves));
    assert_invariants(&jobs);
}
