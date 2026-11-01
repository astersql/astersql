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

use std::{sync::Arc, time::Duration};
pub const DEFAULT_CPU_THRESHOLD: f64 = 0.8;
pub const DEFAULT_MEM_THRESHOLD: f64 = 0.8;
pub const DEFAULT_DELAY: Duration = Duration::from_millis(100);
pub trait TaskBackpressureController: Send + Sync {
    fn should_backpressure(&self) -> (bool, Duration);
}
pub struct CPUMemBackpressureController {
    pub cpu_threshold: f64,
    pub mem_threshold: f64,
    pub delay: Duration,
    pub get_cpu_usage: Arc<dyn Fn() -> (f64, bool) + Send + Sync>,
    pub get_mem_used: Arc<dyn Fn() -> Result<u64, String> + Send + Sync>,
    pub get_mem_total: Arc<dyn Fn() -> u64 + Send + Sync>,
}
impl CPUMemBackpressureController {
    pub fn new(cpu_threshold: f64, mem_threshold: f64, delay: Duration) -> Self {
        Self {
            cpu_threshold,
            mem_threshold,
            delay,
            get_cpu_usage: Arc::new(cpu_crate::GetCPUUsage),
            get_mem_used: Arc::new(|| {
                memory_crate::meminfo::InstanceMemUsed().map_err(|e| e.to_string())
            }),
            get_mem_total: Arc::new(memory_crate::meminfo::GetMemTotalIgnoreErr),
        }
    }
}
impl TaskBackpressureController for CPUMemBackpressureController {
    fn should_backpressure(&self) -> (bool, Duration) {
        let delay = if self.delay.is_zero() {
            DEFAULT_DELAY
        } else {
            self.delay
        };
        if self.cpu_threshold > 0.0 {
            let (usage, unsupported) = (self.get_cpu_usage)();
            if !unsupported && usage >= self.cpu_threshold {
                return (true, delay);
            }
        }
        if self.mem_threshold > 0.0 {
            let total = (self.get_mem_total)();
            if total > 0 {
                if let Ok(used) = (self.get_mem_used)() {
                    if used as f64 / total as f64 >= self.mem_threshold {
                        return (true, delay);
                    }
                }
            }
        }
        (false, Duration::ZERO)
    }
}
