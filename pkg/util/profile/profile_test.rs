// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Collector / ProfileGraph 单测：模拟 performance_schema 各 profile 表的分发。
//
// 对应 Go `TestProfiles`：启动全局 CPU profiler、覆盖采样间隔，并校验
// cpu/heap/allocs/mutex/block/goroutine 查询均成功。

use std::hint::black_box;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use types::datum::{Datum, KindInt64, KindString};

use crate::{
    Collector, ProfileError, RuntimeProfileProvider, cpu_profile_interval,
    install_runtime_profile_provider, set_cpu_profile_interval,
};

/// 将一行 Datum 转为可比较的字符串向量。
fn row_strings(row: &[Datum]) -> Vec<String> {
    row.iter()
        .map(|datum| {
            let kind = datum.Kind();
            if kind == KindString {
                datum.GetString()
            } else if kind == KindInt64 {
                datum.GetInt64().to_string()
            } else {
                panic!("unexpected datum kind {kind}")
            }
        })
        .collect()
}

/// RAII：Drop 时恢复原先的 CPU 采样间隔。
struct RestoreInterval(Duration);

impl Drop for RestoreInterval {
    fn drop(&mut self) {
        set_cpu_profile_interval(self.0);
    }
}

/// RAII：构造时启动全局 CPU profiler，Drop 时停止。
struct RunningGlobalProfiler;

impl RunningGlobalProfiler {
    /// 重置残留消费者后启动全局 profiler（对齐 Go domain bootstrap）。
    fn start() -> Self {
        // Go TestProfiles bootstraps a session/domain, which starts the process
        // global parallel CPU profiler before the performance_schema queries.
        // Match cpuprofile tests: reset leftover consumers before Start.
        cpuprofile::reset_global_profiler_for_test();
        cpuprofile::set_profile_duration(Duration::from_millis(200));
        cpuprofile::StartCPUProfiler().expect("StartCPUProfiler");
        let running = Self;

        // Go reaches this test through a bootstrapped domain, so its global
        // profiler has already completed at least one interval. Warm the Rust
        // pprof backend to the same state; the first report may spend several
        // seconds symbolizing on macOS even with a 200 ms sampling interval.
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut warmup = cpuprofile::NewCollector();
        warmup
            .StartCPUProfile(cpuprofile::shared_buffer_writer(Arc::clone(&output)))
            .expect("start CPU profiler warmup");
        let timeout = if cfg!(target_os = "macos") {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(15)
        };
        let started = Instant::now();
        while started.elapsed() < timeout && output.lock().expect("warmup output mutex").is_empty()
        {
            black_box((0..100_000_u64).fold(0_u64, |sum, value| sum.wrapping_add(value)));
        }
        warmup.StopCPUProfile().expect("stop CPU profiler warmup");
        assert!(
            !output.lock().expect("warmup output mutex").is_empty(),
            "global CPU profiler did not become ready"
        );
        running
    }
}

impl Drop for RunningGlobalProfiler {
    fn drop(&mut self) {
        cpuprofile::StopCPUProfiler();
    }
}

/// 在 stop 标志为真前持续空转，保证 CPU 采样窗口非空。
struct CpuLoad {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl CpuLoad {
    fn spawn() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                black_box((0..50_000_u64).fold(0_u64, |sum, value| sum.wrapping_add(value)));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for CpuLoad {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 模拟 Go runtime/pprof 的 Lookup + WriteTo 边界。
struct MockRuntimeProfileProvider {
    calls: Arc<Mutex<Vec<(String, i32)>>>,
}

impl RuntimeProfileProvider for MockRuntimeProfileProvider {
    fn write_profile(
        &self,
        name: &str,
        debug: i32,
        writer: &mut dyn Write,
    ) -> Result<bool, ProfileError> {
        self.calls
            .lock()
            .expect("calls mutex")
            .push((name.to_owned(), debug));
        match name {
            "heap" | "allocs" | "mutex" | "block" => {
                writer
                    .write_all(include_bytes!("testdata/test.pprof"))
                    .map_err(|error| ProfileError::new(error.to_string()))?;
                Ok(true)
            }
            "goroutine" => {
                writer
                    .write_all(
                        b"goroutine 18 [running]:\nmain.first()\n /tmp/main.go:1\n\
                          main.second()\n /tmp/main.go:2\n",
                    )
                    .map_err(|error| ProfileError::new(error.to_string()))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

// test_profiles 对应 Go 的 TestProfiles。
//
// Go boots a mock store / session and runs:
//   select * from performance_schema.tidb_profile_{cpu,memory,allocs,mutex,block,goroutines}
// Those virtual tables call Collector::ProfileGraph with the names below
// (see infoschema/perfschema getRows). The Rust session + performance_schema
// SQL stack is not fully wired for this util crate, so exercise the same
// Collector dispatch and CPUProfileInterval override/restore here — with the
// global profiler started the way domain bootstrap does in Go.
/// 校验 Go TestProfiles 覆盖的六类 ProfileGraph 查询均成功。
#[test]
fn test_profiles() {
    // Go defer view.Stop(): OpenCensus is unused in the Rust port.
    let _profiler = RunningGlobalProfiler::start();
    let _restore = RestoreInterval(cpu_profile_interval());
    // Go sets profile.CPUProfileInterval = 2 * time.Second for this test.
    set_cpu_profile_interval(Duration::from_secs(2));
    assert_eq!(cpu_profile_interval(), Duration::from_secs(2));

    let collector = Collector::default();

    // Keep a worker busy so the CPU sample window is non-empty.
    let load = CpuLoad::spawn();

    // performance_schema.tidb_profile_cpu → ProfileGraph("cpu")
    let cpu_rows = collector
        .ProfileGraph("cpu")
        .expect("cpu profile collection");
    drop(load);

    assert!(
        !cpu_rows.is_empty(),
        "cpu profile should produce at least root"
    );
    assert_eq!(row_strings(&cpu_rows[0])[0], "root");

    let calls = Arc::new(Mutex::new(Vec::new()));
    let _provider = install_runtime_profile_provider(Arc::new(MockRuntimeProfileProvider {
        calls: Arc::clone(&calls),
    }));

    // Remaining tables map to runtime/pprof names. The provider is the Rust
    // runtime boundary corresponding to Go's Lookup + WriteTo pair.
    for (sql_table, profile_name) in [
        ("tidb_profile_memory", "heap"),
        ("tidb_profile_allocs", "allocs"),
        ("tidb_profile_mutex", "mutex"),
        ("tidb_profile_block", "block"),
        ("tidb_profile_goroutines", "goroutine"),
    ] {
        let rows = collector
            .ProfileGraph(profile_name)
            .unwrap_or_else(|error| panic!("{sql_table}: {error}"));
        assert!(
            !rows.is_empty(),
            "{sql_table}: ProfileGraph({profile_name}) returned no rows"
        );
    }

    let error = match collector.ProfileGraph("unknown") {
        Ok(_) => panic!("unknown runtime profile must preserve Go Lookup failure"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "cannot retrieve unknown profile");
    assert_eq!(
        *calls.lock().expect("calls mutex"),
        vec![
            ("heap".to_owned(), 0),
            ("allocs".to_owned(), 0),
            ("mutex".to_owned(), 0),
            ("block".to_owned(), 0),
            ("goroutine".to_owned(), 2),
            ("unknown".to_owned(), 0),
        ]
    );
}
