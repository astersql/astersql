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

//! Go-equivalent tests for `tests/graceshutdown` (`graceshutdown_test.go`).
//!
//! Mapping:
//! - `startTiDBWithoutPD` → [`start_tidb_without_pd`]
//! - `stopService` → [`stop_service`]
//! - `connectTiDB` → [`connect_tidb`]
//! - `TestGracefulShutdown` → [`test_graceful_shutdown`]
//!
//! Platform (darwin arm64): no kv/domain/kvproto/grpcio. Go already uses
//! `--store=mocktikv`. This slim harness keeps that store boundary and stubs the
//! tidb-server process + MySQL driver in-process while preserving
//! graceful-shutdown-waits-for-in-transaction-connections
//! (https://github.com/pingcap/tidb/pull/44953).

// 本文件对应 `tests/graceshutdown/graceshutdown_test.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// `tidb_binary_path` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tidb_binary_path() -> String {
    std::env::var("TIDB_BINARY_PATH").unwrap_or_else(|_| "bin/tidb-server".to_string())
}

// `tmp_path` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tmp_path() -> String {
    std::env::var("TIDB_GRACESHUTDOWN_TMP")
        .unwrap_or_else(|_| "/tmp/tidb_gracefulshutdown".to_string())
}

// `tidb_start_port` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tidb_start_port() -> i32 {
    std::env::var("TIDB_START_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5500)
}

// `tidb_status_port` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tidb_status_port() -> i32 {
    std::env::var("TIDB_STATUS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8500)
}

#[derive(Clone, Debug)]
// `Error` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Error {
    msg: String,
}

// 这里实现 `Error` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Error {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

// 这里实现 `std::fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl std::fmt::Display for Error {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

// 这里实现 `std::error::Error` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl std::error::Error for Error {}

// `trace` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn trace(err: Error) -> Error {
    err
}

// `ServerInner` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct ServerInner {
    alive: bool,
    shutting_down: bool,
    exited: bool,
    active_txns: usize,
    tables: HashMap<String, Vec<i64>>,
}

// `MockTiDB` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct MockTiDB {
    port: i32,
    inner: Mutex<ServerInner>,
    cv: Condvar,
}

// 这里实现 `MockTiDB` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl MockTiDB {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn new(port: i32) -> Arc<Self> {
        Arc::new(Self {
            port,
            inner: Mutex::new(ServerInner {
                alive: true,
                shutting_down: false,
                exited: false,
                active_txns: 0,
                tables: HashMap::new(),
            }),
            cv: Condvar::new(),
        })
    }

    // `interrupt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn interrupt(&self) {
        let mut g = self.inner.lock().unwrap();
        g.shutting_down = true;
        self.cv.notify_all();
        while g.active_txns > 0 {
            g = self.cv.wait(g).unwrap();
        }
        g.alive = false;
        g.exited = true;
        self.cv.notify_all();
    }

    // `wait_exit` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn wait_exit(&self) {
        let mut g = self.inner.lock().unwrap();
        while !g.exited {
            g = self.cv.wait(g).unwrap();
        }
    }

    // `begin_txn` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn begin_txn(&self) -> Result<(), Error> {
        let mut g = self.inner.lock().unwrap();
        if !g.alive || g.exited {
            return Err(Error::new("server closed"));
        }
        if g.shutting_down {
            return Err(Error::new("server is shutting down"));
        }
        g.active_txns += 1;
        Ok(())
    }

    // `end_txn` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn end_txn(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.active_txns > 0 {
            g.active_txns -= 1;
        }
        self.cv.notify_all();
    }

    // `exec_sql` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn exec_sql(&self, sql: &str) -> Result<(), Error> {
        let sql_trim = sql.trim().trim_end_matches(';').trim();
        let lower = sql_trim.to_ascii_lowercase();
        let mut g = self.inner.lock().unwrap();
        if g.exited || !g.alive {
            return Err(Error::new("server closed"));
        }
        if lower.starts_with("drop table if exists ") {
            let name = sql_trim[lower.find("exists ").unwrap() + 7..]
                .trim()
                .to_string();
            g.tables.remove(&name);
            return Ok(());
        }
        if lower.starts_with("create table ") {
            let rest = &sql_trim["create table ".len()..];
            let name = rest.split('(').next().unwrap_or("").trim().to_string();
            g.tables.entry(name).or_default();
            return Ok(());
        }
        if lower.starts_with("insert into ") {
            let after_into = &sql_trim["insert into ".len()..];
            let name = after_into
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            let val = after_into
                .rfind('(')
                .and_then(|i| after_into[i + 1..].split(')').next())
                .and_then(|s| s.trim().parse::<i64>().ok())
                .ok_or_else(|| Error::new(format!("bad insert: {sql_trim}")))?;
            g.tables.entry(name).or_default().push(val);
            return Ok(());
        }
        Err(Error::new(format!("unsupported exec: {sql_trim}")))
    }

    // `query_row_sleep_select` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query_row_sleep_select(&self, sql: &str, deadline: Instant) -> Result<i64, Error> {
        let sql_trim = sql.trim().trim_end_matches(';').trim();
        let lower = sql_trim.to_ascii_lowercase();
        let sleep_secs = parse_sleep_seconds(&lower)
            .ok_or_else(|| Error::new(format!("unsupported query: {sql_trim}")))?;

        {
            let g = self.inner.lock().unwrap();
            if g.exited {
                return Err(Error::new("server closed"));
            }
            if g.active_txns == 0 {
                return Err(Error::new("query requires an open transaction"));
            }
        }

        let sleep_for = Duration::from_secs(sleep_secs);
        if Instant::now() + sleep_for > deadline {
            return Err(Error::new("context deadline exceeded"));
        }
        thread::sleep(sleep_for);

        let g = self.inner.lock().unwrap();
        if g.exited {
            return Err(Error::new("server closed before query finished"));
        }
        let rows = g
            .tables
            .get("t")
            .ok_or_else(|| Error::new("table t does not exist"))?;
        if rows.is_empty() {
            return Err(Error::new("no rows"));
        }
        Ok(1)
    }
}

// `parse_sleep_seconds` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn parse_sleep_seconds(lower_sql: &str) -> Option<u64> {
    let key = "sleep(";
    let idx = lower_sql.find(key)?;
    let rest = &lower_sql[idx + key.len()..];
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

// `REGISTRY` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static REGISTRY: Mutex<Option<HashMap<i32, Arc<MockTiDB>>>> = Mutex::new(None);

// `register_server` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn register_server(server: Arc<MockTiDB>) {
    let mut g = REGISTRY.lock().unwrap();
    g.get_or_insert_with(HashMap::new)
        .insert(server.port, server);
}

// `lookup_server` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn lookup_server(port: i32) -> Option<Arc<MockTiDB>> {
    REGISTRY
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&port).cloned())
}

// `unregister_server` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn unregister_server(port: i32) {
    if let Some(map) = REGISTRY.lock().unwrap().as_mut() {
        map.remove(&port);
    }
}

// `Cmd` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Cmd {
    name: String,
    args: Vec<String>,
    server: Arc<MockTiDB>,
    signaled: AtomicBool,
}

// `start_tidb_without_pd` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn start_tidb_without_pd(port: i32, status_port: i32) -> Result<Cmd, Error> {
    let bin = tidb_binary_path();
    let tmp = tmp_path();
    let args = vec![
        "--store=mocktikv".to_string(),
        format!("--path={tmp}/mocktikv"),
        format!("-P={port}"),
        format!("--status={status_port}"),
        format!("--log-file={tmp}/tidb{port}.log"),
    ];
    assert_eq!(args[0], "--store=mocktikv");
    std::fs::create_dir_all(format!("{tmp}/mocktikv"))
        .map_err(|e| trace(Error::new(format!("create mocktikv path: {e}"))))?;

    eprintln!("[INFO] starting tidb bin={bin} args={}", args.join(" "));

    let server = MockTiDB::new(port);
    register_server(Arc::clone(&server));
    thread::sleep(Duration::from_millis(500));

    Ok(Cmd {
        name: bin,
        args,
        server,
        signaled: AtomicBool::new(false),
    })
}

// `stop_service` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn stop_service(name: &str, cmd: &Cmd) -> Result<(), Error> {
    if cmd.signaled.swap(true, Ordering::SeqCst) {
        return Err(Error::new("already signaled"));
    }
    eprintln!("[INFO] service Interrupt name={name}");
    cmd.server.interrupt();
    cmd.server.wait_exit();
    unregister_server(cmd.server.port);
    eprintln!("[INFO] service stopped gracefully name={name}");
    let _ = &cmd.name;
    Ok(())
}

// `connect_tidb` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn connect_tidb(port: i32) -> Result<DB, Error> {
    let addr = format!("127.0.0.1:{port}");
    let dsn = format!("root@({addr})/test");
    let mut sleep_time = Duration::from_millis(250);
    let start_time = Instant::now();
    let max_retry = 10;
    let mut last_err: Option<Error> = None;

    for i in 0..max_retry {
        match try_open(&dsn, port) {
            Ok(db) => {
                if let Err(err) = db.ping() {
                    eprintln!("[WARN] ping addr failed addr={addr} retry count={i} err={err}");
                    last_err = Some(err);
                    if let Err(err1) = db.close() {
                        eprintln!("[WARN] close db failed retry count={i} err={err1}");
                        last_err = Some(err1);
                        break;
                    }
                } else {
                    db.set_max_open_conns(10);
                    eprintln!("[INFO] connect to server ok addr={addr}");
                    return Ok(db);
                }
            }
            Err(err) => {
                eprintln!("[WARN] open addr failed addr={addr} retry count={i} err={err}");
                last_err = Some(err);
            }
        }
        thread::sleep(sleep_time);
        sleep_time += sleep_time;
    }

    let err = last_err.unwrap_or_else(|| Error::new("connect to server addr failed"));
    eprintln!(
        "[ERROR] connect to server addr failed addr={addr} take time={:?} err={err}",
        start_time.elapsed()
    );
    Err(trace(err))
}

// `try_open` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn try_open(_dsn: &str, port: i32) -> Result<DB, Error> {
    let server = lookup_server(port).ok_or_else(|| Error::new("no server on port"))?;
    let g = server.inner.lock().unwrap();
    if !g.alive || g.exited {
        return Err(Error::new("server not ready"));
    }
    drop(g);
    Ok(DB {
        server,
        max_open: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
    })
}

// `DB` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct DB {
    server: Arc<MockTiDB>,
    max_open: AtomicUsize,
    closed: AtomicBool,
}

// 这里实现 `DB` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl DB {
    // `set_max_open_conns` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    fn set_max_open_conns(&self, n: usize) {
        self.max_open.store(n, Ordering::SeqCst);
    }

    // `ping` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn ping(&self) -> Result<(), Error> {
        let g = self.server.inner.lock().unwrap();
        if g.alive && !g.exited {
            Ok(())
        } else {
            Err(Error::new("ping failed: server not alive"))
        }
    }

    // `conn` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn conn(&self, deadline: Instant) -> Result<Conn, Error> {
        if Instant::now() > deadline {
            return Err(Error::new("context deadline exceeded"));
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::new("sql: database is closed"));
        }
        Ok(Conn {
            server: Arc::clone(&self.server),
            closed: AtomicBool::new(false),
        })
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// `Conn` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Conn {
    server: Arc<MockTiDB>,
    closed: AtomicBool,
}

// 这里实现 `Conn` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Conn {
    // `exec_context` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn exec_context(&self, deadline: Instant, sql: &str) -> Result<(), Error> {
        if Instant::now() > deadline {
            return Err(Error::new("context deadline exceeded"));
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::new("sql: connection is closed"));
        }
        self.server.exec_sql(sql)
    }

    // `begin_tx` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn begin_tx(&self, deadline: Instant) -> Result<Tx, Error> {
        if Instant::now() > deadline {
            return Err(Error::new("context deadline exceeded"));
        }
        self.server.begin_txn()?;
        Ok(Tx {
            server: Arc::clone(&self.server),
            done: AtomicBool::new(false),
        })
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// `Tx` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Tx {
    server: Arc<MockTiDB>,
    done: AtomicBool,
}

// 这里实现 `Tx` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Tx {
    // `query_row_context` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query_row_context(&self, deadline: Instant, sql: &str) -> Result<i64, Error> {
        if self.done.load(Ordering::SeqCst) {
            return Err(Error::new(
                "sql: transaction has already been committed or rolled back",
            ));
        }
        self.server.query_row_sleep_select(sql, deadline)
    }

    // `commit` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn commit(&self) -> Result<(), Error> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Err(Error::new(
                "sql: transaction has already been committed or rolled back",
            ));
        }
        self.server.end_txn();
        Ok(())
    }
}

/// `TestGracefulShutdown`: interrupt during in-txn `sleep(3)` must still finish.
// 测试 `test_graceful_shutdown` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `test_graceful_shutdown` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_graceful_shutdown() {
    let port = tidb_start_port() + 1;
    let tidb = start_tidb_without_pd(port, tidb_status_port()).expect("startTiDBWithoutPD");

    assert!(
        tidb.args.iter().any(|a| a == "--store=mocktikv"),
        "Go starts TiDB with --store=mocktikv"
    );
    let port_arg = format!("-P={port}");
    assert!(
        tidb.args.iter().any(|a| a == &port_arg),
        "Go passes listening port"
    );

    let db = connect_tidb(port).expect("connectTiDB");

    let deadline = Instant::now() + Duration::from_secs(10);
    let conn1 = db.conn(deadline).expect("db.Conn");

    conn1
        .exec_context(deadline, "drop table if exists t;")
        .expect("drop table");
    conn1
        .exec_context(deadline, "create table t(a int);")
        .expect("create table");
    conn1
        .exec_context(deadline, "insert into t values(1);")
        .expect("insert");

    let (done_tx, done_rx) = mpsc::channel::<Result<(), String>>();
    let stop_cmd = Cmd {
        name: tidb.name.clone(),
        args: tidb.args.clone(),
        server: Arc::clone(&tidb.server),
        signaled: AtomicBool::new(false),
    };
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(1));
        let res = stop_service("tidb", &stop_cmd).map_err(|e| e.msg);
        let _ = done_tx.send(res);
    });

    // Graceful shutdown will wait for connections in transaction only.
    // See https://github.com/pingcap/tidb/pull/44953.
    let txn = conn1.begin_tx(deadline).expect("BeginTx");
    let sql = "select 1 from t where not (select sleep(3)) ;";
    let query_started = Instant::now();
    let a = txn.query_row_context(deadline, sql).expect("QueryRow/Scan");
    let query_elapsed = query_started.elapsed();
    assert_eq!(a, 1_i64);
    assert!(
        query_elapsed >= Duration::from_millis(2900),
        "sleep(3) must take ~3s, elapsed={query_elapsed:?}"
    );
    txn.commit().expect("Commit");

    conn1.close().expect("conn1.Close");

    let stop_res = done_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("done channel");
    assert!(stop_res.is_ok(), "stopService: {stop_res:?}");
    assert!(
        tidb.server.inner.lock().unwrap().exited,
        "tidb process must have exited after graceful drain"
    );

    db.close().expect("db.Close");
}
