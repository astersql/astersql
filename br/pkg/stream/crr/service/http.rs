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

//! CRR 服务 HTTP 探测与状态导出，对齐 Go `http.go`。
//! 注册 `/livez`、`/readyz`、`/status`；就绪/存活读 StatusSnapshot，不触发计算。
//! `RegisterOrPanic` 对应 Go 对 nil mux 的 panic，便于启动期快速失败。
//! 处理器不读请求体，保持与 Go net/http 探针同样的无状态语义。

use std::collections::HashMap;
use std::sync::Arc;

use crate::Service;
use crate::status::encode_status_snapshot;

/// HTTP 200，对齐 net/http StatusOK。
pub const STATUS_OK: u16 = 200;
/// 503：未存活或未就绪时返回，供探针重试。
pub const STATUS_SERVICE_UNAVAILABLE: u16 = 503;
/// 500：状态 JSON 编码失败。
pub const STATUS_INTERNAL_SERVER_ERROR: u16 = 500;

/// 最小请求视图；当前处理器忽略 Method，仅路由 Path。
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    pub Method: String,
    pub Path: String,
}

/// 响应写入抽象，便于测试用内存 Writer，也适配真实 HTTP 框架。
pub trait HttpResponseWriter {
    fn Header(&mut self) -> &mut HashMap<String, String>;
    fn WriteHeader(&mut self, status: u16);
    fn WriteBody(&mut self, body: &[u8]);
}

/// 与 Go http.HandlerFunc 对应的闭包类型；需 Send+Sync 以便多线程服务。
pub type HttpHandler = Box<dyn Fn(&HttpRequest, &mut dyn HttpResponseWriter) + Send + Sync>;

/// 路由注册面；生产可用标准库/框架 mux，测试用 TestMux。
pub trait HttpMux {
    fn HandleFunc(&mut self, path: &str, handler: HttpHandler);
}

impl Service {
    /// 注册三条探针；各自 clone Arc<Service>，避免 handler 生命周期绑定临时借用。
    pub fn Register<M: HttpMux>(self: &Arc<Self>, mux: &mut M) {
        let live = Arc::clone(self);
        mux.HandleFunc(
            "/livez",
            Box::new(move |_req, writer| live.handle_liveness(writer)),
        );

        let ready = Arc::clone(self);
        mux.HandleFunc(
            "/readyz",
            Box::new(move |_req, writer| ready.handle_readiness(writer)),
        );

        let status = Arc::clone(self);
        mux.HandleFunc(
            "/status",
            Box::new(move |_req, writer| status.handle_status(writer)),
        );
    }

    /// 进程存活：Stopped 等非 Live 状态返回 503，无响应体。
    fn handle_liveness(&self, writer: &mut dyn HttpResponseWriter) {
        let snapshot = self.Status();
        if !snapshot.Live {
            writer.WriteHeader(STATUS_SERVICE_UNAVAILABLE);
            return;
        }
        writer.WriteHeader(STATUS_OK);
    }

    /// 业务就绪：Degraded/未完成首轮等 Ready=false 时 503。
    fn handle_readiness(&self, writer: &mut dyn HttpResponseWriter) {
        let snapshot = self.Status();
        if !snapshot.Ready {
            writer.WriteHeader(STATUS_SERVICE_UNAVAILABLE);
            return;
        }
        writer.WriteHeader(STATUS_OK);
    }

    /// 导出 JSON 状态快照；编码失败写 500 与错误文本，成功固定 Content-Type。
    fn handle_status(&self, writer: &mut dyn HttpResponseWriter) {
        writer
            .Header()
            .insert("Content-Type".to_string(), "application/json".to_string());
        match encode_status_snapshot(&self.Status()) {
            Ok(mut body) => {
                // Go json.Encoder.Encode always terminates one encoded value with '\n'.
                body.push('\n');
                writer.WriteHeader(STATUS_OK);
                writer.WriteBody(body.as_bytes());
            }
            Err(err) => {
                writer.WriteHeader(STATUS_INTERNAL_SERVER_ERROR);
                writer.WriteBody(err.to_string().as_bytes());
            }
        }
    }
}

/// 启动辅助：mux 为 None 时 panic，消息与 Go `service: nil mux` 一致。
pub fn RegisterOrPanic<M: HttpMux>(service: &Arc<Service>, mux: Option<&mut M>) {
    match mux {
        Some(mux) => service.Register(mux),
        None => panic!("service: nil mux"),
    }
}
