// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// TopRU（按用户 Resource Unit 消耗排行）的内存数据模型。
//
// 维护用户 → SQL/Plan 维度的 RU 时间序列；超 pre-topN 容量时汇入 others 桶。
// compact / merge 支持窗口聚合与上报前裁剪；`toTopRURecords` 转为 tipb 线格式。
// RU（Resource Unit）是云上计量读写/CPU 等资源的抽象计费单位。

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::{stmtstats, tipb_protobuf as tipb};

/// 上报时保留的最大用户数。
pub const maxTopUsers: usize = 200;
/// 每用户上报时保留的最大 SQL 数。
pub const maxTopSQLsPerUser: usize = 200;
/// 收集阶段用户预留容量（上报上限的 2 倍）。
pub const maxPreTopNUsers: usize = maxTopUsers * 2;
/// 收集阶段每用户 SQL 预留容量。
pub const maxPreTopNSQLsPerUser: usize = maxTopSQLsPerUser * 2;
/// 汇总用户 others 在线上的固定标签。
pub const othersUserWireLabel: &str = "_TIDB_TOPRU_OTHERS_USER";

/// 单个时间戳上的 RU 采样点。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ruItem {
    pub timestamp: u64,
    pub totalRU: f64,
    pub execCount: u64,
    pub execDuration: u64,
}

impl ruItem {
    /// 转为 tipb TopRuRecordItem。
    pub fn toProto(&self) -> tipb::TopRuRecordItem {
        let mut item = tipb::TopRuRecordItem::new();
        item.set_timestamp_sec(self.timestamp);
        item.set_total_ru(self.totalRU);
        item.set_exec_count(self.execCount);
        item.set_exec_duration(self.execDuration);
        item
    }
}

/// ruItem 列表。
pub type ruItems = Vec<ruItem>;

/// SQL digest + Plan digest 复合键。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct sqlPlanKey {
    pub sqlDigest: stmtstats::BinaryDigest,
    pub planDigest: stmtstats::BinaryDigest,
}

/// 空 digest 表示 others 聚合键。
pub static othersKey: LazyLock<sqlPlanKey> = LazyLock::new(sqlPlanKey::default);

/// 构造 sqlPlanKey。
pub fn makeKey(
    sqlDigest: stmtstats::BinaryDigest,
    planDigest: stmtstats::BinaryDigest,
) -> sqlPlanKey {
    sqlPlanKey {
        sqlDigest,
        planDigest,
    }
}

/// 是否为 others 键。
pub fn isOthersKey(key: &sqlPlanKey) -> bool {
    key == &*othersKey
}

/// 一条 SQL/Plan 的多时间戳 RU 记录及总量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ruRecord {
    pub sqlDigest: stmtstats::BinaryDigest,
    pub planDigest: stmtstats::BinaryDigest,
    pub items: ruItems,
    pub totalRU: f64,
}

/// 新建空 ruRecord。
pub fn newRURecord(
    sqlDigest: stmtstats::BinaryDigest,
    planDigest: stmtstats::BinaryDigest,
) -> Box<ruRecord> {
    Box::new(ruRecord {
        sqlDigest,
        planDigest,
        items: Vec::with_capacity(4),
        totalRU: 0.0,
    })
}

/// 新建 others 用的空 digest 记录。
pub fn newOthersRURecord() -> Box<ruRecord> {
    newRURecord(Default::default(), Default::default())
}

impl ruRecord {
    /// 累加到同 timestamp 的 item，否则追加；同步更新 totalRU。
    pub fn add(&mut self, timestamp: u64, totalRU: f64, execCount: u64, execDuration: u64) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.timestamp == timestamp)
        {
            item.totalRU += totalRU;
            item.execCount = item.execCount.wrapping_add(execCount);
            item.execDuration = item.execDuration.wrapping_add(execDuration);
        } else {
            self.items.push(ruItem {
                timestamp,
                totalRU,
                execCount,
                execDuration,
            });
        }
        self.totalRU += totalRU;
    }

    /// 从 RUIncrement 累加；None 则忽略。
    pub fn addIncr(&mut self, timestamp: u64, incr: Option<&stmtstats::RUIncrement>) {
        if let Some(incr) = incr {
            self.add(timestamp, incr.TotalRU, incr.ExecCount, incr.ExecDuration);
        }
    }

    /// 合并另一记录的各时间点（保留原 timestamp）。
    pub fn merge(&mut self, other: Option<&ruRecord>) {
        if let Some(other) = other {
            for item in &other.items {
                self.add(
                    item.timestamp,
                    item.totalRU,
                    item.execCount,
                    item.execDuration,
                );
            }
        }
    }

    /// 将另一记录各项重写到统一 timestamp 后合并。
    pub fn mergeWithTimestamp(&mut self, other: Option<&ruRecord>, timestamp: u64) {
        if let Some(other) = other {
            for item in &other.items {
                self.add(timestamp, item.totalRU, item.execCount, item.execDuration);
            }
        }
    }
}

/// ruRecord 列表。
pub type ruRecords = Vec<Box<ruRecord>>;

/// 按 totalRU 降序取前 n，其余为淘汰集。
fn splitTopRecords(mut records: ruRecords, n: usize) -> (ruRecords, ruRecords) {
    if records.len() <= n {
        return (records, Vec::new());
    }
    records.sort_by(|left, right| right.totalRU.total_cmp(&left.totalRU));
    let evicted = records.split_off(n);
    (records, evicted)
}

/// 单用户收集：SQL 映射 + othersRec + 总量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct userRUCollecting {
    pub records: HashMap<sqlPlanKey, Box<ruRecord>>,
    pub othersRec: Option<Box<ruRecord>>,
    pub user: String,
    pub totalRU: f64,
    pub preTopNSQLsPerUser: usize,
}

/// 使用默认 pre-topN SQL 容量构造。
pub fn newUserRUCollecting(user: impl Into<String>) -> Box<userRUCollecting> {
    newUserRUCollectingWithCap(user, maxPreTopNSQLsPerUser)
}

/// 指定每用户 SQL 预留容量；0 表示用默认。
pub fn newUserRUCollectingWithCap(
    user: impl Into<String>,
    preTopNSQLsPerUser: usize,
) -> Box<userRUCollecting> {
    let cap = if preTopNSQLsPerUser == 0 {
        maxPreTopNSQLsPerUser
    } else {
        preTopNSQLsPerUser
    };
    Box::new(userRUCollecting {
        records: HashMap::with_capacity(cap),
        othersRec: None,
        user: user.into(),
        totalRU: 0.0,
        preTopNSQLsPerUser: cap,
    })
}

/// 构造空用户名的 others 用户收集器。
pub fn newOthersUserRUCollectingWithCap(cap: usize) -> Box<userRUCollecting> {
    newUserRUCollectingWithCap(String::new(), cap)
}

impl userRUCollecting {
    /// 按 key 累加；others 键或超容量则进入 othersRec。
    pub fn add(
        &mut self,
        timestamp: u64,
        sqlDigest: stmtstats::BinaryDigest,
        planDigest: stmtstats::BinaryDigest,
        incr: Option<&stmtstats::RUIncrement>,
    ) {
        let Some(incr) = incr else { return };
        let key = makeKey(sqlDigest, planDigest);
        if isOthersKey(&key) {
            self.addOthers(timestamp, Some(incr));
            return;
        }
        if let Some(record) = self.records.get_mut(&key) {
            record.addIncr(timestamp, Some(incr));
            self.totalRU += incr.TotalRU;
            return;
        }
        // 超出每用户预留条数：汇入 others，避免无界增长。
        if self.records.len() >= self.preTopNSQLsPerUser {
            self.addOthers(timestamp, Some(incr));
            return;
        }
        let mut record = newRURecord(key.sqlDigest.clone(), key.planDigest.clone());
        record.addIncr(timestamp, Some(incr));
        self.records.insert(key, record);
        self.totalRU += incr.TotalRU;
    }

    /// 累加到 othersRec，必要时迁移遗留 othersKey 记录。
    pub fn addOthers(&mut self, timestamp: u64, incr: Option<&stmtstats::RUIncrement>) {
        let Some(incr) = incr else { return };
        if self.othersRec.is_none() {
            let mut others = newOthersRURecord();
            if let Some(legacy) = self.records.remove(&*othersKey) {
                others.merge(Some(&legacy));
            }
            self.othersRec = Some(others);
        }
        self.othersRec
            .as_mut()
            .expect("others record initialized")
            .addIncr(timestamp, Some(incr));
        self.totalRU += incr.TotalRU;
    }

    /// 合并源记录到本用户；超容量或 others 键写入 othersRec。
    pub fn mergeRecord(
        &mut self,
        key: sqlPlanKey,
        src: Option<&ruRecord>,
        targetTimestamp: u64,
        rewriteTimestamp: bool,
    ) {
        let Some(src) = src else { return };
        if src.items.is_empty() {
            return;
        }
        let useOthers = isOthersKey(&key)
            || (!self.records.contains_key(&key) && self.records.len() >= self.preTopNSQLsPerUser);
        let destination = if useOthers {
            self.othersRec.get_or_insert_with(newOthersRURecord)
        } else {
            self.records
                .entry(key.clone())
                .or_insert_with(|| newRURecord(key.sqlDigest.clone(), key.planDigest.clone()))
        };
        if rewriteTimestamp {
            destination.mergeWithTimestamp(Some(src), targetTimestamp);
        } else {
            destination.merge(Some(src));
        }
        self.totalRU += src.totalRU;
    }

    /// 取 topN SQL，淘汰项并入 others 后返回。
    pub fn getReportRecordsWithLimit(&self, topNSQLsPerUser: usize) -> ruRecords {
        let limit = if topNSQLsPerUser == 0 {
            maxTopSQLsPerUser
        } else {
            topNSQLsPerUser
        };
        let all = self.records.values().cloned().collect::<ruRecords>();
        let (mut top, evicted) = splitTopRecords(all, limit);
        let mut others = self.othersRec.clone();
        if !evicted.is_empty() {
            let destination = others.get_or_insert_with(newOthersRURecord);
            for record in evicted {
                destination.merge(Some(&record));
            }
        }
        if let Some(others) = others {
            top.push(others);
        }
        top
    }
}

/// 用户收集器列表。
pub type userRUCollectings = Vec<Box<userRUCollecting>>;

/// 按用户 totalRU 降序取前 n。
fn splitTopUsers(mut users: userRUCollectings, n: usize) -> (userRUCollectings, userRUCollectings) {
    if users.len() <= n {
        return (users, Vec::new());
    }
    users.sort_by(|left, right| right.totalRU.total_cmp(&left.totalRU));
    let evicted = users.split_off(n);
    (users, evicted)
}

/// 全局收集：多用户映射 + othersUser。
#[derive(Debug)]
pub struct ruCollecting {
    pub users: HashMap<String, Box<userRUCollecting>>,
    pub othersUser: Option<Box<userRUCollecting>>,
    pub preTopNUsers: usize,
    pub preTopNSQLsPerUser: usize,
}

impl Clone for ruCollecting {
    fn clone(&self) -> Self {
        Self {
            users: self.users.clone(),
            othersUser: self.othersUser.clone(),
            preTopNUsers: self.preTopNUsers,
            preTopNSQLsPerUser: self.preTopNSQLsPerUser,
        }
    }
}

/// 使用默认 pre-topN 容量构造。
pub fn newRUCollecting() -> Box<ruCollecting> {
    newRUCollectingWithCaps(maxPreTopNUsers, maxPreTopNSQLsPerUser)
}

/// 指定用户/SQL 预留容量；0 表示用默认。
pub fn newRUCollectingWithCaps(
    preTopNUsers: usize,
    preTopNSQLsPerUser: usize,
) -> Box<ruCollecting> {
    let userCap = if preTopNUsers == 0 {
        maxPreTopNUsers
    } else {
        preTopNUsers
    };
    let sqlCap = if preTopNSQLsPerUser == 0 {
        maxPreTopNSQLsPerUser
    } else {
        preTopNSQLsPerUser
    };
    Box::new(ruCollecting {
        users: HashMap::with_capacity(userCap),
        othersUser: None,
        preTopNUsers: userCap,
        preTopNSQLsPerUser: sqlCap,
    })
}

impl ruCollecting {
    /// 按 RUKey 累加；新用户超 cap 则进入 othersUser。
    pub fn add(
        &mut self,
        timestamp: u64,
        key: stmtstats::RUKey,
        incr: Option<&stmtstats::RUIncrement>,
    ) {
        let user = key.User.clone();
        if !self.users.contains_key(&user) {
            if self.users.len() >= self.preTopNUsers {
                self.getOrCreateOthersUser().addOthers(timestamp, incr);
                return;
            }
            self.users.insert(
                user.clone(),
                newUserRUCollectingWithCap(user.clone(), self.preTopNSQLsPerUser),
            );
        }
        self.users.get_mut(&user).expect("user initialized").add(
            timestamp,
            key.SQLDigest,
            key.PlanDigest,
            incr,
        );
    }

    /// 批量 add。
    pub fn addBatch(&mut self, timestamp: u64, increments: stmtstats::RUIncrementMap) {
        for (key, increment) in increments {
            self.add(timestamp, key, Some(&increment));
        }
    }

    /// 取出当前收集内容并重置 users/othersUser。
    pub fn take(&mut self) -> Box<ruCollecting> {
        Box::new(ruCollecting {
            users: std::mem::replace(&mut self.users, HashMap::with_capacity(self.preTopNUsers)),
            othersUser: self.othersUser.take(),
            preTopNUsers: self.preTopNUsers,
            preTopNSQLsPerUser: self.preTopNSQLsPerUser,
        })
    }

    /// 获取或创建用户；超 cap 返回 (None, true) 表示应走 others。
    pub fn getOrCreateUser(&mut self, user: String) -> (Option<&mut userRUCollecting>, bool) {
        if !self.users.contains_key(&user) {
            if self.users.len() >= self.preTopNUsers {
                return (None, true);
            }
            self.users.insert(
                user.clone(),
                newUserRUCollectingWithCap(user.clone(), self.preTopNSQLsPerUser),
            );
        }
        (self.users.get_mut(&user).map(Box::as_mut), false)
    }

    /// 获取或创建 others 用户收集器。
    pub fn getOrCreateOthersUser(&mut self) -> &mut userRUCollecting {
        self.othersUser
            .get_or_insert_with(|| newOthersUserRUCollectingWithCap(self.preTopNSQLsPerUser))
            .as_mut()
    }

    /// 裁剪到 maxUsers/maxSQLsPerUser，淘汰汇入 others；空则 None。
    pub fn compactWithLimits(
        &self,
        maxUsers: usize,
        maxSQLsPerUser: usize,
    ) -> Option<Box<ruCollecting>> {
        let maxUsers = if maxUsers == 0 { maxTopUsers } else { maxUsers };
        let maxSQLsPerUser = if maxSQLsPerUser == 0 {
            maxTopSQLsPerUser
        } else {
            maxSQLsPerUser
        };
        if self.users.is_empty() && self.othersUser.is_none() {
            return None;
        }

        // 先按用户 RU 取 top，再对每用户 SQL 做二次裁剪。
        let allUsers = self.users.values().cloned().collect::<userRUCollectings>();
        let (topUsers, evictedUsers) = splitTopUsers(allUsers, maxUsers);
        let mut result = newRUCollectingWithCaps(maxUsers, maxSQLsPerUser);

        for user in topUsers {
            let mut compacted = newUserRUCollectingWithCap(user.user.clone(), maxSQLsPerUser);
            for record in user.getReportRecordsWithLimit(maxSQLsPerUser) {
                let total = record.totalRU;
                if record.sqlDigest.0.is_empty() && record.planDigest.0.is_empty() {
                    compacted
                        .othersRec
                        .get_or_insert_with(newOthersRURecord)
                        .merge(Some(&record));
                } else {
                    compacted.records.insert(
                        makeKey(record.sqlDigest.clone(), record.planDigest.clone()),
                        record,
                    );
                }
                compacted.totalRU += total;
            }
            result.users.insert(compacted.user.clone(), compacted);
        }

        let mut synthetic = self
            .othersUser
            .as_ref()
            .map(|_| newOthersUserRUCollectingWithCap(maxSQLsPerUser));
        if let (Some(destination), Some(source)) = (synthetic.as_mut(), self.othersUser.as_ref()) {
            mergeUserIntoOthers(Some(destination), Some(source));
        }
        if !evictedUsers.is_empty() {
            let destination =
                synthetic.get_or_insert_with(|| newOthersUserRUCollectingWithCap(maxSQLsPerUser));
            for user in evictedUsers {
                mergeUserIntoOthers(Some(destination), Some(&user));
            }
        }
        result.othersUser = synthetic;
        Some(result)
    }

    /// 转为 tipb TopRuRecord 列表（items 按 timestamp 排序）。
    pub fn toTopRURecords(&mut self, keyspaceName: Vec<u8>) -> Vec<tipb::TopRuRecord> {
        let mut result = Vec::new();
        for user in self.users.values_mut() {
            for record in user.records.values_mut() {
                record.items.sort_by_key(|item| item.timestamp);
                result.push(topRURecord(keyspaceName.clone(), user.user.clone(), record));
            }
            if let Some(record) = user.othersRec.as_mut() {
                record.items.sort_by_key(|item| item.timestamp);
                result.push(topRURecord(keyspaceName.clone(), user.user.clone(), record));
            }
        }
        if let Some(user) = self.othersUser.as_mut() {
            if let Some(record) = user.othersRec.as_mut() {
                record.items.sort_by_key(|item| item.timestamp);
                result.push(topRURecord(
                    keyspaceName,
                    othersUserWireLabel.to_owned(),
                    record,
                ));
            }
        }
        result
    }

    /// 从另一 collecting 合并；溢出用户写入 othersUser。
    pub fn mergeFrom(
        &mut self,
        src: Option<&ruCollecting>,
        targetTimestamp: u64,
        rewriteTimestamp: bool,
    ) {
        let Some(src) = src else { return };
        for sourceUser in src.users.values() {
            let (destination, overflow) = self.getOrCreateUser(sourceUser.user.clone());
            if overflow {
                let destination = self.getOrCreateOthersUser();
                mergeUserIntoOthersWithTimestamp(
                    destination,
                    sourceUser,
                    targetTimestamp,
                    rewriteTimestamp,
                );
                continue;
            }
            let destination = destination.expect("non-overflow user exists");
            for record in sourceUser.records.values() {
                destination.mergeRecord(
                    makeKey(record.sqlDigest.clone(), record.planDigest.clone()),
                    Some(record),
                    targetTimestamp,
                    rewriteTimestamp,
                );
            }
            if let Some(record) = sourceUser.othersRec.as_ref() {
                destination.mergeRecord(
                    (*othersKey).clone(),
                    Some(record),
                    targetTimestamp,
                    rewriteTimestamp,
                );
            }
        }
        if let Some(source) = src.othersUser.as_ref() {
            let destination = self.getOrCreateOthersUser();
            mergeUserIntoOthersWithTimestamp(
                destination,
                source,
                targetTimestamp,
                rewriteTimestamp,
            );
        }
    }
}

/// 将 ruRecord 编码为 tipb TopRuRecord。
fn topRURecord(keyspaceName: Vec<u8>, user: String, record: &ruRecord) -> tipb::TopRuRecord {
    let mut proto = tipb::TopRuRecord::new();
    proto.set_keyspace_name(keyspaceName);
    proto.set_user(user);
    proto.set_sql_digest(record.sqlDigest.as_bytes().to_vec());
    proto.set_plan_digest(record.planDigest.as_bytes().to_vec());
    proto.set_items(record.items.iter().map(ruItem::toProto).collect());
    proto
}

/// 将源用户全部记录并入目标 others（保留原 timestamp）。
pub fn mergeUserIntoOthers(
    destination: Option<&mut userRUCollecting>,
    source: Option<&userRUCollecting>,
) {
    if let (Some(destination), Some(source)) = (destination, source) {
        mergeUserIntoOthersWithTimestamp(destination, source, 0, false);
    }
}

/// 将源用户记录以 others 键并入目标，可选重写 timestamp。
fn mergeUserIntoOthersWithTimestamp(
    destination: &mut userRUCollecting,
    source: &userRUCollecting,
    targetTimestamp: u64,
    rewriteTimestamp: bool,
) {
    for record in source.records.values() {
        destination.mergeRecord(
            (*othersKey).clone(),
            Some(record),
            targetTimestamp,
            rewriteTimestamp,
        );
    }
    if let Some(record) = source.othersRec.as_ref() {
        destination.mergeRecord(
            (*othersKey).clone(),
            Some(record),
            targetTimestamp,
            rewriteTimestamp,
        );
    }
}
