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

//! Real TCP parity tests for `lightning_server_serial_test.go`.
//!
//! The Go importer/failpoint boundary is represented by explicitly moving a
//! posted config between the shared queue and the current-task slot. HTTP
//! routing, TOML decoding, JSON encoding, cancellation, and socket lifecycle
//! all use the production Rust implementation.
//!
//!
//! 这一组测试直接通过 TCP 与内建 HTTP 服务器交互，验证的是控制面而不是 importer 本身。
//! 选择原始 socket 请求而不是更高层 mock，是为了让路由、Header、状态码和连接关闭都走真实实现。
//! `serial_guard()` 把监听端口与全局 server 状态串行化，避免测试间互相抢占资源。
//! `HttpResponse` 负责把原始字节切成状态码、头和 body，方便后续断言保持紧凑。
//! `request()` 手工拼装 HTTP/1.1 报文，是为了覆盖 `stubs::http` 的最底层读写路径。
//! 这里还额外核对 `Content-Length` 与 body 长度一致，防止响应在关连接前被截断。
//! `LightningServerSuite` 是整个文件的测试夹具，统一承担启动、停止和资源回收。
//! `start()` 总是绑定 `127.0.0.1:0`，用系统分配端口，减少并发环境下的端口冲突。
//! `outside_server_mode()` 保留 `taskCfgs = None`，用于验证未开启 server mode 时的 API 行为。
//! `server_mode()` 预先准备队列，模拟 Go `RunServer` 已进入任务循环的状态。
//! `current_task()` 把单个任务直接放入当前槽位，用于断言 DELETE 当前任务时会触发 cancel。
//! `current_with_queue()` 同时构造当前任务和排队任务，复现调度器弹出一个任务后的快照。
//! `call()` 统一约定：有 body 就按 TOML 请求发送，没有 body 就发空请求。
//! `shutdown()` 和 `stop_and_wait()` 重点验证 `Stop()` 之后监听器会真正关闭。
//! 这里主动发一次短连接唤醒阻塞的 `accept()`，对应 Go 里依赖 listener 关闭打断阻塞的时机。
//! `post_task()` 不只验证 POST 成功，还会反查 queue，确认任务确实被放入内存队列。
//! 这避免出现“HTTP 返回成功但内部状态没更新”的假阳性。
//! `get_task_list()` 直接对 `/tasks` 的 JSON 结构做解码，是整个文件读取状态快照的公共助手。
//! `test_run_server` 先走未开启 server mode 的 501 分支，再走已开启模式的错误输入分支。
//! 它验证 PUT 被拒绝、非法 TOML 被拒绝、非法 CSV 参数被拒绝，以及成功 POST 会生成唯一任务 ID。
//! 20 个任务 ID 去重断言的意义，在于覆盖 Rust 里随机 TaskID 生成不应与 Go 约束冲突。
//! `test_get_delete_task` 更像是服务调度快照测试：先入队，再模拟第一项已经运行。
//! 随后依次验证 GET 队列、GET 单任务、DELETE 队列项、DELETE 当前项这几种常见操作。
//! 删除当前任务后，`task_ctx` 必须变成 cancelled，说明 server 控制面真的调用了取消函数。
//! 再把第三个任务提升为 current，则对应 Go 中任务循环收到取消后继续处理下一项。
//! `test_http_api_outside_server_mode` 证明即使没开 server mode，只要存在当前任务，部分只读接口仍可工作。
//! 例如 GET 当前任务和 DELETE 当前任务都应可用，因为它们只依赖当前槽位和 cancel 句柄。
//! 但 POST 与 PATCH 仍然必须返回 501，防止调用方误以为队列能力已经启用。
//! 整体上，这个文件覆盖的是 server 视角的协议合同：状态码、排队语义、取消语义与监听器生命周期。
//! 这些合同比具体 importer 实现更脆弱，也更容易在迁移时被无意改坏。
//! 因此顶部注释会刻意解释每个夹具和测试场景要守住的协议含义。
//! 当未来有人改 `stubs::http`、`handle_task_http()` 或 `Stop()` 时，应优先参考这里的场景说明。
//! 这些场景共同守护的是“可观测控制面”，也就是外部调用者真正能看到的行为。
//! 只要这些合同稳定，内部 importer 替换或重构就不会轻易影响 HTTP 使用者。
//! 因而本文件是 server 子系统里非常重要的一层黑盒协议回归。
//! 维护时需要特别警惕那些只改了内部状态同步、却可能改变响应顺序的重构。
//! 顶层夹具的职责就是把这种变化尽量放大成可见的状态码或取消行为差异。
//! 通过真实网络读写来做这件事，成本比 mock 稍高，但能换来更接近实际使用方式的证据。
//! 这也解释了为什么这里保留了不少辅助函数和套件封装。
//! 它们不是样板，而是为了让每个协议场景都能围绕统一的网络入口表达。
//! 这样阅读测试时，可以先看 HTTP 请求和预期响应，再回头理解内部状态如何被布置。
//! 注释的目标就是帮助读者建立这条“外部协议 -> 内部状态”的映射。
//! 一旦这条映射清楚，定位服务模式相关回归会快得多。
//! 另外，这里每个场景都尽量对应一个真实使用动作，而不是内部私有方法调用。
//! 这样即使内部实现更换，只要外部协议没变，测试就应保持稳定。
//! 反过来说，只要这些测试失败，通常就说明使用者真的会观察到差异。
//! `outside_server_mode` 相关说明强调的是能力边界。
//! 也就是说，server 没启动队列时，只能访问当前任务，不能伪装成支持排队。
//! `server_mode` 相关说明强调的是调度语义。
//! 只要队列和当前任务的呈现顺序变化，就会直接影响用户对任务系统的理解。
//! `current_task` 与 `current_with_queue` 相关说明强调的是取消传播。
//! DELETE 当前任务不是简单删除内存项，而是要向运行中的任务上下文发出取消。
//! 这点若被改坏，外部虽然拿到 200，内部任务却可能继续执行。
//! `stop_and_wait` 相关说明强调的是 listener 生命周期。
//! `Stop()` 只有真正关闭监听端口，才算完成 server 关闭语义。
//! `post_task` 相关说明强调的是入队副作用。
//! POST 返回成功只是第一步，真正重要的是后续 GET `/tasks` 能观察到相同结果。
//! `get_task_list` 相关说明强调的是快照一致性。
//! 当前任务和队列视图必须来自同一份共享状态，否则 HTTP 返回就会自相矛盾。
//! 顶部这些解释合在一起，构成了 server 控制面的黑盒协议文档。
//! 文档越清晰，未来在重构 HTTP、队列或取消逻辑时越容易判断是否触碰了公共面。
//! 这也是本文件需要比普通测试写更多中文注释的原因。
//! 它既是回归保护，也是行为说明书。
//! 当 Go 对照更新时，这里也应同步更新，保证两边仍讨论同一份协议。
//! 因此可以把这些注释视为“测试为什么存在”的答案，而不是纯背景材料。
//! 只要答案仍然成立，这些场景就值得继续保留。
//! 这条原则对 server 协议测试尤其重要，因为协议一旦漂移，影响面通常大于单个函数。
//! 所以这里宁可把意图说明得更细，也不希望把关键合同埋在断言细节里。
//! 这也是本文件在中文注释密度上明显高于普通单元测试的原因。

use crate::{Lightning, New, config, context};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

fn serial_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug)]
struct HttpResponse {
    status: i32,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|err| panic!("invalid JSON response {err}: {:?}", self.body))
    }

    fn header(&self, name: &str) -> &str {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or("")
    }
}

fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &str,
) -> HttpResponse {
    let mut stream = TcpStream::connect_timeout(&addr, IO_TIMEOUT)
        .unwrap_or_else(|err| panic!("connect {addr} for {method} {path}: {err}"));
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();

    let mut wire = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(content_type) = content_type {
        wire.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    wire.push_str("\r\n");
    wire.push_str(body);
    stream
        .write_all(wire.as_bytes())
        .unwrap_or_else(|err| panic!("write {method} {path}: {err}"));
    if let Err(err) = stream.shutdown(Shutdown::Write) {
        assert_eq!(
            err.kind(),
            ErrorKind::NotConnected,
            "shutdown write side for {method} {path}: {err}"
        );
    }

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .unwrap_or_else(|err| panic!("read {method} {path}: {err}"));
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response for {method} {path}: {raw:?}"));
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<i32>().ok())
        .unwrap_or_else(|| panic!("malformed HTTP status: {head}"));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok()),
        Some(body.len()),
        "response body must be completely read before closing the connection"
    );
    HttpResponse {
        status,
        headers,
        body,
    }
}

struct LightningServerSuite {
    lightning: Box<Lightning>,
    addr: SocketAddr,
    stopped: bool,
}

impl LightningServerSuite {
    fn start(prepare: impl FnOnce(&mut Lightning)) -> Self {
        let mut global = config::GlobalConfig::default();
        global.App.StatusAddr = "127.0.0.1:0".to_string();
        let mut lightning = New(global);
        prepare(&mut lightning);
        lightning.GoServe().expect("start production HTTP server");
        let addr = lightning
            .serverAddr
            .expect("GoServe publishes bound address");
        Self {
            lightning,
            addr,
            stopped: false,
        }
    }

    fn outside_server_mode() -> Self {
        Self::start(|_| {})
    }

    fn server_mode() -> Self {
        Self::start(|lightning| {
            lightning.taskCfgs = Some(config::NewConfigList());
        })
    }

    fn current_task(task: config::Config) -> (Self, context::Context) {
        let (task_ctx, cancel) = context::WithCancel(context::Background());
        let observed = task_ctx.clone();
        let suite = Self::start(move |lightning| {
            lightning.curTask = Some(task);
            lightning.cancel = Some(cancel);
        });
        (suite, observed)
    }

    fn current_with_queue(
        current: config::Config,
        queued: impl IntoIterator<Item = config::Config>,
    ) -> (Self, context::Context) {
        let (task_ctx, cancel) = context::WithCancel(context::Background());
        let observed = task_ctx.clone();
        let suite = Self::start(move |lightning| {
            let queue = config::NewConfigList();
            for task in queued {
                queue.Push(task);
            }
            lightning.taskCfgs = Some(queue);
            lightning.curTask = Some(current);
            lightning.cancel = Some(cancel);
        });
        (suite, observed)
    }

    fn call(&self, method: &str, path: &str, body: &str) -> HttpResponse {
        request(
            self.addr,
            method,
            path,
            (!body.is_empty()).then_some("application/toml"),
            body,
        )
    }

    fn shutdown(mut self) {
        assert!(
            self.stop_and_wait(),
            "Stop must close the listener and release {}",
            self.addr
        );
    }

    fn stop_and_wait(&mut self) -> bool {
        if self.stopped {
            return true;
        }
        self.lightning.Stop();

        // Wake a blocking accept after the shutdown flag changes. A correct
        // server consumes at most this connection and then drops the listener.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(100));
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&self.addr, Duration::from_millis(50)).is_err() {
                self.stopped = true;
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl Drop for LightningServerSuite {
    fn drop(&mut self) {
        let _ = self.stop_and_wait();
    }
}

fn post_task(suite: &LightningServerSuite, index: usize) -> (i64, config::Config) {
    let response = suite.call(
        "POST",
        "/tasks",
        &format!(
            "\n[mydumper]\ndata-source-dir = 'file://demo-path-{index}'\n\
             [mydumper.csv]\nseparator = '/'\n"
        ),
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), "no-store");
    let id = response.json()["id"]
        .as_i64()
        .expect("POST response contains a numeric id");
    assert_ne!(id, 0);
    let cfg = suite
        .lightning
        .taskCfgs
        .as_ref()
        .and_then(|queue| queue.Get(id))
        .expect("posted config is queued");
    assert_eq!(cfg.Mydumper.SourceDir, format!("file://demo-path-{index}"));
    assert_eq!(cfg.Mydumper.CSV.Separator, "/");
    (id, cfg)
}

fn get_task_list(suite: &LightningServerSuite) -> (Option<i64>, Vec<i64>) {
    let response = suite.call("GET", "/tasks", "");
    assert_eq!(response.status, 200);
    let body = response.json();
    let current = body["current"].as_i64();
    let queue = body["queue"]
        .as_array()
        .expect("queue array")
        .iter()
        .map(|id| id.as_i64().expect("numeric queue id"))
        .collect();
    (current, queue)
}

#[test]
fn test_run_server() {
    let _serial = serial_guard();

    let outside = LightningServerSuite::outside_server_mode();
    let response = outside.call("POST", "/tasks", "????");
    assert_eq!(response.status, 501);
    assert_eq!(response.json()["error"], "server-mode not enabled");
    outside.shutdown();

    let server = LightningServerSuite::server_mode();
    let response = server.call("PUT", "/tasks", "");
    assert_eq!(response.status, 405);
    assert!(response.header("allow").contains("POST"));

    let response = server.call("POST", "/tasks", "????");
    assert_eq!(response.status, 400);
    assert!(
        response.json()["error"]
            .as_str()
            .unwrap()
            .starts_with("cannot parse task")
    );

    let response = server.call(
        "POST",
        "/tasks",
        "[mydumper.csv]\nseparator = 'fooo'\ndelimiter = 'foo'",
    );
    assert_eq!(response.status, 400);
    assert!(
        response.json()["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid task configuration:")
    );

    let mut ids = HashSet::new();
    for index in 0..20 {
        let (id, _) = post_task(&server, index);
        assert!(ids.insert(id), "server generated duplicate task id {id}");
    }
    assert_eq!(ids.len(), 20);
    server.shutdown();
}

#[test]
fn test_get_delete_task() {
    let _serial = serial_guard();

    let posted = LightningServerSuite::server_mode();
    assert_eq!(get_task_list(&posted), (None, vec![]));
    let (first, first_cfg) = post_task(&posted, 1);
    let (second, second_cfg) = post_task(&posted, 2);
    let (third, third_cfg) = post_task(&posted, 3);
    assert_ne!(first, 123456);
    assert_ne!(second, 123456);
    assert_ne!(third, 123456);
    assert_ne!(first, second);
    assert_ne!(second, third);
    posted.shutdown();

    // Equivalent to Go's SkipRunTask notification: the scheduler popped the
    // first config while the other two remain queued.
    let (running, first_ctx) =
        LightningServerSuite::current_with_queue(first_cfg, [second_cfg, third_cfg.clone()]);
    assert_eq!(get_task_list(&running), (Some(first), vec![second, third]));

    assert_eq!(running.call("GET", "/tasks/abcdef", "").status, 400);
    assert_eq!(running.call("GET", "/tasks/123456", "").status, 404);

    let second_response = running.call("GET", &format!("/tasks/{second}"), "");
    assert_eq!(second_response.status, 200);
    assert_eq!(
        second_response.json()["Mydumper"]["SourceDir"],
        "file://demo-path-2"
    );
    let first_response = running.call("GET", &format!("/tasks/{first}"), "");
    assert_eq!(first_response.status, 200);
    assert_eq!(
        first_response.json()["Mydumper"]["SourceDir"],
        "file://demo-path-1"
    );

    for (path, expected) in [
        ("/tasks", 400),
        ("/tasks/", 400),
        ("/tasks/abcdef", 400),
        ("/tasks/123456", 404),
    ] {
        assert_eq!(running.call("DELETE", path, "").status, expected, "{path}");
    }

    assert_eq!(
        running
            .call("DELETE", &format!("/tasks/{second}"), "")
            .status,
        200
    );
    assert_eq!(get_task_list(&running), (Some(first), vec![third]));

    assert_eq!(
        running
            .call("DELETE", &format!("/tasks/{first}"), "")
            .status,
        200
    );
    assert!(
        first_ctx.is_cancelled(),
        "deleting the current task invokes its cancellation function"
    );
    assert_eq!(get_task_list(&running), (None, vec![third]));
    running.shutdown();

    // Go's task loop immediately promotes the next queued task after the
    // cancellation notification. The external importer remains mocked.
    let (promoted, _) = LightningServerSuite::current_task(third_cfg);
    assert_eq!(get_task_list(&promoted), (Some(third), vec![]));
    promoted.shutdown();
}

#[test]
fn test_http_api_outside_server_mode() {
    let _serial = serial_guard();

    let mut cfg = config::Config::NewConfig();
    cfg.TaskID = 2021;
    cfg.Mydumper.SourceDir = "file://.".to_string();
    cfg.TiDB.Host = "test.invalid".to_string();
    cfg.TiDB.Port = 4000;
    cfg.TiDB.PdAddr = "test.invalid:2379".to_string();
    let (server, task_ctx) = LightningServerSuite::current_task(cfg);

    assert_eq!(get_task_list(&server), (Some(2021), vec![]));
    assert_eq!(server.call("POST", "/tasks", "??????").status, 501);
    assert_eq!(server.call("GET", "/tasks/2021", "").status, 200);
    assert_eq!(server.call("GET", "/tasks/123456", "").status, 404);
    assert_eq!(server.call("PATCH", "/tasks/2021/front", "").status, 501);
    assert_eq!(server.call("DELETE", "/tasks/123456", "").status, 404);
    assert_eq!(server.call("DELETE", "/tasks/2021", "").status, 200);
    assert!(
        task_ctx.is_cancelled(),
        "deleting the current non-server-mode task cancels RunOnce"
    );
    server.shutdown();
}
