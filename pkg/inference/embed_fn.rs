// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type Options = BTreeMap<String, serde_json::Value>;

/// Provider boundary used by the Domain-owned embedding function.
pub trait Embedder: Send + Sync {
    /// Preserve a caller cancellation cause and inspectable errors at the provider boundary.
    fn create_embeddings_with_context(
        &self,
        context: &crate::base::ProviderContext<'_>,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, crate::base::ProviderError> {
        self.create_embeddings(context.cancel, model, texts, opts)
            .map_err(|message| context.cause().unwrap_or_else(|| message.into()))
    }

    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String>;
}

const CACHE_CAPACITY: usize = 10_000;
const BATCH_WINDOW: Duration = Duration::from_millis(100);
const MAX_BATCH_SIZE: usize = 16;

struct Call {
    texts: Vec<String>,
    cacheable: bool,
    result: Mutex<Option<Result<Vec<Vec<f32>>, String>>>,
    done: Condvar,
    waiters: AtomicUsize,
    cancelled: AtomicBool,
    batch: Mutex<Weak<Batch>>,
}

impl Call {
    fn new(texts: Vec<String>, cacheable: bool) -> Self {
        Self {
            texts,
            cacheable,
            result: Mutex::new(None),
            done: Condvar::new(),
            waiters: AtomicUsize::new(1),
            cancelled: AtomicBool::new(false),
            batch: Mutex::new(Weak::new()),
        }
    }
}

struct Batch {
    provider: Arc<dyn Embedder>,
    model: String,
    opts: Options,
    calls: Mutex<Vec<(String, Arc<Call>)>>,
    flush_now: Mutex<bool>,
    flush_ready: Condvar,
    cancelled: AtomicBool,
}

struct State {
    providers: HashMap<String, Arc<dyn Embedder>>,
    cache: HashMap<String, Vec<f32>>,
    cache_order: VecDeque<String>,
    in_flight: HashMap<String, Arc<Call>>,
    batches: HashMap<BatchKey, Vec<Arc<Batch>>>,
    tasks: Vec<JoinHandle<()>>,
    closed: bool,
}

/// Domain-scoped embedding calls, batching, cache and cancellation.
pub struct EmbedFn {
    state: Arc<Mutex<State>>,
    config_version: AtomicU64,
    batch_window: Duration,
    max_batch_size: usize,
    next_call: AtomicU64,
}

impl Default for EmbedFn {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedFn {
    pub fn new() -> Self {
        Self::new_with_config(BATCH_WINDOW, MAX_BATCH_SIZE)
    }

    /// Configure the batching window and request size; zero values use defaults.
    pub fn new_with_config(batch_window: Duration, max_batch_size: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                providers: HashMap::new(),
                cache: HashMap::new(),
                cache_order: VecDeque::new(),
                in_flight: HashMap::new(),
                batches: HashMap::new(),
                tasks: Vec::new(),
                closed: false,
            })),
            config_version: AtomicU64::new(0),
            batch_window: if batch_window.is_zero() {
                BATCH_WINDOW
            } else {
                batch_window
            },
            next_call: AtomicU64::new(0),
            max_batch_size: if max_batch_size == 0 {
                MAX_BATCH_SIZE
            } else {
                max_batch_size
            },
        }
    }

    /// Register before the first embedding request.
    pub fn register(&self, provider: &str, embedder: Arc<dyn Embedder>) -> Result<(), String> {
        let provider = provider.trim().to_lowercase();
        if provider.is_empty() || provider.contains('/') {
            return Err(format!("invalid embedding provider: {provider:?}"));
        }
        let mut state = self.state.lock().expect("embedding state poisoned");
        if state.closed || !state.tasks.is_empty() {
            return Err("embedding provider registration is closed".into());
        }
        if state.providers.contains_key(&provider) {
            return Err(format!(
                "embedding provider {provider:?} is already registered"
            ));
        }
        state.providers.insert(provider, embedder);
        Ok(())
    }

    pub fn has_embedder(&self, provider: &str) -> bool {
        self.state
            .lock()
            .expect("embedding state poisoned")
            .providers
            .contains_key(&provider.trim().to_lowercase())
    }

    /// Invalidate previously cached embeddings after provider configuration changes.
    pub fn set_config_version(&self, version: u64) {
        self.config_version.store(version, Ordering::Release);
    }

    /// Each waiter may cancel independently; the provider is cancelled only
    /// when the entire batch has no remaining waiters or Domain closes.
    pub fn embed(
        &self,
        model_with_provider: &str,
        text: &str,
        opts: &Options,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<f32>, String> {
        let version = self.config_version.load(Ordering::Acquire);
        let key = serde_json::to_string(&(model_with_provider, text, opts, version))
            .map_err(|error| error.to_string())?;
        let values = self.request(
            model_with_provider,
            &[text.to_owned()],
            opts,
            &|| should_cancel().then(|| "context canceled".into()),
            key,
            true,
        )?;
        let value = values.into_iter().next().expect("one text result");
        if value.len() > 16_383 {
            return Err("vector cannot have more than 16383 dimensions".into());
        }
        Ok(value)
    }

    /// Multi-text provider batching without the runtime embedding cache. Owned
    /// snapshots allow a canceled caller to reuse its input immediately.
    pub fn create_embeddings(
        &self,
        model_with_provider: &str,
        texts: &[String],
        opts: &Options,
        cancellation: &dyn Fn() -> Option<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let key = format!(
            "batch-call:{}",
            self.next_call.fetch_add(1, Ordering::Relaxed)
        );
        self.request(model_with_provider, texts, opts, cancellation, key, false)
    }

    fn request(
        &self,
        model_with_provider: &str,
        texts: &[String],
        opts: &Options,
        cancellation: &dyn Fn() -> Option<String>,
        key: String,
        cacheable: bool,
    ) -> Result<Vec<Vec<f32>>, String> {
        if let Some(cause) = poll_cancellation(cancellation) {
            return Err(cause);
        }
        let (provider_name, model) = model_with_provider.split_once('/').ok_or_else(|| {
            format!("model name must be in format 'provider/model', got: {model_with_provider}")
        })?;
        let provider_name = provider_name.trim().to_lowercase();
        let model = model.trim();
        let opts = opts.clone();
        let batch_key = new_batch_key(&provider_name, model, &opts)?;

        let call = {
            let mut state = self.state.lock().expect("embedding state poisoned");
            if state.closed {
                return Err("embedding function is closed".into());
            }
            if cacheable {
                if let Some(cached) = state.cache.get(&key) {
                    if let Some(cause) = poll_cancellation(cancellation) {
                        return Err(cause);
                    }
                    return Ok(vec![cached.clone()]);
                }
            }
            if let Some(call) = state.in_flight.get(&key) {
                call.waiters.fetch_add(1, Ordering::AcqRel);
                Arc::clone(call)
            } else {
                let provider = state
                    .providers
                    .get(&provider_name)
                    .cloned()
                    .ok_or_else(|| {
                        let mut providers = state.providers.keys().cloned().collect::<Vec<_>>();
                        providers.sort();
                        format!(
                            "unknown embedding provider '{provider_name}', available providers: {}",
                            providers.join(", ")
                        )
                    })?;
                let call = Arc::new(Call::new(texts.to_vec(), cacheable));
                state.in_flight.insert(key.clone(), Arc::clone(&call));
                let existing = state
                    .batches
                    .get(&batch_key)
                    .and_then(|batches| {
                        batches.iter().find(|batch| {
                            !batch.cancelled.load(Ordering::Acquire)
                                && batch.opts == opts
                                && batch
                                    .calls
                                    .lock()
                                    .expect("batch poisoned")
                                    .iter()
                                    .map(|(_, call)| call.texts.len())
                                    .sum::<usize>()
                                    < self.max_batch_size
                        })
                    })
                    .cloned();
                let batch = if let Some(batch) = existing {
                    batch
                } else {
                    let batch = Arc::new(Batch {
                        provider,
                        model: model.to_owned(),
                        opts,
                        calls: Mutex::new(Vec::new()),
                        flush_now: Mutex::new(false),
                        flush_ready: Condvar::new(),
                        cancelled: AtomicBool::new(false),
                    });
                    state
                        .batches
                        .entry(batch_key.clone())
                        .or_default()
                        .push(Arc::clone(&batch));
                    let shared = Arc::clone(&self.state);
                    let worker_batch = Arc::clone(&batch);
                    let worker_key = batch_key.clone();
                    let batch_window = self.batch_window;
                    let max_batch_size = self.max_batch_size;
                    state.tasks.retain(|task| !task.is_finished());
                    state.tasks.push(thread::spawn(move || {
                        let flush = worker_batch
                            .flush_now
                            .lock()
                            .expect("batch flush lock poisoned");
                        let _ = worker_batch
                            .flush_ready
                            .wait_timeout_while(flush, batch_window, |flush| !*flush)
                            .expect("batch flush lock poisoned");
                        run_batch(&shared, &worker_key, &worker_batch, max_batch_size);
                    }));
                    batch
                };
                *call.batch.lock().expect("call batch poisoned") = Arc::downgrade(&batch);
                let mut calls = batch.calls.lock().expect("batch poisoned");
                calls.push((key.clone(), Arc::clone(&call)));
                if calls
                    .iter()
                    .map(|(_, call)| call.texts.len())
                    .sum::<usize>()
                    >= self.max_batch_size
                {
                    *batch.flush_now.lock().expect("batch flush lock poisoned") = true;
                    batch.flush_ready.notify_one();
                }
                call
            }
        };

        let mut result = call.result.lock().expect("embedding call poisoned");
        loop {
            if let Some(cause) = poll_cancellation(cancellation) {
                drop(result);
                self.release_waiter(&key, &call);
                return Err(cause);
            }
            if let Some(value) = result.clone() {
                drop(result);
                self.release_waiter(&key, &call);
                return value;
            }
            result = call
                .done
                .wait_timeout(result, Duration::from_millis(10))
                .expect("embedding call poisoned")
                .0;
        }
    }

    fn release_waiter(&self, key: &str, call: &Arc<Call>) {
        if call.waiters.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        if call
            .result
            .lock()
            .expect("embedding call poisoned")
            .is_some()
        {
            return;
        }
        call.cancelled.store(true, Ordering::Release);
        if let Some(batch) = call.batch.lock().expect("call batch poisoned").upgrade() {
            let calls = batch.calls.lock().expect("batch poisoned");
            if calls
                .iter()
                .all(|(_, call)| call.waiters.load(Ordering::Acquire) == 0)
            {
                batch.cancelled.store(true, Ordering::Release);
                *batch.flush_now.lock().expect("batch flush lock poisoned") = true;
                batch.flush_ready.notify_one();
            }
        }
        let mut state = self.state.lock().expect("embedding state poisoned");
        if state
            .in_flight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, call))
        {
            state.in_flight.remove(key);
        }
    }

    pub fn close(&self) {
        let tasks = {
            let mut state = self.state.lock().expect("embedding state poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            for batches in state.batches.values() {
                for batch in batches {
                    batch.cancelled.store(true, Ordering::Release);
                    *batch.flush_now.lock().expect("batch flush lock poisoned") = true;
                    batch.flush_ready.notify_one();
                }
            }
            for call in state.in_flight.values() {
                call.cancelled.store(true, Ordering::Release);
                if let Some(batch) = call.batch.lock().expect("call batch poisoned").upgrade() {
                    batch.cancelled.store(true, Ordering::Release);
                    *batch.flush_now.lock().expect("batch flush lock poisoned") = true;
                    batch.flush_ready.notify_one();
                }
                let mut result = call.result.lock().expect("embedding call poisoned");
                if result.is_none() {
                    *result = Some(Err("embedding function is closed".into()));
                    call.done.notify_all();
                }
            }
            std::mem::take(&mut state.tasks)
        };
        for task in tasks {
            let _ = task.join();
        }
        let mut state = self.state.lock().expect("embedding state poisoned");
        state.cache.clear();
        state.cache_order.clear();
    }
}

impl Drop for EmbedFn {
    fn drop(&mut self) {
        self.close();
    }
}

fn run_batch(
    state: &Arc<Mutex<State>>,
    batch_key: &BatchKey,
    batch: &Arc<Batch>,
    max_batch_size: usize,
) {
    let calls = {
        let mut state = state.lock().expect("embedding state poisoned");
        if let Some(batches) = state.batches.get_mut(batch_key) {
            batches.retain(|entry| !Arc::ptr_eq(entry, batch));
            if batches.is_empty() {
                state.batches.remove(batch_key);
            }
        }
        batch.calls.lock().expect("batch poisoned").clone()
    };
    let active = calls
        .iter()
        .filter(|(_, call)| call.waiters.load(Ordering::Acquire) > 0)
        .cloned()
        .collect::<Vec<_>>();
    if active.is_empty() {
        return;
    }
    let texts = active
        .iter()
        .flat_map(|(_, call)| call.texts.clone())
        .collect::<Vec<_>>();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut values = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(max_batch_size) {
            let result = batch.provider.create_embeddings(
                &batch.cancelled,
                &batch.model,
                chunk,
                &batch.opts,
            )?;
            if result.len() != chunk.len() {
                if result.is_empty() {
                    return Err(format!("no embeddings returned for model {}", batch.model));
                }
                return Err(format!(
                    "embedding provider returned {} embeddings for {} texts",
                    result.len(),
                    chunk.len()
                ));
            }
            values.extend(result);
        }
        Ok(values)
    }))
    .unwrap_or_else(|_| Err("embedding batch processing panicked".into()));
    let mut offset = 0;
    let values = active
        .iter()
        .map(|(_, call)| match &outcome {
            Ok(values) => {
                let end = offset + call.texts.len();
                let result = values[offset..end].to_vec();
                offset = end;
                Ok(result)
            }
            Err(error) => Err(error.clone()),
        })
        .collect::<Vec<_>>();
    let mut state = state.lock().expect("embedding state poisoned");
    for ((key, call), value) in active.into_iter().zip(values) {
        if !state.closed && call.cacheable && call.waiters.load(Ordering::Acquire) > 0 {
            if let Ok(embedding) = &value {
                if embedding[0].len() <= 16_383 {
                    if !state.cache.contains_key(&key) {
                        state.cache_order.push_back(key.clone());
                    }
                    state.cache.insert(key.clone(), embedding[0].clone());
                }
                while state.cache.len() > CACHE_CAPACITY {
                    if let Some(oldest) = state.cache_order.pop_front() {
                        state.cache.remove(&oldest);
                    }
                }
            }
        }
        if state
            .in_flight
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &call))
        {
            state.in_flight.remove(&key);
        }
        let mut result = call.result.lock().expect("embedding call poisoned");
        if result.is_none() {
            *result = Some(value);
            call.done.notify_all();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BatchKey {
    provider: String,
    model: String,
    pub(crate) options_digest: [u8; 32],
}

pub(crate) fn new_batch_key(
    provider: &str,
    model: &str,
    opts: &Options,
) -> Result<BatchKey, String> {
    let serialized =
        serde_json::to_vec(opts).map_err(|error| format!("failed to serialize opts: {error}"))?;
    Ok(BatchKey {
        provider: provider.into(),
        model: model.into(),
        options_digest: Sha256::digest(serialized).into(),
    })
}

impl Embedder for EmbedFn {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.create_embeddings(model, texts, opts, &|| {
            cancel
                .load(Ordering::Acquire)
                .then(|| "context canceled".into())
        })
    }
}

fn poll_cancellation(cancellation: &dyn Fn() -> Option<String>) -> Option<String> {
    // A panicking cancellation observer must release its waiter and cancel a
    // provider with no remaining callers, just as Go's recovery watcher does.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(cancellation))
        .unwrap_or_else(|_| Some("context canceled".into()))
}
