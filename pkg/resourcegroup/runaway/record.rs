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

// Runaway 记录与 quarantine 监视记录的数据结构及系统表 SQL 构造。
//
// - `Record`：写入 `mysql.tidb_runaway_queries` 的失控查询日志。
// - `QuarantineRecord`：写入 `mysql.tidb_runaway_watch` 的隔离监视项；
//   到期或手动移除时转入 `mysql.tidb_runaway_watch_done`。
// `handleRunawayWatchDone` 在同一事务中插入 done 表并删除 watch 表行。

use std::collections::HashMap;

use crate::{ExecutorRef, Result, RunawayAction, RunawayWatchType, Timestamp, nowMicros};

/// 获取插入自增 ID 的最大重试次数。
pub const MAX_ID_RETRIES: usize = 3;
/// quarantine 监视表全名。
pub const RUNAWAY_WATCH_FULL_TABLE_NAME: &str = "mysql.tidb_runaway_watch";
/// 已完成（移除）的监视记录表全名。
pub const RUNAWAY_WATCH_DONE_FULL_TABLE_NAME: &str = "mysql.tidb_runaway_watch_done";

/// 受限 SQL 参数值，对应系统表绑定参数类型。
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Int(i64),
    /// 无符号整数。
    UInt(u64),
    /// 文本。
    Text(String),
    /// 微秒时间戳，映射为 DATETIME。
    Time(Timestamp),
}

/// 一条失控查询日志记录（对应 `tidb_runaway_queries` 行）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Record {
    /// 所属资源组。
    pub ResourceGroupName: String,
    /// 触发时间（微秒）。
    pub StartTime: Timestamp,
    /// 匹配来源：`identify`（规则超限）或 `watch`（监视命中）。
    pub Match: String,
    /// 动作字符串（SwitchGroup 可能带目标组名）。
    pub Action: String,
    /// 采样 SQL 文本。
    pub SampleText: String,
    /// SQL digest 指纹。
    pub SQLDigest: String,
    /// 执行计划 digest。
    pub PlanDigest: String,
    /// 来源 TiDB 节点标识。
    pub Source: String,
    /// 超限原因描述（ElapsedTime / RequestUnit / ProcessedKeys 等）。
    pub ExceedCause: String,
    /// 同一键合并后的重复次数。
    pub Repeats: i64,
}

/// 用于在 map 中合并重复 runaway 查询的复合键。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecordKey {
    /// 资源组名。
    pub ResourceGroupName: String,
    /// SQL digest。
    pub SQLDigest: String,
    /// 计划 digest。
    pub PlanDigest: String,
    /// 匹配类型。
    pub Match: String,
}
impl From<&Record> for RecordKey {
    fn from(r: &Record) -> Self {
        Self {
            ResourceGroupName: r.ResourceGroupName.clone(),
            SQLDigest: r.SQLDigest.clone(),
            PlanDigest: r.PlanDigest.clone(),
            Match: r.Match.clone(),
        }
    }
}

/// 构造批量插入 `tidb_runaway_queries` 的 SQL 与参数列表。
pub fn genRunawayQueriesStmt(records: &HashMap<RecordKey, Record>) -> (String, Vec<SqlValue>) {
    let mut sql = "INSERT INTO mysql.tidb_runaway_queries (resource_group_name, start_time, match_type, action, sample_sql, sql_digest, plan_digest, tidb_server, rule, repeats) VALUES ".to_owned();
    let mut params = Vec::with_capacity(records.len() * 10);
    for (index, r) in records.values().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push_str("(%?, %?, %?, %?, %?, %?, %?, %?, %?, %?)");
        params.extend([
            SqlValue::Text(r.ResourceGroupName.clone()),
            SqlValue::Time(r.StartTime),
            SqlValue::Text(r.Match.clone()),
            SqlValue::Text(r.Action.clone()),
            SqlValue::Text(r.SampleText.clone()),
            SqlValue::Text(r.SQLDigest.clone()),
            SqlValue::Text(r.PlanDigest.clone()),
            SqlValue::Text(r.Source.clone()),
            SqlValue::Text(r.ExceedCause.clone()),
            SqlValue::Int(r.Repeats),
        ]);
    }
    (sql, params)
}

/// quarantine 监视记录（对应 `tidb_runaway_watch` 行）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QuarantineRecord {
    /// 系统表自增主键；本地新建尚未落库时为 0。
    pub ID: i64,
    /// 资源组名。
    pub ResourceGroupName: String,
    /// 监视开始时间。
    pub StartTime: Timestamp,
    /// 过期时间；0 表示不过期（SQL 侧写 NULL）。
    pub EndTime: Timestamp,
    /// 监视匹配类型。
    pub Watch: RunawayWatchType,
    /// 匹配文本（原始 SQL / digest / plan digest）。
    pub WatchText: String,
    /// 来源（节点 ID 或 `manual`）。
    pub Source: String,
    /// 触发 quarantine 的超限原因。
    pub ExceedCause: String,
    /// 命中后执行的动作。
    pub Action: RunawayAction,
    /// SwitchGroup 目标资源组。
    pub SwitchGroupName: String,
}
impl QuarantineRecord {
    /// 内存监视列表键：`资源组/匹配文本`。
    pub fn getRecordKey(&self) -> String {
        format!("{}/{}", self.ResourceGroupName, self.WatchText)
    }
    /// SwitchGroup 时返回目标组名，否则空串。
    pub fn getSwitchGroupName(&self) -> &str {
        if self.Action == RunawayAction::SwitchGroup {
            &self.SwitchGroupName
        } else {
            ""
        }
    }
    /// 超限原因只读访问。
    pub fn GetExceedCause(&self) -> &str {
        &self.ExceedCause
    }
    /// 动作文案；SwitchGroup 附带目标组名。
    pub fn GetActionString(&self) -> String {
        if self.Action == RunawayAction::SwitchGroup {
            format!("{}({})", self.Action, self.SwitchGroupName)
        } else {
            self.Action.to_string()
        }
    }
    /// 单条插入 watch 表的语句。
    pub fn genInsertionStmt(&self) -> (String, Vec<SqlValue>) {
        (
            format!(
                "insert into {RUNAWAY_WATCH_FULL_TABLE_NAME} VALUES (null, %?, %?, %?, %?, %?, %?, %?, %?, %?)"
            ),
            self.watchParams(),
        )
    }
    /// 插入 done 表的语句（含原 ID 与 done_time）。
    pub fn genInsertionDoneStmt(&self) -> (String, Vec<SqlValue>) {
        let mut params = vec![SqlValue::Int(self.ID)];
        params.extend(self.watchParams());
        params.push(SqlValue::Time(nowMicros()));
        (
            format!(
                "insert into {RUNAWAY_WATCH_DONE_FULL_TABLE_NAME} VALUES (null, %?, %?, %?, %?, %?, %?, %?, %?, %?, %?, %?)"
            ),
            params,
        )
    }
    /// 按 ID 删除 watch 表行的语句。
    pub fn genDeletionStmt(&self) -> (String, Vec<SqlValue>) {
        (
            format!("delete from {RUNAWAY_WATCH_FULL_TABLE_NAME} where id = %?"),
            vec![SqlValue::Int(self.ID)],
        )
    }
    /// watch / done 表共用的列参数（不含 ID 与 done_time）。
    fn watchParams(&self) -> Vec<SqlValue> {
        vec![
            SqlValue::Text(self.ResourceGroupName.clone()),
            SqlValue::Time(self.StartTime),
            if self.EndTime == 0 {
                SqlValue::Null
            } else {
                SqlValue::Time(self.EndTime)
            },
            SqlValue::Int(self.Watch as i64),
            SqlValue::Text(self.WatchText.clone()),
            SqlValue::Text(self.Source.clone()),
            SqlValue::Int(self.Action as i64),
            SqlValue::Text(self.getSwitchGroupName().to_owned()),
            SqlValue::Text(self.ExceedCause.clone()),
        ]
    }
}

/// 批量插入 watch 表。
pub fn genBatchInsertWatchStmt(
    records: &HashMap<String, QuarantineRecord>,
) -> (String, Vec<SqlValue>) {
    let mut sql = format!("insert into {RUNAWAY_WATCH_FULL_TABLE_NAME} VALUES ");
    let mut params = Vec::with_capacity(records.len() * 9);
    for (index, record) in records.values().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push_str("(null, %?, %?, %?, %?, %?, %?, %?, %?, %?)");
        params.extend(record.watchParams());
    }
    (sql, params)
}

/// 按 ID 列表批量删除 watch 表行。
pub fn genBatchDeleteWatchByIDStmt(
    records: &HashMap<i64, QuarantineRecord>,
) -> (String, Vec<SqlValue>) {
    let placeholders = std::iter::repeat_n("%?", records.len())
        .collect::<Vec<_>>()
        .join(",");
    (
        format!("delete from {RUNAWAY_WATCH_FULL_TABLE_NAME} where id in ({placeholders})"),
        records.keys().map(|id| SqlValue::Int(*id)).collect(),
    )
}

/// 在事务中将监视记录移入 done 表并删除 watch 行。
pub fn handleRunawayWatchDone(executor: &ExecutorRef, record: &QuarantineRecord) -> Result<()> {
    let (insert, insert_params) = record.genInsertionDoneStmt();
    executor.Execute("begin", &[])?;
    if let Err(error) = executor.Execute(&insert, &insert_params) {
        let _ = executor.Execute("rollback", &[]);
        return Err(error);
    }
    let (delete, delete_params) = record.genDeletionStmt();
    if let Err(error) = executor.Execute(&delete, &delete_params) {
        let _ = executor.Execute("rollback", &[]);
        return Err(error);
    }
    // Go's function has an unnamed return value: the deferred COMMIT runs after
    // the successful result has been fixed, so a commit error is not returned.
    let _ = executor.Execute("commit", &[]);
    Ok(())
}
