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

// DDL 系统表 Manager：通过会话池访问 `mysql.tidb_ddl_job` / `mysql.tidb_mdl_info`。
//
// 提供按 job_id 读取作业元数据（job_meta）、MDL（Metadata Lock，元数据锁）版本、
// 当前最小 job_id，以及是否存在 FLASHBACK CLUSTER 作业的查询接口。

use std::fmt;
use std::sync::Arc;

pub use meta_model::group_3::{ACTION_FLASHBACK_CLUSTER, Job, JobW as JobWrapper};

/// 执行 SQL 时携带的请求上下文（如 request_id）。
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub request_id: String,
}

/// 系统表访问过程中的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 目标行不存在。
    NotFound,
    /// 会话池借出失败。
    Pool(String),
    /// SQL 执行失败。
    Execute(String),
    /// 列类型或 job_meta 解码失败。
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("not found"),
            Self::Pool(message) | Self::Execute(message) | Self::Decode(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for Error {}

/// 系统表查询结果中的单元格值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Bytes(Vec<u8>),
}

/// 一行查询结果，按列下标取值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Row(pub Vec<Value>);

impl Row {
    /// 读取指定列为字节；Null / 缺失视为空切片，类型不符则报 Decode。
    pub fn bytes(&self, index: usize) -> Result<Vec<u8>, Error> {
        match self.0.get(index) {
            Some(Value::Bytes(value)) => Ok(value.clone()),
            Some(Value::Null) | None => Ok(Vec::new()),
            Some(_) => Err(Error::Decode(format!("column {index} is not bytes"))),
        }
    }

    /// 读取指定列为 i64；Null / 缺失视为 0，类型不符则报 Decode。
    pub fn int64(&self, index: usize) -> Result<i64, Error> {
        match self.0.get(index) {
            Some(Value::Int(value)) => Ok(*value),
            Some(Value::Null) | None => Ok(0),
            Some(_) => Err(Error::Decode(format!("column {index} is not int64"))),
        }
    }
}

/// 可执行内部 SQL 的会话抽象。
pub trait Session: Send {
    fn execute(&mut self, context: &Context, sql: &str, label: &str) -> Result<Vec<Row>, Error>;
}

/// 会话池：借出与归还会话，对应 Go 的 ResourcePool Put/Get。
pub trait SessionPool: Send + Sync {
    fn get(&self) -> Result<Box<dyn Session>, Error>;
    fn put(&self, session: Box<dyn Session>);
}

/// 系统表 Manager 接口：查询 job、MDL 版本、最小 job_id、flashback 存在性。
pub trait Manager: Send + Sync {
    fn get_job_by_id(&self, context: &Context, job_id: i64) -> Result<JobWrapper, Error>;
    fn get_job_bytes_by_id_with_session(
        &self,
        context: &Context,
        session: &mut dyn Session,
        job_id: i64,
    ) -> Result<Vec<u8>, Error>;
    fn get_mdl_version(&self, context: &Context, job_id: i64) -> Result<i64, Error>;
    fn get_min_job_id(&self, context: &Context, previous_min_job_id: i64) -> Result<i64, Error>;
    fn has_flashback_cluster_job(&self, context: &Context, min_job_id: i64) -> Result<bool, Error>;
}

/// 基于会话池的系统表 Manager 实现。
pub struct SystemTableManager {
    session_pool: Arc<dyn SessionPool>,
}

/// 由会话池构造 Manager 实例。
pub fn new_manager(pool: Arc<dyn SessionPool>) -> Arc<dyn Manager> {
    Arc::new(SystemTableManager { session_pool: pool })
}

impl SystemTableManager {
    /// The session is returned to the pool on every closure result. Keeping the
    /// return outside the closure is the Rust equivalent of Go's `defer Put`.
    /// 借用会话执行闭包，结束后无论成败都将会话归还池中（对应 Go `defer Put`）。
    fn with_new_session<T>(
        &self,
        function: impl FnOnce(&mut dyn Session) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut session = self.session_pool.get()?;
        let result = function(session.as_mut());
        self.session_pool.put(session);
        result
    }
}

impl Manager for SystemTableManager {
    fn get_job_by_id(&self, context: &Context, job_id: i64) -> Result<JobWrapper, Error> {
        self.with_new_session(|session| {
            let bytes = self.get_job_bytes_by_id_with_session(context, session, job_id)?;
            let job = Job::decode(&bytes).map_err(|error| Error::Decode(error.to_string()))?;
            Ok(meta_model::group_3::new_job_w(job, bytes))
        })
    }

    fn get_job_bytes_by_id_with_session(
        &self,
        context: &Context,
        session: &mut dyn Session,
        job_id: i64,
    ) -> Result<Vec<u8>, Error> {
        let sql = format!("select job_meta from mysql.tidb_ddl_job where job_id = {job_id}");
        let rows = session.execute(context, &sql, "get-job-by-id")?;
        let row = rows.first().ok_or(Error::NotFound)?;
        row.bytes(0)
    }

    fn get_mdl_version(&self, context: &Context, job_id: i64) -> Result<i64, Error> {
        self.with_new_session(|session| {
            let sql = format!("select version from mysql.tidb_mdl_info where job_id = {job_id}");
            let rows = session.execute(context, &sql, "check-mdl-info")?;
            rows.first().ok_or(Error::NotFound)?.int64(0)
        })
    }

    fn get_min_job_id(&self, context: &Context, previous_min_job_id: i64) -> Result<i64, Error> {
        self.with_new_session(|session| {
            // 在 previous_min_job_id 之上求当前仍存在的最小 job_id。
            let sql = format!(
                "select min(job_id) from mysql.tidb_ddl_job where job_id >= {previous_min_job_id}"
            );
            let rows = session.execute(context, &sql, "get-min-job-id")?;
            match rows.first() {
                Some(row) => row.int64(0),
                None => Ok(0),
            }
        })
    }

    fn has_flashback_cluster_job(&self, context: &Context, min_job_id: i64) -> Result<bool, Error> {
        self.with_new_session(|session| {
            // 检查是否存在 id >= min_job_id 的 FLASHBACK CLUSTER 作业。
            let sql = format!(
                "select count(1) from mysql.tidb_ddl_job where job_id >= {min_job_id} and type = {ACTION_FLASHBACK_CLUSTER}"
            );
            let rows = session.execute(context, &sql, "has-flashback-cluster-job")?;
            match rows.first() {
                Some(row) => Ok(row.int64(0)? > 0),
                None => Ok(false),
            }
        })
    }
}
