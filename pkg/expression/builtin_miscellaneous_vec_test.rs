// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 杂项内置函数向量化模块的冒烟测试入口。
//
// 覆盖 IP 转换、UUID 往返与错误、透传函数与 SLEEP 契约，确认与 Go 行为一致。

use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use crate::builtin_miscellaneous_vec::*;

#[test]
/// IP 相关向量函数保持行序、NULL 与非法值语义。
fn vectorized_ip_functions_preserve_rows_nulls_and_invalid_values() {
    assert_eq!(
        vec_inet_aton(&[Some("127.0.0.1"), Some("127"), Some("0.0.0.256"), None]),
        vec![Some(2_130_706_433), Some(127), None, None]
    );
    assert_eq!(
        vec_is_ipv4(&[Some("192.168.1.1"), Some("2001:db8::68"), None]),
        vec![Some(1), Some(0), None]
    );
    assert_eq!(
        vec_is_ipv6(&[Some("2001:db8::68"), Some("192.168.1.1"), None]),
        vec![Some(1), Some(0), None]
    );
    assert_eq!(
        vec_inet_ntoa(&[Some(u32::MAX as i64), Some(-1), None]),
        vec![Some("255.255.255.255".into()), None, None]
    );
}

#[test]
/// UUID 二进制往返、非法输入报错与 VERSION 对齐标量语义。
fn vectorized_uuid_round_trip_and_error_paths_match_scalar_semantics() {
    let source = "6ccd780c-baba-1026-9564-5b8c656024db";
    let encoded = vec_uuid_to_bin(&[Some(source), None], Some(&[Some(1), Some(0)])).unwrap();
    assert_eq!(encoded.len(), 2);
    assert_eq!(encoded[1], None);
    assert_eq!(
        vec_bin_to_uuid(&[encoded[0].as_deref(), None], Some(&[Some(1), Some(0)])).unwrap(),
        vec![Some(source.into()), None]
    );
    assert!(vec_uuid_to_bin(&[Some("bad uuid")], None).is_err());
    assert!(vec_bin_to_uuid(&[Some(&[0_u8; 15])], None).is_err());
    assert_eq!(
        vec_uuid_version(&[Some(source), None]).unwrap(),
        vec![Some(1), None]
    );
}

#[test]
/// 透传、SLEEP 告警/报错与向量化签名数量契约。
fn vectorized_pass_through_sleep_and_inventory_cover_go_contracts() {
    let values = vec![Some(3_i64), None, Some(-9)];
    assert_eq!(vec_any_value(&values), values);
    assert_eq!(vec_name_const(&values), values);

    let session = Arc::new(SleepSession::default());
    let warning = vec_sleep(
        &[None, Some(-1.0), Some(0.0)],
        &session,
        InvalidArgumentMode::Warning,
    )
    .unwrap();
    assert_eq!(warning.values, vec![0, 0, 0]);
    assert_eq!(warning.warnings, 2);
    assert!(matches!(
        vec_sleep(&[Some(-1.0)], &session, InvalidArgumentMode::Error),
        Err(EvalError::IncorrectArguments("sleep"))
    ));
    assert_eq!(VECTORIZED_SIGNATURES.len(), 32);
}

#[test]
/// Go converts a finite SLEEP value beyond time.Duration's range to a negative duration.
fn sleep_duration_overflow_returns_without_blocking_like_go() {
    let session = Arc::new(SleepSession::default());
    let worker_session = Arc::clone(&session);
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        sender
            .send(vec_sleep(
                &[Some((i64::MAX as f64 / 1_000_000_000.0) * 2.0)],
                &worker_session,
                InvalidArgumentMode::Error,
            ))
            .expect("test receiver remains alive");
    });

    let outcome = receiver
        .recv_timeout(Duration::from_millis(100))
        .expect("overflowed Go time.Duration fires immediately")
        .expect("the value is below Go's explicit MaxFloat64 guard");
    assert_eq!(outcome.values, vec![0]);
}
