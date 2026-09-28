// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Extract 任务 HTTP 适配：把 server Domain 的 ExtractHandle 接入 canonical handler。
//
// 对齐 Go `newExtractServeHandler`：Domain 存在时取得其 ExtractHandle，实际任务构建、
// failpoint、提交与 dump 流式响应均委托给 extractorhandler 实现。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_server_handler_extractorhandler::extractor::{
    ExtractError, ExtractReader, ExtractResult, ExtractRuntime, ExtractTask,
    HttpRequest as ExtractRequest, HttpResponseWriter, NewExtractTaskServeHandler, RequestContext,
    Timestamp,
};

use crate::http_status::{Request, Response, serve_error};
use crate::server::Domain;

#[derive(Clone)]
struct SharedExtractRuntime(Arc<dyn ExtractRuntime>);

impl ExtractRuntime for SharedExtractRuntime {
    fn now(&self) -> Timestamp {
        self.0.now()
    }

    fn parse_time(&self, value: &str) -> ExtractResult<Timestamp> {
        self.0.parse_time(value)
    }

    fn extract_task(&self, context: &RequestContext, task: ExtractTask) -> ExtractResult<String> {
        self.0.extract_task(context, task)
    }

    fn extract_task_directory(&self) -> String {
        self.0.extract_task_directory()
    }

    fn open_extract(
        &self,
        context: &RequestContext,
        path: &str,
    ) -> ExtractResult<Box<dyn ExtractReader>> {
        self.0.open_extract(context, path)
    }

    fn failpoint_enabled(&self, name: &str) -> bool {
        self.0.failpoint_enabled(name)
    }

    fn log_error(&self, message: &str, error: &ExtractError) {
        self.0.log_error(message, error);
    }

    fn log_warning(&self, message: &str, error: &ExtractError) {
        self.0.log_warning(message, error);
    }
}

struct ResponseWriter {
    response: Response,
}

impl ResponseWriter {
    fn new() -> Self {
        Self {
            response: Response {
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
            },
        }
    }
}

impl HttpResponseWriter for ResponseWriter {
    fn set_header(&mut self, name: &str, value: &str) {
        self.response.headers.insert(name.into(), value.into());
    }

    fn write_status(&mut self, status: u16) {
        self.response.status = status;
    }

    fn write(&mut self, data: &[u8]) -> ExtractResult<usize> {
        self.response.body.extend_from_slice(data);
        Ok(data.len())
    }

    fn write_error(&mut self, error: ExtractError) {
        self.response.status = 500;
        self.response.body = error.to_string().into_bytes();
    }
}

/// Server 层的 Extract HTTP 适配器。
#[derive(Clone)]
pub struct ExtractTaskServeHandler {
    runtime: Option<SharedExtractRuntime>,
}

impl ExtractTaskServeHandler {
    /// 对齐 Go：Domain 就绪时读取并持有其 ExtractHandle。
    pub fn new(domain: Option<Arc<dyn Domain>>) -> Self {
        Self {
            runtime: domain
                .and_then(|domain| domain.extract_runtime())
                .map(SharedExtractRuntime),
        }
    }

    /// 把 status server 请求交给 canonical extractor handler。
    pub fn handle(&self, request: &Request) -> Response {
        let Some(runtime) = self.runtime.clone() else {
            return serve_error(503, "domain is not initialized");
        };
        let handler = NewExtractTaskServeHandler(runtime);
        let extract_request = ExtractRequest {
            query: request.query.clone(),
            context: RequestContext::default(),
        };
        let mut writer = ResponseWriter::new();
        handler.ServeHTTP(&mut writer, &extract_request);
        writer.response
    }
}
