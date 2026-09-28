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

// Runaway 管理器：内存监视列表、记录队列与系统表同步中枢。
//
// `Manager` 维护：
// - 本地 watch 列表（quarantine 隔离监视）；
// - 待刷盘的 runaway / quarantine / stale 记录队列；
// - 通过 `Syncer` 从 `tidb_runaway_watch` / `_done` 增量同步。
//
// 手动来源（`ManualSource`）的 watch 可强制覆盖同键记录。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::checker::Checker;
use crate::record::{MAX_ID_RETRIES, QuarantineRecord, Record, handleRunawayWatchDone};
use crate::syncer::{Syncer, SystemTableCatalog, WATCH_SYNC_OVERLAP_MICROS};
use crate::{
    CatalogRef, Error, ExecutorRef, Result, RunawayAction, RunawayWatchType, Timestamp, nowMicros,
};

/// 手动添加监视记录时的 Source 标记。
pub const ManualSource: &str = "manual";
/// 等待相关操作的最长毫秒数（与 Go 常量对齐）。
pub const MaxWaitDurationMillis: u64 = 30_000;
/// 内存监视列表容量上限，防止无界增长。
const MAX_WATCH_LIST_CAP: usize = 10_000;
/// 各类待刷盘记录队列的最大长度。
const MAX_WATCH_RECORD_CHANNEL_SIZE: usize = 1024;

/// 监视列表中的一条条目：记录本体与过期时间。
#[derive(Clone)]
struct WatchEntry {
    record: QuarantineRecord,
    expires_at: Timestamp,
}

/// Manager 的内部共享状态。
struct ManagerInner {
    catalog: CatalogRef,
    executor: ExecutorRef,
    server_id: String,
    watch_list: Mutex<HashMap<String, WatchEntry>>,
    /// 每个资源组当前活跃 watch 计数。
    active_group: Mutex<HashMap<String, Arc<AtomicI64>>>,
    runaway_records: Mutex<Vec<Record>>,
    quarantine_records: Mutex<Vec<QuarantineRecord>>,
    stale_records: Mutex<Vec<QuarantineRecord>>,
    syncer: Mutex<Syncer>,
    stopped: AtomicBool,
}

/// 可克隆的 runaway 管理器句柄（内部 `Arc` 共享）。
#[derive(Clone)]
pub struct Manager(Arc<ManagerInner>);

impl Manager {
    /// 构造管理器并初始化空同步器与空监视列表。
    pub fn NewRunawayManager(
        catalog: CatalogRef,
        server_id: impl Into<String>,
        executor: ExecutorRef,
        system_tables: Arc<dyn SystemTableCatalog>,
    ) -> Self {
        Self(Arc::new(ManagerInner {
            catalog,
            server_id: server_id.into(),
            syncer: Mutex::new(Syncer::new(executor.clone(), system_tables)),
            executor,
            watch_list: Mutex::new(HashMap::new()),
            active_group: Mutex::new(HashMap::new()),
            runaway_records: Mutex::new(Vec::new()),
            quarantine_records: Mutex::new(Vec::new()),
            stale_records: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
        }))
    }

    /// 从目录查询资源组。
    pub fn resourceGroup(&self, name: &str) -> Result<Option<crate::ResourceGroup>> {
        self.0.catalog.GetResourceGroup(name)
    }

    /// 获取或创建资源组活跃 watch 计数器；第二个返回值表示是否已存在。
    pub fn loadOrStoreActiveCounter(&self, name: &str) -> Result<(Arc<AtomicI64>, bool)> {
        let mut groups = self.0.active_group.lock().map_err(|_| Error::Poisoned)?;
        if let Some(counter) = groups.get(name) {
            return Ok((counter.clone(), true));
        }
        let counter = Arc::new(AtomicI64::new(0));
        groups.insert(name.to_owned(), counter.clone());
        Ok((counter, false))
    }
    /// 读取指定资源组的活跃 watch 数；不存在则为 0。
    pub fn getActiveWatchCount(&self, name: &str) -> i64 {
        self.0
            .active_group
            .lock()
            .ok()
            .and_then(|m| m.get(name).cloned())
            .map(|v| v.load(Ordering::Acquire))
            .unwrap_or_default()
    }

    /// 有界入队；队列满则丢弃，避免阻塞热路径。
    fn queue<T>(queue: &Mutex<Vec<T>>, value: T) {
        if let Ok(mut queue) = queue.lock() {
            if queue.len() < MAX_WATCH_RECORD_CHANNEL_SIZE {
                queue.push(value);
            }
        }
    }

    /// 构造 quarantine 记录，加入内存列表并入队待刷盘。
    pub fn markQuarantine(
        &self,
        group: String,
        convict: String,
        watch: RunawayWatchType,
        action: RunawayAction,
        switch_group: String,
        ttl_micros: i64,
        now: Timestamp,
        cause: String,
    ) {
        let record = QuarantineRecord {
            ResourceGroupName: group,
            StartTime: now,
            EndTime: if ttl_micros > 0 {
                now.saturating_add(ttl_micros)
            } else {
                0
            },
            Watch: watch,
            WatchText: convict,
            Source: self.0.server_id.clone(),
            Action: action,
            SwitchGroupName: switch_group,
            ExceedCause: cause,
            ..Default::default()
        };
        self.addWatchList(record.clone(), false);
        Self::queue(&self.0.quarantine_records, record);
    }

    /// 将监视记录写入本地列表；`force` 时可覆盖同键。
    pub fn addWatchList(&self, record: QuarantineRecord, force: bool) {
        let key = record.getRecordKey();
        let now = nowMicros();
        let expires_at = record.EndTime;
        let Ok(mut watches) = self.0.watch_list.lock() else {
            return;
        };
        // 先清理已过期的同键条目，并递减活跃计数、入队 stale。
        if let Some(stale) = watches
            .get(&key)
            .filter(|entry| entry.expires_at != 0 && entry.expires_at <= now)
            .cloned()
        {
            watches.remove(&key);
            if let Ok((counter, _)) = self.loadOrStoreActiveCounter(&stale.record.ResourceGroupName)
            {
                counter.fetch_sub(1, Ordering::AcqRel);
            }
            if stale.record.ID != 0 {
                Self::queue(&self.0.stale_records, stale.record);
            }
        }
        let existing = watches.get(&key).cloned();
        // Go 对同 ID 的重复扫描保持原对象不变；手工强制替换不同 ID 时，
        // ttlcache 的 eviction 回调会把旧持久化记录送入 stale 队列。
        if existing
            .as_ref()
            .is_some_and(|old| old.record.ID == record.ID)
        {
            return;
        }
        let replace =
            force || existing.is_none() || existing.as_ref().is_some_and(|old| old.record.ID == 0);
        if replace {
            if watches.len() >= MAX_WATCH_LIST_CAP && existing.is_none() {
                return;
            }
            if force {
                if let Some(old) = existing.as_ref().filter(|old| old.record.ID != 0) {
                    Self::queue(&self.0.stale_records, old.record.clone());
                }
            }
            if existing.is_none() {
                if let Ok((counter, _)) = self.loadOrStoreActiveCounter(&record.ResourceGroupName) {
                    counter.fetch_add(1, Ordering::AcqRel);
                }
            }
            watches.insert(key, WatchEntry { record, expires_at });
        } else {
            Self::queue(&self.0.stale_records, record);
        }
    }

    /// 添加监视；已过期则直接入 stale；手动来源强制覆盖。
    pub fn AddWatch(&self, record: QuarantineRecord) {
        if record.EndTime != 0 && record.EndTime <= nowMicros() {
            Self::queue(&self.0.stale_records, record);
            return;
        }
        let force = record.Source == ManualSource;
        self.addWatchList(record, force);
    }

    /// 按 ID 匹配后从内存列表移除，并递减活跃计数。
    pub fn removeWatch(&self, record: &QuarantineRecord) {
        let key = record.getRecordKey();
        let Ok(mut watches) = self.0.watch_list.lock() else {
            return;
        };
        if watches
            .get(&key)
            .is_some_and(|item| item.record.ID == record.ID)
        {
            if let Some(removed) = watches.remove(&key) {
                if let Ok((counter, _)) =
                    self.loadOrStoreActiveCounter(&removed.record.ResourceGroupName)
                {
                    counter.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
    }

    /// 导出当前全部监视记录快照。
    pub fn GetWatchList(&self) -> Vec<QuarantineRecord> {
        self.0
            .watch_list
            .lock()
            .map(|m| m.values().map(|v| v.record.clone()).collect())
            .unwrap_or_default()
    }
    /// 按 `资源组/convict` 查找监视项，返回是否命中及动作信息。
    pub fn examineWatchList(
        &self,
        group: &str,
        convict: &str,
    ) -> (bool, RunawayAction, String, String) {
        let key = format!("{group}/{convict}");
        let record = self
            .0
            .watch_list
            .lock()
            .ok()
            .and_then(|m| m.get(&key).cloned())
            .map(|v| v.record);
        record.map_or(
            (
                false,
                RunawayAction::NoneAction,
                String::new(),
                String::new(),
            ),
            |r| {
                (
                    true,
                    r.Action,
                    r.getSwitchGroupName().to_owned(),
                    r.ExceedCause,
                )
            },
        )
    }

    /// 将一条 runaway 查询日志入队。
    pub fn markRunaway(
        &self,
        checker: &Checker,
        action: String,
        match_type: String,
        now: Timestamp,
        cause: String,
    ) {
        Self::queue(
            &self.0.runaway_records,
            Record {
                ResourceGroupName: checker.resource_group_name.clone(),
                StartTime: now,
                Match: match_type,
                Action: action,
                SampleText: checker.original_sql.clone(),
                SQLDigest: checker.sql_digest.clone(),
                PlanDigest: checker.plan_digest.clone(),
                Source: self.0.server_id.clone(),
                ExceedCause: cause,
                Repeats: 1,
            },
        );
    }

    /// 取出并清空 runaway 记录队列。
    pub fn drainRunawayRecords(&self) -> Vec<Record> {
        self.0
            .runaway_records
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
    /// 取出并清空待写入 watch 表的 quarantine 队列。
    pub fn drainQuarantineRecords(&self) -> Vec<QuarantineRecord> {
        self.0
            .quarantine_records
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
    /// 取出并清空过期/冲突的 stale 记录队列。
    pub fn drainStaleRecords(&self) -> Vec<QuarantineRecord> {
        self.0
            .stale_records
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    /// 从系统表拉取新增 watch 与 done 记录，更新本地列表。
    pub fn UpdateNewAndDoneWatch(&self) -> Result<()> {
        let mut syncer = self.0.syncer.lock().map_err(|_| Error::Poisoned)?;
        syncer.last_sync_time = nowMicros();
        if !syncer.checkWatchTableExist() {
            return Ok(());
        }
        for record in syncer.getNewWatchRecords()? {
            self.AddWatch(record);
        }
        // 首次同步 done 表时，用 new_watch 上界减去 overlap 初始化删除游标。
        if syncer.deletion_watch_reader.check_point == 0 {
            syncer.deletion_watch_reader.check_point =
                syncer.new_watch_reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS;
        }
        if !syncer.checkWatchDoneTableExist() {
            return Ok(());
        }
        for record in syncer.getNewWatchDoneRecords()? {
            self.removeWatch(&record);
        }
        Ok(())
    }

    /// 将监视记录插入系统表并重试获取自增 ID。
    pub fn AddRunawayWatch(&self, record: &QuarantineRecord) -> Result<u64> {
        let (sql, params) = record.genInsertionStmt();
        self.0.executor.Execute(&sql, &params)?;
        for _ in 0..MAX_ID_RETRIES {
            let id = self.0.executor.LastInsertId();
            if id != 0 {
                return Ok(id);
            }
        }
        Err(Error::Storage(
            "cannot obtain inserted runaway watch ID".into(),
        ))
    }
    /// 按 ID 查找并移入 done 表（事务删除）。
    pub fn RemoveRunawayWatch(&self, record_id: i64) -> Result<()> {
        let records = self
            .0
            .syncer
            .lock()
            .map_err(|_| Error::Poisoned)?
            .getWatchRecordByID(record_id)?;
        if records.len() != 1 {
            return Err(Error::NotFound(format!(
                "runaway watch {record_id} does not exist"
            )));
        }
        handleRunawayWatchDone(&self.0.executor, &records[0])
    }
    /// 移除某资源组下全部监视记录。
    pub fn RemoveRunawayResourceGroupWatch(&self, group: &str) -> Result<()> {
        let records = self
            .0
            .syncer
            .lock()
            .map_err(|_| Error::Poisoned)?
            .getWatchRecordByGroup(group)?;
        for record in records {
            handleRunawayWatchDone(&self.0.executor, &record)?;
        }
        Ok(())
    }
    /// 标记管理器已停止。
    pub fn Stop(&self) {
        self.0.stopped.store(true, Ordering::Release);
    }
    /// 是否已停止。
    pub fn stopped(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }
}

/// 将微秒 checkpoint 转为毫秒浮点，供指标使用；0 保持 0。
pub fn checkpointGaugeValue(checkpoint: Timestamp) -> f64 {
    if checkpoint == 0 {
        0.0
    } else {
        checkpoint as f64 / 1000.0
    }
}
/// 刷盘阈值：队列容量的一半。
pub fn flushThreshold() -> usize {
    MAX_WATCH_RECORD_CHANNEL_SIZE / 2
}
