// Copyright 2026 AsterSQL.

//! Canonical mock transactions backed by the embedded TiKV RPC/MVCC server.

use astersql_kv as kv;
use astersql_store_mockstore_unistore::{self as unistore, RPCClient, Request, Response};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use unistore::tikv::mvcc::{Mutation, MutationOp, PrewriteRequest};
use unistore::tikv::region::RequestContext;
use unistore::tikv::server::{RpcContext, RpcResponse};

pub struct EmbeddedRpcStore {
    client: Arc<RPCClient>,
    cluster: Arc<unistore::Cluster>,
}

fn error(message: impl ToString) -> kv::errors::SharedError {
    kv::errors::New(message.to_string())
}

fn value<T>(response: RpcResponse<T>) -> Result<T, kv::errors::SharedError> {
    if let Some(region) = response.region_error {
        return Err(error(region));
    }
    if let Some(key) = response.key_error {
        return Err(if key.retryable {
            kv::ErrTxnRetryable.FastGenByArgs(&[])
        } else {
            error(key.message)
        });
    }
    response
        .value
        .ok_or_else(|| error("embedded RPC returned no response value"))
}

impl EmbeddedRpcStore {
    pub(crate) fn new() -> crate::Result<Self> {
        let (client, _pd, cluster) =
            unistore::mock::New("", Vec::new(), unistore::pd::NULL_KEYSPACE_ID, Vec::new())
                .map_err(|err| crate::MockStorageError::Rpc(err.to_string()))?;
        Ok(Self { client, cluster })
    }

    pub fn client(&self) -> Arc<RPCClient> {
        self.client.clone()
    }

    pub(crate) fn close(&self) -> crate::Result<()> {
        self.client
            .close()
            .map_err(|err| crate::MockStorageError::Rpc(err.to_string()))
    }

    fn context(
        &self,
        key: &[u8],
        priority: i32,
        marker: Option<u64>,
    ) -> Result<(String, RpcContext), kv::errors::SharedError> {
        let manager = self.cluster.region_manager();
        let region = manager
            .get_region_by_key(key)
            .ok_or_else(|| error("embedded RPC region missing"))?;
        let leader = region
            .leader
            .ok_or_else(|| error("embedded RPC region has no leader"))?;
        let address = manager
            .all_stores()
            .into_iter()
            .find(|store| store.id == leader.store_id)
            .ok_or_else(|| error("embedded RPC store missing"))?
            .address;
        Ok((
            address,
            RpcContext {
                region: RequestContext {
                    region_id: region.meta.id,
                    store_id: Some(leader.store_id),
                    epoch: Some(region.meta.epoch),
                },
                priority,
                request_marker: marker,
                ..Default::default()
            },
        ))
    }

    fn send(&self, address: &str, request: Request) -> Result<Response, kv::errors::SharedError> {
        self.client
            .send_request(address, request, Duration::from_secs(5))
            .map_err(error)
    }

    pub(crate) fn get(
        &self,
        key: &[u8],
        version: u64,
        priority: i32,
        marker: Option<u64>,
    ) -> Result<Option<Vec<u8>>, kv::errors::SharedError> {
        let (address, context) = self.context(key, priority, marker)?;
        match self.send(
            &address,
            Request::Get {
                context,
                key: key.to_vec(),
                version,
            },
        )? {
            Response::Get(response) => value(response),
            _ => Err(error("unexpected embedded Get response")),
        }
    }

    pub(crate) fn scan(
        &self,
        version: u64,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        reverse: bool,
        priority: i32,
        marker: Option<u64>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, kv::errors::SharedError> {
        let lower = lower.unwrap_or_default();
        let regions = self.cluster.region_manager().scan_regions(
            lower,
            upper.unwrap_or_default(),
            usize::MAX,
        );
        let mut rows = Vec::new();
        for region in regions {
            let start = lower.max(region.meta.start_key.as_slice()).to_vec();
            let end = match (upper, region.meta.end_key.is_empty()) {
                (Some(upper), false) => upper.min(region.meta.end_key.as_slice()).to_vec(),
                (Some(upper), true) => upper.to_vec(),
                (None, _) => region.meta.end_key.clone(),
            };
            if !end.is_empty() && start >= end {
                continue;
            }
            let (address, context) = self.context(&start, priority, marker)?;
            match self.send(
                &address,
                Request::Scan {
                    context,
                    start,
                    end,
                    version,
                    limit: usize::MAX,
                    reverse: false,
                    key_only: false,
                },
            )? {
                Response::Scan(response) => {
                    for pair in value(response)? {
                        if let Some(err) = pair.error {
                            return Err(error(err));
                        }
                        rows.push((pair.key, pair.value));
                    }
                }
                _ => return Err(error("unexpected embedded Scan response")),
            }
        }
        if reverse {
            rows.reverse();
        }
        Ok(rows)
    }

    fn groups(
        &self,
        keys: impl IntoIterator<Item = Vec<u8>>,
        priority: i32,
        marker: Option<u64>,
    ) -> Result<BTreeMap<u64, (String, RpcContext, Vec<Vec<u8>>)>, kv::errors::SharedError> {
        let mut groups = BTreeMap::new();
        for key in keys {
            let (address, context) = self.context(&key, priority, marker)?;
            groups
                .entry(context.region.region_id)
                .or_insert_with(|| (address, context, Vec::new()))
                .2
                .push(key);
        }
        Ok(groups)
    }

    pub(crate) fn rollback(
        &self,
        start_ts: u64,
        keys: Vec<Vec<u8>>,
        priority: i32,
        marker: Option<u64>,
    ) -> Result<(), kv::errors::SharedError> {
        for (_, (address, context, keys)) in self.groups(keys, priority, marker)? {
            match self.send(
                &address,
                Request::BatchRollback {
                    context,
                    keys,
                    start_ts,
                },
            )? {
                Response::Unit(response) => value(response)?,
                _ => return Err(error("unexpected embedded rollback response")),
            }
        }
        Ok(())
    }

    pub(crate) fn commit(
        &self,
        start_ts: u64,
        commit_ts: u64,
        writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        priority: i32,
        marker: Option<u64>,
    ) -> Result<(), kv::errors::SharedError> {
        let Some(primary) = writes.keys().next() else {
            return Ok(());
        };
        let groups = self.groups(writes.keys().cloned(), priority, marker)?;
        for (address, context, keys) in groups.values() {
            let mutations = keys
                .iter()
                .map(|key| Mutation {
                    op: if writes[key].is_some() {
                        MutationOp::Put
                    } else {
                        MutationOp::Delete
                    },
                    key: key.clone(),
                    value: writes[key].clone().unwrap_or_default(),
                    is_pessimistic_lock: false,
                })
                .collect();
            let request = PrewriteRequest {
                mutations,
                primary_lock: primary.clone(),
                start_ts,
                lock_ttl: 3000,
                ..Default::default()
            };
            let prewrite = match self.send(
                address,
                Request::Prewrite {
                    context: context.clone(),
                    request,
                },
            ) {
                Ok(Response::Prewrite(response)) => value(response).map(|_| ()),
                Ok(_) => Err(error("unexpected embedded Prewrite response")),
                Err(err) => Err(err),
            };
            if let Err(err) = prewrite {
                self.rollback(start_ts, writes.keys().cloned().collect(), priority, marker)
                    .map_err(|cleanup| error(format!("{err}; rollback failed: {cleanup}")))?;
                return Err(err);
            }
        }
        // Commit the primary first, so a secondary error cannot turn a committed
        // transaction into a rollback. Resolve secondaries with the same commit_ts.
        let (address, context) = self.context(primary, priority, marker)?;
        match self.send(
            &address,
            Request::Commit {
                context,
                keys: vec![primary.clone()],
                start_ts,
                commit_ts,
            },
        )? {
            Response::Unit(response) => value(response)?,
            _ => return Err(error("unexpected embedded Commit response")),
        }
        for (_, (address, context, keys)) in groups {
            let keys = keys
                .into_iter()
                .filter(|key| key != primary)
                .collect::<Vec<_>>();
            if keys.is_empty() {
                continue;
            }
            match self.send(
                &address,
                Request::Commit {
                    context,
                    keys,
                    start_ts,
                    commit_ts,
                },
            )? {
                Response::Unit(response) => value(response)?,
                _ => return Err(error("unexpected embedded secondary Commit response")),
            }
        }
        Ok(())
    }
}
