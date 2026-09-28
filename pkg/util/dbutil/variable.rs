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

// MySQL 全局变量与 SHOW GRANTS 查询封装。
//
// 对应 Go `pkg/util/dbutil` 的 `ShowVersion`/`ShowMySQLVariable`/`ShowGrants` 等。
// 通过 `QueryExecutor` 发查询；迁移稿保留在块注释中。不会在本模块内建立真实连接。

// 读取 MySQL 全局变量和 SHOW GRANTS 的查询拼装流程；不会实际连接数据库或执行 SQL。

use crate::interface::{DbError, QueryExecutor, Value};

/// 从 `Value` 取出字符串；仅接受 String/Bytes，其余视为无效。
fn string_value(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Bytes(value)) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    }
}
/// 查询全局变量 `version`。
pub fn ShowVersion(db: &dyn QueryExecutor) -> Result<String, DbError> {
    ShowMySQLVariable(db, "version")
}
/// 查询全局变量 `log_bin`（是否开启二进制日志）。
pub fn ShowLogBin(db: &dyn QueryExecutor) -> Result<String, DbError> {
    ShowMySQLVariable(db, "log_bin")
}
/// 查询全局变量 `binlog_format`。
pub fn ShowBinlogFormat(db: &dyn QueryExecutor) -> Result<String, DbError> {
    ShowMySQLVariable(db, "binlog_format")
}
/// 查询全局变量 `binlog_row_image`。
pub fn ShowBinlogRowImage(db: &dyn QueryExecutor) -> Result<String, DbError> {
    ShowMySQLVariable(db, "binlog_row_image")
}
/// 查询 `server_id` 并解析为 u64。
pub fn ShowServerID(db: &dyn QueryExecutor) -> Result<u64, DbError> {
    let value = ShowMySQLVariable(db, "server_id")?;
    value.parse().map_err(|error| DbError {
        code: 0,
        sql_state: None,
        message: format!("parse server_id {value} failed: {error}"),
    })
}
/// 执行 `SHOW GLOBAL VARIABLES LIKE '…'`，取结果第二列作为变量值。
pub fn ShowMySQLVariable(db: &dyn QueryExecutor, variable: &str) -> Result<String, DbError> {
    // 与 Go 的 fmt.Sprintf 保持一致：变量名原样插入 LIKE 模式。
    let row = db.QueryRowContext(&format!("SHOW GLOBAL VARIABLES LIKE '{}';", variable), &[])?;
    string_value(row.get(1)).ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: format!("variable {variable} has NULL value"),
    })
}

/// 规范化 GRANT 语句中的 `IDENTIFIED BY PASSWORD` 片段，便于后续 parser 解析。
fn normalize_grant(mut grant: String) -> String {
    grant = grant.replacen(
        "IDENTIFIED BY PASSWORD <secret>",
        "IDENTIFIED BY PASSWORD 'secret'",
        1,
    );
    grant = grant.replacen(
        "IDENTIFIED BY PASSWORD WITH",
        "IDENTIFIED BY PASSWORD 'secret' WITH",
        1,
    );
    if grant.ends_with("IDENTIFIED BY PASSWORD") {
        grant.push_str(" 'secret'");
    }
    grant
}
/// 执行 SHOW GRANTS 查询，并将每行首列规范化后收集。
fn read_grants(db: &dyn QueryExecutor, query: &str) -> Result<Vec<String>, DbError> {
    let mut grants = Vec::new();
    for row in db.QueryContext(query, &[])?.rows {
        let grant = string_value(row.first()).ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "SHOW GRANTS returned a NULL or non-string grant".to_owned(),
        })?;
        grants.push(normalize_grant(grant));
    }
    Ok(grants)
}
/// 从 GRANT 语句中提取角色列表（无 ON 子句的角色授权形式）。
fn keyword_outside_quotes(statement: &str, keyword: &str) -> Option<usize> {
    let bytes = statement.as_bytes();
    let keyword = keyword.as_bytes();
    let mut quote = None;
    let mut index = 0;
    while index < bytes.len() {
        if let Some(delimiter) = quote {
            if bytes[index] == b'\\' && delimiter != b'`' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if bytes[index] == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"' | b'`') {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }
        if bytes[index..].len() >= keyword.len()
            && bytes[index..index + keyword.len()].eq_ignore_ascii_case(keyword)
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn split_roles(roles: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut quote = None;
    let mut start = 0;
    let bytes = roles.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(delimiter) = quote {
            if bytes[index] == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
        } else if matches!(bytes[index], b'\'' | b'"' | b'`') {
            quote = Some(bytes[index]);
        } else if bytes[index] == b',' {
            result.push(roles[start..index].trim().to_owned());
            start = index + 1;
        }
        index += 1;
    }
    result.push(roles[start..].trim().to_owned());
    result.into_iter().filter(|role| !role.is_empty()).collect()
}

fn granted_roles(grants: &[String]) -> Vec<String> {
    // MySQL 8.0 角色授权形如 `GRANT role TO user`。关键字与逗号只在引号外生效，
    // 对齐 Go parser 对带空格/保留字角色名的识别结果。
    grants
        .iter()
        .filter_map(|grant| {
            let grant = grant.trim();
            if grant.len() < 6 || !grant[..6].eq_ignore_ascii_case("GRANT ") {
                return None;
            }
            let end = keyword_outside_quotes(&grant[6..], " TO ")? + 6;
            if keyword_outside_quotes(&grant[6..end], " ON ").is_some() {
                return None;
            }
            Some(grant[6..end].trim().to_owned())
        })
        .flat_map(|roles| split_roles(&roles))
        .collect()
}
/// 查询用户权限；若存在角色授权则再以 `USING` 重查以展开角色权限。
pub fn ShowGrants(db: &dyn QueryExecutor, user: &str, host: &str) -> Result<Vec<String>, DbError> {
    // host 为空时默认 `%`；user 为空则查 CURRENT_USER。
    let host = if host.is_empty() { "%" } else { host };
    let base = if user.is_empty() {
        "SHOW GRANTS FOR CURRENT_USER".to_owned()
    } else {
        format!("SHOW GRANTS FOR '{}'@'{}'", user, host)
    };
    let grants = read_grants(db, &base)?;
    let roles = granted_roles(&grants);
    if roles.is_empty() {
        Ok(grants)
    } else {
        // 追加 USING 角色列表后再次查询，展开角色带来的权限。
        read_grants(db, &format!("{base} USING {}", roles.join(", ")))
    }
}
