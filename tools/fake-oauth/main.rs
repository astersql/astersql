// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Fake OAuth token endpoint (Go package `main` / `tools/fake-oauth`).
//!
//! Registers `GET|POST /oauth/token` on the default ServeMux and listens on
//! `:5000`. Network bind is stubbed locally; handler body matches Go exactly.
//! 这个模块对应 Go 版的独立命令行入口，职责只有注册一个固定令牌接口并启动监听。
//! 之所以拆成 `register_routes` 和 `run_main`，是为了在测试或对照迁移时分别观察路由注册与启动顺序。
//! 返回体、路径和监听地址都保持字面量一致，避免与上游工具链或依赖该桩行为的脚本出现协议偏差。

pub mod stubs;

/// Go literal path registered with `http.HandleFunc`.
/// 保留独立常量，方便在 Rust/Go 对照测试里直接验证注册路径没有被意外改写。
pub const TOKEN_PATH: &str = "/oauth/token";

/// Go `http.ListenAndServe` address.
/// 继续使用与 Go 相同的 `:5000`，让外部脚本按既有默认端口访问这个假 OAuth 服务。
pub const LISTEN_ADDR: &str = ":5000";

/// Exact JSON body written by the Go handler.
/// 这里故意返回固定成功令牌，不解析请求参数，语义上就是为集成流程提供最小可用的认证桩。
pub const TOKEN_JSON: &[u8] =
    br#"{"access_token": "ok", "token_type":"service_account", "expires_in":3600}"#;

/// Body bytes returned by the `/oauth/token` handler (same as Go).
pub fn token_response_body() -> &'static [u8] {
    TOKEN_JSON
}

/// Register the Go `/oauth/token` handler on the default ServeMux.
/// 处理器忽略请求内容，只把预设 JSON 写回去，和 Go 版本的“只写响应、不做校验”保持一致。
pub fn register_routes() {
    stubs::HandleFunc(
        TOKEN_PATH,
        |w: &mut stubs::ResponseWriter, _r: &stubs::Request| {
            let _ = w.Write(TOKEN_JSON);
        },
    );
}

/// Core of Go `main` without process exit (listen error discarded like `_ =`).
/// 先注册路由再监听，顺序不能调换；监听错误也按 Go 原实现一样被显式丢弃。
pub fn run_main() {
    register_routes();
    let _ = stubs::ListenAndServe(LISTEN_ADDR, None);
}

/// Process entry matching Go `main`.
pub fn main() {
    run_main();
}
