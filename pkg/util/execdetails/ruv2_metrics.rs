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

// Statement-level RU metrics retained after removing the deprecated RU v2 model.
// Only the bypass flag and TiKV coprocessor response bytes are needed by the
// current statement-RU calculation and cursor delta synchronization paths.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

#[derive(Clone, Copy)]
pub struct ruv2MetricsKeyType;

pub static RUV2MetricsCtxKey: ruv2MetricsKeyType = ruv2MetricsKeyType;

pub fn RUV2MetricsFromContext(ctx: &context::Context) -> Option<RUV2Metrics> {
    if let Some(stmtDetails) = ctx.value::<StmtExecDetails>(StmtExecDetailKey) {
        if let Some(metrics) = stmtDetails.getRUV2Metrics() {
            return Some(metrics);
        }
    }
    ctx.value::<RUV2Metrics>(RUV2MetricsCtxKey)
}

pub fn UpdateRUV2MetricsFromRUV2(m: Option<&RUV2Metrics>, ru: Option<&kvrpcpb::Ruv2>) {
    let (Some(m), Some(ru)) = (m, ru) else {
        return;
    };
    if m.Bypass() {
        return;
    }
    let response_bytes = ru.get_coprocessor_response_bytes();
    if response_bytes != 0 {
        m.tikvCoprocessorResponseBytes
            .fetch_add(response_bytes as i64, Ordering::Relaxed);
    }
}

pub fn SyncRUV2MetricsFromRUDetails(
    metrics: Option<&RUV2Metrics>,
    ruDetails: Option<&tikvutil::RUDetails>,
) {
    let (Some(metrics), Some(ruDetails)) = (metrics, ruDetails) else {
        return;
    };
    if metrics.Bypass() {
        return;
    }
    UpdateRUV2MetricsFromRUV2(Some(metrics), Some(&ruDetails.DrainRUV2()));
}

pub struct RUV2Metrics {
    bypass: AtomicBool,
    tikvCoprocessorResponseBytes: AtomicI64,
}

impl Default for RUV2Metrics {
    fn default() -> Self {
        Self {
            bypass: AtomicBool::new(false),
            tikvCoprocessorResponseBytes: AtomicI64::new(0),
        }
    }
}

impl Clone for RUV2Metrics {
    fn clone(&self) -> Self {
        Self {
            bypass: AtomicBool::new(self.Bypass()),
            tikvCoprocessorResponseBytes: AtomicI64::new(self.TiKVCoprocessorResponseBytes()),
        }
    }
}

pub fn NewRUV2Metrics() -> RUV2Metrics {
    RUV2Metrics::default()
}

impl RUV2Metrics {
    pub fn SetBypass(&self, enabled: bool) {
        self.bypass.store(enabled, Ordering::Relaxed);
    }

    pub fn Bypass(&self) -> bool {
        self.bypass.load(Ordering::Relaxed)
    }

    pub fn AddTiKVCoprocessorResponseBytes(&self, delta: i64) {
        if !self.Bypass() {
            self.tikvCoprocessorResponseBytes
                .fetch_add(delta, Ordering::Relaxed);
        }
    }

    pub fn TiKVCoprocessorResponseBytes(&self) -> i64 {
        self.tikvCoprocessorResponseBytes.load(Ordering::Relaxed)
    }
}
