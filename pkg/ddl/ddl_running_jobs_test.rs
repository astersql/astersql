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
