// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 优化器追踪（optimize trace）下载 HTTP 处理器。
//
// 根据路由文件名与内部地址构造 `DownloadFileRequest`；本地路径按 Go
// `filepath.Join` 语义清理，同时 URL path 保留原始路由名。

#![allow(dead_code, non_snake_case)]

use std::path::{Path, PathBuf};

/// 内部下载文件所需的请求参数集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFileRequest {
    /// 本地落盘/读取路径（已净化）。
    pub file_path: PathBuf,
    /// 路由给出的原始文件名。
    pub file_name: String,
    /// 内部服务地址。
    pub address: String,
    /// 状态服务端口。
    pub status_port: u16,
    /// HTTP URL 路径（可含未净化的路由名片段）。
    pub url_path: String,
    /// 下载时使用的文件名。
    pub downloaded_filename: String,
    /// 内部 HTTP scheme（如 https）。
    pub scheme: String,
}

/// OptimizeTrace 运行时依赖：路由名、目录、scheme、下载与错误写出。
pub trait OptimizeTraceRuntime {
    type Error;

    fn route_file_name(&self) -> String;
    fn optimizer_trace_directory(&self) -> PathBuf;
    fn internal_http_scheme(&self) -> String;
    fn download_file(&mut self, request: DownloadFileRequest) -> Result<(), Self::Error>;
    fn write_error(&mut self, error: Self::Error);
}

/// 持有内部地址与状态端口的 Optimize Trace handler。
pub struct OptimizeTraceHandler {
    pub address: String,
    pub status_port: u16,
}

/// 构造 `OptimizeTraceHandler`。
pub fn NewOptimizeTraceHandler(address: String, status_port: u16) -> OptimizeTraceHandler {
    OptimizeTraceHandler {
        address,
        status_port,
    }
}

impl OptimizeTraceHandler {
    /// 组装下载请求并调用 runtime；失败时写出错误。
    pub fn ServeHTTP<R: OptimizeTraceRuntime>(&self, runtime: &mut R) {
        let file_name = runtime.route_file_name();
        let request = DownloadFileRequest {
            file_path: join_clean(&runtime.optimizer_trace_directory(), &file_name),
            file_name: file_name.clone(),
            address: self.address.clone(),
            status_port: self.status_port,
            url_path: format!("optimize_trace/dump/{file_name}"),
            downloaded_filename: "optimize_trace".to_owned(),
            scheme: runtime.internal_http_scheme(),
        };
        if let Err(error) = runtime.download_file(request) {
            runtime.write_error(error);
        }
    }
}

/// 包级入口，转发到 handler 的 `ServeHTTP`。
pub fn serve_http<R: OptimizeTraceRuntime>(handler: &OptimizeTraceHandler, runtime: &mut R) {
    handler.ServeHTTP(runtime);
}

/// 按 Go `filepath.Join` 清理 `file_name` 后与目录拼接。
fn join_clean(directory: &Path, file_name: &str) -> PathBuf {
    let mut path = directory.to_path_buf();
    for component in Path::new(file_name).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::Normal(value) => path.push(value),
            // Go filepath.Join keeps the first path as the base when a later
            // element starts with a separator (for example, "/trace.zip").
            std::path::Component::RootDir => {}
            std::path::Component::Prefix(_) => path.push(component.as_os_str()),
        }
    }
    path
}
