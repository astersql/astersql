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

use crate::{
    consistenthash::ConsistentHash,
    time_proxy::{Clock, current_clock},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub id: String,
    pub ip: String,
    pub port: u32,
    pub start_timestamp: i64,
}
/// Cancellation is broadcast by disconnecting the sender; every subscriber wakes.
#[derive(Clone)]
pub struct Cancellation {
    tx: Arc<Mutex<Option<Sender<()>>>>,
    rx: Receiver<()>,
}
impl Default for Cancellation {
    fn default() -> Self {
        let (tx, rx) = bounded(0);
        Self {
            tx: Arc::new(Mutex::new(Some(tx))),
            rx,
        }
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.tx.lock().unwrap().take();
    }
    pub fn done(&self) -> Receiver<()> {
        self.rx.clone()
    }
    pub fn is_canceled(&self) -> bool {
        self.tx.lock().unwrap().is_none()
    }
}
pub trait ServerDiscovery: Send + Sync {
    fn server_filter(&self, _info: &ServerInfo) -> bool {
        true
    }
    fn get_server_info(&self) -> Result<ServerInfo, String>;
    fn get_all_server_info(
        &self,
        ctx: &Cancellation,
    ) -> Result<BTreeMap<String, ServerInfo>, String>;
}
pub(crate) struct Membership {
    pub id: String,
    pub servers: BTreeMap<String, ServerInfo>,
    pub server_ids: Vec<String>,
    pub hash: ConsistentHash,
}
pub struct ServerConsistentHash {
    pub(crate) membership: RwLock<Membership>,
    helper: Arc<dyn ServerDiscovery>,
    ctx: Cancellation,
    clock: Clock,
}
impl ServerConsistentHash {
    pub fn new(ctx: Cancellation, replicas: isize, helper: Arc<dyn ServerDiscovery>) -> Self {
        Self::with_clock(ctx, replicas, helper, current_clock())
    }
    pub fn with_clock(
        ctx: Cancellation,
        replicas: isize,
        helper: Arc<dyn ServerDiscovery>,
        clock: Clock,
    ) -> Self {
        Self {
            membership: RwLock::new(Membership {
                id: String::new(),
                servers: BTreeMap::new(),
                server_ids: Vec::new(),
                hash: ConsistentHash::new(replicas),
            }),
            helper,
            ctx,
            clock,
        }
    }
    pub fn init(&self) -> bool {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.helper.get_server_info() {
                Ok(info) => {
                    self.membership.write().unwrap().id = info.id;
                    return true;
                }
                Err(err) => log::warn!("get local TiDB server info failed: {err}"),
            }
            let wait = backoff.min(Duration::from_secs(5));
            let cancel = self.ctx.done();
            let after = self.clock.after(wait);
            crossbeam_channel::select! {
                recv(cancel) -> _ => return false,
                recv(after) -> _ => { backoff = wait * 2; },
            }
        }
    }
    pub fn refresh(&self) -> Result<bool, String> {
        let old_count = self.membership.read().unwrap().servers.len();
        let mut infos = match self.helper.get_all_server_info(&self.ctx) {
            Ok(infos) => infos,
            Err(err) => {
                log::warn!(
                    "get available TiDB nodes failed; old_server_count={old_count}; canceled={}; error={err}",
                    self.ctx.is_canceled()
                );
                return Err(err);
            }
        };
        infos.retain(|_, v| self.helper.server_filter(v));
        let mut m = self.membership.write().unwrap();
        if m.servers.keys().eq(infos.keys()) {
            return Ok(false);
        }
        m.server_ids = infos.keys().filter(|id| !id.is_empty()).cloned().collect();
        m.hash.rebuild(infos.keys().map(String::as_str));
        m.servers = infos;
        Ok(true)
    }
    pub fn id(&self) -> String {
        self.membership.read().unwrap().id.clone()
    }
    pub fn server_ids(&self) -> Vec<String> {
        self.membership.read().unwrap().server_ids.clone()
    }
    pub fn available_bytes(&self, key: &[u8]) -> bool {
        let m = self.membership.read().unwrap();
        m.hash.get_node(key) == m.id
    }
    pub fn available_string(&self, key: &str) -> bool {
        self.available_bytes(key.as_bytes())
    }
    pub fn available_int64(&self, key: i64) -> bool {
        self.available_bytes(&key.to_be_bytes())
    }
    pub fn available(&self, key: &dyn Any) -> bool {
        if let Some(v) = key.downcast_ref::<&str>() {
            self.available_string(v)
        } else if let Some(v) = key.downcast_ref::<&[u8]>() {
            self.available_bytes(v)
        } else if let Some(v) = key.downcast_ref::<String>() {
            self.available_string(v)
        } else if let Some(v) = key.downcast_ref::<Vec<u8>>() {
            self.available_bytes(v)
        } else if let Some(v) = key.downcast_ref::<i64>() {
            self.available_int64(*v)
        } else {
            false
        }
    }
}
