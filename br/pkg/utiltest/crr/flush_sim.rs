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

//! 日志备份 flush 模拟器：按 store 生成 log 文件与 tagged meta。
//! 对应 Go utiltest/crr FlushSim，驱动假 PD 的 region flush 与 checkpoint。
//! TS 范围由确定性 RNG 在 [globalCheckpoint, latestTS] 内抽取。
//! 同 store 串行锁保证并发 FlushStore 不交错写同一 store 元数据。
//! failpoint 注释位点保留与 Go 注入边界对齐，当前为空操作。
//! 空 log 仅占位路径与 TS 元数据，不写入真实 redo 内容。
//! meta 文件名编码 flush/store/min/max TS，供 ParseName 回读校验。
//! MinTs 取各 region 下界聚合；Metadata.MaxTs 使用 checkpointTS。
//! GlobalCheckpoint 作为随机下界，避免生成早于全局点的虚假范围。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_stream::GetStreamBackupMetaPrefix;
use astersql_br_pkg_stream::stubs::backuppb::{DataFileGroup, DataFileInfo, Metadata};
use astersql_br_pkg_stream_backupmetas::{
    NAME_MAX_TS_TAG, NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG, NAME_MIN_TS_TAG,
};

use crate::pd_sim::PDSim;
use crate::stubs::{Context, Error, Result, Storage};
use crate::types::{
    DeterministicRNG, FlushRecord, RegionState, TestContext, newDeterministicRNG, regionIDTag,
};

/// FlushSim simulates log backup file generation for region flushes.
/// 按 store 模拟一次 flush：写 log、写 meta、推进 PD checkpoint。
pub struct FlushSim {
    // 保护 stores 映射的创建。
    mu: Mutex<()>,
    // 假 PD：分配 TSO、查询 region、执行 flushStore。
    pd: Arc<PDSim>,
    // 测试种子，派生每次 flush 的确定性 RNG。
    seed: i64,
    // 上游存储：写入 log/meta 文件。
    storage: Arc<dyn Storage>,
    // 全局 flush 序号，写入记录与文件名后缀。
    seq: Mutex<u64>,
    // 按 Sequence 有序的 flush 历史。
    records: Mutex<Vec<FlushRecord>>,
    // 每 store 一把锁，串行化 FlushStore。
    stores: Mutex<HashMap<u64, Arc<Mutex<()>>>>,
}

// 单次 flush 产出的分组、路径与 TS 聚合。
struct RegionFiles {
    // 写入 Metadata.FileGroups 的分组。
    groups: Vec<DataFileGroup>,
    // 对应空内容 log 文件路径列表。
    log_paths: Vec<String>,
    // 参与 flush 的 region id。
    region_ids: Vec<u64>,
    // 各 region 最小 TS 的全局最小。
    min_ts: u64,
    // 各 region 最大 TS 的全局最大。
    max_ts: u64,
}

/// 用 TestContext 种子构造 FlushSim。
pub fn NewFlushSimWithTestContext(
    // 假 PD：分配 TSO、查询 region、执行 flushStore。
    pd: Arc<PDSim>,
    // 上游存储：写入 log/meta 文件。
    storage: Arc<dyn Storage>,
    tc: &TestContext,
) -> FlushSim {
    FlushSim {
        mu: Mutex::new(()),
        pd,
        seed: tc.Seed(),
        storage,
        seq: Mutex::new(0),
        records: Mutex::new(Vec::new()),
        stores: Mutex::new(HashMap::new()),
    }
}

// 拼带 TS/store 标签的 meta 文件名，字符标签对齐 backupmetas。
fn formatTaggedMetaName(
    flushTS: u64,
    storeID: u64,
    minDefaultTS: u64,
    minTS: u64,
    maxTS: u64,
    suffixToken: u64,
) -> String {
    format!(
        "{flushTS:016X}{storeID:016X}-{d}{minDefaultTS:016X}{l}{minTS:016X}{u}{maxTS:016X}{r}{suffixToken:016X}.meta",
        d = NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG as char,
        l = NAME_MIN_TS_TAG as char,
        u = NAME_MAX_TS_TAG as char,
        r = regionIDTag as char,
    )
}

// 在全局检查点与最新 TS 间随机取有序 [min,max]。
fn pickRegionTSRange(
    rng: &mut DeterministicRNG,
    globalCheckpoint: u64,
    latestTS: u64,
) -> (u64, u64) {
    let mut minTS = rng.Uint64InRange(globalCheckpoint, latestTS);
    let mut maxTS = rng.Uint64InRange(globalCheckpoint, latestTS);
    if maxTS < minTS {
        std::mem::swap(&mut minTS, &mut maxTS);
    }
    (minTS, maxTS)
}

impl FlushSim {
    // 为 store 上每个 region 写空 log 并组装 DataFileGroup。
    fn buildRegionFiles(
        &self,
        ctx: &Context,
        storeID: u64,
        flushSeq: u64,
        globalCheckpoint: u64,
        latestTS: u64,
        states: &[RegionState],
        rng: &mut DeterministicRNG,
    ) -> Result<RegionFiles> {
        let mut result = RegionFiles {
            groups: Vec::with_capacity(states.len()),
            log_paths: Vec::with_capacity(states.len()),
            region_ids: Vec::with_capacity(states.len()),
            min_ts: u64::MAX,
            max_ts: 0,
        };

        for state in states {
            let (rMinTS, rMaxTS) = pickRegionTSRange(rng, globalCheckpoint, latestTS);
            if rMinTS < result.min_ts {
                result.min_ts = rMinTS;
            }
            if rMaxTS > result.max_ts {
                result.max_ts = rMaxTS;
            }

            let logPath = format!(
                "v1/log/store-{storeID}/flush-{flushSeq:08}-region-{}.log",
                state.ID
            );
            self.storage
                .WriteFile(ctx, &logPath, &[])
                .map_err(|e| Error::new(format!("write log file {logPath}: {e}")))?;

            result.log_paths.push(logPath.clone());
            result.region_ids.push(state.ID);
            result.groups.push(DataFileGroup {
                Path: logPath,
                MinTs: rMinTS,
                MaxTs: rMaxTS,
                DataFilesInfo: vec![DataFileInfo {
                    MinTs: rMinTS,
                    MaxTs: rMaxTS,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        Ok(result)
    }

    // 懒创建并返回指定 store 的互斥锁。
    fn lockStore(&self, storeID: u64) -> Arc<Mutex<()>> {
        let _g = self.mu.lock().unwrap();
        let mut stores = self.stores.lock().unwrap();
        stores
            .entry(storeID)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    // 单调递增 flush 序号。
    fn nextFlushSequence(&self) -> u64 {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        *seq
    }

    // 按 seed+store+seq 派生确定性 RNG。
    fn flushRNG(&self, storeID: u64, flushSeq: u64) -> DeterministicRNG {
        newDeterministicRNG(
            self.seed,
            &format!("flush-sim-store-{storeID}-flush-{flushSeq}"),
        )
    }

    // 按 Sequence 插入有序 records，供 RecordsUpTo 查询。
    fn appendRecord(&self, record: FlushRecord) {
        let mut records = self.records.lock().unwrap();
        let mut insertAt = records.len();
        while insertAt > 0 && records[insertAt - 1].Sequence > record.Sequence {
            insertAt -= 1;
        }
        records.insert(insertAt, record);
    }

    // 序列化 Metadata 并写到 stream backup meta 前缀下。
    fn writeBackupMeta(
        &self,
        ctx: &Context,
        storeID: u64,
        flushSeq: u64,
        flushTS: u64,
        checkpointTS: u64,
        files: &RegionFiles,
    ) -> Result<String> {
        let metaPath = format!(
            "{}/{}",
            GetStreamBackupMetaPrefix(),
            formatTaggedMetaName(
                flushTS,
                storeID,
                files.min_ts,
                files.min_ts,
                files.max_ts,
                flushSeq
            )
        );
        let metadata = Metadata {
            StoreId: storeID as i64,
            MinTs: files.min_ts,
            MaxTs: checkpointTS,
            FileGroups: files.groups.clone(),
            ..Default::default()
        };
        let payload = metadata
            .Marshal()
            .map_err(|e| Error::new(format!("marshal backupmeta {metaPath}: {e}")))?;
        self.storage
            .WriteFile(ctx, &metaPath, &payload)
            .map_err(|e| Error::new(format!("write backupmeta {metaPath}: {e}")))?;
        Ok(metaPath)
    }

    // 通知 PDSim 完成该 store 的 region flush。
    fn flushRegions(&self, ctx: &Context, storeID: u64, checkpointTS: u64) -> Result<()> {
        self.pd
            .flushStore(ctx, storeID, checkpointTS)
            .map_err(|e| Error::new(format!("flush store {storeID}: {e}")))?;
        Ok(())
    }

    /// FlushStore simulates one store flush and emits one metadata file with all regions on that store.
    /// 模拟单 store flush：分配 TS、写文件、推进 PD、记录 FlushRecord。
    pub fn FlushStore(&self, ctx: &Context, storeID: u64) -> Result<FlushRecord> {
        // failpoint.InjectCall("begin-flush-store") — no-op boundary
        // 与 Go failpoint 边界对齐的空位点，便于后续注入。

        let store_mu = self.lockStore(storeID);
        // 持 store 锁，避免同 store 并发 flush 交错。
        let _guard = store_mu.lock().unwrap();

        let states = self.pd.RegionSnapshotsOnStore(storeID)?;
        // 无 region 则无法生成有效 meta。
        if states.is_empty() {
            return Err(Error::new(format!(
                "store {storeID} has no regions to flush"
            )));
        }

        let checkpointTS = self.pd.AllocTSO();
        // 先分配检查点 TS，再分配 flush TS，顺序对齐 Go。
        let flushTS = self.pd.AllocTSO();
        let latestTS = checkpointTS;
        let globalCheckpoint = self.pd.GlobalCheckpoint();

        let flushSeq = self.nextFlushSequence();
        // 序号同时进入文件名与 FlushRecord。
        let mut rng = self.flushRNG(storeID, flushSeq);
        let files = self.buildRegionFiles(
            ctx,
            storeID,
            flushSeq,
            globalCheckpoint,
            latestTS,
            &states,
            &mut rng,
        )?;

        // failpoint.InjectCall("before-write-flush-meta")
        // 写 meta 前注入点。
        let metaPath =
            self.writeBackupMeta(ctx, storeID, flushSeq, flushTS, checkpointTS, &files)?;
        // failpoint.InjectCall("after-write-flush-meta")
        // 写 meta 后注入点。

        self.flushRegions(ctx, storeID, checkpointTS)?;
        // failpoint.InjectCall("after-flush-regions")
        // PD flush 完成后注入点。

        // 汇总本轮产出供 harness 断言与 RecordsUpTo。
        let record = FlushRecord {
            Sequence: flushSeq,
            StoreID: storeID,
            RegionIDs: files.region_ids,
            CheckpointTS: checkpointTS,
            FlushTS: flushTS,
            MinTS: files.min_ts,
            MaxTS: files.max_ts,
            MetadataPath: metaPath,
            LogPaths: files.log_paths,
        };
        self.appendRecord(record.clone_record());
        // 有序插入后返回克隆，调用方持有独立副本。
        Ok(record.clone_record())
    }

    /// Records returns all flush records in creation order.
    /// 返回全部 flush 历史快照（深拷贝）。
    pub fn Records(&self) -> Vec<FlushRecord> {
        let records = self.records.lock().unwrap();
        records.iter().map(|r| r.clone_record()).collect()
    }

    /// RecordsUpTo returns flush records with CheckpointTS <= tso.
    /// 筛选检查点已不超过目标 TSO 的记录，供恢复断言。
    pub fn RecordsUpTo(&self, tso: u64) -> Vec<FlushRecord> {
        let records = self.records.lock().unwrap();
        records
            .iter()
            .filter(|r| r.CheckpointTS <= tso)
            .map(|r| r.clone_record())
            .collect()
    }
}
