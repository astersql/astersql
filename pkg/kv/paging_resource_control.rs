// Copyright 2026 AsterSQL.
// Copyright 2023 TiKV Project Authors.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and limitations under the License.

//! Paging-only RU admission at the existing KV interceptor boundary.
//! Token settings/grants are supplied by the resource-group owner; this module
//! does not replace the PD resource-group controller or its allocation loop.
use crate::resourcegroup::{CopRPCRequestInfo, CopRPCResponseInfo, CopRUInterceptor, RUDetails};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PagingTokenConfig {
    pub fill_rate: f64,
    /// Negative: unlimited; zero: unlimited capacity; positive: refill cap.
    pub burst: i64,
    pub tokens: f64,
    pub max_wait: Duration,
    pub retry_times: usize,
    pub retry_interval: Duration,
}

#[derive(Debug)]
struct Bucket {
    config: PagingTokenConfig,
    last: Instant,
    tokens: f64,
    generation: u64,
}

#[derive(Debug)]
pub struct PagingRUInterceptor {
    group: String,
    bucket: Mutex<Bucket>,
    reconfigured: Condvar,
    metrics: PagingMetrics,
    throttled: AtomicBool,
}

impl PagingRUInterceptor {
    pub fn new(group: impl Into<String>, config: PagingTokenConfig) -> Self {
        let tokens = config.tokens;
        Self {
            group: group.into(),
            bucket: Mutex::new(Bucket {
                config,
                last: Instant::now(),
                tokens,
                generation: 0,
            }),
            reconfigured: Condvar::new(),
            metrics: PagingMetrics::new(),
            throttled: AtomicBool::new(false),
        }
    }

    fn refill(bucket: &Bucket, now: Instant) -> f64 {
        if bucket.config.burst < 0 {
            return bucket.tokens;
        }
        let mut tokens = bucket.tokens
            + now.saturating_duration_since(bucket.last).as_secs_f64()
                * bucket.config.fill_rate.max(0.0);
        if bucket.config.burst > 0 {
            tokens = tokens.min(bucket.config.burst as f64);
        }
        tokens
    }

    /// Apply a token grant and wake failed-reservation retry loops.
    pub fn reconfigure(&self, config: PagingTokenConfig) {
        let mut bucket = self.bucket.lock().expect("paging RU lock poisoned");
        let now = Instant::now();
        bucket.tokens = if config.burst < 0 {
            config.tokens
        } else {
            Self::refill(&bucket, now) + config.tokens
        };
        bucket.last = bucket.last.max(now);
        bucket.config = config;
        bucket.generation += 1;
        self.reconfigured.notify_all();
    }

    pub fn set_throttled(&self, throttled: bool) {
        self.throttled.store(throttled, Ordering::Release);
    }

    pub fn available_tokens(&self) -> f64 {
        let bucket = self.bucket.lock().expect("paging RU lock poisoned");
        Self::refill(&bucket, Instant::now())
    }

    fn adjust(&self, debit: f64) {
        let mut bucket = self.bucket.lock().expect("paging RU lock poisoned");
        if bucket.config.burst < 0 || bucket.config.fill_rate == f64::MAX {
            return;
        }
        let now = Instant::now();
        bucket.tokens = Self::refill(&bucket, now) - debit;
        bucket.last = bucket.last.max(now);
    }

    fn wait(&self, amount: f64, cancelled: Option<&AtomicBool>) -> Result<(), String> {
        let is_cancelled = || cancelled.is_some_and(|flag| flag.load(Ordering::Acquire));
        let mut bucket = self.bucket.lock().expect("paging RU lock poisoned");
        let attempts = bucket.config.retry_times;
        for attempt in 0..attempts {
            if is_cancelled() {
                return Err("resource control cancelled".into());
            }
            if bucket.config.burst < 0 || bucket.config.fill_rate == f64::MAX {
                return Ok(());
            }
            let now = Instant::now();
            let remaining = Self::refill(&bucket, now) - amount;
            let delay = if remaining >= 0.0 {
                Some(Duration::ZERO)
            } else if bucket.config.fill_rate > 0.0 {
                Duration::try_from_secs_f64(-remaining / bucket.config.fill_rate).ok()
            } else {
                None
            };
            if let Some(delay) = delay.filter(|delay| *delay <= bucket.config.max_wait) {
                bucket.tokens = remaining;
                bucket.last = bucket.last.max(now);
                let act_at = now + delay;
                loop {
                    if is_cancelled() {
                        // Go WaitReservations cancels at the original reservation time.
                        if bucket.config.burst >= 0 && bucket.config.fill_rate != f64::MAX {
                            bucket.tokens = Self::refill(&bucket, now) + amount;
                            bucket.last = bucket.last.max(now);
                        }
                        return Err("resource control cancelled".into());
                    }
                    let left = act_at.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(());
                    }
                    (bucket, _) = self
                        .reconfigured
                        .wait_timeout(bucket, left.min(Duration::from_millis(10)))
                        .expect("paging RU lock poisoned");
                }
            }
            if attempt + 1 == attempts {
                break;
            }
            let generation = bucket.generation;
            let retry_at = Instant::now() + bucket.config.retry_interval;
            while bucket.generation == generation {
                if is_cancelled() {
                    return Err("resource control cancelled".into());
                }
                let left = retry_at.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                (bucket, _) = self
                    .reconfigured
                    .wait_timeout(bucket, left.min(Duration::from_millis(10)))
                    .expect("paging RU lock poisoned");
            }
        }
        Err("resource group throttled: reservation exceeds maximum wait".into())
    }

    fn request(
        &self,
        request: &CopRPCRequestInfo,
        cancelled: Option<&AtomicBool>,
    ) -> Result<RUDetails, String> {
        if request.resource_group_name != self.group {
            return Err(format!(
                "resource group {} is not configured",
                request.resource_group_name
            ));
        }
        let delta = 0.125 + 0.5 * 0.7 + request.predicted_read_bytes as f64 / 65536.0;
        self.wait(delta, cancelled)?;
        self.metrics
            .observe_request(&self.group, request.predicted_read_bytes);
        Ok(RUDetails {
            read_ru: delta,
            write_ru: 0.0,
        })
    }
}

impl CopRUInterceptor for PagingRUInterceptor {
    fn OnRequestWait(&self, request: &CopRPCRequestInfo) -> Result<RUDetails, String> {
        self.request(request, None)
    }
    fn OnRequestWaitCancellable(
        &self,
        request: &CopRPCRequestInfo,
        cancelled: Option<&AtomicBool>,
    ) -> Result<RUDetails, String> {
        self.request(request, cancelled)
    }
    fn OnResponseWait(
        &self,
        request: &CopRPCRequestInfo,
        response: &CopRPCResponseInfo,
    ) -> Result<RUDetails, String> {
        let delta = (response.read_bytes as f64 - request.predicted_read_bytes as f64) / 65536.0
            + response.kv_cpu_ms / 3.0;
        // A precharged cop response settles immediately: extra cost becomes debt,
        // and an overestimate is refunded. It must not queue completed work again.
        if request.predicted_read_bytes > 0 {
            self.adjust(delta);
        } else if delta > 0.0 {
            if response.read_bytes < 4 * 1024 * 1024 || !self.throttled.load(Ordering::Acquire) {
                self.adjust(delta);
            } else {
                self.wait(delta, None)?;
            }
        }
        self.metrics.observe_response(
            &self.group,
            request.predicted_read_bytes,
            response.read_bytes,
        );
        Ok(RUDetails {
            read_ru: delta,
            write_ru: 0.0,
        })
    }
}

#[derive(Clone, Debug)]
struct PagingMetrics {
    precharged: prometheus::CounterVec,
    uncharged: prometheus::CounterVec,
    predicted: prometheus::CounterVec,
    actual: prometheus::CounterVec,
    residual: prometheus::HistogramVec,
}
impl PagingMetrics {
    fn new() -> Self {
        static METRICS: std::sync::OnceLock<PagingMetrics> = std::sync::OnceLock::new();
        METRICS
            .get_or_init(|| {
                let labels = &["resource_group", "keyspace_name"];
                let counter = |name: &str, help: &str| {
                    let metric = prometheus::CounterVec::new(
                        prometheus::Opts::new(
                            format!("resource_manager_client_request_{name}"),
                            help,
                        ),
                        labels,
                    )
                    .unwrap();
                    prometheus::register(Box::new(metric.clone()))
                        .expect("register paging RU metric");
                    metric
                };
                let residual = prometheus::HistogramVec::new(
                    prometheus::HistogramOpts::new(
                        "resource_manager_client_request_paging_prediction_residual_bytes",
                        "Signed actual minus predicted read bytes for precharged coprocessor RPCs.",
                    )
                    .buckets(vec![
                        -67108864., -16777216., -4194304., -1048576., -262144., -65536., -16384.,
                        -4096., 0., 4096., 16384., 65536., 262144., 1048576., 4194304., 16777216.,
                        67108864.,
                    ]),
                    labels,
                )
                .unwrap();
                prometheus::register(Box::new(residual.clone()))
                    .expect("register paging residual metric");
                PagingMetrics {
                    precharged: counter(
                        "cop_read_precharge_total",
                        "Coprocessor reads with a positive precharge hint.",
                    ),
                    uncharged: counter(
                        "cop_read_no_precharge_total",
                        "Coprocessor reads without a positive precharge hint.",
                    ),
                    predicted: counter(
                        "paging_precharge_bytes_total",
                        "Predicted bytes used for paging precharge.",
                    ),
                    actual: counter(
                        "paging_actual_bytes_total",
                        "Actual MVCC bytes read by precharged requests.",
                    ),
                    residual,
                }
            })
            .clone()
    }
    fn observe_request(&self, group: &str, bytes: u64) {
        let keyspace = keyspace::GetKeyspaceNameBySettings();
        let labels = &[group, &keyspace];
        if bytes == 0 {
            self.uncharged.with_label_values(labels).inc();
        } else {
            self.precharged.with_label_values(labels).inc();
            self.predicted
                .with_label_values(labels)
                .inc_by(bytes as f64);
        }
    }
    fn observe_response(&self, group: &str, predicted: u64, actual: u64) {
        if predicted == 0 {
            return;
        }
        let keyspace = keyspace::GetKeyspaceNameBySettings();
        let labels = &[group, &keyspace];
        self.actual.with_label_values(labels).inc_by(actual as f64);
        self.residual
            .with_label_values(labels)
            .observe(actual as f64 - predicted as f64);
    }
}
