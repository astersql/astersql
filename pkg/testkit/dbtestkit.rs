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

// 基于 [`DbDriver`] 的“必须成功”测试工具包（DBTestKit）。
//
// 提供 MustExec / MustQuery / MustPrepare 等失败即 panic 的便捷 API，
// 对应 Go 侧以 database/sql 驱动执行的测试路径。

use std::sync::Arc;

use crate::db_driver::{
    Database, DbDriver, DbValue, ExecutionResult, PreparedStatement, QueryRows,
};

/// 包装 [`DbDriver`] 的断言式测试 API。
#[derive(Clone)]
pub struct DBTestKit {
    driver: DbDriver,
}

impl DBTestKit {
    /// 用给定 [`Database`] 构造。
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self {
            driver: DbDriver::new(database),
        }
    }

    /// 预编译语句（prepare 本身不失败时直接返回句柄）。
    pub fn MustPrepare(&self, query: &str) -> PreparedStatement {
        self.driver.prepare(query)
    }

    /// 执行预编译语句，失败则 panic。
    pub fn MustExecPrepared(
        &self,
        statement: &PreparedStatement,
        args: Vec<DbValue>,
    ) -> ExecutionResult {
        statement
            .execute(&args)
            .unwrap_or_else(|error| panic!("execute prepared {}: {error}", statement.sql()))
    }

    /// 查询预编译语句，失败则 panic。
    pub fn MustQueryPrepared(
        &self,
        statement: &PreparedStatement,
        args: Vec<DbValue>,
    ) -> QueryRows {
        statement
            .query(&args)
            .unwrap_or_else(|error| panic!("query prepared {}: {error}", statement.sql()))
    }

    /// 执行 SQL，失败则 panic。
    pub fn MustExec(&self, sql: &str, args: Vec<DbValue>) -> ExecutionResult {
        self.driver
            .execute(sql, &args)
            .unwrap_or_else(|error| panic!("sql={sql:?}, args={args:?}: {error}"))
    }

    /// 查询 SQL，失败则 panic。
    pub fn MustQuery(&self, sql: &str, args: Vec<DbValue>) -> QueryRows {
        self.driver
            .query(sql, &args)
            .unwrap_or_else(|error| panic!("sql={sql:?}, args={args:?}: {error}"))
    }

    /// 查询且断言至少返回一行。
    pub fn MustQueryRows(&self, sql: &str, args: Vec<DbValue>) {
        let rows = self.MustQuery(sql, args);
        assert!(!rows.rows.is_empty(), "query {sql:?} returned no rows");
    }

    /// 取出内部 [`DbDriver`] 引用。
    pub fn GetDB(&self) -> &DbDriver {
        &self.driver
    }
}
