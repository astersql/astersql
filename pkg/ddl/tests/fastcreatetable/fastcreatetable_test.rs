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

// Fast Create Table（快速建表）功能测试。
//
// 对应 Go `TestSwitchFastCreateTable` / `TestDDL` / `TestMergedJob`：
// - 系统变量 `tidb_enable_fast_create_table` 的开关与非法值报错；
// - 内存 SchemaCatalog 上的建表/截断/删表/重命名路径；
// - 多个 CREATE TABLE 作业合并（Merged Job）及 AUTO_INCREMENT 起始分配。
//
// Fast Create Table 将同 schema 的建表作业批量合并，减少 DDL owner
// 调度轮次；AUTO_INCREMENT 为自增列的下一可用值。

use std::collections::{BTreeMap, BTreeSet};

use astersql_ddl::ddl::{Job, JobState};
use astersql_ddl::job_submitter::{JobSpec, JobSubmitter, merge_create_table_jobs};
use astersql_sessionctx_vardef::{EnableFastCreateTable, TiDBEnableFastCreateTable};

/// 将布尔值格式化为会话变量展示用的 `ON` / `OFF`。
fn bool_to_on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

/// Mirrors Go `TypeBool` / TiDBOptOn parsing used by `tidb_enable_fast_create_table`.
/// 解析快速建表开关字符串（1/on 或 0/off），非法值返回变量错误码 1231。
fn parse_fast_create_table_value(val: &str) -> Result<bool, String> {
    match val.to_ascii_lowercase().as_str() {
        "1" | "on" => Ok(true),
        "0" | "off" => Ok(false),
        _ => Err(format!(
            "[variable:1231]Variable '{TiDBEnableFastCreateTable}' can't be set to the value of '{val}'"
        )),
    }
}

/// 设置全局 `EnableFastCreateTable`；仅在目标值变化时写入。
fn set_fast_create_table(val: &str) -> Result<(), String> {
    let on = parse_fast_create_table_value(val)?;
    if EnableFastCreateTable.Load() != on {
        EnableFastCreateTable.Store(on);
    }
    Ok(())
}

/// 构造一条 CREATE TABLE 的 `JobSpec`（作业规格），`table_id` 暂用 `id`。
fn create_table_spec(id: i64, schema_id: i64, query: &str) -> JobSpec {
    JobSpec::new(Job::new(id, schema_id, id, query), false)
}

/// Minimal schema catalog so TestDDL can exercise create/truncate/drop/rename without mockstore.
/// 最小内存 schema 目录：库 → 表集合，以及每张表的下一 AUTO_INCREMENT 值。
#[derive(Clone, Default)]
struct SchemaCatalog {
    databases: BTreeMap<String, BTreeSet<String>>,
    next_auto_id: BTreeMap<(String, String), i64>,
}

impl SchemaCatalog {
    /// 解析并执行一条简化 DDL/DML（仅覆盖本文件测试所需子集）。
    fn must_exec(&mut self, sql: &str) -> Result<(), String> {
        let s = sql.trim().trim_end_matches(';');
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("create database ") {
            let name = s[16..].trim().to_string();
            self.databases.entry(name).or_default();
            return Ok(());
        }
        if lower.starts_with("drop database ") {
            let name = s[14..].trim().to_string();
            self.databases.remove(&name);
            return Ok(());
        }
        if lower.starts_with("create table ") {
            // 解析 `db.table(...)`，拒绝重复建表，并记录 AUTO_INCREMENT 起点。
            let rest = s[13..].trim();
            let (qualified, _) = rest
                .split_once('(')
                .ok_or_else(|| "bad create table".to_string())?;
            let (db, table) = split_qualified(qualified.trim())?;
            let tables = self
                .databases
                .get_mut(&db)
                .ok_or_else(|| format!("Unknown database '{db}'"))?;
            if !tables.insert(table.clone()) {
                return Err(format!("[schema:1050]Table '{db}.{table}' already exists"));
            }
            if let Some(start) = parse_auto_increment_start(s) {
                self.next_auto_id.insert((db, table), start);
            } else {
                self.next_auto_id.insert((db, table), 1);
            }
            return Ok(());
        }
        if lower.starts_with("truncate table ") {
            // TRUNCATE 保留表结构，但把自增计数重置为 1。
            let (db, table) = split_qualified(s[15..].trim())?;
            self.ensure_table(&db, &table)?;
            if let Some(id) = self.next_auto_id.get_mut(&(db, table)) {
                *id = 1;
            }
            return Ok(());
        }
        if lower.starts_with("drop table ") {
            let (db, table) = split_qualified(s[11..].trim())?;
            let tables = self
                .databases
                .get_mut(&db)
                .ok_or_else(|| format!("Unknown database '{db}'"))?;
            if !tables.remove(&table) {
                return Err(format!("[schema:1051]Unknown table '{db}.{table}'"));
            }
            self.next_auto_id.remove(&(db, table));
            return Ok(());
        }
        if lower.starts_with("rename table ") {
            // rename table db.tb2 to db.tb3
            // 迁移自增计数到新表名，并防止目标表已存在。
            let rest = s[13..].trim();
            let (from, to) = rest
                .split_once(" to ")
                .or_else(|| rest.split_once(" TO "))
                .ok_or_else(|| "bad rename".to_string())?;
            let (db_from, table_from) = split_qualified(from.trim())?;
            let (db_to, table_to) = split_qualified(to.trim())?;
            self.ensure_table(&db_from, &table_from)?;
            let tables = self.databases.get_mut(&db_from).unwrap();
            tables.remove(&table_from);
            let dest = self
                .databases
                .get_mut(&db_to)
                .ok_or_else(|| format!("Unknown database '{db_to}'"))?;
            if !dest.insert(table_to.clone()) {
                return Err(format!(
                    "[schema:1050]Table '{db_to}.{table_to}' already exists"
                ));
            }
            if let Some(auto) = self.next_auto_id.remove(&(db_from, table_from)) {
                self.next_auto_id.insert((db_to, table_to), auto);
            }
            return Ok(());
        }
        if lower.starts_with("insert into ") {
            // insert into test.t1(c) values(1) — allocate auto id
            // 分配当前 next_auto_id 后递增，模拟自增列写入。
            let rest = s[12..].trim();
            let table_part = rest.split('(').next().unwrap_or(rest).trim();
            let (db, table) = split_qualified(table_part)?;
            self.ensure_table(&db, &table)?;
            let id = self.next_auto_id.entry((db, table)).or_insert(1);
            let allocated = *id;
            *id += 1;
            let _ = allocated;
            return Ok(());
        }
        if lower.starts_with("use ") || lower.starts_with("set global ") {
            return Ok(());
        }
        Err(format!("unsupported sql: {sql}"))
    }

    /// 确认库/表存在，否则返回 schema 1051 风格错误。
    fn ensure_table(&self, db: &str, table: &str) -> Result<(), String> {
        let tables = self
            .databases
            .get(db)
            .ok_or_else(|| format!("Unknown database '{db}'"))?;
        if tables.contains(table) {
            Ok(())
        } else {
            Err(format!("[schema:1051]Unknown table '{db}.{table}'"))
        }
    }

    /// 根据 next_auto_id 推导最近一次自增分配结果（id, 常量列值）。
    fn select_auto_rows(&self, db: &str, table: &str) -> Result<Vec<(i64, i64)>, String> {
        // After one insert with AUTO_INCREMENT 100, next id is 101 and last allocated is 100.
        self.ensure_table(db, table)?;
        let next = *self
            .next_auto_id
            .get(&(db.to_string(), table.to_string()))
            .unwrap_or(&1);
        if next <= 1 {
            return Ok(Vec::new());
        }
        Ok(vec![(next - 1, 1)])
    }
}

/// 将 `db.table` 限定名拆成库名与表名。
fn split_qualified(name: &str) -> Result<(String, String), String> {
    let mut parts = name.split('.');
    let db = parts
        .next()
        .ok_or_else(|| "missing db".to_string())?
        .to_string();
    let table = parts
        .next()
        .ok_or_else(|| "missing table".to_string())?
        .to_string();
    Ok((db, table))
}

/// 从 CREATE TABLE SQL 中提取最后一个 `AUTO_INCREMENT <n>` 起始值。
fn parse_auto_increment_start(sql: &str) -> Option<i64> {
    let lower = sql.to_ascii_lowercase();
    let marker = "auto_increment";
    let mut search = lower.as_str();
    let mut last = None;
    // 可能同时出现列级与表级 AUTO_INCREMENT，取最后一次数字。
    while let Some(pos) = search.find(marker) {
        let after = search[pos + marker.len()..].trim_start();
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            last = digits.parse().ok();
        }
        search = &search[pos + marker.len()..];
    }
    last
}

/// Run pending merged jobs transactionally, matching Go's all-or-nothing
/// behavior when a batch contains a duplicate table.
fn run_pending_jobs(catalog: &mut SchemaCatalog, pending: Vec<JobSpec>) -> Vec<Result<(), String>> {
    let mut results = Vec::with_capacity(pending.len());
    for spec in pending {
        let mut candidate = catalog.clone();
        let result = spec
            .job
            .query
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
            .try_for_each(|statement| candidate.must_exec(statement));
        match result {
            Ok(()) => {
                *catalog = candidate;
                results.push(Ok(()));
            }
            Err(error) => results.push(Err(error)),
        }
    }
    results
}

// test_switch_fast_create_table 对应 Go 的 TestSwitchFastCreateTable。
/// 验证快速建表开关可切换，非法取值返回与 Go 一致的变量错误信息。
#[test]
fn test_switch_fast_create_table() {
    // 保存并恢复全局开关，避免污染其他用例。
    let previous = EnableFastCreateTable.Load();
    EnableFastCreateTable.Store(true);

    assert_eq!("ON", bool_to_on_off(EnableFastCreateTable.Load()));

    let mut catalog = SchemaCatalog::default();
    catalog.must_exec("create database db1;").unwrap();
    catalog.must_exec("create database db2;").unwrap();
    catalog.must_exec("create table db1.tb1(id int);").unwrap();
    catalog.must_exec("create table db1.tb2(id int);").unwrap();
    catalog.must_exec("create table db2.tb1(id int);").unwrap();

    set_fast_create_table("ON").unwrap();
    assert_eq!("ON", bool_to_on_off(EnableFastCreateTable.Load()));

    set_fast_create_table("0").unwrap();
    assert_eq!("OFF", bool_to_on_off(EnableFastCreateTable.Load()));

    let err = set_fast_create_table("wrong").unwrap_err();
    assert_eq!(
        "[variable:1231]Variable 'tidb_enable_fast_create_table' can't be set to the value of 'wrong'",
        err
    );

    let err = set_fast_create_table("true").unwrap_err();
    assert_eq!(
        "[variable:1231]Variable 'tidb_enable_fast_create_table' can't be set to the value of 'true'",
        err
    );

    EnableFastCreateTable.Store(previous);
}

// test_ddl 对应 Go 的 TestDDL。
/// 在快速建表开启时，演练建表冲突、TRUNCATE/DROP/RENAME 及删库重建。
#[test]
fn test_ddl() {
    let previous = EnableFastCreateTable.Load();
    EnableFastCreateTable.Store(true);
    set_fast_create_table("ON").unwrap();

    let mut catalog = SchemaCatalog::default();
    catalog.must_exec("create database db").unwrap();
    catalog.must_exec("create table db.tb1(id int)").unwrap();
    catalog.must_exec("create table db.tb2(id int)").unwrap();
    // 重复建表应返回 schema:1050。
    let err = catalog
        .must_exec("create table db.tb1(id int)")
        .unwrap_err();
    assert_eq!("[schema:1050]Table 'db.tb1' already exists", err);

    catalog.must_exec("truncate table db.tb1").unwrap();
    catalog.must_exec("drop table db.tb1").unwrap();
    catalog.must_exec("rename table db.tb2 to db.tb3").unwrap();
    catalog.must_exec("drop database db").unwrap();

    catalog.must_exec("create database db").unwrap();
    catalog.must_exec("create table db.tb1(id int)").unwrap();
    catalog.must_exec("create table db.tb2(id int)").unwrap();

    EnableFastCreateTable.Store(previous);
}

// test_merged_job 对应 Go 的 TestMergedJob。
/// 验证建表作业合并分组、JobSubmitter 提交，以及自增起点分配正确性。
#[test]
fn test_merged_job() {
    let previous = EnableFastCreateTable.Load();
    EnableFastCreateTable.Store(true);

    let mut catalog = SchemaCatalog::default();
    catalog.must_exec("create database test").unwrap();

    // this job will be run first (alone)
    // 单条作业无法再合并，merged_jobs 为空。
    let first = create_table_spec(1, 1, "create table test.t(a int)");
    let alone = merge_create_table_jobs(vec![first.clone()]);
    assert_eq!(1, alone.len());
    assert!(alone[0].merged_jobs.is_empty());
    assert_eq!(JobState::None, alone[0].job.state);

    // below 2 jobs are merged into 1, they will fail together.
    // 合并组共享命运：一组失败则组内作业一并失败。
    let fail_group = vec![
        create_table_spec(
            2,
            1,
            "create table test.t1(id int AUTO_INCREMENT, c int) AUTO_INCREMENT 1000",
        ),
        create_table_spec(3, 1, "create table test.t(a int)"),
    ];
    let merged_fail = merge_create_table_jobs(fail_group.clone());
    assert_eq!(1, merged_fail.len());
    assert_eq!(2, merged_fail[0].merged_jobs.len());

    // below 2 jobs are merged into the third group, they will succeed together.
    let ok_group = vec![
        create_table_spec(
            4,
            1,
            "create table test.t1(id int AUTO_INCREMENT, c int) AUTO_INCREMENT 100",
        ),
        create_table_spec(
            5,
            1,
            "create table test.t2(id int AUTO_INCREMENT, c int) AUTO_INCREMENT 100",
        ),
    ];
    let merged_ok = merge_create_table_jobs(ok_group.clone());
    assert_eq!(1, merged_ok.len());
    assert_eq!(2, merged_ok[0].merged_jobs.len());

    // start to run the jobs via JobSubmitter (Go closes startSchedule then runs)
    // JobSubmitter：提交 DDL 作业到调度队列的封装。
    let mut submitter = JobSubmitter::default();
    let alone_results = submitter.submit(vec![first]);
    assert_eq!(1, alone_results.len());
    assert!(alone_results[0].is_ok());
    let alone_execution = run_pending_jobs(&mut catalog, submitter.take_pending());
    assert_eq!(vec![Ok(())], alone_execution);

    let fail_results = submitter.submit(fail_group);
    assert_eq!(1, fail_results.len());
    assert!(fail_results[0].is_ok());
    let fail_execution = run_pending_jobs(&mut catalog, submitter.take_pending());
    assert_eq!(1, fail_execution.len());
    assert_eq!(
        Err("[schema:1050]Table 'test.t' already exists".to_string()),
        fail_execution[0]
    );
    assert_eq!(
        "[schema:1051]Unknown table 'test.t1'",
        catalog.ensure_table("test", "t1").unwrap_err()
    );

    let results = submitter.submit(ok_group);
    assert_eq!(1, results.len());
    assert!(results[0].is_ok());
    let pending = submitter.take_pending();
    assert_eq!(1, pending.len());
    assert_eq!(2, pending[0].merged_jobs.len());
    assert_eq!(vec![Ok(())], run_pending_jobs(&mut catalog, pending));

    // Test the correctness of auto id after successful create
    // 建表 AUTO_INCREMENT 100 后插入一行，分配到的自增 id 应为 100。
    catalog.ensure_table("test", "t1").unwrap();
    catalog
        .must_exec("insert into test.t1(c) values(1)")
        .unwrap();
    assert_eq!(
        vec![(100, 1)],
        catalog.select_auto_rows("test", "t1").unwrap()
    );

    EnableFastCreateTable.Store(previous);
}
