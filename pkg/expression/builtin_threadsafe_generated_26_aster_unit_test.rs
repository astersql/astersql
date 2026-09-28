// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 线程安全签名共享判定的 Aster 单元测试。
//
// 验证 `safeToShareAcrossSession` 的缓存语义：全参数检查、短路失败、
// 命中缓存不再探测、生成签名递归路径、未知/unsafe 签名永不默认递归安全，
// 以及并发发布稳定缓存结果。

use crate::builtin_threadsafe_generated_kernel::*;
use crate::expression_builtin::{self, baseBuiltinFunc};
use crate::mysql;
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[derive(Clone)]
/// 探测用 Expression：记录 SafeToShareAcrossSession 调用次数。
struct Probe {
    safe: bool,
    calls: Arc<AtomicUsize>,
    field_type: expression_builtin::types::FieldType,
}

impl Probe {
    /// 构造指定安全标志的 Probe。
    fn new(safe: bool) -> Self {
        Self {
            safe,
            calls: Arc::new(AtomicUsize::new(0)),
            field_type: expression_builtin::types::FieldType::new(mysql::TypeUnspecified),
        }
    }

    /// 返回安全标志并递增调用计数。
    fn is_safe(&self) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.safe
    }

    /// 读取累计探测次数。
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl expression_builtin::Expression for Probe {
    /// Expression::as_any 实现。
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// 返回占位 FieldType。
    fn GetType(
        &self,
        _ctx: &dyn expression_builtin::EvalContext,
    ) -> &expression_builtin::types::FieldType {
        &self.field_type
    }

    /// 转发到 is_safe 以统计调用。
    fn SafeToShareAcrossSession(&self) -> bool {
        self.is_safe()
    }
}

#[test]
/// 未缓存且全部安全时检查每个参数并缓存 1。
fn uncached_safe_result_checks_every_argument_and_caches_one() {
    let flag = AtomicU32::new(0);
    let args = [Probe::new(true), Probe::new(true), Probe::new(true)];

    assert!(safeToShareAcrossSession(&flag, &args, Probe::is_safe));
    assert_eq!(flag.load(Ordering::SeqCst), 1);
    assert_eq!(args.iter().map(Probe::calls).collect::<Vec<_>>(), [1, 1, 1]);
}

#[test]
/// 未缓存遇首个失败即短路并缓存 2。
fn uncached_unsafe_result_stops_at_first_failure_and_caches_two() {
    let flag = AtomicU32::new(0);
    let args = [Probe::new(true), Probe::new(false), Probe::new(true)];

    assert!(!safeToShareAcrossSession(&flag, &args, Probe::is_safe));
    assert_eq!(flag.load(Ordering::SeqCst), 2);
    assert_eq!(args.iter().map(Probe::calls).collect::<Vec<_>>(), [1, 1, 0]);
}

#[test]
/// 已缓存结果不再重新探测参数。
fn cached_results_do_not_recheck_arguments() {
    for (cached, expected) in [(1, true), (2, false)] {
        let flag = AtomicU32::new(cached);
        let args = [Probe::new(!expected)];

        assert_eq!(
            safeToShareAcrossSession(&flag, &args, Probe::is_safe),
            expected
        );
        assert_eq!(args[0].calls(), 0);
        assert_eq!(flag.load(Ordering::SeqCst), cached);
    }
}

#[test]
/// 生成安全签名走同一递归缓存路径。
fn generated_signature_method_uses_the_same_recursive_cache_path() {
    let first = Probe::new(true);
    let second = Probe::new(true);
    let first_calls = first.calls.clone();
    let second_calls = second.calls.clone();
    let mut signature = baseBuiltinFunc::new(
        vec![Box::new(first), Box::new(second)],
        expression_builtin::types::FieldType::new(mysql::TypeUnspecified),
    );
    signature.SetGeneratedSignature("builtinASCIISig").unwrap();

    assert!(signature.SafeToShareAcrossSession());
    assert!(signature.SafeToShareAcrossSession());
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
}

#[test]
/// 未知与 unsafe 签名不默认递归安全。
fn unknown_and_generated_unsafe_signatures_never_default_to_recursive_safe() {
    let unknown_child = Probe::new(true);
    let unknown_calls = unknown_child.calls.clone();
    let unknown = baseBuiltinFunc::new(
        vec![Box::new(unknown_child)],
        expression_builtin::types::FieldType::new(mysql::TypeUnspecified),
    );
    assert!(!unknown.SafeToShareAcrossSession());
    assert_eq!(unknown_calls.load(Ordering::SeqCst), 0);

    let unsafe_child = Probe::new(true);
    let unsafe_calls = unsafe_child.calls.clone();
    let mut unsafe_signature = baseBuiltinFunc::new(
        vec![Box::new(unsafe_child)],
        expression_builtin::types::FieldType::new(mysql::TypeUnspecified),
    );
    unsafe_signature
        .SetGeneratedSignature("builtinLikeSig")
        .unwrap();
    assert!(!unsafe_signature.SafeToShareAcrossSession());
    assert_eq!(unsafe_calls.load(Ordering::SeqCst), 0);
    assert!(
        unsafe_signature
            .SetGeneratedSignature("builtinUnknownSig")
            .is_err()
    );
}

#[test]
/// 并发调用发布稳定的缓存结果。
fn concurrent_callers_publish_a_stable_cached_result() {
    let flag = Arc::new(AtomicU32::new(0));
    let args = Arc::new([Probe::new(true), Probe::new(true)]);
    let workers = (0..16)
        .map(|_| {
            let flag = Arc::clone(&flag);
            let args = Arc::clone(&args);
            std::thread::spawn(move || {
                safeToShareAcrossSession(flag.as_ref(), args.as_ref(), Probe::is_safe)
            })
        })
        .collect::<Vec<_>>();

    assert!(workers.into_iter().all(|worker| worker.join().unwrap()));
    assert_eq!(flag.load(Ordering::SeqCst), 1);
    assert!(args.iter().all(|arg| arg.calls() >= 1));
}

#[test]
/// 核对 Go 生成的全部安全签名数量与去重。
fn harness_covers_every_go_generated_safe_signature() {
    assert_eq!(GENERATED_SIGNATURE_COUNT, 510);
    assert_eq!(GENERATED_THREADSAFE_SIGNATURES.len(), 510);
    assert!(IsGeneratedThreadsafeSignature("builtinASCIISig"));
    assert!(IsGeneratedThreadsafeSignature(
        "builtinYearWeekWithoutModeSig"
    ));
    let unique = GENERATED_THREADSAFE_SIGNATURES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), GENERATED_SIGNATURE_COUNT);
}

#[test]
/// 每个正式工厂签名恰有一条生成策略且 safe/unsafe 不重叠。
fn every_formal_factory_signature_has_one_exact_generated_policy() {
    use crate::builtin_threadunsafe_generated_kernel::GENERATED_THREADUNSAFE_SIGNATURES;

    for signature in GENERATED_THREADSAFE_SIGNATURES
        .iter()
        .chain(GENERATED_THREADUNSAFE_SIGNATURES)
    {
        assert!(GeneratedThreadSafetyPolicyForSignature(signature).is_some());
        assert!(crate::formal_registry::ValidateGeneratedBuiltinSignature(signature).is_ok());
    }
    assert!(
        crate::formal_registry::ValidateGeneratedBuiltinSignature("builtinUnknownSig").is_err()
    );

    let safe = GENERATED_THREADSAFE_SIGNATURES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        GENERATED_THREADUNSAFE_SIGNATURES
            .iter()
            .all(|signature| !safe.contains(signature))
    );
}
