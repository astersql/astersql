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

// 系统表 Manager 的单元测试。
//
// 用内存 Mock 会话池模拟 `mysql.tidb_ddl_job` / `mysql.tidb_mdl_info`，
// 验证 GetJobByID、GetMDLVer、GetMinJobID、HasFlashbackClusterJob 行为。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    ACTION_FLASHBACK_CLUSTER, Context, Error, Row, Session, SessionPool, Value, new_manager,
};

/// 内存中的系统表状态：DDL job 行与 MDL version 映射。
#[derive(Default)]
struct TableState {
    ddl_jobs: HashMap<i64, DdlJobRow>,
    mdl_info: HashMap<i64, i64>,
}

/// 单条 DDL job 的简化行：job_meta 字节与作业类型。
#[derive(Clone)]
struct DdlJobRow {
    job_meta: Vec<u8>,
    job_type: i64,
}

/// 按 SQL 前缀分发到内存表状态的 Mock 会话。
struct MockSession {
    state: Arc<Mutex<TableState>>,
}

impl Session for MockSession {
    fn execute(&mut self, _context: &Context, sql: &str, _label: &str) -> Result<Vec<Row>, Error> {
        let mut state = self.state.lock().expect("table state poisoned");
        let sql_lower = sql.to_ascii_lowercase();
        // 按 job_id 返回 job_meta；不存在则空结果（上层映射为 NotFound）。
        if sql_lower.starts_with("select job_meta from mysql.tidb_ddl_job") {
            let job_id = parse_trailing_i64(sql);
            return match state.ddl_jobs.get(&job_id) {
                Some(row) => Ok(vec![Row(vec![Value::Bytes(row.job_meta.clone())])]),
                None => Ok(Vec::new()),
            };
        }
        if sql_lower.starts_with("select version from mysql.tidb_mdl_info") {
            let job_id = parse_trailing_i64(sql);
            return match state.mdl_info.get(&job_id) {
                Some(ver) => Ok(vec![Row(vec![Value::Int(*ver)])]),
                None => Ok(Vec::new()),
            };
        }
        // 计算 >= previous 的最小 job_id；无匹配返回空行。
        if sql_lower.starts_with("select min(job_id) from mysql.tidb_ddl_job") {
            let prev = parse_trailing_i64(sql);
            let min_id = state
                .ddl_jobs
                .keys()
                .copied()
                .filter(|id| *id >= prev)
                .min()
                .unwrap_or(0);
            if min_id == 0 {
                return Ok(Vec::new());
            }
            return Ok(vec![Row(vec![Value::Int(min_id)])]);
        }
        if sql_lower.starts_with("select count(1) from mysql.tidb_ddl_job") {
            let min_job_id = extract_after(sql, "job_id >=").unwrap_or(0);
            let count = state
                .ddl_jobs
                .iter()
                .filter(|(id, row)| {
                    **id >= min_job_id && row.job_type == i64::from(ACTION_FLASHBACK_CLUSTER)
                })
                .count() as i64;
            return Ok(vec![Row(vec![Value::Int(count)])]);
        }
        if sql_lower.starts_with("insert into mysql.tidb_ddl_job")
            || sql_lower.starts_with("replace into mysql.tidb_ddl_job")
        {
            // values(job_id, reorg, schema_ids, table_ids, job_meta, type, processing)
            let (job_id, job_meta, job_type) = parse_ddl_job_insert(sql);
            state
                .ddl_jobs
                .insert(job_id, DdlJobRow { job_meta, job_type });
            return Ok(Vec::new());
        }
        if sql_lower.starts_with("replace into mysql.tidb_mdl_info")
            || sql_lower.starts_with("insert into mysql.tidb_mdl_info")
        {
            let (job_id, version) = parse_mdl_insert(sql);
            state.mdl_info.insert(job_id, version);
            return Ok(Vec::new());
        }
        if sql_lower.starts_with("delete from mysql.tidb_ddl_job") {
            state.ddl_jobs.clear();
            return Ok(Vec::new());
        }
        Err(Error::Execute(format!("unsupported sql: {sql}")))
    }
}

/// 共享同一 TableState 的 Mock 会话池。
struct MockPool {
    state: Arc<Mutex<TableState>>,
}

impl SessionPool for MockPool {
    fn get(&self) -> Result<Box<dyn Session>, Error> {
        Ok(Box::new(MockSession {
            state: Arc::clone(&self.state),
        }))
    }

    fn put(&self, _session: Box<dyn Session>) {}
}

/// 从 SQL 末尾倒序截取连续数字作为 i64（用于 `where job_id = N`）。
fn parse_trailing_i64(sql: &str) -> i64 {
    let digits: String = sql
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().unwrap_or(0)
}

/// 在 marker 之后截取首段数字。
fn extract_after(sql: &str, marker: &str) -> Option<i64> {
    let lower = sql.to_ascii_lowercase();
    let pos = lower.find(&marker.to_ascii_lowercase())?;
    let after = sql[pos + marker.len()..].trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// 解析 insert/replace DDL job 的 values 列表，取出 job_id、job_meta、type。
fn parse_ddl_job_insert(sql: &str) -> (i64, Vec<u8>, i64) {
    // values(9999, 0, '1', '1', '{"id":9999}', 1, 0)
    let start = sql
        .to_ascii_lowercase()
        .find("values")
        .map(|i| i + "values".len())
        .unwrap_or(0);
    let body = sql[start..].trim_start().trim_start_matches('(');
    let parts = split_sql_values(body);
    let job_id: i64 = parts[0].parse().unwrap_or(0);
    let job_meta = parts[4].trim_matches('\'').as_bytes().to_vec();
    let job_type: i64 = parts[5].parse().unwrap_or(0);
    (job_id, job_meta, job_type)
}

/// 解析 MDL info 插入语句，取出 job_id 与 version。
fn parse_mdl_insert(sql: &str) -> (i64, i64) {
    // values(9999, 123, '1')
    let start = sql
        .to_ascii_lowercase()
        .find("values")
        .map(|i| i + "values".len())
        .unwrap_or(0);
    let body = sql[start..].trim_start().trim_start_matches('(');
    let parts = split_sql_values(body);
    let job_id: i64 = parts[0].parse().unwrap_or(0);
    let version: i64 = parts[1].parse().unwrap_or(0);
    (job_id, version)
}

/// 按逗号拆分 VALUES 列表，忽略引号内的逗号。
fn split_sql_values(body: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in body.chars() {
        match ch {
            '\'' if !in_quote => in_quote = true,
            '\'' if in_quote => in_quote = false,
            ',' if !in_quote => {
                parts.push(current.trim().to_string());
                current.clear();
                continue;
            }
            ')' if !in_quote => {
                parts.push(current.trim().to_string());
                break;
            }
            _ => current.push(ch),
        }
        if in_quote && ch != '\'' {
            // keep quoted content including digits
        }
    }
    if !current.trim().is_empty() && parts.is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

/// 通过池借出会话执行一条 setup SQL 并归还。
fn exec(pool: &MockPool, sql: &str) {
    let mut session = pool.get().unwrap();
    session.execute(&Context::default(), sql, "setup").unwrap();
    pool.put(session);
}

/// 对应 Go 的 TestManager：覆盖 job / MDL / min job id / flashback 四类查询。
// test_manager 对应 Go 的 TestManager。
#[test]
fn test_manager() {
    let pool = Arc::new(MockPool {
        state: Arc::new(Mutex::new(TableState::default())),
    });
    let mgr = new_manager(pool.clone() as Arc<dyn SessionPool>);
    let ctx = Context {
        request_id: "test".into(),
    };

    // GetJobByID
    {
        let err = mgr
            .get_job_by_id(&ctx, 9999)
            .err()
            .expect("missing job must return an error");
        assert_eq!(Error::NotFound, err);
        exec(
            &pool,
            r#"insert into mysql.tidb_ddl_job(job_id, reorg, schema_ids, table_ids, job_meta, type, processing)
							values(9999, 0, '1', '1', '{"id":9999,"type":62,"schema_id":7,"table_id":8,"query":"flashback cluster"}', 1, 0)"#,
        );
        let job = mgr.get_job_by_id(&ctx, 9999).unwrap();
        assert_eq!(9999, job.job.id);
        assert_eq!(ACTION_FLASHBACK_CLUSTER, job.job.tp);
        assert_eq!(7, job.job.schema_id);
        assert_eq!(8, job.job.table_id);
        assert_eq!("flashback cluster", job.job.query);
        assert_eq!(
            br#"{"id":9999,"type":62,"schema_id":7,"table_id":8,"query":"flashback cluster"}"#
                .as_slice(),
            job.bytes
        );
    }

    // GetMDLVer
    {
        let err = mgr.get_mdl_version(&ctx, 9999).unwrap_err();
        assert_eq!(Error::NotFound, err);
        exec(
            &pool,
            r#"replace into mysql.tidb_mdl_info (job_id, version, table_ids)
							values(9999, 123, '1')"#,
        );
        let ver = mgr.get_mdl_version(&ctx, 9999).unwrap();
        assert_eq!(123, ver);
    }

    // GetMinJobID
    {
        exec(&pool, "delete from mysql.tidb_ddl_job");
        let id = mgr.get_min_job_id(&ctx, 0).unwrap();
        assert_eq!(0, id);

        exec(
            &pool,
            r#"insert into mysql.tidb_ddl_job(job_id, reorg, schema_ids, table_ids, job_meta, type, processing)
						values(123456, 0, '1', '1', '{"id":9998}', 1, 0)"#,
        );
        let id = mgr.get_min_job_id(&ctx, 0).unwrap();
        assert_eq!(123456, id);
        let id = mgr.get_min_job_id(&ctx, 123456).unwrap();
        assert_eq!(123456, id);
        let id = mgr.get_min_job_id(&ctx, 123457).unwrap();
        assert_eq!(0, id);
    }

    // HasFlashbackClusterJob
    {
        exec(&pool, "delete from mysql.tidb_ddl_job");
        let found = mgr.has_flashback_cluster_job(&ctx, 0).unwrap();
        assert!(!found);

        exec(
            &pool,
            &format!(
                r#"insert into mysql.tidb_ddl_job(job_id, reorg, schema_ids, table_ids, job_meta, type, processing)
						values(123, 0, '1', '1', '{{"id":9998}}', {}, 0)"#,
                ACTION_FLASHBACK_CLUSTER
            ),
        );
        let found = mgr.has_flashback_cluster_job(&ctx, 0).unwrap();
        assert!(found);
        let found = mgr.has_flashback_cluster_job(&ctx, 123).unwrap();
        assert!(found);
        let found = mgr.has_flashback_cluster_job(&ctx, 124).unwrap();
        assert!(!found);
    }
}

/// `ActionType` is persisted in `mysql.tidb_ddl_job.type`; keep the Rust value
/// pinned to Go `model.ActionFlashbackCluster` instead of testing with the same
/// production constant on both sides of the mock query.
#[test]
fn flashback_cluster_action_value_matches_go_persisted_protocol() {
    assert_eq!(62, ACTION_FLASHBACK_CLUSTER);
}

/// Go `model.Job.Decode` delegates to `encoding/json` and rejects malformed
/// persisted metadata instead of extracting an `id` from an invalid prefix.
#[test]
fn job_decode_rejects_malformed_json_like_go() {
    assert!(crate::Job::decode(br#"{"id":42"#).is_err());
}
