// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//! Adapt the existing task register state machine to a real metadata connection.
use crate::register::*;
use crate::stubs::context::Context;
use astersql_errors::SharedError;
use astersql_metaservice::{Context as MetadataContext, NamespacedEtcdClient};
use std::{sync::mpsc, thread, time::Duration};
#[derive(Clone)]
pub struct MetadataRegisterClient(pub NamespacedEtcdClient);
impl MetadataRegisterClient {
    fn client(&self, ctx: &Context) -> NamespacedEtcdClient {
        let ctx = ctx.clone();
        self.0
            .with_context(MetadataContext::with_cancellation_checker(move || {
                ctx.is_cancelled()
            }))
    }
}
impl EtcdRegisterClient for MetadataRegisterClient {
    fn put(&self, ctx: &Context, key: &str, value: &str, lease: i64) -> Result<(), SharedError> {
        self.client(ctx)
            .put(key, value.as_bytes().to_vec(), Some(lease))
            .map_err(SharedError::new)
    }
    fn grant(&self, ctx: &Context, ttl: i64) -> Result<LeaseGrantResponse, SharedError> {
        self.client(ctx)
            .grant(ttl)
            .map(|id| LeaseGrantResponse {
                id,
                ttl,
                error: String::new(),
            })
            .map_err(SharedError::new)
    }
    fn keep_alive(
        &self,
        ctx: &Context,
        lease: i64,
    ) -> Result<mpsc::Receiver<LeaseKeepAliveResponse>, SharedError> {
        let client = self.client(ctx);
        client.keepalive(lease).map_err(SharedError::new)?;
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            loop {
                let ttl = match client.time_to_live(lease) {
                    Ok(ttl) if ttl > 0 => ttl,
                    _ => break,
                };
                if sender
                    .send(LeaseKeepAliveResponse { id: lease, ttl })
                    .is_err()
                {
                    break;
                }
                if ctx.wait_cancelled_timeout(Duration::from_secs((ttl / 3).max(1) as u64)) {
                    break;
                }
                if client.keepalive(lease).is_err() {
                    break;
                }
            }
        });
        Ok(receiver)
    }
    fn keep_alive_once(&self, ctx: &Context, lease: i64) -> Result<(), SharedError> {
        self.client(ctx).keepalive(lease).map_err(SharedError::new)
    }
    fn get(&self, ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SharedError> {
        self.client(ctx)
            .get_entries(key, prefix)
            .map(|entries| GetResponse {
                kvs: entries
                    .into_iter()
                    .map(|entry| KeyValue {
                        key: String::from_utf8_lossy(&entry.key).into_owned(),
                        value: String::from_utf8_lossy(&entry.value).into_owned(),
                        lease: entry.lease,
                    })
                    .collect(),
            })
            .map_err(SharedError::new)
    }
    fn revoke(&self, ctx: &Context, lease: i64) -> Result<(), SharedError> {
        self.client(ctx).revoke(lease).map_err(|error| {
            if error.to_string().contains("requested lease not found") {
                SharedError::new(LeaseNotFound)
            } else {
                SharedError::new(error)
            }
        })
    }
    fn time_to_live(
        &self,
        ctx: &Context,
        lease: i64,
    ) -> Result<LeaseTimeToLiveResponse, SharedError> {
        self.client(ctx)
            .time_to_live(lease)
            .map(|ttl| LeaseTimeToLiveResponse { ttl })
            .map_err(SharedError::new)
    }
}
