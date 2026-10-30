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

use std::sync::OnceLock;

/// Interval between successful scheduled refreshes, excluding the current refresh.
static SCHEDULE_DURATION: OnceLock<prometheus::Histogram> = OnceLock::new();

pub fn InitMVMetrics() {
    SCHEDULE_DURATION.get_or_init(|| {
        prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
            "tidb_mv_service_refresh_schedule_duration_seconds",
            "Bucketed histogram of the interval between two successful MV service refreshes, excluding the current refresh duration.",
        ).buckets(prometheus::exponential_buckets(1.0, 2.0, 25).expect("fixed MV histogram buckets")))
        .expect("fixed MV histogram definition")
    });
}

/// One process-wide collector is shared by the runtime and package registration.
pub fn MVServiceRefreshScheduleDurationHistogram() -> &'static prometheus::Histogram {
    InitMVMetrics();
    SCHEDULE_DURATION.get().expect("MV metrics initialized")
}
