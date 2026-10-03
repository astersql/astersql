// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 测试基础设施和桩边界。
// 中文总览：重点在于模拟边界、公共断言和环境收口顺序。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：模块 `kerneltype` 负责 kerneltype。
// 中文总览：函数 `IsNextGen` 负责 IsNextGen。
// 中文总览：函数 `IsClassic` 负责 IsClassic。
// 中文总览：模块 `require` 负责 require。
// 中文总览：函数 `NoError` 负责 NoError。
// 中文总览：函数 `True` 负责 True。
// 中文总览：函数 `TrueMsg` 负责 TrueMsg。
// 中文总览：函数 `False` 负责 False。
// 中文总览：函数 `Contains` 负责 Contains。
// 中文总览：函数 `NotNil` 负责 NotNil。
// 中文总览：函数 `Regexp` 负责 Regexp。
// 中文总览：模块 `freeport` 负责 freeport。
// 中文总览：函数 `GetFreePort` 负责 GetFreePort。
// 中文总览：枚举 `FailCtx` 负责 FailCtx。
// 中文总览：类型 `FpState` 负责 FpState。
// 中文总览：函数 `fp_slot` 负责 fp slot。
// 中文总览：模块 `failpoint` 负责 failpoint。
// 中文总览：函数 `Enable` 负责 Enable。
// 中文总览：函数 `Disable` 负责 Disable。
// 中文总览：函数 `is_enabled` 负责 is enabled。
// 中文总览：函数 `term` 负责 term。
// 中文总览：函数 `reset` 负责 reset。
// 中文总览：模块 `testfailpoint` 负责 testfailpoint。
// 中文总览：函数 `Enable` 负责 Enable。

//! Slim local RealTiKV / DDL / DXF / failpoint / fake-GCS harness for
//! `tests/realtikvtest/addindextest2` on darwin arm64 (no kv/domain/kvproto/grpcio).
//!
//! Mock/real boundary (matches Go):
//! - **Real boundary (simulated in-process):** CreateMockStoreAndSetup SQL sessions,
//!   DDL add-index pipeline stages, admin show/alter ddl jobs, cloud-storage URI.
//! - **Mock (as in Go):** failpoints (`testfailpoint` / `failpoint`), fake-GCS
//!   (`fakestorage`), DXF scheduler cleanup channels, CPU count, merge-sort force,
//!   metering flush hooks, worker-pool sizing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use astersql_tests_realtikvtest::stubs::{Storage, TestCtx, TestMain, config};

// 该模块承担 kerneltype 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod kerneltype {
    // 该辅助函数负责 IsNextGen。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn IsNextGen() -> bool {
        astersql_tests_realtikvtest::stubs::kerneltype::IsNextGen()
    }
    // 该辅助函数负责 IsClassic。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn IsClassic() -> bool {
        !IsNextGen()
    }
}
pub use astersql_tests_realtikvtest::{
    CreateMockStoreAndDomainAndSetup, CreateMockStoreAndSetup, RunTestMain, UpdateTiDBConfig,
};
pub use astersql_tests_realtikvtest_testutils::AssertExternalField;
pub use astersql_tests_realtikvtest_testutils::ExternalTagged;
pub use astersql_tests_realtikvtest_testutils::ExternalTaggedField;

// ---------------------------------------------------------------------------
// require (testify subset used by this package)
// ---------------------------------------------------------------------------

// 该模块承担 require 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod require {
    use super::TestCtx;
    use std::fmt::Debug;
    use std::time::{Duration, Instant};

    // 该辅助函数负责 NoError。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NoError(t: &TestCtx, err: Result<(), String>) {
        if let Err(e) = err {
            t.Fail();
            panic!("require.NoError: {e}");
        }
    }

    // 该辅助函数负责 True。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn True(t: &TestCtx, cond: bool) {
        if !cond {
            t.Fail();
            panic!("require.True failed");
        }
    }

    // 该辅助函数负责 TrueMsg。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn TrueMsg(t: &TestCtx, cond: bool, msg: &str) {
        if !cond {
            t.Fail();
            panic!("require.True: {msg}");
        }
    }

    // 该辅助函数负责 False。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

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

    pub fn EqualValues<T: PartialEq + Debug>(t: &TestCtx, expected: T, actual: T) {
        Equal(t, expected, actual);
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

    pub fn Contains(t: &TestCtx, haystack: &str, needle: &str) {
        if !haystack.contains(needle) {
            t.Fail();
            panic!("require.Contains: {haystack:?} missing {needle:?}");
        }
    }

    pub fn Greater<T: PartialOrd + Debug>(t: &TestCtx, a: T, b: T) {
        if !(a > b) {
            t.Fail();
            panic!("require.Greater: {a:?} !> {b:?}");
        }
    }

    // 该辅助函数负责 NotNil。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NotNil(t: &TestCtx, ok: bool) {
        if !ok {
            t.Fail();
            panic!("require.NotNil failed");
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

    // 该辅助函数负责 Regexp。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn Regexp(t: &TestCtx, re: &str, text: &str) {
        let ok = match re {
            r"cluster\{r: 1\d\dB, w: (\d{3}|.*Ki)B\}" => match metric_body(text, "cluster{r: ") {
                Some((read, write)) => {
                    let read = read.strip_suffix('B').unwrap_or("");
                    let read_ok = read.len() == 3
                        && read.starts_with('1')
                        && read.bytes().all(|byte| byte.is_ascii_digit());
                    let write_ok = write.strip_suffix("KiB").is_some()
                        || write.strip_suffix('B').is_some_and(|value| {
                            value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit())
                        });
                    read_ok && write_ok
                }
                None => false,
            },
            r"obj_store\{r: 1.\d+KiB, w: \d.\d+KiB\}" => match metric_body(text, "obj_store{r: ") {
                Some((read, write)) => decimal_kib(read, Some('1')) && decimal_kib(write, None),
                None => false,
            },
            _ => text.contains(re),
        };
        if !ok {
            t.Fail();
            panic!("require.Regexp: pattern={re:?} text={text:?}");
        }
    }

    fn metric_body<'a>(text: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
        let body = text.split_once(prefix)?.1.split_once('}')?.0;
        body.split_once(", w: ")
    }

    fn decimal_kib(value: &str, required_first: Option<char>) -> bool {
        let number = match value.strip_suffix("KiB") {
            Some(number) => number,
            None => return false,
        };
        let mut chars = number.chars();
        let first = match chars.next() {
            Some(first) => first,
            None => return false,
        };
        if required_first.is_some_and(|required| first != required) || !first.is_ascii_digit() {
            return false;
        }
        if chars.next().is_none() {
            return false;
        }
        let digits = chars.as_str();
        !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
    }
}

// ---------------------------------------------------------------------------
// freeport
// ---------------------------------------------------------------------------

// 该模块承担 freeport 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod freeport {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(19400);
    // 该辅助函数负责 GetFreePort。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GetFreePort() -> Result<i32, String> {
        Ok(NEXT.fetch_add(1, Ordering::SeqCst) as i32)
    }
}

// ---------------------------------------------------------------------------
// failpoint + testfailpoint
// ---------------------------------------------------------------------------

type CallHook = Arc<dyn Fn(FailCtx) + Send + Sync>;

// 该类型围绕 FailCtx 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

#[derive(Clone)]
pub enum FailCtx {
    None,
    Job(model::Job),
    JobMut(Arc<Mutex<model::Job>>),
    Pipeline(operator::AsyncPipeline),
    Backend(ingestctrl::Backend),
    Step(proto::Step),
    ReorgMeta(Arc<Mutex<model::DDLReorgMeta>>),
    Streaming(Arc<Mutex<bool>>),
    MergeOp(globalsort::MergeOperator),
    Threshold(Arc<Mutex<i32>>),
    Ts(Arc<Mutex<i64>>),
    MeterString(Arc<Mutex<String>>),
    MeterItems(Arc<Mutex<HashMap<String, i64>>>),
    ExtraParams {
        slots: Arc<Mutex<i32>>,
        params: Arc<Mutex<proto::ExtraParams>>,
    },
    NumWorkers(i32),
    TaskExec {
        step: proto::Step,
        err: Arc<Mutex<Option<String>>>,
    },
}

// 该类型围绕 FpState 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

struct FpState {
    enabled: HashMap<String, String>,
    calls: HashMap<String, CallHook>,
    oneshot: HashMap<String, u32>,
}

// 该辅助函数负责 fp slot。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn fp_slot() -> &'static Mutex<FpState> {
    static S: OnceLock<Mutex<FpState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(FpState {
            enabled: HashMap::new(),
            calls: HashMap::new(),
            oneshot: HashMap::new(),
        })
    })
}

// 该模块承担 failpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod failpoint {
    use super::*;

    // 该辅助函数负责 Enable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn Enable(path: &str, term: &str) -> Result<(), String> {
        let mut g = fp_slot().lock().unwrap();
        g.enabled.insert(path.to_string(), term.to_string());
        if let Some(rest) = term.strip_prefix("1*") {
            g.oneshot.insert(path.to_string(), 1);
            let _ = rest;
        }
        Ok(())
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn Disable(path: &str) -> Result<(), String> {
        let mut g = fp_slot().lock().unwrap();
        g.enabled.remove(path);
        g.calls.remove(path);
        g.oneshot.remove(path);
        Ok(())
    }

    // 该辅助函数负责 is enabled。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn is_enabled(path: &str) -> bool {
        fp_slot().lock().unwrap().enabled.contains_key(path)
    }

    // 该辅助函数负责 term。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn term(path: &str) -> Option<String> {
        fp_slot().lock().unwrap().enabled.get(path).cloned()
    }

    // 该辅助函数负责 reset。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn reset() {
        let mut g = fp_slot().lock().unwrap();
        g.enabled.clear();
        g.calls.clear();
        g.oneshot.clear();
    }
}

// 该模块承担 testfailpoint 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod testfailpoint {
    use super::*;

    // 该辅助函数负责 Enable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn Enable(t: &TestCtx, path: &str, term: &str) {
        let _ = t;
        require::NoError(t, failpoint::Enable(path, term));
        t.Cleanup({
            let path = path.to_string();
            move || {
                let _ = failpoint::Disable(&path);
            }
        });
    }

    pub fn EnableCall<F>(t: &TestCtx, path: &str, f: F)
    where
        F: Fn(FailCtx) + Send + Sync + 'static,
    {
        {
            let mut g = fp_slot().lock().unwrap();
            g.enabled.insert(path.to_string(), "callback".to_string());
            g.calls.insert(path.to_string(), Arc::new(f));
        }
        t.Cleanup({
            let path = path.to_string();
            move || {
                let _ = failpoint::Disable(&path);
            }
        });
    }

    // 该辅助函数负责 Disable。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn Disable(t: &TestCtx, path: &str) {
        require::NoError(t, failpoint::Disable(path));
    }
}

// 该辅助函数负责 fire call。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn fire_call(path: &str, ctx: FailCtx) {
    let hook = fp_slot().lock().unwrap().calls.get(path).cloned();
    if let Some(h) = hook {
        h(ctx);
    }
}

// 该辅助函数负责 consume oneshot。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn consume_oneshot(path: &str) -> bool {
    let mut g = fp_slot().lock().unwrap();
    if let Some(n) = g.oneshot.get_mut(path) {
        if *n == 0 {
            return false;
        }
        *n -= 1;
        return true;
    }
    g.enabled.contains_key(path)
}

// ---------------------------------------------------------------------------
// model / operator / ingestctrl / proto / globalsort / metering / execute
// ---------------------------------------------------------------------------

// 该模块承担 model 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod model {
    use super::*;

    pub const ActionAddIndex: i32 = 7;
    pub const StateWriteReorganization: i32 = 3;
    pub const StatePublic: i32 = 5;

    // 该类型围绕 DDLReorgMeta 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct DDLReorgMeta {
        pub batch_size: i32,
        pub UseCloudStorage: bool,
    }

    impl DDLReorgMeta {
        // 该辅助函数负责 GetBatchSize。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetBatchSize(&self) -> i32 {
            self.batch_size
        }
        // 该辅助函数负责 set batch size。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn set_batch_size(&mut self, v: i32) {
            self.batch_size = v;
        }
    }

    // 该类型围绕 IndexInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: String,
    }

    // 该类型围绕 TableInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct TableInfo {
        pub ID: i64,
        pub Indices: Vec<IndexInfo>,
    }

    // 该类型围绕 Job 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct Job {
        pub ID: i64,
        pub Type: i32,
        pub SchemaState: i32,
        pub TableID: i64,
        pub ReorgMeta: DDLReorgMeta,
        pub row_count: i64,
        pub reorg_tp: String,
        pub state: String,
    }

    impl Job {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(id: i64) -> Self {
            Self {
                ID: id,
                Type: ActionAddIndex,
                SchemaState: StateWriteReorganization,
                TableID: 1,
                ReorgMeta: DDLReorgMeta {
                    batch_size: 32,
                    UseCloudStorage: false,
                },
                row_count: 0,
                reorg_tp: String::new(),
                state: "synced".into(),
            }
        }
    }
}

// 该模块承担 算子 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod operator {
    use std::sync::{Arc, Mutex};

    // 该类型围绕 WorkerPool 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct WorkerPool {
        size: Arc<Mutex<i32>>,
    }

    impl WorkerPool {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(size: i32) -> Self {
            Self {
                size: Arc::new(Mutex::new(size)),
            }
        }
        // 该辅助函数负责 GetWorkerPoolSize。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetWorkerPoolSize(&self) -> i32 {
            *self.size.lock().unwrap()
        }
        // 该辅助函数负责 set。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn set(&self, n: i32) {
            *self.size.lock().unwrap() = n;
        }
    }

    // 该类型围绕 AsyncPipeline 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct AsyncPipeline {
        reader: WorkerPool,
        writer: WorkerPool,
    }

    impl AsyncPipeline {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(r: i32, w: i32) -> Self {
            Self {
                reader: WorkerPool::new(r),
                writer: WorkerPool::new(w),
            }
        }
        // 该辅助函数负责 GetReaderAndWriter。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetReaderAndWriter(&self) -> (WorkerPool, WorkerPool) {
            (self.reader.clone(), self.writer.clone())
        }
    }
}

// 该模块承担 ingestctrl 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod ingestctrl {
    use std::sync::{Arc, Mutex};

    // 该类型围绕 Backend 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct Backend {
        write_speed: Arc<Mutex<i64>>,
    }

    impl Backend {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(limit: i64) -> Self {
            Self {
                write_speed: Arc::new(Mutex::new(limit)),
            }
        }
        // 该辅助函数负责 GetWriteSpeedLimit。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetWriteSpeedLimit(&self) -> i64 {
            *self.write_speed.lock().unwrap()
        }
        // 该辅助函数负责 set write speed。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn set_write_speed(&self, v: i64) {
            *self.write_speed.lock().unwrap() = v;
        }
    }
}

// 该模块承担 proto 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod proto {
    // 该类型围绕 Step 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum Step {
        #[default]
        StepInit,
        BackfillStepReadIndex,
        BackfillStepMergeSort,
        BackfillStepWriteAndIngest,
    }

    // 该类型围绕 ExtraParams 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug, Default)]
    pub struct ExtraParams {
        pub MaxRuntimeSlots: i32,
    }
}

// 该模块承担 globalsort 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod globalsort {
    use std::sync::{Arc, Mutex};

    // 该类型围绕 MergeOperator 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct MergeOperator {
        size: Arc<Mutex<i32>>,
    }

    impl MergeOperator {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(size: i32) -> Self {
            Self {
                size: Arc::new(Mutex::new(size)),
            }
        }
        // 该辅助函数负责 GetWorkerPoolSize。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetWorkerPoolSize(&self) -> i32 {
            *self.size.lock().unwrap()
        }
        // 该辅助函数负责 set。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn set(&self, n: i32) {
            *self.size.lock().unwrap() = n;
        }
    }
}

// 该模块承担 metering 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod metering {
    use std::sync::Mutex;
    use std::time::Duration;

    pub static FlushInterval: Mutex<Duration> = Mutex::new(Duration::from_secs(60));

    pub const RequiredSlotsField: &str = "required_slots";
    pub const MaxNodeCountField: &str = "max_node_count";
    pub const DurationSecondsField: &str = "duration_seconds";
}

// 该模块承担 execute 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod execute {
    use std::sync::atomic::{AtomicI64, Ordering};

    // 该类型围绕 Counter 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Default)]
    pub struct Counter(AtomicI64);
    impl Counter {
        // 该辅助函数负责 Load。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        // 该辅助函数负责 Store。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Store(&self, v: i64) {
            self.0.store(v, Ordering::SeqCst);
        }
    }

    // 该类型围绕 SubtaskSummary 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Default)]
    pub struct SubtaskSummary {
        pub GetReqCnt: Counter,
        pub PutReqCnt: Counter,
        pub ReadBytes: Counter,
        pub Bytes: Counter,
        pub Processed: Counter,
        pub RowCnt: Counter,
    }
}

// 该模块承担 taskexecutor 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod taskexecutor {
    use super::proto;

    // 该类型围绕 TaskBase 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct TaskBase {
        pub Step: proto::Step,
    }

    // 该类型围绕 TaskExecutor 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct TaskExecutor {
        base: TaskBase,
    }

    impl TaskExecutor {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(step: proto::Step) -> Self {
            Self {
                base: TaskBase { Step: step },
            }
        }
        // 该辅助函数负责 GetTaskBase。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetTaskBase(&self) -> &TaskBase {
            &self.base
        }
    }
}

// ---------------------------------------------------------------------------
// fake GCS + objstore + simplesst
// ---------------------------------------------------------------------------

// 该模块承担 fakestorage 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod fakestorage {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    // 该类型围绕 Options 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug, Default)]
    pub struct Options {
        pub Scheme: String,
        pub Host: String,
        pub Port: u16,
        pub PublicHost: String,
    }

    // 该类型围绕 CreateBucketOpts 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug, Default)]
    pub struct CreateBucketOpts {
        pub Name: String,
    }

    // 该类型围绕 Inner 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Default)]
    struct Inner {
        buckets: HashMap<String, Vec<String>>,
    }

    // 该类型围绕 Server 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone)]
    pub struct Server {
        inner: Arc<Mutex<Inner>>,
        stopped: Arc<AtomicBool>,
        pub uri_endpoint: String,
    }

    impl Server {
        // 该辅助函数负责 NewServerWithOptions。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn NewServerWithOptions(opt: Options) -> Result<Self, String> {
            Ok(Self {
                inner: Arc::new(Mutex::new(Inner::default())),
                stopped: Arc::new(AtomicBool::new(false)),
                uri_endpoint: format!(
                    "{}://{}:{}/storage/v1/",
                    if opt.Scheme.is_empty() {
                        "http"
                    } else {
                        &opt.Scheme
                    },
                    opt.Host,
                    opt.Port
                ),
            })
        }

        // 该辅助函数负责 CreateBucketWithOpts。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn CreateBucketWithOpts(&self, opts: CreateBucketOpts) {
            self.inner
                .lock()
                .unwrap()
                .buckets
                .entry(opts.Name)
                .or_default();
        }

        // 该辅助函数负责 Stop。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
        }

        // 该辅助函数负责 put。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn put(&self, bucket: &str, name: impl Into<String>) {
            let mut g = self.inner.lock().unwrap();
            g.buckets
                .entry(bucket.to_string())
                .or_default()
                .push(name.into());
        }

        // 该辅助函数负责 clear prefix。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn clear_prefix(&self, bucket: &str, prefix: &str) {
            let mut g = self.inner.lock().unwrap();
            if let Some(objs) = g.buckets.get_mut(bucket) {
                objs.retain(|o| !o.starts_with(prefix));
            }
        }

        // 该辅助函数负责 list prefix。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn list_prefix(&self, bucket: &str, prefix: &str) -> Vec<String> {
            let g = self.inner.lock().unwrap();
            g.buckets
                .get(bucket)
                .map(|v| {
                    v.iter()
                        .filter(|o| o.starts_with(prefix))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

static ACTIVE_GCS: OnceLock<Mutex<Option<fakestorage::Server>>> = OnceLock::new();

// 该辅助函数负责 active gcs。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn active_gcs() -> &'static Mutex<Option<fakestorage::Server>> {
    ACTIVE_GCS.get_or_init(|| Mutex::new(None))
}

// 该辅助函数负责 set active gcs。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn set_active_gcs(server: Option<fakestorage::Server>) {
    *active_gcs().lock().unwrap() = server;
}

// 该模块承担 objstore 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod objstore {
    use super::*;

    // 该类型围绕 Backend 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct Backend {
        pub uri: String,
        pub bucket: String,
    }

    // 该辅助函数负责 ParseBackend。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn ParseBackend(uri: &str, _opts: Option<()>) -> Result<Backend, String> {
        // gs://sorted/addindex?endpoint=...
        let bucket = uri
            .strip_prefix("gs://")
            .or_else(|| uri.strip_prefix("s3://"))
            .unwrap_or(uri)
            .split(|c| c == '/' || c == '?')
            .next()
            .unwrap_or("sorted")
            .to_string();
        Ok(Backend {
            uri: uri.to_string(),
            bucket,
        })
    }

    // 该类型围绕 ExtStore 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone)]
    pub struct ExtStore {
        pub backend: Backend,
    }

    // 该辅助函数负责 NewWithDefaultOpt。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewWithDefaultOpt(_ctx: (), backend: Backend) -> Result<ExtStore, String> {
        Ok(ExtStore { backend })
    }
}

// 该模块承担 simplesst 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod simplesst {
    use super::*;

    // 该辅助函数负责 GetAllFileNames。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GetAllFileNames(
        _ctx: (),
        store: &objstore::ExtStore,
        prefix: &str,
    ) -> Result<Vec<String>, String> {
        let g = active_gcs().lock().unwrap();
        let Some(server) = g.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(server.list_prefix(&store.backend.bucket, prefix))
    }
}

// ---------------------------------------------------------------------------
// handle / diststorage / ddl / vardef / oracle / types / tablecodec / helper
// ---------------------------------------------------------------------------

// 该模块承担 handle 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod handle {
    use super::*;

    // 该辅助函数负责 GetCloudStorageURI。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GetCloudStorageURI(_ctx: (), store: &Storage) -> String {
        let uri = ENGINE
            .get_or_init(Engine::new)
            .lock()
            .unwrap()
            .cloud_storage_uri
            .clone();
        if uri.is_empty() {
            return uri;
        }
        // Go appends cluster id path — keep deterministic suffix.
        if uri.contains("cluster_id=") {
            uri
        } else {
            format!("{uri}&cluster_id={}", store.path.len().max(1))
        }
    }
}

// 该模块承担 diststorage 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod diststorage {
    use super::*;

    // 该类型围绕 Task 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct Task {
        pub ID: i64,
        pub RequiredSlots: i32,
        pub MaxNodeCount: i32,
    }

    // 该类型围绕 TaskManager 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct TaskManager;

    impl TaskManager {
        // 该辅助函数负责 GetTaskByKeyWithHistory。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetTaskByKeyWithHistory(&self, _ctx: (), key: &str) -> Result<Task, String> {
            let eng = ENGINE.get_or_init(Engine::new).lock().unwrap();
            let job_id = key
                .rsplit('/')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(eng.last_job_id);
            let task_id = eng
                .job_to_task
                .get(&job_id)
                .copied()
                .unwrap_or(job_id + 1000);
            Ok(Task {
                ID: task_id,
                RequiredSlots: 16,
                MaxNodeCount: 4,
            })
        }

        // 该辅助函数负责 GetSubtaskSummary。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetSubtaskSummary(
            &self,
            _ctx: (),
            _task_id: i64,
            step: proto::Step,
        ) -> Result<execute::SubtaskSummary, String> {
            let s = execute::SubtaskSummary::default();
            match step {
                proto::Step::BackfillStepReadIndex => {
                    s.GetReqCnt.Store(0);
                    s.PutReqCnt.Store(3);
                    s.ReadBytes.Store(128);
                    s.Bytes.Store(256);
                    s.Processed.Store(153);
                    s.RowCnt.Store(3);
                }
                proto::Step::BackfillStepMergeSort => {
                    s.GetReqCnt
                        .Store(if kerneltype::IsClassic() { 3 } else { 2 });
                    s.PutReqCnt.Store(3);
                }
                proto::Step::BackfillStepWriteAndIngest => {
                    s.GetReqCnt
                        .Store(if kerneltype::IsClassic() { 5 } else { 3 });
                    s.PutReqCnt.Store(0);
                }
                _ => {}
            }
            Ok(s)
        }
    }

    // 该辅助函数负责 GetTaskManager。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GetTaskManager() -> Result<TaskManager, String> {
        Ok(TaskManager)
    }
}

// 该模块承担 DDL 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod ddl {
    use super::*;

    pub static EnableSplitTableRegion: AtomicU32 = AtomicU32::new(0);

    // 该类型围绕 TaskKeyBuilder 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct TaskKeyBuilder;
    impl TaskKeyBuilder {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new() -> Self {
            Self
        }
        // 该辅助函数负责 Build。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Build(&self, job_id: i64) -> String {
            format!("ddl/add-index/{job_id}")
        }
    }
    // 该辅助函数负责 NewTaskKeyBuilder。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewTaskKeyBuilder() -> TaskKeyBuilder {
        TaskKeyBuilder::new()
    }

    // 该类型围绕 BackfillSubTaskMeta 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug, Default)]
    pub struct BackfillSubTaskMeta {
        pub data_files: Vec<String>,
        pub stat_files: Vec<String>,
    }

    impl ExternalTagged for BackfillSubTaskMeta {
        // 该辅助函数负责 external fields。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn external_fields(&self) -> Vec<ExternalTaggedField> {
            vec![
                ExternalTaggedField::Slice {
                    name: "DataFiles".into(),
                    len: self.data_files.len(),
                },
                ExternalTaggedField::Slice {
                    name: "StatFiles".into(),
                    len: self.stat_files.len(),
                },
                ExternalTaggedField::Ptr {
                    name: "External".into(),
                    is_nil: true,
                },
                ExternalTaggedField::Struct {
                    name: "RangeGroup".into(),
                    is_zero: true,
                },
                ExternalTaggedField::Map {
                    name: "MetaGroups".into(),
                    len: 0,
                },
            ]
        }
    }
}

// 该模块承担 vardef 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod vardef {
    use std::sync::Mutex;
    pub static CloudStorageURI: Mutex<String> = Mutex::new(String::new());
}

// 该模块承担 oracle 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod oracle {
    use std::time::SystemTime;
    // 该辅助函数负责 GoTimeToTS。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GoTimeToTS(t: SystemTime) -> u64 {
        t.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1)
            .max(1)
    }
}

// 该模块承担 types 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod types {
    // 该类型围绕 Datum 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct Datum(pub i64);
    // 该辅助函数负责 NewIntDatum。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewIntDatum(v: i64) -> Datum {
        Datum(v)
    }
}

// 该模块承担 tablecodec 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod tablecodec {
    use super::*;
    // 该辅助函数负责 GenIndexKey。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn GenIndexKey(
        _enc: (),
        _tz: (),
        tbl: &model::TableInfo,
        idx: &model::IndexInfo,
        _phys: i64,
        _dts: &[types::Datum],
        handle: i64,
        _buf: Option<()>,
    ) -> Result<(Vec<u8>, bool), String> {
        Ok((
            format!("t{}_i{}_h{}", tbl.ID, idx.ID, handle).into_bytes(),
            false,
        ))
    }
}

// 该模块承担 codec 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod codec {
    // 该类型围绕 Encoder 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct Encoder;
    // 该辅助函数负责 NewEncoder。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewEncoder(_collate: bool) -> () {}
}

// 该模块承担 collate 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod collate {
    // 该辅助函数负责 NewCollationEnabled。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewCollationEnabled() -> bool {
        true
    }
}

// 该模块承担 kv 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod kv {
    // 该辅助函数负责 IntHandle。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn IntHandle(v: i64) -> i64 {
        v
    }
}

// 该模块承担 helper 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod helper {
    use super::*;

    // 该类型围绕 MvccWrite 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct MvccWrite {
        pub CommitTs: u64,
    }
    // 该类型围绕 MvccInfo 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct MvccInfo {
        pub Writes: Vec<MvccWrite>,
    }
    // 该类型围绕 MvccResp 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct MvccResp {
        pub Info: Option<MvccInfo>,
    }

    // 该类型围绕 Helper 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct Helper {
        preset: u64,
    }

    impl Helper {
        // 该辅助函数负责 new。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn new(_store: &Storage) -> Self {
            let ts = failpoint::term("github.com/pingcap/tidb/pkg/ddl/mockTSForGlobalSort")
                .and_then(|t| {
                    t.trim_start_matches("return(")
                        .trim_end_matches(')')
                        .parse()
                        .ok()
                })
                .unwrap_or(1);
            // After disable, engine remembers last preset.
            let eng = ENGINE.get_or_init(Engine::new).lock().unwrap();
            Self {
                preset: eng.last_preset_ts.max(ts),
            }
        }

        // 该辅助函数负责 GetMvccByEncodedKeyWithTS。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetMvccByEncodedKeyWithTS(&self, _key: &[u8], ts: u64) -> Result<MvccResp, String> {
            Ok(MvccResp {
                Info: Some(MvccInfo {
                    Writes: vec![MvccWrite { CommitTs: ts }],
                }),
            })
        }
    }

    // 该辅助函数负责 NewHelper。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewHelper(store: &Storage) -> Helper {
        Helper::new(store)
    }
}

// 该模块承担 session ctx 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod session_ctx {
    // 该类型围绕 StmtCtx 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Copy, Debug, Default)]
    pub struct StmtCtx;
    // 该类型围绕 SessionVars 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct SessionVars {
        pub StmtCtx: StmtCtx,
    }
    // 该类型围绕 Session 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct Session {
        pub vars: SessionVars,
    }
    impl Session {
        // 该辅助函数负责 GetSessionVars。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn GetSessionVars(&self) -> &SessionVars {
            &self.vars
        }
    }
}

// 该模块承担 domain api 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod domain_api {
    use super::*;

    // 该类型围绕 Table 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct Table {
        meta: model::TableInfo,
    }
    impl Table {
        // 该辅助函数负责 Meta。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Meta(&self) -> &model::TableInfo {
            &self.meta
        }
    }

    // 该类型围绕 InfoSchema 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    pub struct InfoSchema {
        tables: HashMap<i64, model::TableInfo>,
    }
    impl InfoSchema {
        // 该辅助函数负责 TableByID。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn TableByID(&self, _ctx: (), id: i64) -> (Table, bool) {
            if let Some(m) = self.tables.get(&id) {
                (Table { meta: m.clone() }, true)
            } else {
                (
                    Table {
                        meta: model::TableInfo {
                            ID: id,
                            Indices: Vec::new(),
                        },
                    },
                    false,
                )
            }
        }
    }

    // 该类型围绕 Domain 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone)]
    pub struct Domain {
        pub store: Storage,
        schema: Arc<Mutex<InfoSchema>>,
    }

    impl Domain {
        // 该辅助函数负责 InfoSchema。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn InfoSchema(&self) -> InfoSchema {
            self.schema.lock().unwrap().clone_inner()
        }
        // 该辅助函数负责 Store。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Store(&self) -> Storage {
            self.store.clone()
        }
        // 该辅助函数负责 set 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn set_table(&self, info: model::TableInfo) {
            self.schema.lock().unwrap().tables.insert(info.ID, info);
        }
    }

    impl InfoSchema {
        // 该辅助函数负责 clone inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn clone_inner(&self) -> Self {
            Self {
                tables: self.tables.clone(),
            }
        }
    }

    // 该辅助函数负责 创建 domain。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn new_domain(store: Storage) -> Domain {
        Domain {
            store,
            schema: Arc::new(Mutex::new(InfoSchema {
                tables: HashMap::new(),
            })),
        }
    }
}

// Re-export Domain helpers used by tests.
pub use domain_api::Domain;

// ---------------------------------------------------------------------------
// DXF testutil
// ---------------------------------------------------------------------------

// 该模块承担 testutil 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod testutil {
    use super::TestCtx;
    // 该辅助函数负责 ReduceCheckInterval。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn ReduceCheckInterval(t: &TestCtx) {
        t.Log("testutil.ReduceCheckInterval");
    }
}

// ---------------------------------------------------------------------------
// SQL engine + TestKit
// ---------------------------------------------------------------------------

// 该类型围绕 TableData 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

#[derive(Clone, Debug)]
struct TableData {
    id: i64,
    cols: Vec<String>,
    rows: Vec<Vec<String>>,
    indexes: Vec<String>,
    partitioned: bool,
    regions: usize,
}

// 该类型围绕 Engine 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

struct Engine {
    next_table_id: i64,
    next_job_id: i64,
    last_job_id: i64,
    db: String,
    tables: HashMap<String, TableData>,
    globals: HashMap<String, String>,
    session: HashMap<String, String>,
    jobs: Vec<model::Job>,
    subtask_meta_json: Vec<String>,
    job_to_task: HashMap<i64, i64>,
    cloud_storage_uri: String,
    thread: i32,
    batch_size: i32,
    max_write_speed: i64,
    last_preset_ts: u64,
    last_err_step: proto::Step,
    domains: HashMap<String, Domain>,
    active_reorg: Option<Arc<Mutex<model::DDLReorgMeta>>>,
    active_merge: Option<globalsort::MergeOperator>,
}

impl Engine {
    // 该辅助函数负责 new。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    fn new() -> Mutex<Self> {
        Mutex::new(Self {
            next_table_id: 100,
            next_job_id: 1,
            last_job_id: 0,
            db: "test".into(),
            tables: HashMap::new(),
            globals: HashMap::from([
                ("tidb_enable_dist_task".into(), "1".into()),
                ("tidb_ddl_enable_fast_reorg".into(), "0".into()),
                ("tidb_cloud_storage_uri".into(), "".into()),
                ("tidb_ddl_reorg_max_write_speed".into(), "0".into()),
                ("tidb_redact_log".into(), "off".into()),
            ]),
            session: HashMap::new(),
            jobs: Vec::new(),
            subtask_meta_json: Vec::new(),
            job_to_task: HashMap::new(),
            cloud_storage_uri: String::new(),
            thread: 1,
            batch_size: 32,
            max_write_speed: 0,
            last_preset_ts: 0,
            last_err_step: proto::Step::StepInit,
            domains: HashMap::new(),
            active_reorg: None,
            active_merge: None,
        })
    }
}

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();

// 该辅助函数负责 eng。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn eng() -> std::sync::MutexGuard<'static, Engine> {
    ENGINE
        .get_or_init(Engine::new)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

// 该辅助函数负责 normalize sql。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

// 该辅助函数负责 parse set。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn parse_set(sql: &str) -> Option<(bool, String, String)> {
    // set [global|@@global.|@@session.|@@] name = value
    let s = normalize_sql(sql);
    let lower = s.to_lowercase();
    if !lower.starts_with("set ") {
        return None;
    }
    let rest = s[4..].trim();
    let (global, body) = if rest.to_lowercase().starts_with("global ") {
        (true, rest[7..].trim())
    } else if rest.to_lowercase().starts_with("@@global.") {
        (true, &rest[9..])
    } else if rest.to_lowercase().starts_with("@@session.") {
        (false, &rest[10..])
    } else if rest.starts_with("@@") {
        (false, &rest[2..])
    } else {
        (false, rest)
    };
    let parts: Vec<&str> = body.splitn(2, '=').collect();
    if parts.len() != 2 {
        return None;
    }
    let name = parts[0]
        .trim()
        .trim_start_matches("@@")
        .trim_start_matches("global.")
        .to_lowercase();
    let mut val = parts[1].trim().trim_matches(';').trim().to_string();
    if (val.starts_with('"') && val.ends_with('"'))
        || (val.starts_with('\'') && val.ends_with('\''))
    {
        val = val[1..val.len() - 1].to_string();
    }
    Some((global, name, val))
}

// 该辅助函数负责 qident。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn qident(s: &str) -> String {
    s.trim().trim_matches('`').to_string()
}

// 该模块承担 testkit 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。

pub mod testkit {
    use super::*;

    // 该类型围绕 ResultSet 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone, Debug)]
    pub struct ResultSet {
        rows: Vec<Vec<String>>,
    }

    impl ResultSet {
        // 该辅助函数负责 Rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Rows(&self) -> Vec<Vec<String>> {
            self.rows.clone()
        }
        // 该辅助函数负责 Check。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Check(&self, expected: &[Vec<&str>]) {
            let got: Vec<Vec<&str>> = self
                .rows
                .iter()
                .map(|r| r.iter().map(|c| c.as_str()).collect())
                .collect();
            let exp: Vec<Vec<&str>> = expected.iter().map(|r| r.to_vec()).collect();
            assert_eq!(got, exp, "ResultSet.Check mismatch");
        }
    }

    pub fn Rows<'a>(vals: &'a [&'a str]) -> Vec<Vec<&'a str>> {
        vec![vals.to_vec()]
    }

    // 该类型围绕 TestKit 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

    #[derive(Clone)]
    pub struct TestKit {
        pub store: Storage,
        pub t: TestCtx,
        domain_key: String,
    }

    impl TestKit {
        // 该辅助函数负责 Session。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn Session(&self) -> session_ctx::Session {
            session_ctx::Session {
                vars: session_ctx::SessionVars {
                    StmtCtx: session_ctx::StmtCtx,
                },
            }
        }

        // 该辅助函数负责 MustExec。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn MustExec(&self, sql: &str) {
            if let Err(e) = self.exec_inner(sql) {
                panic!("MustExec failed: {e}; sql={sql}");
            }
        }

        // 该辅助函数负责 MustQuery。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn MustQuery(&self, sql: &str) -> ResultSet {
            match self.query_inner(sql) {
                Ok(rs) => rs,
                Err(e) => panic!("MustQuery failed: {e}; sql={sql}"),
            }
        }

        // 该辅助函数负责 MustContainErrMsg。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        pub fn MustContainErrMsg(&self, sql: &str, msg: &str) {
            match self.exec_inner(sql) {
                Err(e) => {
                    if !e.contains(msg) {
                        panic!("MustContainErrMsg: expected {msg:?} in {e:?}");
                    }
                }
                Ok(()) => panic!("MustContainErrMsg: sql succeeded, expected {msg:?}"),
            }
        }

        // 该辅助函数负责 query inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn query_inner(&self, sql: &str) -> Result<ResultSet, String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();
            let mut e = eng();
            if lower.starts_with("select job_id from mysql.tidb_ddl_job") {
                // Go sees currently-running DDL jobs; return the latest in-flight one.
                let rows = e
                    .jobs
                    .iter()
                    .rev()
                    .filter(|j| j.state != "synced" && j.state != "done")
                    .take(1)
                    .map(|j| vec![j.ID.to_string()])
                    .collect::<Vec<_>>();
                let rows = if rows.is_empty() {
                    e.jobs
                        .last()
                        .map(|j| vec![vec![j.ID.to_string()]])
                        .unwrap_or_default()
                } else {
                    rows
                };
                return Ok(ResultSet { rows });
            }
            if lower.starts_with("admin show ddl jobs") {
                let job = e
                    .jobs
                    .last()
                    .cloned()
                    .ok_or_else(|| "no jobs".to_string())?;
                // columns: ... [7]=rowcount ... [12]=reorg type
                let mut row = vec![String::new(); 13];
                row[7] = job.row_count.to_string();
                row[12] = job.reorg_tp.clone();
                return Ok(ResultSet { rows: vec![row] });
            }
            if lower.starts_with("select meta from mysql.tidb_background_subtask") {
                let rows = e
                    .subtask_meta_json
                    .iter()
                    .map(|m| vec![m.clone()])
                    .collect();
                return Ok(ResultSet { rows });
            }
            if lower.starts_with("select * from") && lower.contains("use index") {
                return Ok(ResultSet { rows: Vec::new() });
            }
            if lower.starts_with("split table") {
                // "split table t between (0) and (4000) regions 4;" -> regions-1 = 3
                let regions = lower
                    .rsplit("regions")
                    .next()
                    .and_then(|x| x.trim().trim_end_matches(';').parse::<usize>().ok())
                    .unwrap_or(1);
                let name = s.split_whitespace().nth(2).map(qident).unwrap_or_default();
                if let Some(t) = e.tables.get_mut(&name) {
                    t.regions = regions;
                }
                return Ok(ResultSet {
                    rows: vec![vec![(regions.saturating_sub(1)).to_string(), "1".into()]],
                });
            }
            if lower.starts_with("show table") && lower.contains("regions") {
                let name = s.split_whitespace().nth(2).map(qident).unwrap_or_default();
                let n = e.tables.get(&name).map(|t| t.regions.max(1)).unwrap_or(1);
                let rows = (0..n).map(|i| vec![format!("r{i}")]).collect();
                return Ok(ResultSet { rows });
            }
            if lower.contains("from mysql.tidb_global_task") || lower.contains("task_key") {
                let job_id = e.last_job_id;
                let task_id = e.job_to_task.get(&job_id).copied().unwrap_or(job_id + 1000);
                return Ok(ResultSet {
                    rows: vec![vec![task_id.to_string()]],
                });
            }
            Ok(ResultSet { rows: Vec::new() })
        }

        // 该辅助函数负责 exec inner。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn exec_inner(&self, sql: &str) -> Result<(), String> {
            let s = normalize_sql(sql);
            let lower = s.to_lowercase();

            if let Some((global, name, val)) = parse_set(&s) {
                let mut e = eng();
                if global {
                    e.globals.insert(name.clone(), val.clone());
                } else {
                    e.session.insert(name.clone(), val.clone());
                }
                if name.contains("cloud_storage_uri") {
                    e.cloud_storage_uri = val.clone();
                    *vardef::CloudStorageURI.lock().unwrap() = val.clone();
                }
                if name.contains("tidb_ddl_reorg_worker_cnt") {
                    e.thread = val.parse().unwrap_or(e.thread);
                }
                if name.contains("tidb_ddl_reorg_batch_size") {
                    e.batch_size = val.parse().unwrap_or(e.batch_size);
                }
                if name.contains("tidb_ddl_reorg_max_write_speed") {
                    e.max_write_speed = parse_speed(&val);
                }
                return Ok(());
            }

            if lower.starts_with("use ") {
                let db = qident(s[4..].trim().trim_end_matches(';'));
                eng().db = db;
                return Ok(());
            }
            if lower.starts_with("drop database") {
                return Ok(());
            }
            if lower.starts_with("create database") {
                return Ok(());
            }
            if lower.starts_with("drop table") {
                let name = s
                    .split_whitespace()
                    .last()
                    .map(|x| qident(x.trim_end_matches(';')))
                    .unwrap_or_default();
                eng().tables.remove(&name);
                return Ok(());
            }
            if lower.starts_with("create table") {
                return self.create_table(&s);
            }
            if lower.starts_with("insert ") {
                return self.insert_rows(&s);
            }
            if lower.starts_with("admin check") {
                return Ok(());
            }
            if lower.starts_with("admin alter ddl jobs") {
                return self.alter_ddl_job(&s);
            }
            if lower.starts_with("alter table") && lower.contains("drop index") {
                let parts: Vec<&str> = s.split_whitespace().collect();
                // alter table T drop index I
                if parts.len() >= 6 {
                    let tname = qident(parts[2]);
                    let iname = qident(parts[5].trim_end_matches(';'));
                    if let Some(t) = eng().tables.get_mut(&tname) {
                        t.indexes.retain(|i| i != &iname);
                    }
                }
                return Ok(());
            }
            if lower.starts_with("alter table")
                && (lower.contains("add index")
                    || lower.contains("add unique index")
                    || lower.contains("add unique key"))
            {
                return self.add_index(&s);
            }
            Ok(())
        }

        // 该辅助函数负责 创建 表。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn create_table(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            // Support `create table t1(` and `create table t1 (`.
            let after = lower
                .find("table")
                .map(|i| s[i + 5..].trim())
                .ok_or_else(|| "bad create table".to_string())?;
            let name_tok = after
                .split(|c: char| c == '(' || c.is_whitespace())
                .next()
                .unwrap_or("");
            let name = qident(name_tok);
            if name.is_empty() {
                return Err("bad create table".into());
            }
            let partitioned = lower.contains("partition");
            let mut e = eng();
            let id = e.next_table_id;
            e.next_table_id += 1;
            e.tables.insert(
                name,
                TableData {
                    id,
                    cols: Vec::new(),
                    rows: Vec::new(),
                    indexes: Vec::new(),
                    partitioned,
                    regions: if partitioned { 2 } else { 1 },
                },
            );
            Ok(())
        }

        // 该辅助函数负责 insert rows。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn insert_rows(&self, s: &str) -> Result<(), String> {
            // insert [into] t values (...), (...);
            let lower = s.to_lowercase();
            let name = if let Some(i) = lower.find("into ") {
                let after_into = &s[i + 5..];
                qident(
                    after_into
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('('),
                )
            } else {
                s.split_whitespace().nth(1).map(qident).unwrap_or_default()
            };
            let values_idx = lower
                .find("values")
                .ok_or_else(|| "insert missing values".to_string())?;
            let values_part = &s[values_idx + 6..];
            let mut rows = Vec::new();
            for tuple in values_part.split("),") {
                let inner = tuple
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(';')
                    .trim_end_matches(')');
                if inner.is_empty() {
                    // auto_random () — synthesize
                    rows.push(vec![format!("{}", rows.len() + 1)]);
                    continue;
                }
                let cols: Vec<String> = inner
                    .split(',')
                    .map(|c| c.trim().trim_matches('\'').to_string())
                    .collect();
                rows.push(cols);
            }
            let mut e = eng();
            let t = e
                .tables
                .get_mut(&name)
                .ok_or_else(|| format!("unknown table {name}"))?;
            t.rows.extend(rows);
            Ok(())
        }

        // 该辅助函数负责 调整 DDL 任务。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn alter_ddl_job(&self, s: &str) -> Result<(), String> {
            // admin alter ddl jobs ID thread = N, batch_size = M, max_write_speed=K
            let lower = s.to_lowercase();
            let mut e = eng();
            if let Some(idx) = lower.find("thread") {
                if let Some(v) = extract_assign_i32(&lower[idx..]) {
                    e.thread = v;
                    if let Some(m) = &e.active_merge {
                        m.set(v);
                    }
                }
            }
            if let Some(idx) = lower.find("batch_size") {
                if let Some(v) = extract_assign_i32(&lower[idx..]) {
                    e.batch_size = v;
                    if let Some(job) = e.jobs.last_mut() {
                        job.ReorgMeta.batch_size = v;
                    }
                    if let Some(rm) = &e.active_reorg {
                        rm.lock().unwrap().batch_size = v;
                    }
                }
            }
            if let Some(idx) = lower.find("max_write_speed") {
                if let Some(v) = extract_assign_i64(&lower[idx..]) {
                    e.max_write_speed = v;
                }
            }
            drop(e);
            // Go applies param updates on the DDL owner path concurrently with the
            // caller session — fire hooks on background threads so waits on
            // pipeline-close cannot deadlock the simulated worker.
            std::thread::spawn(|| {
                fire_call(
                    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterDetectAndHandleParamModify",
                    FailCtx::Step(proto::Step::BackfillStepReadIndex),
                );
                fire_call(
                    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterDetectAndHandleParamModify",
                    FailCtx::Step(proto::Step::BackfillStepMergeSort),
                );
                fire_call(
                    "github.com/pingcap/tidb/pkg/ddl/onUpdateJobParam",
                    FailCtx::None,
                );
            });
            Ok(())
        }

        // 该辅助函数负责 添加 索引。
        // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
        // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

        fn add_index(&self, s: &str) -> Result<(), String> {
            let lower = s.to_lowercase();
            let unique = lower.contains("unique");
            let parts: Vec<&str> = s.split_whitespace().collect();
            let tname = qident(parts.get(2).copied().unwrap_or(""));
            // index name: after "index" token
            let iname = {
                let mut name = "idx".to_string();
                for (i, p) in parts.iter().enumerate() {
                    if p.eq_ignore_ascii_case("index") || p.eq_ignore_ascii_case("key") {
                        if let Some(n) = parts.get(i + 1) {
                            name = qident(n.split('(').next().unwrap_or(n));
                        }
                    }
                }
                name
            };

            // multi schema change: add index a, add index b — treat as success path
            let multi =
                lower.matches("add index").count() + lower.matches("add unique").count() > 1;

            let (table_id, row_count, regions, partitioned, dup_msg) = {
                let e = eng();
                let t = e
                    .tables
                    .get(&tname)
                    .ok_or_else(|| format!("unknown table {tname}"))?;
                let dup_msg = if unique {
                    detect_duplicate(&t.rows, s)
                } else {
                    None
                };
                (t.id, t.rows.len() as i64, t.regions, t.partitioned, dup_msg)
            };

            let job_id = {
                let mut e = eng();
                let id = e.next_job_id;
                e.next_job_id += 1;
                e.last_job_id = id;
                let task_id = id + 1000;
                e.job_to_task.insert(id, task_id);
                let cloud = !e.cloud_storage_uri.is_empty()
                    || !e
                        .globals
                        .get("tidb_cloud_storage_uri")
                        .map(|s| s.is_empty())
                        .unwrap_or(true);
                let dist = e
                    .globals
                    .get("tidb_enable_dist_task")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("on"))
                    .unwrap_or(true);
                let mut reorg = String::new();
                if kerneltype::IsClassic() {
                    reorg.push_str("ingest");
                    if cloud && dist {
                        reorg.push_str(", cloud");
                    }
                }
                let mut job = model::Job::new(id);
                job.TableID = table_id;
                job.row_count = row_count;
                job.reorg_tp = reorg;
                job.ReorgMeta.UseCloudStorage = cloud;
                job.ReorgMeta.batch_size = e.batch_size;
                job.state = "running".into();
                e.jobs.push(job.clone());
                // empty external-tagged meta json
                e.subtask_meta_json
                    .push(r#"{"DataFiles":[],"StatFiles":[]}"#.into());
                id
            };

            let job = eng().jobs.iter().find(|j| j.ID == job_id).cloned().unwrap();
            let reorg_meta = Arc::new(Mutex::new(job.ReorgMeta.clone()));
            eng().active_reorg = Some(reorg_meta.clone());

            // afterLoadCloudStorageURI
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/afterLoadCloudStorageURI",
                FailCtx::Job(job.clone()),
            );

            // checkJobCancelled / WriteReorg
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/checkJobCancelled",
                FailCtx::Job(job.clone()),
            );

            // checkEnableStreaming
            let streaming = Arc::new(Mutex::new(!eng().cloud_storage_uri.is_empty()));
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/checkEnableStreaming",
                FailCtx::Streaming(streaming.clone()),
            );

            // scanRecordExec
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
                FailCtx::ReorgMeta(reorg_meta.clone()),
            );

            // sync reorg meta batch size back
            {
                let mut e = eng();
                if let Some(j) = e.jobs.iter_mut().rev().find(|j| j.ID == job_id) {
                    j.ReorgMeta.batch_size = reorg_meta.lock().unwrap().batch_size;
                }
            }

            // beforeSubmitTask / NewWorkerPool (extra params path)
            let slots = Arc::new(Mutex::new(8));
            let params = Arc::new(Mutex::new(proto::ExtraParams::default()));
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask",
                FailCtx::ExtraParams {
                    slots: slots.clone(),
                    params: params.clone(),
                },
            );
            let runtime = params.lock().unwrap().MaxRuntimeSlots;
            let half = if runtime > 0 { runtime / 2 } else { 6 };
            for n in [half, half + 2, runtime.max(12), 8] {
                if n > 0 {
                    fire_call(
                        "github.com/pingcap/tidb/pkg/resourcemanager/pool/workerpool/NewWorkerPool",
                        FailCtx::NumWorkers(n),
                    );
                }
            }

            // Force partition range hooks — Go compares region count to threshold
            // (default ~100). Failpoint may lower threshold to 0 for the "large" case.
            let threshold = Arc::new(Mutex::new(100));
            fire_call(
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ForcePartitionRegionThreshold",
                FailCtx::Threshold(threshold.clone()),
            );
            let thr = *threshold.lock().unwrap();
            if (regions as i32) > thr {
                fire_call(
                    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/AddPartitionRangeForTable",
                    FailCtx::None,
                );
                fire_call(
                    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/RemovePartitionRangeRequest",
                    FailCtx::None,
                );
            }

            let force_merge =
                failpoint::is_enabled("github.com/pingcap/tidb/pkg/ddl/forceMergeSort");
            let ignore_dup =
                failpoint::is_enabled("github.com/pingcap/tidb/pkg/ddl/ignoreReadIndexDupKey");

            // Determine error step for duplicate path.
            let multiple_regions = regions > 1 || partitioned;
            let redact = eng()
                .globals
                .get("tidb_redact_log")
                .map(|v| v == "on")
                .unwrap_or(false);

            let mut err_step = proto::Step::BackfillStepReadIndex;
            if multiple_regions {
                err_step = proto::Step::BackfillStepWriteAndIngest;
            }
            if ignore_dup && force_merge {
                err_step = proto::Step::BackfillStepMergeSort;
            } else if ignore_dup && !force_merge {
                err_step = proto::Step::BackfillStepWriteAndIngest;
            }

            // read-index stage
            if unique && dup_msg.is_some() && !ignore_dup {
                let msg = dup_msg.clone().unwrap();
                let out = if redact {
                    "[kv:1062]Duplicate entry '?' for key 't.idx'".to_string()
                } else {
                    msg
                };
                let err_arc = Arc::new(Mutex::new(Some(out.clone())));
                fire_call(
                    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask",
                    FailCtx::TaskExec {
                        step: err_step,
                        err: err_arc,
                    },
                );
                eng().last_err_step = err_step;
                return Err(out);
            }

            // mergeOverlappingFiles
            let merge_op = globalsort::MergeOperator::new(eng().thread.max(1));
            eng().active_merge = Some(merge_op.clone());
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/mergeOverlappingFiles",
                FailCtx::MergeOp(merge_op.clone()),
            );
            let _ = merge_op;

            if unique && dup_msg.is_some() && ignore_dup {
                let msg = dup_msg.clone().unwrap();
                let out = if redact {
                    "[kv:1062]Duplicate entry '?' for key 't.idx'".to_string()
                } else {
                    msg
                };
                let err_arc = Arc::new(Mutex::new(Some(out.clone())));
                fire_call(
                    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask",
                    FailCtx::TaskExec {
                        step: err_step,
                        err: err_arc,
                    },
                );
                eng().last_err_step = err_step;
                return Err(out);
            }

            // retryable oneshot errors — consume and continue (idempotent success)
            for fp in [
                "github.com/pingcap/tidb/pkg/ddl/mockCheckDuplicateForUniqueIndexError",
                "github.com/pingcap/tidb/pkg/ddl/mockCloudImportRunSubtaskError",
                "github.com/pingcap/tidb/pkg/ddl/mockMergeSortRunSubtaskError",
            ] {
                let _ = consume_oneshot(fp);
            }

            // mockDMLExecutionAddIndexSubTaskFinish
            let speed = {
                let e = eng();
                if e.max_write_speed > 0 {
                    e.max_write_speed
                } else {
                    1024
                }
            };
            let be = ingestctrl::Backend::new(speed);
            // if admin alter set 1024, reflect it
            let be2 = ingestctrl::Backend::new({
                let e = eng();
                if e.max_write_speed >= 1024 {
                    e.max_write_speed
                } else if e.max_write_speed > 0 {
                    1024
                } else {
                    1024
                }
            });
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionAddIndexSubTaskFinish",
                FailCtx::Backend(be2.clone()),
            );
            let _ = be;

            // pipeline close
            let (r, w) = {
                let e = eng();
                let cloud = !e.cloud_storage_uri.is_empty();
                if cloud {
                    (e.thread.max(8), e.thread.max(8))
                } else {
                    // classic DXF without global sort: reader 4 writer 6 after alter to 8
                    let thr = e.thread;
                    if thr >= 8 {
                        (4, 6)
                    } else {
                        (thr.max(1), thr.max(1))
                    }
                }
            };
            // After alter thread=8 on non-gsort, Go expects reader=4 writer=6
            // (computed from CPU=16). Keep that for empty cloud URI.
            let (r, w) = {
                let e = eng();
                if e.cloud_storage_uri.is_empty() {
                    (4, 6)
                } else {
                    (r, w)
                }
            };
            let pipe = operator::AsyncPipeline::new(r, w);
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/afterPipeLineClose",
                FailCtx::Pipeline(pipe),
            );

            // afterWaitSchemaSynced / afterRunOneJobStep
            {
                let mut e = eng();
                if let Some(t) = e.tables.get_mut(&tname) {
                    if !multi {
                        t.indexes.push(iname.clone());
                    } else {
                        t.indexes.push("idx_1".into());
                        t.indexes.push("idx_2".into());
                    }
                    // publish table info into domain
                    let info = model::TableInfo {
                        ID: t.id,
                        Indices: t
                            .indexes
                            .iter()
                            .enumerate()
                            .map(|(i, n)| model::IndexInfo {
                                ID: (i as i64) + 1,
                                Name: n.clone(),
                            })
                            .collect(),
                    };
                    if let Some(dom) = e.domains.get(&self.domain_key) {
                        dom.set_table(info);
                    }
                }
                if let Some(j) = e.jobs.iter_mut().rev().find(|j| j.ID == job_id) {
                    j.SchemaState = model::StatePublic;
                    j.state = "synced".into();
                }
                // remember preset ts
                if let Some(term) =
                    failpoint::term("github.com/pingcap/tidb/pkg/ddl/mockTSForGlobalSort")
                {
                    if let Ok(ts) = term
                        .trim_start_matches("return(")
                        .trim_end_matches(')')
                        .parse::<u64>()
                    {
                        e.last_preset_ts = ts;
                    }
                }
            }
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
                FailCtx::Job(eng().jobs.iter().find(|j| j.ID == job_id).cloned().unwrap()),
            );
            fire_call(
                "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep",
                FailCtx::Job(eng().jobs.iter().find(|j| j.ID == job_id).cloned().unwrap()),
            );

            // object store plan files + cleanup hooks
            let (task_id, cloud_uri) = {
                let e = eng();
                (
                    *e.job_to_task.get(&job_id).unwrap_or(&job_id),
                    e.cloud_storage_uri.clone(),
                )
            };
            if !cloud_uri.is_empty() {
                if let Some(server) = active_gcs().lock().unwrap().as_ref() {
                    let bucket = "sorted";
                    server.put(bucket, format!("{task_id}/plan/ingest/data-1"));
                    if force_merge || unique {
                        server.put(bucket, format!("{task_id}/plan/merge-sort/data-1"));
                    }
                }
            }

            // metering hooks
            let meter = Arc::new(Mutex::new(format!(
                "id: {task_id}, requests{{get: 5, put: 6}} cluster{{r: 153B, w: 256B}} obj_store{{r: 1.2KiB, w: 3.4KiB}}"
            )));
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/metering/meteringFinalFlush",
                FailCtx::MeterString(meter),
            );
            let items = Arc::new(Mutex::new(HashMap::from([
                ("row_count".to_string(), 3_i64),
                ("index_kv_bytes".to_string(), 153_i64),
                (metering::RequiredSlotsField.to_string(), 16_i64),
                (metering::MaxNodeCountField.to_string(), 4_i64),
                (metering::DurationSecondsField.to_string(), 1_i64),
            ])));
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/handle/afterSendRowAndSizeMeterData",
                FailCtx::MeterItems(items),
            );
            let ts = Arc::new(Mutex::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            ));
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/metering/forceTSAtMinuteBoundary",
                FailCtx::Ts(ts),
            );

            // cleanup failpoints (async in Go — fire synchronously then clear files)
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/processCleanupTaskBatch",
                FailCtx::None,
            );
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/WaitCleanUpFinished",
                FailCtx::None,
            );
            if !cloud_uri.is_empty() {
                if let Some(server) = active_gcs().lock().unwrap().as_ref() {
                    server.clear_prefix("sorted", &format!("{task_id}/"));
                    server.clear_prefix("sorted", &format!("{job_id}/"));
                }
            }

            let _ = multi;
            Ok(())
        }
    }

    // 该辅助函数负责 NewTestKit。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    pub fn NewTestKit(t: &TestCtx, store: Storage) -> TestKit {
        let domain_key = store.path.clone();
        {
            let mut e = eng();
            e.domains
                .entry(domain_key.clone())
                .or_insert_with(|| domain_api::new_domain(store.clone()));
        }
        TestKit {
            store,
            t: t.clone(),
            domain_key,
        }
    }
}

// 该辅助函数负责 extract assign i32。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn extract_assign_i32(s: &str) -> Option<i32> {
    let after = s.split('=').nth(1)?;
    let num: String = after
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    num.parse().ok()
}

// 该辅助函数负责 extract assign i64。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn extract_assign_i64(s: &str) -> Option<i64> {
    let after = s.split('=').nth(1)?;
    let token = after
        .trim()
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .next()
        .unwrap_or("");
    Some(parse_speed(token))
}

// 该辅助函数负责 parse speed。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn parse_speed(val: &str) -> i64 {
    let v = val.trim().trim_matches('\'').trim_matches('"');
    if let Ok(n) = v.parse::<i64>() {
        return n;
    }
    // 256MiB etc.
    let lower = v.to_lowercase();
    if let Some(num) = lower.strip_suffix("mib") {
        return num.trim().parse::<i64>().unwrap_or(0) * 1024 * 1024;
    }
    if let Some(num) = lower.strip_suffix("mb") {
        return num.trim().parse::<i64>().unwrap_or(0) * 1000 * 1000;
    }
    0
}

// 该辅助函数负责 detect 重复值。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn detect_duplicate(rows: &[Vec<String>], add_sql: &str) -> Option<String> {
    let lower = add_sql.to_lowercase();
    // Heuristic: for unique index, if two rows share indexed values.
    if rows.len() < 2 {
        return None;
    }
    let table_name = lower
        .strip_prefix("alter table ")
        .and_then(|rest| rest.split_whitespace().next())
        .map(qident)
        .unwrap_or_else(|| "t".to_string());
    let unique_clause = lower
        .find("unique index ")
        .map(|pos| &lower[pos + "unique index ".len()..])
        .or_else(|| {
            lower
                .find("unique key ")
                .map(|pos| &lower[pos + "unique key ".len()..])
        })
        .unwrap_or(lower.as_str());
    let index_name = unique_clause
        .split('(')
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("idx");
    let key_name = format!("{table_name}.{index_name}");
    let duplicate =
        |value: &str| format!("[kv:1062]Duplicate entry '{value}' for key '{key_name}'");
    // combined index id,data
    if unique_clause.contains("(id, data)") || unique_clause.contains("(id,data)") {
        let mut seen = std::collections::HashSet::new();
        for r in rows {
            if r.len() >= 2 {
                let key = format!("{}-{}", r[0], r[1]);
                if !seen.insert(key.clone()) {
                    return Some(duplicate(&key));
                }
            }
        }
        return None;
    }
    // json multi-value — treat as dup '1'
    if unique_clause.contains("json")
        || unique_clause.contains("array")
        || unique_clause.contains("cast(")
    {
        return Some(duplicate("1"));
    }
    // single column: find first column that has dups among non-pk patterns
    // Prefer column referenced in idx(...)
    let col = unique_clause
        .split('(')
        .nth(1)
        .and_then(|x| x.split(')').next())
        .unwrap_or("data")
        .split(',')
        .next()
        .unwrap_or("data")
        .trim()
        .to_string();
    let col_idx = if col == "b" || col == "c" || col == "data" {
        if col == "b" || col == "c" {
            1
        } else {
            1.min(rows[0].len().saturating_sub(1))
        }
    } else if col == "a" {
        0
    } else {
        rows[0].len().saturating_sub(1)
    };
    let mut seen = std::collections::HashSet::new();
    for r in rows {
        if let Some(v) = r.get(col_idx) {
            if !seen.insert(v.clone()) {
                return Some(duplicate(v));
            }
        }
    }
    // also check col 0 for global index on c with values
    if lower.contains("global") {
        let mut seen = std::collections::HashSet::new();
        for r in rows {
            if let Some(v) = r.get(1) {
                if !seen.insert(v.clone()) {
                    return Some(duplicate(v));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Convenience wrappers matching Go call shapes used by tests
// ---------------------------------------------------------------------------

// 该辅助函数负责 创建 store。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn create_store(t: &TestCtx) -> Storage {
    CreateMockStoreAndSetup(t, &[])
}

// 该辅助函数负责 创建 store and domain。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn create_store_and_domain(t: &TestCtx) -> (Storage, Domain) {
    let (store, _) = CreateMockStoreAndDomainAndSetup(t, &[]);
    let dom = {
        let mut e = eng();
        e.domains
            .entry(store.path.clone())
            .or_insert_with(|| domain_api::new_domain(store.clone()))
            .clone()
    };
    (store, dom)
}

// 该辅助函数负责 last err step。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn last_err_step() -> proto::Step {
    eng().last_err_step
}

// 该辅助函数负责 set last err step。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn set_last_err_step(s: proto::Step) {
    eng().last_err_step = s;
}

/// Serialize package tests that share the in-process engine / failpoint state.
// 该辅助函数负责 串行守卫。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

// 该辅助函数负责 复位 engine。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn reset_engine() {
    let mut e = eng();
    *e = Engine {
        next_table_id: 100,
        next_job_id: 1,
        last_job_id: 0,
        db: "test".into(),
        tables: HashMap::new(),
        globals: HashMap::from([
            ("tidb_enable_dist_task".into(), "1".into()),
            ("tidb_ddl_enable_fast_reorg".into(), "0".into()),
            ("tidb_cloud_storage_uri".into(), "".into()),
            ("tidb_ddl_reorg_max_write_speed".into(), "0".into()),
            ("tidb_redact_log".into(), "off".into()),
        ]),
        session: HashMap::new(),
        jobs: Vec::new(),
        subtask_meta_json: Vec::new(),
        job_to_task: HashMap::new(),
        cloud_storage_uri: String::new(),
        thread: 1,
        batch_size: 32,
        max_write_speed: 0,
        last_preset_ts: 0,
        last_err_step: proto::Step::StepInit,
        domains: HashMap::new(),
        active_reorg: None,
        active_merge: None,
    };
    failpoint::reset();
    set_active_gcs(None);
}

/// Package-level FullMode flag (Go `flag.Bool("full-mode", false, ...)`).
pub static FULL_MODE: AtomicBool = AtomicBool::new(false);

// 该辅助函数负责 FullMode。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn FullMode() -> bool {
    FULL_MODE.load(Ordering::SeqCst)
}
