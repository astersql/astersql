// Copyright 2026 AsterSQL.

//! Go-compatible etcd transport for the shared TableTimerStore notifier.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use astersql_timer_api::{self as api, WatchTimerEvent, WatchTimerResponse};
use astersql_timer_tablestore::{EtcdClient, EtcdNotifyEvent};

pub(super) struct RealTimerEtcdClient {
    client: etcd_client::Client,
    namespace: String,
    stopped: Arc<AtomicBool>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl RealTimerEtcdClient {
    pub(super) fn new(client: etcd_client::Client, namespace: String) -> Self {
        Self {
            client,
            namespace,
            stopped: Arc::new(AtomicBool::new(false)),
            workers: Mutex::new(Vec::new()),
        }
    }

    fn key(&self, key: &str) -> String {
        format!("{}{key}", self.namespace)
    }

    fn runtime() -> Result<tokio::runtime::Runtime, String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("start timer etcd runtime: {error}"))
    }

    fn track(&self, handle: JoinHandle<()>) {
        let mut workers = self
            .workers
            .lock()
            .expect("timer etcd workers lock poisoned");
        let mut active = Vec::new();
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        active.push(handle);
        *workers = active;
    }
}

pub(super) fn decode_timer_notify_message(value: &[u8]) -> Result<WatchTimerResponse, String> {
    let message: serde_json::Value =
        serde_json::from_slice(value).map_err(|error| format!("decode timer notice: {error}"))?;
    let events = message["events"]
        .as_array()
        .ok_or_else(|| "timer notice missing events".to_owned())?
        .iter()
        .filter_map(|event| {
            let timer_id = event["timer_id"].as_str().filter(|id| !id.is_empty())?;
            let tp = match event["tp"].as_str() {
                Some("create") => api::WatchTimerEventCreate,
                Some("update") => api::WatchTimerEventUpdate,
                Some("delete") => api::WatchTimerEventDelete,
                _ => return None,
            };
            Some(WatchTimerEvent {
                Tp: tp,
                TimerID: timer_id.to_owned(),
            })
        })
        .collect();
    Ok(WatchTimerResponse { Events: events })
}

pub(super) fn encode_timer_notify_message(events: &[EtcdNotifyEvent]) -> String {
    serde_json::json!({
        "events": events.iter().map(|event| serde_json::json!({
            "tp": event.tp,
            "timer_id": event.timer_id,
            "timestamp": event.timestamp,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

impl EtcdClient for RealTimerEtcdClient {
    fn grant(&self, ttl_seconds: i64) -> Result<i64, String> {
        let mut client = self.client.clone();
        Self::runtime()?
            .block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    client.lease_grant(ttl_seconds, None),
                )
                .await
            })
            .map_err(|error| format!("grant timer etcd lease timed out: {error}"))?
            .map(|lease| lease.id())
            .map_err(|error| format!("grant timer etcd lease: {error}"))
    }

    fn keep_alive(&self, lease_id: i64) -> Result<mpsc::Receiver<()>, String> {
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let mut client = self.client.clone();
        let stopped = Arc::clone(&self.stopped);
        let handle = std::thread::Builder::new()
            .name("ttl-timer-etcd-lease".into())
            .spawn(move || {
                let Ok(runtime) = Self::runtime() else {
                    let _ = ready_sender.send(Err("start timer etcd runtime failed".to_owned()));
                    return;
                };
                runtime.block_on(async move {
                    let (mut keeper, mut stream) = match tokio::time::timeout(
                        Duration::from_secs(5),
                        client.lease_keep_alive(lease_id),
                    )
                    .await
                    {
                        Ok(Ok(lease)) => lease,
                        other => {
                            let _ = ready_sender.send(Err(format!("keep timer lease: {other:?}")));
                            return;
                        }
                    };
                    let _ = ready_sender.send(Ok(()));
                    while !stopped.load(Ordering::Acquire) {
                        for _ in 0..100 {
                            if stopped.load(Ordering::Acquire) {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        if stopped.load(Ordering::Acquire)
                            || !matches!(
                                tokio::time::timeout(Duration::from_secs(5), keeper.keep_alive())
                                    .await,
                                Ok(Ok(()))
                            )
                            || !matches!(
                                tokio::time::timeout(Duration::from_secs(3), stream.message())
                                    .await,
                                Ok(Ok(Some(_)))
                            )
                        {
                            break;
                        }
                        if sender.send(()).is_err() {
                            break;
                        }
                    }
                });
            })
            .map_err(|error| format!("start timer etcd lease worker: {error}"))?;
        self.track(handle);
        ready_receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("wait for timer etcd lease: {error}"))??;
        Ok(receiver)
    }

    fn watch_prefix(&self, prefix: &str, ctx: &api::Context) -> api::WatchTimerChan {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let key = self.key(prefix);
        let mut client = self.client.clone();
        let stopped = Arc::clone(&self.stopped);
        let ctx = ctx.clone();
        if let Ok(handle) = std::thread::Builder::new()
            .name("ttl-timer-etcd-watch".into())
            .spawn(move || {
                let Ok(runtime) = Self::runtime() else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(Ok(mut stream)) = tokio::time::timeout(
                        Duration::from_secs(5),
                        client.watch(key, Some(etcd_client::WatchOptions::new().with_prefix())),
                    )
                    .await
                    else {
                        return;
                    };
                    while !stopped.load(Ordering::Acquire) && !ctx.is_cancelled() {
                        match tokio::time::timeout(Duration::from_millis(100), stream.message())
                            .await
                        {
                            Ok(Ok(Some(response))) if !response.canceled() => {
                                for event in response.events() {
                                    if event.event_type() != etcd_client::EventType::Put {
                                        continue;
                                    }
                                    if let Some(kv) = event.kv() {
                                        if let Ok(notice) = decode_timer_notify_message(kv.value())
                                        {
                                            if sender.send(notice).is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(Ok(Some(_))) | Ok(Ok(None)) | Ok(Err(_)) => return,
                            Err(_) => {}
                        }
                    }
                });
            })
        {
            self.track(handle);
        }
        receiver
    }

    fn put_events(
        &self,
        key: &str,
        events: &[EtcdNotifyEvent],
        lease_id: i64,
        timeout: Duration,
    ) -> Result<(), String> {
        let key = self.key(key);
        let value = encode_timer_notify_message(events);
        let mut client = self.client.clone();
        Self::runtime()?
            .block_on(async {
                tokio::time::timeout(
                    timeout,
                    client.put(
                        key,
                        value,
                        Some(etcd_client::PutOptions::new().with_lease(lease_id)),
                    ),
                )
                .await
            })
            .map_err(|error| format!("timer notice put timed out: {error}"))?
            .map(|_| ())
            .map_err(|error| format!("put timer notice: {error}"))
    }
}

impl Drop for RealTimerEtcdClient {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        for worker in self
            .workers
            .lock()
            .expect("timer etcd workers lock poisoned")
            .drain(..)
        {
            let _ = worker.join();
        }
    }
}
