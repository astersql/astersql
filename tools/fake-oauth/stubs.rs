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
//! Local `net/http` stubs for fake-oauth (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Mirrors Go `http.HandleFunc` / `http.ListenAndServe` / `ResponseWriter.Write`
//! against an in-process default ServeMux. Network bind is simulated so unit
//! tests never open ports; handler registration and dispatch keep Go order.
//! 这里实现的是最小可用的 `net/http` 替身，只覆盖 fake-oauth
//! 主程序真正依赖的注册路由、写响应体和记录监听调用三件事。
//! 它故意不模拟完整 HTTP 协议栈，也不会真的绑定端口，因此测试能够在
//! 任意机器上稳定运行，同时保持与 Go `main.go` 的外部可观察语义一致。
//! 对 fake-oauth 来说，桩的边界比“功能更多”更重要：未知路径直接 miss，
//! 自定义 handler 只保留 `nil` 与默认 mux 的区分，不把未使用能力伪装成已支持。

use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, OnceLock};

/// Go `*http.Request` — fake-oauth does not read fields; kept for signature parity.
/// 当前调用方只依赖 `method` 与 `path` 两个最小字段，
/// 结构体存在的主要价值是让 handler 签名与 Go 版本保持一致，
/// 而不是复刻 `net/http.Request` 的全部状态机和派生字段。
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
}

/// Go `http.ResponseWriter` — body buffer records `Write` side effects.
/// 这里把响应写入简化成“状态码 + 内存缓冲区”，
/// 因为 fake-oauth 只写固定 JSON，不涉及 header、flush、chunked 编码等能力。
/// 测试通过检查 `body` 与默认 `status`，即可验证与 Go `ResponseWriter`
/// 在本工具场景下的关键副作用是否一致。
#[derive(Clone, Debug)]
pub struct ResponseWriter {
    pub status: u16,
    pub body: Vec<u8>,
}

impl ResponseWriter {
    pub fn new() -> Self {
        Self {
            // Go 中未显式调用 `WriteHeader` 时，默认状态就是 200。
            status: 200,
            body: Vec::new(),
        }
    }

    /// Go `w.Write(data)` — appends bytes, returns written count.
    /// 只追加字节并返回写入长度，不引入部分写入、短写重试
    /// 或 header 提交等更复杂语义，避免超出 fake-oauth 原始依赖面。
    #[allow(non_snake_case)]
    pub fn Write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.body.extend_from_slice(data);
        Ok(data.len())
    }
}

impl Default for ResponseWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// handler 保持与 Go `func(ResponseWriter, *Request)` 等价的调用形状，
/// 再补上 `Send + Sync`，以便安全地存入全局 mux 的互斥保护状态中。
type HandlerFn = Box<dyn Fn(&mut ResponseWriter, &Request) + Send + Sync>;

/// 进程内默认路由表，只支持按路径精确匹配。
/// fake-oauth 只有一个固定端点，因此这里不实现 Go `ServeMux`
/// 的清理规则、最长前缀匹配或主机名相关行为，避免测试误以为这些能力可用。
struct ServeMux {
    handlers: Mutex<HashMap<String, HandlerFn>>,
}

impl ServeMux {
    fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    #[allow(non_snake_case)]
    fn HandleFunc<F>(&self, pattern: &str, handler: F)
    where
        F: Fn(&mut ResponseWriter, &Request) + Send + Sync + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        if handlers.contains_key(pattern) {
            drop(handlers);
            panic!("multiple registrations for {pattern}");
        }
        handlers.insert(pattern.to_string(), Box::new(handler));
    }

    fn serve(&self, w: &mut ResponseWriter, r: &Request) -> bool {
        let handlers = self.handlers.lock().unwrap();
        // 只按 `Request.path` 做精确命中；未命中时返回 false，让测试显式断言 miss。
        if let Some(h) = handlers.get(&r.path) {
            h(w, r);
            true
        } else {
            false
        }
    }

    fn clear(&self) {
        self.handlers.lock().unwrap().clear();
    }
}

/// 默认 mux 以 `OnceLock` 延迟初始化，
/// 既模拟 Go 包级单例的生命周期，又避免在测试未触发前构造全局状态。
fn default_mux() -> &'static ServeMux {
    static MUX: OnceLock<ServeMux> = OnceLock::new();
    MUX.get_or_init(ServeMux::new)
}

/// Recorded `ListenAndServe` call (addr + whether default mux was used).
/// fake-oauth 并不真正监听端口，所以这里把“监听结果”降级为
/// 最近一次调用记录，供 parity 测试验证地址和 `nil` handler 语义是否保持一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenCall {
    pub addr: String,
    pub used_default_mux: bool,
}

/// 监听相关的全局状态被单独拆出，
/// 这样可以在不触碰路由表的前提下，独立模拟监听失败与读取最近一次调用。
struct ListenState {
    last: Option<ListenCall>,
    force_err: bool,
}

/// 与默认 mux 一样，监听状态也采用惰性初始化的全局单例，
/// 方便多轮测试在统一入口下重置，而不用暴露内部状态结构。
fn listen_state() -> &'static Mutex<ListenState> {
    static STATE: OnceLock<Mutex<ListenState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(ListenState {
            last: None,
            force_err: false,
        })
    })
}

/// Go `http.HandleFunc(pattern, handler)` on the default ServeMux.
/// 公开函数只是把注册动作转发到默认 mux，
/// 让 `main.rs` 可以像 Go 包级函数一样直接调用，而不需要显式持有路由表对象。
#[allow(non_snake_case)]
pub fn HandleFunc<F>(pattern: &str, handler: F)
where
    F: Fn(&mut ResponseWriter, &Request) + Send + Sync + 'static,
{
    default_mux().HandleFunc(pattern, handler);
}

/// Go `http.ListenAndServe(addr, handler)`.
///
/// `handler == None` means the default ServeMux (Go `nil`). Does not bind a
/// real socket; records the call for tests. When `set_listen_error(true)`,
/// returns an error (Go still discards it with `_ =`).
///
/// Custom handlers are unused by fake-oauth; `Option<()>` stands in for Go's
/// `nil` Handler without exposing the private ServeMux type.
/// 这里最关键的约束是“记录而不监听”。
/// fake-oauth 只需要证明自己会以正确地址尝试启动默认 mux，
/// 不需要真实网络资源，因此用 `Option<()>` 区分 `nil` 与“非 nil”即可。
/// 即使开启强制错误，调用也只返回 `io::Error`，由上层决定是否像 Go 一样忽略。
#[allow(non_snake_case)]
pub fn ListenAndServe(addr: &str, handler: Option<()>) -> io::Result<()> {
    let used_default_mux = handler.is_none();
    let mut st = listen_state().lock().unwrap();
    st.last = Some(ListenCall {
        addr: addr.to_string(),
        used_default_mux,
    });
    if st.force_err {
        return Err(io::Error::other("forced listen error"));
    }
    Ok(())
}

/// Dispatch `r` against the default ServeMux (test harness; not in Go).
/// 这是专门给 Rust 测试夹具使用的辅助入口，
/// 便于在不启动真实服务器的情况下直接触发 handler 并观察写入结果。
pub fn serve_default(w: &mut ResponseWriter, r: &Request) -> bool {
    default_mux().serve(w, r)
}

/// Clear default mux handlers and last listen record.
/// 每轮测试前调用它，确保先前注册的 handler、监听记录和错误注入
/// 都不会泄漏到下一轮断言，模拟 Go 新进程启动时的干净初始状态。
pub fn reset_for_test() {
    default_mux().clear();
    let mut st = listen_state().lock().unwrap();
    st.last = None;
    st.force_err = false;
}

/// Force the next `ListenAndServe` to fail (simulates bind error).
/// 错误注入只影响监听路径，不碰路由分发表，
/// 这样测试可以单独验证“监听失败被忽略”的 Go 语义，而不混入其他副作用。
pub fn set_listen_error(force: bool) {
    listen_state().lock().unwrap().force_err = force;
}

/// Take the last recorded listen call, if any.
/// 读取后即清空，避免测试重复消费同一条记录导致误判，
/// 也更贴近“只关心最近一次启动尝试”的断言模型。
pub fn take_last_listen() -> Option<ListenCall> {
    listen_state().lock().unwrap().last.take()
}
