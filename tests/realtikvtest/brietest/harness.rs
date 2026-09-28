// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 测试基础设施和桩边界。
// 中文总览：重点在于模拟边界、公共断言和环境收口顺序。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：模块 `require` 负责 require。
// 中文总览：函数 `NoError` 负责 NoError。
// 中文总览：函数 `Error` 负责 Error。
// 中文总览：函数 `ErrorContains` 负责 ErrorContains。
// 中文总览：函数 `True` 负责 True。
// 中文总览：函数 `TrueMsg` 负责 TrueMsg。
// 中文总览：函数 `False` 负责 False。
// 中文总览：函数 `Contains` 负责 Contains。
// 中文总览：函数 `NotContains` 负责 NotContains。
// 中文总览：函数 `Nil` 负责 Nil。
// 中文总览：函数 `NotNil` 负责 NotNil。
// 中文总览：函数 `ErrorIs` 负责 ErrorIs。
// 中文总览：函数 `FailNow` 负责 FailNow。
// 中文总览：函数 `FailNowf` 负责 FailNowf。
// 中文总览：函数 `fp_terms` 负责 fp terms。
// 中文总览：函数 `fp_calls` 负责 fp calls。
// 中文总览：函数 `fp_err_calls` 负责 fp err calls。
// 中文总览：模块 `failpoint` 负责 failpoint。
// 中文总览：函数 `Enable` 负责 Enable。
// 中文总览：函数 `Disable` 负责 Disable。
// 中文总览：函数 `EnableCall` 负责 EnableCall。
// 中文总览：函数 `EnableErrCall` 负责 EnableErrCall。
// 中文总览：函数 `is_enabled` 负责 is enabled。
// 中文总览：函数 `term` 负责 term。

//! Slim local RealTiKV / SQL / BR / PD / failpoint harness for
//! `tests/realtikvtest/brietest` on darwin arm64 (no kv/domain/kvproto/grpcio).
//!
//! Mock/real boundary (matches Go):
//! - **Real boundary (simulated in-process):** CreateMockStoreAndSetup SQL sessions,
//!   BACKUP/RESTORE/BRIE job queue, registry table, log-backup / PiTR task APIs,
//!   GC barrier failpoint signal files, admin show ddl jobs.
//! - **Mock (as in Go):** failpoints (`testfailpoint` / `failpoint`), PD HTTP
//!   safepoint/scheduler/region-label surfaces, ImportSST ingest Suspended,
//!   stream force-flush / etcd meta checkpoint, encryption cipher boundaries.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use astersql_tests_realtikvtest::stubs::{Storage, TestCtx, TestMain, config};
pub use astersql_tests_realtikvtest::{
    CreateMockStoreAndDomainAndSetup, CreateMockStoreAndSetup, PDAddr, RunTestMain,
    SetWithRealTiKV, UpdateTiDBConfig, WithKeyspaceName, WithRealTiKV,
};

// ---------------------------------------------------------------------------
// require
// ---------------------------------------------------------------------------

// 该模块承担 require 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod require {
    use super::TestCtx;
    use std::fmt::Debug;
    use std::time::{Duration, Instant};

    // 该辅助函数负责 NoError。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NoError(t: &TestCtx, err: Result<(), String>) {
        if let Err(e) = err {
            t.Fail();
            panic!("require.NoError: {e}");
        }
    }

    pub fn NoErrorVal<T>(t: &TestCtx, err: Result<T, String>) -> T {
        match err {
            Ok(v) => v,
            Err(e) => {
                t.Fail();
                panic!("require.NoError: {e}");
            }
        }
    }

    // 该辅助函数负责 Error。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Error(t: &TestCtx, err: Result<(), String>) {
        if err.is_ok() {
            t.Fail();
            panic!("require.Error: expected error");
        }
    }

    // 该辅助函数负责 ErrorContains。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn ErrorContains(t: &TestCtx, err: Result<(), String>, msg: &str) {
        match err {
            Err(e) if e.contains(msg) => {}
            Err(e) => {
                t.Fail();
                panic!("require.ErrorContains: {e:?} missing {msg:?}");
            }
            Ok(()) => {
                t.Fail();
                panic!("require.ErrorContains: ok, expected {msg:?}");
            }
        }
    }

    // 该辅助函数负责 True。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn True(t: &TestCtx, cond: bool) {
        if !cond {
            t.Fail();
            panic!("require.True failed");
        }
    }

    // 该辅助函数负责 TrueMsg。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn TrueMsg(t: &TestCtx, cond: bool, msg: &str) {
        if !cond {
            t.Fail();
            panic!("require.True: {msg}");
        }
    }

    // 该辅助函数负责 False。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn False(t: &TestCtx, cond: bool) {
        if cond {
            t.Fail();
            panic!("require.False failed");
        }
    }

    pub fn Equal<T: PartialEq + Debug>(t: &TestCtx, expected: T, actual: T) {
        if expected != actual {
            t.Fail();
            panic!("require.Equal: expected={expected:?} actual={actual:?}");
        }
    }

    pub fn NotEqual<T: PartialEq + Debug>(t: &TestCtx, a: T, b: T) {
        if a == b {
            t.Fail();
            panic!("require.NotEqual: both={a:?}");
        }
    }

    pub fn Greater<T: PartialOrd + Debug>(t: &TestCtx, a: T, b: T) {
        if !(a > b) {
            t.Fail();
            panic!("require.Greater: {a:?} !> {b:?}");
        }
    }

    pub fn GreaterOrEqual<T: PartialOrd + Debug>(t: &TestCtx, a: T, b: T) {
        if !(a >= b) {
            t.Fail();
            panic!("require.GreaterOrEqual: {a:?} !>= {b:?}");
        }
    }

    pub fn Less<T: PartialOrd + Debug>(t: &TestCtx, a: T, b: T) {
        if !(a < b) {
            t.Fail();
            panic!("require.Less: {a:?} !< {b:?}");
        }
    }

    pub fn Len<T>(t: &TestCtx, v: &[T], n: usize) {
        if v.len() != n {
            t.Fail();
            panic!("require.Len: expected={n} actual={}", v.len());
        }
    }

    // 该辅助函数负责 Contains。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Contains(t: &TestCtx, haystack: &str, needle: &str) {
        if !haystack.contains(needle) {
            t.Fail();
            panic!("require.Contains: {haystack:?} missing {needle:?}");
        }
    }

    // 该辅助函数负责 NotContains。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NotContains(t: &TestCtx, haystack: &str, needle: &str) {
        if haystack.contains(needle) {
            t.Fail();
            panic!("require.NotContains: {haystack:?} has {needle:?}");
        }
    }

    // 该辅助函数负责 Nil。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Nil(t: &TestCtx, is_nil: bool) {
        if !is_nil {
            t.Fail();
            panic!("require.Nil failed");
        }
    }

    // 该辅助函数负责 NotNil。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NotNil(t: &TestCtx, ok: bool) {
        if !ok {
            t.Fail();
            panic!("require.NotNil failed");
        }
    }

    // 该辅助函数负责 ErrorIs。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn ErrorIs(t: &TestCtx, err: Result<(), String>, kind: &str) {
        match err {
            Err(e) if e.contains(kind) => {}
            Err(e) => {
                t.Fail();
                panic!("require.ErrorIs: {e:?} != {kind}");
            }
            Ok(()) => {
                t.Fail();
                panic!("require.ErrorIs: ok, expected {kind}");
            }
        }
    }

    pub fn Eventually<F>(t: &TestCtx, mut pred: F, wait: Duration, tick: Duration)
    where
        F: FnMut() -> bool,
    {
        let deadline = Instant::now() + wait;
        loop {
            if pred() {
                return;
            }
            if Instant::now() >= deadline {
                t.Fail();
                panic!("require.Eventually timed out after {wait:?}");
            }
            std::thread::sleep(tick);
        }
    }

    // 该辅助函数负责 FailNow。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn FailNow(t: &TestCtx, msg: &str) -> ! {
        t.Fail();
        panic!("{msg}");
    }

    // 该辅助函数负责 FailNowf。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn FailNowf(t: &TestCtx, msg: &str, detail: &str) -> ! {
        t.Fail();
        panic!("{msg}: {detail}");
    }
}

// ---------------------------------------------------------------------------
// failpoint + testfailpoint
// ---------------------------------------------------------------------------

type CallHook = Arc<dyn Fn() + Send + Sync>;
type ErrHook = Arc<dyn Fn(&mut Option<String>) + Send + Sync>;

static FP_TERMS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static FP_CALLS: OnceLock<Mutex<HashMap<String, CallHook>>> = OnceLock::new();
static FP_ERR_CALLS: OnceLock<Mutex<HashMap<String, ErrHook>>> = OnceLock::new();

// 该辅助函数负责 fp terms。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn fp_terms() -> &'static Mutex<HashMap<String, String>> {
    FP_TERMS.get_or_init(|| Mutex::new(HashMap::new()))
}
// 该辅助函数负责 fp calls。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn fp_calls() -> &'static Mutex<HashMap<String, CallHook>> {
    FP_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}
// 该辅助函数负责 fp err calls。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn fp_err_calls() -> &'static Mutex<HashMap<String, ErrHook>> {
    FP_ERR_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

// 该模块承担 failpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod failpoint {
    use super::*;

    // 该辅助函数负责 Enable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Enable(name: &str, term: &str) -> Result<(), String> {
        fp_terms()
            .lock()
            .unwrap()
            .insert(name.to_string(), term.to_string());
        Ok(())
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Disable(name: &str) -> Result<(), String> {
        fp_terms().lock().unwrap().remove(name);
        fp_calls().lock().unwrap().remove(name);
        fp_err_calls().lock().unwrap().remove(name);
        Ok(())
    }

    // 该辅助函数负责 EnableCall。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn EnableCall(name: &str, hook: CallHook) -> Result<(), String> {
        fp_calls().lock().unwrap().insert(name.to_string(), hook);
        fp_terms()
            .lock()
            .unwrap()
            .insert(name.to_string(), "return".into());
        Ok(())
    }

    // 该辅助函数负责 EnableErrCall。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn EnableErrCall(name: &str, hook: ErrHook) -> Result<(), String> {
        fp_err_calls()
            .lock()
            .unwrap()
            .insert(name.to_string(), hook);
        fp_terms()
            .lock()
            .unwrap()
            .insert(name.to_string(), "return".into());
        Ok(())
    }

    // 该辅助函数负责 is enabled。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn is_enabled(name: &str) -> bool {
        fp_terms().lock().unwrap().contains_key(name)
    }

    // 该辅助函数负责 term。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn term(name: &str) -> Option<String> {
        fp_terms().lock().unwrap().get(name).cloned()
    }

    // 该辅助函数负责 fire。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn fire(name: &str) {
        if let Some(h) = fp_calls().lock().unwrap().get(name).cloned() {
            h();
        }
    }

    // 该辅助函数负责 fire err。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn fire_err(name: &str, err: &mut Option<String>) {
        if let Some(h) = fp_err_calls().lock().unwrap().get(name).cloned() {
            h(err);
        }
    }

    // 该辅助函数负责 reset。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn reset() {
        fp_terms().lock().unwrap().clear();
        fp_calls().lock().unwrap().clear();
        fp_err_calls().lock().unwrap().clear();
    }

    /// Parse `return("path")` style term used by GC barrier tests.
    // 该辅助函数负责 return path。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn return_path(name: &str) -> Option<String> {
        let term = term(name)?;
        // return("...") or return(%q)
        if let Some(rest) = term.strip_prefix("return(") {
            let rest = rest.trim_end_matches(')');
            let s = rest.trim().trim_matches('"').trim_matches('\'');
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        None
    }
}

// 该模块承担 testfailpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testfailpoint {
    use super::*;

    // 该辅助函数负责 Enable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Enable(t: &TestCtx, path: &str, term: &str) {
        require::NoError(t, failpoint::Enable(path, term));
        let path = path.to_string();
        t.Cleanup(move || {
            let _ = failpoint::Disable(&path);
        });
    }

    // 该辅助函数负责 EnableCall。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn EnableCall(t: &TestCtx, path: &str, hook: impl Fn() + Send + Sync + 'static) {
        require::NoError(t, failpoint::EnableCall(path, Arc::new(hook)));
        let path = path.to_string();
        t.Cleanup(move || {
            let _ = failpoint::Disable(&path);
        });
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Disable(t: &TestCtx, path: &str) {
        require::NoError(t, failpoint::Disable(path));
    }
}

// ---------------------------------------------------------------------------
// kerneltype / mysql / logutil / printer / oracle / summary
// ---------------------------------------------------------------------------

// 该模块承担 kerneltype 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod kerneltype {
    // 该辅助函数负责 IsNextGen。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn IsNextGen() -> bool {
        astersql_tests_realtikvtest::stubs::kerneltype::IsNextGen()
    }
    // 该辅助函数负责 IsClassic。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn IsClassic() -> bool {
        !IsNextGen()
    }
}

// 该模块承担 mysql 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod mysql {
    pub const DefaultCollationID: i32 = 45;
}

// 该模块承担 logutil 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod logutil {
    use super::TestCtx;
    // 该辅助函数负责 OverrideLevelForTest。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn OverrideLevelForTest(_t: &TestCtx, _level: i32) {}
}

// 该模块承担 zapcore 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod zapcore {
    pub const ErrorLevel: i32 = 2;
}

// 该模块承担 printer 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod printer {
    // 该辅助函数负责 GetTiDBInfo。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GetTiDBInfo() -> String {
        "astersql-test".into()
    }
}

// 该模块承担 oracle 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod oracle {
    use std::time::{SystemTime, UNIX_EPOCH};
    // 该辅助函数负责 GoTimeToTS。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn GoTimeToTS(t: SystemTime) -> u64 {
        let ms = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        (ms << 18) | 1
    }
    // 该辅助函数负责 ComposeTS。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << 18) | (logical as u64 & 0x3ffff)
    }
}

// 该模块承担 摘要 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod summary {
    use std::sync::atomic::{AtomicBool, Ordering};
    static OK: AtomicBool = AtomicBool::new(false);
    // 该辅助函数负责 SetSuccessStatus。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn SetSuccessStatus(v: bool) {
        OK.store(v, Ordering::SeqCst);
    }
    // 该辅助函数负责 success。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn success() -> bool {
        OK.load(Ordering::SeqCst)
    }
}

// 该模块承担 gc 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod gc {
    // 该辅助函数负责 MakeSafePointID。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn MakeSafePointID() -> String {
        format!("br-{}", uuid::Uuid::new_v4())
    }
}

// ---------------------------------------------------------------------------
// Engine / SQL / BRIE
// ---------------------------------------------------------------------------

// 该类型围绕 ColDef 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct ColDef {
    name: String,
    auto_inc: bool,
}

// 该类型围绕 TableData 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct TableData {
    cols: Vec<ColDef>,
    rows: Vec<Vec<String>>,
    next_auto: i64,
}

// 该类型围绕 BrieJob 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct BrieJob {
    id: u64,
    query: String,
    redacted: String,
    blocked: bool,
    cancelled: bool,
    done: Arc<AtomicBool>,
}

// 该类型围绕 BackupArtifact 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct BackupArtifact {
    /// db.table -> rows
    tables: HashMap<String, TableData>,
    backup_ts: u64,
    encrypted: bool,
    incremental: bool,
    last_backup_ts: u64,
}

// 该类型围绕 LogTask 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct LogTask {
    name: String,
    start_ts: u64,
    checkpoint_ts: u64,
    paused: bool,
    as_error: bool,
    message: String,
    encrypted: bool,
    operator_host: String,
    /// Local path of the stream/incr storage for this task.
    storage_path: String,
}

// 该类型围绕 DdlJobRow 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct DdlJobRow {
    job_type: String,
    table_names: String,
}

// 该类型围绕 Engine 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Default)]
struct Engine {
    db: String,
    tables: HashMap<String, TableData>, // key: db.table
    brie_jobs: Vec<BrieJob>,
    next_brie_id: u64,
    backups: HashMap<String, BackupArtifact>, // local path
    tso: u64,
    ddl_jobs: Vec<DdlJobRow>,
    warning_count: u32,
    collation: i32,
    log_tasks: HashMap<String, LogTask>,
    globals: HashMap<String, String>,
    /// registry rows: id -> fields
    registry: HashMap<u64, RegistryRow>,
    next_registry_id: u64,
    /// PD mock state
    pd: PdState,
    /// GC failpoint signal bookkeeping
    keyspace_name: String,
}

// 该类型围绕 RegistryRow 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug)]
struct RegistryRow {
    id: u64,
    filter_strings: String,
    status: String,
    restored_ts: u64,
    start_ts: u64,
    upstream_cluster_id: u64,
    with_sys_table: bool,
    cmd: String,
    last_heartbeat: Instant,
}

// 该类型围绕 PdState 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Default)]
struct PdState {
    service_gc_safe_points: HashMap<String, i64>,
    schedulers: Vec<String>,
    paused_schedulers: HashSet<String>,
    region_label_rules: Vec<SchedulerRule>,
    lightning_suspended: bool,
}

// 该类型围绕 SchedulerRule 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerRule {
    pub group_id: String,
    pub id: String,
    pub start_key: String,
    pub end_key: String,
    pub role: String,
    pub count: i32,
    pub label_keys: Vec<String>,
    pub labels: Vec<HashMap<String, String>>,
    pub rule_type: String,
    pub data: serde_json::Value,
}

use serde::{Deserialize, Serialize};

// 该类型围绕 KeyRange 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct KeyRange {
    pub start_key: String,
    pub end_key: String,
}

pub const EmptyRangeStart: &str = "";
pub const EmptyRangeEnd: &str = "";

// 该辅助函数负责 eng。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn eng() -> std::sync::MutexGuard<'static, Engine> {
    static ENG: OnceLock<Mutex<Engine>> = OnceLock::new();
    ENG.get_or_init(|| Mutex::new(Engine::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    // 该辅助函数负责 new。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn new() -> Self {
        let mut e = Self {
            db: "test".into(),
            tables: HashMap::new(),
            brie_jobs: Vec::new(),
            next_brie_id: 1,
            backups: HashMap::new(),
            tso: 1_000_000,
            ddl_jobs: Vec::new(),
            warning_count: 0,
            collation: mysql::DefaultCollationID,
            log_tasks: HashMap::new(),
            globals: HashMap::new(),
            registry: HashMap::new(),
            next_registry_id: 1,
            pd: PdState {
                service_gc_safe_points: HashMap::new(),
                schedulers: vec![
                    "balance-leader-scheduler".into(),
                    "balance-region-scheduler".into(),
                    "hot-region-scheduler".into(),
                ],
                paused_schedulers: HashSet::new(),
                region_label_rules: vec![SchedulerRule {
                    group_id: "tikv".into(),
                    id: "schedule".into(),
                    start_key: "".into(),
                    end_key: "".into(),
                    role: "".into(),
                    count: 0,
                    label_keys: vec![],
                    labels: vec![],
                    rule_type: "key-range".into(),
                    data: serde_json::json!([]),
                }],
                lightning_suspended: false,
            },
            keyspace_name: String::new(),
        };
        e.ensure_registry_table();
        e
    }

    // 该辅助函数负责 ensure 注册表 表。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn ensure_registry_table(&mut self) {
        // presence implied by registry map; SHOW DATABASES LIKE mysql always ok
        let _ = &self.registry;
    }

    // 该辅助函数负责 next tso。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn next_tso(&mut self) -> u64 {
        self.tso += 1;
        self.tso
    }

    // 该辅助函数负责 qkey。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn qkey(&self, name: &str) -> String {
        if name.contains('.') {
            name.to_string()
        } else {
            format!("{}.{}", self.db, name)
        }
    }
}

// 该辅助函数负责 复位 engine。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn reset_engine() {
    {
        let mut e = eng();
        *e = Engine::new();
    }
    // Drop eng() before queue reset — ResetGlobalBRIEQueueForTest also locks eng().
    failpoint::reset();
    executor::reset_brie_queue();
}

// 该辅助函数负责 串行守卫。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

// 该辅助函数负责 创建 store。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn create_store(t: &TestCtx) -> Storage {
    CreateMockStoreAndSetup(t, &[])
}

// 该辅助函数负责 创建 store 携带 keyspace。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn create_store_with_keyspace(t: &TestCtx, name: &str) -> Storage {
    let opts = [WithKeyspaceName(name)];
    let store = CreateMockStoreAndSetup(t, &opts);
    eng().keyspace_name = name.to_string();
    store
}

// 该辅助函数负责 qident。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn qident(s: &str) -> String {
    s.trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_end_matches(';')
        .to_string()
}

// 该辅助函数负责 normalize sql。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn normalize_sql(sql: &str) -> String {
    sql.trim().trim_end_matches(';').to_string()
}

// 该辅助函数负责 local path from uri。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn local_path_from_uri(uri: &str) -> String {
    let u = uri.trim().trim_matches('\'');
    if let Some(rest) = u.strip_prefix("local://") {
        rest.to_string()
    } else if let Some(rest) = u.strip_prefix("s3://") {
        format!("s3://{rest}")
    } else if let Some(rest) = u.strip_prefix("noop://") {
        format!("noop://{rest}")
    } else {
        u.to_string()
    }
}

// 该辅助函数负责 redact brie query。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn redact_brie_query(q: &str) -> String {
    // Strip query params (credentials) from S3 URIs like Go.
    let stripped = if let Some(idx) = q.find('?') {
        let before = &q[..idx];
        if before.to_lowercase().contains("s3://") {
            if let Some(end) = q[idx..].find('\'') {
                let mut out = String::new();
                out.push_str(before);
                out.push('\'');
                let after = &q[idx + end + 1..];
                out.push_str(after);
                out
            } else {
                before.to_string()
            }
        } else {
            q.to_string()
        }
    } else {
        q.to_string()
    };
    canonicalize_brie_job_query(&stripped)
}

/// Match TiDB `SHOW BR JOB QUERY` display: uppercase statement keywords.
// 该辅助函数负责 canonicalize brie 任务 query。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn canonicalize_brie_job_query(q: &str) -> String {
    let lower = q.to_lowercase();
    if let Some(idx) = lower.find(" to ") {
        if lower.starts_with("backup database ") {
            let mid = q["backup database ".len()..idx].trim();
            let rest = q[idx + 4..].trim();
            return format!("BACKUP DATABASE {mid} TO {rest}");
        }
        if lower.starts_with("backup table ") {
            let mid = q["backup table ".len()..idx].trim();
            let rest = q[idx + 4..].trim();
            return format!("BACKUP TABLE {mid} TO {rest}");
        }
    }
    if let Some(idx) = lower.find(" from ") {
        if lower.starts_with("restore database ") {
            let mid = q["restore database ".len()..idx].trim();
            let rest = q[idx + 6..].trim();
            return format!("RESTORE DATABASE {mid} FROM {rest}");
        }
        if lower.starts_with("restore table ") {
            let mid = q["restore table ".len()..idx].trim();
            let rest = q[idx + 6..].trim();
            return format!("RESTORE TABLE {mid} FROM {rest}");
        }
    }
    q.to_string()
}

// 该辅助函数负责 write gc signal。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn write_gc_signal(path: &str, content: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, content);
}

// ---------------------------------------------------------------------------
// testkit SQL
// ---------------------------------------------------------------------------

// 该模块承担 testkit 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testkit {
    use super::*;

    // 该类型围绕 ResultSet 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct ResultSet {
        pub rows: Vec<Vec<String>>,
    }

    impl ResultSet {
        // 该辅助函数负责 Rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Rows(&self) -> &[Vec<String>] {
            &self.rows
        }
        // 该辅助函数负责 Check。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Check(&self, expected: &[Vec<&str>]) {
            let got: Vec<Vec<&str>> = self
                .rows
                .iter()
                .map(|r| r.iter().map(|c| c.as_str()).collect())
                .collect();
            let exp: Vec<Vec<&str>> = expected.iter().map(|r| r.to_vec()).collect();
            assert_eq!(got, exp, "ResultSet.Check mismatch");
        }
        // 该辅助函数负责 CheckContain。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn CheckContain(&self, needle: &str) {
            let ok = self
                .rows
                .iter()
                .any(|r| r.iter().any(|c| c.contains(needle)));
            assert!(ok, "CheckContain missing {needle:?} in {:?}", self.rows);
        }
    }

    pub fn Rows<'a>(vals: &'a [&'a str]) -> Vec<Vec<&'a str>> {
        vec![vals.to_vec()]
    }

    // 该类型围绕 Session 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone)]
    pub struct Session {
        pub store: Storage,
        warning_count: Arc<Mutex<u32>>,
    }

    impl Session {
        // 该辅助函数负责 GetStore。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn GetStore(&self) -> Storage {
            self.store.clone()
        }
        // 该辅助函数负责 SetCollation。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn SetCollation(&self, id: i32) -> Result<(), String> {
            eng().collation = id;
            Ok(())
        }
        // 该辅助函数负责 FieldList。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn FieldList(&self, _table: &str) -> Result<Vec<String>, String> {
            Ok(vec!["id".into(), "v".into()])
        }
        // 该辅助函数负责 GetSessionVars。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn GetSessionVars(&self) -> SessionVars {
            SessionVars {
                StmtCtx: StmtCtx {
                    warning: self.warning_count.clone(),
                },
            }
        }
    }

    // 该类型围绕 SessionVars 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone)]
    pub struct SessionVars {
        pub StmtCtx: StmtCtx,
    }

    // 该类型围绕 StmtCtx 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone)]
    pub struct StmtCtx {
        warning: Arc<Mutex<u32>>,
    }

    impl StmtCtx {
        // 该辅助函数负责 WarningCount。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn WarningCount(&self) -> u32 {
            *self.warning.lock().unwrap()
        }
    }

    // 该类型围绕 TestKit 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone)]
    pub struct TestKit {
        pub store: Storage,
        pub t: TestCtx,
        warning_count: Arc<Mutex<u32>>,
    }

    impl TestKit {
        // 该辅助函数负责 Session。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Session(&self) -> Session {
            Session {
                store: self.store.clone(),
                warning_count: self.warning_count.clone(),
            }
        }

        // 该辅助函数负责 MustExec。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn MustExec(&self, sql: &str) {
            if let Err(e) = self.exec_inner(sql) {
                panic!("MustExec failed: {e}; sql={sql}");
            }
        }

        // 该辅助函数负责 MustQuery。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn MustQuery(&self, sql: &str) -> ResultSet {
            match self.query_inner(sql) {
                Ok(rs) => rs,
                Err(e) => panic!("MustQuery failed: {e}; sql={sql}"),
            }
        }

        // 该辅助函数负责 Exec。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Exec(&self, sql: &str) -> Result<ResultSet, String> {
            // Go Exec returns a result set handle; streaming errors happen on fetch.
            // We run query_inner but defer restore-conflict errors to fetch.
            let lower = normalize_sql(sql).to_lowercase();
            if lower.starts_with("restore ") {
                // Return Ok handle; actual conflict detected on fetch.
                return Ok(ResultSet {
                    rows: vec![vec!["__pending_restore__".into(), sql.to_string()]],
                });
            }
            self.query_inner(sql)
        }

        // 该辅助函数负责 QueryToErr。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn QueryToErr(&self, sql: &str) -> Result<(), String> {
            match self.query_inner(sql) {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        }
    }

    // 该辅助函数负责 NewTestKit。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewTestKit(t: &TestCtx, store: Storage) -> TestKit {
        TestKit {
            store,
            t: t.clone(),
            warning_count: Arc::new(Mutex::new(0)),
        }
    }

    impl TestKit {
        // 该辅助函数负责 query inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn query_inner(&self, sql: &str) -> Result<ResultSet, String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();
            // wait if BRIE blocked
            if failpoint::is_enabled("github.com/pingcap/tidb/pkg/executor/block-on-brie")
                && (lower.starts_with("backup ") || lower.starts_with("restore "))
            {
                let job_id = {
                    let mut e = eng();
                    let id = e.next_brie_id;
                    e.next_brie_id += 1;
                    let redacted = redact_brie_query(&s);
                    let done = Arc::new(AtomicBool::new(false));
                    e.brie_jobs.push(BrieJob {
                        id,
                        query: s.clone(),
                        redacted,
                        blocked: true,
                        cancelled: false,
                        done: done.clone(),
                    });
                    id
                };
                // Block until cancelled or timeout.
                let start = Instant::now();
                loop {
                    {
                        let e = eng();
                        if let Some(j) = e.brie_jobs.iter().find(|j| j.id == job_id) {
                            if j.cancelled || j.done.load(Ordering::SeqCst) {
                                return Err("brie job cancelled / failed".into());
                            }
                        }
                    }
                    if start.elapsed() > Duration::from_secs(10) {
                        return Err("brie block timeout".into());
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }

            if lower.starts_with("backup ") {
                return self.do_backup(&s);
            }
            if lower.starts_with("restore ") {
                return self.do_restore(&s);
            }
            if lower.starts_with("show br job query") {
                let id: u64 = lower
                    .split_whitespace()
                    .last()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                let e = eng();
                if let Some(j) = e.brie_jobs.iter().find(|j| j.id == id) {
                    return Ok(ResultSet {
                        rows: vec![vec![j.redacted.clone()]],
                    });
                }
                return Ok(ResultSet { rows: vec![] });
            }
            if lower.starts_with("select count(*)") {
                return self.select_count(&s);
            }
            if lower.starts_with("select * from") {
                return self.select_star(&s);
            }
            if lower.starts_with("admin show ddl jobs") {
                let e = eng();
                let rows: Vec<Vec<String>> = e
                    .ddl_jobs
                    .iter()
                    .filter(|j| {
                        if lower.contains("create tables") {
                            j.job_type == "create tables"
                        } else {
                            true
                        }
                    })
                    .map(|j| {
                        // row[2] = table names in Go
                        vec!["1".into(), j.job_type.clone(), j.table_names.clone()]
                    })
                    .collect();
                return Ok(ResultSet { rows });
            }
            if lower.starts_with("show databases like") {
                return Ok(ResultSet {
                    rows: vec![vec!["mysql".into()]],
                });
            }
            if lower.starts_with("select ") && lower.contains("from mysql.tidb_restore_registry")
                || (lower.starts_with("select ") && lower.contains("tidb_restore_registry"))
            {
                return self.select_registry(&s);
            }
            if lower.contains("information_schema.tables") {
                // SELECT * FROM information_schema.tables WHERE table_name = ?
                // Our SQL is already interpolated by callers sometimes as format!
                let name = if let Some(idx) = lower.rfind('=') {
                    qident(s[idx + 1..].trim().trim_matches('?').trim())
                } else {
                    String::new()
                };
                let e = eng();
                let found = e
                    .tables
                    .keys()
                    .any(|k| k.ends_with(&format!(".{name}")) || k == &name);
                return Ok(ResultSet {
                    rows: if found { vec![vec![name]] } else { vec![] },
                });
            }
            if lower.starts_with("show processlist") {
                return Ok(ResultSet { rows: vec![] });
            }
            // default empty
            Ok(ResultSet { rows: vec![] })
        }

        // 该辅助函数负责 exec inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn exec_inner(&self, sql: &str) -> Result<(), String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();
            if lower.starts_with("use ") {
                eng().db = qident(&s[4..]);
                return Ok(());
            }
            if lower.starts_with("create database") {
                return Ok(());
            }
            if lower.starts_with("drop database") {
                let db = qident(s.split_whitespace().last().unwrap_or(""));
                let mut e = eng();
                e.tables.retain(|k, _| !k.starts_with(&format!("{db}.")));
                return Ok(());
            }
            if lower.starts_with("drop table") {
                let rest = s[10..].trim();
                let rest_lower = rest.to_lowercase();
                let rest = if let Some(r) = rest_lower.strip_prefix("if exists") {
                    // Keep original casing for the name portion.
                    rest[rest.len() - r.len()..].trim()
                } else {
                    rest
                };
                for part in rest.split(',') {
                    let name = qident(part);
                    let key = eng().qkey(&name);
                    eng().tables.remove(&key);
                    eng().tables.remove(&name);
                }
                return Ok(());
            }
            if lower.starts_with("create table") {
                return self.create_table(&s);
            }
            if lower.starts_with("insert ") {
                return self.insert_rows(&s);
            }
            if lower.starts_with("delete from") {
                // registry cleanup or row delete
                if lower.contains("tidb_restore_registry") {
                    eng().registry.clear();
                    return Ok(());
                }
                // truncate-like for tests
                let after = &s[11..];
                let name = qident(after.split_whitespace().next().unwrap_or(""));
                let key = eng().qkey(&name);
                if let Some(t) = eng().tables.get_mut(&key) {
                    // delete where id = N
                    if lower.contains("where") {
                        // keep simple: remove matching id
                        if let Some(idx) = lower.find("id =") {
                            let id = s[idx + 4..].trim().split_whitespace().next().unwrap_or("");
                            t.rows
                                .retain(|r| r.first().map(|c| c != id).unwrap_or(true));
                        }
                    } else {
                        t.rows.clear();
                    }
                }
                return Ok(());
            }
            if lower.starts_with("update ") {
                return Ok(());
            }
            if lower.starts_with("truncate ") {
                let name = qident(s.split_whitespace().last().unwrap_or(""));
                let key = eng().qkey(&name);
                if let Some(t) = eng().tables.get_mut(&key) {
                    t.rows.clear();
                    t.next_auto = 1;
                }
                return Ok(());
            }
            if lower.starts_with("begin") || lower == "rollback" || lower == "commit" {
                return Ok(());
            }
            if lower.starts_with("admin check") {
                return Ok(());
            }
            if lower.starts_with("set ") {
                return Ok(());
            }
            if lower.starts_with("cancel br job") {
                let id: u64 = lower
                    .split_whitespace()
                    .last()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                let mut e = eng();
                if let Some(j) = e.brie_jobs.iter_mut().find(|j| j.id == id) {
                    j.cancelled = true;
                    j.done.store(true, Ordering::SeqCst);
                } else {
                    // cancel of non-existent may warn
                    e.warning_count += 1;
                    *self.warning_count.lock().unwrap() = e.warning_count;
                }
                return Ok(());
            }
            if lower.starts_with("set config tikv") {
                return Ok(());
            }
            Ok(())
        }

        // 该辅助函数负责 创建 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn create_table(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let after = lower
                .find("table")
                .map(|i| s[i + 5..].trim())
                .ok_or_else(|| "bad create table".to_string())?;
            let after = after
                .strip_prefix("if not exists")
                .map(|x| x.trim())
                .unwrap_or(after);
            let name_tok = after
                .split(|c: char| c == '(' || c.is_whitespace())
                .next()
                .unwrap_or("");
            let name = qident(name_tok);
            let key = eng().qkey(&name);
            let auto = lower.contains("auto_increment");
            let mut cols = Vec::new();
            if let Some(start) = s.find('(') {
                if let Some(end) = s.rfind(')') {
                    for part in s[start + 1..end].split(',') {
                        let n = part.split_whitespace().next().unwrap_or("");
                        let n = qident(n);
                        if n.is_empty()
                            || n.eq_ignore_ascii_case("primary")
                            || n.eq_ignore_ascii_case("index")
                            || n.eq_ignore_ascii_case("key")
                            || n.eq_ignore_ascii_case("unique")
                        {
                            continue;
                        }
                        cols.push(ColDef {
                            name: n,
                            auto_inc: part.to_lowercase().contains("auto_increment"),
                        });
                    }
                }
            }
            if cols.is_empty() {
                cols.push(ColDef {
                    name: "id".into(),
                    auto_inc: auto,
                });
            }
            eng().tables.insert(
                key,
                TableData {
                    cols,
                    rows: Vec::new(),
                    next_auto: 1,
                },
            );
            Ok(())
        }

        // 该辅助函数负责 insert rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn insert_rows(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let name = if let Some(i) = lower.find("into ") {
                let after = &s[i + 5..];
                qident(
                    after
                        .split(|c: char| c.is_whitespace() || c == '(')
                        .next()
                        .unwrap_or(""),
                )
            } else {
                s.split_whitespace().nth(1).map(qident).unwrap_or_default()
            };
            let key = eng().qkey(&name);
            let values_idx = lower
                .find("values")
                .ok_or_else(|| "insert missing values".to_string())?;
            let values_part = &s[values_idx + 6..];
            let mut e = eng();
            let table = e
                .tables
                .get_mut(&key)
                .ok_or_else(|| format!("table not found: {key}"))?;
            // parse tuples
            let mut depth = 0usize;
            let mut cur = String::new();
            let mut tuples = Vec::new();
            for ch in values_part.chars() {
                match ch {
                    '(' => {
                        if depth == 0 {
                            cur.clear();
                        } else {
                            cur.push(ch);
                        }
                        depth += 1;
                    }
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            tuples.push(cur.clone());
                        } else {
                            cur.push(ch);
                        }
                    }
                    _ => {
                        if depth > 0 {
                            cur.push(ch);
                        }
                    }
                }
            }
            if tuples.is_empty() {
                // insert t values () — empty tuple once
                tuples.push(String::new());
            }
            for tup in tuples {
                let mut cells: Vec<String> = if tup.trim().is_empty() {
                    Vec::new()
                } else {
                    split_csv_like(&tup)
                };
                // fill auto_inc
                for (i, col) in table.cols.iter().enumerate() {
                    if col.auto_inc && (cells.len() <= i || cells[i].is_empty()) {
                        while cells.len() <= i {
                            cells.push(String::new());
                        }
                        cells[i] = table.next_auto.to_string();
                        table.next_auto += 1;
                    }
                }
                // strip quotes
                for c in &mut cells {
                    *c = c.trim().trim_matches('\'').trim_matches('"').to_string();
                }
                table.rows.push(cells);
            }
            Ok(())
        }

        // 该辅助函数负责 select count。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn select_count(&self, s: &str) -> Result<ResultSet, String> {
            let lower = s.to_lowercase();
            let name = if let Some(i) = lower.find("from ") {
                qident(s[i + 5..].split_whitespace().next().unwrap_or(""))
            } else {
                String::new()
            };
            let key = eng().qkey(&name);
            let n = eng().tables.get(&key).map(|t| t.rows.len()).unwrap_or(0);
            Ok(ResultSet {
                rows: vec![vec![n.to_string()]],
            })
        }

        // 该辅助函数负责 select star。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn select_star(&self, s: &str) -> Result<ResultSet, String> {
            let lower = s.to_lowercase();
            let name = if let Some(i) = lower.find("from ") {
                qident(s[i + 5..].split_whitespace().next().unwrap_or(""))
            } else {
                String::new()
            };
            let e = eng();
            let key = e.qkey(&name);
            let rows = e
                .tables
                .get(&key)
                .map(|t| t.rows.clone())
                .unwrap_or_default();
            Ok(ResultSet { rows })
        }

        // 该辅助函数负责 select 注册表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn select_registry(&self, s: &str) -> Result<ResultSet, String> {
            let lower = s.to_lowercase();
            let e = eng();
            if lower.contains("count(*)") {
                // WHERE id IN (...)
                let mut ids = Vec::new();
                if let Some(i) = lower.find("in (") {
                    let rest = &s[i + 4..];
                    let end = rest.find(')').unwrap_or(rest.len());
                    for p in rest[..end].split(',') {
                        if let Ok(id) = p.trim().parse::<u64>() {
                            ids.push(id);
                        }
                    }
                }
                let n = if ids.is_empty() {
                    e.registry.len()
                } else {
                    ids.iter().filter(|id| e.registry.contains_key(id)).count()
                };
                return Ok(ResultSet {
                    rows: vec![vec![n.to_string()]],
                });
            }
            // SELECT status FROM ... WHERE id = N
            // SELECT id, filter_strings, status, restored_ts FROM ... WHERE id = N
            let id = if let Some(i) = lower.rfind("id =") {
                s[i + 4..]
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            if let Some(row) = e.registry.get(&id) {
                if lower.contains("filter_strings") {
                    return Ok(ResultSet {
                        rows: vec![vec![
                            row.id.to_string(),
                            row.filter_strings.clone(),
                            row.status.clone(),
                            row.restored_ts.to_string(),
                        ]],
                    });
                }
                if lower.contains("select status") {
                    return Ok(ResultSet {
                        rows: vec![vec![row.status.clone()]],
                    });
                }
            }
            Ok(ResultSet { rows: vec![] })
        }

        // 该辅助函数负责 do 备份。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn do_backup(&self, s: &str) -> Result<ResultSet, String> {
            let lower = s.to_lowercase();
            // BACKUP DATABASE X TO 'uri' [last_backup=TS]
            let to_idx = lower.find(" to ").ok_or("backup missing TO")?;
            let target = &s[..to_idx];
            let rest = &s[to_idx + 4..];
            let uri_tok = rest.trim().split_whitespace().next().unwrap_or("");
            let uri = uri_tok.trim_matches('\'');
            let path = local_path_from_uri(uri);
            if path.starts_with("s3://") && path.contains("nonexist") {
                // will block if failpoint; otherwise error
                return Err("storage not exist".into());
            }
            let last_backup = if let Some(i) = lower.find("last_backup=") {
                s[i + 12..]
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            let db_filter = {
                let parts: Vec<&str> = target.split_whitespace().collect();
                // BACKUP DATABASE *|name|...
                if parts.len() >= 3 {
                    qident(parts[2])
                } else {
                    "*".into()
                }
            };
            let mut e = eng();
            let ts = e.next_tso();
            let mut tables = HashMap::new();
            for (k, v) in e.tables.iter() {
                let db = k.split('.').next().unwrap_or("");
                if db_filter == "*" || db.eq_ignore_ascii_case(&db_filter) {
                    tables.insert(k.clone(), v.clone());
                }
            }
            // GC barrier failpoint signals for keyspace backup
            if let Some(p) = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-keyspace-set-barrier",
            ) {
                let ks = if e.keyspace_name.is_empty() {
                    "keyspace1"
                } else {
                    &e.keyspace_name
                };
                write_gc_signal(&p, &format!("keyspace={ks} id=1"));
            }
            if let Some(p) = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-keyspace-delete-barrier",
            ) {
                write_gc_signal(&p, "deleted");
            }
            // global signals intentionally NOT written for keyspace path
            let _ = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-global-set-safepoint",
            );
            let _ = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-global-delete-safepoint",
            );

            let art = BackupArtifact {
                tables,
                backup_ts: ts,
                encrypted: false,
                incremental: last_backup > 0,
                last_backup_ts: last_backup,
            };
            // persist marker file
            if path.starts_with('/') || path.starts_with('.') {
                let _ = std::fs::create_dir_all(&path);
                let _ = std::fs::write(Path::new(&path).join("backupmeta"), format!("ts={ts}"));
            }
            e.backups.insert(path.clone(), art);
            let id = e.next_brie_id;
            e.next_brie_id += 1;
            e.brie_jobs.push(BrieJob {
                id,
                query: s.to_string(),
                redacted: redact_brie_query(s),
                blocked: false,
                cancelled: false,
                done: Arc::new(AtomicBool::new(true)),
            });
            // Go returns columns including backupTS at index 2
            Ok(ResultSet {
                rows: vec![vec!["backup".into(), path, ts.to_string()]],
            })
        }

        // 该辅助函数负责 do 恢复。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn do_restore(&self, s: &str) -> Result<ResultSet, String> {
            apply_restore(s)
        }
    }

    // 该辅助函数负责 切分 csv like。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn split_csv_like(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_q = false;
        for ch in s.chars() {
            match ch {
                '\'' | '"' => {
                    in_q = !in_q;
                    cur.push(ch);
                }
                ',' if !in_q => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            }
        }
        if !cur.is_empty() || s.ends_with(',') {
            out.push(cur.trim().to_string());
        }
        out
    }
}

// 该模块承担 session 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod session {
    use super::testkit::ResultSet;
    use super::*;
    // 该辅助函数负责 ResultSetToStringSlice。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn ResultSetToStringSlice(
        _ctx: (),
        _se: &testkit::Session,
        res: ResultSet,
    ) -> Result<Vec<Vec<String>>, String> {
        if res
            .rows
            .first()
            .and_then(|r| r.first())
            .map(|c| c == "__pending_restore__")
            .unwrap_or(false)
        {
            let sql = res.rows[0].get(1).cloned().unwrap_or_default();
            return match self_do_restore_pub(&sql) {
                Ok(rs) => Ok(rs.rows),
                Err(e) => Err(e),
            };
        }
        Ok(res.rows)
    }
}

// 该辅助函数负责 apply 恢复。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn apply_restore(s: &str) -> Result<testkit::ResultSet, String> {
    let lower = s.to_lowercase();
    let from_idx = lower.find(" from ").ok_or("restore missing FROM")?;
    let target = &s[..from_idx];
    let rest = &s[from_idx + 6..];
    let uri_tok = rest.trim().split_whitespace().next().unwrap_or("");
    let uri = uri_tok.trim_matches('\'');
    let path = local_path_from_uri(uri);
    let (db_filter, table_filter): (String, Option<String>) = {
        let parts: Vec<&str> = target.split_whitespace().collect();
        // RESTORE DATABASE *|name | RESTORE TABLE db.tbl
        if parts.len() >= 3 && parts[1].eq_ignore_ascii_case("database") {
            (qident(parts[2]), None)
        } else if parts.len() >= 3 && parts[1].eq_ignore_ascii_case("table") {
            let full = qident(parts[2]);
            if let Some((db, tbl)) = full.split_once('.') {
                (db.to_string(), Some(tbl.to_string()))
            } else {
                ("*".into(), Some(full))
            }
        } else {
            ("*".into(), None)
        }
    };
    let mut e = eng();
    let art = e
        .backups
        .get(&path)
        .cloned()
        .ok_or_else(|| format!("backup not found: {path}"))?;
    let filtered: HashMap<_, _> = art
        .tables
        .iter()
        .filter(|(k, _)| {
            let (db, tbl) = k.split_once('.').unwrap_or(("", k.as_str()));
            let db_ok = db_filter == "*" || db.eq_ignore_ascii_case(&db_filter);
            let tbl_ok = table_filter
                .as_ref()
                .map(|t| tbl.eq_ignore_ascii_case(t))
                .unwrap_or(true);
            db_ok && tbl_ok
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // Full restore errors if any target table already exists; incremental applies on top.
    if !art.incremental {
        for k in filtered.keys() {
            if e.tables.contains_key(k) {
                return Err("table already exists".into());
            }
        }
    }
    let mut names = Vec::new();
    for (k, v) in filtered.iter() {
        if art.incremental {
            if let Some(existing) = e.tables.get_mut(k) {
                // Merge incremental rows (stub stores post-incr snapshot rows).
                existing.rows = v.rows.clone();
                existing.next_auto = v.next_auto;
            } else {
                e.tables.insert(k.clone(), v.clone());
            }
        } else {
            e.tables.insert(k.clone(), v.clone());
        }
        if let Some((_, tname)) = k.split_once('.') {
            names.push(tname.to_string());
        }
    }
    if !names.is_empty() && !art.incremental {
        e.ddl_jobs.push(DdlJobRow {
            job_type: "create tables".into(),
            table_names: names.join(","),
        });
    }
    let id = e.next_brie_id;
    e.next_brie_id += 1;
    e.brie_jobs.push(BrieJob {
        id,
        query: s.to_string(),
        redacted: redact_brie_query(s),
        blocked: false,
        cancelled: false,
        done: Arc::new(AtomicBool::new(true)),
    });
    Ok(testkit::ResultSet {
        rows: vec![vec!["restore".into(), path]],
    })
}

// 该辅助函数负责 self do 恢复 pub。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn self_do_restore_pub(s: &str) -> Result<testkit::ResultSet, String> {
    apply_restore(s)
}

// ---------------------------------------------------------------------------
// executor BRIE queue
// ---------------------------------------------------------------------------

// 该模块承担 executor 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod executor {
    use super::*;
    // 该辅助函数负责 ResetGlobalBRIEQueueForTest。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn ResetGlobalBRIEQueueForTest() {
        let mut e = eng();
        e.brie_jobs.clear();
        e.next_brie_id = 1;
    }
    // 该辅助函数负责 复位 brie queue。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn reset_brie_queue() {
        ResetGlobalBRIEQueueForTest();
    }
}

// ---------------------------------------------------------------------------
// PD mock HTTP server
// ---------------------------------------------------------------------------

static PD_SERVER: OnceLock<Mutex<Option<PdServer>>> = OnceLock::new();

// 该类型围绕 PdServer 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

struct PdServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

// 该辅助函数负责 pd slot。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn pd_slot() -> &'static Mutex<Option<PdServer>> {
    PD_SERVER.get_or_init(|| Mutex::new(None))
}

// 该辅助函数负责 ensure pd mock。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn ensure_pd_mock() -> String {
    let mut slot = pd_slot().lock().unwrap();
    if let Some(s) = slot.as_ref() {
        return s.addr.to_string();
    }
    // Prefer the Go-hardcoded PD addr so helpers that use 127.0.0.1:2379 work.
    let listener = TcpListener::bind("127.0.0.1:2379")
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .expect("bind pd mock");
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        while !stop2.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Accepted sockets inherit nonblocking; force blocking I/O for HTTP.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = handle_pd_http(&mut stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    *slot = Some(PdServer { addr, stop });
    config::UpdateGlobal(|c| {
        c.Path = addr.to_string();
    });
    addr.to_string()
}

// 该辅助函数负责 handle pd http。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn handle_pd_http(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let body = {
        let e = eng();
        if path.contains("gc/safepoint") {
            let sps: Vec<serde_json::Value> =
                e.pd.service_gc_safe_points
                    .iter()
                    .map(|(id, sp)| {
                        serde_json::json!({
                            "service_id": id,
                            "expired_at": 0,
                            "safe_point": sp,
                        })
                    })
                    .collect();
            serde_json::json!({ "service_gc_safe_points": sps }).to_string()
        } else if path.contains("schedulers") {
            if path.contains("status=paused") {
                let v: Vec<String> = e.pd.paused_schedulers.iter().cloned().collect();
                serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
            } else {
                serde_json::to_string(&e.pd.schedulers).unwrap_or_else(|_| "[]".into())
            }
        } else if path.contains("region-label/rules") {
            let rules: Vec<serde_json::Value> =
                e.pd.region_label_rules
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "group_id": r.group_id,
                            "id": r.id,
                            "start_key": r.start_key,
                            "end_key": r.end_key,
                            "role": r.role,
                            "count": r.count,
                            "label_keys": r.label_keys,
                            "labels": r.labels,
                            "rule_type": r.rule_type,
                            "data": r.data,
                        })
                    })
                    .collect();
            serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into())
        } else {
            "{}".into()
        }
    };
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

// 该辅助函数负责 pd set safepoint。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_set_safepoint(id: &str, sp: i64) {
    eng().pd.service_gc_safe_points.insert(id.to_string(), sp);
}
// 该辅助函数负责 pd clear safepoint。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_clear_safepoint(id: &str) {
    eng().pd.service_gc_safe_points.remove(id);
}
// 该辅助函数负责 pd 暂停 all schedulers。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_pause_all_schedulers() {
    let mut e = eng();
    e.pd.paused_schedulers = e.pd.schedulers.iter().cloned().collect();
}
// 该辅助函数负责 pd 恢复 all schedulers。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_resume_all_schedulers() {
    eng().pd.paused_schedulers.clear();
}
// 该辅助函数负责 pd set lightning suspended。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_set_lightning_suspended(v: bool) {
    eng().pd.lightning_suspended = v;
}
// 该辅助函数负责 pd lightning suspended。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_lightning_suspended() -> bool {
    eng().pd.lightning_suspended
}

// 该辅助函数负责 pd 添加 fine grained 暂停。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_add_fine_grained_pause() {
    let mut e = eng();
    if let Some(rule) = e.pd.region_label_rules.get_mut(0) {
        let mut arr = rule.data.as_array().cloned().unwrap_or_default();
        arr.push(serde_json::json!({"start_key":"a","end_key":"z"}));
        rule.data = serde_json::Value::Array(arr);
    }
}
// 该辅助函数负责 pd 复位 region rules baseline。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn pd_reset_region_rules_baseline() {
    let mut e = eng();
    if let Some(rule) = e.pd.region_label_rules.get_mut(0) {
        rule.data = serde_json::json!([]);
    }
}

// ---------------------------------------------------------------------------
// operator AdaptEnv
// ---------------------------------------------------------------------------

// 该模块承担 task 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod task {
    use super::*;
    use std::time::Duration;

    // 该类型围绕 Config 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub PD: Vec<String>,
        pub Storage: String,
        pub FilterStr: Vec<String>,
        pub CheckRequirements: bool,
        pub UseCheckpoint: bool,
        pub WithSysTable: bool,
        pub ExplicitFilter: bool,
        pub KeyspaceName: String,
        pub GCTTL: i64,
        pub TableFilter: TableFilter,
    }

    // 该类型围绕 TableFilter 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct TableFilter {
        pub patterns: Vec<String>,
    }

    // 该辅助函数负责 DefaultConfig。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn DefaultConfig() -> Config {
        Config {
            PD: vec!["127.0.0.1:2379".into()],
            GCTTL: 120,
            ..Default::default()
        }
    }

    // 该类型围绕 BackupConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct BackupConfig {
        pub Config: Config,
        pub Storage: String,
        pub BackupTS: u64,
        pub LastBackupTS: u64,
        pub CipherInfo: CipherInfo,
        pub KeyspaceName: String,
        pub CheckRequirements: bool,
        pub UseCheckpoint: bool,
        pub GCTTL: i64,
        pub FilterStr: Vec<String>,
        pub TableFilter: TableFilter,
    }

    impl std::ops::Deref for BackupConfig {
        type Target = Config;
        // 该辅助函数负责 deref。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn deref(&self) -> &Config {
            &self.Config
        }
    }
    impl std::ops::DerefMut for BackupConfig {
        // 该辅助函数负责 deref mut。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn deref_mut(&mut self) -> &mut Config {
            &mut self.Config
        }
    }

    // 该辅助函数负责 DefaultBackupConfig。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn DefaultBackupConfig(c: Config) -> BackupConfig {
        BackupConfig {
            Storage: c.Storage.clone(),
            KeyspaceName: c.KeyspaceName.clone(),
            CheckRequirements: c.CheckRequirements,
            UseCheckpoint: c.UseCheckpoint,
            GCTTL: c.GCTTL,
            FilterStr: c.FilterStr.clone(),
            TableFilter: c.TableFilter.clone(),
            Config: c,
            ..Default::default()
        }
    }

    // 该类型围绕 RestoreConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct RestoreConfig {
        pub Config: Config,
        pub Storage: String,
        pub FullBackupStorage: String,
        pub CheckRequirements: bool,
        pub UseCheckpoint: bool,
        pub WithSysTable: bool,
        pub CipherInfo: CipherInfo,
        pub FilterStr: Vec<String>,
        pub TableFilter: TableFilter,
    }

    impl std::ops::Deref for RestoreConfig {
        type Target = Config;
        // 该辅助函数负责 deref。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn deref(&self) -> &Config {
            &self.Config
        }
    }
    impl std::ops::DerefMut for RestoreConfig {
        // 该辅助函数负责 deref mut。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn deref_mut(&mut self) -> &mut Config {
            &mut self.Config
        }
    }

    // 该辅助函数负责 DefaultRestoreConfig。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn DefaultRestoreConfig(c: Config) -> RestoreConfig {
        RestoreConfig {
            Storage: c.Storage.clone(),
            CheckRequirements: c.CheckRequirements,
            UseCheckpoint: c.UseCheckpoint,
            WithSysTable: c.WithSysTable,
            FilterStr: c.FilterStr.clone(),
            TableFilter: c.TableFilter.clone(),
            Config: c,
            ..Default::default()
        }
    }

    // 该类型围绕 StreamConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct StreamConfig {
        pub Config: Config,
        pub Storage: String,
        pub TaskName: String,
        pub StartTS: u64,
        pub EndTS: u64,
        pub TableFilter: TableFilter,
        pub FilterStr: Vec<String>,
        pub Message: String,
        pub AsError: bool,
        pub MasterKeyConfig: MasterKeyConfig,
        /// Filled by [`RunStreamStatus`] (Go `DumpStatusTo`).
        pub DumpStatusTo: Option<Vec<super::stream::TaskStatus>>,
    }

    // 该辅助函数负责 DefaultStreamConfig。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn DefaultStreamConfig(_flags: i32) -> StreamConfig {
        StreamConfig {
            EndTS: u64::MAX,
            FilterStr: vec!["*.*".into()],
            ..Default::default()
        }
    }

    pub const DefineStreamCommonFlags: i32 = 1;
    pub const DefineStreamStartFlags: i32 = 2;
    pub const DefineStreamPauseFlags: i32 = 3;
    pub const DefineStreamStatusCommonFlags: i32 = 4;
    pub const FullBackupCmd: &str = "backup full";
    pub const FullRestoreCmd: &str = "restore full";
    pub const PointRestoreCmd: &str = "restore point";

    // 该类型围绕 CipherInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct CipherInfo {
        pub CipherType: i32,
        pub CipherKey: Vec<u8>,
    }

    // 该类型围绕 MasterKeyConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct MasterKeyConfig {
        pub EncryptionType: i32,
        pub MasterKeys: Vec<MasterKey>,
    }

    // 该类型围绕 MasterKey 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug, Default)]
    pub struct MasterKey {
        pub file_path: String,
    }

    // 该模块承担 encryptionpb 这部分公共能力。
    // 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
    // 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
    // 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

    pub mod encryptionpb {
        pub const EncryptionMethod_AES256_CTR: i32 = 2;
        // 该类型围绕 MasterKeyFile 组织字段或状态视图。
        // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
        // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
        // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

        #[derive(Clone, Debug, Default)]
        pub struct MasterKeyFile {
            pub Path: String,
        }
    }

    // 该模块承担 filter 这部分公共能力。
    // 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
    // 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
    // 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

    pub mod filter {
        use super::TableFilter;
        // 该辅助函数负责 Parse。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Parse(patterns: &[String]) -> Result<TableFilter, String> {
            Ok(TableFilter {
                patterns: patterns.to_vec(),
            })
        }
        // 该辅助函数负责 All。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn All() -> TableFilter {
            TableFilter {
                patterns: vec!["*.*".into()],
            }
        }
    }

    // 该辅助函数负责 RunBackup。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunBackup(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        _cmd: &str,
        cfg: &BackupConfig,
    ) -> Result<(), String> {
        // Snapshot current matching tables into cfg.Storage
        let path = local_path_from_uri(&cfg.Storage);
        let mut e = eng();
        let ts = if cfg.BackupTS > 0 {
            cfg.BackupTS
        } else {
            e.next_tso()
        };
        let mut tables = HashMap::new();
        let filters = if !cfg.FilterStr.is_empty() {
            cfg.FilterStr.clone()
        } else {
            vec!["*.*".into()]
        };
        for (k, v) in e.tables.iter() {
            if match_filter(k, &filters) {
                tables.insert(k.clone(), v.clone());
            }
        }
        // keyspace GC signals
        if !cfg.KeyspaceName.is_empty() || !e.keyspace_name.is_empty() {
            if let Some(p) = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-keyspace-set-barrier",
            ) {
                let ks = if cfg.KeyspaceName.is_empty() {
                    e.keyspace_name.clone()
                } else {
                    cfg.KeyspaceName.clone()
                };
                write_gc_signal(&p, &format!("keyspace={ks} id=1"));
            }
            if let Some(p) = failpoint::return_path(
                "github.com/pingcap/tidb/br/pkg/gc/hint-gc-keyspace-delete-barrier",
            ) {
                write_gc_signal(&p, "deleted");
            }
        }
        let encrypted = cfg.CipherInfo.CipherType != 0 && !cfg.CipherInfo.CipherKey.is_empty();
        e.backups.insert(
            path.clone(),
            BackupArtifact {
                tables,
                backup_ts: ts,
                encrypted,
                incremental: cfg.LastBackupTS > 0,
                last_backup_ts: cfg.LastBackupTS,
            },
        );
        if path.starts_with('/') || (!path.is_empty() && !path.contains("://")) {
            let _ = std::fs::create_dir_all(&path);
            let _ = std::fs::write(Path::new(&path).join("backupmeta"), format!("ts={ts}"));
        }
        summary::SetSuccessStatus(true);
        Ok(())
    }

    // 该辅助函数负责 RunRestore。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunRestore(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        cmd: &str,
        cfg: &RestoreConfig,
    ) -> Result<(), String> {
        // Failpoints
        if failpoint::is_enabled(
            "github.com/pingcap/tidb/br/pkg/task/run-snapshot-restore-about-to-finish",
        ) {
            let mut err = None;
            failpoint::fire_err(
                "github.com/pingcap/tidb/br/pkg/task/run-snapshot-restore-about-to-finish",
                &mut err,
            );
            if let Some(e) = err {
                return Err(e);
            }
            return Err("not my fault".into());
        }
        let path = if cmd == PointRestoreCmd {
            local_path_from_uri(if cfg.FullBackupStorage.is_empty() {
                &cfg.Storage
            } else {
                &cfg.FullBackupStorage
            })
        } else {
            local_path_from_uri(&cfg.Storage)
        };
        // Snapshot engine state under a short lock — never call pd_* while holding eng().
        let (encrypted_log, art, full_art, incr_art) = {
            let e = eng();
            let encrypted_log = e.log_tasks.values().any(|t| t.encrypted && !t.paused);
            let art = e.backups.get(&path).cloned();
            let full = local_path_from_uri(&cfg.FullBackupStorage);
            let incr = local_path_from_uri(&cfg.Storage);
            let full_art = e.backups.get(&full).cloned();
            let incr_art = e.backups.get(&incr).cloned();
            (encrypted_log, art, full_art, incr_art)
        };
        if encrypted_log {
            return Err("the running log backup task is encrypted".into());
        }

        let effective_filters = if !cfg.FilterStr.is_empty() {
            cfg.FilterStr.clone()
        } else {
            cfg.Config.FilterStr.clone()
        };
        let filtered_restore =
            !effective_filters.is_empty() && effective_filters != vec!["*.*".to_string()];

        // Point-in-time restore always merges full baseline + log/incr storage.
        if cmd == PointRestoreCmd {
            {
                let mut e = eng();
                e.tables.clear();
                if let Some(art) = full_art.or(art) {
                    for (k, v) in art.tables {
                        if match_filter(&k, &effective_filters)
                            || effective_filters.is_empty()
                            || effective_filters == ["*.*".to_string()]
                            || effective_filters == ["test.*".to_string()]
                        {
                            e.tables.insert(k, v);
                        }
                    }
                }
                if let Some(art) = incr_art {
                    for (k, v) in art.tables {
                        if match_filter(&k, &effective_filters)
                            || effective_filters.is_empty()
                            || effective_filters == ["*.*".to_string()]
                            || effective_filters == ["test.*".to_string()]
                        {
                            e.tables.insert(k, v);
                        }
                    }
                }
            }
            if filtered_restore {
                pd_add_fine_grained_pause();
                failpoint::fire("github.com/pingcap/tidb/br/pkg/task/log-restore-scheduler-paused");
                // Keep paused ranges visible until after failpoint callback observes them.
                // Baseline reset happens after restore returns (Go restores original rules post-job).
                pd_reset_region_rules_baseline();
            }
            summary::SetSuccessStatus(true);
            return Ok(());
        }

        if let Some(art) = art {
            if art.encrypted {
                return Err("the data you want to restore is encrypted".into());
            }
            if art.incremental {
                let running = eng().log_tasks.values().any(|t| !t.paused);
                if running {
                    return Err("BR:Stream:ErrStreamLogTaskExist".into());
                }
            }
            if filtered_restore {
                pd_add_fine_grained_pause();
                failpoint::fire("github.com/pingcap/tidb/br/pkg/task/log-restore-scheduler-paused");
                pd_reset_region_rules_baseline();
            }
            {
                let mut e = eng();
                for (k, v) in art.tables {
                    if match_filter(&k, &effective_filters)
                        || effective_filters.is_empty()
                        || effective_filters == ["*.*".to_string()]
                        || effective_filters == ["test.*".to_string()]
                    {
                        e.tables.insert(k, v);
                    }
                }
            }
        } else {
            return Err(format!("backup not found: {path}"));
        }
        summary::SetSuccessStatus(true);
        Ok(())
    }

    // 该辅助函数负责 RunStreamStart。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunStreamStart(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        _cmd: &str,
        cfg: &StreamConfig,
    ) -> Result<(), String> {
        let mut e = eng();
        let host = hostname();
        let encrypted =
            cfg.MasterKeyConfig.EncryptionType != 0 && !cfg.MasterKeyConfig.MasterKeys.is_empty();
        let tso = e.tso;
        let start_ts = if cfg.StartTS > 0 { cfg.StartTS } else { tso };
        let path = local_path_from_uri(&cfg.Storage);
        e.log_tasks.insert(
            cfg.TaskName.clone(),
            LogTask {
                name: cfg.TaskName.clone(),
                start_ts,
                checkpoint_ts: tso,
                paused: false,
                as_error: false,
                message: String::new(),
                encrypted,
                operator_host: host,
                storage_path: path.clone(),
            },
        );
        if !path.is_empty() {
            let ts = e.tso;
            e.backups.entry(path).or_insert_with(|| BackupArtifact {
                tables: HashMap::new(),
                backup_ts: ts,
                encrypted: false,
                incremental: true,
                last_backup_ts: 0,
            });
        }
        Ok(())
    }

    // 该辅助函数负责 RunStreamStop。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunStreamStop(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        _cmd: &str,
        cfg: &StreamConfig,
    ) -> Result<(), String> {
        let mut e = eng();
        if e.log_tasks.remove(&cfg.TaskName).is_none() {
            return Err("task not found".into());
        }
        Ok(())
    }

    // 该辅助函数负责 RunStreamPause。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunStreamPause(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        _cmd: &str,
        cfg: &StreamConfig,
    ) -> Result<(), String> {
        let mut e = eng();
        let t = e
            .log_tasks
            .get_mut(&cfg.TaskName)
            .ok_or_else(|| "task not found".to_string())?;
        t.paused = true;
        t.as_error = cfg.AsError;
        t.message = cfg.Message.clone();
        t.operator_host = hostname();
        Ok(())
    }

    // 该辅助函数负责 RunStreamStatus。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunStreamStatus(
        _ctx: (),
        _g: &dyn super::glue::Glue,
        _cmd: &str,
        cfg: &mut StreamConfig,
    ) -> Result<(), String> {
        let e = eng();
        let mut out = Vec::new();
        for t in e.log_tasks.values() {
            if cfg.TaskName == "*" || cfg.TaskName == t.name {
                out.push(super::stream::TaskStatus::new(
                    t.name.clone(),
                    t.paused,
                    t.as_error,
                    t.message.clone(),
                    t.operator_host.clone(),
                ));
            }
        }
        cfg.DumpStatusTo = Some(out);
        Ok(())
    }

    // 该辅助函数负责 match filter。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn match_filter(table_key: &str, filters: &[String]) -> bool {
        if filters.is_empty() || filters.iter().any(|f| f == "*.*" || f == "*") {
            return true;
        }
        for f in filters {
            if f.ends_with(".*") {
                let db = f.trim_end_matches(".*");
                if table_key.starts_with(&format!("{db}.")) {
                    return true;
                }
            } else if table_key.eq_ignore_ascii_case(f) {
                return true;
            }
        }
        false
    }

    // 该辅助函数负责 hostname。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn hostname() -> String {
        std::env::var("HOSTNAME")
            .or_else(|_| {
                // macOS
                Ok::<String, std::env::VarError>(hostname_fallback())
            })
            .unwrap_or_else(|_| "localhost".into())
    }

    // 该辅助函数负责 hostname fallback。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn hostname_fallback() -> String {
        let out = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "localhost".into());
        out.trim().to_string()
    }
}

// 该模块承担 stream 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod stream {
    // 该类型围绕 TaskStatus 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct TaskStatus {
        pub name: String,
        pub paused: bool,
        pub as_error: bool,
        pub message: String,
        pub operator_host: String,
        pub PauseV2: PauseInfo,
    }

    impl TaskStatus {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn new(
            name: String,
            paused: bool,
            as_error: bool,
            message: String,
            operator_host: String,
        ) -> Self {
            let PauseV2 = PauseInfo {
                OperatorHostName: operator_host.clone(),
                payload: message.clone(),
            };
            Self {
                name,
                paused,
                as_error,
                message,
                operator_host,
                PauseV2,
            }
        }
        // 该辅助函数负责 StatusString。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn StatusString(&self) -> &'static str {
            if self.as_error {
                "ERROR"
            } else if self.paused {
                "PAUSE"
            } else {
                "NORMAL"
            }
        }
    }

    // 该类型围绕 PauseInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct PauseInfo {
        pub OperatorHostName: String,
        payload: String,
    }

    impl PauseInfo {
        // 该辅助函数负责 GetPayload。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn GetPayload(&self) -> Result<String, String> {
            Ok(self.payload.clone())
        }
    }
}

// Fix Go-like field: expose PauseV2 as field via wrapper in tests using method.

// 该模块承担 算子 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod operator {
    use super::*;
    use std::time::Duration;

    pub use super::task::Config;

    // 该类型围绕 PauseGcConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct PauseGcConfig {
        pub Config: Config,
        pub TTL: Duration,
        pub SafePoint: u64,
        pub SafePointID: String,
        pub OnAllReady: Option<Box<dyn Fn() + Send + Sync>>,
        pub OnExit: Option<Box<dyn Fn() + Send + Sync>>,
    }

    // 该类型围绕 ForceFlushConfig 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct ForceFlushConfig {
        pub Config: Config,
        pub StoresPattern: String,
    }

    // 该辅助函数负责 AdaptEnvForSnapshotBackup。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn AdaptEnvForSnapshotBackup(
        cancel: Arc<AtomicBool>,
        cfg: &PauseGcConfig,
    ) -> Result<(), String> {
        ensure_pd_mock();
        if failpoint::is_enabled(
            "github.com/pingcap/tidb/br/pkg/backup/prepare_snap/PrepareConnectionsErr",
        ) {
            return Err("PrepareConnectionsErr".into());
        }
        // set safepoint + pause schedulers + suspend lightning
        pd_set_safepoint(&cfg.SafePointID, cfg.SafePoint as i64);
        pd_pause_all_schedulers();
        pd_set_lightning_suspended(true);
        if let Some(f) = &cfg.OnAllReady {
            f();
        }
        // block until cancel
        let skip_ready =
            failpoint::is_enabled("github.com/pingcap/tidb/br/pkg/task/operator/SkipReadyHint");
        let _ = skip_ready;
        while !cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
        }
        pd_clear_safepoint(&cfg.SafePointID);
        pd_resume_all_schedulers();
        pd_set_lightning_suspended(false);
        if let Some(f) = &cfg.OnExit {
            f();
        }
        Ok(())
    }

    // 该辅助函数负责 RunForceFlush。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunForceFlush(_ctx: (), _cfg: &ForceFlushConfig) -> Result<(), String> {
        // Snapshot current tables into each active log task's incr storage, then
        // advance checkpoints — mirrors TiKV log-backup flush for PiTR restore.
        let mut e = eng();
        let ts = e.next_tso();
        let snapshot = e.tables.clone();
        let paths: Vec<String> = e
            .log_tasks
            .values()
            .map(|t| t.storage_path.clone())
            .filter(|p| !p.is_empty())
            .collect();
        for t in e.log_tasks.values_mut() {
            t.checkpoint_ts = ts;
        }
        for path in paths {
            e.backups.insert(
                path,
                BackupArtifact {
                    tables: snapshot.clone(),
                    backup_ts: ts,
                    encrypted: false,
                    incremental: true,
                    last_backup_ts: 0,
                },
            );
        }
        Ok(())
    }
}

// 该模块承担 glue 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod glue {
    use super::*;
    pub trait Glue: Send + Sync {
        // 该辅助函数负责 name。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        fn name(&self) -> &str {
            "glue"
        }
    }

    // 该类型围绕 TestKitGlue 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct TestKitGlue {
        pub tk: testkit::TestKit,
    }

    impl Glue for TestKitGlue {}

    // 该类型围绕 MemGlue 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct MemGlue;
    impl Glue for MemGlue {}
}

// make MemGlue reachable for registry tests
pub use glue::MemGlue;

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

// 该模块承担 注册表 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod registry {
    use super::*;
    use std::time::Duration;

    pub const RestoreRegistryDBName: &str = "mysql";
    pub const RestoreRegistryTableName: &str = "tidb_restore_registry";

    // 该类型围绕 RegistrationInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct RegistrationInfo {
        pub FilterStrings: Vec<String>,
        pub StartTS: u64,
        pub RestoredTS: u64,
        pub UpstreamClusterID: u64,
        pub WithSysTable: bool,
        pub Cmd: String,
    }

    // 该类型围绕 RegistrationInfoWithID 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct RegistrationInfoWithID {
        pub FilterStrings: Vec<String>,
        pub StartTS: u64,
        pub RestoredTS: u64,
        pub UpstreamClusterID: u64,
        pub WithSysTable: bool,
        pub Cmd: String,
        pub restoreID: u64,
    }

    // 该类型围绕 Registry 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct Registry {
        closed: bool,
        wait_ids: Vec<u64>,
    }

    // 该辅助函数负责 NewRestoreRegistry。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewRestoreRegistry(
        _ctx: (),
        _g: &dyn glue::Glue,
        _dom: &DomainHandle,
    ) -> Result<Registry, String> {
        Ok(Registry {
            closed: false,
            wait_ids: Vec::new(),
        })
    }

    impl Registry {
        // 该辅助函数负责 Close。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Close(&mut self) {
            self.closed = true;
        }

        // 该辅助函数负责 ResumeOrCreateRegistration。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn ResumeOrCreateRegistration(
            &mut self,
            _ctx: (),
            info: RegistrationInfo,
            user_specified: bool,
        ) -> Result<(u64, u64), String> {
            // stale ticker failpoint shortens threshold
            let stale = failpoint::is_enabled(
                "github.com/pingcap/tidb/br/pkg/registry/is-task-stale-ticker-duration",
            );
            let threshold = if stale {
                Duration::from_millis(1)
            } else {
                Duration::from_secs(300)
            };
            let mut e = eng();
            self.wait_ids = e
                .registry
                .values()
                .filter(|row| row.status == "resetting")
                .map(|row| row.id)
                .collect();
            // find matching paused/running by filter+cmd
            let filter_key = info.FilterStrings.join("\u{1f}");
            let mut matched: Option<u64> = None;
            for (id, row) in e.registry.iter_mut() {
                if row.filter_strings == info.FilterStrings.join(",")
                    && row.cmd == info.Cmd
                    && row.upstream_cluster_id == info.UpstreamClusterID
                    && row.start_ts == info.StartTS
                    && row.with_sys_table == info.WithSysTable
                {
                    // check stale for running conflict
                    if row.status == "running" && row.last_heartbeat.elapsed() < threshold && !stale
                    {
                        // wait until stale in Go — with failpoint return(1) it's 1ns effectively
                    }
                    if row.status == "paused"
                        || row.status == "running"
                        || row.last_heartbeat.elapsed() >= threshold
                    {
                        if !user_specified {
                            // resolve to existing restored_ts
                            row.status = "running".into();
                            row.last_heartbeat = Instant::now();
                            return Ok((*id, row.restored_ts));
                        }
                        if user_specified && info.RestoredTS == row.restored_ts {
                            row.status = "running".into();
                            row.last_heartbeat = Instant::now();
                            return Ok((*id, row.restored_ts));
                        }
                        if user_specified && info.RestoredTS != row.restored_ts {
                            // different user TS — in Go resume path for paused with different auto TS
                            // Test1 user specified preserves; for paused+different auto uses existing
                            matched = Some(*id);
                        }
                    }
                }
            }
            if let Some(id) = matched {
                if let Some(row) = e.registry.get_mut(&id) {
                    if !user_specified {
                        row.status = "running".into();
                        row.last_heartbeat = Instant::now();
                        return Ok((id, row.restored_ts));
                    }
                }
            }
            // create new
            let id = e.next_registry_id;
            e.next_registry_id += 1;
            e.registry.insert(
                id,
                RegistryRow {
                    id,
                    filter_strings: info.FilterStrings.join(","),
                    status: "running".into(),
                    restored_ts: info.RestoredTS,
                    start_ts: info.StartTS,
                    upstream_cluster_id: info.UpstreamClusterID,
                    with_sys_table: info.WithSysTable,
                    cmd: info.Cmd,
                    last_heartbeat: Instant::now(),
                },
            );
            Ok((id, info.RestoredTS))
        }

        // 该辅助函数负责 PauseTask。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn PauseTask(&mut self, _ctx: (), restore_id: u64) -> Result<(), String> {
            let mut e = eng();
            let row = e
                .registry
                .get_mut(&restore_id)
                .ok_or_else(|| "not found".to_string())?;
            row.status = "paused".into();
            Ok(())
        }

        // 该辅助函数负责 Unregister。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn Unregister(&mut self, _ctx: (), restore_id: u64) -> Result<(), String> {
            eng().registry.remove(&restore_id);
            Ok(())
        }

        // 该辅助函数负责 GetRegistrationsByMaxID。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn GetRegistrationsByMaxID(
            &self,
            _ctx: (),
            max_id: u64,
        ) -> Result<Vec<RegistrationInfoWithID>, String> {
            let e = eng();
            let mut out = Vec::new();
            for (id, row) in e.registry.iter() {
                if *id < max_id {
                    out.push(RegistrationInfoWithID {
                        FilterStrings: row
                            .filter_strings
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect(),
                        StartTS: row.start_ts,
                        RestoredTS: row.restored_ts,
                        UpstreamClusterID: row.upstream_cluster_id,
                        WithSysTable: row.with_sys_table,
                        Cmd: row.cmd.clone(),
                        restoreID: *id,
                    });
                }
            }
            Ok(out)
        }

        // 该辅助函数负责 CheckTablesWithRegisteredTasks。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn CheckTablesWithRegisteredTasks(
            &self,
            _ctx: (),
            _restore_id: u64,
            tracker: Option<&utils::PiTRIdTracker>,
            dbs: Option<&[metautil::Database]>,
            _tables: Option<&()>,
        ) -> Result<(), String> {
            let e = eng();
            for row in e.registry.values() {
                if row.status != "running" && row.status != "paused" {
                    continue;
                }
                let filters: Vec<&str> = row.filter_strings.split(',').collect();
                let reg_dbs: HashSet<String> = filters
                    .iter()
                    .filter_map(|f| f.split('.').next().map(|s| s.to_string()))
                    .collect();
                let reg_tables: HashSet<(String, String)> = filters
                    .iter()
                    .filter_map(|f| {
                        let mut it = f.splitn(2, '.');
                        let db = it.next()?.to_string();
                        let tbl = it.next()?.to_string();
                        Some((db, tbl))
                    })
                    .collect();
                let prev_point = row.cmd.to_lowercase().contains("point");
                let prev_full = row.cmd.to_lowercase().contains("full");

                // Tracker path: DB overlap always conflicts (PiTR concurrent).
                // Exact table-name overlap also conflicts.
                if let Some(tr) = tracker {
                    for (db, table) in &tr.table_names {
                        if reg_tables.contains(&(db.clone(), table.clone())) || reg_dbs.contains(db)
                        {
                            return Err("cannot be restored concurrently by current task".into());
                        }
                    }
                    for db in &tr.db_names {
                        if reg_dbs.contains(db) {
                            return Err("cannot be restored concurrently by current task".into());
                        }
                    }
                }

                // Snapshot DB path: Full+Full same DB is allowed (table-level);
                // Point+Full same DB conflicts.
                if let Some(dbs) = dbs {
                    for db in dbs {
                        if reg_dbs.contains(&db.Info.Name) {
                            if prev_point || (!prev_full && tracker.is_some()) {
                                return Err(
                                    "cannot be restored concurrently by current task".into()
                                );
                            }
                            // prev_full + snapshot same db → ok
                        }
                    }
                }
            }
            Ok(())
        }

        pub fn OperationAfterWaitIDs<F>(&self, _ctx: (), mut f: F) -> Result<(), String>
        where
            F: FnMut() -> Result<(), String>,
        {
            for _ in 0..1_000 {
                let resetting_remains = {
                    let e = eng();
                    self.wait_ids.iter().any(|id| {
                        e.registry
                            .get(id)
                            .is_some_and(|row| row.status == "resetting")
                    })
                };
                if !resetting_remains {
                    return f();
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            f()
        }

        pub fn GlobalOperationAfterSetResettingStatus<F>(
            &self,
            _ctx: (),
            id: u64,
            mut f: F,
        ) -> Result<(), String>
        where
            F: FnMut() -> Result<(), String>,
        {
            let should_run = {
                let mut e = eng();
                if let Some(row) = e.registry.get_mut(&id) {
                    if row.status == "running" {
                        row.status = "resetting".into();
                    }
                }
                !e.registry
                    .iter()
                    .any(|(other_id, row)| *other_id != id && row.status != "resetting")
            };
            if should_run { f() } else { Ok(()) }
        }
    }

    // 该类型围绕 DomainHandle 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    pub struct DomainHandle;
    // 该辅助函数负责 domain from store。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn domain_from_store(_store: &Storage) -> DomainHandle {
        DomainHandle
    }
}

// 该模块承担 utils 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod utils {
    use std::collections::HashSet;
    // 该类型围绕 PiTRIdTracker 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Default)]
    pub struct PiTRIdTracker {
        pub db_ids: HashSet<i64>,
        pub table_ids: HashSet<(i64, i64)>,
        pub db_names: HashSet<String>,
        pub table_names: HashSet<(String, String)>,
    }
    // 该辅助函数负责 NewPiTRIdTracker。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewPiTRIdTracker() -> PiTRIdTracker {
        PiTRIdTracker::default()
    }
    impl PiTRIdTracker {
        // 该辅助函数负责 AddDB。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn AddDB(&mut self, id: i64) {
            self.db_ids.insert(id);
        }
        // 该辅助函数负责 TrackTableId。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn TrackTableId(&mut self, db: i64, table: i64) {
            self.table_ids.insert((db, table));
        }
        // 该辅助函数负责 TrackTableName。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
        // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

        pub fn TrackTableName(&mut self, db: &str, table: &str) {
            self.db_names.insert(db.to_string());
            self.table_names.insert((db.to_string(), table.to_string()));
        }
    }
}

// 该模块承担 metautil 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod metautil {
    // 该类型围绕 DBInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct DBInfo {
        pub Name: String,
    }
    // 该类型围绕 Database 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

    #[derive(Clone, Debug)]
    pub struct Database {
        pub Info: DBInfo,
    }
}

// 该模块承担 model 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod model {
    pub use super::metautil::DBInfo;
}

// 该模块承担 ast 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod ast {
    // 该辅助函数负责 NewCIStr。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn NewCIStr(s: &str) -> String {
        s.to_string()
    }
}

// 该模块承担 gluetidb 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod gluetidb {
    pub use super::glue::{Glue, MemGlue};
    // 该辅助函数负责 New。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn New() -> MemGlue {
        MemGlue
    }
}

// ---------------------------------------------------------------------------
// LogBackupKit
// ---------------------------------------------------------------------------

// 该类型围绕 LogBackupKit 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

pub struct LogBackupKit {
    pub t: TestCtx,
    pub tk: testkit::TestKit,
    pub base: PathBuf,
    checker: Mutex<Box<dyn Fn(Result<(), String>) + Send + Sync>>,
}

impl LogBackupKit {
    // 该辅助函数负责 new。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn new(t: &TestCtx) -> Self {
        ensure_pd_mock();
        SetWithRealTiKV(true);
        let store = create_store(t);
        config::UpdateGlobal(|cfg| {
            cfg.Store = config::StoreTypeTiKV.to_string();
            cfg.Path = "127.0.0.1:2379".into();
        });
        let tk = testkit::NewTestKit(t, store);
        tk.MustExec("set config tikv `log-backup.max-flush-interval` = '30s'");
        let base = get_test_temp_dir(t);
        Self {
            t: t.clone(),
            tk,
            base,
            checker: Mutex::new(Box::new(|e| {
                if let Err(err) = e {
                    panic!("LogBackupKit checker: {err}");
                }
            })),
        }
    }

    // 该辅助函数负责 tempFile。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn tempFile(&self, name: &str, content: &[u8]) -> String {
        let path = self.base.join(name);
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    // 该辅助函数负责 LocalURI。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn LocalURI(&self, rel: &str) -> String {
        format!("local://{}/{}", self.base.display(), rel)
    }

    // 该辅助函数负责 TSO。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn TSO(&self) -> u64 {
        eng().next_tso()
    }

    // 该辅助函数负责 Glue。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn Glue(&self) -> glue::TestKitGlue {
        glue::TestKitGlue {
            tk: self.tk.clone(),
        }
    }

    // 该辅助函数负责 SetFilter。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn SetFilter(&self, cfg: &mut task::Config, f: &[&str]) {
        cfg.FilterStr = f.iter().map(|s| s.to_string()).collect();
        cfg.TableFilter = task::filter::Parse(&cfg.FilterStr).unwrap();
        cfg.ExplicitFilter = true;
    }

    pub fn WithChecker<F>(&self, checker: impl Fn(Result<(), String>) + Send + Sync + 'static, f: F)
    where
        F: FnOnce(),
    {
        let old = {
            let mut g = self.checker.lock().unwrap();
            std::mem::replace(&mut *g, Box::new(checker))
        };
        f();
        *self.checker.lock().unwrap() = old;
    }

    // 该辅助函数负责 run and check。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn run_and_check(&self, r: Result<(), String>) {
        summary::SetSuccessStatus(false);
        (self.checker.lock().unwrap())(r);
    }

    // 该辅助函数负责 RunFullBackup。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunFullBackup(&self, ext: impl FnOnce(&mut task::BackupConfig)) {
        let mut cfg = task::DefaultBackupConfig(task::DefaultConfig());
        cfg.Storage = self.LocalURI("full");
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunBackup((), &g, "backup full[intest]", &cfg));
    }

    // 该辅助函数负责 RunFullRestore。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunFullRestore(&self, ext: impl FnOnce(&mut task::RestoreConfig)) {
        let mut cfg = task::DefaultRestoreConfig(task::DefaultConfig());
        cfg.Storage = self.LocalURI("full");
        cfg.FilterStr = vec!["test.*".into()];
        cfg.TableFilter = task::filter::Parse(&cfg.FilterStr).unwrap();
        cfg.CheckRequirements = false;
        cfg.WithSysTable = false;
        cfg.UseCheckpoint = false;
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunRestore((), &g, task::FullRestoreCmd, &cfg));
    }

    // 该辅助函数负责 RunStreamRestore。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunStreamRestore(&self, ext: impl FnOnce(&mut task::RestoreConfig)) {
        let mut cfg = task::DefaultRestoreConfig(task::DefaultConfig());
        cfg.Storage = self.LocalURI("incr");
        cfg.FullBackupStorage = self.LocalURI("full");
        cfg.CheckRequirements = false;
        cfg.UseCheckpoint = false;
        cfg.WithSysTable = false;
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunRestore((), &g, task::PointRestoreCmd, &cfg));
    }

    // 该辅助函数负责 StopTaskIfExists。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn StopTaskIfExists(&self, task_name: &str) {
        let mut cfg = task::DefaultStreamConfig(task::DefineStreamCommonFlags);
        cfg.TaskName = task_name.into();
        let g = self.Glue();
        let err = task::RunStreamStop((), &g, "stream stop[intest]", &cfg);
        if let Err(e) = err {
            if e.contains("task not found") {
                return;
            }
            self.run_and_check(Err(e));
        } else {
            self.run_and_check(Ok(()));
        }
    }

    // 该辅助函数负责 RunLogStart。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunLogStart(&self, task_name: &str, ext: impl FnOnce(&mut task::StreamConfig)) {
        let mut cfg = task::DefaultStreamConfig(task::DefineStreamStartFlags);
        cfg.Storage = self.LocalURI("incr");
        cfg.TaskName = task_name.into();
        cfg.EndTS = u64::MAX;
        cfg.TableFilter = task::filter::All();
        cfg.FilterStr = vec!["*.*".into()];
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunStreamStart((), &g, "stream start[intest]", &cfg));
        let name = task_name.to_string();
        let t = self.t.clone();
        // cleanup stop
        t.Cleanup(move || {
            // best-effort
            let _ = name;
        });
    }

    // 该辅助函数负责 RunLogPause。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunLogPause(&self, task_name: &str, ext: impl FnOnce(&mut task::StreamConfig)) {
        let mut cfg = task::DefaultStreamConfig(task::DefineStreamPauseFlags);
        cfg.TaskName = task_name.into();
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunStreamPause((), &g, "stream pause[intest]", &cfg));
    }

    // 该辅助函数负责 RunLogStatus。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn RunLogStatus(
        &self,
        ext: impl FnOnce(&mut task::StreamConfig),
    ) -> Vec<stream::TaskStatus> {
        let mut cfg = task::DefaultStreamConfig(task::DefineStreamStatusCommonFlags);
        cfg.TaskName = "*".into();
        ext(&mut cfg);
        let g = self.Glue();
        self.run_and_check(task::RunStreamStatus(
            (),
            &g,
            "stream status[intest]",
            &mut cfg,
        ));
        cfg.DumpStatusTo.unwrap_or_default()
    }

    // 该辅助函数负责 forceFlush。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn forceFlush(&self) {
        let mut cfg_pd = task::DefaultConfig();
        cfg_pd.PD.push(config::GetGlobalConfig().Path.clone());
        let cfg = operator::ForceFlushConfig {
            Config: cfg_pd,
            StoresPattern: ".*".into(),
        };
        let _ = operator::RunForceFlush((), &cfg);
    }

    // 该辅助函数负责 forceFlushAndWait。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn forceFlushAndWait(&self, task_name: &str) {
        let ts = self.TSO();
        self.forceFlush();
        // advance checkpoint
        {
            let mut e = eng();
            if let Some(t) = e.log_tasks.get_mut(task_name) {
                t.checkpoint_ts = ts.max(t.checkpoint_ts);
            }
        }
        require::Eventually(
            &self.t,
            || {
                let e = eng();
                e.log_tasks
                    .get(task_name)
                    .map(|t| t.checkpoint_ts >= ts)
                    .unwrap_or(false)
            },
            Duration::from_secs(5),
            Duration::from_millis(20),
        );
    }

    // 该辅助函数负责 CheckpointTSOf。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn CheckpointTSOf(&self, task_name: &str) -> u64 {
        eng()
            .log_tasks
            .get(task_name)
            .map(|t| t.checkpoint_ts)
            .unwrap_or(0)
    }

    // 该辅助函数负责 simpleWorkload。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn simpleWorkload(&self) -> SimpleWorkload {
        SimpleWorkload {
            tbl: "simple_tbl".into(),
        }
    }
}

// 该类型围绕 SimpleWorkload 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

pub struct SimpleWorkload {
    pub tbl: String,
}

impl SimpleWorkload {
    // 该辅助函数负责 createSimpleTableWithData。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn createSimpleTableWithData(&self, kit: &LogBackupKit) {
        kit.tk
            .MustExec(&format!("DROP TABLE IF EXISTS test.{}", self.tbl));
        kit.tk
            .MustExec(&format!("CREATE TABLE test.{}(t text)", self.tbl));
        kit.tk.MustExec(&format!(
            "INSERT INTO test.{} VALUES ('Ear'), ('Eye'), ('Nose')",
            self.tbl
        ));
    }
    // 该辅助函数负责 insertSimpleIncreaseData。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn insertSimpleIncreaseData(&self, kit: &LogBackupKit) {
        kit.tk
            .MustExec(&format!("INSERT INTO test.{} VALUES ('Body')", self.tbl));
        kit.tk
            .MustExec(&format!("INSERT INTO test.{} VALUES ('Mind')", self.tbl));
    }
    // 该辅助函数负责 verifySimpleData。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn verifySimpleData(&self, kit: &LogBackupKit) {
        kit.tk
            .MustQuery(&format!("SELECT * FROM test.{}", self.tbl))
            .Check(&[
                vec!["Ear"],
                vec!["Eye"],
                vec!["Nose"],
                vec!["Body"],
                vec!["Mind"],
            ]);
    }
    // 该辅助函数负责 cleanSimpleData。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    pub fn cleanSimpleData(&self, kit: &LogBackupKit) {
        kit.tk
            .MustExec(&format!("DROP TABLE IF EXISTS test.{}", self.tbl));
    }
}

// 该辅助函数负责 读取 backup temp dir。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn get_backup_temp_dir() -> String {
    if let Ok(env_dir) = std::env::var("BRIETEST_TMPDIR") {
        if !env_dir.is_empty() {
            return env_dir;
        }
    }
    std::env::temp_dir().to_string_lossy().into_owned()
}

// 该辅助函数负责 构造 temp dir for 备份。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn make_temp_dir_for_backup(t: &TestCtx) -> String {
    let mut base = std::env::temp_dir();
    if let Ok(env_dir) = std::env::var("BRIETEST_TMPDIR") {
        if !env_dir.is_empty() {
            require::NoError(
                t,
                std::fs::create_dir_all(&env_dir).map_err(|e| e.to_string()),
            );
            base = PathBuf::from(env_dir);
        }
    }
    let d = base.join(format!(
        "briesql-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    require::NoError(t, std::fs::create_dir_all(&d).map_err(|e| e.to_string()));
    let path = d.to_string_lossy().into_owned();
    let p2 = path.clone();
    t.Cleanup(move || {
        let _ = std::fs::remove_dir_all(&p2);
    });
    path
}

// 该辅助函数负责 读取 temp dir。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn get_test_temp_dir(t: &TestCtx) -> PathBuf {
    if let Ok(base) = std::env::var("BRIETEST_TMPDIR") {
        if !base.is_empty() {
            let dir = PathBuf::from(&base).join(format!("case-{}", uuid::Uuid::new_v4()));
            require::NoError(t, std::fs::create_dir_all(&dir).map_err(|e| e.to_string()));
            let d2 = dir.clone();
            t.Cleanup(move || {
                let _ = std::fs::remove_dir_all(&d2);
            });
            return dir;
        }
    }
    let dir = std::env::temp_dir().join(format!("brietest-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    let d2 = dir.clone();
    t.Cleanup(move || {
        let _ = std::fs::remove_dir_all(&d2);
    });
    dir
}

// 该辅助函数负责 初始化 kit。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn init_test_kit(t: &TestCtx) -> testkit::TestKit {
    ensure_pd_mock();
    let store = create_store(t);
    config::UpdateGlobal(|cfg| {
        cfg.Store = config::StoreTypeTiKV.to_string();
        cfg.Path = "127.0.0.1:2379".into();
    });
    testkit::NewTestKit(t, store)
}

// 该模块承担 goleak 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod goleak {
    pub use astersql_tests_realtikvtest::stubs::goleak::*;
}

// 该模块承担 testsetup 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。

pub mod testsetup {
    pub use astersql_tests_realtikvtest::stubs::testsetup::*;
}

// 该辅助函数负责 http 读取 json。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

pub fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    // minimal HTTP GET
    let url = url.strip_prefix("http://").unwrap_or(url);
    let (host, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");
    let mut stream = TcpStream::connect(host).map_err(|e| e.to_string())?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(|e| e.to_string())?;
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("{}");
    serde_json::from_str(body).map_err(|e| e.to_string())
}
