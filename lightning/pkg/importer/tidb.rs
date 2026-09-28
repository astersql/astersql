// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! TiDB manager helpers matching Go `tidb.go`.

use crate::common;
use crate::common_ext;
use crate::config;
use crate::context::Context;
use crate::errors::{self, Result};
use crate::importdef;
use crate::log;
use crate::logutil;
use crate::metric;
use crate::model;
use crate::mydump;
use crate::mysql;
use crate::parser::Parser;
use crate::sql::DB;
use crate::tikv_util;
use crate::vardef;
use crate::zap;
use std::collections::HashMap;

// 语义说明：`TiDBManager` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub struct TiDBManager {
    pub db: DB,
    pub parser: Parser,
}

// 语义说明：`DBFromConfig` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn DBFromConfig(ctx: Context, dsn: &config::DBStore) -> Result<DB> {
    let mut param = common::MySQLConnectParam {
        Host: dsn.Host.clone(),
        Port: dsn.Port,
        User: dsn.User.clone(),
        Password: dsn.Psw.clone(),
        SQLMode: dsn.StrSQLMode.clone(),
        MaxAllowedPacket: dsn.MaxAllowedPacket,
        TLSConfig: dsn.Security.TLSConfig.clone(),
        AllowFallbackToPlaintext: dsn.Security.AllowFallbackToPlaintext,
        Net: dsn.UUID.clone(),
        Vars: HashMap::new(),
    };

    let db = param.Connect().map_err(errors::Trace)?;

    let mut vars: HashMap<String, String> = HashMap::from([
        (
            vardef::TiDBBuildStatsConcurrency.into(),
            dsn.BuildStatsConcurrency.to_string(),
        ),
        (
            vardef::TiDBDistSQLScanConcurrency.into(),
            dsn.DistSQLScanConcurrency.to_string(),
        ),
        (
            vardef::TiDBIndexSerialScanConcurrency.into(),
            dsn.IndexSerialScanConcurrency.to_string(),
        ),
        (
            vardef::TiDBChecksumTableConcurrency.into(),
            dsn.ChecksumTableConcurrency.to_string(),
        ),
        (vardef::TiDBAllowAutoRandExplicitInsert.into(), "1".into()),
        (vardef::TiDBOptWriteRowID.into(), "1".into()),
        (vardef::AutoCommit.into(), "1".into()),
        (vardef::TiDBTxnMode.into(), "optimistic".into()),
        (vardef::ForeignKeyChecks.into(), "0".into()),
        (
            vardef::TiDBExplicitRequestSourceType.into(),
            tikv_util::ExplicitTypeImport.into(),
        ),
    ]);
    if let Some(extra) = &dsn.Vars {
        for (k, v) in extra {
            vars.insert(k.clone(), v.clone());
        }
    }

    let mut failed_keys = Vec::new();
    for (k, v) in &vars {
        let q = format!("SET SESSION {k} = '{v}';");
        if let Err(err1) = db.Exec(&q, &[]) {
            logutil::Logger(ctx.clone()).Warn(
                "set session variable failed, will skip this query",
                &[zap::String("query", &q), zap::Error(&err1)],
            );
            failed_keys.push(k.clone());
        }
    }
    for k in failed_keys {
        vars.remove(&k);
    }
    let _ = db.Close();

    param.Vars = vars;
    let db = param.Connect().map_err(errors::Trace)?;
    Ok(db)
}

// 语义说明：`NewTiDBManager` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn NewTiDBManager(
    ctx: Context,
    dsn: &config::DBStore,
    _tls: Option<&common::TLS>,
) -> Result<TiDBManager> {
    let db = DBFromConfig(ctx, dsn).map_err(errors::Trace)?;
    Ok(NewTiDBManagerWithDB(db, dsn.SQLMode))
}

// 语义说明：`NewTiDBManagerWithDB` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn NewTiDBManagerWithDB(db: DB, sqlMode: mysql::SQLMode) -> TiDBManager {
    let mut parser = Parser::New();
    parser.SetSQLMode(sqlMode);
    TiDBManager { db, parser }
}

// 语义说明：`TiDBManager` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
impl TiDBManager {
    pub fn Close(&self) {
        let _ = self.db.Close();
    }

    pub fn DropTable(&self, ctx: Context, tableName: &str) -> Result<()> {
        let sql = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx.clone()).With(zap::String("table", tableName))),
            HideQueryLog: false,
        };
        sql.Exec(ctx, "drop table", format!("DROP TABLE {tableName}"))
    }
}

// 语义说明：`LoadSchemaInfo` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn LoadSchemaInfo(
    ctx: Context,
    schemas: &[mydump::MDDatabaseMeta],
    getTables: &dyn Fn(Context, &str) -> Result<Vec<model::TableInfo>>,
) -> Result<HashMap<String, importdef::DBInfo>> {
    let mut result = HashMap::with_capacity(schemas.len());
    for schema in schemas {
        let tables = getTables(ctx.clone(), &schema.Name)?;
        let mut tableMap = HashMap::with_capacity(tables.len());
        for tbl in tables {
            tableMap.insert(tbl.Name.L.clone(), tbl);
        }
        let mut dbInfo = importdef::DBInfo {
            Name: schema.Name.clone(),
            Tables: HashMap::new(),
        };
        for tbl in &schema.Tables {
            let tblInfo = match tableMap.get(&tbl.Name.to_ascii_lowercase()) {
                Some(t) => t.clone(),
                None => return Err(common_ext::ErrSchemaNotExists(&tbl.DB, &tbl.Name)),
            };
            let tableName = tblInfo.Name.String();
            if tblInfo.State != model::StatePublic {
                let err = errors::Errorf(format!(
                    "table [{}.{}] state is not public",
                    schema.Name, tableName
                ));
                if let Some(m) = metric::FromContext(ctx.clone()) {
                    m.RecordTableCount(metric::TableStatePending, Some(&err));
                }
                return Err(errors::Trace(err));
            }
            if let Some(m) = metric::FromContext(ctx.clone()) {
                m.RecordTableCount(metric::TableStatePending, None);
            }
            let tableInfo = importdef::TableInfo {
                ID: tblInfo.ID,
                DB: schema.Name.clone(),
                Name: tbl.Name.clone(),
                Core: tblInfo.clone(),
                Desired: Some(tblInfo),
            };
            dbInfo.Tables.insert(tbl.Name.clone(), tableInfo);
        }
        result.insert(schema.Name.clone(), dbInfo);
    }
    Ok(result)
}

// 语义说明：`ObtainImportantVariables` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn ObtainImportantVariables(
    ctx: Context,
    db: &DB,
    needTiDBVars: bool,
) -> HashMap<String, String> {
    let mut query = String::from("SHOW VARIABLES WHERE Variable_name IN ('");
    let mut first = true;
    for k in common_ext::DefaultImportantVariables().keys() {
        if first {
            first = false;
        } else {
            query.push_str("','");
        }
        query.push_str(k);
    }
    if needTiDBVars {
        for k in common_ext::DefaultImportVariablesTiDB().keys() {
            query.push_str("','");
            query.push_str(k);
        }
    }
    query.push_str("')");
    let exec = common::SQLWithRetry {
        DB: db.clone(),
        Logger: log::Wrap(logutil::Logger(ctx.clone())),
        HideQueryLog: false,
    };
    let kvs = match exec.QueryStringRows(ctx.clone(), "obtain system variables", &query) {
        Ok(v) => v,
        Err(err) => {
            logutil::Logger(ctx).Warn(
                "obtain system variables failed, use default variables instead",
                &[log::ShortError(&err)],
            );
            Vec::new()
        }
    };

    let mut result = HashMap::new();
    for kv in kvs {
        if kv.len() >= 2 {
            result.insert(kv[0].clone(), kv[1].clone());
        }
    }
    for (k, defV) in common_ext::DefaultImportantVariables() {
        result.entry(k.clone()).or_insert_with(|| defV.clone());
    }
    if needTiDBVars {
        for (k, defV) in common_ext::DefaultImportVariablesTiDB() {
            result.entry(k.clone()).or_insert_with(|| defV.clone());
        }
    }
    result
}

// 语义说明：`ObtainNewCollationEnabled` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn ObtainNewCollationEnabled(ctx: Context, db: &DB) -> Result<bool> {
    let mut newCollationVal = String::new();
    let exec = common::SQLWithRetry {
        DB: db.clone(),
        Logger: log::Wrap(logutil::Logger(ctx.clone())),
        HideQueryLog: false,
    };
    let err = exec.QueryRow(
        ctx,
        "obtain new collation enabled",
        "SELECT variable_value FROM mysql.tidb WHERE variable_name = 'new_collation_enabled'",
        &mut newCollationVal,
    );
    match err {
        Ok(()) if newCollationVal == "True" => Ok(true),
        Ok(()) => Ok(false),
        Err(e) if e.not_found || e.class == Some("ErrNoRows") => Ok(false),
        Err(e) => Err(errors::Trace(e)),
    }
}

// 语义说明：`AlterAutoIncrement` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn AlterAutoIncrement(ctx: Context, db: &DB, tableName: &str, incr: u64) -> Result<()> {
    let logger = log::Wrap(
        logutil::Logger(ctx.clone())
            .With(zap::String("table", tableName))
            .With(zap::Uint64("auto_increment", incr)),
    );
    let base = adjustIDBase(incr);
    let mut forceStr = "";
    if incr > i64::MAX as u64 {
        logger.Warn(
            "auto_increment out of the maximum value TiDB supports, automatically set to the max",
            &[zap::Uint64("auto_increment", incr)],
        );
        forceStr = "FORCE";
    }
    let query = format!("ALTER TABLE {tableName} {forceStr} AUTO_INCREMENT={base}");
    let task = logger.Begin(zap::InfoLevel, "alter table auto_increment");
    let exec = common::SQLWithRetry {
        DB: db.clone(),
        Logger: logger.clone(),
        HideQueryLog: false,
    };
    let err = exec.Exec(ctx, "alter table auto_increment", query.clone());
    let _ = task.End(zap::ErrorLevel, err.as_ref().err());
    if let Err(ref e) = err {
        task.Error(
            "alter table auto_increment failed, please perform the query manually (this is needed no matter the table has an auto-increment column or not)",
            &[zap::String("query", &query)],
        );
        return Err(errors::Annotatef(e.clone(), format!("{query}")));
    }
    Ok(())
}

// 语义说明：`adjustIDBase` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn adjustIDBase(incr: u64) -> i64 {
    if incr > i64::MAX as u64 {
        i64::MAX
    } else {
        incr as i64
    }
}

// 语义说明：`AlterAutoRandom` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
pub fn AlterAutoRandom(
    ctx: Context,
    db: &DB,
    tableName: &str,
    mut randomBase: u64,
    maxAutoRandom: u64,
) -> Result<()> {
    let logger = log::Wrap(
        logutil::Logger(ctx.clone())
            .With(zap::String("table", tableName))
            .With(zap::Uint64("auto_random", randomBase)),
    );
    if randomBase == maxAutoRandom.wrapping_add(1) {
        randomBase = maxAutoRandom;
    } else if randomBase > maxAutoRandom {
        logger.Warn("auto_random out of the maximum value TiDB supports", &[]);
        return Ok(());
    }
    let query = format!("ALTER TABLE {tableName} AUTO_RANDOM_BASE={randomBase}");
    let task = logger.Begin(zap::InfoLevel, "alter table auto_random");
    let exec = common::SQLWithRetry {
        DB: db.clone(),
        Logger: logger.clone(),
        HideQueryLog: false,
    };
    let err = exec.Exec(ctx, "alter table auto_random_base", query.clone());
    let _ = task.End(zap::ErrorLevel, err.as_ref().err());
    if let Err(ref e) = err {
        task.Error(
            "alter table auto_random_base failed, please perform the query manually (this is needed no matter the table has an auto-random column or not)",
            &[zap::String("query", &query)],
        );
        return Err(errors::Annotatef(e.clone(), format!("{query}")));
    }
    Ok(())
}
