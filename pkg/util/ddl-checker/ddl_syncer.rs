// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 上游 DDL 同步器：从源库拉取建表语句并应用到本地 ExecutableChecker。
//
// 用于在检查目标 SQL 可执行性前，把依赖表结构对齐到上游快照。
// DDL（Data Definition Language）指 CREATE/ALTER/DROP 等结构变更语句。

use crate::executable_checker::{CheckerError, CheckerResult, ExecutableChecker, ExecutionContext};

/// 连接上游数据库所需的主机、账号、库名与快照配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DBConfig {
    /// 上游主机地址。
    pub host: String,
    /// 登录用户名。
    pub user: String,
    /// 登录密码。
    pub password: String,
    /// 目标 schema（数据库）名。
    pub schema: String,
    /// 可选快照标识（如一致性读位点）。
    pub snapshot: String,
    /// 上游端口。
    pub port: u16,
}

/// 上游数据库抽象：按 schema/表名取 CREATE TABLE，并关闭连接。
pub trait UpstreamDatabase: Send {
    /// 返回指定表的建表 SQL 文本。
    fn get_create_table_sql(
        &mut self,
        context: &ExecutionContext,
        schema_name: &str,
        table_name: &str,
    ) -> CheckerResult<String>;
    /// 关闭上游连接并释放资源。
    fn close(&mut self) -> CheckerResult<()>;
}

/// Opens and pings the configured upstream database before returning it.
/// 按配置打开并 ping 上游库，返回可用的 `UpstreamDatabase` 实例。
pub trait UpstreamDatabaseFactory {
    fn open_database(&self, config: &DBConfig) -> CheckerResult<Box<dyn UpstreamDatabase>>;
}

// DDLSyncer can sync the table structure from upstream to ExecutableChecker.
/// 持有上游连接与本地检查器，负责把上游表结构同步进检查会话。
pub struct DDLSyncer<'checker> {
    /// 上游数据库句柄。
    db: Box<dyn UpstreamDatabase>,
    /// 本地可执行性检查器（可变借用，生命周期与 syncer 绑定）。
    ec: &'checker mut ExecutableChecker,
}

// NewDDLSyncer creates a new DDLSyncer.
/// 经 factory 打开上游库并构造 `DDLSyncer`。
pub fn NewDDLSyncer<'checker>(
    cfg: &DBConfig,
    executableChecker: &'checker mut ExecutableChecker,
    factory: &dyn UpstreamDatabaseFactory,
) -> CheckerResult<DDLSyncer<'checker>> {
    let db = factory.open_database(cfg)?;
    Ok(DDLSyncer {
        db,
        ec: executableChecker,
    })
}

impl<'checker> DDLSyncer<'checker> {
    /// 由已有上游句柄与检查器直接组装，跳过 factory（测试/注入用）。
    pub fn from_parts(
        db: Box<dyn UpstreamDatabase>,
        executable_checker: &'checker mut ExecutableChecker,
    ) -> Self {
        Self {
            db,
            ec: executable_checker,
        }
    }

    // SyncTable gets upstream DDL with a background context, then replaces the local table.
    /// 用后台 context 取上游建表 SQL，先 Drop 本地表再 Execute 重建。
    pub fn SyncTable(
        &mut self,
        tidbContext: &ExecutionContext,
        schemaName: &str,
        tableName: &str,
    ) -> CheckerResult<()> {
        // 拉取与应用分两步：先背景上下文取 DDL，再在调用方上下文落地。
        let create_table_sql =
            self.db
                .get_create_table_sql(&ExecutionContext::background(), schemaName, tableName)?;
        self.ec.DropTable(tidbContext, tableName)?;
        self.ec.Execute(tidbContext, &create_table_sql)
    }

    // Close attempts both resources and gives the checker error precedence, matching Go.
    /// 关闭检查器与上游库；若两者都错，优先返回检查器错误（对齐 Go）。
    pub fn Close(&mut self) -> CheckerResult<()> {
        let checker_error = self.ec.Close().err();
        let database_error = self.db.close().err();
        match (checker_error, database_error) {
            (Some(error), _) => Err(error),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(()),
        }
    }

    /// 只读访问内嵌的 ExecutableChecker。
    pub fn checker(&self) -> &ExecutableChecker {
        &self.ec
    }

    /// 可变访问内嵌的 ExecutableChecker。
    pub fn checker_mut(&mut self) -> &mut ExecutableChecker {
        &mut self.ec
    }
}

impl From<&str> for CheckerError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}
