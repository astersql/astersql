// Copyright 2026 AsterSQL.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::stubs::EtcdKV;
use crate::{
    AdvancerExt, EventType, GlobalCheckpointOf, MemEtcd, NewMetaDataClient, NewTaskInfo, Pause,
    StorageBackend, TaskEvent, TaskOf, WatchContext, encodeUint64, lastCheckpointMetric,
};

fn task(name: &str) -> crate::TaskInfo {
    NewTaskInfo(name)
        .WithTableFilter(&["*.*"])
        .ToStorage(StorageBackend {
            Uri: "noop://".into(),
        })
        .WithRange(b"a", b"z")
}

fn recv(rx: &mpsc::Receiver<TaskEvent>) -> TaskEvent {
    rx.recv_timeout(Duration::from_secs(2)).expect("task event")
}

#[test]
fn begin_delivers_snapshot_then_live_task_and_pause_events() {
    let kv = Arc::new(MemEtcd::new());
    let meta = NewMetaDataClient(kv.clone());
    meta.PutTask(&task("before")).unwrap();
    let ext = AdvancerExt { meta: meta.clone() };
    let ctx = WatchContext::new();
    let (tx, rx) = mpsc::channel();

    ext.Begin(ctx.clone(), tx).unwrap();
    let initial = recv(&rx);
    assert_eq!(initial.Type, EventType::EventAdd);
    assert_eq!(initial.Name, "before");
    assert_eq!(initial.Ranges.len(), 1);

    meta.PutTask(&task("after")).unwrap();
    let added = recv(&rx);
    assert_eq!(
        (added.Type, added.Name.as_str()),
        (EventType::EventAdd, "after")
    );
    assert_eq!(added.Ranges.len(), 1);

    kv.Put(&Pause("after"), b"paused").unwrap();
    assert_eq!(recv(&rx).Type, EventType::EventPause);
    kv.Delete(&Pause("after")).unwrap();
    assert_eq!(recv(&rx).Type, EventType::EventResume);
    kv.Delete(&TaskOf("after")).unwrap();
    assert_eq!(recv(&rx).Type, EventType::EventDel);

    ctx.cancel();
    let cancelled = recv(&rx);
    assert_eq!(cancelled.Type, EventType::EventErr);
    assert!(cancelled.Err.unwrap().contains("canceled"));
}

#[test]
fn malformed_live_task_is_reported_without_killing_other_watch() {
    let kv = Arc::new(MemEtcd::new());
    let ext = AdvancerExt {
        meta: NewMetaDataClient(kv.clone()),
    };
    let ctx = WatchContext::new();
    let (tx, rx) = mpsc::channel();
    ext.Begin(ctx.clone(), tx).unwrap();

    kv.Put(&TaskOf("bad"), b"not-json").unwrap();
    let event = recv(&rx);
    assert_eq!(event.Type, EventType::EventErr);
    assert!(event.Err.is_some());

    kv.Put(&Pause("still-alive"), b"paused").unwrap();
    assert_eq!(recv(&rx).Type, EventType::EventPause);
    ctx.cancel();
}

#[test]
fn wait_checkpoint_observes_revisioned_put_and_honors_cancel() {
    let kv = Arc::new(MemEtcd::new());
    let meta = NewMetaDataClient(kv.clone());
    let waiter = meta.clone();
    let ctx = WatchContext::new();
    let wait_ctx = ctx.clone();
    let handle =
        std::thread::spawn(move || waiter.WaitGlobalCheckpointAdvance(wait_ctx, "task", 10));
    kv.Put(&GlobalCheckpointOf("task"), &encodeUint64(11))
        .unwrap();
    assert_eq!(handle.join().unwrap(), Ok(()));

    let cancel_ctx = WatchContext::new();
    let thread_ctx = cancel_ctx.clone();
    let waiter = meta.clone();
    let handle =
        std::thread::spawn(move || waiter.WaitGlobalCheckpointAdvance(thread_ctx, "task", 11));
    cancel_ctx.cancel();
    assert!(handle.join().unwrap().unwrap_err().contains("canceled"));
}

#[test]
fn upload_updates_metric_only_after_successful_monotonic_write() {
    let kv = Arc::new(MemEtcd::new());
    let ext = AdvancerExt {
        meta: NewMetaDataClient(kv),
    };
    ext.UploadV3GlobalCheckpointForTask("metric-task", 20)
        .unwrap();
    assert_eq!(lastCheckpointMetric("metric-task"), Some(20));
    ext.UploadV3GlobalCheckpointForTask("metric-task", 10)
        .unwrap();
    assert_eq!(lastCheckpointMetric("metric-task"), Some(20));
}

struct CompactedCheckpointKV {
    inner: MemEtcd,
}
impl EtcdKV for CompactedCheckpointKV {
    fn Put(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.inner.Put(k, v)
    }
    fn Get(&self, k: &str) -> Result<Vec<u8>, String> {
        self.inner.Get(k)
    }
    fn Delete(&self, k: &str) -> Result<(), String> {
        self.inner.Delete(k)
    }
    fn GetPrefix(&self, p: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        self.inner.GetPrefix(p)
    }
    fn DeletePrefix(&self, p: &str) -> Result<(), String> {
        self.inner.DeletePrefix(p)
    }
    fn GetWithRevision(&self, k: &str) -> Result<crate::stubs::RevisionedValue, String> {
        self.inner.GetWithRevision(k)
    }
    fn GetPrefixWithRevision(&self, p: &str) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, i64), String> {
        self.inner.GetPrefixWithRevision(p)
    }
    fn WatchPrefix(
        &self,
        p: &str,
        r: i64,
    ) -> Result<mpsc::Receiver<crate::stubs::WatchEvent>, String> {
        let header = self.inner.GetWithRevision(p)?.Revision;
        if r <= header {
            return Err("required revision has been compacted".into());
        }
        let receiver = self.inner.WatchPrefix(p, r)?;
        self.inner.Put(p, &encodeUint64(20))?;
        Ok(receiver)
    }
    fn RequestWatchProgress(&self) -> Result<(), String> {
        self.inner.RequestWatchProgress()
    }
}

#[test]
fn checkpoint_watch_uses_response_revision_after_compaction() {
    let kv = Arc::new(CompactedCheckpointKV {
        inner: MemEtcd::new(),
    });
    kv.Put(&GlobalCheckpointOf("compacted"), &encodeUint64(10))
        .unwrap();
    kv.Put("unrelated-key", b"later-revision").unwrap();
    let meta = NewMetaDataClient(kv);
    assert_eq!(
        meta.WaitGlobalCheckpointAdvance(WatchContext::new(), "compacted", 10),
        Ok(())
    );
}

#[derive(Default)]
struct MetadataFaultKV {
    inner: MemEtcd,
    get_errors: std::sync::Mutex<std::collections::VecDeque<crate::stubs::MetadataRequestError>>,
    get_attempts: std::sync::atomic::AtomicUsize,
    put_attempts: std::sync::atomic::AtomicUsize,
    commit_timeout_once: std::sync::atomic::AtomicBool,
    watch_timeouts: std::sync::atomic::AtomicUsize,
    resets: std::sync::atomic::AtomicUsize,
}
impl EtcdKV for MetadataFaultKV {
    fn Put(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.inner.Put(k, v)
    }
    fn Get(&self, k: &str) -> Result<Vec<u8>, String> {
        self.inner.Get(k)
    }
    fn Delete(&self, k: &str) -> Result<(), String> {
        self.inner.Delete(k)
    }
    fn GetPrefix(&self, p: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        self.inner.GetPrefix(p)
    }
    fn DeletePrefix(&self, p: &str) -> Result<(), String> {
        self.inner.DeletePrefix(p)
    }
    fn GetWithRevision(&self, k: &str) -> Result<crate::stubs::RevisionedValue, String> {
        self.inner.GetWithRevision(k)
    }
    fn GetPrefixWithRevision(&self, p: &str) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, i64), String> {
        self.inner.GetPrefixWithRevision(p)
    }
    fn WatchPrefix(
        &self,
        p: &str,
        r: i64,
    ) -> Result<mpsc::Receiver<crate::stubs::WatchEvent>, String> {
        self.inner.WatchPrefix(p, r)
    }
    fn RequestWatchProgress(&self) -> Result<(), String> {
        self.inner.RequestWatchProgress()
    }
    fn GetWithRequestContext(
        &self,
        ctx: &crate::stubs::MetadataRequestContext,
        key: &str,
    ) -> Result<crate::stubs::RevisionedValue, crate::stubs::MetadataRequestError> {
        self.get_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ctx.check()?;
        if let Some(error) = self.get_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        self.inner
            .GetWithRevision(key)
            .map_err(crate::stubs::MetadataRequestError::Other)
    }
    fn PutWithRequestContext(
        &self,
        ctx: &crate::stubs::MetadataRequestContext,
        key: &str,
        value: &[u8],
    ) -> Result<(), crate::stubs::MetadataRequestError> {
        self.put_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ctx.check()?;
        self.inner
            .Put(key, value)
            .map_err(crate::stubs::MetadataRequestError::Other)?;
        if self
            .commit_timeout_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(crate::stubs::MetadataRequestError::DeadlineExceeded);
        }
        Ok(())
    }
    fn WatchPrefixWithRequestContext(
        &self,
        ctx: &crate::stubs::MetadataRequestContext,
        key: &str,
        revision: i64,
    ) -> Result<mpsc::Receiver<crate::stubs::WatchEvent>, crate::stubs::MetadataRequestError> {
        ctx.check()?;
        if self
            .watch_timeouts
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |value| value.checked_sub(1),
            )
            .is_ok()
        {
            return Err(crate::stubs::MetadataRequestError::DeadlineExceeded);
        }
        let watch = self
            .inner
            .WatchPrefix(key, revision)
            .map_err(crate::stubs::MetadataRequestError::Other)?;
        self.inner.Put(key, &encodeUint64(40)).unwrap();
        Ok(watch)
    }
    fn ResetWatcher(&self) -> Result<(), String> {
        self.resets
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.ResetWatcher()
    }
}
#[test]
fn metadata_requests_retry_only_deadline_and_unavailable_and_preserve_committed_put() {
    use crate::stubs::MetadataRequestError::{DeadlineExceeded, Other, Unavailable};
    use std::sync::atomic::Ordering::SeqCst;
    let kv = Arc::new(MetadataFaultKV::default());
    kv.Put(&GlobalCheckpointOf("retry"), &encodeUint64(20))
        .unwrap();
    kv.get_errors
        .lock()
        .unwrap()
        .extend([DeadlineExceeded, Unavailable("PD unavailable".into())]);
    let meta = NewMetaDataClient(kv.clone());
    assert_eq!(
        meta.WaitGlobalCheckpointAdvance(WatchContext::new(), "retry", 10),
        Ok(())
    );
    assert_eq!(kv.get_attempts.load(SeqCst), 3);
    kv.get_errors
        .lock()
        .unwrap()
        .push_back(Other("permission denied".into()));
    assert_eq!(
        meta.WaitGlobalCheckpointAdvance(WatchContext::new(), "retry", 10),
        Err("permission denied".into())
    );
    assert_eq!(kv.get_attempts.load(SeqCst), 4);
    kv.commit_timeout_once.store(true, SeqCst);
    let ext = AdvancerExt { meta };
    ext.UploadV3GlobalCheckpointForTask("retry", 30).unwrap();
    assert_eq!(kv.put_attempts.load(SeqCst), 2);
    assert_eq!(ext.GetGlobalCheckpointForTask("retry").unwrap(), 30);
    assert_eq!(lastCheckpointMetric("retry"), Some(30));
    ext.UploadV3GlobalCheckpointForTask("retry", 25).unwrap();
    assert_eq!(kv.put_attempts.load(SeqCst), 2);
    assert_eq!(ext.GetGlobalCheckpointForTask("retry").unwrap(), 30);
}
#[test]
fn checkpoint_watch_creation_resets_each_timed_out_watcher_and_retries() {
    use std::sync::atomic::Ordering::SeqCst;
    let kv = Arc::new(MetadataFaultKV::default());
    kv.Put(&GlobalCheckpointOf("watch-retry"), &encodeUint64(10))
        .unwrap();
    kv.watch_timeouts.store(2, SeqCst);
    let meta = NewMetaDataClient(kv.clone());
    assert_eq!(
        meta.WaitGlobalCheckpointAdvance(WatchContext::new(), "watch-retry", 10),
        Ok(())
    );
    assert_eq!(kv.resets.load(SeqCst), 2);
    kv.watch_timeouts.store(3, SeqCst);
    assert_eq!(
        meta.WaitGlobalCheckpointAdvance(WatchContext::new(), "watch-retry", 40),
        Err("PiTR checkpoint watch restart required".into())
    );
    assert_eq!(kv.resets.load(SeqCst), 5);
}
#[test]
fn metadata_deadlines_and_parent_cancellation_join_workers() {
    use crate::advancer_cliext::runMetadataRequestWithRetry;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let active = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let worker_active = active.clone();
    let worker_attempts = attempts.clone();
    let result: Result<(), String> = runMetadataRequestWithRetry(
        &WatchContext::new(),
        &[Duration::from_millis(10); 3],
        None,
        move |ctx| {
            worker_attempts.fetch_add(1, SeqCst);
            worker_active.fetch_add(1, SeqCst);
            while ctx.check().is_ok() {
                std::thread::sleep(Duration::from_millis(1));
            }
            worker_active.fetch_sub(1, SeqCst);
            ctx.check()
        },
    );
    assert_eq!(result, Err("context deadline exceeded".into()));
    assert_eq!(attempts.load(SeqCst), 3);
    assert_eq!(active.load(SeqCst), 0);
    let parent = WatchContext::new();
    let cancel = parent.clone();
    let worker_active = active.clone();
    let worker_attempts = attempts.clone();
    let result: Result<(), String> =
        runMetadataRequestWithRetry(&parent, &[Duration::from_secs(5); 3], None, move |ctx| {
            worker_attempts.fetch_add(1, SeqCst);
            worker_active.fetch_add(1, SeqCst);
            cancel.cancel();
            let result = ctx.check();
            worker_active.fetch_sub(1, SeqCst);
            result
        });
    assert_eq!(result, Err("watch canceled".into()));
    assert_eq!(attempts.load(SeqCst), 4);
    assert_eq!(active.load(SeqCst), 0);
}

// Only the watch transport is replaced: snapshot and event conversion use the real client.
struct ClosingWatchKV {
    inner: MemEtcd,
    ctx: WatchContext,
    cancel_on_watch: bool,
    cancel_on_conversion: bool,
    closed_prefix: String,
}
impl EtcdKV for ClosingWatchKV {
    fn Put(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.inner.Put(k, v)
    }
    fn Get(&self, k: &str) -> Result<Vec<u8>, String> {
        self.inner.Get(k)
    }
    fn Delete(&self, k: &str) -> Result<(), String> {
        self.inner.Delete(k)
    }
    fn GetPrefix(&self, p: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        if self.cancel_on_conversion && p == crate::RangesOf("buffered") {
            self.ctx.cancel();
        }
        self.inner.GetPrefix(p)
    }
    fn DeletePrefix(&self, p: &str) -> Result<(), String> {
        self.inner.DeletePrefix(p)
    }
    fn GetWithRevision(&self, k: &str) -> Result<crate::stubs::RevisionedValue, String> {
        self.inner.GetWithRevision(k)
    }
    fn GetPrefixWithRevision(&self, p: &str) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, i64), String> {
        self.inner.GetPrefixWithRevision(p)
    }
    fn RequestWatchProgress(&self) -> Result<(), String> {
        Ok(())
    }
    fn WatchPrefix(&self, p: &str, r: i64) -> Result<mpsc::Receiver<crate::WatchEvent>, String> {
        let (tx, rx) = mpsc::channel();
        if self.cancel_on_conversion && p == crate::PrefixOfTask() {
            tx.send(crate::WatchEvent {
                Type: crate::WatchEventType::Put,
                Key: TaskOf("buffered").into_bytes(),
                Value: serde_json::to_vec(&task("buffered").PBInfo).unwrap(),
                ModRevision: r,
            })
            .unwrap();
        }
        if self.cancel_on_watch || self.cancel_on_conversion {
            tx.send(crate::WatchEvent {
                Type: crate::WatchEventType::Delete,
                Key: format!("{p}buffered").into_bytes(),
                Value: Vec::new(),
                ModRevision: r,
            })
            .unwrap();
            if self.cancel_on_watch {
                self.ctx.cancel();
            }
        } else if p != self.closed_prefix {
            return self.inner.WatchPrefix(p, r);
        }
        Ok(rx)
    }
}

#[test]
fn canceled_listener_drains_both_closed_watches_before_cancel_error() {
    let ctx = WatchContext::new();
    let kv = Arc::new(ClosingWatchKV {
        inner: MemEtcd::new(),
        ctx: ctx.clone(),
        cancel_on_watch: true,
        cancel_on_conversion: false,
        closed_prefix: String::new(),
    });
    let meta = NewMetaDataClient(kv);
    meta.PutTask(&task("snapshot")).unwrap();
    let (tx, rx) = mpsc::channel();
    AdvancerExt { meta }.Begin(ctx, tx).unwrap();
    assert_eq!(recv(&rx).Name, "snapshot");
    for expected in [EventType::EventDel, EventType::EventResume] {
        let event = recv(&rx);
        assert_eq!(event.Type, expected);
        assert_eq!(event.Name, "buffered");
    }
    let error = recv(&rx);
    assert_eq!(error.Type, EventType::EventErr);
    assert_eq!(error.Err.as_deref(), Some("watch canceled"));
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn closed_task_or_pause_watch_without_cancellation_reports_eof() {
    for prefix in [crate::PrefixOfTask(), crate::PrefixOfPause()] {
        let ctx = WatchContext::new();
        let meta = NewMetaDataClient(Arc::new(ClosingWatchKV {
            inner: MemEtcd::new(),
            ctx: ctx.clone(),
            cancel_on_watch: false,
            cancel_on_conversion: false,
            closed_prefix: prefix,
        }));
        let (tx, rx) = mpsc::channel();
        AdvancerExt { meta }.Begin(ctx, tx).unwrap();
        let error = recv(&rx);
        assert_eq!(error.Type, EventType::EventErr);
        assert_eq!(error.Err.as_deref(), Some("EOF"));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}

#[test]
fn cancellation_when_task_watch_closes_drains_pause_watch() {
    let ctx = WatchContext::new();
    let meta = NewMetaDataClient(Arc::new(ClosingWatchKV {
        inner: MemEtcd::new(),
        ctx: ctx.clone(),
        cancel_on_watch: false,
        cancel_on_conversion: true,
        closed_prefix: String::new(),
    }));
    let (tx, rx) = mpsc::channel();
    AdvancerExt { meta }.Begin(ctx, tx).unwrap();
    assert_eq!(recv(&rx).Type, EventType::EventAdd);
    assert_eq!(recv(&rx).Type, EventType::EventDel);
    assert_eq!(recv(&rx).Type, EventType::EventResume);
    let error = recv(&rx);
    assert_eq!(error.Err.as_deref(), Some("watch canceled"));
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}
