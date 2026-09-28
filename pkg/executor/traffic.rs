// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Traffic（流量捕获/回放）执行器：经 TiProxy HTTP API 管理 capture/replay。
//
// TiProxy 是 SQL 流量代理；本模块向各 TiProxy 实例下发 capture（录制）、
// replay（回放）、cancel（取消）与 show（查询任务）请求，并处理共享存储路径分配。

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::time::Duration;

/// 上下文键：存放当前选中的 TiProxy 地址列表。
pub struct TiProxyAddrKey;
/// 上下文键：存放 Traffic 相关外部存储句柄。
pub struct TrafficStoreKey;

#[derive(Clone, Debug, Default, PartialEq)]
/// 单个 Traffic 任务的展示字段（对应 SHOW TRAFFIC 一行）。
pub struct TrafficJob {
    pub instance: String,
    pub job_type: String,
    pub status: String,
    pub start_time: String,
    pub end_time: String,
    pub progress: String,
    pub error: String,
    pub output: String,
    pub duration: String,
    pub compress: bool,
    pub encryption_method: String,
    pub input: String,
    pub username: String,
    pub speed: f64,
    pub read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 集群中的一个 TiProxy 节点（IP + status 端口）。
pub struct TiProxyNode {
    pub ip: String,
    pub status_port: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 内部 HTTP 请求方法。
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 内部 HTTP 响应：状态码与原始 body。
pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// 表单字段：任务开始时间（RFC3339）。
pub const startTimeKey: &str = "start-time";
/// 表单字段：capture 输出路径。
pub const outputKey: &str = "output";
/// 表单字段：replay 输入路径。
pub const inputKey: &str = "input";
/// TiProxy capture API 路径。
pub const capturePath: &str = "/api/traffic/capture";
/// TiProxy replay API 路径。
pub const replayPath: &str = "/api/traffic/replay";
/// TiProxy cancel API 路径。
pub const cancelPath: &str = "/api/traffic/cancel";
/// TiProxy show（列出任务）API 路径。
pub const showPath: &str = "/api/traffic/show";
/// 访问共享对象存储时的超时。
pub const sharedStorageTimeout: Duration = Duration::from_secs(10);
/// 共享存储下按实例划分目录的前缀。
pub const filePrefix: &str = "tiproxy-";

/// SHOW TRAFFIC 结果行写入接口。
pub trait TrafficChunk {
    type Time;

    fn grow_and_reset(&mut self, capacity: usize);
    fn append_time(&mut self, column: usize, time: &Self::Time);
    fn append_null(&mut self, column: usize);
    fn append_string(&mut self, column: usize, value: &str);
}

/// 外部存储句柄；`owned` 为 false 时执行器不得关闭（来自 Context 的 mock）。
pub struct TrafficStorageHandle<S> {
    pub storage: S,
    /// 来自 Context 的 mock 存储，执行器不得关闭。
    /// Mock storage comes from Context and must not be closed by the executor.
    pub owned: bool,
}

/// 批量请求失败：已收集的成功响应与首个错误。
pub struct TrafficRequestFailure<E> {
    pub responses: HashMap<String, String>,
    pub error: E,
}

/// Production boundary for InfoSync, internal HTTP, object storage, JSON,
/// privileges, statement warnings/errors, and time conversion.
/// 生产边界：InfoSync、内部 HTTP、对象存储、JSON、权限、告警与时间转换。
pub trait TrafficBackend {
    type Context;
    type Error: Display;
    type Time;
    type TimeoutContext;
    type Url;
    type StorageBackend;
    type Storage;

    fn error(&self, message: String) -> Self::Error;
    fn now_rfc3339(&self) -> String;
    fn max_chunk_size(&self) -> usize;
    fn open_base(&mut self, context: &Self::Context) -> Result<(), Self::Error>;
    fn tiproxy_nodes(&self, context: &Self::Context) -> Result<Vec<TiProxyNode>, Self::Error>;
    fn join_host_port(&self, host: &str, port: &str) -> String;

    fn internal_http_schema(&self) -> &str;
    fn http_request(
        &self,
        method: HttpMethod,
        url: &str,
        body: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<HttpResponse, Self::Error>;
    fn log_request_failure(
        &self,
        context: &Self::Context,
        path: &str,
        address: &str,
        response: &str,
        error: &Self::Error,
    );
    fn log_request_success(&self, context: &Self::Context, path: &str, addresses: &[String]);

    fn parse_url(&self, value: &str) -> Result<Self::Url, Self::Error>;
    fn url_is_local(&self, url: &Self::Url) -> bool;
    fn join_url_path(&self, url: &Self::Url, path: &str) -> String;
    fn parse_storage_backend(&self, value: &str) -> Result<Self::StorageBackend, Self::Error>;
    fn storage_backend_is_local(&self, backend: &Self::StorageBackend) -> bool;
    fn timeout_context(&self, context: &Self::Context, timeout: Duration) -> Self::TimeoutContext;
    fn finish_timeout_context(&self, context: Self::TimeoutContext);
    fn traffic_storage(
        &self,
        context: &Self::TimeoutContext,
        backend: Self::StorageBackend,
    ) -> Result<TrafficStorageHandle<Self::Storage>, Self::Error>;
    fn walk_storage(
        &self,
        context: &Self::TimeoutContext,
        storage: &mut Self::Storage,
        object_prefix: &str,
    ) -> Result<Vec<String>, Self::Error>;
    fn close_storage(&self, storage: Self::Storage);
    fn parse_raw_url(&self, value: &str) -> Result<Self::Url, Self::Error>;

    fn traffic_privileges(&self) -> (bool, bool);
    fn append_warning(&mut self, error: &Self::Error);
    fn log_replay_path_mismatch(
        &self,
        context: &Self::Context,
        too_many_paths: bool,
        proxies: usize,
        paths: usize,
    );
    fn decode_jobs(&self, response: &str) -> Result<Vec<TrafficJob>, Self::Error>;
    fn log_job_decode_error(
        &self,
        context: &Self::Context,
        address: &str,
        response: &str,
        error: &Self::Error,
    );
    fn parse_rfc3339(&self, value: &str) -> Result<Self::Time, Self::Error>;
    fn zero_time(&self) -> Self::Time;
    fn append_statement_error(&mut self, error: &Self::Error);
    fn log_time_parse_error(&self, context: &Self::Context, value: &str, error: &Self::Error);
}

/// TRAFFIC CAPTURE：向各 TiProxy 发起录制请求。
pub struct TrafficCaptureExec<B: TrafficBackend> {
    pub BaseExecutor: B,
    pub Args: HashMap<String, String>,
}

impl<B: TrafficBackend> TrafficCaptureExec<B> {
    /// 写入 start-time，为各实例构造表单并 POST capture。
    pub fn Next<Q>(&mut self, context: &B::Context, _request: &mut Q) -> Result<(), B::Error> {
        self.Args
            .insert(startTimeKey.to_owned(), self.BaseExecutor.now_rfc3339());
        let addresses = getTiProxyAddrs(&self.BaseExecutor, context)?;
        let readers = formReader4Capture(&self.BaseExecutor, &self.Args, addresses.len())?;
        request(
            &self.BaseExecutor,
            context,
            &addresses,
            Some(&readers),
            HttpMethod::Post,
            capturePath,
        )
        .map_err(|failure| failure.error)?;
        Ok(())
    }
}

/// TRAFFIC REPLAY：按输入路径向 TiProxy 发起回放。
pub struct TrafficReplayExec<B: TrafficBackend> {
    pub BaseExecutor: B,
    pub Args: HashMap<String, String>,
}

impl<B: TrafficBackend> TrafficReplayExec<B> {
    /// 扫描共享存储输入、对齐实例数与路径数后 POST replay。
    pub fn Next<Q>(&mut self, context: &B::Context, _request: &mut Q) -> Result<(), B::Error> {
        self.Args
            .insert(startTimeKey.to_owned(), self.BaseExecutor.now_rfc3339());
        let mut addresses = getTiProxyAddrs(&self.BaseExecutor, context)?;
        let form_context = self
            .BaseExecutor
            .timeout_context(context, sharedStorageTimeout);
        let readers_result = formReader4Replay(
            &self.BaseExecutor,
            &form_context,
            &self.Args,
            addresses.len(),
        );
        self.BaseExecutor.finish_timeout_context(form_context);
        let readers = readers_result?;
        let path_count = readers.len();
        let proxy_count = addresses.len();
        // 输入路径多于 TiProxy 实例：无法全覆盖，直接报错。
        if path_count > proxy_count {
            self.BaseExecutor
                .log_replay_path_mismatch(context, true, proxy_count, path_count);
            return Err(self.BaseExecutor.error(format!(
                "tiproxy instances number ({proxy_count}) is less than input paths number ({path_count})"
            )));
        }
        // 实例多于路径：截断地址列表并告警，部分实例不参与回放。
        if path_count < proxy_count {
            addresses.truncate(path_count);
            let warning = self.BaseExecutor.error(format!(
                "tiproxy instances number ({proxy_count}) is greater than input paths number ({path_count}), some instances won't replay"
            ));
            self.BaseExecutor.append_warning(&warning);
            self.BaseExecutor
                .log_replay_path_mismatch(context, false, proxy_count, path_count);
        }
        request(
            &self.BaseExecutor,
            context,
            &addresses,
            Some(&readers),
            HttpMethod::Post,
            replayPath,
        )
        .map_err(|failure| failure.error)?;
        Ok(())
    }
}

/// TRAFFIC CANCEL：按权限取消 capture 和/或 replay 任务。
pub struct TrafficCancelExec<B: TrafficBackend> {
    pub BaseExecutor: B,
}

impl<B: TrafficBackend> TrafficCancelExec<B> {
    /// 仅有一侧权限时在表单中限定 type，再 POST cancel。
    pub fn Next<Q>(&mut self, context: &B::Context, _request: &mut Q) -> Result<(), B::Error> {
        let addresses = getTiProxyAddrs(&self.BaseExecutor, context)?;
        let (capture, replay) = hasTrafficPriv(&self.BaseExecutor);
        let mut arguments = HashMap::new();
        // 仅 capture 权限：只取消录制类任务。
        if capture && !replay {
            arguments.insert("type".to_owned(), "capture".to_owned());
        } else if replay && !capture {
            arguments.insert("type".to_owned(), "replay".to_owned());
        }
        let form = getForm(&arguments);
        let readers = vec![form; addresses.len()];
        request(
            &self.BaseExecutor,
            context,
            &addresses,
            Some(&readers),
            HttpMethod::Post,
            cancelPath,
        )
        .map_err(|failure| failure.error)?;
        Ok(())
    }
}

/// SHOW TRAFFIC JOBS：汇总各 TiProxy 任务并分页输出。
pub struct TrafficShowExec<B: TrafficBackend> {
    pub BaseExecutor: B,
    pub jobs: Vec<TrafficJob>,
    pub cursor: usize,
}

impl<B: TrafficBackend> TrafficShowExec<B> {
    /// GET show、按权限过滤、按 start_time 降序与 instance 升序排序。
    pub fn Open(&mut self, context: &B::Context) -> Result<(), B::Error> {
        self.BaseExecutor.open_base(context)?;
        let addresses = getTiProxyAddrs(&self.BaseExecutor, context)?;
        let responses = request(
            &self.BaseExecutor,
            context,
            &addresses,
            None,
            HttpMethod::Get,
            showPath,
        )
        .map_err(|failure| failure.error)?;
        let (capture, replay) = hasTrafficPriv(&self.BaseExecutor);
        let mut all_jobs = Vec::with_capacity(responses.len());
        for (address, response) in responses {
            let jobs = match self.BaseExecutor.decode_jobs(&response) {
                Ok(jobs) => jobs,
                Err(error) => {
                    self.BaseExecutor
                        .log_job_decode_error(context, &address, &response, &error);
                    return Err(error);
                }
            };
            for mut job in jobs {
                if (job.job_type == "capture" && !capture) || (job.job_type == "replay" && !replay)
                {
                    continue;
                }
                job.instance.clone_from(&address);
                all_jobs.push(job);
            }
        }
        all_jobs.sort_by(|left, right| {
            right
                .start_time
                .cmp(&left.start_time)
                .then_with(|| left.instance.cmp(&right.instance))
        });
        self.jobs = all_jobs;
        Ok(())
    }

    /// 将缓存任务按 Chunk 批次写出（含参数串格式化）。
    pub fn Next<C: TrafficChunk<Time = B::Time>>(
        &mut self,
        context: &B::Context,
        request: &mut C,
    ) -> Result<(), B::Error> {
        let batch_size = self
            .BaseExecutor
            .max_chunk_size()
            .min(self.jobs.len() - self.cursor);
        request.grow_and_reset(batch_size);
        for _ in 0..batch_size {
            let job = self.jobs[self.cursor].clone();
            self.cursor += 1;
            let start_time = parseTime(&mut self.BaseExecutor, context, &job.start_time);
            request.append_time(0, &start_time);
            if job.end_time.is_empty() {
                request.append_null(1);
            } else {
                let end_time = parseTime(&mut self.BaseExecutor, context, &job.end_time);
                request.append_time(1, &end_time);
            }
            // capture / replay 参数串格式与 SQL 语法展示对齐。
            let parameters = if job.job_type == "capture" {
                format!(
                    "OUTPUT=\"{}\", DURATION=\"{}\", COMPRESS={}, ENCRYPTION_METHOD=\"{}\"",
                    job.output, job.duration, job.compress, job.encryption_method
                )
            } else {
                format!(
                    "INPUT=\"{}\", USER=\"{}\", SPEED={:.6}, READ_ONLY={}",
                    job.input, job.username, job.speed, job.read_only
                )
            };
            request.append_string(2, &job.instance);
            request.append_string(3, &job.job_type);
            request.append_string(4, &job.progress);
            request.append_string(5, &job.status);
            request.append_string(6, &job.error);
            request.append_string(7, &parameters);
        }
        Ok(())
    }
}

/// 向地址列表逐个发 HTTP；任一失败则返回已成功响应与错误。
pub fn request<B: TrafficBackend>(
    backend: &B,
    context: &B::Context,
    addresses: &[String],
    readers: Option<&[String]>,
    method: HttpMethod,
    path: &str,
) -> Result<HashMap<String, String>, TrafficRequestFailure<B::Error>> {
    let mut responses = HashMap::with_capacity(addresses.len());
    for (index, address) in addresses.iter().enumerate() {
        let reader = readers.and_then(|readers| readers.get(index).map(String::as_str));
        match requestOne(backend, method, address, path, reader) {
            Ok(response) => {
                responses.insert(address.clone(), response);
            }
            Err((response, error)) => {
                backend.log_request_failure(context, path, address, &response, &error);
                return Err(TrafficRequestFailure {
                    responses,
                    error: backend.error(format!("request to tiproxy '{address}' failed: {error}")),
                });
            }
        }
    }
    backend.log_request_success(context, path, addresses);
    Ok(responses)
}

/// 从 InfoSync 取 TiProxy 节点并拼成 host:port；无节点则报错。
pub fn getTiProxyAddrs<B: TrafficBackend>(
    backend: &B,
    context: &B::Context,
) -> Result<Vec<String>, B::Error> {
    let nodes = backend.tiproxy_nodes(context)?;
    if nodes.is_empty() {
        return Err(backend.error("no tiproxy server found".to_owned()));
    }
    Ok(nodes
        .into_iter()
        .map(|node| backend.join_host_port(&node.ip, &node.status_port))
        .collect())
}

/// 对单个地址发一次请求；非 200 将 body 作为错误信息。
pub fn requestOne<B: TrafficBackend>(
    backend: &B,
    method: HttpMethod,
    address: &str,
    path: &str,
    reader: Option<&str>,
) -> Result<String, (String, B::Error)> {
    let url = format!("{}://{address}{path}", backend.internal_http_schema());
    let content_type = (method == HttpMethod::Post).then_some("application/x-www-form-urlencoded");
    let response = backend
        .http_request(method, &url, reader, content_type)
        .map_err(|error| (String::new(), error))?;
    let body = String::from_utf8_lossy(&response.body).into_owned();
    if response.status_code == 200 {
        Ok(body)
    } else {
        Err((body.clone(), backend.error(body)))
    }
}

/// 将参数 map 按 key 排序后编码为 `application/x-www-form-urlencoded`。
pub fn getForm(arguments: &HashMap<String, String>) -> String {
    let mut entries: Vec<(&String, &String)> = arguments.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
        .into_iter()
        .map(|(key, value)| format!("{}={}", queryEscape(key), queryEscape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// 解析 RFC3339 时间；失败则记 statement error 并返回零时间。
pub fn parseTime<B: TrafficBackend>(backend: &mut B, context: &B::Context, value: &str) -> B::Time {
    match backend.parse_rfc3339(value) {
        Ok(time) => time,
        Err(error) => {
            backend.append_statement_error(&error);
            backend.log_time_parse_error(context, value, &error);
            backend.zero_time()
        }
    }
}

/// 为 capture 构造每实例一份表单：本地路径复用，远程则追加 tiproxy-N 子路径。
pub fn formReader4Capture<B: TrafficBackend>(
    backend: &B,
    arguments: &HashMap<String, String>,
    tiproxy_count: usize,
) -> Result<Vec<String>, B::Error> {
    let output = arguments
        .get(outputKey)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| backend.error("the output path for capture must be specified".to_owned()))?;
    let url = backend
        .parse_url(output)
        .map_err(|error| backend.error(format!("parse output path failed: {error}")))?;
    // 本地输出：所有实例共用同一表单。
    if backend.url_is_local(&url) {
        return Ok(vec![getForm(arguments); tiproxy_count]);
    }
    let mut readers = Vec::with_capacity(tiproxy_count);
    for index in 0..tiproxy_count {
        let mut arguments = arguments.clone();
        arguments.insert(
            outputKey.to_owned(),
            backend.join_url_path(&url, &format!("{filePrefix}{index}")),
        );
        readers.push(getForm(&arguments));
    }
    Ok(readers)
}

/// 为 replay 构造表单：本地路径复用；远程则遍历 tiproxy-* 目录。
pub fn formReader4Replay<B: TrafficBackend>(
    backend: &B,
    context: &B::TimeoutContext,
    arguments: &HashMap<String, String>,
    tiproxy_count: usize,
) -> Result<Vec<String>, B::Error> {
    let input = arguments
        .get(inputKey)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| backend.error("the input path for replay must be specified".to_owned()))?;
    let storage_backend = backend
        .parse_storage_backend(input)
        .map_err(|error| backend.error(format!("parse input path failed: {error}")))?;
    // 本地输入：所有实例共用同一表单。
    if backend.storage_backend_is_local(&storage_backend) {
        return Ok(vec![getForm(arguments); tiproxy_count]);
    }

    let mut handle = backend
        .traffic_storage(context, storage_backend)
        .map_err(|error| backend.error(format!("create storage for input failed: {error}")))?;
    let readers_result = (|| {
        let object_names = backend
            .walk_storage(context, &mut handle.storage, filePrefix)
            .map_err(|error| backend.error(format!("walk input path failed: {error}")))?;
        // 从对象名中提取 tiproxy-N/ 一级目录，每个目录对应一份 replay 输入。
        let mut directories = HashSet::with_capacity(tiproxy_count);
        for name in object_names {
            if let Some(index) = name.find('/') {
                directories.insert(name[..index].to_owned());
            }
        }
        if directories.is_empty() {
            return Err(backend.error("no replay files found in the input path".to_owned()));
        }
        let url = backend
            .parse_raw_url(input)
            .map_err(|error| backend.error(format!("parse input path failed: {error}")))?;
        let mut readers = Vec::with_capacity(directories.len());
        for directory in directories {
            let mut arguments = arguments.clone();
            arguments.insert(inputKey.to_owned(), backend.join_url_path(&url, &directory));
            readers.push(getForm(&arguments));
        }
        Ok(readers)
    })();
    // 仅关闭执行器自己创建的存储，Context 注入的 mock 保持打开。
    if handle.owned {
        backend.close_storage(handle.storage);
    }
    readers_result
}

/// 返回 (capture 权限, replay 权限)。
pub fn hasTrafficPriv<B: TrafficBackend>(backend: &B) -> (bool, bool) {
    backend.traffic_privileges()
}

/// 等价 Go `url.QueryEscape`：空格为 `+`，其余非 unreserved 为 `%XX`。
fn queryEscape(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}
