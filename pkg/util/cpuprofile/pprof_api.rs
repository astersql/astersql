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

// pprof 采集器与 HTTP 剖析入口。
//
// 提供与 Go `net/http/pprof` 风格相近的请求/响应结构、`Collector`（注册为
// 全局剖析消费者、合并多段 Profile、写出 protobuf），以及秒数解析与超时校验。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pprof::protos::{Message, Profile};

use crate::{
    CpuProfileError, ProfileConsumer, ProfileData, Register, Unregister, profile_duration,
};

/// 线程安全的剖析结果写入器（互斥保护的 `Write`）。
pub type ProfileWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// 将写入转发到共享 `Vec<u8>` 缓冲区，便于测试捕获输出。
struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBufferWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("profile output mutex poisoned"))?
            .write(data)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 由共享缓冲区构造 `ProfileWriter`。
pub fn shared_buffer_writer(buffer: Arc<Mutex<Vec<u8>>>) -> ProfileWriter {
    Arc::new(Mutex::new(Box::new(SharedBufferWriter(buffer))))
}

/// 精简 HTTP 请求视图：剖析时长与服务端写超时。
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    /// 查询参数 `seconds`，非法或非正时回落默认 30。
    pub seconds: Option<String>,
    /// 服务端 WriteTimeout；用于拒绝过长的剖析请求。
    pub write_timeout: Option<Duration>,
}

/// 精简 HTTP 响应：状态码、头与正文。
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Default for HttpResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }
}

/// Equivalent to Go's pprof profile handler, but expressed with small native
/// request/response values so it can be adapted to any Rust HTTP server.
/// 等价于 Go pprof profile handler：校验超时后采集固定秒数并写回 protobuf。
pub fn ProfileHTTPHandler(response: &mut HttpResponse, request: &HttpRequest) {
    response
        .headers
        .insert("X-Content-Type-Options".into(), "nosniff".into());
    let seconds = parse_profile_seconds(request.seconds.as_deref());
    if durationExceedsWriteTimeout(request, seconds as f64) {
        serveError(
            response,
            400,
            "profile duration exceeds server's WriteTimeout",
        );
        return;
    }

    response
        .headers
        .insert("Content-Type".into(), "application/octet-stream".into());
    response.headers.insert(
        "Content-Disposition".into(),
        r#"attachment; filename="profile""#.into(),
    );

    // 启动采集 → 睡眠 seconds → 停止并取出缓冲中的 pprof 字节。
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut collector = NewCollector();
    if let Err(error) = collector.StartCPUProfile(shared_buffer_writer(output.clone())) {
        serveError(
            response,
            500,
            &format!("Could not enable CPU profiling: {error}"),
        );
        return;
    }
    std::thread::sleep(Duration::from_secs(seconds as u64));
    if let Err(error) = collector.StopCPUProfile() {
        serveError(
            response,
            500,
            &format!("Could not enable CPU profiling: {error}"),
        );
        return;
    }
    response.body = output
        .lock()
        .expect("profile response mutex poisoned")
        .clone();
}

/// 采集工作线程与主线程共享的合并结果 / 错误状态。
#[derive(Default)]
struct CollectorState {
    err: Option<CpuProfileError>,
    result: Option<Profile>,
}

/// CPU profile 消费者：注册到全局剖析器，合并多段数据并写出 pprof。
pub struct Collector {
    cancelled: Arc<AtomicBool>,
    writer: Option<ProfileWriter>,
    first_read: crossbeam_channel::Sender<()>,
    first_read_recv: crossbeam_channel::Receiver<()>,
    data_ch: ProfileConsumer,
    data_recv: crossbeam_channel::Receiver<Arc<ProfileData>>,
    state: Arc<Mutex<CollectorState>>,
    wg: Option<JoinHandle<()>>,
    started: bool,
}

/// 构造未启动的空 `Collector`。
pub fn NewCollector() -> Collector {
    let (first_read, first_read_recv) = crossbeam_channel::bounded(1);
    let (data_ch, data_recv) = crossbeam_channel::bounded(1);
    Collector {
        cancelled: Arc::new(AtomicBool::new(false)),
        writer: None,
        first_read,
        first_read_recv,
        data_ch,
        data_recv,
        state: Arc::new(Mutex::new(CollectorState::default())),
        wg: None,
        started: false,
    }
}

impl Collector {
    /// 启动后台线程：注册为全局消费者并持续合并收到的 ProfileData。
    pub fn StartCPUProfile(&mut self, writer: ProfileWriter) -> Result<(), CpuProfileError> {
        if self.started {
            return Err(CpuProfileError::new("Collector already started"));
        }
        self.started = true;
        self.writer = Some(writer);
        self.cancelled.store(false, Ordering::SeqCst);
        *self.state.lock().expect("collector state mutex poisoned") = CollectorState::default();
        // 清空上次残留的 first_read 信号。
        while self.first_read_recv.try_recv().is_ok() {}

        let cancelled = self.cancelled.clone();
        let data_ch = self.data_ch.clone();
        let data_recv = self.data_recv.clone();
        let first_read = self.first_read.clone();
        let state = self.state.clone();
        self.wg = Some(std::thread::spawn(move || {
            Register(Some(data_ch.clone()));
            let mut is_first_read = true;
            while !cancelled.load(Ordering::SeqCst) {
                match data_recv.recv_timeout(Duration::from_millis(10)) {
                    Ok(data) => {
                        let result = handle_profile_data(&state, data.as_ref());
                        if is_first_read {
                            is_first_read = false;
                            let _ = first_read.try_send(());
                        }
                        if result.is_err() {
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
            Unregister(Some(data_ch));
        }));
        Ok(())
    }

    /// 等待首包、停止工作线程，编码合并结果并写入 `writer`。
    pub fn StopCPUProfile(&mut self) -> Result<(), CpuProfileError> {
        if !self.started {
            return Ok(());
        }
        // 与 Go 一致，最多等待 DefProfileDuration*2 收到首包。
        let first_read_timeout = profile_duration().saturating_mul(2);
        let _ = self.first_read_recv.recv_timeout(first_read_timeout);
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(handle) = self.wg.take() {
            handle
                .join()
                .map_err(|_| CpuProfileError::new("collector worker panicked"))?;
        }

        let Some(profile) = self.buildProfileData()? else {
            return Ok(());
        };
        let mut encoded = Vec::new();
        profile.encode(&mut encoded)?;
        if encoded.is_empty() {
            return Err(CpuProfileError::new(
                "cpu profile collector encoded an empty profile",
            ));
        }
        if let Some(writer) = &self.writer {
            writer
                .lock()
                .map_err(|_| CpuProfileError::new("profile writer mutex poisoned"))?
                .write_all(&encoded)?;
        }
        Ok(())
    }

    /// 同步处理一段 ProfileData（测试或直接注入路径）。
    pub fn handleProfileData(&mut self, data: &ProfileData) -> Result<(), CpuProfileError> {
        handle_profile_data(&self.state, data)
    }

    /// 取出合并后的 Profile，并过滤非 `sql` 标签。
    pub fn buildProfileData(&mut self) -> Result<Option<Profile>, CpuProfileError> {
        let state = self.state.lock().expect("collector state mutex poisoned");
        if let Some(error) = &state.err {
            return Err(error.clone());
        }
        let Some(mut profile) = state.result.clone() else {
            return Ok(None);
        };
        drop(state);
        self.removeLabel(&mut profile);
        Ok(Some(profile))
    }

    /// 仅保留 key 为 `sql` 的样本标签（对齐 Go TopSQL 相关过滤）。
    pub fn removeLabel(&self, profile: &mut Profile) {
        for sample in &mut profile.sample {
            sample.label.retain(|label| {
                profile
                    .string_table
                    .get(label.key as usize)
                    .is_some_and(|key| key == labelSQL)
            });
        }
    }

    /// 测试辅助：直接写入错误到共享状态。
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state
            .lock()
            .expect("collector state mutex poisoned")
            .err = Some(CpuProfileError::new(message));
    }

    /// 测试辅助：向消费者通道注入一段 ProfileData。
    pub fn inject_profile_for_test(&self, data: ProfileData) {
        let _ = self.data_ch.send(Arc::new(data));
    }
}

/// 保留的样本标签名：SQL digest。
const labelSQL: &str = "sql";

/// 解码并合并一段 ProfileData；遇 Error 字段则记入状态并返回。
fn handle_profile_data(
    state: &Arc<Mutex<CollectorState>>,
    data: &ProfileData,
) -> Result<(), CpuProfileError> {
    if let Some(error) = &data.Error {
        let error = error.clone();
        state.lock().expect("collector state mutex poisoned").err = Some(error.clone());
        return Err(error);
    }
    let profile = Profile::decode(data.Data.as_slice())?;
    let mut state = state.lock().expect("collector state mutex poisoned");
    state.result = Some(match state.result.take() {
        Some(existing) => merge_profiles(existing, profile)?,
        None => profile,
    });
    Ok(())
}

/// 将 source Profile 合并进 destination：重映射字符串表与 ID，再追加样本与元数据。
fn merge_profiles(
    mut destination: Profile,
    mut source: Profile,
) -> Result<Profile, CpuProfileError> {
    validate_compatible_sample_types(&destination, &source)?;
    if destination.string_table.is_empty() {
        destination.string_table.push(String::new());
    }
    if source.string_table.is_empty() {
        source.string_table.push(String::new());
    }

    // 建立 destination 字符串 → 下标映射，并把 source 字符串并入同一表。
    let mut string_ids: HashMap<String, i64> = destination
        .string_table
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, value)| (value, index as i64))
        .collect();
    let mut remap = Vec::with_capacity(source.string_table.len());
    for value in &source.string_table {
        let id = match string_ids.get(value) {
            Some(id) => *id,
            None => {
                let id = destination.string_table.len() as i64;
                destination.string_table.push(value.clone());
                string_ids.insert(value.clone(), id);
                id
            }
        };
        remap.push(id);
    }
    remap_profile_strings(&mut source, &remap)?;

    // ID 偏移：避免与 destination 已有 mapping/function/location 冲突。
    let mapping_offset = destination
        .mapping
        .iter()
        .map(|value| value.id)
        .max()
        .unwrap_or(0);
    let function_offset = destination
        .function
        .iter()
        .map(|value| value.id)
        .max()
        .unwrap_or(0);
    let location_offset = destination
        .location
        .iter()
        .map(|value| value.id)
        .max()
        .unwrap_or(0);

    for mapping in &mut source.mapping {
        mapping.id = mapping.id.saturating_add(mapping_offset);
    }
    for function in &mut source.function {
        function.id = function.id.saturating_add(function_offset);
    }
    for location in &mut source.location {
        location.id = location.id.saturating_add(location_offset);
        if location.mapping_id != 0 {
            location.mapping_id = location.mapping_id.saturating_add(mapping_offset);
        }
        for line in &mut location.line {
            if line.function_id != 0 {
                line.function_id = line.function_id.saturating_add(function_offset);
            }
        }
    }
    for sample in &mut source.sample {
        for location_id in &mut sample.location_id {
            if *location_id != 0 {
                *location_id = location_id.saturating_add(location_offset);
            }
        }
    }

    // 时间窗口取并集；period 等标量取较大/补缺。
    let destination_end = destination
        .time_nanos
        .saturating_add(destination.duration_nanos);
    let source_end = source.time_nanos.saturating_add(source.duration_nanos);
    let start = match (destination.time_nanos, source.time_nanos) {
        (0, right) => right,
        (left, 0) => left,
        (left, right) => left.min(right),
    };
    let end = destination_end.max(source_end);
    destination.time_nanos = start;
    destination.duration_nanos = end.saturating_sub(start);
    destination.period = destination.period.max(source.period);
    if destination.period_type.is_none() {
        destination.period_type = source.period_type.take();
    }
    if destination.drop_frames == 0 {
        destination.drop_frames = source.drop_frames;
    }
    if destination.keep_frames == 0 {
        destination.keep_frames = source.keep_frames;
    }
    if destination.default_sample_type == 0 {
        destination.default_sample_type = source.default_sample_type;
    }
    destination.comment.append(&mut source.comment);
    destination.sample.append(&mut source.sample);
    destination.mapping.append(&mut source.mapping);
    destination.location.append(&mut source.location);
    destination.function.append(&mut source.function);
    Ok(destination)
}

/// 校验两端 sample_type 的 type/unit 字符串一致，否则拒绝合并。
fn validate_compatible_sample_types(
    destination: &Profile,
    source: &Profile,
) -> Result<(), CpuProfileError> {
    if destination.sample_type.len() != source.sample_type.len() {
        return Err(CpuProfileError::new("incompatible pprof sample types"));
    }
    for (left, right) in destination.sample_type.iter().zip(&source.sample_type) {
        let left_type = profile_string(destination, left.ty)?;
        let right_type = profile_string(source, right.ty)?;
        let left_unit = profile_string(destination, left.unit)?;
        let right_unit = profile_string(source, right.unit)?;
        if left_type != right_type || left_unit != right_unit {
            return Err(CpuProfileError::new("incompatible pprof sample types"));
        }
    }
    Ok(())
}

/// 按 string_table 下标取字符串。
fn profile_string(profile: &Profile, index: i64) -> Result<&str, CpuProfileError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| profile.string_table.get(index))
        .map(String::as_str)
        .ok_or_else(|| CpuProfileError::new("pprof string index is out of bounds"))
}

/// 将 Profile 内所有字符串下标按 remap 表改写到目标 string_table。
fn remap_profile_strings(profile: &mut Profile, remap: &[i64]) -> Result<(), CpuProfileError> {
    /// 单一下标重映射。
    fn remap_one(index: &mut i64, remap: &[i64]) -> Result<(), CpuProfileError> {
        let mapped = usize::try_from(*index)
            .ok()
            .and_then(|index| remap.get(index))
            .copied()
            .ok_or_else(|| CpuProfileError::new("pprof string index is out of bounds"))?;
        *index = mapped;
        Ok(())
    }

    for value_type in &mut profile.sample_type {
        remap_one(&mut value_type.ty, remap)?;
        remap_one(&mut value_type.unit, remap)?;
    }
    if let Some(period_type) = &mut profile.period_type {
        remap_one(&mut period_type.ty, remap)?;
        remap_one(&mut period_type.unit, remap)?;
    }
    for sample in &mut profile.sample {
        for label in &mut sample.label {
            remap_one(&mut label.key, remap)?;
            remap_one(&mut label.str, remap)?;
            remap_one(&mut label.num_unit, remap)?;
        }
    }
    for mapping in &mut profile.mapping {
        remap_one(&mut mapping.filename, remap)?;
        remap_one(&mut mapping.build_id, remap)?;
    }
    for function in &mut profile.function {
        remap_one(&mut function.name, remap)?;
        remap_one(&mut function.system_name, remap)?;
        remap_one(&mut function.filename, remap)?;
    }
    remap_one(&mut profile.drop_frames, remap)?;
    remap_one(&mut profile.keep_frames, remap)?;
    remap_one(&mut profile.default_sample_type, remap)?;
    for comment in &mut profile.comment {
        remap_one(comment, remap)?;
    }
    Ok(())
}

/// 解析剖析秒数；缺失、非法或非正时默认 30。
pub fn parse_profile_seconds(value: Option<&str>) -> i64 {
    value
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(30)
}

/// 若请求配置了非零 WriteTimeout 且剖析秒数达到/超过该超时，则返回 true。
pub fn durationExceedsWriteTimeout(request: &HttpRequest, seconds: f64) -> bool {
    request
        .write_timeout
        .is_some_and(|timeout| timeout != Duration::ZERO && seconds >= timeout.as_secs_f64())
}

/// 按 Go pprof 约定写明文错误：Content-Type、X-Go-Pprof，并去掉 Content-Disposition。
pub fn serveError(response: &mut HttpResponse, status: u16, text: &str) {
    response
        .headers
        .insert("Content-Type".into(), "text/plain; charset=utf-8".into());
    response.headers.insert("X-Go-Pprof".into(), "1".into());
    response.headers.remove("Content-Disposition");
    response.status = status;
    response.body.clear();
    response.body.extend_from_slice(text.as_bytes());
    response.body.push(b'\n');
}
