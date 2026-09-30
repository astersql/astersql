// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! `tikv/client-rust` 到 canonical `astersql-kv` 的事务与快照适配。

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use astersql_kv as kv;
use astersql_store_copr as copr;
use astersql_store_driver_txn::{ClientTransactionMode, canonical_value, map_client_error};
use kv::{MemBuffer as _, Retriever as _};
use tikv_client::{
    BoundRange, KvPair, Snapshot as ClientSnapshotHandle, Timestamp, TimestampExt,
    Transaction as ClientTransactionHandle,
};

use crate::{ClientRuntime, TikvStore};

type SharedRuntime = Arc<RwLock<ClientRuntime>>;
pub(crate) type ScanPage = Box<
    dyn FnMut(
        Vec<u8>,
        Option<Vec<u8>>,
        bool,
        u32,
    ) -> Result<ScanPageResult, kv::errors::SharedError>,
>;

pub(crate) const CLIENT_SCAN_PAGE_SIZE: u32 = 256;

pub(crate) struct ScanPageResult {
    pub(crate) entries: Vec<(kv::Key, Vec<u8>)>,
    pub(crate) lower_bound: Vec<u8>,
    pub(crate) upper_bound: Option<Vec<u8>>,
    pub(crate) exhausted: bool,
}

pub(crate) fn configured_scan_batch_size(value: &dyn Any) -> Option<u32> {
    let value = value
        .downcast_ref::<usize>()
        .and_then(|value| u32::try_from(*value).ok())
        .or_else(|| value.downcast_ref::<u32>().copied())
        .or_else(|| {
            value
                .downcast_ref::<i32>()
                .and_then(|value| u32::try_from(*value).ok())
        })
        .or_else(|| {
            value
                .downcast_ref::<u64>()
                .and_then(|value| u32::try_from(*value).ok())
        })?;
    // client-go newScanner resets 0 and 1 to DefaultScanBatchSize.
    Some(if value <= 1 {
        CLIENT_SCAN_PAGE_SIZE
    } else {
        value
    })
}

pub(crate) fn snapshot_timestamp(
    version: u64,
    current_timestamp: impl FnOnce() -> Result<u64, kv::errors::SharedError>,
) -> Result<Timestamp, kv::errors::SharedError> {
    // MaxVersion means a latest committed snapshot, not a literal far-future
    // timestamp. Clamping it to i64::MAX sends a normal scan TSO that violates
    // TiKV's max_ts bound and can terminate the server. Resolve the sentinel
    // once through PD; retain that timestamp for every page of this snapshot.
    let version = if version == kv::MaxVersion.Ver {
        current_timestamp()?
    } else {
        version
    };
    if version > i64::MAX as u64 {
        return Err(adapter_error(
            "snapshot timestamp exceeds client-rust's signed range",
        ));
    }
    Ok(Timestamp::from_version(version))
}

fn adapter_error(error: impl ToString) -> kv::errors::SharedError {
    kv::errors::New(error.to_string())
}

fn tiflash_http_client(
    store: &TikvStore,
) -> Result<(reqwest::blocking::Client, &'static str), kv::errors::SharedError> {
    let mut builder = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10));
    let scheme = if let Some(tls) = store.TLSConfig() {
        let ca = std::fs::read(&tls.ca_path).map_err(adapter_error)?;
        let cert = std::fs::read(&tls.cert_path).map_err(adapter_error)?;
        let key = std::fs::read(&tls.key_path).map_err(adapter_error)?;
        let mut identity = cert;
        identity.extend_from_slice(&key);
        builder = builder
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).map_err(adapter_error)?)
            .identity(reqwest::Identity::from_pem(&identity).map_err(adapter_error)?);
        "https"
    } else {
        "http"
    };
    Ok((builder.build().map_err(adapter_error)?, scheme))
}

fn runtime_error() -> kv::errors::SharedError {
    adapter_error("TiKV store was opened without the client-rust runtime")
}

fn key_name(key: &kv::Key) -> String {
    kv::KeyMapName(key.as_ref())
}

fn pairs(iterator: impl Iterator<Item = KvPair>) -> Vec<(kv::Key, Vec<u8>)> {
    iterator
        .map(|pair| {
            let (key, value): (tikv_client::Key, Vec<u8>) = pair.into();
            (kv::Key(key.into()), value)
        })
        .collect()
}

fn key_pairs(iterator: impl Iterator<Item = tikv_client::Key>) -> Vec<(kv::Key, Vec<u8>)> {
    iterator
        .map(|key| {
            let key: Vec<u8> = key.into();
            (kv::Key(key), Vec::new())
        })
        .collect()
}

pub(crate) fn option_enabled(options: &HashMap<i32, Box<dyn Any>>, option: i32) -> bool {
    options
        .get(&option)
        .and_then(|value| value.downcast_ref::<bool>())
        .copied()
        .unwrap_or(false)
}

/// canonical 迭代器；内存/事务缓冲区可直接物化，远端 TiKV 扫描则分页按需拉取。
pub(crate) struct ClientIterator {
    entries: Vec<(kv::Key, Vec<u8>)>,
    position: usize,
    paging: Option<PagingState>,
}

struct PagingState {
    scan_page: ScanPage,
    lower_bound: Vec<u8>,
    upper_bound: Option<Vec<u8>>,
    reverse: bool,
    page_size: u32,
    exhausted: bool,
}

impl ClientIterator {
    fn new(entries: Vec<(kv::Key, Vec<u8>)>) -> Self {
        Self {
            entries,
            position: 0,
            paging: None,
        }
    }

    pub(crate) fn paged(
        scan_page: ScanPage,
        lower_bound: Vec<u8>,
        upper_bound: Option<Vec<u8>>,
        reverse: bool,
        page_size: u32,
    ) -> Result<Self, kv::errors::SharedError> {
        let mut iterator = Self {
            entries: Vec::new(),
            position: 0,
            paging: Some(PagingState {
                scan_page,
                lower_bound,
                upper_bound,
                reverse,
                page_size,
                exhausted: false,
            }),
        };
        iterator.fetch_page()?;
        Ok(iterator)
    }

    fn fetch_page(&mut self) -> Result<(), kv::errors::SharedError> {
        let Some(paging) = self.paging.as_mut() else {
            return Ok(());
        };
        if paging.exhausted {
            self.entries.clear();
            self.position = 0;
            return Ok(());
        }

        loop {
            let page = (paging.scan_page)(
                paging.lower_bound.clone(),
                paging.upper_bound.clone(),
                paging.reverse,
                paging.page_size,
            )?;
            if page.entries.len() > paging.page_size as usize {
                return Err(adapter_error("TiKV scan exceeded its requested batch size"));
            }
            paging.lower_bound = page.lower_bound;
            paging.upper_bound = page.upper_bound;
            paging.exhausted = page.exhausted;
            self.entries = page.entries;
            self.position = 0;
            if !self.entries.is_empty() || paging.exhausted {
                return Ok(());
            }
        }
    }
}

impl kv::Iterator for ClientIterator {
    fn Valid(&self) -> bool {
        self.position < self.entries.len()
    }

    fn Key(&self) -> kv::Key {
        self.entries
            .get(self.position)
            .map(|entry| entry.0.clone())
            .unwrap_or_default()
    }

    fn Value(&self) -> Vec<u8> {
        self.entries
            .get(self.position)
            .map(|entry| entry.1.clone())
            .unwrap_or_default()
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        if !self.Valid() {
            return Err(adapter_error("iterator is invalid"));
        }
        self.position += 1;
        if self.position == self.entries.len() && self.paging.is_some() {
            self.fetch_page()?;
        }
        Ok(())
    }

    fn Close(&mut self) {
        self.entries.clear();
        self.position = self.entries.len();
        if let Some(paging) = self.paging.as_mut() {
            paging.exhausted = true;
        }
    }
}

#[derive(Clone, Default)]
struct BufferState {
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    flags: HashMap<Vec<u8>, kv::KeyFlags>,
    stages: Vec<(kv::StagingHandle, BTreeMap<Vec<u8>, Option<Vec<u8>>>)>,
    next_stage: kv::StagingHandle,
}

/// canonical MemBuffer 的本地镜像；真实提交仍由 client-rust Transaction 承载。
#[derive(Default)]
struct ClientMemBuffer {
    state: RwLock<BufferState>,
}

impl ClientMemBuffer {
    fn record_set(&self, key: &[u8], value: &[u8]) {
        self.state
            .write()
            .unwrap()
            .writes
            .insert(key.to_vec(), Some(value.to_vec()));
    }

    fn record_delete(&self, key: &[u8]) {
        self.state
            .write()
            .unwrap()
            .writes
            .insert(key.to_vec(), None);
    }

    fn entries(&self, reverse: bool) -> Vec<(kv::Key, Vec<u8>)> {
        let state = self.state.read().unwrap();
        let mut entries = state
            .writes
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_ref()
                    .map(|value| (kv::Key(key.clone()), value.clone()))
            })
            .collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        entries
    }
}

impl kv::Getter for ClientMemBuffer {
    fn Get(
        &self,
        _ctx: &kv::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        self.state
            .read()
            .unwrap()
            .writes
            .get(key.as_ref())
            .and_then(Clone::clone)
            .map(|value| kv::NewValueEntry(value, 0))
            .ok_or_else(|| kv::ErrNotExist.FastGenByArgs(&[]))
    }
}

impl kv::Retriever for ClientMemBuffer {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let entries = self
            .entries(false)
            .into_iter()
            .filter(|(candidate, _)| {
                candidate.as_ref() >= key.as_ref()
                    && upper_bound
                        .as_ref()
                        .is_none_or(|upper| candidate.as_ref() < upper.as_ref())
            })
            .collect();
        Ok(Box::new(ClientIterator::new(entries)))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let entries = self
            .entries(true)
            .into_iter()
            .filter(|(candidate, _)| {
                key.as_ref()
                    .is_none_or(|upper| candidate.as_ref() < upper.as_ref())
                    && lower_bound
                        .as_ref()
                        .is_none_or(|lower| candidate.as_ref() >= lower.as_ref())
            })
            .collect();
        Ok(Box::new(ClientIterator::new(entries)))
    }
}

impl kv::Mutator for ClientMemBuffer {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), kv::errors::SharedError> {
        if value.is_empty() {
            return Err(kv::ErrCannotSetNilValue.FastGenByArgs(&[]));
        }
        self.record_set(key.as_ref(), &value);
        Ok(())
    }

    fn Delete(&mut self, key: kv::Key) -> Result<(), kv::errors::SharedError> {
        self.record_delete(key.as_ref());
        Ok(())
    }
}

impl kv::RetrieverMutator for ClientMemBuffer {}

impl kv::MemBuffer for ClientMemBuffer {
    fn RLock(&self) {}

    fn RUnlock(&self) {}

    fn GetFlags(&self, key: &kv::Key) -> Result<kv::KeyFlags, kv::errors::SharedError> {
        Ok(self
            .state
            .read()
            .unwrap()
            .flags
            .get(key.as_ref())
            .copied()
            .unwrap_or_default())
    }

    fn SetWithFlags(
        &mut self,
        key: kv::Key,
        value: Vec<u8>,
        operations: &[kv::FlagsOp],
    ) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Set(self, key.clone(), value)?;
        self.UpdateFlags(key, operations);
        Ok(())
    }

    fn UpdateFlags(&mut self, key: kv::Key, operations: &[kv::FlagsOp]) {
        let mut state = self.state.write().unwrap();
        let current = state.flags.get(key.as_ref()).copied().unwrap_or_default();
        state
            .flags
            .insert(key.0, kv::ApplyFlagsOps(current, operations));
    }

    fn UpdateAssertionFlags(&mut self, key: kv::Key, operation: kv::AssertionOp) {
        let mut state = self.state.write().unwrap();
        let current = state.flags.get(key.as_ref()).copied().unwrap_or_default();
        state
            .flags
            .insert(key.0, kv::ApplyAssertionOp(current, operation));
    }

    fn DeleteWithFlags(
        &mut self,
        key: kv::Key,
        operations: &[kv::FlagsOp],
    ) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Delete(self, key.clone())?;
        self.UpdateFlags(key, operations);
        Ok(())
    }

    fn Staging(&mut self) -> kv::StagingHandle {
        let mut state = self.state.write().unwrap();
        state.next_stage += 1;
        let handle = state.next_stage;
        let writes = state.writes.clone();
        state.stages.push((handle, writes));
        handle
    }

    fn Release(&mut self, handle: kv::StagingHandle) {
        let mut state = self.state.write().unwrap();
        if let Some(index) = state.stages.iter().position(|stage| stage.0 == handle) {
            state.stages.remove(index);
        }
    }

    fn Cleanup(&mut self, handle: kv::StagingHandle) {
        let mut state = self.state.write().unwrap();
        if let Some(index) = state.stages.iter().position(|stage| stage.0 == handle) {
            state.writes = state.stages[index].1.clone();
            state.stages.truncate(index);
        }
    }

    fn InspectStage(
        &self,
        handle: kv::StagingHandle,
        callback: &mut dyn FnMut(kv::Key, kv::KeyFlags, Vec<u8>),
    ) {
        let state = self.state.read().unwrap();
        let baseline = state
            .stages
            .iter()
            .find(|stage| stage.0 == handle)
            .map(|stage| &stage.1);
        for (key, value) in &state.writes {
            if baseline.is_some_and(|writes| writes.get(key) == Some(value)) {
                continue;
            }
            callback(
                kv::Key(key.clone()),
                state.flags.get(key).copied().unwrap_or_default(),
                value.clone().unwrap_or_default(),
            );
        }
    }

    fn SnapshotGetter(&self) -> Box<dyn kv::Getter> {
        Box::new(ClientMemBuffer {
            state: RwLock::new(self.state.read().unwrap().clone()),
        })
    }

    fn SnapshotIter(&self, key: kv::Key, upper_bound: Option<kv::Key>) -> Box<dyn kv::Iterator> {
        self.Iter(key, upper_bound)
            .unwrap_or_else(|_| Box::new(ClientIterator::new(Vec::new())))
    }

    fn SnapshotIterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Box<dyn kv::Iterator> {
        self.IterReverse(key, lower_bound)
            .unwrap_or_else(|_| Box::new(ClientIterator::new(Vec::new())))
    }

    fn Len(&self) -> usize {
        self.state.read().unwrap().writes.len()
    }

    fn Size(&self) -> usize {
        self.state
            .read()
            .unwrap()
            .writes
            .iter()
            .map(|(key, value)| key.len() + value.as_ref().map_or(0, Vec::len))
            .sum()
    }

    fn RemoveFromBuffer(&mut self, key: kv::Key) {
        let mut state = self.state.write().unwrap();
        state.writes.remove(key.as_ref());
        state.flags.remove(key.as_ref());
    }

    fn GetLocal(&self, ctx: &kv::Context, key: &[u8]) -> Result<Vec<u8>, kv::errors::SharedError> {
        kv::Getter::Get(self, ctx, kv::Key(key.to_vec()), &[]).map(|entry| entry.Value)
    }

    fn BatchGet(
        &self,
        ctx: &kv::Context,
        keys: &[Vec<u8>],
        options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        let get_options = kv::BatchGetToGetOptions(options.to_vec()).unwrap_or_default();
        Ok(keys
            .iter()
            .filter_map(|key| {
                kv::Getter::Get(self, ctx, kv::Key(key.clone()), &get_options)
                    .ok()
                    .map(|value| (kv::KeyMapName(key), value))
            })
            .collect())
    }
}

/// 指定 TSO 的官方只读快照。
struct ClientSnapshot {
    runtime: SharedRuntime,
    handle: Arc<Mutex<ClientSnapshotHandle>>,
    coprocessor_store: Arc<copr::Store>,
    scan_batch_size: AtomicU32,
    options: Mutex<HashMap<i32, Box<dyn Any>>>,
}

pub(crate) fn scan_region_page<L, S>(
    mut locate_region: L,
    mut scan_region: S,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
    reverse: bool,
    limit: u32,
) -> Result<ScanPageResult, kv::errors::SharedError>
where
    L: FnMut(&[u8], bool) -> Result<copr::KeyLocation, kv::errors::SharedError>,
    S: FnMut(
        Vec<u8>,
        Option<Vec<u8>>,
        bool,
        u32,
    ) -> Result<Vec<(kv::Key, Vec<u8>)>, kv::errors::SharedError>,
{
    if limit == 0 {
        return Ok(ScanPageResult {
            entries: Vec::new(),
            lower_bound: start,
            upper_bound: end,
            exhausted: true,
        });
    }

    if reverse {
        let lower_bound = start;
        if end
            .as_ref()
            .is_some_and(|upper| lower_bound.as_slice() >= upper.as_slice())
        {
            return Ok(ScanPageResult {
                entries: Vec::new(),
                lower_bound,
                upper_bound: end,
                exhausted: true,
            });
        }
        let location = locate_region(end.as_deref().unwrap_or_default(), true)?;
        let region_start = if location.start_key > lower_bound {
            location.start_key
        } else {
            lower_bound.clone()
        };
        if end
            .as_ref()
            .is_some_and(|upper| region_start.as_slice() >= upper.as_slice())
        {
            return Err(adapter_error(
                "PD LocateEndKey returned a non-progressing Region",
            ));
        }
        let entries = scan_region(region_start.clone(), end.clone(), true, limit)?;
        let full_page = entries.len() == limit as usize;
        let next_upper_bound = if full_page {
            entries.last().map(|entry| entry.0.0.clone())
        } else {
            Some(region_start.clone())
        };
        let exhausted = next_upper_bound
            .as_ref()
            .is_some_and(|upper| lower_bound.as_slice() >= upper.as_slice())
            || (!full_page && region_start.is_empty());
        return Ok(ScanPageResult {
            entries,
            lower_bound,
            upper_bound: next_upper_bound,
            exhausted,
        });
    }

    if end
        .as_ref()
        .is_some_and(|upper| start.as_slice() >= upper.as_slice())
    {
        return Ok(ScanPageResult {
            entries: Vec::new(),
            lower_bound: start,
            upper_bound: end,
            exhausted: true,
        });
    }
    let location = locate_region(&start, false)?;
    if !location.contains_start(&start) {
        return Err(adapter_error(
            "PD LocateKey returned a Region that does not contain the scan key",
        ));
    }
    let region_end = match (location.end_key.is_empty(), end.as_ref()) {
        (true, None) => None,
        (true, Some(end)) => Some(end.clone()),
        (false, None) => Some(location.end_key),
        (false, Some(end)) => Some(location.end_key.min(end.clone())),
    };
    if region_end
        .as_ref()
        .is_some_and(|upper| start.as_slice() >= upper.as_slice())
    {
        return Err(adapter_error(
            "PD LocateKey returned a non-progressing Region",
        ));
    }
    let entries = scan_region(start.clone(), region_end.clone(), false, limit)?;
    let full_page = entries.len() == limit as usize;
    let next_lower_bound = if full_page {
        entries
            .last()
            .map(|entry| entry.0.Next().0)
            .unwrap_or(start)
    } else {
        region_end.clone().unwrap_or(start)
    };
    let exhausted = end
        .as_ref()
        .is_some_and(|upper| next_lower_bound.as_slice() >= upper.as_slice())
        || (!full_page && region_end.is_none());
    Ok(ScanPageResult {
        entries,
        lower_bound: next_lower_bound,
        upper_bound: end,
        exhausted,
    })
}

fn locate_scan_region(
    store: &copr::Store,
    key: &[u8],
    reverse: bool,
) -> Result<copr::KeyLocation, kv::errors::SharedError> {
    let region_cache = store.kv_store().region_cache();
    if reverse {
        region_cache.locate_end_key(key)
    } else {
        region_cache.locate_key(key)
    }
    .map_err(adapter_error)
}

fn scan_snapshot_page(
    runtime: &SharedRuntime,
    handle: &Arc<Mutex<ClientSnapshotHandle>>,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
    reverse: bool,
    limit: u32,
    key_only: bool,
) -> Result<Vec<(kv::Key, Vec<u8>)>, kv::errors::SharedError> {
    let guard = runtime.read().map_err(adapter_error)?;
    let mut snapshot = handle.lock().map_err(adapter_error)?;
    let range = BoundRange::new(
        Bound::Included(start.into()),
        end.map_or(Bound::Unbounded, |end| Bound::Excluded(end.into())),
    );
    let result = if reverse && key_only {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.scan_keys_reverse(range, limit))
            .map(|keys| key_pairs(keys))
    } else if reverse {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.scan_reverse(range, limit))
            .map(|values| pairs(values))
    } else if key_only {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.scan_keys(range, limit))
            .map(|keys| key_pairs(keys))
    } else {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.scan(range, limit))
            .map(|values| pairs(values))
    };
    result.map_err(map_client_error)
}

fn scan_transaction_page(
    runtime: &SharedRuntime,
    handle: &Arc<Mutex<ClientTransactionHandle>>,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
    reverse: bool,
    limit: u32,
    key_only: bool,
) -> Result<Vec<(kv::Key, Vec<u8>)>, kv::errors::SharedError> {
    let guard = runtime.read().map_err(adapter_error)?;
    let mut transaction = handle.lock().map_err(adapter_error)?;
    let range = BoundRange::new(
        Bound::Included(start.into()),
        end.map_or(Bound::Unbounded, |end| Bound::Excluded(end.into())),
    );
    let result = if reverse && key_only {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.scan_keys_reverse(range, limit))
            .map(|keys| key_pairs(keys))
    } else if reverse {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.scan_reverse(range, limit))
            .map(|values| pairs(values))
    } else if key_only {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.scan_keys(range, limit))
            .map(|keys| key_pairs(keys))
    } else {
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.scan(range, limit))
            .map(|values| pairs(values))
    };
    result.map_err(map_client_error)
}

impl ClientSnapshot {
    fn new(
        runtime: SharedRuntime,
        coprocessor_store: Arc<copr::Store>,
        version: u64,
    ) -> Result<Self, kv::errors::SharedError> {
        crate::read_request::install_read_queue_delay();
        let guard = runtime.read().map_err(adapter_error)?;
        let client = guard.transaction_client().map_err(adapter_error)?;
        let handle = client.snapshot(
            snapshot_timestamp(version, || guard.current_timestamp().map_err(adapter_error))?,
            ClientTransactionMode::Optimistic.options(),
        );
        drop(guard);
        Ok(Self {
            runtime,
            handle: Arc::new(Mutex::new(handle)),
            coprocessor_store,
            scan_batch_size: AtomicU32::new(CLIENT_SCAN_PAGE_SIZE),
            options: Mutex::new(HashMap::new()),
        })
    }
}

impl kv::Getter for ClientSnapshot {
    fn Get(
        &self,
        _ctx: &kv::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut snapshot = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.get(key.0))
            .map_err(map_client_error)
            .and_then(canonical_value)
    }
}

impl kv::Retriever for ClientSnapshot {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let runtime = Arc::clone(&self.runtime);
        let handle = Arc::clone(&self.handle);
        let coprocessor_store = Arc::clone(&self.coprocessor_store);
        let key_only = option_enabled(&self.options.lock().unwrap(), kv::KeyOnly);
        let scan_page = Box::new(move |start, end, reverse, limit| {
            scan_region_page(
                |key, reverse| locate_scan_region(&coprocessor_store, key, reverse),
                |start, end, reverse, limit| {
                    scan_snapshot_page(&runtime, &handle, start, end, reverse, limit, key_only)
                },
                start,
                end,
                reverse,
                limit,
            )
        });
        Ok(Box::new(ClientIterator::paged(
            scan_page,
            key.0,
            upper_bound.map(|key| key.0),
            false,
            self.scan_batch_size.load(Ordering::Acquire),
        )?))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let runtime = Arc::clone(&self.runtime);
        let handle = Arc::clone(&self.handle);
        let coprocessor_store = Arc::clone(&self.coprocessor_store);
        let key_only = option_enabled(&self.options.lock().unwrap(), kv::KeyOnly);
        let scan_page = Box::new(move |start, end, reverse, limit| {
            scan_region_page(
                |key, reverse| locate_scan_region(&coprocessor_store, key, reverse),
                |start, end, reverse, limit| {
                    scan_snapshot_page(&runtime, &handle, start, end, reverse, limit, key_only)
                },
                start,
                end,
                reverse,
                limit,
            )
        });
        Ok(Box::new(ClientIterator::paged(
            scan_page,
            lower_bound.map_or_else(Vec::new, |key| key.0),
            key.map(|key| key.0),
            true,
            self.scan_batch_size.load(Ordering::Acquire),
        )?))
    }
}

impl kv::Snapshot for ClientSnapshot {
    fn BatchGet(
        &self,
        _ctx: &kv::Context,
        keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut snapshot = self.handle.lock().map_err(adapter_error)?;
        let values = guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(snapshot.batch_get(keys.iter().map(|key| key.0.clone())))
            .map_err(map_client_error)?;
        Ok(pairs(values)
            .into_iter()
            .map(|(key, value)| (key_name(&key), kv::NewValueEntry(value, 0)))
            .collect())
    }

    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        if option == kv::ScanBatchSize {
            let page_size = value
                .as_deref()
                .and_then(configured_scan_batch_size)
                .unwrap_or(CLIENT_SCAN_PAGE_SIZE);
            self.scan_batch_size.store(page_size, Ordering::Release);
        }
        let mut options = self.options.lock().unwrap();
        if let Some(value) = value {
            options.insert(option, value);
        } else {
            options.remove(&option);
        }
        let timeout = options
            .get(&kv::TiKVClientReadTimeout)
            .and_then(|value| value.downcast_ref::<u64>())
            .copied()
            .unwrap_or_default();
        let stats = options
            .get(&kv::CollectRuntimeStats)
            .and_then(|value| value.downcast_ref::<Arc<crate::ReadStats>>())
            .cloned();
        let policy = if timeout != 0 || stats.is_some() {
            Some(crate::ReadOptions {
                timeout: std::time::Duration::from_millis(timeout),
                stats: stats.unwrap_or_default(),
            })
        } else {
            None
        };
        self.handle.lock().unwrap().set_read_options(policy);
    }
}

// Storage::GetSnapshot cannot return a Result. Preserve oracle failures on
// the snapshot and surface them through every fallible read rather than panic.
pub(crate) struct FailedSnapshot(pub kv::errors::SharedError);
impl kv::Getter for FailedSnapshot {
    fn Get(
        &self,
        _: &kv::Context,
        _: kv::Key,
        _: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        Err(self.0.clone())
    }
}
impl kv::Retriever for FailedSnapshot {
    fn Iter(
        &self,
        _: kv::Key,
        _: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Err(self.0.clone())
    }
    fn IterReverse(
        &self,
        _: Option<kv::Key>,
        _: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Err(self.0.clone())
    }
}
impl kv::Snapshot for FailedSnapshot {
    fn BatchGet(
        &self,
        _: &kv::Context,
        _: &[kv::Key],
        _: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        Err(self.0.clone())
    }
    fn SetOption(&mut self, _: i32, _: Option<Box<dyn Any>>) {}
}

/// 官方读写事务。所有网络操作在 Store 的单一 Tokio runtime 上执行。
struct ClientTransaction {
    runtime: SharedRuntime,
    handle: Arc<Mutex<ClientTransactionHandle>>,
    snapshot: ClientSnapshot,
    mem_buffer: ClientMemBuffer,
    mode: ClientTransactionMode,
    start_ts: u64,
    commit_ts: u64,
    valid: bool,
    options: HashMap<i32, Box<dyn Any>>,
    vars: Box<dyn Any>,
    memory_hook: Option<Box<dyn Fn(u64)>>,
    table_info: HashMap<i64, kv::model::TableInfo>,
    checkpoint: kv::tikv::MemDBCheckpoint,
    scan_batch_size: u32,
    statement_stages: Vec<(kv::StagingHandle, kv::StagingHandle, i32)>,
    next_statement_stage: kv::StagingHandle,
}

pub(crate) fn begin_transaction(
    store: &TikvStore,
    mode: ClientTransactionMode,
) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError> {
    let runtime = store.client_runtime().ok_or_else(runtime_error)?;
    let guard = runtime.read().map_err(adapter_error)?;
    let client = guard.transaction_client().map_err(adapter_error)?;
    let transaction = guard
        .runtime()
        .map_err(adapter_error)?
        .block_on(client.begin_with_options(mode.options()))
        .map_err(map_client_error)?;
    let start_ts = transaction.start_timestamp().version();
    drop(guard);
    let coprocessor_store = store
        .coprocessor_store()
        .ok_or_else(|| adapter_error("TiKV store has no RegionCache"))?;
    let snapshot = ClientSnapshot::new(
        Arc::clone(&runtime),
        Arc::clone(&coprocessor_store),
        start_ts,
    )?;
    Ok(Box::new(ClientTransaction {
        runtime,
        handle: Arc::new(Mutex::new(transaction)),
        snapshot,
        mem_buffer: ClientMemBuffer::default(),
        mode,
        start_ts,
        commit_ts: 0,
        valid: true,
        options: HashMap::new(),
        vars: Box::new(()),
        memory_hook: None,
        table_info: HashMap::new(),
        checkpoint: kv::tikv::MemDBCheckpoint,
        scan_batch_size: CLIENT_SCAN_PAGE_SIZE,
        statement_stages: Vec::new(),
        next_statement_stage: 0,
    }))
}

impl kv::Getter for ClientTransaction {
    fn Get(
        &self,
        _ctx: &kv::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.get(key.0))
            .map_err(map_client_error)
            .and_then(canonical_value)
    }
}

impl kv::Retriever for ClientTransaction {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let runtime = Arc::clone(&self.runtime);
        let handle = Arc::clone(&self.handle);
        let coprocessor_store = Arc::clone(&self.snapshot.coprocessor_store);
        let key_only = option_enabled(&self.options, kv::KeyOnly);
        let scan_page = Box::new(move |start, end, reverse, limit| {
            scan_region_page(
                |key, reverse| locate_scan_region(&coprocessor_store, key, reverse),
                |start, end, reverse, limit| {
                    scan_transaction_page(&runtime, &handle, start, end, reverse, limit, key_only)
                },
                start,
                end,
                reverse,
                limit,
            )
        });
        Ok(Box::new(ClientIterator::paged(
            scan_page,
            key.0,
            upper_bound.map(|key| key.0),
            false,
            self.scan_batch_size,
        )?))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let runtime = Arc::clone(&self.runtime);
        let handle = Arc::clone(&self.handle);
        let coprocessor_store = Arc::clone(&self.snapshot.coprocessor_store);
        let key_only = option_enabled(&self.options, kv::KeyOnly);
        let scan_page = Box::new(move |start, end, reverse, limit| {
            scan_region_page(
                |key, reverse| locate_scan_region(&coprocessor_store, key, reverse),
                |start, end, reverse, limit| {
                    scan_transaction_page(&runtime, &handle, start, end, reverse, limit, key_only)
                },
                start,
                end,
                reverse,
                limit,
            )
        });
        Ok(Box::new(ClientIterator::paged(
            scan_page,
            lower_bound.map_or_else(Vec::new, |key| key.0),
            key.map(|key| key.0),
            true,
            self.scan_batch_size,
        )?))
    }
}

impl kv::Mutator for ClientTransaction {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), kv::errors::SharedError> {
        if value.is_empty() {
            return Err(kv::ErrCannotSetNilValue.FastGenByArgs(&[]));
        }
        if let Some(limits) = self
            .options
            .get(&kv::SizeLimits)
            .and_then(|value| value.downcast_ref::<kv::TxnSizeLimits>())
        {
            if (key.0.len() + value.len()) as u64 > limits.Entry {
                return Err(adapter_error("transaction entry is too large"));
            }
            if (self.mem_buffer.Size() + key.0.len() + value.len()) as u64 > limits.Total {
                return Err(adapter_error("transaction is too large"));
            }
        }
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.stage_put(key.0.clone(), value.clone()))
            .map_err(map_client_error)?;
        self.mem_buffer.record_set(key.as_ref(), &value);
        if let Some(hook) = &self.memory_hook {
            hook(self.mem_buffer.Size() as u64);
        }
        Ok(())
    }

    fn Delete(&mut self, key: kv::Key) -> Result<(), kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.stage_delete(key.0.clone()))
            .map_err(map_client_error)?;
        self.mem_buffer.record_delete(key.as_ref());
        if let Some(hook) = &self.memory_hook {
            hook(self.mem_buffer.Size() as u64);
        }
        Ok(())
    }
}

impl kv::RetrieverMutator for ClientTransaction {}

impl kv::FairLockingController for ClientTransaction {
    fn StartFairLocking(&mut self) -> Result<(), kv::errors::SharedError> {
        self.handle
            .lock()
            .map_err(adapter_error)?
            .start_fair_locking();
        Ok(())
    }

    fn RetryFairLocking(&mut self, _ctx: &kv::Context) -> Result<(), kv::errors::SharedError> {
        self.handle
            .lock()
            .map_err(adapter_error)?
            .retry_fair_locking();
        Ok(())
    }

    fn CancelFairLocking(&mut self, _ctx: &kv::Context) -> Result<(), kv::errors::SharedError> {
        self.handle
            .lock()
            .map_err(adapter_error)?
            .cancel_fair_locking();
        Ok(())
    }

    fn DoneFairLocking(&mut self, _ctx: &kv::Context) -> Result<(), kv::errors::SharedError> {
        self.handle
            .lock()
            .map_err(adapter_error)?
            .done_fair_locking();
        Ok(())
    }

    fn IsInFairLockingMode(&self) -> bool {
        self.handle
            .lock()
            .map(|transaction| transaction.is_in_fair_locking_mode())
            .unwrap_or(false)
    }
}

impl kv::Transaction for ClientTransaction {
    fn StageStatement(&mut self) -> Result<kv::StagingHandle, kv::errors::SharedError> {
        let client_stage = self.handle.lock().map_err(adapter_error)?.stage_statement();
        let local_stage = kv::MemBuffer::Staging(&mut self.mem_buffer);
        self.next_statement_stage += 1;
        let stage = self.next_statement_stage;
        self.statement_stages
            .push((stage, local_stage, client_stage));
        Ok(stage)
    }

    fn ReleaseStatement(
        &mut self,
        stage: kv::StagingHandle,
    ) -> Result<(), kv::errors::SharedError> {
        let (_, local_stage, client_stage) = self
            .statement_stages
            .last()
            .copied()
            .filter(|entry| entry.0 == stage)
            .ok_or_else(|| adapter_error("statement stage is not active"))?;
        self.handle
            .lock()
            .map_err(adapter_error)?
            .release_statement(client_stage)
            .map_err(map_client_error)?;
        kv::MemBuffer::Release(&mut self.mem_buffer, local_stage);
        self.statement_stages.pop();
        Ok(())
    }

    fn CleanupStatement(
        &mut self,
        stage: kv::StagingHandle,
    ) -> Result<(), kv::errors::SharedError> {
        let (_, local_stage, client_stage) = self
            .statement_stages
            .last()
            .copied()
            .filter(|entry| entry.0 == stage)
            .ok_or_else(|| adapter_error("statement stage is not active"))?;
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.cleanup_statement(client_stage))
            .map_err(map_client_error)?;
        kv::MemBuffer::Cleanup(&mut self.mem_buffer, local_stage);
        self.statement_stages.pop();
        Ok(())
    }
    fn Size(&self) -> usize {
        self.mem_buffer.Size()
    }

    fn Mem(&self) -> u64 {
        self.mem_buffer.Size() as u64
    }

    fn SetMemoryFootprintChangeHook(&mut self, hook: Box<dyn Fn(u64)>) {
        self.memory_hook = Some(hook);
    }

    fn MemHookSet(&self) -> bool {
        self.memory_hook.is_some()
    }

    fn Len(&self) -> usize {
        self.mem_buffer.Len()
    }

    fn Commit(&mut self, _ctx: &kv::Context) -> Result<(), kv::errors::SharedError> {
        // TiDB's read-only transaction has no mutations to prewrite. Release
        // pessimistic locks without creating an MVCC Lock write/commit TS.
        if self.mode == ClientTransactionMode::Pessimistic && self.mem_buffer.Len() == 0 {
            self.Rollback()?;
            self.commit_ts = 0;
            return Ok(());
        }
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        self.commit_ts = guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.commit())
            .map_err(map_client_error)?
            .map_or(0, |timestamp| timestamp.version());
        self.valid = false;
        Ok(())
    }

    fn Rollback(&mut self) -> Result<(), kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.rollback())
            .map_err(map_client_error)?;
        self.valid = false;
        Ok(())
    }

    fn String(&self) -> String {
        format!("client-rust-{}-txn-{}", self.mode.as_str(), self.start_ts)
    }

    fn LockKeys(
        &mut self,
        _ctx: &kv::Context,
        lock_ctx: &mut kv::LockCtx,
        keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        let runtime = guard.runtime().map_err(adapter_error)?;
        let result = if lock_ctx.Shared {
            runtime
                .block_on(transaction.lock_shared_keys_with_wait_timeout(
                    keys.iter().map(|key| key.0.clone()),
                    lock_ctx.WaitTimeoutMs,
                ))
                .map(|_| tikv_client::transaction::FairLockDetails::default())
        } else {
            runtime.block_on(transaction.lock_keys_with_wait_timeout_and_details(
                keys.iter().map(|key| key.0.clone()),
                lock_ctx.WaitTimeoutMs,
            ))
        };
        let details = result.map_err(map_client_error)?;
        lock_ctx.AggressiveLockNewCount +=
            i32::try_from(details.aggressive_lock_new_count).unwrap_or(i32::MAX);
        lock_ctx.AggressiveLockDerivedCount +=
            i32::try_from(details.aggressive_lock_derived_count).unwrap_or(i32::MAX);
        lock_ctx.LockedWithConflictCount +=
            i32::try_from(details.locked_with_conflict_count).unwrap_or(i32::MAX);
        Ok(())
    }

    fn LockKeysFunc(
        &mut self,
        ctx: &kv::Context,
        lock_ctx: &mut kv::LockCtx,
        callback: &mut dyn FnMut(),
        keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        callback();
        self.LockKeys(ctx, lock_ctx, keys)
    }

    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        if matches!(
            option,
            kv::EnableAsyncCommit | kv::Enable1PC | kv::Pessimistic
        ) {
            let enabled = value
                .as_deref()
                .and_then(|value| value.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let mut transaction = self
                .handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match option {
                kv::EnableAsyncCommit => transaction.set_async_commit(enabled),
                kv::Enable1PC => transaction.set_one_pc(enabled),
                kv::Pessimistic => {
                    transaction.set_pessimistic(enabled);
                    self.mode = if enabled {
                        ClientTransactionMode::Pessimistic
                    } else {
                        ClientTransactionMode::Optimistic
                    };
                }
                _ => unreachable!(),
            }
        }

        if option == kv::ScanBatchSize {
            self.scan_batch_size = value
                .as_deref()
                .and_then(configured_scan_batch_size)
                .unwrap_or(CLIENT_SCAN_PAGE_SIZE);
            self.snapshot
                .scan_batch_size
                .store(self.scan_batch_size, Ordering::Release);
        }
        if let Some(value) = value {
            self.options.insert(option, value);
        } else {
            self.options.remove(&option);
        }
    }

    fn GetOption(&self, option: i32) -> Option<&dyn Any> {
        self.options.get(&option).map(Box::as_ref)
    }

    fn IsReadOnly(&self) -> bool {
        self.mem_buffer.Len() == 0
    }

    fn StartTS(&self) -> u64 {
        self.start_ts
    }

    fn CommitTS(&self) -> u64 {
        self.commit_ts
    }

    fn Valid(&self) -> bool {
        self.valid
    }

    fn GetMemBuffer(&self) -> &dyn kv::MemBuffer {
        &self.mem_buffer
    }

    fn GetSnapshot(&self) -> &dyn kv::Snapshot {
        &self.snapshot
    }

    fn SetVars(&mut self, vars: Box<dyn Any>) {
        self.vars = vars;
    }

    fn GetVars(&self) -> &dyn Any {
        self.vars.as_ref()
    }

    fn BatchGet(
        &self,
        _ctx: &kv::Context,
        keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        let guard = self.runtime.read().map_err(adapter_error)?;
        let mut transaction = self.handle.lock().map_err(adapter_error)?;
        let values = guard
            .runtime()
            .map_err(adapter_error)?
            .block_on(transaction.batch_get(keys.iter().map(|key| key.0.clone())))
            .map_err(map_client_error)?;
        Ok(pairs(values)
            .into_iter()
            .map(|(key, value)| (key_name(&key), kv::NewValueEntry(value, 0)))
            .collect())
    }

    fn IsPessimistic(&self) -> bool {
        self.mode == ClientTransactionMode::Pessimistic
    }

    fn CacheTableInfo(&mut self, id: i64, info: kv::model::TableInfo) {
        self.table_info.insert(id, info);
    }

    fn GetTableInfo(&self, id: i64) -> Option<&kv::model::TableInfo> {
        self.table_info.get(&id)
    }

    fn SetDiskFullOpt(&mut self, level: kv::kvrpcpb::DiskFullOpt) {
        let option = match level {
            kv::kvrpcpb::DiskFullOpt::NotAllowedOnFull => {
                tikv_client::DiskFullOpt::NotAllowedOnFull
            }
            kv::kvrpcpb::DiskFullOpt::AllowedOnAlmostFull => {
                tikv_client::DiskFullOpt::AllowedOnAlmostFull
            }
            kv::kvrpcpb::DiskFullOpt::AllowedOnAlreadyFull => {
                tikv_client::DiskFullOpt::AllowedOnAlreadyFull
            }
        };
        self.handle
            .lock()
            .expect("client transaction lock poisoned")
            .set_disk_full_opt(option);
    }

    fn ClearDiskFullOpt(&mut self) {
        self.handle
            .lock()
            .expect("client transaction lock poisoned")
            .set_disk_full_opt(tikv_client::DiskFullOpt::NotAllowedOnFull);
    }

    fn GetMemDBCheckpoint(&self) -> &kv::tikv::MemDBCheckpoint {
        &self.checkpoint
    }

    fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &kv::tikv::MemDBCheckpoint) {}

    fn IsPipelined(&self) -> bool {
        false
    }

    fn MayFlush(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }
}

struct CopResultSubset {
    data: Vec<u8>,
    start_key: kv::Key,
    memory_size: i64,
    response_time: Duration,
}

impl From<copr::CopResponse> for CopResultSubset {
    fn from(mut response: copr::CopResponse) -> Self {
        let data = response.get_data().to_vec();
        let memory_size = response.mem_size() as i64;
        Self {
            data,
            start_key: kv::Key(response.start_key),
            memory_size,
            response_time: response.response_time,
        }
    }
}

impl From<copr::batch_request_sender::BatchResponse> for CopResultSubset {
    fn from(response: copr::batch_request_sender::BatchResponse) -> Self {
        let memory_size = response.data.len() as i64;
        Self {
            data: response.data,
            start_key: kv::Key(Vec::new()),
            memory_size,
            response_time: Duration::ZERO,
        }
    }
}

impl kv::ResultSubset for CopResultSubset {
    fn GetData(&self) -> &[u8] {
        &self.data
    }

    fn GetStartKey(&self) -> kv::Key {
        self.start_key.clone()
    }

    fn MemSize(&self) -> i64 {
        self.memory_size
    }

    fn RespTime(&self) -> Duration {
        self.response_time
    }
}

struct CopResponse {
    stream: Option<copr::CopResponseStream>,
    pending_error: Option<kv::errors::SharedError>,
    closed: bool,
}

impl CopResponse {
    fn error(error: impl ToString) -> Self {
        Self {
            stream: None,
            pending_error: Some(adapter_error(error)),
            closed: false,
        }
    }
}

impl kv::Response for CopResponse {
    fn Next(
        &mut self,
        _ctx: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
        if self.closed {
            return Ok(None);
        }
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        match stream {
            copr::CopResponseStream::Standard(iterator) => iterator
                .next()
                .map(|response| {
                    response.map(|response| {
                        Box::new(CopResultSubset::from(response)) as Box<dyn kv::ResultSubset>
                    })
                })
                .map_err(adapter_error),
            copr::CopResponseStream::Batch(_) => Err(adapter_error(
                "batch coprocessor response reached the standard TiKV DAG adapter",
            )),
            copr::CopResponseStream::BatchDirect(responses) => match responses.pop_front() {
                Some(response) if !response.other_error.is_empty() => {
                    Err(adapter_error(response.other_error))
                }
                Some(response) => Ok(Some(Box::new(CopResultSubset::from(response)))),
                None => Ok(None),
            },
        }
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if let Some(stream) = self.stream.as_mut() {
            match stream {
                copr::CopResponseStream::Standard(iterator) => iterator.close(),
                copr::CopResponseStream::Batch(iterator) => iterator.close(),
                copr::CopResponseStream::BatchDirect(_) => {}
            }
        }
        self.stream = None;
        Ok(())
    }
}

pub(crate) fn cop_request(req: &kv::Request) -> Result<copr::CopRequest, kv::errors::SharedError> {
    let request_type = match req.Tp {
        kv::ReqTypeDAG => copr::RequestType::Dag,
        kv::ReqTypeAnalyze => copr::RequestType::Analyze,
        kv::ReqTypeChecksum => copr::RequestType::Checksum,
        other => {
            return Err(adapter_error(format!(
                "unsupported coprocessor request type {other}"
            )));
        }
    };
    if !matches!(
        (req.StoreType, req.BatchCop),
        (kv::StoreType::TiKV, false) | (kv::StoreType::TiFlash, true)
    ) {
        return Err(adapter_error(format!(
            "unsupported coprocessor transport for {}",
            req.StoreType.Name()
        )));
    }

    let mut key_ranges = Vec::new();
    if let Some(ranges) = &req.KeyRanges {
        ranges.ForEachPartitionWithErr(|ranges, hints| {
            key_ranges.push(copr::PartitionKeyRanges {
                ranges: ranges
                    .iter()
                    .map(|range| copr::KeyRange {
                        start: range.StartKey.0.clone(),
                        end: range.EndKey.0.clone(),
                    })
                    .collect(),
                row_hints: hints
                    .iter()
                    .map(|hint| usize::try_from(*hint).unwrap_or_default())
                    .collect(),
            });
            Ok(())
        })?;
    }

    let replica_read = match req.ReplicaRead {
        kv::ReplicaReadType::ReplicaReadLeader | kv::ReplicaReadType::ReplicaReadPreferLeader => {
            copr::ReplicaReadType::Leader
        }
        kv::ReplicaReadType::ReplicaReadFollower | kv::ReplicaReadType::ReplicaReadLearner => {
            copr::ReplicaReadType::Follower
        }
        kv::ReplicaReadType::ReplicaReadMixed
        | kv::ReplicaReadType::ReplicaReadClosest
        | kv::ReplicaReadType::ReplicaReadClosestAdaptive => copr::ReplicaReadType::Mixed,
    };
    let isolation_level = match req.IsolationLevel {
        kv::IsoLevel::SI => copr::IsolationLevel::SnapshotIsolation,
        kv::IsoLevel::RC => copr::IsolationLevel::ReadCommitted,
        kv::IsoLevel::RCCheckTS => copr::IsolationLevel::ReadCommittedCheckTs,
    };
    let priority = match req.Priority {
        kv::PriorityLow => copr::Priority::Low,
        kv::PriorityHigh => copr::Priority::High,
        _ => copr::Priority::Normal,
    };

    Ok(copr::CopRequest {
        read_stats: None,
        request_type,
        store_type: if req.StoreType == kv::StoreType::TiFlash {
            copr::StoreType::TiFlash
        } else {
            copr::StoreType::TiKv
        },
        batch_cop: req.BatchCop,
        start_ts: req.StartTs,
        data: req.Data.clone(),
        schema_version: req.SchemaVar,
        key_ranges,
        keep_order: req.KeepOrder,
        descending: req.Desc,
        concurrency: usize::try_from(req.Concurrency).unwrap_or_default().max(1),
        store_batch_size: usize::try_from(req.StoreBatchSize).unwrap_or_default(),
        allow_batch_task_data_merge: req.AllowBatchTaskDataMerge,
        execute_batch_tasks_serially: req.ExecuteBatchTasksSerially,
        replica_read,
        paging: copr::PagingOptions {
            enabled: req.Paging.Enable,
            minimum_size: req.Paging.MinPagingSize,
            maximum_size: req.Paging.MaxPagingSize,
            size_bytes: req.Paging.PagingSizeBytes,
        },
        limit_size: req.LimitSize,
        maximum_execution_time: Duration::from_millis(req.MaxExecutionTime),
        tikv_client_read_timeout: Duration::from_millis(req.TiKVClientReadTimeout),
        store_busy_threshold: req.StoreBusyThreshold,
        // canonical kv::util::RequestSource 当前仍是无字段桩；其余请求语义不降级。
        request_source: copr::RequestSource::default(),
        priority,
        isolation_level,
        not_fill_cache: req.NotFillCache,
        task_id: req.TaskID,
        connection_id: req.ConnID,
        connection_alias: req.ConnAlias.clone(),
        resource_group_name: req.ResourceGroupName.clone(),
        // canonical kv stubs do not yet expose a usable shared limiter/checker
        // object; direct copr callers preserve both through CopRequest.
        copr_request_rate_limit: None,
        copr_request_limiter: req.CoprRequestLimiter.clone(),
        query_cop_store_limiter: req.QueryCopStoreLimiter.clone(),
        resolved_locks: Vec::new(),
        committed_locks: Vec::new(),
        runaway_checker: req
            .RunawayChecker
            .as_ref()
            .map(|checker| crate::runaway_adapter::KVRunawayChecker::new(Arc::clone(checker))),
        resource_control_interceptor: req
            .ResourceControlInterceptor
            .as_ref()
            .map(|interceptor| {
                crate::runaway_adapter::KVCopRUInterceptor::new(Arc::clone(interceptor))
            })
            .or_else(|| {
                req.RunawayChecker.as_ref().map(|_| {
                    Arc::new(copr::ProductionCopRUInterceptor) as Arc<dyn copr::CopRUInterceptor>
                })
            }),
        resource_control_ru: Arc::new(Mutex::new(copr::CopRUDetails::default())),
        is_staleness: req.IsStaleness,
        maximum_keys_read: req.MaxKeysRead,
    })
}

impl kv::Client for TikvStore {
    fn Send(
        &self,
        _ctx: &kv::Context,
        req: &kv::Request,
        vars: &dyn Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        let Some(store) = self.coprocessor_store() else {
            return Some(Box::new(CopResponse::error(
                "coprocessor transport is unavailable for this store",
            )));
        };
        let mut request = match cop_request(req) {
            Ok(request) => request,
            Err(error) => {
                return Some(Box::new(CopResponse {
                    stream: None,
                    pending_error: Some(error),
                    closed: false,
                }));
            }
        };
        request.read_stats = vars.downcast_ref::<Arc<crate::ReadStats>>().cloned();
        Some(match store.get_client().send(request) {
            Ok(stream) => Box::new(CopResponse {
                stream: Some(stream),
                pending_error: None,
                closed: false,
            }) as Box<dyn kv::Response>,
            Err(error) => Box::new(CopResponse::error(error)),
        })
    }

    fn IsRequestTypeSupported(&self, req_type: i64, sub_type: i64) -> bool {
        kv::RequestTypeSupportedChecker.IsRequestTypeSupported(req_type, sub_type)
    }
}

struct UnsupportedMppClient;

impl kv::MPPClient for UnsupportedMppClient {
    fn ConstructMPPTasks(
        &self,
        _ctx: &kv::Context,
        _request: &kv::MPPBuildTasksRequest,
        _timeout: Duration,
        _policy: kv::tiflashcompute::DispatchPolicy,
        _replica_read: kv::tiflash::ReplicaRead,
        _on_error: &mut dyn FnMut(kv::Error),
    ) -> Result<Vec<Box<dyn kv::MPPTaskMeta>>, kv::Error> {
        Ok(Vec::new())
    }

    fn DispatchMPPTask(
        &self,
        _param: kv::DispatchMPPTaskParam<'_>,
    ) -> Result<(kv::DispatchTaskResponse, bool), kv::Error> {
        Err(adapter_error("client-rust adapter has no MPP client"))
    }

    fn EstablishMPPConns(
        &self,
        _param: kv::EstablishMPPConnsParam<'_>,
    ) -> Result<(kv::MPPStreamResponse, bool), kv::Error> {
        Err(adapter_error("client-rust adapter has no MPP client"))
    }

    fn CancelMPPTasks(&self, _param: kv::CancelMPPTasksParam) {}

    fn CheckVisibility(&self, _start_time: u64) -> Result<(), kv::Error> {
        Ok(())
    }

    fn GetMPPStoreCount(&self) -> Result<i32, kv::Error> {
        Ok(0)
    }
}

struct ClientOracle;
impl kv::oracle::Oracle for ClientOracle {}

#[derive(Default)]
struct AdapterMemManager {
    tables: RwLock<HashMap<i64, HashMap<Vec<u8>, Vec<u8>>>>,
}

impl kv::MemManager for AdapterMemManager {
    fn UnionGet(
        &self,
        ctx: &kv::Context,
        table_id: i64,
        snapshot: &dyn kv::Snapshot,
        key: &kv::Key,
    ) -> Result<Vec<u8>, kv::Error> {
        if let Some(value) = self
            .tables
            .read()
            .unwrap()
            .get(&table_id)
            .and_then(|table| table.get(key.as_ref()))
            .cloned()
        {
            return Ok(value);
        }
        let value = kv::GetValue(ctx, snapshot, key.clone())?;
        self.tables
            .write()
            .unwrap()
            .entry(table_id)
            .or_default()
            .insert(key.0.clone(), value.clone());
        Ok(value)
    }

    fn Delete(&self, table_id: i64) {
        self.tables.write().unwrap().remove(&table_id);
    }
}

static MPP_CLIENT: UnsupportedMppClient = UnsupportedMppClient;
static ORACLE: ClientOracle = ClientOracle;

fn mem_manager() -> &'static AdapterMemManager {
    static MEM_MANAGER: OnceLock<AdapterMemManager> = OnceLock::new();
    MEM_MANAGER.get_or_init(AdapterMemManager::default)
}

impl kv::Storage for TikvStore {
    fn ObserveTiFlashReplicaProgress(
        &self,
        table_id: i64,
        replica_count: u64,
    ) -> Result<Option<f64>, kv::errors::SharedError> {
        if replica_count == 0 {
            return Ok(Some(0.0));
        }
        if !self.GetKeyspace().is_empty() {
            return Err(adapter_error(
                "TiFlash progress for named keyspaces is not configured",
            ));
        }
        let (client, scheme) = tiflash_http_client(self)?;
        let addresses = self.GetPDAddrs().map_err(adapter_error)?;
        let mut pd = None;
        for address in addresses {
            let base = if address.contains("://") {
                address.trim_end_matches('/').to_owned()
            } else {
                format!("{scheme}://{address}")
            };
            if client
                .get(format!("{base}/pd/api/v1/stores"))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                pd = Some(base);
                break;
            }
        }
        let pd = pd.ok_or_else(|| adapter_error("PD stores API is unavailable"))?;
        let stores: serde_json::Value = client
            .get(format!("{pd}/pd/api/v1/stores"))
            .send()
            .map_err(adapter_error)?
            .error_for_status()
            .map_err(adapter_error)?
            .json()
            .map_err(adapter_error)?;
        let stores = stores["stores"]
            .as_array()
            .ok_or_else(|| adapter_error("invalid PD stores response"))?;
        let mut start = b"t".to_vec();
        start.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
        start.extend_from_slice(b"_r");
        let mut end = b"t".to_vec();
        let next_id = table_id
            .checked_add(1)
            .ok_or_else(|| adapter_error("invalid table ID"))?;
        end.extend_from_slice(&(next_id as u64 ^ (1_u64 << 63)).to_be_bytes());
        let hex = |bytes: &[u8]| -> String {
            astersql_util_codec::EncodeBytes(Vec::new(), bytes)
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect()
        };
        let stats: serde_json::Value = client
            .get(format!("{pd}/pd/api/v1/stats/region"))
            .query(&[("start_key", hex(&start)), ("end_key", hex(&end))])
            .send()
            .map_err(adapter_error)?
            .error_for_status()
            .map_err(adapter_error)?
            .json()
            .map_err(adapter_error)?;
        let region_count = stats["count"]
            .as_u64()
            .ok_or_else(|| adapter_error("invalid PD region count"))?;
        if region_count == 0 {
            return Ok(Some(0.0));
        }
        let mut peers = 0_u64;
        let mut covered = std::collections::HashSet::new();
        for entry in stores {
            let store = &entry["store"];
            let is_tiflash = store["labels"].as_array().is_some_and(|labels| {
                labels
                    .iter()
                    .any(|label| label["key"] == "engine" && label["value"] == "tiflash")
            });
            if !is_tiflash {
                continue;
            }
            let state = store["state_name"].as_str().unwrap_or_default();
            if state != "Up" && state != "Disconnected" {
                continue;
            }
            let address = store["status_address"]
                .as_str()
                .ok_or_else(|| adapter_error("TiFlash store has no status address"))?;
            let status = client
                .get(format!(
                    "{scheme}://{address}/tiflash/sync-status/keyspace/4294967295/table/{table_id}"
                ))
                .send()
                .map_err(adapter_error)?
                .error_for_status()
                .map_err(adapter_error)?
                .text()
                .map_err(adapter_error)?;
            let mut lines = status.lines();
            let claimed = lines
                .next()
                .unwrap_or_default()
                .trim()
                .parse::<usize>()
                .map_err(adapter_error)?;
            let ids = lines
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(adapter_error)?;
            if ids.len() != claimed {
                return Err(adapter_error("TiFlash sync status region count mismatch"));
            }
            peers += ids.len() as u64;
            covered.extend(ids);
        }
        let denominator = region_count.saturating_mul(replica_count);
        let fraction = (peers as f64 / denominator as f64).min(1.0);
        Ok(Some(if covered.len() as u64 == region_count {
            fraction
        } else {
            fraction.min(0.999_999)
        }))
    }
    fn PublishTiFlashPlacementRule(
        &self,
        table_id: i64,
        count: u64,
        location_labels: &[String],
    ) -> Result<(), kv::errors::SharedError> {
        if !self.GetKeyspace().is_empty() {
            return Err(adapter_error(
                "TiFlash placement for named keyspaces is not configured",
            ));
        }
        let count = i32::try_from(count)
            .map_err(|_| adapter_error("TiFlash replica count exceeds PD limit"))?;
        let (client, scheme) = tiflash_http_client(self)?;
        let rule_id = format!("table-{table_id}-r");
        let mut last_error = String::new();
        for address in self.GetPDAddrs().map_err(adapter_error)? {
            let base = if address.contains("://") {
                address.trim_end_matches('/').to_owned()
            } else {
                format!("{scheme}://{address}")
            };
            let response = if count == 0 {
                client
                    .delete(format!("{base}/pd/api/v1/config/rule/tiflash/{rule_id}"))
                    .send()
            } else {
                let mut start = b"t".to_vec();
                start.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
                start.extend_from_slice(b"_r");
                let mut end = b"t".to_vec();
                let next_id = table_id
                    .checked_add(1)
                    .ok_or_else(|| adapter_error("invalid table ID"))?;
                end.extend_from_slice(&(next_id as u64 ^ (1_u64 << 63)).to_be_bytes());
                let start = astersql_util_codec::EncodeBytes(Vec::new(), &start);
                let end = astersql_util_codec::EncodeBytes(Vec::new(), &end);
                let hex = |bytes: &[u8]| -> String {
                    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
                };
                let rule = serde_json::json!({
                    "group_id": "tiflash", "id": rule_id, "index": 120,
                    "start_key": hex(&start), "end_key": hex(&end),
                    "role": "learner", "count": count,
                    "label_constraints": [{"key": "engine", "op": "in", "values": ["tiflash"]}],
                    "location_labels": location_labels,
                });
                client
                    .post(format!("{base}/pd/api/v1/config/rule"))
                    .json(&rule)
                    .send()
            };
            match response {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => last_error = format!("PD returned {}", response.status()),
                Err(error) => last_error = error.to_string(),
            }
        }
        Err(adapter_error(format!(
            "publish TiFlash placement rule {rule_id}: {last_error}"
        )))
    }
    fn ImportSST(
        &self,
        commit_ts: u64,
        pairs: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<kv::SSTImportStats, kv::errors::SharedError> {
        if !self.GetKeyspace().is_empty() {
            return Err(adapter_error(
                "physical SST import keyspace codec is not configured",
            ));
        }
        let stats = crate::sst_import::write_and_ingest(
            &self.GetPDAddrs().map_err(adapter_error)?,
            self.TLSConfig(),
            commit_ts,
            pairs,
        )
        .map_err(adapter_error)?;
        Ok(kv::SSTImportStats {
            keys: stats.keys,
            bytes: stats.bytes,
            write_rpcs: stats.write_rpcs,
            ingest_rpcs: stats.ingest_rpcs,
        })
    }
    fn Begin(
        &self,
        options: &[kv::tikv::TxnOption],
    ) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError> {
        if options
            .iter()
            .any(|option| matches!(option, kv::tikv::TxnOption::StartTS(_)))
        {
            return Err(adapter_error(
                "client-rust 0.4.0 public API cannot open a writable transaction at a supplied start_ts",
            ));
        }
        begin_transaction(self, ClientTransactionMode::Optimistic)
    }

    fn GetSnapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot> {
        let runtime = self
            .client_runtime()
            .expect("canonical snapshots require the client-rust production runtime");
        match ClientSnapshot::new(
            runtime,
            self.coprocessor_store()
                .expect("canonical snapshots require the production RegionCache"),
            version.Ver,
        ) {
            Ok(snapshot) => Box::new(snapshot),
            Err(error) => Box::new(FailedSnapshot(error)),
        }
    }

    fn GetClient(&self) -> &dyn kv::Client {
        self
    }

    fn GetMPPClient(&self) -> &dyn kv::MPPClient {
        &MPP_CLIENT
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        TikvStore::Close(self).map_err(adapter_error)
    }

    fn UUID(&self) -> String {
        self.uuid()
    }

    fn CurrentVersion(
        &self,
        transaction_scope: &str,
    ) -> Result<kv::Version, kv::errors::SharedError> {
        TikvStore::CurrentVersion(self, transaction_scope)
            .map(|version| kv::NewVersion(version.0))
            .map_err(adapter_error)
    }

    fn GetOracle(&self) -> &dyn kv::oracle::Oracle {
        &ORACLE
    }

    fn SupportDeleteRange(&self) -> bool {
        true
    }

    fn Name(&self) -> String {
        TikvStore::Name(self).to_owned()
    }

    fn Describe(&self) -> String {
        TikvStore::Describe(self).to_owned()
    }

    fn ShowStatus(
        &self,
        _ctx: &kv::Context,
        key: &str,
    ) -> Result<Box<dyn Any>, kv::errors::SharedError> {
        TikvStore::ShowStatus(self, key).map_err(adapter_error)
    }

    fn GetMemCache(&self) -> &dyn kv::MemManager {
        mem_manager()
    }

    fn GetMinSafeTS(&self, _transaction_scope: &str) -> u64 {
        0
    }

    fn GetLockWaits(&self) -> Result<Vec<kv::deadlockpb::WaitForEntry>, kv::errors::SharedError> {
        TikvStore::GetLockWaits(self)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| kv::deadlockpb::WaitForEntry {
                        txn: entry.txn,
                        wait_for_txn: entry.waiting_for_txn,
                        key: entry.key,
                        key_hash: entry.key_hash,
                        resource_group_tag: entry.resource_group_tag,
                        wait_time: entry.wait_time,
                    })
                    .collect()
            })
            .map_err(adapter_error)
    }

    fn GetCodec(&self) -> kv::tikv::Codec {
        kv::tikv::Codec
    }

    fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}

    fn GetOption(&self, _key: &dyn Any) -> Option<&dyn Any> {
        None
    }

    fn GetClusterID(&self) -> u64 {
        TikvStore::GetClusterID(self)
    }

    fn GetKeyspace(&self) -> String {
        TikvStore::GetKeyspace(self)
    }
}
