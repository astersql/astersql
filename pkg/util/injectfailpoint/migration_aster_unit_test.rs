// Copyright 2026 AsterSQL.
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

// `injectfailpoint` 随机错误注入的单元测试。
//
// 用 `StepRng` 固定采样点，校验概率边界（严格小于）、已有错误优先、
// failpoint 开关行为，以及读路径 UnexpectedEof 的全量/部分失败分支。

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rand::rngs::mock::StepRng;

use super::random_retry::{
    DXFRandomErrorWithOnePercent, DXFRandomErrorWithOnePercentWrapper, Error,
    RandomErrorForReadWithOnePerPercent, random_error_with_rng, random_read_error_with_rng,
};

/// 串行化依赖全局 failpoint 配置的测试，避免互相干扰。
static FAILPOINT_TEST_LOCK: Mutex<()> = Mutex::new(());
/// 统计 failpoint 回调触发次数。
static FAILPOINT_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

/// 构造带固定消息的测试错误。
fn test_error(message: &'static str) -> Error {
    Box::new(io::Error::new(io::ErrorKind::Other, message))
}

/// 概率 0 永不命中，概率 1 必命中，NaN 视为不命中（对齐 Go Float64 边界）。
#[test]
fn random_error_matches_go_probability_boundaries() {
    let mut zero_rng = StepRng::new(0, 0);
    let err = random_error_with_rng(0.0, test_error("zero"), &mut zero_rng);
    assert!(err.is_none(), "Go rand.Float64 is never below zero");

    let mut zero_rng = StepRng::new(0, 0);
    let err = random_error_with_rng(1.0, test_error("one"), &mut zero_rng)
        .expect("a sample in [0, 1) is always below one");
    assert_eq!(err.to_string(), "one");

    let mut zero_rng = StepRng::new(0, 0);
    assert!(random_error_with_rng(f64::NAN, test_error("nan"), &mut zero_rng).is_none());
}

/// 比较使用严格小于：样本等于概率阈值时不注入。
#[test]
fn random_error_uses_strict_less_than_like_go() {
    let mut zero_rng = StepRng::new(0, 0);
    assert!(random_error_with_rng(0.0, test_error("boundary"), &mut zero_rng).is_none());

    let mut zero_rng = StepRng::new(0, 0);
    assert!(random_error_with_rng(f64::MIN_POSITIVE, test_error("below"), &mut zero_rng).is_some());
}

/// Wrapper 在已有错误时直接返回原错误，不评估 failpoint。
#[test]
fn wrapper_preserves_an_existing_error_before_failpoint_evaluation() {
    let _lock = FAILPOINT_TEST_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();
    fail::cfg("DXFRandomError", "panic(wrapper evaluated failpoint)").unwrap();

    let err = DXFRandomErrorWithOnePercentWrapper(Some(test_error("original")))
        .expect("the existing error must be returned");
    assert_eq!(err.to_string(), "original");
}

/// failpoint 未启用时保持正常返回值。
#[test]
fn disabled_failpoint_keeps_normal_return_values() {
    let _lock = FAILPOINT_TEST_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();

    assert!(DXFRandomErrorWithOnePercent().is_ok());
    assert!(DXFRandomErrorWithOnePercentWrapper(None).is_none());

    let (n, err) = RandomErrorForReadWithOnePerPercent(7, None);
    assert_eq!(n, 7);
    assert!(err.is_none());
}

/// 读透传路径即使 n=0 也会评估已启用的 failpoint 回调。
#[test]
fn read_passthrough_still_evaluates_the_enabled_failpoint() {
    let _lock = FAILPOINT_TEST_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();
    FAILPOINT_CALLBACKS.store(0, Ordering::SeqCst);
    fail::cfg_callback("DXFRandomError", || {
        FAILPOINT_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    let (n, err) = RandomErrorForReadWithOnePerPercent(0, None);
    assert_eq!((n, err.is_none()), (0, true));
    assert_eq!(FAILPOINT_CALLBACKS.load(Ordering::SeqCst), 1);
}

/// 读注入：n=0 或已有错误时保持不变。
#[test]
fn read_injection_keeps_zero_and_existing_error_unchanged() {
    let mut rng = StepRng::new(0, 0);
    let (n, err) = random_read_error_with_rng(0, None, &mut rng);
    assert_eq!(n, 0);
    assert!(err.is_none());

    let mut rng = StepRng::new(0, 0);
    let (n, err) = random_read_error_with_rng(9, Some(test_error("read failed")), &mut rng);
    assert_eq!(n, 9);
    assert_eq!(err.expect("existing error").to_string(), "read failed");
}

/// 全量失败返回 (0, UnexpectedEof)；部分失败返回 [0,n) 与 UnexpectedEof。
#[test]
fn read_injection_matches_go_full_and_partial_unexpected_eof_branches() {
    let mut full_failure_rng = StepRng::new(0, 0);
    let (n, err) = random_read_error_with_rng(8, None, &mut full_failure_rng);
    assert_eq!(n, 0);
    assert_eq!(
        err.expect("injected error")
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );

    let mut partial_failure_rng = StepRng::new(0, u64::MAX / 2);
    let (n, err) = random_read_error_with_rng(8, None, &mut partial_failure_rng);
    assert!((0..8).contains(&n), "Go rand.Intn(n) returns [0, n)");
    assert_eq!(
        err.expect("injected error")
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
}
