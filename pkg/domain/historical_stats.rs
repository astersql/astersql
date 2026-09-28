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

// 历史统计信息（historical stats）异步 dump worker。
//
// 提供有界 channel 投递表 ID、按表存在性检查后调用存储层持久化历史统计，
// 以及测试用的非阻塞出队接口。语义对齐 Go 侧非阻塞投递（队列满则丢弃）。

// 历史统计信息 worker 的 channel 投递、表信息解析和持久化调用顺序。
//
// HistoricalStatsWorker indicates for dump historical stats
// HistoricalStatsWorker 对应 Go 结构体：持有待 dump 表 ID channel 和 session context。
// pub struct HistoricalStatsWorker {
//     pub tblCH: chan::Sender<i64>,
//     pub sctx: sessionctx::Context,
// }
//
// impl HistoricalStatsWorker {
// SendTblToDumpHistoricalStats send tableID to worker to dump historical stats
// SendTblToDumpHistoricalStats 对应 Go 的非阻塞投递逻辑。
//     pub fn SendTblToDumpHistoricalStats(&mut self, tableID: i64) {
//         let mut send = enableDumpHistoricalStats.Load();
//         failpoint::Inject("sendHistoricalStats", |val: failpoint::Value| {
//             if val.downcast_ref::<bool>() == Some(&true) {
//                 send = true;
//             }
//         });
//         if !send {
//             return;
//         }
//
// Go 使用 select 的 default 分支避免阻塞；用 try_send 保留“满了就丢弃”的语义。
//         if self.tblCH.try_send(tableID).is_ok() {
//             return;
//         }
//         logutil::BgLogger().Warn(
//             "discard dump historical stats task",
//             zap::Int64("table-id", tableID),
//         );
//     }
//
// DumpHistoricalStats dump stats by given tableID
// DumpHistoricalStats 对应 Go 中按 tableID dump 历史统计信息的主流程。
//     pub fn DumpHistoricalStats(
//         &mut self,
//         tableID: i64,
//         statsHandle: &handle::Handle,
//     ) -> Result<(), errors::Error> {
//         let historicalStatsEnabled = statsHandle
//             .CheckHistoricalStatsEnable()
//             .map_err(|err| errors::Errorf(format!("check tidb_enable_historical_stats failed: {}", err)))?;
//         if !historicalStatsEnabled {
//             return Ok(());
//         }
//
//         let sctx = &self.sctx;
//         let is = GetDomain(sctx).InfoSchema();
//         let mut isPartition = false;
//         let tblInfo: &model::TableInfo;
//
//         let (tbl, existed) = is.TableByID(context::Background(), tableID);
//         if !existed {
//             let (tbl, db, p) = is.FindTableByPartitionID(tableID);
//             if !(tbl.is_some() && db.is_some() && p.is_some()) {
//                 return Err(errors::Errorf(format!("cannot get table by id {}", tableID)));
//             }
// Go 在 partition ID 命中时记录 isPartition，并使用分区所属表的 Meta。
//             isPartition = true;
//             tblInfo = tbl.unwrap().Meta();
//         } else {
//             tblInfo = tbl.Meta();
//         }
//
//         let (dbInfo, existed) = infoschema::SchemaByTable(is, tblInfo);
//         if !existed {
//             return Err(errors::Errorf(format!("cannot get DBInfo by TableID {}", tableID)));
//         }
//         if let Err(err) =
//             statsHandle.RecordHistoricalStatsToStorage(dbInfo.Name.O, tblInfo, tableID, isPartition)
//         {
//             domain_metrics::GenerateHistoricalStatsFailedCounter.Inc();
//             return Err(errors::Errorf(format!(
//                 "record table {}.{}'s historical stats failed, err:{}",
//                 dbInfo.Name.O, tblInfo.Name.O, err
//             )));
//         }
//         domain_metrics::GenerateHistoricalStatsSuccessCounter.Inc();
//         Ok(())
//     }
//
// GetOneHistoricalStatsTable gets one tableID from channel, only used for test
// GetOneHistoricalStatsTable 对应 Go 的测试辅助方法：非阻塞读取一个表 ID。
//     pub fn GetOneHistoricalStatsTable(&mut self) -> i64 {
//         match self.tblCH.try_recv() {
//             Ok(tblID) => tblID,
//             Err(_) => -1,
//         }
//     }
// }
// */
use std::sync::{Arc, Mutex, mpsc};

/// 历史统计持久化后端抽象：检查表是否存在并执行 dump。
pub trait HistoricalStatsStore: Send + Sync {
    /// 判断给定表 ID 是否仍存在于元数据中。
    fn table_exists(&self, table_id: i64) -> Result<bool, String>;
    /// 将指定表的历史统计写入存储。
    fn dump_historical_stats(&self, table_id: i64) -> Result<(), String>;
}

/// Bounded delivery intentionally matches Go's non-blocking channel send.
/// 有界投递，故意对齐 Go 的非阻塞 channel send。
pub struct HistoricalStatsWorker {
    /// 向 worker 投递待 dump 的表 ID。
    sender: mpsc::SyncSender<i64>,
    /// 接收端；用 Mutex 包装以支持 Sync。
    receiver: Mutex<mpsc::Receiver<i64>>,
    /// 实际执行持久化的存储实现。
    store: Arc<dyn HistoricalStatsStore>,
}

impl HistoricalStatsWorker {
    /// 创建 worker；容量至少为 1，对应 Go sync channel 的有界语义。
    pub fn new(store: Arc<dyn HistoricalStatsStore>, capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        Self {
            sender,
            receiver: Mutex::new(receiver),
            store,
        }
    }

    /// Returns false when the queue is full; callers can retry on a later tick.
    /// 队列满时返回 false，调用方可在后续 tick 重试。
    pub fn send_table_to_dump_historical_stats(&self, table_id: i64) -> bool {
        self.sender.try_send(table_id).is_ok()
    }

    /// 若表存在则 dump 历史统计；返回是否实际执行了 dump。
    pub fn dump_historical_stats(&self, table_id: i64) -> Result<bool, String> {
        // 表已删除则跳过，避免对不存在对象写历史统计。
        if !self.store.table_exists(table_id)? {
            return Ok(false);
        }
        self.store.dump_historical_stats(table_id)?;
        Ok(true)
    }

    /// 非阻塞取出一个待 dump 表 ID，仅测试使用；队列空时返回 None。
    pub fn get_one_historical_stats_table(&self) -> Option<i64> {
        self.receiver
            .lock()
            .expect("historical stats queue poisoned")
            .try_recv()
            .ok()
    }
}
