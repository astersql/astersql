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

use crate::arbitrator::{ArbitratorModeDisable, MemArbitrator};
use crate::global_arbitrator::HeapProfileRuntime;
use crate::{calcRatio, multiRatio};
use chrono::{DateTime, FixedOffset, Local};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const LEVELS: [(i64, i32); 3] = [(700, 70), (800, 80), (850, 85)];
const RESET_MILLI: i64 = 650;
const CUTOFF_MILLI: i64 = 900;
const EMERGENCY_THRESHOLD: i32 = 95;
const MIN_INTERVAL: Duration = Duration::from_secs(60);
const EMERGENCY_INTERVAL: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const MAX_GROUPS: usize = 10;
const METADATA_SUFFIX: &str = ".meta.json";
const TIMESTAMP_LAYOUT: &str = "%Y-%m-%dT%H-%M-%S%z";

pub type ProfileWriter = Arc<dyn Fn(&mut dyn Write) -> io::Result<()> + Send + Sync>;
pub type ProfileClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

#[derive(Default)]
struct TriggerState {
    last_capture_at: Option<SystemTime>,
    emergency_last_capture_at: Option<SystemTime>,
    last_limit: i64,
    last_capture_threshold: i32,
    attempted: u32,
    closed: bool,
}

#[derive(Default)]
struct CollectorState {
    last_check_at: Option<SystemTime>,
    trigger: TriggerState,
}

pub struct HeapProfileCollector {
    dir: PathBuf,
    now: ProfileClock,
    write_profile: ProfileWriter,
    state: Mutex<CollectorState>,
}

#[derive(Clone, Copy)]
struct Snapshot {
    heap_alloc: i64,
    heap_inuse: i64,
    mem_inuse: i64,
    quota_alloc: i64,
    out_of_control: i64,
    limit: i64,
    capture_cutoff: i64,
}

impl Snapshot {
    fn from_arbitrator(m: &MemArbitrator) -> Self {
        let (heap_alloc, heap_inuse, mem_inuse) = m.HeapProfileCounters();
        let limit = m.Limit();
        Self {
            heap_alloc,
            heap_inuse,
            mem_inuse,
            quota_alloc: m.Allocated(),
            out_of_control: m.OutOfControl(),
            limit,
            capture_cutoff: multiRatio(limit, CUTOFF_MILLI),
        }
    }
}

impl HeapProfileCollector {
    /// Creates the production collector backed by the process allocation sampler.
    pub fn new_default(dir: PathBuf) -> Self {
        Self::new(
            dir,
            Arc::new(|out| {
                use rpprof::protos::Message;
                // rpprof resolves symbols while holding its callsite read lock.
                // Pause sampling so allocations made by the report itself cannot
                // re-enter the recorder and attempt to take the write lock.
                if !rpprof::alloc::is_active() {
                    return Err(io::Error::other("heap allocation sampler is not active"));
                }
                struct ResumeSampling;
                impl Drop for ResumeSampling {
                    fn drop(&mut self) {
                        rpprof::alloc::start();
                    }
                }
                rpprof::alloc::stop();
                let _resume = ResumeSampling;
                let report = rpprof::alloc::heap_report()
                    .map_err(|err| io::Error::other(err.to_string()))?;
                let profile = report
                    .pprof()
                    .map_err(|err| io::Error::other(err.to_string()))?;
                let mut bytes = Vec::new();
                profile
                    .encode(&mut bytes)
                    .map_err(|err| io::Error::other(err.to_string()))?;
                out.write_all(&bytes)
            }),
        )
    }
    pub fn new(dir: PathBuf, write_profile: ProfileWriter) -> Self {
        Self::new_with_hooks(dir, Arc::new(SystemTime::now), write_profile)
    }

    pub fn new_with_hooks(dir: PathBuf, now: ProfileClock, write_profile: ProfileWriter) -> Self {
        let collector = Self {
            dir,
            now,
            write_profile,
            state: Mutex::new(CollectorState::default()),
        };
        collector.enforce_retention();
        collector
    }

    pub fn capture(&self, m: &MemArbitrator, threshold: i32) -> bool {
        let mut state = self.state.lock().expect("heap profile lock poisoned");
        self.capture_locked(m, threshold, &mut state.trigger)
    }

    fn capture_locked(
        &self,
        m: &MemArbitrator,
        threshold: i32,
        trigger: &mut TriggerState,
    ) -> bool {
        let snapshot = Snapshot::from_arbitrator(m);
        if m.WorkMode() == ArbitratorModeDisable || snapshot.limit <= 0 {
            return false;
        }
        if threshold != EMERGENCY_THRESHOLD
            && (m.AtMemRisk() || snapshot.mem_inuse >= snapshot.capture_cutoff)
        {
            return false;
        }
        trigger.last_capture_at = Some((self.now)());
        trigger.last_capture_threshold = threshold;
        if create_private_dir(&self.dir).is_err() {
            return false;
        }
        let mut profile = match tempfile::Builder::new()
            .prefix(".heap-profile.")
            .suffix(".tmp")
            .tempfile_in(&self.dir)
        {
            Ok(file) => file,
            Err(_) => return false,
        };
        if profile
            .as_file()
            .set_permissions(private_permissions())
            .is_err()
        {
            return false;
        }
        let start = (self.now)();
        trigger.last_capture_at = Some(start);
        let local: DateTime<Local> = start.into();
        let base = format!("{}.{}pct", local.format(TIMESTAMP_LAYOUT), threshold);
        if (self.write_profile)(profile.as_file_mut()).is_err() {
            return true;
        }
        if profile.as_file_mut().sync_all().is_err() {
            return true;
        }
        if profile
            .persist(self.dir.join(format!("{base}.pprof")))
            .is_err()
        {
            return true;
        }
        let duration_ms = (self.now)()
            .duration_since(start)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let metadata = json!({
            "start_time": local.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            "version": 1,
            "threshold_pct": threshold,
            "duration_ms": duration_ms,
            "state": {
                "heap_alloc_bytes": snapshot.heap_alloc,
                "heap_inuse_bytes": snapshot.heap_inuse,
                "mem_inuse_bytes": snapshot.mem_inuse,
                "quota_alloc_bytes": snapshot.quota_alloc,
                "out_of_control_bytes": snapshot.out_of_control,
                "limit_bytes": snapshot.limit,
            },
        });
        let _ = self.write_metadata_atomically(&format!("{base}{METADATA_SUFFIX}"), &metadata);
        self.enforce_retention();
        true
    }

    fn write_metadata_atomically(
        &self,
        name: &str,
        metadata: &serde_json::Value,
    ) -> io::Result<()> {
        let mut file = tempfile::Builder::new()
            .prefix(".heap-metadata.")
            .suffix(".tmp")
            .tempfile_in(&self.dir)?;
        file.as_file().set_permissions(private_permissions())?;
        serde_json::to_writer_pretty(file.as_file_mut(), metadata)?;
        writeln!(file)?;
        file.as_file_mut().sync_all()?;
        file.persist(self.dir.join(name)).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn enforce_retention(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut groups: HashMap<String, (DateTime<FixedOffset>, Option<PathBuf>, Option<PathBuf>)> =
            HashMap::new();
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_heap_profile_temp(&name) {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            let Some((base, time, is_profile)) = parse_heap_profile_file_name(&name) else {
                continue;
            };
            let group = groups.entry(base).or_insert((time, None, None));
            if is_profile {
                group.1 = Some(entry.path());
            } else {
                group.2 = Some(entry.path());
            }
        }
        let mut profiles = Vec::new();
        for (base, group) in groups {
            if group.1.is_none() {
                if let Some(path) = group.2 {
                    let _ = fs::remove_file(path);
                }
            } else {
                profiles.push((base, group));
            }
        }
        profiles.sort_by(|a, b| a.1.0.cmp(&b.1.0).then(a.0.cmp(&b.0)));
        let remove = profiles.len().saturating_sub(MAX_GROUPS);
        for (_, (_, profile, metadata)) in profiles.into_iter().take(remove) {
            if let Some(path) = profile {
                let _ = fs::remove_file(path);
            }
            if let Some(path) = metadata {
                let _ = fs::remove_file(path);
            }
        }
    }
}

impl HeapProfileRuntime for HeapProfileCollector {
    fn reset_trigger_state(&self) {
        *self.state.lock().expect("heap profile lock poisoned") = CollectorState::default();
    }

    fn should_check(&self) -> bool {
        let now = (self.now)();
        let mut state = self.state.lock().expect("heap profile lock poisoned");
        if state
            .last_check_at
            .is_some_and(|last| now.duration_since(last).unwrap_or_default() < CHECK_INTERVAL)
        {
            return false;
        }
        state.last_check_at = Some(now);
        true
    }

    fn try_capture(&self, m: &MemArbitrator) {
        let snapshot = Snapshot::from_arbitrator(m);
        if snapshot.limit <= 0 {
            return;
        }
        let current_ratio = calcRatio(snapshot.mem_inuse, snapshot.limit);
        let mut state = self.state.lock().expect("heap profile lock poisoned");
        let trigger = &mut state.trigger;
        if trigger.last_limit != snapshot.limit {
            trigger.last_limit = snapshot.limit;
            trigger.attempted = 0;
            trigger.emergency_last_capture_at = None;
            trigger.closed = false;
        }
        if current_ratio < RESET_MILLI {
            trigger.attempted = 0;
            trigger.emergency_last_capture_at = None;
            trigger.closed = false;
            return;
        }
        let now = (self.now)();
        if m.AtOOMRisk()
            && snapshot.quota_alloc == 0
            && snapshot.out_of_control > 0
            && trigger.emergency_last_capture_at.is_none_or(|last| {
                now.duration_since(last).unwrap_or_default() >= EMERGENCY_INTERVAL
            })
        {
            trigger.emergency_last_capture_at = Some(now);
            self.capture_locked(m, EMERGENCY_THRESHOLD, trigger);
        }
        if current_ratio >= CUTOFF_MILLI {
            trigger.closed = true;
        }
        if trigger.closed {
            return;
        }
        let mut highest = None;
        let mut reached_mask = 0;
        for (index, (ratio, _)) in LEVELS.iter().enumerate() {
            if current_ratio >= *ratio {
                reached_mask |= 1 << index;
                if trigger.attempted & (1 << index) == 0 {
                    highest = Some(index);
                }
            }
        }
        let Some(highest) = highest else {
            return;
        };
        let threshold = LEVELS[highest].1;
        if trigger.last_capture_at.is_some_and(|last| {
            threshold <= trigger.last_capture_threshold
                && now.duration_since(last).unwrap_or_default() < MIN_INTERVAL
        }) {
            return;
        }
        if self.capture_locked(m, threshold, trigger) {
            trigger.attempted |= reached_mask;
        }
    }
}

fn is_heap_profile_temp(name: &str) -> bool {
    (name.starts_with(".heap-profile.") || name.starts_with(".heap-metadata."))
        && name.ends_with(".tmp")
}

pub fn parse_heap_profile_file_name(name: &str) -> Option<(String, DateTime<FixedOffset>, bool)> {
    let (base, is_profile) = if let Some(base) = name.strip_suffix(".pprof") {
        (base, true)
    } else {
        (name.strip_suffix(METADATA_SUFFIX)?, false)
    };
    let (timestamp, threshold_text) = base.rsplit_once('.')?;
    let threshold_text = threshold_text.strip_suffix("pct")?;
    let threshold: i32 = threshold_text.parse().ok()?;
    if threshold.to_string() != threshold_text || ![70, 80, 85, 95].contains(&threshold) {
        return None;
    }
    let time = DateTime::parse_from_str(timestamp, TIMESTAMP_LAYOUT).ok()?;
    Some((base.to_owned(), time, is_profile))
}

#[cfg(unix)]
fn private_permissions() -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(0o600)
}

#[cfg(not(unix))]
fn private_permissions() -> fs::Permissions {
    fs::metadata(std::env::current_exe().expect("current executable path unavailable"))
        .expect("current executable metadata unavailable")
        .permissions()
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o750).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}
