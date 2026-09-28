// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL executor 的无 testkit 单元测试（nokit test）。
//
// 本文件针对 DDL（Data Definition Language，数据定义语言，如 CREATE/ALTER/DROP）
// 执行器中的纯逻辑函数进行测试，不依赖完整的测试集群或 testkit 框架：
// - `build_query_string_from_jobs`：把多个 DDL Job 的 SQL 语句拼接成一条查询串；
// - `merge_create_table_jobs`：把同一 schema 下可合并的建表 Job 批量合并，
//   以减少 DDL Job 数量、提升批量建表吞吐；
// - `is_undroppable_table` / `is_undroppable_table_with_id`：判断某张表是否禁止删除
//   （如系统表、保留全局 ID 范围内的表）。
//

use crate::ddl::{Job, JobState};
use crate::executor::{is_undroppable_table, is_undroppable_table_with_id};
use crate::job_submitter::{JobSpec, build_query_string_from_jobs, merge_create_table_jobs};

/// 构造一个用于测试的 `JobSpec`（DDL Job 提交前的封装描述）。
///
/// 参数说明：
/// - `id`：Job ID，同时复用为 table_id；
/// - `schema_id`：所属数据库（schema）的 ID，用于判断能否同库合并；
/// - `query`：该 Job 对应的原始 SQL 文本；
/// - `id_allocated`：表 ID 是否已预分配（预分配 ID 的 Job 不参与合并）；
/// - `has_foreign_keys`：是否带外键（带外键的建表 Job 不参与合并）。
fn spec(
    id: i64,
    schema_id: i64,
    query: &str,
    id_allocated: bool,
    has_foreign_keys: bool,
) -> JobSpec {
    JobSpec {
        // 其余元数据固定为稳定默认值，让测试只关注 SQL 拼接、
        // schema 归组以及"是否允许合并"这些判定条件。
        job: Job {
            id,
            query: query.into(),
            state: JobState::None,
            version: 2,
            start_ts: 0,
            real_start_ts: 0,
            action_type: crate::ddl::ActionType::Other,
            table_id: id,
            schema_id,
            paused_by: None,
        },
        id_allocated,
        has_foreign_keys,
        // 输入样本初始都是未合并状态；执行 `merge_create_table_jobs`
        // 后，合并结果会把被吸收的原始 Job 填入这里。
        merged_jobs: Vec::new(),
    }
}

/// 验证 SQL 拼接逻辑与 Go 版本的格式化行为一致：
/// 每条语句去掉首尾空白、确保以分号结尾，多条语句用单个空格连接。
#[test]
fn build_query_string_from_jobs_matches_go_formatting() {
    // 空 Job 列表应产生空字符串。
    assert_eq!("", build_query_string_from_jobs(&[]));
    assert_eq!(
        "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255));",
        build_query_string_from_jobs(&[spec(
            1,
            1,
            "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255));",
            false,
            false,
        )])
    );
    assert_eq!(
        "CREATE TABLE users (id INT); CREATE TABLE products (id INT);",
        build_query_string_from_jobs(&[
            spec(1, 1, "CREATE TABLE users (id INT);", false, false),
            spec(2, 1, "CREATE TABLE products (id INT);", false, false),
        ])
    );
    // 混合场景：有无分号、带前后空白的语句都应被规范化后拼接。
    assert_eq!(
        "CREATE TABLE users (id INT); CREATE TABLE products (id INT); CREATE TABLE orders (id INT);",
        build_query_string_from_jobs(&[
            spec(1, 1, "CREATE TABLE users (id INT)", false, false),
            spec(2, 1, "CREATE TABLE products (id INT);", false, false),
            spec(3, 1, "   CREATE TABLE orders (id INT) ", false, false),
        ])
    );
}

/// 验证建表 Job 合并的资格规则与 Go 版本一致：
/// 非建表 Job、预分配 ID 的 Job、带外键的 Job、不同 schema 的 Job 均不合并。
#[test]
fn merge_create_table_jobs_respects_go_eligibility_rules() {
    // 空列表与单个 Job 都原样返回，不做合并。
    assert!(merge_create_table_jobs(Vec::new()).is_empty());
    let one = spec(1, 1, "create table db.t1 (id int)", false, false);
    assert_eq!(vec![one.clone()], merge_create_table_jobs(vec![one]));

    // 非建表类 Job（如加列）不参与合并，应独立保留。
    let mut non_create = spec(2, 1, "alter table db.t add column c int", false, false);
    non_create.job.state = JobState::Running;
    let merged = merge_create_table_jobs(vec![
        spec(1, 1, "create table db.t1 (id int)", false, false),
        non_create.clone(),
        spec(3, 1, "create table db.t2 (id int)", false, false),
    ]);
    // 两个建表 Job 被合并为一个（含 2 个子 Job），非建表 Job 单独保留。
    assert_eq!(2, merged.len());
    assert_eq!(non_create, merged[1]);
    assert_eq!(2, merged[0].merged_jobs.len());

    // 表 ID 已预分配的 Job 不参与合并。
    let allocated = vec![
        spec(1, 1, "create table db.t1 (id int)", true, false),
        spec(2, 1, "create table db.t2 (id int)", false, false),
    ];
    assert_eq!(allocated, merge_create_table_jobs(allocated.clone()));

    // 带外键（Foreign Key，引用其他表的约束）的建表 Job 不参与合并。
    let foreign_key = vec![
        spec(1, 1, "create table db.t1 (id int)", false, true),
        spec(2, 1, "create table db.t2 (id int)", false, false),
    ];
    assert_eq!(foreign_key, merge_create_table_jobs(foreign_key.clone()));

    // 不同 schema（数据库）下的建表 Job 互不合并。
    let different_schemas = vec![
        spec(1, 1, "create table db1.t1 (id int)", false, false),
        spec(2, 2, "create table db2.t2 (id int)", false, false),
    ];
    assert_eq!(
        different_schemas,
        merge_create_table_jobs(different_schemas.clone())
    );
}

/// 对应 Go `TestMergeCreateTableJobsOfSameSchema`：除合并数量外，还必须保留
/// 原始 Job 顺序并按 Go 的分号规则生成宿主 Job 查询串。
#[test]
fn merge_create_table_jobs_of_same_schema_matches_go_query() {
    let merged = merge_create_table_jobs(vec![
        spec(1, 1, "create table db1.t1 (c1 int, c2 int)", false, false),
        spec(2, 1, "create table db1.t2 (c1 int, c2 int);", false, false),
    ]);

    assert_eq!(1, merged.len());
    assert_eq!(2, merged[0].merged_jobs.len());
    assert_eq!(
        "create table db1.t1 (c1 int, c2 int); create table db1.t2 (c1 int, c2 int);",
        merged[0].job.query
    );
}

/// 验证合并时的批次均衡策略：单批最多 8 个 Job，且各批尽量均匀。
/// 22 个同库建表 Job 应被分成 8/7/7 三个批次。
#[test]
fn merge_create_table_jobs_balances_batches_of_at_most_eight() {
    // 与 Go 用例一致，同时覆盖 9/7/22 三种分组以及三个不可合并 Job。
    let mut jobs = vec![
        spec(0, 0, "alter table db0.t add column c int", false, false),
        spec(1, 1, "create table db1.t1 (id int)", true, false),
        spec(2, 2, "create table db2.t1 (id int)", false, true),
    ];
    jobs[0].job.state = JobState::Running;
    let mut next_id = 3;
    for (schema_id, count) in [(3, 9), (4, 7), (5, 22)] {
        jobs.extend((0..count).map(|table_index| {
            let job = spec(
                next_id + table_index,
                schema_id,
                &format!("create table db{schema_id}.t{table_index} (id int)"),
                false,
                false,
            );
            job
        }));
        next_id += count;
    }
    let merged = merge_create_table_jobs(jobs);
    assert_eq!(9, merged.len());

    let mut schema_batches = std::collections::BTreeMap::<i64, Vec<usize>>::new();
    for job in &merged {
        if !job.merged_jobs.is_empty() {
            schema_batches
                .entry(job.job.schema_id)
                .or_default()
                .push(job.merged_jobs.len());
        }
    }
    for batches in schema_batches.values_mut() {
        batches.sort_unstable();
    }
    assert_eq!(
        std::collections::BTreeMap::from([(3, vec![4, 5]), (4, vec![7]), (5, vec![7, 7, 8]),]),
        schema_batches
    );
    assert!(merged.iter().all(|job| job.merged_jobs.len() <= 8));
}

/// 验证"禁止删除表"的判定规则与 Go 版本一致：
/// 保留全局 ID 区间内的表、workload_schema 下的表以及 mysql 库中的
/// 关键系统表（tidb、GC 删除区间表）都不允许被 DROP。
#[test]
fn undroppable_table_rules_match_go() {
    // 保留的全局 ID 区间上下界：落在 (LOWER, UPPER] 内的表 ID 属于系统保留。
    const UPPER: i64 = 0x0000_FFFF_FFFF_FFFF;
    const LOWER: i64 = UPPER - 1000;

    // 表 ID 落在保留区间内的表不可删除；普通 ID 的表可删除。
    assert!(is_undroppable_table_with_id("test", "table", UPPER));
    assert!(is_undroppable_table_with_id("test", "table", LOWER + 1));
    assert!(!is_undroppable_table_with_id("test", "table", 100));
    // workload_schema 库整体禁删；mysql 库中 tidb 与 GC（垃圾回收）
    // 相关的 gc_delete_range 系列表禁删，其余表（如 user）可删除。
    assert!(is_undroppable_table("workload_schema", "any_table"));
    assert!(is_undroppable_table("mysql", "tidb"));
    assert!(is_undroppable_table("mysql", "gc_delete_range"));
    assert!(is_undroppable_table("mysql", "gc_delete_range_done"));
    assert!(!is_undroppable_table("mysql", "user"));
    assert!(!is_undroppable_table("test", "test_table"));
    assert!(!is_undroppable_table("information_schema", "tables"));
}
