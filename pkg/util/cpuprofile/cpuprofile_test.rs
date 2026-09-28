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

// `cpuprofile` 单元测试：全局启停、并行消费者投递、Collector 合并与 HTTP Handler。
//
// 用例串行执行（`#[serial]`），因进程级 pprof 采样器同一时刻只能有一个实例。

#![allow(non_snake_case)]

use std::hint::black_box;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::*;
use cpuprofile_testutil::{CancellationToken, mock_cpu_load};
use pprof::protos::{Label, Message, Profile};
use serial_test::serial;

/// RAII：启动全局 profiler，Drop 时停止。
struct RunningGlobalProfiler;

impl RunningGlobalProfiler {
    /// 重置全局状态后按给定间隔启动 profiler。
    fn start(interval: Duration) -> Self {
        // Match Go tests that replace globalCPUProfiler between cases.
        reset_global_profiler_for_test();
        set_profile_duration(interval);
        StartCPUProfiler().expect("first StartCPUProfiler succeeds");
        Self
    }
}

impl Drop for RunningGlobalProfiler {
    fn drop(&mut self) {
        StopCPUProfiler();
    }
}

/// 占用进程级原生采样器（用于制造冲突错误）。
fn native_profiler() -> Result<pprof::ProfilerGuard<'static>, pprof::Error> {
    pprof::ProfilerGuardBuilder::default()
        .frequency(100)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
}

/// 通过 Collector 采集一段时间内的 CPU profile 字节。
fn getCPUProfile(duration: Duration) -> Result<Vec<u8>, CpuProfileError> {
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut collector = NewCollector();
    collector.StartCPUProfile(shared_buffer_writer(output.clone()))?;
    thread::sleep(duration);
    collector.StopCPUProfile()?;
    let data = output
        .lock()
        .expect("profile output mutex is not poisoned")
        .clone();
    Ok(data)
}

/// 从全局 profiler 取一份真实 profile，并注入 sql/plan_digest 标签供合并测试。
fn capture_labelled_profile() -> Vec<u8> {
    let (sender, receiver) = crossbeam_channel::bounded(4);
    Register(Some(sender.clone()));
    let deadline = Instant::now() + Duration::from_secs(5);
    let data = loop {
        assert!(Instant::now() < deadline, "profile template timed out");
        black_box((0..100_000_u64).fold(0_u64, |sum, value| sum.wrapping_add(value)));
        if let Ok(data) = receiver.recv_timeout(Duration::from_millis(20)) {
            if data.Error.is_none() && !data.Data.is_empty() {
                break data;
            }
        }
    };
    Unregister(Some(sender));

    let mut profile = Profile::decode(data.Data.as_slice()).expect("template is valid pprof");
    let sql_key = profile.string_table.len() as i64;
    profile.string_table.push("sql".into());
    let sql_value = profile.string_table.len() as i64;
    profile.string_table.push("sql_digest".into());
    let plan_key = profile.string_table.len() as i64;
    profile.string_table.push("plan_digest".into());
    let plan_value = profile.string_table.len() as i64;
    profile.string_table.push("plan_digest_value".into());
    let sample = profile
        .sample
        .first_mut()
        .expect("CPU load produces at least one sample");
    sample.label.push(Label {
        key: sql_key,
        str: sql_value,
        num: 0,
        num_unit: 0,
    });
    sample.label.push(Label {
        key: plan_key,
        str: plan_value,
        num: 0,
        num_unit: 0,
    });
    profile.encode_to_vec()
}

// TestBasicAPI mirrors Go's duplicate Start, repeated Stop, and restart checks.
/// 重复 Start 失败、重复 Stop 安全、Stop 后可再次 Start。
#[test]
#[serial]
fn TestBasicAPI() {
    let _running = RunningGlobalProfiler::start(Duration::from_millis(200));

    assert_eq!(
        StartCPUProfiler().expect_err("a second Start must fail"),
        errProfilerAlreadyStarted()
    );

    StopCPUProfiler();
    StopCPUProfiler();

    StartCPUProfiler().expect("Start succeeds after Stop");
    assert_eq!(
        StartCPUProfiler().expect_err("a second Start after restart must fail"),
        errProfilerAlreadyStarted()
    );
}

// TestParallelCPUProfiler mirrors nil/duplicate/closed consumers, profile errors,
// successful delivery, and stopping the native sampler after the last consumer.
/// 覆盖 nil/重复/已关闭消费者、采样冲突错误、成功投递与最后消费者释放采样器。
#[test]
#[serial]
fn TestParallelCPUProfiler() {
    let _running = RunningGlobalProfiler::start(Duration::from_millis(40));

    Register(None);
    assert_eq!(global_consumers_count(), 0);
    Unregister(None);
    assert_eq!(global_consumers_count(), 0);

    let occupied = native_profiler().expect("reserve the process-global native profiler");
    let expected_error = match native_profiler() {
        Ok(_) => panic!("a second native profiler unexpectedly started"),
        Err(error) => error.to_string(),
    };
    let (error_sender, error_receiver) = crossbeam_channel::bounded(10);
    Register(Some(error_sender.clone()));
    Register(Some(error_sender.clone()));
    assert_eq!(global_consumers_count(), 1);

    let data = error_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("the native profiler conflict is delivered to the consumer");
    assert_eq!(
        data.Error.as_ref().map(ToString::to_string),
        Some(expected_error)
    );
    assert!(data.Data.is_empty());
    Unregister(Some(error_sender.clone()));
    assert_eq!(global_consumers_count(), 0);
    assert!(error_receiver.try_recv().is_err());
    Unregister(Some(error_sender));
    assert_eq!(global_consumers_count(), 0);
    drop(occupied);

    let (closed_sender, closed_receiver) = crossbeam_channel::bounded(10);
    drop(closed_receiver);
    Register(Some(closed_sender.clone()));
    assert_eq!(global_consumers_count(), 1);
    thread::sleep(Duration::from_millis(100));
    Unregister(Some(closed_sender));
    assert_eq!(global_consumers_count(), 0);

    let (sender, receiver) = crossbeam_channel::bounded(10);
    Register(Some(sender.clone()));
    let deadline = Instant::now() + Duration::from_secs(5);
    let data = loop {
        assert!(Instant::now() < deadline, "profile delivery timed out");
        black_box((0..100_000_u64).fold(0_u64, |sum, value| sum.wrapping_add(value)));
        if let Ok(data) = receiver.recv_timeout(Duration::from_millis(20)) {
            if data.Error.is_none() {
                break data;
            }
        }
    };
    assert!(!data.Data.is_empty());
    Profile::decode(data.Data.as_slice()).expect("consumer data is valid pprof protobuf");
    Unregister(Some(sender.clone()));
    assert_eq!(global_consumers_count(), 0);

    Register(Some(sender.clone()));
    let count_before = cpu_profile_count();
    let deadline = Instant::now() + Duration::from_secs(2);
    while cpu_profile_count() == count_before && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        cpu_profile_count() > count_before,
        "profiling did not start"
    );
    Unregister(Some(sender));
    assert_eq!(global_consumers_count(), 0);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(guard) = native_profiler() {
            drop(guard);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the native profiler was not released after the last consumer"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

// TestGetCPUProfile mirrors the occupied-profiler error and ten concurrent
// collectors. Injected labels stand in for Go runtime/pprof labels while the
// real load and collector merge/filter paths still execute.
/// 占用采样器时 Collector 报错；十并发 Collector 合并注入标签后仍保留 sql label。
#[test]
#[serial]
fn TestGetCPUProfile() {
    let _running = RunningGlobalProfiler::start(Duration::from_millis(100));

    let occupied = native_profiler().expect("reserve the process-global native profiler");
    let expected_error = match native_profiler() {
        Ok(_) => panic!("a second native profiler unexpectedly started"),
        Err(error) => error.to_string(),
    };
    assert_eq!(
        getCPUProfile(Duration::from_millis(50))
            .expect_err("collector reports the occupied native profiler")
            .to_string(),
        expected_error
    );
    drop(occupied);

    let cancellation = CancellationToken::new();
    let load = mock_cpu_load(&cancellation, ["sql", "sql_digest", "plan_digest"]);
    let labelled_profile = capture_labelled_profile();
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let barrier = barrier.clone();
        let labelled_profile = labelled_profile.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            let output = Arc::new(Mutex::new(Vec::new()));
            let mut collector = NewCollector();
            collector
                .StartCPUProfile(shared_buffer_writer(output.clone()))
                .expect("collector starts");
            collector
                .handleProfileData(&ProfileData::success(labelled_profile))
                .expect("the labelled profile is accepted");

            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(1) {
                black_box((0..100_000_u64).fold(0_u64, |sum, value| sum.wrapping_add(value)));
            }
            let before_stop = collector
                .buildProfileData()
                .expect("collector state is valid")
                .expect("collector retained at least the injected profile");
            assert!(!before_stop.sample.is_empty());
            collector.StopCPUProfile().expect("collector stops");

            let bytes = output
                .lock()
                .expect("profile output mutex is not poisoned")
                .clone();
            let profile = Profile::decode(bytes.as_slice()).expect("valid merged pprof profile");
            let mut label_count = 0;
            for sample in &profile.sample {
                for label in &sample.label {
                    assert_eq!(profile.string_table[label.key as usize], "sql");
                    label_count += 1;
                }
            }
            assert!(
                label_count > 0,
                "the sql label is preserved; strings={:?}, sample_labels={:?}",
                profile.string_table,
                profile
                    .sample
                    .iter()
                    .map(|sample| &sample.label)
                    .collect::<Vec<_>>()
            );
        }));
    }
    for handle in handles {
        handle.join().expect("collector worker completes");
    }
    cancellation.cancel();
    assert!(load.join_timeout(Duration::from_secs(2)));
}

// TestProfileHTTPHandler mirrors the successful profile response and the
// WriteTimeout rejection. The Rust production API is transport-independent, so
// the test drives its request/response adapter values directly.
/// HTTP Handler：正常返回 pprof 体；seconds 超过 WriteTimeout 时返回 400。
#[test]
#[serial]
fn TestProfileHTTPHandler() {
    let _running = RunningGlobalProfiler::start(Duration::from_millis(100));

    let request = HttpRequest {
        seconds: Some("1".into()),
        write_timeout: Some(Duration::from_secs(60)),
    };
    let mut response = HttpResponse::default();
    ProfileHTTPHandler(&mut response, &request);
    assert_eq!(response.status, 200);
    assert_eq!(response.headers["X-Content-Type-Options"], "nosniff");
    assert_eq!(response.headers["Content-Type"], "application/octet-stream");
    Profile::decode(response.body.as_slice()).expect("HTTP body is valid pprof protobuf");

    let rejected = HttpRequest {
        seconds: Some("100000".into()),
        write_timeout: Some(Duration::from_secs(60)),
    };
    let mut rejected_response = HttpResponse::default();
    ProfileHTTPHandler(&mut rejected_response, &rejected);
    assert_eq!(rejected_response.status, 400);
    assert_eq!(
        rejected_response.body,
        b"profile duration exceeds server's WriteTimeout\n"
    );
}
