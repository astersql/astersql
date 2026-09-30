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

use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_RU_EMA_TAU: Duration = Duration::from_secs(1);

/// A time-aware estimate shared by all workers in one logical scan.
#[derive(Debug)]
pub struct RuEma {
    state: Mutex<(f64, Option<Instant>)>,
    tau: Duration,
}

impl RuEma {
    pub fn new(seed_read_bytes: u64) -> Self {
        Self {
            state: Mutex::new((seed_read_bytes as f64, None)),
            tau: DEFAULT_RU_EMA_TAU,
        }
    }

    pub fn observe(&self, bytes: u64, now: Instant) {
        let mut state = self.state.lock().expect("RU EMA lock poisoned");
        let alpha = state.1.map_or(1.0, |last| {
            1.0 - (-now.saturating_duration_since(last).as_secs_f64() / self.tau.as_secs_f64())
                .exp()
        });
        state.0 += alpha * (bytes as f64 - state.0);
        if state.1.is_none_or(|last| now > last) {
            state.1 = Some(now);
        }
    }

    pub fn predict(&self) -> u64 {
        self.state.lock().expect("RU EMA lock poisoned").0 as u64
    }
}
