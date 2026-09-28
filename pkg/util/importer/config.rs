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

// 数据导入工具的配置与错误类型。
//
// 对应 Go `pkg/util/importer` 的配置结构：数据库连接参数、建表/建索引 SQL、
// 并发 worker/job/batch 规模，以及导入过程中可能返回的错误枚举。

use std::fmt::{Display, Formatter};

/// 目标库连接参数（用户、口令、主机、端口、schema）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DbConfig {
    pub user: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    pub schema: String,
}

impl DbConfig {
    /// 拼装 MySQL 风格 DSN：`user:pass@tcp(host:port)/schema?charset=utf8`。
    pub fn dsn(&self) -> String {
        format!(
            "{}:{}@tcp({}:{})/{}?charset=utf8",
            self.user, self.password, self.host, self.port, self.schema
        )
    }
}

/// 导入任务总配置：DDL、日志级别、库连接与并发/批大小。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Config {
    /// 建表 SQL（CREATE TABLE）。
    pub table_sql: String,
    /// 建索引 SQL（CREATE [UNIQUE] INDEX），可为空。
    pub index_sql: String,
    pub log_level: String,
    pub db_config: DbConfig,
    /// 并发 worker 数；每个 worker 对应一个数据库连接。
    pub worker_count: usize,
    /// 待投递的 job 总数（每个 job 最终对应一行插入）。
    pub job_count: usize,
    /// 每个事务批量提交的行数。
    pub batch: usize,
}

impl Config {
    /// 对齐 Go `(*Config).String`，包括 nil receiver 与 `%+v` 的字段名输出。
    pub fn string(config: Option<&Self>) -> String {
        let Some(config) = config else {
            return "<nil>".to_owned();
        };
        format!(
            "Config({{TableSQL:{} IndexSQL:{} LogLevel:{} DBCfg:{{Host:{} User:{} Password:{} Schema:{} Snapshot: Port:{}}} WorkerCount:{} JobCount:{} Batch:{}}})",
            config.table_sql,
            config.index_sql,
            config.log_level,
            config.db_config.host,
            config.db_config.user,
            config.db_config.password,
            config.db_config.schema,
            config.db_config.port,
            config.worker_count,
            config.job_count,
            config.batch,
        )
    }
}

impl Display for Config {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::string(Some(self)))
    }
}

/// 导入工具错误：解析失败、不支持的列类型、非法区间、数据库错误、配置无效、通道关闭或 worker panic。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImporterError {
    Parse(String),
    UnsupportedColumn(String),
    InvalidRange(String),
    Database(String),
    InvalidConfig(String),
    ChannelClosed,
    WorkerPanic,
}

impl Display for ImporterError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "parse failed: {error}"),
            Self::UnsupportedColumn(column) => write!(f, "unsupported column type - {column}"),
            Self::InvalidRange(value) => write!(f, "invalid range: {value}"),
            Self::Database(error) => write!(f, "database error: {error}"),
            Self::InvalidConfig(error) => write!(f, "invalid config: {error}"),
            Self::ChannelClosed => write!(f, "job channel is closed"),
            Self::WorkerPanic => write!(f, "import worker panicked"),
        }
    }
}

impl std::error::Error for ImporterError {}
