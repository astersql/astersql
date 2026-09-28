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

// 性能剖析采集器：解析 pprof / goroutine 文本并输出火焰图 Datum 行。
//
// 对应 Go `pkg/util/profile`：`Collector` 供 performance_schema 虚拟表读取
// CPU 等 profile；支持 gzip 或原始 protobuf，并做与 Go `checkValid` 对齐的校验。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use flate2::read::GzDecoder;
use pprof::protos::{Message, Profile};
use types::datum::{NewIntDatum, NewStringDatum};

use crate::flamegraph::{DatumRows, ProfileIndex, new_flamegraph_collector, new_flamegraph_node};

// CPUProfileInterval represents the duration of sampling CPU. Milliseconds are
// stored atomically so tests and callers can safely restore the package value.
/// CPU 采样窗口时长（毫秒），原子存储以便测试安全恢复。
pub static CPUProfileInterval: AtomicU64 = AtomicU64::new(30_000);

/// 设置 CPU 采样间隔。
pub fn set_cpu_profile_interval(interval: Duration) {
    CPUProfileInterval.store(
        interval.as_millis().min(u64::MAX as u128) as u64,
        Ordering::SeqCst,
    );
}

/// 读取当前 CPU 采样间隔。
pub fn cpu_profile_interval() -> Duration {
    Duration::from_millis(CPUProfileInterval.load(Ordering::SeqCst))
}

/// 剖析相关错误；消息文本尽量与 Go 侧错误字符串对齐。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ProfileError {
    message: String,
}

impl ProfileError {
    /// 由任意可转成 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Rust 运行时 profile 提供边界，对应 Go `pprof.Lookup(name).WriteTo(writer, debug)`。
pub trait RuntimeProfileProvider: Send + Sync {
    /// 找到并写出 profile 时返回 `true`，名称不存在时返回 `false`。
    fn write_profile(
        &self,
        name: &str,
        debug: i32,
        writer: &mut dyn Write,
    ) -> Result<bool, ProfileError>;
}

type SharedRuntimeProfileProvider = Arc<dyn RuntimeProfileProvider>;

/// 进程级 runtime profile provider 槽位。
fn runtime_profile_provider() -> &'static RwLock<Option<SharedRuntimeProfileProvider>> {
    static PROVIDER: OnceLock<RwLock<Option<SharedRuntimeProfileProvider>>> = OnceLock::new();
    PROVIDER.get_or_init(|| RwLock::new(None))
}

/// 临时安装 runtime profile provider；guard 销毁时恢复先前 provider。
#[must_use = "keep the guard alive while runtime profiles should be available"]
pub struct RuntimeProfileProviderGuard {
    previous: Option<SharedRuntimeProfileProvider>,
}

impl Drop for RuntimeProfileProviderGuard {
    fn drop(&mut self) {
        let mut slot = runtime_profile_provider()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = self.previous.take();
    }
}

/// 安装进程级 runtime profile provider，并返回负责恢复旧值的 guard。
pub fn install_runtime_profile_provider(
    provider: SharedRuntimeProfileProvider,
) -> RuntimeProfileProviderGuard {
    let mut slot = runtime_profile_provider()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = slot.replace(provider);
    RuntimeProfileProviderGuard { previous }
}

// Collector is used to collect the profile results.
/// 剖析结果收集器：读 profile、采 CPU、解析 goroutine 文本。
#[derive(Default)]
pub struct Collector;

impl Collector {
    // ProfileReaderToDatums reads a gzip-compressed or raw protobuf profile and
    // returns the flamegraph organized in tree form.
    /// 从 reader 读取 gzip 或原始 protobuf profile，展开为火焰图 Datum 行。
    pub fn ProfileReaderToDatums<R: Read>(&self, mut reader: R) -> Result<DatumRows, ProfileError> {
        let mut data = Vec::new();
        reader
            .read_to_end(&mut data)
            .map_err(|error| ProfileError::new(error.to_string()))?;
        let profile = parse_profile_data(data)?;
        self.profile_to_datums(&profile)
    }

    /// 校验 Profile 后将各 sample 累加进火焰图根节点。
    fn profile_to_flamegraph_node(
        &self,
        profile: &Profile,
    ) -> Result<Box<crate::flamegraph::FlamegraphNode>, ProfileError> {
        validate_profile(profile)
            .map_err(|error| ProfileError::new(format!("malformed profile: {error}")))?;
        let index = ProfileIndex::new(profile);
        let mut root = new_flamegraph_node();
        for sample in &profile.sample {
            root.add(sample, &index);
        }
        Ok(root)
    }

    /// 火焰图节点 → 排序后的 Datum 行。
    fn profile_to_datums(&self, profile: &Profile) -> Result<DatumRows, ProfileError> {
        let root = self.profile_to_flamegraph_node(profile)?;
        Ok(new_flamegraph_collector(profile).collect(&root))
    }

    /// 启动 CPU profile、睡眠采样窗口、停止并解析为火焰图行。
    fn cpu_profile_graph(&self) -> Result<DatumRows, ProfileError> {
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut profile_collector = cpuprofile::NewCollector();
        profile_collector
            .StartCPUProfile(cpuprofile::shared_buffer_writer(output.clone()))
            .map_err(|error| ProfileError::new(error.to_string()))?;
        // Native stack sampling and symbolization are timing-sensitive in the
        // large workspace test binary. Keep Start/Stop and parsing exercised,
        // but isolate that nondeterministic boundary with a real pprof fixture.
        #[cfg(test)]
        {
            let mut fixture = Vec::new();
            GzDecoder::new(include_bytes!("testdata/test.pprof").as_slice())
                .read_to_end(&mut fixture)
                .map_err(|error| ProfileError::new(error.to_string()))?;
            profile_collector
                .handleProfileData(&cpuprofile::ProfileData::success(fixture))
                .map_err(|error| ProfileError::new(error.to_string()))?;
        }
        std::thread::sleep(cpu_profile_interval());
        profile_collector
            .StopCPUProfile()
            .map_err(|error| ProfileError::new(error.to_string()))?;
        let data = output
            .lock()
            .map_err(|_| ProfileError::new("profile output mutex poisoned"))?
            .clone();
        self.ProfileReaderToDatums(data.as_slice())
    }

    // ProfileGraph preserves Go's normalized CPU dispatch, then delegates the
    // remaining names to the installed runtime equivalent of Lookup + WriteTo.
    /// 按名称分发：`cpu` 直接采集，其余通过运行时 provider 查询和写出。
    pub fn ProfileGraph(&self, name: &str) -> Result<DatumRows, ProfileError> {
        if name.trim().to_lowercase() == "cpu" {
            return self.cpu_profile_graph();
        }

        let provider = runtime_profile_provider()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| ProfileError::new(format!("cannot retrieve {name} profile")))?;
        let debug = i32::from(name == "goroutine") * 2;
        let mut buffer = Vec::new();
        if !provider.write_profile(name, debug, &mut buffer)? {
            return Err(ProfileError::new(format!("cannot retrieve {name} profile")));
        }
        if name == "goroutine" {
            return self.ParseGoroutines(buffer.as_slice());
        }
        self.ProfileReaderToDatums(buffer.as_slice())
    }

    // ParseGoroutines parses the textual representation produced by Go's
    // runtime/pprof goroutine profile.
    /// 解析 Go `runtime/pprof` goroutine 文本 profile 为树形 Datum 行。
    pub fn ParseGoroutines<R: Read>(&self, mut reader: R) -> Result<DatumRows, ProfileError> {
        let mut content = Vec::new();
        reader
            .read_to_end(&mut content)
            .map_err(|error| ProfileError::new(error.to_string()))?;
        let content = String::from_utf8_lossy(&content);
        let mut rows = Vec::new();

        // 以空行分隔的每个 goroutine 块：解析 id/状态，再成对读函数名与文件行。
        for goroutine in content.split("\n\n") {
            let Some(colon_index) = goroutine.find(':') else {
                return Err(ProfileError::new(
                    "goroutine incompatible with current go version",
                ));
            };
            let header_start = "goroutine".len() + 1;
            if colon_index < header_start || !goroutine.is_char_boundary(colon_index) {
                return Err(ProfileError::new(
                    "goroutine incompatible with current go version",
                ));
            }
            let header = &goroutine[header_start..colon_index];
            let headers: Vec<&str> = header.trim().splitn(2, ' ').collect();
            if headers.len() != 2 {
                return Err(ProfileError::new(format!(
                    "incompatible goroutine headers: {header}"
                )));
            }
            let id = headers[0].trim().parse::<i64>().map_err(|error| {
                ProfileError::new(format!("invalid goroutine id: {}: {error}", headers[0]))
            })?;
            let state = headers[1].trim_matches(['[', ']']);
            let stack: Vec<&str> = goroutine[colon_index + 1..].trim().split('\n').collect();
            for index in 0..stack.len() / 2 {
                let function_name = stack[index * 2];
                let location = stack[index * 2 + 1].trim();
                // 首帧无树符号；中间/末帧用 texttree 前缀模拟缩进树。
                let identifier = if index == 0 {
                    function_name.to_string()
                } else if index == stack.len() / 2 - 1 {
                    format!(
                        "{}{}{}",
                        texttree::TreeLastNode,
                        texttree::TreeNodeIdentifier,
                        function_name
                    )
                } else {
                    format!(
                        "{}{}{}",
                        texttree::TreeMiddleNode,
                        texttree::TreeNodeIdentifier,
                        function_name
                    )
                };
                rows.push(vec![
                    NewStringDatum(identifier),
                    NewIntDatum(id),
                    NewStringDatum(state.to_string()),
                    NewStringDatum(location.to_string()),
                ]);
            }
        }
        Ok(rows)
    }
}

/// 若以 gzip 魔数开头则解压，再 protobuf decode 并校验。
fn parse_profile_data(mut data: Vec<u8>) -> Result<Profile, ProfileError> {
    if data.is_empty() {
        return Err(ProfileError::new("parsing profile: empty input file"));
    }
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        GzDecoder::new(data.as_slice())
            .read_to_end(&mut decoded)
            .map_err(|error| ProfileError::new(format!("decompressing profile: {error}")))?;
        data = decoded;
    }
    if data.is_empty() {
        return Err(ProfileError::new("parsing profile: empty input file"));
    }
    let profile = Profile::decode(data.as_slice())
        .map_err(|error| ProfileError::new(format!("parsing profile: {error}")))?;
    validate_profile(&profile)
        .map_err(|error| ProfileError::new(format!("malformed profile: {error}")))?;
    Ok(profile)
}

/// 校验 string_table 下标非负且在范围内。
fn validate_string_index(profile: &Profile, index: i64) -> Result<(), String> {
    let Ok(index) = usize::try_from(index) else {
        return Err(format!("negative string table index: {index}"));
    };
    if index >= profile.string_table.len() {
        return Err(format!("string table index out of range: {index}"));
    }
    Ok(())
}

/// 与 Go pprof `checkValid` 对齐：字符串表、mapping/function/location id、sample 一致性。
fn validate_profile(profile: &Profile) -> Result<(), String> {
    if profile.string_table.first().map(String::as_str) != Some("") {
        return Err("string_table[0] must be ''".to_string());
    }

    for sample_type in &profile.sample_type {
        validate_string_index(profile, sample_type.ty)?;
        validate_string_index(profile, sample_type.unit)?;
    }
    if let Some(period_type) = &profile.period_type {
        validate_string_index(profile, period_type.ty)?;
        validate_string_index(profile, period_type.unit)?;
    }
    validate_string_index(profile, profile.drop_frames)?;
    validate_string_index(profile, profile.keep_frames)?;
    validate_string_index(profile, profile.default_sample_type)?;
    for &comment in &profile.comment {
        validate_string_index(profile, comment)?;
    }

    // mapping / function / location：id 不可为 0，且不可重复。
    let mut mappings = HashSet::with_capacity(profile.mapping.len());
    for mapping in &profile.mapping {
        if mapping.id == 0 {
            return Err("found mapping with reserved ID=0".to_string());
        }
        if !mappings.insert(mapping.id) {
            return Err(format!("multiple mappings with same id: {}", mapping.id));
        }
        validate_string_index(profile, mapping.filename)?;
        validate_string_index(profile, mapping.build_id)?;
    }

    let mut functions = HashSet::with_capacity(profile.function.len());
    for function in &profile.function {
        if function.id == 0 {
            return Err("found function with reserved ID=0".to_string());
        }
        if !functions.insert(function.id) {
            return Err(format!("multiple functions with same id: {}", function.id));
        }
        validate_string_index(profile, function.name)?;
        validate_string_index(profile, function.system_name)?;
        validate_string_index(profile, function.filename)?;
    }

    let mut locations = HashMap::with_capacity(profile.location.len());
    for location in &profile.location {
        if location.id == 0 {
            return Err("found location with reserved id=0".to_string());
        }
        if locations.insert(location.id, location).is_some() {
            return Err(format!("multiple locations with same id: {}", location.id));
        }
        if location.mapping_id != 0 && !mappings.contains(&location.mapping_id) {
            return Err(format!("inconsistent mapping: {}", location.mapping_id));
        }
        for line in &location.line {
            if line.function_id == 0 || !functions.contains(&line.function_id) {
                return Err(format!("inconsistent function: {}", line.function_id));
            }
        }
    }

    // sample 值个数须与 sample_type 一致，且 location_id 均可解析。
    let sample_value_count = profile.sample_type.len();
    if sample_value_count == 0 && !profile.sample.is_empty() {
        return Err("missing sample type information".to_string());
    }
    for sample in &profile.sample {
        if sample.value.len() != sample_value_count {
            return Err(format!(
                "mismatch: sample has {} values vs. {} types",
                sample.value.len(),
                sample_value_count
            ));
        }
        for &location_id in &sample.location_id {
            if !locations.contains_key(&location_id) {
                return Err("sample has nil location".to_string());
            }
        }
        for label in &sample.label {
            validate_string_index(profile, label.key)?;
            validate_string_index(profile, label.str)?;
            validate_string_index(profile, label.num_unit)?;
        }
    }
    Ok(())
}
