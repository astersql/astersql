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

// 语句摘要 LRU 淘汰聚合：按历史时间窗口归并被踢出的 digest 统计。
//
// 对应 Go `evicted.go`。当 `stmtSummaryByDigestMap` 因容量限制淘汰条目时，
// 把各时间片统计并入 `other`（evicted）结构，供信息模式表展示淘汰计数。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use crate::{
    StmtDigestKey, mysql, stmtSummaryByDigest, stmtSummaryByDigestElement, stmtSummaryByDigestMap,
    stmtSummaryStats, types,
};
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Digests evicted from the statement-summary LRU, grouped by history interval.
/// 按历史区间分组保存被 LRU 淘汰的 digest 聚合结果。
#[derive(Default)]
pub struct stmtSummaryByDigestEvicted {
    /// Oldest interval is at the front; newest interval is at the back.
    /// 队头为最旧区间，队尾为最新区间。
    pub history: VecDeque<Box<stmtSummaryByDigestEvictedElement>>,
}

/// Aggregated evictions for one statement-summary interval.
/// 单个摘要刷新区间内被淘汰条目的聚合。
pub struct stmtSummaryByDigestEvictedElement {
    /// 区间起始时间（Unix 秒）。
    pub beginTime: i64,
    /// 区间结束时间（Unix 秒）。
    pub endTime: i64,
    /// 该区间内淘汰的 digest 个数。
    pub count: i64,
    /// 被淘汰条目的指标汇总（“other” 桶）。
    pub otherSummary: Box<stmtSummaryByDigestElement>,
}

/// 构造空的淘汰历史容器。
pub fn newStmtSummaryByDigestEvicted() -> Box<stmtSummaryByDigestEvicted> {
    Box::new(stmtSummaryByDigestEvicted::default())
}

/// 构造指定时间窗口的淘汰元素，并初始化 otherSummary 的边界。
pub fn newStmtSummaryByDigestEvictedElement(
    beginTime: i64,
    endTime: i64,
) -> Box<stmtSummaryByDigestEvictedElement> {
    Box::new(stmtSummaryByDigestEvictedElement {
        beginTime,
        endTime,
        count: 0,
        otherSummary: Box::new(stmtSummaryByDigestElement {
            beginTime,
            endTime,
            stmtSummaryStats: stmtSummaryStats {
                minLatency: Duration::from_nanos(i64::MAX as u64),
                firstSeen: unix_system_time(endTime),
                ..Default::default()
            },
        }),
    })
}

/// 将 Unix 秒转为 `SystemTime`（支持负偏移）。
fn unix_system_time(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// 将 Unix 秒转为 MySQL TIMESTAMP Datum 用的 `types::Time`。
fn mysql_timestamp(seconds: i64) -> types::Time {
    let utc = DateTime::<Utc>::from_timestamp(seconds, 0)
        .expect("statement summary timestamp is outside chrono range")
        .with_timezone(&chrono_tz::UTC);
    types::NewTime(types::FromGoTime(utc), mysql::TypeTimestamp, 0)
}

impl stmtSummaryByDigestEvicted {
    /// Adds every interval from an evicted digest, preserving Go's ordering,
    /// matching, nil-key refresh, and oldest-first trimming rules.
    /// 将被淘汰 digest 的各历史区间并入本结构，保持 Go 侧匹配/插入/裁剪语义。
    pub fn AddEvicted(
        &mut self,
        evictedKey: Option<&StmtDigestKey>,
        evictedValue: Option<&stmtSummaryByDigest>,
        historySize: usize,
    ) {
        let Some(evicted_value) = evictedValue else {
            return;
        };

        // 从新到旧遍历被淘汰 digest 的历史窗口。
        for evicted_element in evicted_value.history.iter().rev() {
            if self.history.is_empty() && historySize != 0 {
                let mut record = newStmtSummaryByDigestEvictedElement(
                    evicted_element.beginTime,
                    evicted_element.endTime,
                );
                record.addEvicted(evictedKey, Some(evicted_element));
                self.history.push_front(record);
            } else {
                // 自新向旧扫描，按 begin/end 边界决定匹配、插新或前插。
                let mut cursor = self.history.len().checked_sub(1);
                while let Some(index) = cursor {
                    match self.history[index].matchAndAdd(evictedKey, Some(evicted_element)) {
                        isMatch => break,
                        isTooYoung => {
                            let mut record = newStmtSummaryByDigestEvictedElement(
                                evicted_element.beginTime,
                                evicted_element.endTime,
                            );
                            record.addEvicted(evictedKey, Some(evicted_element));
                            self.history.insert(index + 1, record);
                            break;
                        }
                        isTooOld if index == 0 => {
                            let mut record = newStmtSummaryByDigestEvictedElement(
                                evicted_element.beginTime,
                                evicted_element.endTime,
                            );
                            record.addEvicted(evictedKey, Some(evicted_element));
                            self.history.push_front(record);
                            break;
                        }
                        isTooOld => cursor = index.checked_sub(1),
                        _ => unreachable!("matchAndAdd returned an unknown state"),
                    }
                }
            }

            // 超出 historySize 时丢掉最旧区间。
            while self.history.len() > historySize {
                self.history.pop_front();
            }
        }
    }

    /// 清空全部淘汰历史。
    pub fn Clear(&mut self) {
        self.history.clear();
    }

    /// 转为 Datum 行（最新区间在前），供信息模式表读取。
    pub fn ToEvictedCountDatum(&self) -> Vec<Vec<types::Datum>> {
        self.history
            .iter()
            .rev()
            .map(|element| element.toEvictedCountDatum())
            .collect()
    }

    /// 收集最新的 `historySize` 条历史淘汰元素，并按时间升序返回。
    pub fn collectHistorySummaries(
        &self,
        historySize: usize,
    ) -> Vec<&stmtSummaryByDigestEvictedElement> {
        self.history
            .iter()
            .skip(self.history.len().saturating_sub(historySize))
            .map(Box::as_ref)
            .collect()
    }
}

impl stmtSummaryByDigestEvictedElement {
    /// 将一条被淘汰摘要并入本窗口；key 为 Some 时递增 count 并合并统计。
    pub fn addEvicted(
        &mut self,
        digestKey: Option<&StmtDigestKey>,
        digestValue: Option<&stmtSummaryByDigestElement>,
    ) {
        if digestKey.is_some() {
            let digest_value = digestValue
                .expect("digest value must be present when an evicted digest key is provided");
            self.count += 1;
            addInfo(self.otherSummary.as_mut(), digest_value);
        }
    }

    /// 按时间边界判断匹配/过旧/过新，匹配则并入统计。
    pub fn matchAndAdd(
        &mut self,
        digestKey: Option<&StmtDigestKey>,
        digestValue: Option<&stmtSummaryByDigestElement>,
    ) -> i32 {
        let Some(digest_value) = digestValue else {
            return isTooYoung;
        };
        if self.beginTime <= digest_value.beginTime && digest_value.endTime <= self.endTime {
            self.addEvicted(digestKey, Some(digest_value));
            isMatch
        } else if digest_value.endTime <= self.beginTime {
            isTooOld
        } else {
            isTooYoung
        }
    }

    /// 输出 (begin, end, count) 三列 Datum。
    pub fn toEvictedCountDatum(&self) -> Vec<types::Datum> {
        vec![
            types::NewTimeDatum(mysql_timestamp(self.beginTime)),
            types::NewTimeDatum(mysql_timestamp(self.endTime)),
            types::NewIntDatum(self.count),
        ]
    }
}

/// `matchAndAdd`：时间窗口完全匹配。
pub const isMatch: i32 = 0;
/// `matchAndAdd`：待并入区间早于当前窗口。
pub const isTooOld: i32 = 1;
/// `matchAndAdd`：待并入区间晚于当前窗口（或值为空）。
pub const isTooYoung: i32 = 2;

impl stmtSummaryByDigestMap {
    /// 导出当前 map 上淘汰桶的计数 Datum 行。
    pub fn ToEvictedCountDatum(&self) -> Vec<Vec<types::Datum>> {
        self.other.ToEvictedCountDatum()
    }
}

/// Adds every statistic using the same sum/max/min/set/overwrite rule as Go.
/// 按 Go 相同规则合并两项摘要统计（求和 / 取极值 / 集合并 / 覆盖字段）。
pub fn addInfo(addTo: &mut stmtSummaryByDigestElement, addWith: &stmtSummaryByDigestElement) {
    let addTo = &mut addTo.stmtSummaryStats;
    let addWith = &addWith.stmtSummaryStats;
    for user in &addWith.authUsers {
        addTo.authUsers.insert(user.clone());
    }

    addTo.execCount += addWith.execCount;
    addTo.sumWarnings += addWith.sumWarnings;
    addTo.sumLatency += addWith.sumLatency;
    addTo.maxLatency = addTo.maxLatency.max(addWith.maxLatency);
    addTo.minLatency = addTo.minLatency.min(addWith.minLatency);
    addTo.sumParseLatency += addWith.sumParseLatency;
    addTo.maxParseLatency = addTo.maxParseLatency.max(addWith.maxParseLatency);
    addTo.sumCompileLatency += addWith.sumCompileLatency;
    addTo.maxCompileLatency = addTo.maxCompileLatency.max(addWith.maxCompileLatency);

    addTo.sumNumCopTasks += addWith.sumNumCopTasks;
    if addTo.maxCopProcessTime < addWith.maxCopProcessTime {
        addTo.maxCopProcessTime = addWith.maxCopProcessTime;
        addTo
            .maxCopProcessAddress
            .clone_from(&addWith.maxCopProcessAddress);
    }
    if addTo.maxCopWaitTime < addWith.maxCopWaitTime {
        addTo.maxCopWaitTime = addWith.maxCopWaitTime;
        addTo
            .maxCopWaitAddress
            .clone_from(&addWith.maxCopWaitAddress);
    }

    addTo.sumProcessTime += addWith.sumProcessTime;
    addTo.maxProcessTime = addTo.maxProcessTime.max(addWith.maxProcessTime);
    addTo.sumWaitTime += addWith.sumWaitTime;
    addTo.maxWaitTime = addTo.maxWaitTime.max(addWith.maxWaitTime);
    addTo.sumBackoffTime += addWith.sumBackoffTime;
    addTo.maxBackoffTime = addTo.maxBackoffTime.max(addWith.maxBackoffTime);
    addTo.sumTotalKeys += addWith.sumTotalKeys;
    addTo.maxTotalKeys = addTo.maxTotalKeys.max(addWith.maxTotalKeys);
    addTo.sumProcessedKeys += addWith.sumProcessedKeys;
    addTo.maxProcessedKeys = addTo.maxProcessedKeys.max(addWith.maxProcessedKeys);
    addTo.sumRocksdbDeleteSkippedCount += addWith.sumRocksdbDeleteSkippedCount;
    addTo.maxRocksdbDeleteSkippedCount = addTo
        .maxRocksdbDeleteSkippedCount
        .max(addWith.maxRocksdbDeleteSkippedCount);
    addTo.sumRocksdbKeySkippedCount += addWith.sumRocksdbKeySkippedCount;
    addTo.maxRocksdbKeySkippedCount = addTo
        .maxRocksdbKeySkippedCount
        .max(addWith.maxRocksdbKeySkippedCount);
    addTo.sumRocksdbBlockCacheHitCount += addWith.sumRocksdbBlockCacheHitCount;
    addTo.maxRocksdbBlockCacheHitCount = addTo
        .maxRocksdbBlockCacheHitCount
        .max(addWith.maxRocksdbBlockCacheHitCount);
    addTo.sumRocksdbBlockReadCount += addWith.sumRocksdbBlockReadCount;
    addTo.maxRocksdbBlockReadCount = addTo
        .maxRocksdbBlockReadCount
        .max(addWith.maxRocksdbBlockReadCount);
    addTo.sumRocksdbBlockReadByte += addWith.sumRocksdbBlockReadByte;
    addTo.maxRocksdbBlockReadByte = addTo
        .maxRocksdbBlockReadByte
        .max(addWith.maxRocksdbBlockReadByte);
    addTo.iaExecCount += addWith.iaExecCount;
    addTo.sumIARemoteReadSegmentCount += addWith.sumIARemoteReadSegmentCount;
    addTo.maxIARemoteReadSegmentCount = addTo
        .maxIARemoteReadSegmentCount
        .max(addWith.maxIARemoteReadSegmentCount);
    addTo.sumIARemoteReadSegmentSize += addWith.sumIARemoteReadSegmentSize;
    addTo.maxIARemoteReadSegmentSize = addTo
        .maxIARemoteReadSegmentSize
        .max(addWith.maxIARemoteReadSegmentSize);
    addTo.sumIARemoteReadSegmentWaitTime += addWith.sumIARemoteReadSegmentWaitTime;
    addTo.maxIARemoteReadSegmentWaitTime = addTo
        .maxIARemoteReadSegmentWaitTime
        .max(addWith.maxIARemoteReadSegmentWaitTime);

    addTo.commitCount += addWith.commitCount;
    addTo.sumPrewriteTime += addWith.sumPrewriteTime;
    addTo.maxPrewriteTime = addTo.maxPrewriteTime.max(addWith.maxPrewriteTime);
    addTo.sumCommitTime += addWith.sumCommitTime;
    addTo.maxCommitTime = addTo.maxCommitTime.max(addWith.maxCommitTime);
    addTo.sumGetCommitTsTime += addWith.sumGetCommitTsTime;
    addTo.maxGetCommitTsTime = addTo.maxGetCommitTsTime.max(addWith.maxGetCommitTsTime);
    addTo.sumCommitBackoffTime += addWith.sumCommitBackoffTime;
    addTo.maxCommitBackoffTime = addTo.maxCommitBackoffTime.max(addWith.maxCommitBackoffTime);
    addTo.sumResolveLockTime += addWith.sumResolveLockTime;
    addTo.maxResolveLockTime = addTo.maxResolveLockTime.max(addWith.maxResolveLockTime);
    addTo.sumLocalLatchTime += addWith.sumLocalLatchTime;
    addTo.maxLocalLatchTime = addTo.maxLocalLatchTime.max(addWith.maxLocalLatchTime);
    addTo.sumWriteKeys += addWith.sumWriteKeys;
    addTo.maxWriteKeys = addTo.maxWriteKeys.max(addWith.maxWriteKeys);
    addTo.sumWriteSize += addWith.sumWriteSize;
    addTo.maxWriteSize = addTo.maxWriteSize.max(addWith.maxWriteSize);
    addTo.sumPrewriteRegionNum += addWith.sumPrewriteRegionNum;
    addTo.maxPrewriteRegionNum = addTo.maxPrewriteRegionNum.max(addWith.maxPrewriteRegionNum);
    addTo.sumTxnRetry += addWith.sumTxnRetry;
    addTo.maxTxnRetry = addTo.maxTxnRetry.max(addWith.maxTxnRetry);
    addTo.sumBackoffTimes += addWith.sumBackoffTimes;
    for (kind, count) in &addWith.backoffTypes {
        *addTo.backoffTypes.entry(kind.clone()).or_default() += count;
    }

    addTo.planCacheHits += addWith.planCacheHits;
    addTo.sumAffectedRows += addWith.sumAffectedRows;
    addTo.sumMem += addWith.sumMem;
    addTo.maxMem = addTo.maxMem.max(addWith.maxMem);
    addTo.sumMemArbitration += addWith.sumMemArbitration;
    addTo.maxMemArbitration = addTo.maxMemArbitration.max(addWith.maxMemArbitration);
    addTo.sumDisk += addWith.sumDisk;
    addTo.maxDisk = addTo.maxDisk.max(addWith.maxDisk);
    addTo.firstSeen = addTo.firstSeen.min(addWith.firstSeen);
    addTo.lastSeen = addTo.lastSeen.max(addWith.lastSeen);
    addTo.execRetryCount += addWith.execRetryCount;
    addTo.execRetryTime += addWith.execRetryTime;
    addTo.sumKVTotal += addWith.sumKVTotal;
    addTo.sumPDTotal += addWith.sumPDTotal;
    addTo.sumBackoffTotal += addWith.sumBackoffTotal;
    addTo.sumWriteSQLRespTotal += addWith.sumWriteSQLRespTotal;
    addTo.sumTidbCPU += addWith.sumTidbCPU;
    addTo.sumTikvCPU += addWith.sumTikvCPU;
    addTo.sumErrors += addWith.sumErrors;
    addTo.StmtRUSummary.Merge(&addWith.StmtRUSummary);
    addTo
        .resourceGroupName
        .clone_from(&addWith.resourceGroupName);
}
