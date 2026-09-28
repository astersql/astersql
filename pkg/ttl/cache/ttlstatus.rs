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

// TTL 表级作业状态（mysql.tidb_ttl_table_status）的查询、缓存与行解码。
//
// `TableStatusCache` 按物理表 ID 缓存当前/上次 job 元数据，供调度器判断
// 是否需要刷新或启动新的 TTL job。

// mysql.tidb_ttl_table_status 的查询、缓存更新和 row 解码逻辑。

use std::collections::HashMap;
use std::time::Duration;

use crate::base::{baseCache, newBaseCache};
use crate::task::{Datum, Row, int, string};

/// TTL job 生命周期状态。Go 使用字符串别名，必须保留系统表中的未知状态值。
pub type JobStatus = String;
pub const JobStatusWaiting: &str = "waiting";
pub const JobStatusRunning: &str = "running";
pub const JobStatusCancelling: &str = "cancelling";
pub const JobStatusCancelled: &str = "cancelled";
pub const JobStatusTimeout: &str = "timeout";
pub const JobStatusFinished: &str = "finished";
pub const selectFromTTLTableStatus: &str = "SELECT LOW_PRIORITY table_id,parent_table_id,table_statistics,last_job_id,last_job_start_time,last_job_finish_time,last_job_ttl_expire,last_job_summary,current_job_id,current_job_owner_id,current_job_owner_addr,current_job_owner_hb_time,current_job_start_time,current_job_ttl_expire,current_job_state,current_job_status,current_job_status_update_time FROM mysql.tidb_ttl_table_status";
/// 按 table_id 查询单表状态。
pub fn SelectFromTTLTableStatusWithID(table_id: i64) -> (String, Vec<Datum>) {
    (
        format!("{selectFromTTLTableStatus} WHERE table_id = %?"),
        vec![Datum::Int(table_id)],
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableStatus {
    pub TableID: i64,
    pub ParentTableID: i64,
    pub TableStatistics: String,
    pub LastJobID: String,
    pub LastJobStartTime: i64,
    pub LastJobFinishTime: i64,
    pub LastJobTTLExpire: i64,
    pub LastJobSummary: String,
    pub CurrentJobID: String,
    pub CurrentJobOwnerID: String,
    pub CurrentJobOwnerAddr: String,
    pub CurrentJobOwnerHBTime: i64,
    pub CurrentJobStartTime: i64,
    pub CurrentJobTTLExpire: i64,
    pub CurrentJobState: String,
    pub CurrentJobStatus: JobStatus,
    pub CurrentJobStatusUpdateTime: i64,
}

/// 执行状态查询所需的最小会话能力。
pub trait StatusSession {
    fn execute_sql(&self, sql: &str) -> Result<Vec<Row>, String>;
}
pub struct TableStatusCache {
    cache: baseCache,
    pub Tables: HashMap<i64, TableStatus>,
}
/// 创建空缓存并设置刷新间隔。
pub fn NewTableStatusCache(update_interval: Duration) -> TableStatusCache {
    TableStatusCache {
        cache: newBaseCache(update_interval),
        Tables: HashMap::new(),
    }
}
impl TableStatusCache {
    /// 距上次更新是否已超过间隔。
    pub fn ShouldUpdate(&self) -> bool {
        self.cache.ShouldUpdate()
    }
    /// 调整缓存刷新间隔。
    pub fn SetInterval(&mut self, interval: Duration) {
        self.cache.SetInterval(interval);
    }
    /// 全量重读系统表并替换 map，避免半更新可见。
    pub fn Update(&mut self, session: &dyn StatusSession) -> Result<(), String> {
        let rows = session.execute_sql(selectFromTTLTableStatus)?;
        let mut tables = HashMap::with_capacity(rows.len());
        for row in rows {
            let status = RowToTableStatus(&row)?;
            tables.insert(status.TableID, status);
        }
        self.Tables = tables;
        self.cache.MarkUpdated();
        Ok(())
    }
}
/// 将 17 列状态行映射为 TableStatus。
pub fn RowToTableStatus(row: &Row) -> Result<TableStatus, String> {
    if row.len() < 17 {
        return Err(format!(
            "TTL status row has {} columns, expected 17",
            row.len()
        ));
    }
    // Go 仅把非 NULL 的空串规范化为 waiting；NULL 保持字符串零值，未知值原样保留。
    let current_status = match row.get(15) {
        Some(Datum::Null) | None => String::new(),
        Some(_) => {
            let value = string(row, 15);
            if value.is_empty() {
                JobStatusWaiting.to_owned()
            } else {
                value
            }
        }
    };
    Ok(TableStatus {
        TableID: int(row, 0),
        ParentTableID: int(row, 1),
        TableStatistics: string(row, 2),
        LastJobID: string(row, 3),
        LastJobStartTime: int(row, 4),
        LastJobFinishTime: int(row, 5),
        LastJobTTLExpire: int(row, 6),
        LastJobSummary: string(row, 7),
        CurrentJobID: string(row, 8),
        CurrentJobOwnerID: string(row, 9),
        CurrentJobOwnerAddr: string(row, 10),
        CurrentJobOwnerHBTime: int(row, 11),
        CurrentJobStartTime: int(row, 12),
        CurrentJobTTLExpire: int(row, 13),
        CurrentJobState: string(row, 14),
        CurrentJobStatus: current_status,
        CurrentJobStatusUpdateTime: int(row, 16),
    })
}
