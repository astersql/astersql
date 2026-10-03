// Copyright 2026 AsterSQL.

//! Restore lifecycle bindings supplied by the owner of the live PD/import clients.
//! Prepared file sets retain table IDs, rewrite rules and full SST metadata. They
//! are consumed by the same restorer used for compacted SSTs, not progress counters.

use crate::stubs::backuppb::File;
use crate::stubs::{Error, Glue, Progress, Result};
use astersql_br_pkg_restore as restore;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreKind {
    Snapshot,
    Raw,
    Txn,
    Stream,
}

pub enum RestoreImporter {
    Simple(Arc<dyn restore::FileImporter>),
    Snapshot(Arc<dyn restore::BalancedFileImporter>),
}
impl RestoreImporter {
    pub fn FileImporter(&self) -> Arc<dyn restore::FileImporter> {
        match self {
            Self::Simple(importer) => importer.clone(),
            Self::Snapshot(importer) => importer.clone(),
        }
    }
}

pub struct RestoreLifecycle {
    pub context: restore::stubs::Context,
    pub pd: Arc<dyn restore::stubs::PdClient>,
    pub schedulers: Arc<dyn restore::stubs::ConnMgr>,
    pub mode_transport: Arc<dyn restore::stubs::ImportSstSwitcher>,
    pub importer: RestoreImporter,
    pub file_sets: restore::BatchBackupFileSet,
    pub checkpoint_runner: Option<Arc<dyn restore::stubs::RestoreCheckpoint>>,
    pub checkpoint_compacted_size: u64,
    /// Prepared key ranges for fine-grained scheduler pausing, when applicable.
    pub key_ranges: Option<Vec<[restore::stubs::Key; 2]>>,
}

pub type RestoreLifecycleFactory =
    Arc<dyn Fn(RestoreKind, &[File]) -> Result<RestoreLifecycle> + Send + Sync>;

/// Production glue adapter: clients and prepared metadata remain owned by the
/// embedding restore service. Every task entry asks this binding for its runtime.
pub struct RestoreGlue<'a> {
    pub inner: &'a dyn Glue,
    pub factory: RestoreLifecycleFactory,
}
impl Glue for RestoreGlue<'_> {
    fn GetVersion(&self) -> String {
        self.inner.GetVersion()
    }
    fn StartProgress(&self, cmd: &str, total: i64, log: bool) -> Arc<dyn Progress> {
        self.inner.StartProgress(cmd, total, log)
    }
    fn Record(&self, key: &str, value: u64) {
        self.inner.Record(key, value)
    }
    fn ConsoleOutWrite(&self, msg: &[u8]) -> Result<()> {
        self.inner.ConsoleOutWrite(msg)
    }
    fn GetRestoreLifecycle(&self, kind: RestoreKind, files: &[File]) -> Result<RestoreLifecycle> {
        (self.factory)(kind, files)
    }
}

impl RestoreLifecycle {
    /// Prevent a factory from silently dropping metadata-selected backup files.
    pub fn ValidateFiles(&self, files: &[File]) -> Result<()> {
        let mut expected: Vec<_> = files
            .iter()
            .map(|f| (&f.Name, &f.StartKey, &f.EndKey, &f.Cf, f.Size_))
            .collect();
        let mut actual: Vec<_> = self
            .file_sets
            .iter()
            .flat_map(|s| &s.SSTFiles)
            .map(|f| (&f.Name, &f.StartKey, &f.EndKey, &f.Cf, f.Size_))
            .collect();
        expected.sort();
        actual.sort();
        if expected != actual {
            return Err(Error::new(
                "prepared SST file sets do not match selected backup files",
            ));
        }
        Ok(())
    }
}

/// Use the live PD store discovery client, preserving TiFlash labels for filtering.
pub struct RestorePD(pub Arc<dyn astersql_br_pkg_conn::StoreMeta>);
impl restore::stubs::PdClient for RestorePD {
    fn GetTS(&self, ctx: &restore::stubs::Context) -> restore::stubs::Result<(i64, i64)> {
        if let Some(error) = ctx.Err() {
            return Err(error);
        }
        self.0
            .GetTS()
            .map_err(|e| restore::stubs::Error::new(e.to_string()))
    }
    fn GetAllStores(
        &self,
        ctx: &restore::stubs::Context,
    ) -> restore::stubs::Result<Vec<restore::stubs::metapb::Store>> {
        if let Some(error) = ctx.Err() {
            return Err(error);
        }
        self.0
            .GetAllStores(true)
            .map(|stores| {
                stores
                    .into_iter()
                    .map(|s| restore::stubs::metapb::Store {
                        Id: s.id,
                        Address: s.address,
                        Labels: s.labels.into_iter().map(|l| (l.key, l.value)).collect(),
                    })
                    .collect()
            })
            .map_err(|e| restore::stubs::Error::new(e.to_string()))
    }
}

struct RestoreSession {
    ctx: restore::stubs::Context,
    mode: restore::ImportModeSwitcher,
    undo: restore::stubs::UndoFunc,
    online: bool,
    restore_schedulers: bool,
}
impl Drop for RestoreSession {
    fn drop(&mut self) {
        if self.restore_schedulers {
            restore::RestorePostWork(
                self.ctx.clone(),
                &mut self.mode,
                self.undo.clone(),
                self.online,
            );
        } else {
            self.mode.StopRefreshing();
        }
    }
}
impl RestoreLifecycle {
    fn Start(
        &self,
        interval: std::time::Duration,
        online: bool,
        import: bool,
        ranges: Option<&[[restore::stubs::Key; 2]]>,
        pause: bool,
    ) -> Result<RestoreSession> {
        let mut mode =
            restore::NewImportModeSwitcher(self.pd.clone(), interval, self.mode_transport.clone());
        let undo = if !pause {
            Ok(restore::stubs::nop_undo())
        } else if let Some(ranges) = ranges {
            restore::FineGrainedRestorePreWork(
                &self.context,
                self.schedulers.as_ref(),
                &mut mode,
                ranges,
                import && !online,
            )
            .map(|(undo, _)| undo)
        } else {
            restore::RestorePreWork(
                &self.context,
                self.schedulers.as_ref(),
                &mut mode,
                online,
                import,
            )
            .map(|(undo, _)| undo)
        }
        .map_err(|e| Error::new(e.to_string()))?;
        Ok(RestoreSession {
            ctx: self.context.clone(),
            mode,
            undo,
            online,
            restore_schedulers: true,
        })
    }

    pub fn RestoreFiles(
        &self,
        kind: RestoreKind,
        interval: std::time::Duration,
        concurrency: u32,
        online: bool,
        checkpoint: bool,
        progress: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> Result<()> {
        use restore::SstRestorer;
        let result = (|| {
            let mut session = self.Start(
                interval,
                online,
                true,
                if kind == RestoreKind::Snapshot {
                    self.key_ranges.as_deref()
                } else {
                    None
                },
                true,
            )?;
            session.restore_schedulers = !checkpoint;
            if checkpoint && self.checkpoint_runner.is_none() {
                return Err(Error::new("snapshot checkpoint runner is not configured"));
            }
            let pool = restore::stubs::NewWorkerPool(concurrency.max(1) as u64, "restore files");
            let restorer: Box<dyn restore::SstRestorer> = if kind == RestoreKind::Snapshot {
                let RestoreImporter::Snapshot(importer) = &self.importer else {
                    return Err(Error::new("snapshot balanced importer is not configured"));
                };
                Box::new(restore::NewMultiTablesRestorer(
                    &self.context,
                    importer.clone(),
                    pool,
                    self.checkpoint_runner.clone(),
                ))
            } else {
                Box::new(restore::NewSimpleSstRestorer(
                    &self.context,
                    self.importer.FileImporter(),
                    pool,
                    None,
                ))
            };
            let result = restorer
                .GoRestore(progress, vec![self.file_sets.clone()])
                .and_then(|_| restorer.WaitUntilFinish());
            // Snapshot checkpoint retries retain paused schedulers on a failed restore.
            session.restore_schedulers = !checkpoint || result.is_ok();
            result.map_err(|e| Error::new(e.to_string()))
        })();
        // Mirrors the outer restore client's deferred Close, including pre-work errors.
        let _ = self.importer.FileImporter().Close();
        result
    }

    pub fn RestoreCompactedSST(
        &self,
        client: &mut astersql_br_pkg_restore_log_client::LogClient,
        cfg: &crate::restore::RestoreConfig,
        progress: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> Result<()> {
        let mut session = self.Start(
            cfg.Config.SwitchModeInterval,
            cfg.RestoreCommonConfig.Online,
            false,
            if cfg.Config.ExplicitFilter {
                self.key_ranges
                    .as_deref()
                    .filter(|ranges| !ranges.is_empty())
            } else {
                None
            },
            !cfg.Config.ExplicitFilter
                || self
                    .key_ranges
                    .as_ref()
                    .is_some_and(|ranges| !ranges.is_empty()),
        )?;
        client
            .InitSSTFileRestorer(
                &self.context,
                self.importer.FileImporter(),
                self.checkpoint_runner.clone(),
            )
            .map_err(|e| Error::new(e.to_string()))?;
        let parent = self.context.clone();
        let log_context =
            astersql_br_pkg_restore_log_client::stubs::Context::WithCancellationSource(move || {
                parent
                    .Err()
                    .map(|error| astersql_br_pkg_restore_log_client::stubs::Error {
                        msg: error.msg,
                        code: error.code,
                    })
            });
        client
            .RestoreSSTFileSets(
                &log_context,
                &self.context,
                self.file_sets.clone(),
                &mut session.mode,
                cfg.RestoreCommonConfig.Online,
                cfg.snapshotRestoreDataSize,
                self.checkpoint_compacted_size,
                progress,
            )
            .map_err(|e| Error::new(e.to_string()))
    }
}
