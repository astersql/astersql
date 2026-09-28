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

//! 测试内 failpoint 有序编排器，移植自 Go `br/pkg/utiltest/syncpoint`。
//! Rust `fail` 仅支持零参回调，故 [`Step`] 固定为 `Fn()`（Go 用 `any`+reflect）。
//! 顺序放行、取消唤醒与注册生命周期语义与 Go 对齐。
//! 无活跃序列时命中直接忽略，避免泄漏到无关用例。
//! 同一 Script 可跨多次 BeginSeq/EndSeq 复用，registered 缓存跨序列保留。
//! Fatal 路径用 panic 模拟 Go t.Fatal，由测试 catch_unwind 断言。
//! Condvar 仅用于错序等待，EndSeq 本身不阻塞等待完成。
//! AfterFunc 在序列完成或 EndSeq 时 Stop，防止迟到取消污染成功结果。
//! prepare_step 在锁外注册，避免 EnableCall 与 advance 互相等待。
//! advance 返回回调后由 failpoint 线程在锁外执行；相邻回调可按 Go 语义并发。
//! Step 名称必须与 inject 使用同一全路径，短名不会自动展开。
//! 本模块仅编排测试同步，不包含生产备份/恢复业务逻辑。
//! 注册表以 failpoint 全名为键，重复 BeginSeq 同名步骤直接复用 Guard。

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use astersql_testkit_testfailpoint::FailGuard;

use crate::stubs::{self, Context, StopWatch};

/// 单步回调类型：对应 Go 无返回值的 `any` 函数。
pub type StepFn = Arc<dyn Fn() + Send + Sync + 'static>;

/// 序列声明项：failpoint 全路径名 + 命中后执行的回调。
#[derive(Clone)]
pub struct StepDecl {
    name: String,
    fn_value: StepFn,
}

/// 已激活序列中的一步；由 BeginSeq 从 StepDecl 转换而来。
struct ActiveStep {
    name: String,
    fn_value: StepFn,
}

struct RegisteredStep {
    /// Kept alive for Script lifetime (Go `t.Cleanup` via testfailpoint.EnableCall).
    /// FailGuard 存活即保持 enable；Script drop 时自动 Disable，对齐 Go Cleanup。
    _guard: FailGuard,
}

/// Step builds one sequence step.
///
/// The name must be the full failpoint path accepted by EnableCall. The fn must
/// be a non-nil function with no return values and a signature compatible with
/// the failpoint arguments.
/// 名称必须是 EnableCall 接受的完整路径；短名需调用方自行拼包路径。
pub fn Step<F>(name: impl Into<String>, fn_value: F) -> StepDecl
where
    F: Fn() + Send + Sync + 'static,
{
    StepDecl {
        name: name.into(),
        fn_value: Arc::new(fn_value),
    }
}

/// 脚本协调器：一次 Script 可复用多次 BeginSeq/EndSeq。
/// `registered` 跨序列缓存 FailGuard，同名只注册一次。
pub struct Script {
    state: Arc<State>,
    registered: Mutex<HashMap<String, RegisteredStep>>,
}

/// 共享状态：互斥保护序列，Condvar 用于错序命中时阻塞等待。
struct State {
    mu: Mutex<StateInner>,
    cond: Condvar,
}

/// 当前活跃序列：步骤表、下一期望下标、取消错误、AfterFunc 句柄。
struct StateInner {
    seq: Vec<ActiveStep>,
    next: usize,
    err: Option<String>,
    stop_watch: Option<StopWatch>,
}

impl Default for StateInner {
    fn default() -> Self {
        Self {
            seq: Vec::new(),
            next: 0,
            err: None,
            stop_watch: None,
        }
    }
}

/// New creates an empty script (Go `New(t testing.TB)`).
/// Rust 侧无 testing.TB；清理依赖 Script drop 释放 FailGuard。
pub fn New() -> Script {
    Script {
        state: Arc::new(State {
            mu: Mutex::new(StateInner::default()),
            cond: Condvar::new(),
        }),
        registered: Mutex::new(HashMap::new()),
    }
}

impl Script {
    /// BeginSeq starts an explicit ordered sequence for subsequent failpoint hits.
    /// ctx 不可为 None、steps 不可为空；已有活跃序列时 fatal。
    /// 注册步骤时不持序列锁，与 Go 一致，避免 EnableCall 与 advance 死锁。
    pub fn BeginSeq(&self, ctx: Option<&Context>, steps: Vec<StepDecl>) {
        let ctx = match ctx {
            Some(c) => c,
            None => fatal("syncpoint sequence context must not be nil"),
        };
        if steps.is_empty() {
            fatal("syncpoint sequence must not be empty");
        }

        {
            let inner = self.state.mu.lock().unwrap();
            let already_active = !inner.seq.is_empty();
            drop(inner);
            if already_active {
                fatal("syncpoint sequence already active");
            }
        }

        // Go validates/registers steps without holding the sequence lock.
        // 先 prepare/register，再二次检查活跃性，防止并发 BeginSeq 竞态。
        let mut active: Vec<ActiveStep> = Vec::with_capacity(steps.len());
        for step in steps {
            active.push(self.prepare_step(step));
        }

        let mut inner = self.state.mu.lock().unwrap();
        if !inner.seq.is_empty() {
            fatal("syncpoint sequence already active");
        }
        if let Some(sw) = inner.stop_watch.take() {
            sw.stop();
        }

        inner.seq.clear();
        inner.seq.extend(active);
        inner.next = 0;
        inner.err = None;

        // 上下文取消时写入序列错误并唤醒所有 Condvar 等待者。
        let state = Arc::clone(&self.state);
        let ctx_watch = ctx.clone();
        inner.stop_watch = Some(stubs::after_func(ctx.clone(), move || {
            let mut inner = state.mu.lock().unwrap();
            // 序列已结束或已有错误则忽略，避免覆盖真实失败原因。
            if inner.seq.is_empty() || inner.next >= inner.seq.len() || inner.err.is_some() {
                return;
            }
            let step_name = inner.seq[inner.next].name.clone();
            let next = inner.next;
            let ctx_err = ctx_watch
                .err_message()
                .unwrap_or_else(|| "context canceled".to_string());
            inner.err = Some(format!(
                "sequence canceled while waiting for step {next} ({step_name}): {ctx_err}"
            ));
            state.cond.notify_all();
        }));
    }

    /// EndSeq waits for the active sequence to complete and validates it.
    ///
    /// Go does not block on the condition variable here; callers must observe
    /// step side effects before calling EndSeq. This matches that contract.
    /// 不在此阻塞 Condvar：调用方须先等步骤副作用，再 EndSeq 校验完整性。
    pub fn EndSeq(&self) {
        let mut inner = self.state.mu.lock().unwrap();

        if inner.seq.is_empty() {
            fatal("syncpoint sequence underflow");
        }
        if let Some(sw) = inner.stop_watch.take() {
            sw.stop();
        }
        if let Some(err) = inner.err.as_ref() {
            fatal(format!("syncpoint sequence error: {err}"));
        }
        if inner.next != inner.seq.len() {
            fatal(format!(
                "syncpoint sequence incomplete: next={} len={}",
                inner.next,
                inner.seq.len()
            ));
        }

        // 清空后允许同一 Script 再次 BeginSeq。
        inner.seq.clear();
        inner.next = 0;
        inner.err = None;
        inner.stop_watch = None;
    }

    /// 校验步名非空并确保 failpoint 已注册，再装入 ActiveStep。
    fn prepare_step(&self, step: StepDecl) -> ActiveStep {
        if step.name.is_empty() {
            fatal("syncpoint step name must not be empty");
        }
        self.register(&step.name);
        ActiveStep {
            name: step.name,
            fn_value: step.fn_value,
        }
    }

    /// 同名只 Enable 一次；回调经 advance 决定是否执行用户 StepFn。
    fn register(&self, name: &str) {
        let mut registered = self.registered.lock().unwrap();
        if registered.contains_key(name) {
            // Go also checks reflect.Type consistency; Rust Step is always Fn().
            // Go 还会比对 reflect.Type；Rust 统一 Fn()，重复注册直接返回。
            return;
        }

        let state = Arc::clone(&self.state);
        let name_owned = name.to_string();
        let guard = astersql_testkit_testfailpoint::enable_call(name, move || {
            if let Some(callback) = advance(&state, &name_owned) {
                callback();
            }
        });
        registered.insert(name.to_string(), RegisteredStep { _guard: guard });
    }
}

/// failpoint 命中入口：非期望步则 Condvar 等待；匹配则推进 next 并返回回调。
/// 无活跃序列返回 None（静默忽略）；已取消或越界则记错并不执行回调。
fn advance(state: &State, name: &str) -> Option<StepFn> {
    let mut inner = state.mu.lock().unwrap();

    if inner.seq.is_empty() {
        return None;
    }

    loop {
        if inner.err.is_some() {
            return None;
        }
        if inner.next >= inner.seq.len() {
            inner.err = Some(format!("unexpected step {name} after sequence completed"));
            state.cond.notify_all();
            return None;
        }
        if inner.seq[inner.next].name == name {
            let callback = Arc::clone(&inner.seq[inner.next].fn_value);
            inner.next += 1;
            // 最后一步完成时停止 AfterFunc，避免迟到取消污染已成功序列。
            if inner.next == inner.seq.len() {
                if let Some(sw) = inner.stop_watch.take() {
                    sw.stop();
                }
            }
            state.cond.notify_all();
            return Some(callback);
        }
        // 错序命中：阻塞直到期望步被其他线程推进或取消。
        inner = state.cond.wait(inner).unwrap();
    }
}

/// 对齐 Go `t.Fatal`：以 panic 终止当前序列操作。
fn fatal(msg: impl AsRef<str>) -> ! {
    panic!("{}", msg.as_ref());
}
