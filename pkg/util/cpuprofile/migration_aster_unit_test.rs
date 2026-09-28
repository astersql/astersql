// Copyright 2026 AsterSQL.

// `cpuprofile` 迁移期单元测试：对齐 Go 全局剖析器、采集器与 HTTP 行为。
//
// 覆盖：Start/Stop/Register 生命周期、非阻塞分发、pprof 合并与 sql 标签保留、
// HTTP 默认秒数/超时拒绝，以及端到端 ProfileHTTPHandler 输出。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pprof::protos::{Label, Message, Profile, Sample, ValueType};
use serial_test::serial;
use util_cpuprofile::*;

/// 构造带指定标签与采样值的最小 pprof protobuf 字节流，供采集器测试注入。
fn encoded_profile(labels: &[(&str, &str)], value: i64) -> Vec<u8> {
    // string_table[0] 必须为空串；1/2 分别为 sample type 的 type/unit 名。
    let mut strings = vec![String::new(), "samples".into(), "count".into()];
    let mut profile_labels = Vec::new();
    for (key, label_value) in labels {
        let key_index = strings.len() as i64;
        strings.push((*key).into());
        let value_index = strings.len() as i64;
        strings.push((*label_value).into());
        profile_labels.push(Label {
            key: key_index,
            str: value_index,
            num: 0,
            num_unit: 0,
        });
    }
    let profile = Profile {
        sample_type: vec![ValueType { ty: 1, unit: 2 }],
        sample: vec![Sample {
            location_id: Vec::new(),
            value: vec![value],
            label: profile_labels,
        }],
        string_table: strings,
        duration_nanos: 10,
        ..Profile::default()
    };
    profile.encode_to_vec()
}

/// 校验全局剖析器启停、重复 Start 报错、消费者去重与数据投递。
#[test]
#[serial]
fn global_profiler_matches_go_start_stop_registration_and_delivery() {
    // Go TestBasicAPI replaces globalCPUProfiler; clear leftover consumers first.
    // 先清空全局状态，避免串行测试间残留消费者。
    reset_global_profiler_for_test();
    set_profile_duration(Duration::from_millis(40));

    StartCPUProfiler().expect("first start succeeds");
    assert_eq!(
        StartCPUProfiler().unwrap_err().to_string(),
        "parallelCPUProfiler is already started"
    );

    Register(None);
    assert_eq!(global_consumers_count(), 0);

    let cancellation = CancellationToken::new();
    let load = mock_cpu_load(&cancellation, ["global-profiler"]);
    let (sender, receiver) = crossbeam_channel::bounded(2);
    Register(Some(sender.clone()));
    Register(Some(sender.clone()));
    assert_eq!(global_consumers_count(), 1, "duplicate channel is ignored");

    // macOS 上 pprof 首次初始化系统采样器可能耗时数十秒；仍须收到真实可解码数据。
    let first_profile_timeout = if cfg!(target_os = "macos") {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(15)
    };
    let deadline = Instant::now() + first_profile_timeout;
    let mut first_error = None;
    let mut empty_profiles = 0;
    let data = loop {
        if Instant::now() >= deadline {
            cancellation.cancel();
            assert!(load.join_timeout(Duration::from_secs(2)));
            StopCPUProfiler();
            panic!(
                "profiler did not deliver data before timeout (first error: {first_error:?}, empty profiles: {empty_profiles})"
            );
        }
        if let Ok(data) = receiver.recv_timeout(Duration::from_millis(20)) {
            if let Some(error) = &data.Error {
                first_error.get_or_insert_with(|| error.to_string());
            } else if data.Data.is_empty() {
                empty_profiles += 1;
            } else {
                break data;
            }
        }
    };
    Profile::decode(data.Data.as_slice()).expect("delivery is valid pprof protobuf");
    cancellation.cancel();
    assert!(load.join_timeout(Duration::from_secs(2)));

    Unregister(Some(sender));
    assert_eq!(global_consumers_count(), 0);
    StopCPUProfiler();
    StopCPUProfiler();
}

/// 校验向已满消费者通道分发时丢弃新 profile，并清空当前缓冲。
#[test]
#[serial]
fn distribution_is_nonblocking_and_clears_the_current_profile() {
    let mut profiler = newParallelCPUProfiler();
    let (sender, receiver) = crossbeam_channel::bounded(1);
    profiler.register(sender.clone());
    profiler.register(sender.clone());
    assert_eq!(profiler.consumersCount(), 1);

    profiler.set_profile_data(ProfileData::success(vec![1, 2, 3]));
    profiler.sendToConsumers();
    profiler.set_profile_data(ProfileData::success(vec![4, 5, 6]));
    profiler.sendToConsumers();

    assert_eq!(receiver.recv().unwrap().Data, vec![1, 2, 3]);
    assert!(
        receiver.try_recv().is_err(),
        "full consumer drops the latest profile"
    );
    assert!(!profiler.has_profile_data());
}

/// 校验采集器合并多样本并仅保留 `sql` 标签（去掉 plan_digest/trace 等）。
#[test]
#[serial]
fn collector_merges_profiles_and_removes_non_sql_labels() {
    let mut collector = NewCollector();
    collector
        .handleProfileData(&ProfileData::success(encoded_profile(
            &[("sql", "digest-a"), ("plan_digest", "plan-a")],
            3,
        )))
        .unwrap();
    collector
        .handleProfileData(&ProfileData::success(encoded_profile(
            &[("sql", "digest-b"), ("trace", "trace-b")],
            7,
        )))
        .unwrap();

    let profile = collector.buildProfileData().unwrap().unwrap();
    assert_eq!(profile.sample.len(), 2);
    assert_eq!(profile.sample.iter().map(|s| s.value[0]).sum::<i64>(), 10);
    for sample in &profile.sample {
        assert_eq!(sample.label.len(), 1);
        assert_eq!(profile.string_table[sample.label[0].key as usize], "sql");
    }
}

/// 校验错误传播，以及 Start/Stop 后写出可解码的 pprof 字节。
#[test]
#[serial]
fn collector_propagates_profile_errors_and_writes_valid_pprof() {
    let mut collector = NewCollector();
    collector
        .handleProfileData(&ProfileData::failure("cpu profiling already in use"))
        .unwrap_err();
    collector.set_error("cpu profiling already in use");
    assert_eq!(
        collector.buildProfileData().unwrap_err().to_string(),
        "cpu profiling already in use"
    );

    let output = Arc::new(Mutex::new(Vec::new()));
    let mut writer_collector = NewCollector();
    writer_collector
        .StartCPUProfile(shared_buffer_writer(output.clone()))
        .unwrap();
    writer_collector.inject_profile_for_test(ProfileData::success(encoded_profile(
        &[("sql", "digest")],
        1,
    )));
    writer_collector.StopCPUProfile().unwrap();
    let bytes = output.lock().unwrap().clone();
    Profile::decode(bytes.as_slice()).expect("collector writes pprof protobuf");
}

/// 校验解析秒数默认值、写超时判定与 serveError 响应头/正文格式。
#[test]
#[serial]
fn http_helpers_match_go_defaults_timeout_and_error_headers() {
    assert_eq!(parse_profile_seconds(None), 30);
    assert_eq!(parse_profile_seconds(Some("invalid")), 30);
    assert_eq!(parse_profile_seconds(Some("0")), 30);
    assert_eq!(parse_profile_seconds(Some("9")), 9);

    let request = HttpRequest {
        seconds: Some("60".into()),
        write_timeout: Some(Duration::from_secs(60)),
    };
    assert!(durationExceedsWriteTimeout(&request, 60.0));
    assert!(!durationExceedsWriteTimeout(&request, 59.0));

    let mut response = HttpResponse::default();
    response
        .headers
        .insert("Content-Disposition".into(), "attachment".into());
    serveError(
        &mut response,
        400,
        "profile duration exceeds server's WriteTimeout",
    );
    assert_eq!(response.status, 400);
    assert_eq!(
        response.headers["Content-Type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.headers["X-Go-Pprof"], "1");
    assert!(!response.headers.contains_key("Content-Disposition"));
    assert_eq!(
        response.body,
        b"profile duration exceeds server's WriteTimeout\n"
    );
}

/// 端到端：短时剖析返回 200+pprof；超长 duration 返回 400。
#[test]
#[serial]
fn http_handler_returns_pprof_and_rejects_excessive_duration() {
    StopCPUProfiler();
    set_profile_duration(Duration::from_millis(100));
    StartCPUProfiler().unwrap();

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

    StopCPUProfiler();
}
