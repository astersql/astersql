// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Schema backup logic matching `br/pkg/backup/schema.go`.
//!
//! 备份 schema 收集/校验/编码，对齐 Go `schema.go`。
//! 流程：iter 收集 → 工作池算 checksum/match/merge_option → 有序写 MetaWriter。
//! 系统库名会改写为临时库名；checkpoint 可跳过重复 checksum。
//! merge_option 通过 label rule 的 `merge_option=allow` 判定。
//! `*_for_test` 辅助把私有方法暴露给单测，不改变生产语义。
//! 进度回调 `updateCh` 每成功写出一个 schema 递增一次。
//! skipChecksum 为真时跳过计算与匹配，但仍编码元数据。
//! statsHandle 存在时才会 dump 统计；否则 StatsIndex 保持空。
//! 分区表 checksum 对分区做 xor/累加后再与表级字段比对。

// WorkerPool/wait_jobs 近似 Go errgroup；stubs 替代真实 TiDB 依赖。
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// MetaWriter/checksum/label 等均经 stubs，便于 darwin 编译。
use crate::stubs::backuppb::{self, Schema, StatsFileIndex};
use crate::stubs::berrors;
use crate::stubs::checkpoint::{CheckpointRunner, ChecksumItem};
use crate::stubs::checksum;
use crate::stubs::glue::Progress;
use crate::stubs::infosync;
use crate::stubs::kvutil;
use crate::stubs::label;
use crate::stubs::metautil::{self, ChecksumStats, MetaPayload, MetaWriter};
use crate::stubs::model::{DBInfo, TableInfo};
use crate::stubs::statistics;
use crate::stubs::summary;
use crate::stubs::utils;
use crate::stubs::{Codec, Context, Error, KvClient, Result, Storage, WorkerPool, wait_jobs};

/// DefaultSchemaConcurrency mirrors Go constant.
/// DefaultSchemaConcurrency mirrors Go constant.
/// 默认 schema 并发度，与 Go 常量同值。
pub const DefaultSchemaConcurrency: u64 = 64;

/// 单表（或空库）备份单元：元信息、checksum、统计与 merge 标志。
struct schemaInfo {
    /// 空库时为 None。
    tableInfo: Option<TableInfo>,
    /// 所属库；系统库可能已被改名。
    dbInfo: DBInfo,
    // 校验和异或值。
    crc64xor: u64,
    // 总 KV 数。
    totalKvs: u64,
    // 总字节数。
    totalBytes: u64,
    /// 可选统计 JSON（当前路径常走 StatsIndex）。
    stats: Option<statistics::JSONTable>,
    /// 统计文件索引。
    statsIndex: Vec<StatsFileIndex>,
    /// 表级是否允许 merge_option。
    isMergeOptionAllowed: bool,
    /// 分区名 → 是否允许 merge。
    partitionMergeOptionAllowed: HashMap<String, bool>,
}

/// Callback type for iterating DB/table pairs (table may be None for empty DB).
/// Callback type for iterating DB/table pairs (table may be None for empty DB).
/// 迭代回调类型：空库时 table 为 None。
pub type iterFuncTp = Arc<
    dyn Fn(&dyn Storage, &mut dyn FnMut(&DBInfo, Option<&TableInfo>)) -> Result<()> + Send + Sync,
>;

/// Schemas is the task for backing up schemas.
/// Schemas is the task for backing up schemas.
/// 持有迭代函数、规模估计与可选 checkpoint checksum。
pub struct Schemas {
    /// 遍历 DB/表的回调。
    iterFunc: iterFuncTp,
    /// 预估 schema 数量，供进度条。
    size: isize,
    /// 表 id → 检查点 checksum。
    checkpointChecksum: Option<HashMap<i64, ChecksumItem>>,
}

/// NewBackupSchemas mirrors Go constructor.
/// NewBackupSchemas mirrors Go constructor.
/// 构造时不带 checkpoint，后续可 SetCheckpointChecksum。
pub fn NewBackupSchemas(iterFunc: iterFuncTp, size: isize) -> Schemas {
    Schemas {
        iterFunc,
        size,
        checkpointChecksum: None,
    }
}

impl Schemas {
    /// 注入检查点 checksum，命中则跳过重新计算。
    pub fn SetCheckpointChecksum(&mut self, checkpointChecksum: HashMap<i64, ChecksumItem>) {
        self.checkpointChecksum = Some(checkpointChecksum);
    }

    /// 先收集再并行处理；写 Meta 仍按迭代序串行，保证测试确定性。
    /// BackupSchemas mirrors Go `Schemas.BackupSchemas`.
    ///
    /// Work items are collected first (matching Go's iter-then-errgroup shape), then executed
    /// through a bounded worker pool. MetaWriter sends happen after each task completes and are
    /// serialized to keep side-effects deterministic for tests.
    pub fn BackupSchemas(
        &self,
        ctx: &Context,
        metaWriter: &dyn MetaWriter,
        checkpointRunner: Option<&CheckpointRunner>,
        store: &dyn Storage,
        statsHandle: Option<&dyn statistics::Handle>,
        backupTS: u64,
        checksumMap: Option<&HashMap<i64, ChecksumStats>>,
        concurrency: u32,
        copConcurrency: u32,
        skipChecksum: bool,
        updateCh: Option<&dyn Progress>,
    ) -> Result<()> {
        // 汇总 checksum 阶段耗时。
        let startAll = Instant::now();
        // 追加 schema 元数据操作类型。
        let op = metautil::AppendSchema;
        // 异步开启 meta 写会话，结束时 FinishWriteMetas。
        metaWriter.StartWriteMetasAsync(ctx, op);

        // 迭代阶段只组装 schemaInfo，不在此做重计算。
        // Collect schema units under the iterator (Go schedules into errgroup inside the callback).
        // units 顺序即后续 MetaWriter 发送顺序的基准。
        let mut units: Vec<schemaInfo> = Vec::new();
        (self.iterFunc)(store, &mut |dbInfo, tableInfo| {
            let mut schema = schemaInfo {
                tableInfo: tableInfo.cloned(),
                dbInfo: dbInfo.clone(),
                crc64xor: 0,
                totalKvs: 0,
                totalBytes: 0,
                stats: None,
                statsIndex: Vec::new(),
                isMergeOptionAllowed: false,
                partitionMergeOptionAllowed: HashMap::new(),
            };
            // 系统库改写临时名，避免与在线库冲突。
            if utils::IsSysDB(&schema.dbInfo.Name.L) {
                schema.dbInfo.Name = utils::TemporaryDBName(&schema.dbInfo.Name.O);
            }
            units.push(schema);
        })?;

        // 保留与 Go 相同的并发参数形状（占位池随即被真正 pool 取代）。
        let _ = (concurrency, WorkerPool::new(concurrency, "Schemas"));
        // 并行算 checksum；随后按 idx 排序再 Send。
        // Process units with Go-equivalent per-schema steps. Concurrency is approximated by
        // computing checksums eagerly; metaWriter.Send remains ordered by iteration.
        let mut jobs = Vec::new();
        let computed: Arc<Mutex<Vec<(usize, Result<schemaInfo>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        // 真正用于 ApplyOnErrorGroup 的工作池。
        let pool = WorkerPool::new(concurrency, "Schemas");

        for (idx, mut schema) in units.into_iter().enumerate() {
            let ctx2 = ctx.clone();
            let client = store.GetClient();
            let codec = store.GetCodec();
            let checksum_map_owned = checksumMap.cloned();
            let checkpoint_map = self.checkpointChecksum.clone();
            let computed2 = computed.clone();

            // 每表任务：checkpoint/checksum → match → merge_option。
            pool.ApplyOnErrorGroup(&mut jobs, move || {
                let res = (|| -> Result<schemaInfo> {
                    // 空库只有 dbInfo，跳过 checksum/merge 查询。
                    if schema.tableInfo.is_some() {
                        let table_id = schema.tableInfo.as_ref().unwrap().ID;
                        if !skipChecksum {
                            // 命中检查点则直接填入三字段。
                            let mut from_ckpt = false;
                            if let Some(map) = &checkpoint_map {
                                if let Some(c) = map.get(&table_id) {
                                    schema.crc64xor = c.Crc64xor;
                                    schema.totalKvs = c.TotalKvs;
                                    schema.totalBytes = c.TotalBytes;
                                    from_ckpt = true;
                                }
                            }
                            // 无检查点则现场计算 checksum。
                            if !from_ckpt {
                                schema.calculateChecksum(
                                    &ctx2,
                                    client.as_ref(),
                                    backupTS,
                                    copConcurrency,
                                )?;
                            }
                            // 外部 checksumMap 存在时做一致性校验。
                            if let Some(map) = &checksum_map_owned {
                                schema.matchChecksum(map)?;
                            }
                        }
                        // merge_option 查询失败时保持默认 false，不中断备份。
                        match schema.checkMergeOptionAllowed(&ctx2, codec.as_ref()) {
                            Ok((allowed, part)) => {
                                schema.isMergeOptionAllowed = allowed;
                                schema.partitionMergeOptionAllowed = part;
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(schema)
                })();
                // 任务错误记入 computed，同时让 error group 感知失败。
                let ok = res.is_ok();
                computed2.lock().unwrap().push((idx, res));
                if ok {
                    Ok(())
                } else {
                    Err(Error::new("schema task failed"))
                }
            });
        }

        // 先等全部任务，再按序写 meta；错误延后返回。
        let wait_err = wait_jobs(jobs);
        let mut finished = computed.lock().unwrap().drain(..).collect::<Vec<_>>();
        // 按收集序号排序，保证 MetaWriter 顺序稳定。
        finished.sort_by_key(|(i, _)| *i);

        for (_idx, item) in finished {
            let mut schema = match item {
                Ok(s) => s,
                Err(e) => return Err(Error::Trace(e)),
            };
            // 写 meta 前：可选 flush checkpoint 与 dump stats。
            if let Some(table) = schema.tableInfo.as_ref() {
                if !skipChecksum {
                    let table_id = table.ID;
                    let from_ckpt = self
                        .checkpointChecksum
                        .as_ref()
                        .and_then(|m| m.get(&table_id))
                        .is_some();
                    if !from_ckpt {
                        // 非检查点路径才 FlushChecksum。
                        if let Some(runner) = checkpointRunner {
                            runner.FlushChecksum(
                                ctx,
                                table_id,
                                schema.crc64xor,
                                schema.totalKvs,
                                schema.totalBytes,
                            )?;
                        }
                    }
                }
                // 有统计句柄才导出 JSON/索引。
                if let Some(handle) = statsHandle {
                    // The writer owns stats-file policy (inline data, external files, encryption,
                    // and naming), so obtain the child writer from it just like Go does.
                    let statsWriter = metaWriter.NewStatsWriter();
                    schema.dumpStatsToJSON(ctx, &statsWriter, handle, backupTS)?;
                }
            }
            // 编码为 backuppb.Schema 后发送。
            let s = schema.encodeToSchema()?;
            metaWriter.Send(MetaPayload::Schema(s), op)?;
            // 进度推进与 Go updateCh.Inc 对齐。
            if let Some(ch) = updateCh {
                ch.Inc();
            }
        }

        // 全部写出后再表面工作池错误，避免半截 meta。
        if let Err(e) = wait_err {
            return Err(Error::Trace(e));
        }

        // 记录 checksum 阶段耗时到 summary。
        summary::CollectDuration("backup checksum", startAll.elapsed());
        // 关闭 AppendSchema 会话。
        metaWriter.FinishWriteMetas(ctx, op)
    }

    /// 返回预估 schema 数量。
    pub fn Len(&self) -> isize {
        self.size
    }
}

impl schemaInfo {
    /// 对表执行 checksum executor，写入 crc/kvs/bytes。
    fn calculateChecksum(
        &mut self,
        ctx: &Context,
        client: &dyn KvClient,
        backupTS: u64,
        concurrency: u32,
    ) -> Result<()> {
        let table = self
            .tableInfo
            .as_ref()
            .ok_or_else(|| Error::new("nil tableInfo"))?
            .clone();
        // BR 显式 source type，避免与其它请求混计。
        let exe = checksum::NewExecutorBuilder(table, backupTS)
            .SetExplicitRequestSourceType(kvutil::ExplicitTypeBR)
            .SetConcurrency(concurrency)
            .Build()?;
        // 进度闭包为空：schema 阶段不单独汇报行级进度。
        let checksumResp = exe.Execute(ctx, client, || {})?;
        self.crc64xor = checksumResp.Checksum;
        self.totalKvs = checksumResp.TotalKvs;
        self.totalBytes = checksumResp.TotalBytes;
        Ok(())
    }

    /// 与外部 checksumMap（含分区）比对，不一致则 ErrBackupChecksumMismatch。
    fn matchChecksum(&self, checksumMap: &HashMap<i64, ChecksumStats>) -> Result<()> {
        let table = self
            .tableInfo
            .as_ref()
            .ok_or_else(|| Error::new("nil tableInfo"))?;
        // 缺省 0：map 无条目时要求本地也为 0。
        let mut crc = 0u64;
        let mut kvs = 0u64;
        let mut bytes = 0u64;
        if let Some(ckm) = checksumMap.get(&table.ID) {
            crc = ckm.Crc64Xor;
            kvs = ckm.TotalKvs;
            bytes = ckm.TotalBytes;
        }
        // 分区定义参与聚合或分区级 rule 检查。
        if let Some(part) = &table.Partition {
            for def in &part.Definitions {
                if let Some(ckm) = checksumMap.get(&def.ID) {
                    crc ^= ckm.Crc64Xor;
                    kvs += ckm.TotalKvs;
                    bytes += ckm.TotalBytes;
                }
            }
        }
        // 三字段任一不匹配即失败。
        if self.crc64xor != crc || self.totalKvs != kvs || self.totalBytes != bytes {
            return Err(Error::Trace(berrors::ErrBackupChecksumMismatch()));
        }
        Ok(())
    }

    /// 经 StatsWriter 持久化快照统计，并取出 StatsIndex。
    fn dumpStatsToJSON(
        &mut self,
        ctx: &Context,
        statsWriter: &metautil::StatsWriter,
        statsHandle: &dyn statistics::Handle,
        backupTS: u64,
    ) -> Result<()> {
        let table = self
            .tableInfo
            .as_ref()
            .ok_or_else(|| Error::new("nil tableInfo"))?
            .clone();
        let db_name = self.dbInfo.Name.O.clone();
        // 回调把统计交给 StatsWriter.BackupStats。
        statsHandle.PersistStatsBySnapshot(ctx, &db_name, &table, backupTS, &|db, t, stats| {
            statsWriter.BackupStats(db, t, stats)
        })?;
        self.statsIndex = statsWriter.BackupStatsDone(ctx)?;
        Ok(())
    }

    /// JSON 序列化 db/table/stats，填入 backuppb.Schema。
    fn encodeToSchema(&self) -> Result<Schema> {
        // 序列化失败包装为 Trace 错误。
        let dbBytes = serde_json::to_vec(&self.dbInfo)
            .map_err(|e| Error::Trace(Error::new(e.to_string())))?;
        // 空库：Table 字节为空切片。
        let tableBytes = if let Some(t) = &self.tableInfo {
            serde_json::to_vec(t).map_err(|e| Error::Trace(Error::new(e.to_string())))?
        } else {
            Vec::new()
        };
        let statsBytes = if let Some(s) = &self.stats {
            serde_json::to_vec(s).map_err(|e| Error::Trace(Error::new(e.to_string())))?
        } else {
            Vec::new()
        };
        Ok(Schema {
            Db: dbBytes,
            Table: tableBytes,
            Crc64Xor: self.crc64xor,
            TotalKvs: self.totalKvs,
            TotalBytes: self.totalBytes,
            Stats: statsBytes,
            StatsIndex: self.statsIndex.clone(),
            IsMergeOptionAllowed: self.isMergeOptionAllowed,
            PartitionMergeOptionAllowed: self.partitionMergeOptionAllowed.clone(),
        })
    }

    /// 批量拉 label rules，检查表/分区是否标注 merge_option=allow。
    fn checkMergeOptionAllowed(
        &self,
        ctx: &Context,
        codec: &dyn Codec,
    ) -> Result<(bool, HashMap<String, bool>)> {
        let table = self
            .tableInfo
            .as_ref()
            .ok_or_else(|| Error::new("nil tableInfo"))?;
        // 默认空 map：未标注的分区视为不允许。
        let mut partitionMergeOptionAllowed = HashMap::new();
        let dbName = &self.dbInfo.Name.L;
        let tableName = &table.Name.L;
        // 表级 rule id；分区再追加带分区名的 id。
        let ruleID = label::NewRuleID(codec, dbName, tableName, "");

        // 先表后分区，批量拉取。
        let mut ruleIDs = vec![ruleID.clone()];
        // 分区定义参与聚合或分区级 rule 检查。
        if let Some(part) = &table.Partition {
            for def in &part.Definitions {
                ruleIDs.push(label::NewRuleID(codec, dbName, tableName, &def.Name.L));
            }
        }

        let mut rules = HashMap::new();
        // 分批查询，避免单次请求过大。
        for batch in ruleIDs.chunks(utils::LabelRuleBatchSize) {
            let batchRules = infosync::GetLabelRules(ctx, batch)?;
            rules.extend(batchRules);
        }

        // 默认不允许 merge，需显式 allow 标签。
        let mut tableMergeOptionAllowed = false;
        if let Some(rule) = rules.get(&ruleID) {
            for lab in &rule.Labels {
                // 仅识别 allow；其它值视为不允许。
                if lab.Key == "merge_option" && lab.Value == "allow" {
                    tableMergeOptionAllowed = true;
                    break;
                }
            }
        }

        if let Some(part) = &table.Partition {
            for def in &part.Definitions {
                let partitionRuleID = label::NewRuleID(codec, dbName, tableName, &def.Name.L);
                if let Some(rule) = rules.get(&partitionRuleID) {
                    for lab in &rule.Labels {
                        if lab.Key == "merge_option" && lab.Value == "allow" {
                            partitionMergeOptionAllowed.insert(def.Name.O.clone(), true);
                            break;
                        }
                    }
                }
            }
        }

        Ok((tableMergeOptionAllowed, partitionMergeOptionAllowed))
    }
}

/// 测试入口：构造 schemaInfo 后调用 matchChecksum。
pub fn match_checksum_for_test(
    table: &TableInfo,
    db: &DBInfo,
    // 校验和异或值。
    crc64xor: u64,
    // 总 KV 数。
    totalKvs: u64,
    // 总字节数。
    totalBytes: u64,
    checksumMap: &HashMap<i64, ChecksumStats>,
) -> Result<()> {
    let s = schemaInfo {
        tableInfo: Some(table.clone()),
        dbInfo: db.clone(),
        crc64xor,
        totalKvs,
        totalBytes,
        stats: None,
        statsIndex: Vec::new(),
        isMergeOptionAllowed: false,
        partitionMergeOptionAllowed: HashMap::new(),
    };
    s.matchChecksum(checksumMap)
}

/// 测试入口：编码 schema，便于直接断言 protobuf 字段。
pub fn encode_schema_for_test(
    table: Option<&TableInfo>,
    db: &DBInfo,
    // 校验和异或值。
    crc64xor: u64,
    // 总 KV 数。
    totalKvs: u64,
    // 总字节数。
    totalBytes: u64,
) -> Result<backuppb::Schema> {
    let s = schemaInfo {
        tableInfo: table.cloned(),
        dbInfo: db.clone(),
        crc64xor,
        totalKvs,
        totalBytes,
        stats: None,
        statsIndex: Vec::new(),
        isMergeOptionAllowed: false,
        partitionMergeOptionAllowed: HashMap::new(),
    };
    s.encodeToSchema()
}

/// 测试入口：暴露 checkMergeOptionAllowed。
pub fn check_merge_option_allowed_for_test(
    ctx: &Context,
    table: &TableInfo,
    db: &DBInfo,
    codec: &dyn Codec,
) -> Result<(bool, HashMap<String, bool>)> {
    let s = schemaInfo {
        tableInfo: Some(table.clone()),
        dbInfo: db.clone(),
        crc64xor: 0,
        totalKvs: 0,
        totalBytes: 0,
        stats: None,
        statsIndex: Vec::new(),
        isMergeOptionAllowed: false,
        partitionMergeOptionAllowed: HashMap::new(),
    };
    s.checkMergeOptionAllowed(ctx, codec)
}
