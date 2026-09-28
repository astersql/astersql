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

//! Checksum table operators — mirrors `br/pkg/task/operator/checksum_table.go`.
//! 提供三类表级 checksum 入口：rewrite-rules（对照备份 meta）、upstream（仅当前库）、
//! PiTR id-map（上下游 ID 改写后对照）。公共流程：建 ConnMgr/Domain → 筛表 →
//! 生成 checksum 请求 → 挂 GC safepoint → 线程池执行 → JSON 打印结果。
//! 执行期间必须维持 service safepoint，结束时 TTL=0 删除，避免长期钉住 GC。
//! 双层哈希表加速旧表查找。
//! 分区与表级改写共用同一路由表。
//! 默认安全点存活时间与全局默认一致。
//! 生成唯一安全点标识。
//! 原子进度计数仅用于日志。
//! 互斥锁保护结果向量跨线程追加。
//! 测试表构造支持可选分区定义。
//! 按库名建路由，缺库即硬错误。
//! 缺表映射或缺标识映射均返回明确错误。
//! 兼容模式恢复标识固定为零。
//! 会话用完必须关闭，避免泄漏。
//! 受限查询占位符风格与远端一致。
//! 反序列化分片拼接后的完整元数据。
//! 响应字段大小写差异已在赋值处对齐。
//! 改写路径校验时间取自当前时间戳。
//! 过滤使用小写名匹配。
//! 连接管理器与域成对初始化。
//! 完整流水线顺序不可颠倒。
//! 上游入口忽略备份存储元数据。
//! 时间点恢复入口依赖上游集群标识。
//! 子请求总数用于进度分母。
//! 存活时间为零时删除服务安全点。
//! 路由键区分大小写敏感的原始名。
//! 无分区时分区信息为空。
//! 名称构造支持大小写不敏感比较。
//! 序列化特征便于跨语言消费结果。
//! 相等特征便于单测直接比较结构。
//! 共享指针克隆存储供各线程取客户端。
//! 首个错误优先于后续错误返回。
//! 跳过库表时只打印日志，不计入结果。
//! 从元数据取出库映射后再转为向量。
//! 配置克隆避免移动后无法再读字段。
//! 初始化失败不得留下半开连接。
//! 拉取表失败时注解库名。
//! 工作线程崩溃映射为统一错误字符串。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::config::{
    ChecksumUpstreamConfig, ChecksumWithPitrIdMapConfig, ChecksumWithRewriteRulesConfig,
};
use crate::stubs::{
    BRServiceSafePoint, BackupMeta, CIStr, ChecksumExecutor, ComposeTS, ConnMgr, DIAL_HOOKS,
    DefaultBRGCSafePointTTL, Domain, Error, ExecutorBuilder, GetStorage, GetTSWithRetry, Glue,
    MakeSafePointID, MetaReader, MetaTable, NewMgr, PitrDBMap, PitrIDMapsFilename, Result,
    StartServiceSafePointKeeper, TableInfo,
};

/// 单次 checksum 运行上下文：配置、连接、Domain 与最终 checksumTS。
struct checksumTableCtx {
    cfg: ChecksumWithRewriteRulesConfig,
    mgr: Option<Arc<ConnMgr>>,
    dom: Option<Arc<dyn Domain>>,
    /// 实际用于 checksum 的 TSO；由 gen*Requests 写入。
    checksumTS: u64,
}

/// 当前集群中一张已通过 TableFilter 的表及其库名。
#[derive(Clone, Debug)]
// 序列化失败包装为通用错误。
struct tableInDB {
    info: TableInfo,
    dbName: String,
}

/// rewrite-rules 路径：对照备份 meta 中的旧表 ID 做 checksum。
/// 步骤：init → getTables → loadOldTableIDs → genRequests → runChecksum → JSON stdout。
pub fn RunChecksumTable(g: &dyn Glue, cfg: ChecksumWithRewriteRulesConfig) -> Result<()> {
    let mut c = checksumTableCtx {
        cfg,
        mgr: None,
        dom: None,
        checksumTS: 0,
    };
    c.init(g)?;
    let curr = c.getTables()?;
    let old = c.loadOldTableIDs()?;
    let reqs = c.genRequests(&old, &curr)?;
    let results = c.runChecksum(reqs)?;
    // stderr 人类可读摘要；stdout 留给脚本消费的 JSON。
    for result in &results {
        eprintln!(
            "Checksum result db={} table={} checksum={} total_bytes={} total_kvs={}",
            result.DBName, result.TableName, result.Checksum, result.TotalBytes, result.TotalKVs
        );
    }
    println!(
        "{}",
        serde_json::to_string(&results).map_err(|e| Error::new(e.to_string()))?
    );
    Ok(())
}

/// upstream 路径：不加载备份 meta，直接对当前表在 RestoreTS 下 checksum。
pub fn RunUpstreamChecksumTable(g: &dyn Glue, cfg: ChecksumUpstreamConfig) -> Result<()> {
    // 复用 rewrite 上下文结构，仅取 RestoreConfig.Config 作为公共配置。
    let mut c = checksumTableCtx {
        cfg: ChecksumWithRewriteRulesConfig {
            Config: cfg.RestoreConfig.Config.clone(),
        },
        mgr: None,
        dom: None,
        checksumTS: 0,
    };
    c.init(g)?;
    let curr = c.getTables()?;
    let reqs = c.genUpstreamRequests(&curr, cfg.RestoreConfig.RestoreTS)?;
    let results = c.runChecksum(reqs)?;
    for result in &results {
        eprintln!(
            "Checksum result db={} table={} checksum={} total_bytes={} total_kvs={}",
            result.DBName, result.TableName, result.Checksum, result.TotalBytes, result.TotalKVs
        );
    }
    println!(
        "{}",
        serde_json::to_string(&results).map_err(|e| Error::new(e.to_string()))?
    );
    Ok(())
}

// 元数据解析失败要带文件路径上下文。
impl checksumTableCtx {
    /// 创建 ConnMgr（可经 DIAL_HOOKS 注入）并取得 Domain。
    fn init(&mut self, g: &dyn Glue) -> Result<()> {
        let cfg = &self.cfg;
        let mgr = {
            let hooks = DIAL_HOOKS.lock().unwrap();
            if let Some(hook) = hooks.new_mgr.as_ref() {
                hook(g, &cfg.Config)?
            } else {
                NewMgr(g, &cfg.Config, None, None)?
            }
        };
        let dom = g.GetDomain(mgr.GetStorage().as_ref())?;
        self.mgr = Some(mgr);
        self.dom = Some(dom);
        Ok(())
    }

    /// 按 TableFilter 从 infoschema 收集当前表；库级/表级过滤与 Go 相同。
    fn getTables(&self) -> Result<Vec<tableInDB>> {
        let sch = self.dom.as_ref().unwrap().InfoSchema();
        let dbs = sch.AllSchemas();
        let mut res = Vec::new();
        // 忙等取槽是简化实现，语义等同信号量。
        for db in dbs {
            // 带重试取时用于未指定校验时间的情况。
            if !self.cfg.Config.TableFilter.MatchSchema(&db.Name.L) {
                continue;
            }
            let tbls = sch.SchemaTableInfos(&db.Name.L).map_err(|err| {
                Error::Annotatef(err.msg, format!("failed to load data for db {}", db.Name.L))
            })?;
            // 伪造旧表先克隆再改标识，避免污染当前信息。
            for tbl in tbls {
                // 切换恢复标识时刷新拼接缓冲。
                if !self
                    .cfg
                    .Config
                    .TableFilter
                    .MatchTable(&db.Name.L, &tbl.Name.L)
                {
                    continue;
                }
                eprintln!(
                    "Added table from cluster. db={} table={}",
                    db.Name.L, tbl.Name.L
                );
                res.push(tableInDB {
                    info: tbl,
                    dbName: db.Name.L.clone(),
                });
            }
        }
        Ok(res)
    }

    /// 从备份 storage 读 backupmeta/schema，筛出与 filter 匹配的旧表。
    fn loadOldTableIDs(&self) -> Result<Vec<MetaTable>> {
        let (_, strg) = GetStorage(&self.cfg.Config.Storage, &self.cfg.Config)
            .map_err(|err| Error::Annotate(err.msg, "failed to create storage"))?;
        let mPath = crate::stubs::MetaFile;
        let metaContent = strg
            .ReadFile(mPath)
            .map_err(|err| Error::Annotatef(err.msg, format!("failed to open metafile {mPath}")))?;
        let backupMeta = BackupMeta::Unmarshal(&metaContent)
            .map_err(|err| Error::Annotate(err.msg, "failed to parse backupmeta"))?;
        let metaReader = MetaReader::new(backupMeta, strg);
        let tables = metaReader
            .ReadSchemasFiles()
            .map_err(|err| Error::Annotate(err.msg, "failed to read schema files"))?;
        let mut res = Vec::new();
        // 运行结束前必须清理安全点守护。
        for tbl in tables {
            // 结果字段命名对齐远端序列化标签。
            if !self
                .cfg
                .Config
                .TableFilter
                .MatchTable(&tbl.DB.Name.L, &tbl.Info.Name.L)
            {
                continue;
            }
            eprintln!(
                "Added table from backup data. db={} table={}",
                tbl.DB.Name.L, tbl.Info.Name.L
            );
            res.push(tbl);
        }
        Ok(res)
    }

    /// 加载 PiTR ID map：优先外部存储文件，否则查 `mysql.tidb_pitr_id_map`。
    /// 系统表路径需兼容有/无 `restore_id` 列；分片必须连续且非空。
    fn loadPitrIdMap(
        &self,
        g: &dyn Glue,
        restoredTS: u64,
        clusterID: u64,
    ) -> Result<Vec<PitrDBMap>> {
        // 外部存储模式：文件名由 clusterID+restoredTS 决定。
        if !self.cfg.Config.Storage.is_empty() {
            let (_, stg) = GetStorage(&self.cfg.Config.Storage, &self.cfg.Config)
                .map_err(|err| Error::Annotate(err.msg, "failed to create storage"))?;
            let metaFileName = PitrIDMapsFilename(clusterID, restoredTS);
            eprintln!("get pitr id map from the external storage file name={metaFileName}");
            let metaData = stg
                .ReadFile(&metaFileName)
                .map_err(|err| Error::Annotate(err.msg, "failed to load pitr id map file"))?;
            let backupMeta = BackupMeta::Unmarshal(&metaData)
                .map_err(|err| Error::Annotate(err.msg, "failed to unmarshal pitr id map file"))?;
            return Ok(backupMeta.GetDbMaps().to_vec());
        }
        eprintln!("get pitr id map from table restored-ts={restoredTS}");
        let table = self
            .dom
            .as_ref()
            .unwrap()
            .InfoSchema()
            .TableByName("mysql", "tidb_pitr_id_map")
            .map_err(|err| Error::Annotate(err.msg, "failed to get the table"))?;
        // 新版本表含 restore_id；旧版本无该列，走兼容 SQL。
        let hasRestoreIDColumn = table
            .Meta()
            .Columns
            .iter()
            .any(|col| col.Name.L == "restore_id");
        let getPitrIDMapSQL = if hasRestoreIDColumn {
            "SELECT restore_id, segment_id, id_map FROM mysql.tidb_pitr_id_map WHERE restored_ts = %? and upstream_cluster_id = %? ORDER BY restore_id, segment_id;"
        } else {
            eprintln!(
                "mysql.tidb_pitr_id_map table does not have restore_id column, using backward compatible mode"
            );
            "SELECT segment_id, id_map FROM mysql.tidb_pitr_id_map WHERE restored_ts = %? and upstream_cluster_id = %? ORDER BY segment_id;"
        };
        let se = g
            .CreateSession(self.mgr.as_ref().unwrap().GetStorage().as_ref())
            .map_err(|err| Error::Annotate(err.msg, "failed to create session"))?;
        let rows = se
            .ExecRestrictedSQL(getPitrIDMapSQL, &[restoredTS, clusterID])
            .map_err(|err| {
                Error::Annotate(
                    err.msg,
                    "failed to get pitr id map from mysql.tidb_pitr_id_map",
                )
            })?;
        se.Close();

        // 按 restore_id 分组拼接分片字节，再 Unmarshal 为 BackupMeta.DbMaps。
        let mut pitrDBMap = Vec::new();
        let mut metaData = Vec::new();
        let mut lastRestoreID = 0u64;
        let mut nextSegmentID = 0u64;
        // 请求源类型固定为备份恢复以便观测打标。
        for row in rows {
            let (restoreID, elementID, data) = if hasRestoreIDColumn {
                (row.GetUint64(0), row.GetUint64(1), row.GetBytes(2))
            } else {
                // 兼容模式把 restore_id 固定为 0。
                (0, row.GetUint64(0), row.GetBytes(1))
            };
            if lastRestoreID != restoreID {
                // 切换 restore_id 时先落盘上一组已拼接的 meta。
                if !metaData.is_empty() {
                    let backupMeta = BackupMeta::Unmarshal(&metaData).map_err(Error::Trace)?;
                    pitrDBMap.extend(backupMeta.DbMaps);
                    metaData.clear();
                }
                lastRestoreID = restoreID;
                nextSegmentID = 0;
            }
            // 分片必须从 0 连续递增，缺片直接失败。
            if nextSegmentID != elementID {
                // 执行器构建器并发度取自配置。
                return Err(Error::Errorf(format!(
                    "the part(segment_id = {nextSegmentID}) of pitr id map is lost"
                )));
            }
            if data.is_empty() {
                // 元数据阅读器读取模式文件列表。
                return Err(Error::Errorf(format!(
                    "get the empty part(segment_id = {nextSegmentID}) of pitr id map"
                )));
            }
            metaData.extend_from_slice(&data);
            nextSegmentID += 1;
        }
        // 合并物理与逻辑时钟得到六十四位时间戳。
        if !metaData.is_empty() {
            let backupMeta = BackupMeta::Unmarshal(&metaData).map_err(Error::Trace)?;
            pitrDBMap.extend(backupMeta.DbMaps);
        }
        Ok(pitrDBMap)
    }

    /// rewrite-rules：用 PD TSO 作 checksumTS，按库表名匹配旧 MetaTable。
    /// 备份中缺失的库/表跳过并打日志，不整批失败。
    fn genRequests(&mut self, bkup: &[MetaTable], curr: &[tableInDB]) -> Result<Vec<request>> {
        let (phy, logi) = self
            .mgr
            .as_ref()
            .unwrap()
            .GetPDClient()
            .GetTS()
            .map_err(|err| Error::Annotate(err.msg, "failed to get TSO for checksumming"))?;
        let tso = ComposeTS(phy, logi);
        self.checksumTS = tso;

        // db → table → MetaTable，加速当前表查找旧 schema。
        let mut bkupTbls: HashMap<String, HashMap<String, MetaTable>> = HashMap::new();
        // 测试辅助跳过集群初始化，只验标识改写。
        for t in bkup {
            bkupTbls
                .entry(t.DB.Name.L.clone())
                .or_default()
                .insert(t.Info.Name.L.clone(), t.clone());
        }

        let mut reqs = Vec::new();
        // 校验时间为零时向调度器取当前时间戳。
        for t in curr {
            let mut rb = ExecutorBuilder::new(t.info.clone(), tso);
            rb.SetConcurrency(self.cfg.Config.ChecksumConcurrency);
            let Some(oldDB) = bkupTbls.get(&t.dbName) else {
                eprintln!("db not found, will skip db={}", t.dbName);
                continue;
            };
            let Some(oldTable) = oldDB.get(&t.info.Name.L) else {
                eprintln!(
                    "table not found, will skip db={} table={}",
                    t.dbName, t.info.Name.L
                );
                continue;
            };
            // SetOldTable 启用 rewrite：按旧 ID 读数据、在新表上聚合。
            rb.SetOldTable(oldTable);
            rb.SetExplicitRequestSourceType("br");
            let req = rb.Build().map_err(|err| {
                Error::Annotatef(
                    err.msg,
                    format!(
                        "failed to build checksum builder for table {}.{}",
                        t.dbName, t.info.Name.L
                    ),
                )
            })?;
            reqs.push(request {
                copReq: req,
                dbName: t.dbName.clone(),
                tableName: t.info.Name.L.clone(),
            });
        }
        Ok(reqs)
    }

    /// PiTR：根据 id-map 构造假旧表（上游 ID），使 checksum 走 rewrite 对照。
    /// 库/表名用 Name.O（原始大小写）索引 map，与 Go 一致。
    pub fn genRequestsWithIDMap(
        &mut self,
        curr: &[tableInDB],
        idmaps: &[PitrDBMap],
        checksumTS: u64,
    ) -> Result<Vec<request>> {
        self.checksumTS = checksumTS;
        // router[db][table][downstreamId] = upstreamId（含分区）。
        let mut router: HashMap<String, HashMap<String, HashMap<i64, i64>>> = HashMap::new();
        // 删除安全点失败只告警，不覆盖业务错误。
        for dbidmap in idmaps {
            let tableRouter = router.entry(dbidmap.Name.clone()).or_default();
            // 首错保留但继续等待，避免残留线程。
            for tableidmap in &dbidmap.Tables {
                let down2upmap = tableRouter.entry(tableidmap.Name.clone()).or_default();
                down2upmap.insert(tableidmap.IdMap.DownstreamId, tableidmap.IdMap.UpstreamId);
                // 作用域守卫保证异常时归还并发槽。
                for phyidmap in &tableidmap.Partitions {
                    down2upmap.insert(phyidmap.DownstreamId, phyidmap.UpstreamId);
                }
            }
        }

        let mut reqs = Vec::new();
        // 表并发控制并发槽，至少为一。
        for t in curr {
            let mut rb = ExecutorBuilder::new(t.info.clone(), checksumTS);
            rb.SetConcurrency(self.cfg.Config.ChecksumConcurrency);
            // 克隆当前表 info，再把 ID/分区 ID 改写为上游值。
            let mut fakeOldTable = t.info.Clone();
            let tableRouter = router.get(&t.dbName).ok_or_else(|| {
                Error::Errorf(format!("no db map found by db name: {}", t.dbName))
            })?;
            let idRouter = tableRouter.get(&t.info.Name.O).ok_or_else(|| {
                Error::Errorf(format!(
                    "no table map found by table name: {}",
                    t.info.Name.O
                ))
            })?;
            let upstreamID = *idRouter
                .get(&t.info.ID)
                .ok_or_else(|| Error::Errorf(format!("no id map found by id: {}", t.info.ID)))?;
            // 服务安全点防止校验期间被垃圾回收。
            if let Some(part) = t.info.Partition.as_ref() {
                // 上游路径不设置旧表，直接校验当前标识。
                for (i, p) in part.Definitions.iter().enumerate() {
                    let upstreamPartID = *idRouter.get(&p.ID).ok_or_else(|| {
                        Error::Errorf(format!("no part id map found by id: {}", p.ID))
                    })?;
                    // 分区标识必须逐一定位上游映射。
                    if let Some(fake_part) = fakeOldTable.Partition.as_mut() {
                        fake_part.Definitions[i].ID = upstreamPartID;
                    }
                }
            }
            fakeOldTable.ID = upstreamID;
            rb.SetOldTable(&MetaTable {
                Info: fakeOldTable,
                ..Default::default()
            });
            rb.SetExplicitRequestSourceType("br");
            let req = rb.Build().map_err(|err| {
                Error::Annotatef(
                    err.msg,
                    format!(
                        "failed to build checksum executor for table {}.{}",
                        t.dbName, t.info.Name.O
                    ),
                )
            })?;
            reqs.push(request {
                copReq: req,
                dbName: t.dbName.clone(),
                tableName: t.info.Name.L.clone(),
            });
        }
        Ok(reqs)
    }

    /// upstream：不为旧表赋值，checksumTS 直接使用调用方传入的 RestoreTS。
    fn genUpstreamRequests(&mut self, curr: &[tableInDB], checksumTS: u64) -> Result<Vec<request>> {
        self.checksumTS = checksumTS;
        let mut reqs = Vec::new();
        // 原始大小写名用于映射索引，小写名用于过滤。
        for t in curr {
            let mut rb = ExecutorBuilder::new(t.info.clone(), checksumTS);
            rb.SetConcurrency(self.cfg.Config.ChecksumConcurrency);
            rb.SetExplicitRequestSourceType("br");
            let req = rb.Build().map_err(|err| {
                Error::Annotatef(
                    err.msg,
                    format!(
                        "failed to build checksum builder for table {}.{}",
                        t.dbName, t.info.Name.L
                    ),
                )
            })?;
            reqs.push(request {
                copReq: req,
                dbName: t.dbName.clone(),
                tableName: t.info.Name.L.clone(),
            });
        }
        Ok(reqs)
    }

    /// 挂 GC safepoint 后按 TableConcurrency 并发执行；收集首错，最后清理 safepoint。
    fn runChecksum(&self, reqs: Vec<request>) -> Result<Vec<ChecksumResult>> {
        eprintln!("checksum tables checksum ts={}", self.checksumTS);
        let mut sp = BRServiceSafePoint {
            BackupTS: self.checksumTS,
            TTL: DefaultBRGCSafePointTTL,
            ID: MakeSafePointID(),
        };
        let mgr = self.mgr.as_ref().unwrap();
        // 防止 checksum 期间 GC 越过 BackupTS。
        StartServiceSafePointKeeper(sp.clone(), mgr.GetGCManager().as_ref())
            .map_err(Error::Trace)?;

        let results = Arc::new(Mutex::new(Vec::with_capacity(reqs.len())));
        let mut handles = Vec::new();
        // 信号量槽位数 = TableConcurrency，至少为 1。
        let concurrency = self.cfg.Config.TableConcurrency.max(1) as usize;
        let sem = Arc::new(Mutex::new(concurrency));

        // SetOldTable 启用标识改写对照校验。
        for req in reqs {
            // Simple worker pool: wait for a slot.
            // 忙等拿槽；释放靠 ScopeGuard::Drop，避免 panic 泄漏并发度。
            loop {
                let mut guard = sem.lock().unwrap();
                if *guard > 0 {
                    *guard -= 1;
                    break;
                }
                drop(guard);
                thread::sleep(std::time::Duration::from_millis(1));
            }
            let results = results.clone();
            let store = mgr.GetStorage();
            let sem = sem.clone();
            handles.push(thread::spawn(move || -> Result<()> {
                let _slot = ScopeGuard(sem);
                let total = req.copReq.Len();
                let finished = AtomicI64::new(0);
                // 每完成一个 cop 请求回调一次进度日志。
                let resp = req.copReq.Execute(store.GetClient().as_ref(), || {
                    let n = finished.fetch_add(1, Ordering::SeqCst) + 1;
                    eprintln!(
                        "Finish one request of a table. db={} table={} finished={} total={}",
                        req.dbName, req.tableName, n, total
                    );
                })?;
                results.lock().unwrap().push(ChecksumResult {
                    DBName: req.dbName,
                    TableName: req.tableName,
                    Checksum: resp.Checksum,
                    TotalBytes: resp.TotalBytes,
                    TotalKVs: resp.TotalKvs,
                });
                Ok(())
            }));
        }

        // 等待全部 worker；记录首个错误但继续 join，避免僵尸线程。
        let mut first_err = None;
        // 外部存储映射文件名含集群与时间戳。
        for h in handles {
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(_) => {
                    if first_err.is_none() {
                        first_err = Some(Error::new("checksum worker panicked"));
                    }
                }
            }
        }

        // TTL=0 删除 service safepoint；失败只告警，不覆盖业务错误。
        eprintln!("start to remove gc-safepoint keeper");
        sp.TTL = 0;
        // 系统表缺 restore_id 列时走兼容查询。
        if let Err(err) = mgr.GetGCManager().DeleteServiceSafePoint(sp) {
            eprintln!(
                "failed to update service safe point, backup may fail if gc triggered: {err}"
            );
        }
        eprintln!("finish removing gc-safepoint keeper");

        // PiTR 分片必须从零连续且非空。
        if let Some(e) = first_err {
            // 备份缺表时跳过而非整批失败。
            return Err(e);
        }
        let out = results.lock().unwrap().clone();
        Ok(out)
    }
}

/// RAII：离开作用域时归还一个并发槽。
struct ScopeGuard(Arc<Mutex<usize>>);
// TableFilter 同时作用于当前库与备份元数据。
impl Drop for ScopeGuard {
    // DIAL_HOOKS 可在测试中注入假连接管理器。
    fn drop(&mut self) {
        *self.0.lock().unwrap() += 1;
    }
}

/// 单表 checksum 任务：已构建的 executor + 展示用库表名。
struct request {
    copReq: ChecksumExecutor,
    tableName: String,
    dbName: String,
}

/// 对外 JSON 结果字段，命名与 Go 结构体 json tag 对齐。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
// stderr 给人看摘要，stdout 输出 JSON。
pub struct ChecksumResult {
    #[serde(rename = "db_name")]
    pub DBName: String,
    #[serde(rename = "table_name")]
    pub TableName: String,
    #[serde(rename = "checksum")]
    pub Checksum: u64,
    #[serde(rename = "total_bytes")]
    pub TotalBytes: u64,
    #[serde(rename = "total_kvs")]
    pub TotalKVs: u64,
}

/// PiTR 入口：加载 id-map，ChecksumTS=0 时向 PD 取当前 TSO。
pub fn RunPitrChecksumTable(g: &dyn Glue, mut cfg: ChecksumWithPitrIdMapConfig) -> Result<()> {
    let mut c = checksumTableCtx {
        cfg: ChecksumWithRewriteRulesConfig {
            Config: cfg.RestoreConfig.Config.clone(),
        },
        mgr: None,
        dom: None,
        checksumTS: 0,
    };
    c.init(g)?;
    let curr = c.getTables()?;
    let pitrIdMap = c.loadPitrIdMap(
        g,
        cfg.RestoreConfig.RestoreTS,
        cfg.RestoreConfig.UpstreamClusterID,
    )?;
    // 未显式指定 checksum-ts 时使用 PD 当前时间戳。
    if cfg.ChecksumTS == 0 {
        let checksumTS =
            GetTSWithRetry(c.mgr.as_ref().unwrap().GetPDClient().as_ref()).map_err(Error::Trace)?;
        cfg.ChecksumTS = checksumTS;
    }
    let reqs = c.genRequestsWithIDMap(&curr, &pitrIdMap, cfg.ChecksumTS)?;
    let results = c.runChecksum(reqs)?;
    // 三类入口共享上下文，减少重复初始化。
    for result in &results {
        eprintln!(
            "Checksum result db={} table={} checksum={} total_bytes={} total_kvs={}",
            result.DBName, result.TableName, result.Checksum, result.TotalBytes, result.TotalKVs
        );
    }
    println!(
        "{}",
        serde_json::to_string(&results).map_err(|e| Error::new(e.to_string()))?
    );
    Ok(())
}

/// Test helper exposing ID-map request generation without full cluster init.
/// 测试专用：跳过 ConnMgr，只验证 id-map 改写后的 table_id。
pub fn gen_requests_with_id_map_for_test(
    curr: Vec<(String, TableInfo)>,
    idmaps: &[PitrDBMap],
    checksumTS: u64,
    checksumConcurrency: u32,
) -> Result<Vec<(String, String, i64)>> {
    let mut c = checksumTableCtx {
        cfg: ChecksumWithRewriteRulesConfig {
            Config: {
                let mut cfg = crate::stubs::Config::default();
                cfg.ChecksumConcurrency = checksumConcurrency;
                cfg
            },
        },
        mgr: None,
        dom: None,
        checksumTS: 0,
    };
    let curr: Vec<tableInDB> = curr
        .into_iter()
        .map(|(dbName, info)| tableInDB { info, dbName })
        .collect();
    let reqs = c.genRequestsWithIDMap(&curr, idmaps, checksumTS)?;
    // 返回 (db, table, rewritten table_id) 供断言。
    Ok(reqs
        .into_iter()
        .map(|r| (r.dbName, r.tableName, r.copReq.req.table_id))
        .collect())
}

/// Expose CIStr constructor for tests.
/// 构造带可选分区定义的 TableInfo，供 id-map 单测使用。
pub fn test_table(id: i64, name: &str, parts: &[(i64, &str)]) -> TableInfo {
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Partition: if parts.is_empty() {
            None
        } else {
            Some(crate::stubs::PartitionInfo {
                Definitions: parts
                    .iter()
                    .map(|(id, _)| crate::stubs::PartitionDefinition { ID: *id })
                    .collect(),
            })
        },
        ..Default::default()
    }
}
