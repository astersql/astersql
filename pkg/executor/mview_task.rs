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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// None represents Go's zero time; backwards and absent endpoints yield zero.
pub fn duration_microseconds_between(start: Option<SystemTime>, end: Option<SystemTime>) -> i64 {
    match (start, end) {
        (Some(start), Some(end)) => end
            .duration_since(start)
            .unwrap_or_default()
            .min(Duration::from_nanos(i64::MAX as u64))
            .as_micros() as i64,
        _ => 0,
    }
}

pub fn format_duration_seconds_from_microseconds(micros: i64) -> String {
    if micros <= 0 {
        return "0.000000".into();
    }
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

pub fn format_duration_seconds_between(
    start: Option<SystemTime>,
    end: Option<SystemTime>,
) -> String {
    let micros = duration_microseconds_between(start, end);
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

pub fn format_duration_seconds(duration: Duration) -> String {
    let micros = duration.as_micros();
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// SystemTime stores an absolute instant: timezone conversion preserves that
/// instant. Truncate on the UTC timeline, including instants before Unix epoch.
pub fn hist_time(time: Option<SystemTime>) -> Option<SystemTime> {
    time.map(|time| match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            UNIX_EPOCH + Duration::new(duration.as_secs(), duration.subsec_nanos() / 1_000 * 1_000)
        }
        Err(error) => {
            let duration = error.duration();
            UNIX_EPOCH
                - Duration::new(
                    duration.as_secs(),
                    duration.subsec_nanos().div_ceil(1_000) * 1_000,
                )
        }
    })
}
