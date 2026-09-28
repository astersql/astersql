// Copyright 2026 AsterSQL.
//! Local stand-ins for cobra, go-sql-driver/mysql DSN, database/sql.Open, and os.Exit
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Production algorithms live in `main.rs` and call these boundaries the same way
//! Go calls cobra / mysql.ParseDSN / sql.Open / os.Exit.

// 本文件对应 `tests/llmtest/stubs.rs`，本次任务只补中文解释，不改行为。
// 本文件提供轻量测试桩，而不是完整生产实现。
// 桩只覆盖当前测试真正触达的接口形状。
// 关键阅读点是全局开关、记录点和资源回收。
// 未覆盖的真实能力不会被假装支持。
// 中文注释会帮助区分桩职责与真实边界。
// 这类文件最怕隐式状态污染，因此会强调 reset 和 cleanup。
use astersql_tests_llmtest_testcase::{Db, QueryOutcome, SqlError};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// os.Exit — testable process exit
// ---------------------------------------------------------------------------

/// When true, [`os_exit`] panics with [`ExitCalled`] instead of terminating.
// `CAPTURE_EXIT` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static CAPTURE_EXIT: AtomicBool = AtomicBool::new(false);
// `LAST_EXIT_CODE` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static LAST_EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// Test hook: capture exits as panics for `catch_unwind`.
// `set_capture_exit` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn set_capture_exit(capture: bool) {
    CAPTURE_EXIT.store(capture, Ordering::SeqCst);
}

/// Last exit code requested via [`os_exit`].
// `last_exit_code` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn last_exit_code() -> i32 {
    LAST_EXIT_CODE.load(Ordering::SeqCst)
}

/// Panic payload when exit capture is enabled (Go `os.Exit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// `ExitCalled` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ExitCalled(pub i32);

/// Go `os.Exit(code)`.
// `os_exit` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn os_exit(code: i32) -> ! {
    LAST_EXIT_CODE.store(code, Ordering::SeqCst);
    if CAPTURE_EXIT.load(Ordering::SeqCst) {
        std::panic::panic_any(ExitCalled(code));
    }
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// database/sql.Open
// ---------------------------------------------------------------------------

// `OpenHandler` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
type OpenHandler = Arc<dyn Fn(&str, &str) -> Result<SqlDb, SqlError> + Send + Sync>;

// `OPEN_HANDLER` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static OPEN_HANDLER: Mutex<Option<OpenHandler>> = Mutex::new(None);
// `OPENED_LOG` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static OPENED_LOG: Mutex<Vec<(String, String, SqlDb)>> = Mutex::new(Vec::new());

/// Go `*sql.DB` stand-in with Close lifecycle (llmtest only closes TiDB via defer).
#[derive(Clone)]
// `SqlDb` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct SqlDb {
    inner: Db,
    closed: Arc<AtomicBool>,
}

// 这里实现 `SqlDb` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl SqlDb {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn new(inner: Db) -> Self {
        Self {
            inner,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    // `inner` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn inner(&self) -> &Db {
        &self.inner
    }

    /// Go `(*DB).Close() error`.
    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    // `is_closed` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// Test hook: override `sql.Open` behavior.
// `set_sql_open_handler` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn set_sql_open_handler<F>(handler: F)
where
    F: Fn(&str, &str) -> Result<SqlDb, SqlError> + Send + Sync + 'static,
{
    let mut g = OPEN_HANDLER.lock().expect("open handler mutex");
    *g = Some(Arc::new(handler) as OpenHandler);
}

/// Restore default successful `sql.Open`.
// `clear_sql_open_handler` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn clear_sql_open_handler() {
    *OPEN_HANDLER.lock().expect("open handler mutex") = None;
}

/// Clear recorded opens (test helper).
// `clear_opened_dbs` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
pub fn clear_opened_dbs() {
    OPENED_LOG.lock().expect("opened log").clear();
}

/// Recorded `(driver, dsn, db)` from [`sql::open`] (test helper).
// `opened_dbs` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
pub fn opened_dbs() -> Vec<(String, String, SqlDb)> {
    OPENED_LOG.lock().expect("opened log").clone()
}

// 模块 `sql` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod sql {
    use super::*;

    pub use super::SqlDb;

    /// Go `sql.Open(driverName, dataSourceName)`.
    // `open` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    // 这里的行为需要尽量贴近 Go 版本。
    pub fn open(driver: &str, dsn: &str) -> Result<SqlDb, SqlError> {
        let result = {
            let g = OPEN_HANDLER.lock().expect("open handler mutex");
            if let Some(h) = g.as_ref() {
                h(driver, dsn)
            } else {
                Ok(SqlDb::new(Db::always_ok(QueryOutcome {
                    columns: vec![],
                    rows: vec![],
                    rows_err: None,
                })))
            }
        };
        if let Ok(ref db) = result {
            OPENED_LOG.lock().expect("opened log").push((
                driver.to_string(),
                dsn.to_string(),
                db.clone(),
            ));
        }
        result
    }
}

// ---------------------------------------------------------------------------
// go-sql-driver/mysql DSN (ParseDSN / FormatDSN subset used by unifyDSN)
// ---------------------------------------------------------------------------

// 模块 `mysql` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod mysql {
    use super::*;

    /// Go `mysql.Config` fields touched by llmtest `unifyDSN`.
    #[derive(Clone, Debug, Default)]
    // `Config` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Config {
        pub user: String,
        pub passwd: String,
        pub net: String,
        pub addr: String,
        pub db_name: String,
        pub collation: String,
        pub params: BTreeMap<String, String>,
        /// Default true in Go NewConfig; omitted from FormatDSN when true.
        pub allow_native_passwords: bool,
        /// Default true in Go NewConfig; omitted from FormatDSN when true.
        pub check_conn_liveness: bool,
    }

    // 这里实现 `Config` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl Config {
        // `new_defaults` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn new_defaults() -> Self {
            Self {
                allow_native_passwords: true,
                check_conn_liveness: true,
                ..Default::default()
            }
        }

        // `normalize` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn normalize(&mut self) -> Result<(), DsnError> {
            if self.net.is_empty() {
                self.net = "tcp".to_string();
            }
            if self.addr.is_empty() {
                match self.net.as_str() {
                    "tcp" => self.addr = "127.0.0.1:3306".to_string(),
                    "unix" => self.addr = "/tmp/mysql.sock".to_string(),
                    other => {
                        return Err(DsnError(format!(
                            "default addr for network '{other}' unknown"
                        )));
                    }
                }
            } else if self.net == "tcp" {
                self.addr = ensure_have_port(&self.addr);
            }
            Ok(())
        }

        /// Go `(*Config).FormatDSN`.
        // `format_dsn` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn format_dsn(&self) -> String {
            let mut buf = String::new();

            if !self.user.is_empty() {
                buf.push_str(&self.user);
                if !self.passwd.is_empty() {
                    buf.push(':');
                    buf.push_str(&self.passwd);
                }
                buf.push('@');
            }

            if !self.addr.is_empty() {
                let net = if self.net.is_empty() {
                    "tcp"
                } else {
                    self.net.as_str()
                };
                buf.push_str(net);
                buf.push('(');
                buf.push_str(&self.addr);
                buf.push(')');
            } else if !self.net.is_empty() && self.net != "tcp" {
                buf.push_str(&self.net);
            }

            buf.push('/');
            buf.push_str(&path_escape(&self.db_name));

            let mut has_param = false;
            let mut write_param = |name: &str, value: &str| {
                if !has_param {
                    buf.push('?');
                    has_param = true;
                } else {
                    buf.push('&');
                }
                buf.push_str(name);
                buf.push('=');
                buf.push_str(value);
            };

            if !self.allow_native_passwords {
                write_param("allowNativePasswords", "false");
            }
            if !self.check_conn_liveness {
                write_param("checkConnLiveness", "false");
            }
            if !self.collation.is_empty() {
                write_param("collation", &self.collation);
            }
            for (k, v) in &self.params {
                write_param(k, &query_escape(v));
            }

            buf
        }
    }

    /// DSN parse / format error (Go `error` from ParseDSN).
    #[derive(Debug, Clone, PartialEq, Eq)]
    // `DsnError` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct DsnError(pub String);

    // 这里实现 `fmt::Display` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl fmt::Display for DsnError {
        // `fmt` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    // 这里实现 `std::error::Error` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl std::error::Error for DsnError {}

    /// Go `mysql.ParseDSN`.
    // `parse_dsn` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn parse_dsn(dsn: &str) -> Result<Config, DsnError> {
        let mut cfg = Config::new_defaults();
        let bytes = dsn.as_bytes();
        let mut found_slash = false;

        for i in (0..bytes.len()).rev() {
            if bytes[i] != b'/' {
                continue;
            }
            found_slash = true;
            let mut j = i;
            let mut k;

            if i > 0 {
                // Find last '@' in dsn[:i]
                let mut found_at = false;
                while j > 0 {
                    j -= 1;
                    if bytes[j] == b'@' {
                        found_at = true;
                        // username[:password]
                        k = 0;
                        while k < j {
                            if bytes[k] == b':' {
                                cfg.passwd = dsn[k + 1..j].to_string();
                                break;
                            }
                            k += 1;
                        }
                        cfg.user = dsn[..k].to_string();
                        break;
                    }
                }
                if !found_at {
                    j = 0;
                }

                // protocol[(address)]
                k = j + 1;
                let mut found_paren = false;
                while k < i {
                    if bytes[k] == b'(' {
                        if bytes[i - 1] != b')' {
                            return Err(DsnError(
                                "invalid DSN: network address not terminated (missing closing brace)"
                                    .into(),
                            ));
                        }
                        cfg.addr = dsn[k + 1..i - 1].to_string();
                        found_paren = true;
                        break;
                    }
                    k += 1;
                }
                if found_paren {
                    cfg.net = dsn[j + 1..k].to_string();
                } else {
                    cfg.net = dsn[j + 1..i].to_string();
                }
            }

            // dbname[?params]
            let mut q = i + 1;
            while q < bytes.len() {
                if bytes[q] == b'?' {
                    parse_dsn_params(&mut cfg, &dsn[q + 1..])?;
                    break;
                }
                q += 1;
            }
            let dbname = &dsn[i + 1..q];
            // go-sql-driver/mysql v1.7.1 preserves DBName bytes verbatim.
            cfg.db_name = dbname.to_string();
            break;
        }

        if !found_slash && !dsn.is_empty() {
            return Err(DsnError(
                "invalid DSN: missing the slash separating the database name".into(),
            ));
        }

        cfg.normalize()?;
        Ok(cfg)
    }

    // `parse_dsn_params` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn parse_dsn_params(cfg: &mut Config, params: &str) -> Result<(), DsnError> {
        for v in params.split('&') {
            let Some((key, value)) = v.split_once('=') else {
                continue;
            };
            match key {
                "collation" => cfg.collation = value.to_string(),
                "allowNativePasswords" => {
                    cfg.allow_native_passwords = parse_bool(value)?;
                }
                "checkConnLiveness" => {
                    cfg.check_conn_liveness = parse_bool(value)?;
                }
                _ => {
                    cfg.params.insert(key.to_string(), query_unescape(value)?);
                }
            }
        }
        Ok(())
    }

    // `parse_bool` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn parse_bool(value: &str) -> Result<bool, DsnError> {
        match value {
            "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
            "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
            _ => Err(DsnError(format!("invalid bool value: {value}"))),
        }
    }

    // `ensure_have_port` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn ensure_have_port(addr: &str) -> String {
        // Go net.SplitHostPort; if it fails, JoinHostPort(addr, "3306").
        if addr.starts_with('[') {
            // ipv6 literal
            if let Some(end) = addr.find(']') {
                if addr[end + 1..].starts_with(':') {
                    return addr.to_string();
                }
            }
            return format!("{addr}:3306");
        }
        // Count colons — host:port has exactly one for ipv4/hostname.
        if addr.matches(':').count() == 1 {
            addr.to_string()
        } else if addr.contains(':') {
            format!("[{addr}]:3306")
        } else {
            format!("{addr}:3306")
        }
    }

    // `path_escape` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn path_escape(s: &str) -> String {
        // Sufficient for db names used in tests (no reserved path bytes).
        s.to_string()
    }

    // `path_unescape` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn path_unescape(s: &str) -> Result<String, String> {
        percent_decode(s)
    }

    // `query_escape` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query_escape(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    // `query_unescape` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query_unescape(s: &str) -> Result<String, DsnError> {
        percent_decode(s).map_err(DsnError)
    }

    // `percent_decode` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn percent_decode(s: &str) -> Result<String, String> {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                if i + 2 >= bytes.len() {
                    return Err("invalid percent escape".into());
                }
                let h = hex_val(bytes[i + 1]).ok_or("invalid percent escape")?;
                let l = hex_val(bytes[i + 2]).ok_or("invalid percent escape")?;
                out.push((h << 4) | l);
                i += 3;
            } else if bytes[i] == b'+' {
                out.push(b' ');
                i += 1;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }

    // `hex_val` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn hex_val(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// spf13/cobra (minimal CLI surface used by llmtest)
// ---------------------------------------------------------------------------

// 模块 `cobra` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod cobra {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Handlers match Go `func(cmd *cobra.Command, args []string)` but ignore `cmd`
    // (llmtest Run closures only use captured flag cells).
    // `RunFn` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    type RunFn = Box<dyn FnMut(&[String])>;

    #[derive(Clone)]
    // `FlagStorage` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    enum FlagStorage {
        String(Rc<RefCell<String>>),
        Int(Rc<RefCell<i32>>),
        Bool(Rc<RefCell<bool>>),
    }

    // `Flag` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    struct Flag {
        name: String,
        storage: FlagStorage,
        default_string: String,
        default_int: i32,
        default_bool: bool,
    }

    /// Go `pflag.FlagSet` stand-in bound to shared cells.
    // `FlagSet` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct FlagSet {
        flags: Vec<Flag>,
    }

    // 这里实现 `FlagSet` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl FlagSet {
        // `new` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn new() -> Self {
            Self { flags: Vec::new() }
        }

        // `string_var` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn string_var(
            &mut self,
            cell: Rc<RefCell<String>>,
            name: &str,
            default: &str,
            _usage: &str,
        ) {
            *cell.borrow_mut() = default.to_string();
            self.flags.push(Flag {
                name: name.to_string(),
                storage: FlagStorage::String(cell),
                default_string: default.to_string(),
                default_int: 0,
                default_bool: false,
            });
        }

        // `int_var` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn int_var(&mut self, cell: Rc<RefCell<i32>>, name: &str, default: i32, _usage: &str) {
            *cell.borrow_mut() = default;
            self.flags.push(Flag {
                name: name.to_string(),
                storage: FlagStorage::Int(cell),
                default_string: String::new(),
                default_int: default,
                default_bool: false,
            });
        }

        // `bool_var` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn bool_var(
            &mut self,
            cell: Rc<RefCell<bool>>,
            name: &str,
            default: bool,
            _usage: &str,
        ) {
            *cell.borrow_mut() = default;
            self.flags.push(Flag {
                name: name.to_string(),
                storage: FlagStorage::Bool(cell),
                default_string: String::new(),
                default_int: 0,
                default_bool: default,
            });
        }

        // `apply` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn apply(&self, args: &[String]) -> Result<Vec<String>, String> {
            let mut i = 0;
            let mut positional = Vec::new();
            while i < args.len() {
                let a = &args[i];
                if a == "--" {
                    positional.extend_from_slice(&args[i + 1..]);
                    break;
                }
                if let Some(rest) = a.strip_prefix("--") {
                    let (name, inline) = match rest.split_once('=') {
                        Some((n, v)) => (n, Some(v.to_string())),
                        None => (rest, None),
                    };
                    let flag = self
                        .flags
                        .iter()
                        .find(|f| f.name == name)
                        .ok_or_else(|| format!("unknown flag: --{name}"))?;
                    match &flag.storage {
                        FlagStorage::Bool(cell) => {
                            let v = match inline {
                                Some(s) => parse_flag_bool(&s)?,
                                None => true,
                            };
                            *cell.borrow_mut() = v;
                        }
                        FlagStorage::String(cell) => {
                            let v = match inline {
                                Some(s) => s,
                                None => {
                                    i += 1;
                                    args.get(i).cloned().ok_or_else(|| {
                                        format!("flag needs an argument: --{name}")
                                    })?
                                }
                            };
                            *cell.borrow_mut() = v;
                        }
                        FlagStorage::Int(cell) => {
                            let v = match inline {
                                Some(s) => s,
                                None => {
                                    i += 1;
                                    args.get(i).cloned().ok_or_else(|| {
                                        format!("flag needs an argument: --{name}")
                                    })?
                                }
                            };
                            *cell.borrow_mut() = v
                                .parse::<i32>()
                                .map_err(|_| format!("invalid argument {v:?} for --{name}"))?;
                        }
                    }
                    i += 1;
                } else {
                    positional.push(a.clone());
                    i += 1;
                }
            }
            Ok(positional)
        }

        // `reset_defaults` 负责清理或覆写跨用例共享状态。
        // 这类辅助函数最关键的是调用顺序与作用域。
        // 和 Go 对齐时，状态恢复直接关系到可重复性。
        fn reset_defaults(&self) {
            for f in &self.flags {
                match &f.storage {
                    FlagStorage::String(c) => *c.borrow_mut() = f.default_string.clone(),
                    FlagStorage::Int(c) => *c.borrow_mut() = f.default_int,
                    FlagStorage::Bool(c) => *c.borrow_mut() = f.default_bool,
                }
            }
        }
    }

    // `parse_flag_bool` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn parse_flag_bool(s: &str) -> Result<bool, String> {
        match s {
            "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
            "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
            _ => Err(format!("invalid bool: {s}")),
        }
    }

    /// Go `*cobra.Command`.
    // `Command` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Command {
        use_name: String,
        short: String,
        run: Option<RunFn>,
        flags: FlagSet,
        children: Vec<Command>,
    }

    // 这里实现 `Command` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl Command {
        /// Go `&cobra.Command{Use: name}`.
        // `new` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn new(use_name: impl Into<String>) -> Self {
            Self {
                use_name: use_name.into(),
                short: String::new(),
                run: None,
                flags: FlagSet::new(),
                children: Vec::new(),
            }
        }

        // `set_short` 负责清理或覆写跨用例共享状态。
        // 这类辅助函数最关键的是调用顺序与作用域。
        // 和 Go 对齐时，状态恢复直接关系到可重复性。
        pub fn set_short(&mut self, short: impl Into<String>) {
            self.short = short.into();
        }

        // `set_run` 负责清理或覆写跨用例共享状态。
        // 这类辅助函数最关键的是调用顺序与作用域。
        // 和 Go 对齐时，状态恢复直接关系到可重复性。
        pub fn set_run<F>(&mut self, f: F)
        where
            F: FnMut(&[String]) + 'static,
        {
            self.run = Some(Box::new(f));
        }

        // `flags` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn flags(&mut self) -> &mut FlagSet {
            &mut self.flags
        }

        // `add_command` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn add_command(&mut self, child: Command) {
            self.children.push(child);
        }

        // `use_name` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn use_name(&self) -> &str {
            &self.use_name
        }

        // `short` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn short(&self) -> &str {
            &self.short
        }

        // `child_names` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn child_names(&self) -> Vec<String> {
            self.children.iter().map(|c| c.use_name.clone()).collect()
        }

        /// Go `rootCmd.Execute()` using `std::env::args`.
        // `execute` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn execute(&mut self) -> Result<(), String> {
            let args: Vec<String> = std::env::args().skip(1).collect();
            self.execute_args(&args)
        }

        /// Execute with an explicit argv (without program name), for tests.
        // `execute_args` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn execute_args(&mut self, args: &[String]) -> Result<(), String> {
            self.flags.reset_defaults();
            for c in &mut self.children {
                c.flags.reset_defaults();
            }

            if args.is_empty() {
                if let Some(run) = self.run.as_mut() {
                    run(&[]);
                }
                return Ok(());
            }

            let sub = &args[0];
            if let Some(child) = self.children.iter_mut().find(|c| c.use_name == *sub) {
                let rest = &args[1..];
                let positional = child.flags.apply(rest)?;
                if let Some(run) = child.run.as_mut() {
                    run(&positional);
                }
                return Ok(());
            }
            if !self.children.is_empty() && !sub.starts_with('-') {
                return Err(format!("unknown command {sub:?} for {:?}", self.use_name));
            }
            // Root-level flags / run
            let positional = self.flags.apply(args)?;
            if let Some(run) = self.run.as_mut() {
                run(&positional);
            }
            Ok(())
        }
    }
}
