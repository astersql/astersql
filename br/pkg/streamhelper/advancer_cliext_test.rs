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
