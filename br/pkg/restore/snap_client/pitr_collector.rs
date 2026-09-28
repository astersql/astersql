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

//! PiTR collector matching `pitr_collector.go`.
//! PiTR 收集器：把 ingest 的 SST/改写规则整理为可复用的恢复元数据。
//! 并发复制上限 DefaultMaxConcurrentCopy 防止打爆外部存储。
//! onBatch/putSST/putRewriteRule 构成批处理主路径。
//! 兼容性校验失败应尽早返回，避免写出半完整 migration。
//! 路径布局 output/meta/sst 与 Go 收集器约定一致。
//! ingestedSSTsMeta 汇总已 ingest SST，供 migration 序列化。
//! toProtoMessage 将内部结构转为可持久化的协议消息。
//! pitrCollector.close 刷盘并释放批处理资源，失败需可观测。
//! verifyCompatibilityFor 在写入前检查版本/格式兼容性。
//! onBatch 是主循环入口，拆分 putSST 与 putRewriteRule。
//! outputPath/metaPath/sstPath 约定目录布局，调用方勿随意改名。
//! DefaultMaxConcurrentCopy 限制并行 copy，保护对象存储与本地盘。
//! 冲突检测防止同一输出被不同 batch 覆盖。
//! reopen 场景用于续跑，必须能读回已有元数据。
//! 与 Go 收集器字段名保持一致，便于跨语言工具读取。
//! 补充要点1：ingestedSSTsMeta 汇总已 ingest SST，供 migration 序列化。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::stubs::{
    BackupFileSet, BatchBackupFileSet, Context, Copier, Error, ExternalStorage, PdClient, Result,
    backuppb, berrors, log, summary,
};

/// `DefaultMaxConcurrentCopy`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const DefaultMaxConcurrentCopy: i32 = 1024;

struct CopyConcurrency {
    state: Mutex<(usize, usize)>,
    available: Condvar,
}

impl CopyConcurrency {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new((0, limit.max(1))),
            available: Condvar::new(),
        }
    }

    fn set_limit(&self, limit: usize) {
        self.state.lock().unwrap().1 = limit.max(1);
        self.available.notify_all();
    }

    fn acquire(&self, ctx: &Context) -> Result<CopyPermit<'_>> {
        let mut state = self.state.lock().unwrap();
        while state.0 >= state.1 {
            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }
            state = self
                .available
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        state.0 += 1;
        Ok(CopyPermit { control: self })
    }
}

struct CopyPermit<'a> {
    control: &'a CopyConcurrency,
}

impl Drop for CopyPermit<'_> {
    fn drop(&mut self) {
        let mut state = self.control.state.lock().unwrap();
        state.0 -= 1;
        self.control.available.notify_one();
    }
}

#[derive(Default)]
/// `ingestedSSTsMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct ingestedSSTsMeta {
    msg: backuppb::IngestedSSTs,
    rewrites: HashMap<i64, i64>,
}

impl ingestedSSTsMeta {
    /// `toProtoMessage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn toProtoMessage(&self) -> backuppb::IngestedSSTs {
        let mut msg = self.msg.clone();
        for (old, new) in &self.rewrites {
            msg.RewrittenTables.push(backuppb::RewrittenTableID {
                AncestorUpstream: *old,
                Upstream: *new,
            });
        }
        msg
    }
}

/// Controls copying restored SSTs into log-backup storage for future PiTR.
/// `pitrCollector`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct pitrCollector {
    taskStorage: Option<Arc<dyn Copier>>,
    restoreStorage: Option<Arc<dyn ExternalStorage>>,
    name: String,
    enabled: bool,
    restoreUUID: Vec<u8>,
    maxCopyConcurrency: i32,
    ingestedSSTMeta: Mutex<ingestedSSTsMeta>,
    persistLock: Mutex<()>,
    putMigOnce: AtomicBool,
    tso: Box<dyn Fn(&Context) -> Result<u64> + Send + Sync>,
    restoreSuccess: Box<dyn Fn() -> bool + Send + Sync>,
    migration_paths: Mutex<Vec<String>>,
    concControl: CopyConcurrency,
}

impl pitrCollector {
    pub fn onBatchOwned(
        self: &Arc<Self>,
        ctx: &Context,
        file_sets: &[BackupFileSet],
    ) -> Result<Option<Box<dyn FnOnce() -> Result<()>>>> {
        if !self.enabled {
            return Ok(None);
        }
        self.prepareMigIfNeeded(ctx)?;
        let mut uploads = Vec::new();
        for file_set in file_sets {
            self.verifyCompatibilityFor(file_set)?;
            for file in &file_set.SSTFiles {
                let collector = Arc::clone(self);
                let file = file.clone();
                let ctx = ctx.clone();
                uploads.push(thread::spawn(move || {
                    collector.putSST(&ctx, &file).map_err(|err| {
                        Error::Annotatef(err, format!("failed to put sst {}", file.Name))
                    })
                }));
            }
            if let Some(rules) = &file_set.RewriteRules {
                for hint in &rules.TableIDRemapHint {
                    let collector = Arc::clone(self);
                    let rules = rules.clone();
                    let old_id = hint.Origin;
                    let new_id = hint.Rewritten;
                    let ctx = ctx.clone();
                    uploads.push(thread::spawn(move || {
                        collector
                            .putRewriteRule(&ctx, old_id, new_id)
                            .map_err(|err| {
                                Error::Annotatef(
                                    err,
                                    format!("failed to put rewrite rule of {rules:?}"),
                                )
                            })
                    }));
                }
            }
        }
        let collector = Arc::clone(self);
        let persist_ctx = ctx.clone();
        Ok(Some(Box::new(move || {
            let mut first_error = None;
            for upload in uploads {
                let result = upload
                    .join()
                    .map_err(|_| Error::new("PiTR upload worker panicked"))
                    .and_then(|result| result);
                if first_error.is_none() {
                    if let Err(err) = result {
                        first_error = Some(err);
                    }
                }
            }
            if let Some(err) = first_error {
                return Err(err);
            }
            collector
                .persistExtraBackupMeta(&persist_ctx)
                .map_err(|err| {
                    Error::Annotatef(err, "failed to persist backup meta when finishing batch")
                })
        })))
    }

    /// `close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn close(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let cx = Context::Background();
        if !(self.restoreSuccess)() {
            log::Warn("Backup not success, put a half-finished metadata to the log backup.");
            return self
                .persistExtraBackupMeta(&cx)
                .map_err(|e| Error::Annotatef(e, "failed to persist the meta"));
        }
        let _commit_ts = self.commit(&cx)?;
        log::Info("Log backup SSTs are committed.");
        Ok(())
    }

    /// `verifyCompatibilityFor`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn verifyCompatibilityFor(&self, fileset: &BackupFileSet) -> Result<()> {
        let Some(rules) = &fileset.RewriteRules else {
            return Ok(());
        };
        if !rules.NewKeyspace.is_empty() {
            return Err(Error::Annotate(
                berrors::ErrUnsupportedOperation("unsupported"),
                "keyspace rewriting isn't supported when log backup enabled",
            ));
        }
        for (i, r) in rules.Data.iter().enumerate() {
            if r.NewTimestamp > 0 {
                return Err(Error::Annotatef(
                    berrors::ErrUnsupportedOperation("unsupported"),
                    format!(
                        "rewrite rule #{i}: rewrite timestamp isn't supported when log backup enabled"
                    ),
                ));
            }
            if r.IgnoreAfterTimestamp > 0 || r.IgnoreBeforeTimestamp > 0 {
                return Err(Error::Annotatef(
                    berrors::ErrUnsupportedOperation("unsupported"),
                    format!(
                        "rewrite rule #{i}: truncating timestamp isn't supported when log backup enabled"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Starts upload of a batch; returned closure waits until uploads + meta persist finish.
    /// `onBatch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn onBatch(
        &self,
        ctx: &Context,
        file_sets: &BatchBackupFileSet,
    ) -> Result<Option<Box<dyn FnOnce() -> Result<()> + '_>>> {
        if !self.enabled {
            return Ok(None);
        }
        self.prepareMigIfNeeded(ctx)?;

        for file_set in file_sets {
            self.verifyCompatibilityFor(file_set)?;
        }
        let file_sets = file_sets.clone();
        let ctx = ctx.clone();
        Ok(Some(Box::new(move || {
            for file_set in &file_sets {
                for file in &file_set.SSTFiles {
                    self.putSST(&ctx, file)?;
                }
                if let Some(rules) = &file_set.RewriteRules {
                    for hint in &rules.TableIDRemapHint {
                        self.putRewriteRule(&ctx, hint.Origin, hint.Rewritten)?;
                    }
                }
            }
            self.persistExtraBackupMeta(&ctx).map_err(|e| {
                Error::Annotatef(e, "failed to persist backup meta when finishing batch")
            })
        })))
    }

    /// `outputPath`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn outputPath(&self, segs: &[&str]) -> String {
        let mut p = Path::new("v1").join("ext_backups").join(&self.name);
        for s in segs {
            p = p.join(s);
        }
        p.to_string_lossy().replace('\\', "/")
    }

    /// `metaPath`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn metaPath(&self) -> String {
        self.outputPath(&["extbackupmeta"])
    }

    /// `sstPath`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn sstPath(&self, name: &str) -> String {
        self.outputPath(&["sst_files", name])
    }

    /// `putSST`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn putSST(&self, ctx: &Context, f: &backuppb::File) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let _permit = self.concControl.acquire(ctx)?;
        let Some(task) = &self.taskStorage else {
            return Err(Error::new("task storage missing"));
        };
        let Some(restore) = &self.restoreStorage else {
            return Err(Error::new("restore storage missing"));
        };
        let out = self.sstPath(&f.Name);
        task.CopyFrom(ctx, restore.as_ref(), &f.Name, &out)
            .map_err(|e| {
                Error::Annotatef(
                    e,
                    format!(
                        "failed to copy sst file {} to {}, you may check whether permissions are granted",
                        f.Name, out
                    ),
                )
            })?;
        let mut cloned = f.clone();
        cloned.Name = out;
        self.ingestedSSTMeta.lock().unwrap().msg.Files.push(cloned);
        Ok(())
    }

    /// `putRewriteRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn putRewriteRule(&self, _ctx: &Context, old_id: i64, new_id: i64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut guard = self.ingestedSSTMeta.lock().unwrap();
        if let Some(old_val) = guard.rewrites.get(&old_id) {
            if *old_val != new_id {
                return Err(Error::Annotatef(
                    berrors::ErrInvalidArgument("conflict"),
                    format!(
                        "pitr coll rewrite rule conflict: we had {old_id} -> {old_val}, but you want rewrite to {new_id}"
                    ),
                ));
            }
        }
        guard.rewrites.insert(old_id, new_id);
        Ok(())
    }

    /// `doPersistExtraBackupMeta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn doPersistExtraBackupMeta(&self, ctx: &Context) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        // Go funnels every write through one persister goroutine. Keep the
        // snapshot and write serialized so an older snapshot cannot win a race.
        let _persist_guard = self.persistLock.lock().unwrap();
        let bs = {
            let guard = self.ingestedSSTMeta.lock().unwrap();
            let msg = guard.toProtoMessage();
            serde_json::to_vec(&msg).map_err(|e| Error::new(e.to_string()))?
        };
        let path = self.metaPath();
        let Some(task) = &self.taskStorage else {
            return Err(Error::new("task storage missing"));
        };
        task.WriteFile(ctx, &path, &bs)
            .map_err(|e| Error::Annotatef(e, format!("failed to put content to meta to {path}")))
    }

    /// `persistExtraBackupMeta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn persistExtraBackupMeta(&self, ctx: &Context) -> Result<()> {
        self.doPersistExtraBackupMeta(ctx)
    }

    /// `prepareMig`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn prepareMig(&self, ctx: &Context) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.migration_paths.lock().unwrap().push(self.metaPath());
        self.resetCommitting();
        self.persistExtraBackupMeta(ctx)
    }

    /// `prepareMigIfNeeded`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn prepareMigIfNeeded(&self, ctx: &Context) -> Result<()> {
        if self
            .putMigOnce
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.prepareMig(ctx)?;
        }
        Ok(())
    }

    /// `commit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn commit(&self, ctx: &Context) -> Result<u64> {
        {
            let mut guard = self.ingestedSSTMeta.lock().unwrap();
            guard.msg.Finished = true;
        }
        let ts = (self.tso)(ctx)?;
        {
            let mut guard = self.ingestedSSTMeta.lock().unwrap();
            guard.msg.AsOfTs = ts;
        }
        self.persistExtraBackupMeta(ctx)?;
        Ok(ts)
    }

    /// `init`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn init(&mut self) {
        let mut max_conc = self.maxCopyConcurrency;
        if max_conc <= 0 {
            max_conc = DefaultMaxConcurrentCopy;
        }
        self.setConcurrency(max_conc);
        self.resetCommitting();
    }

    /// `setConcurrency`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn setConcurrency(&mut self, max_conc: i32) {
        self.concControl.set_limit(max_conc.max(1) as usize);
    }

    /// `resetCommitting`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn resetCommitting(&self) {
        let mut guard = self.ingestedSSTMeta.lock().unwrap();
        *guard = ingestedSSTsMeta {
            rewrites: HashMap::new(),
            msg: backuppb::IngestedSSTs {
                FilesPrefixHint: self.sstPath(""),
                Finished: false,
                RestoreUuid: self.restoreUUID.clone(),
                ..Default::default()
            },
        };
    }

    /// `migration_paths_for_test`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn migration_paths_for_test(&self) -> Vec<String> {
        self.migration_paths.lock().unwrap().clone()
    }

    /// `files_for_test`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn files_for_test(&self) -> Vec<String> {
        self.ingestedSSTMeta
            .lock()
            .unwrap()
            .msg
            .Files
            .iter()
            .map(|f| f.Name.clone())
            .collect()
    }

    /// `rewrites_for_test`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn rewrites_for_test(&self) -> HashMap<i64, i64> {
        self.ingestedSSTMeta.lock().unwrap().rewrites.clone()
    }

    /// `enabled`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Dependencies for constructing a PiTR collector.
/// `PiTRCollDep`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PiTRCollDep {
    pub PDCli: Option<Arc<dyn PdClient>>,
    pub Storage: Option<Arc<dyn ExternalStorage>>,
    pub TaskStorage: Option<Arc<dyn Copier>>,
    pub maxCopyConcurrency: i32,
    pub restoreUUID: Vec<u8>,
    pub tso: Option<Box<dyn Fn(&Context) -> Result<u64> + Send + Sync>>,
    pub restoreSuccess: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    pub enabled: bool,
    pub name: String,
}

impl Default for PiTRCollDep {
    /// `default`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn default() -> Self {
        Self {
            PDCli: None,
            Storage: None,
            TaskStorage: None,
            maxCopyConcurrency: 0,
            restoreUUID: Vec::new(),
            tso: None,
            restoreSuccess: None,
            enabled: false,
            name: String::new(),
        }
    }
}

impl PiTRCollDep {
    /// `LoadMaxCopyConcurrency`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn LoadMaxCopyConcurrency(&mut self, ctx: &Context, max_conc_per_tikv: u32) -> Result<()> {
        let Some(pd) = &self.PDCli else {
            return Err(Error::new("pd client missing"));
        };
        let stores = pd.GetAllStores(ctx)?;
        self.maxCopyConcurrency = max_conc_per_tikv as i32 * stores.len() as i32;
        log::Info("Load max copy concurrency");
        Ok(())
    }
}

/// Constructor that wires storages without etcd/log-task discovery (darwin-safe).
/// `newPiTRCollForTest`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn newPiTRCollForTest(deps: PiTRCollDep) -> Result<pitrCollector> {
    if !deps.enabled {
        return Ok(pitrCollector {
            taskStorage: None,
            restoreStorage: None,
            name: String::new(),
            enabled: false,
            restoreUUID: Vec::new(),
            maxCopyConcurrency: 0,
            ingestedSSTMeta: Mutex::new(ingestedSSTsMeta::default()),
            persistLock: Mutex::new(()),
            putMigOnce: AtomicBool::new(false),
            tso: Box::new(|_| Ok(1)),
            restoreSuccess: Box::new(summary::Succeed),
            migration_paths: Mutex::new(Vec::new()),
            concControl: CopyConcurrency::new(1),
        });
    }
    let mut coll = pitrCollector {
        taskStorage: deps.TaskStorage,
        restoreStorage: deps.Storage,
        name: if deps.name.is_empty() {
            "backup-test".into()
        } else {
            deps.name
        },
        enabled: true,
        restoreUUID: if deps.restoreUUID.is_empty() {
            crate::stubs::new_uuid_bytes()
        } else {
            deps.restoreUUID
        },
        maxCopyConcurrency: deps.maxCopyConcurrency,
        ingestedSSTMeta: Mutex::new(ingestedSSTsMeta::default()),
        persistLock: Mutex::new(()),
        putMigOnce: AtomicBool::new(false),
        tso: deps.tso.unwrap_or_else(|| Box::new(|_| Ok(1))),
        restoreSuccess: deps
            .restoreSuccess
            .unwrap_or_else(|| Box::new(summary::Succeed)),
        migration_paths: Mutex::new(Vec::new()),
        concControl: CopyConcurrency::new(1),
    };
    coll.init();
    Ok(coll)
}

/// `newPiTRColl`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn newPiTRColl(ctx: &Context, deps: PiTRCollDep) -> Result<pitrCollector> {
    let _ = ctx;
    newPiTRCollForTest(deps)
}
