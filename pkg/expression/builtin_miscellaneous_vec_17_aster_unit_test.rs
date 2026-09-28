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

// 杂项内置函数向量化路径的 Aster 单元测试。
//
// 对齐 Go `builtin_miscellaneous_vec` 用例：网络地址转换边界、UUID 解析/生成/时间戳、
// ANY_VALUE/NAME_CONST 透传、SLEEP 告警与 kill 传播、Vitess Hash 与向量化签名清单。

use std::{sync::Arc, thread, time::Duration};

use crate::builtin_miscellaneous_vec::{
    EvalError, InvalidArgumentMode, SleepSession, VECTORIZED_SIGNATURES, vec_any_value,
    vec_bin_to_uuid, vec_inet_aton, vec_inet_ntoa, vec_inet6_aton, vec_inet6_ntoa,
    vec_int_any_value_string, vec_is_ipv4, vec_is_ipv4_compat, vec_is_ipv4_mapped, vec_is_ipv6,
    vec_is_uuid, vec_name_const, vec_sleep, vec_uuid_timestamp, vec_uuid_to_bin, vec_uuid_v1,
    vec_uuid_v4, vec_uuid_v7, vec_uuid_version, vec_vitess_hash,
};
use uuid::Uuid;

#[test]
/// INET_ATON / INET_NTOA：省略段、越界、多段与负数等边界与 Go 一致。
fn inet_aton_and_ntoa_match_go_edge_cases() {
    let encoded = vec_inet_aton(&[
        Some(""),
        None,
        Some("255.255.255.255"),
        Some("0.0.0.0"),
        Some("127.0.0.1"),
        Some("0.0.0.256"),
        Some("127"),
        Some(".122"),
        Some(".123.123"),
        Some("127.255"),
        Some("127.2.1"),
        Some("123.2.1."),
        Some("127.0.0.1.1"),
    ]);
    assert_eq!(
        encoded,
        vec![
            None,
            None,
            Some(4_294_967_295),
            Some(0),
            Some(2_130_706_433),
            None,
            Some(127),
            Some(122),
            Some(8_061_051),
            Some(2_130_706_687),
            Some(2_130_837_505),
            None,
            None,
        ]
    );

    assert_eq!(
        vec_inet_ntoa(&[
            Some(167_773_449),
            Some(2_063_728_641),
            Some(0),
            Some(545_460_846_593),
            Some(-1),
            Some(u32::MAX as i64),
            None,
        ]),
        vec![
            Some("10.0.5.9".to_owned()),
            Some("123.2.0.1".to_owned()),
            Some("0.0.0.0".to_owned()),
            None,
            None,
            Some("255.255.255.255".to_owned()),
            None,
        ]
    );
}

#[test]
/// IS_IPV4 / IS_IPV6：严格 IPv4 规则、映射写法与 NULL 传播。
fn ip_predicates_preserve_nulls_and_strict_ipv4_rules() {
    assert_eq!(
        vec_is_ipv4(&[
            Some("192.168.1.1"),
            Some("10.t.255.255"),
            Some("::ffff:1.2.3.4"),
            Some("1...1"),
            Some("192.168.1."),
            Some(".168.1.2"),
            Some("168.1.2"),
            Some("99999.1.1.1"),
            None,
        ]),
        vec![
            Some(1),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            None
        ]
    );
    assert_eq!(
        vec_is_ipv6(&[
            Some("2001:250:207:0:0:eef2::1"),
            Some("192.168.1.1"),
            Some("::ffff:1.2.3.4"),
            Some("2001:250:207::eff2::1，"),
            None,
        ]),
        vec![Some(1), Some(0), Some(1), Some(0), None]
    );
}

#[test]
/// INET6_ATON / INET6_NTOA：IPv4/IPv6/映射地址二进制往返。
fn inet6_binary_conversion_matches_go() {
    let mapped = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 1, 2, 3, 4];
    let ipv6 = [
        0xfd, 0xfe, 0, 0, 0, 0, 0, 0, 0x5a, 0x55, 0xca, 0xff, 0xfe, 0xfa, 0x90, 0x89,
    ];
    assert_eq!(
        vec_inet6_aton(&[
            Some("0.0.0.0"),
            Some("10.0.5.9"),
            Some("fdfe::5a55:caff:fefa:9089"),
            Some("::ffff:1.2.3.4"),
            Some(""),
            Some("1.0002.3.4"),
            None,
        ]),
        vec![
            Some(vec![0, 0, 0, 0]),
            Some(vec![10, 0, 5, 9]),
            Some(ipv6.to_vec()),
            Some(mapped.to_vec()),
            None,
            None,
            None,
        ]
    );
    assert_eq!(
        vec_inet6_ntoa(&[
            Some(&[10, 0, 5, 9]),
            Some(&ipv6),
            Some(&mapped),
            Some(&[10, 0, 5]),
            None,
        ]),
        vec![
            Some("10.0.5.9".to_owned()),
            Some("fdfe::5a55:caff:fefa:9089".to_owned()),
            Some("::ffff:1.2.3.4".to_owned()),
            None,
            None,
        ]
    );
}

#[test]
/// IS_IPV4_MAPPED / IS_IPV4_COMPAT：前缀字节与 Go 完全一致。
fn ipv4_mapped_and_compat_check_the_same_prefixes_as_go() {
    let mapped = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 1, 2, 3, 4];
    let compat = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4];
    let wrong = [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff, 0xff, 1, 2, 3, 4];
    assert_eq!(
        vec_is_ipv4_mapped(&[Some(&mapped), Some(&compat), Some(&wrong), Some(&[]), None]),
        vec![Some(1), Some(0), Some(0), Some(0), None]
    );
    assert_eq!(
        vec_is_ipv4_compat(&[Some(&mapped), Some(&compat), Some(&wrong), Some(&[]), None]),
        vec![Some(0), Some(1), Some(0), Some(0), None]
    );
}

#[test]
/// UUID 校验、v1/v4/v7 生成、VERSION 与 TIMESTAMP 微秒值对齐 Go。
fn uuid_validation_generation_version_and_timestamp_match_go() {
    assert_eq!(
        vec_is_uuid(&[
            Some("6ccd780c-baba-1026-9564-5b8c656024db"),
            Some("6ccd780cbaba102695645b8c656024db"),
            Some("{6ccd780c-baba-1026-9564-5b8c656024db}"),
            Some("{99a9ad03-5298-11ec-8f5c-00ff90147ac3*"),
            Some("urn:uuid:99a9ad03-5298-11ec-8f5c-00ff90147ac3"),
            Some(" 6ccd780c-baba-1026-9564-5b8c656024db"),
            Some("6CCD780C-BABA-1026-9564-5B8C656024DQ"),
            None,
        ]),
        vec![
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(0),
            Some(0),
            None
        ]
    );

    for generated in [vec_uuid_v1(3), vec_uuid_v4(3), vec_uuid_v7(3)] {
        let values = generated.expect("UUID generation should succeed");
        assert_eq!(values.len(), 3);
        assert!(values.iter().all(|value| value.len() == 36));
    }
    assert!(
        vec_uuid_v1(1)
            .unwrap()
            .iter()
            .all(|value| Uuid::parse_str(value).unwrap().get_version_num() == 1)
    );
    assert!(
        vec_uuid_v4(1)
            .unwrap()
            .iter()
            .all(|value| Uuid::parse_str(value).unwrap().get_version_num() == 4)
    );
    assert!(
        vec_uuid_v7(1)
            .unwrap()
            .iter()
            .all(|value| Uuid::parse_str(value).unwrap().get_version_num() == 7)
    );

    assert_eq!(
        vec_uuid_version(&[
            Some("5f13f854-d74a-11f0-9b7a-0ae0156bd76b"),
            Some("c6437ef1-5b86-3a4e-a071-c2d4ad414e65"),
            Some("a3e3b4a1-ea6d-471e-9860-8303a8b261f6"),
            Some("271a8175-dadd-5df9-b0bd-20a4a0b441e6"),
            Some("1f0e48c1-7860-69cc-9b3f-35f89c103d4d"),
            Some("019b1440-87b7-7380-ab00-ce413e795004"),
            None,
        ])
        .unwrap(),
        vec![Some(1), Some(3), Some(4), Some(5), Some(6), Some(7), None]
    );
    assert_eq!(
        vec_uuid_timestamp(&[
            Some("5f13f854-d74a-11f0-9b7a-0ae0156bd76b"),
            Some("c6437ef1-5b86-3a4e-a071-c2d4ad414e65"),
            Some("1f0e48c1-7860-69cc-9b3f-35f89c103d4d"),
            Some("019b1440-87b7-7380-ab00-ce413e795004"),
            None,
        ])
        .unwrap()
        .into_iter()
        .map(|value| value.map(|d| d.micros()))
        .collect::<Vec<_>>(),
        vec![
            Some(1_765_537_487_118_139),
            None,
            Some(1_766_995_078_970_004),
            Some(1_765_571_332_023_000),
            None
        ]
    );
    assert!(matches!(
        vec_uuid_version(&[Some("bad uuid")]),
        Err(EvalError::WrongValueForType {
            function: "uuid_version",
            ..
        })
    ));
    assert!(matches!(
        vec_uuid_timestamp(&[Some("bad uuid")]),
        Err(EvalError::WrongValueForType {
            function: "uuid_timestamp",
            ..
        })
    ));
}

#[test]
/// UUID_TO_BIN / BIN_TO_UUID：swap 标志与非法输入错误路径。
fn uuid_binary_round_trip_and_swap_flag_match_go() {
    let source = "6ccd780c-baba-1026-9564-5b8c656024db";
    let plain = vec_uuid_to_bin(&[Some(source), None], None).unwrap();
    assert_eq!(
        plain[0],
        Some(vec![
            0x6c, 0xcd, 0x78, 0x0c, 0xba, 0xba, 0x10, 0x26, 0x95, 0x64, 0x5b, 0x8c, 0x65, 0x60,
            0x24, 0xdb
        ])
    );
    assert_eq!(plain[1], None);

    let swapped = vec_uuid_to_bin(&[Some(source)], Some(&[Some(1)])).unwrap();
    assert_eq!(
        swapped[0],
        Some(vec![
            0x10, 0x26, 0xba, 0xba, 0x6c, 0xcd, 0x78, 0x0c, 0x95, 0x64, 0x5b, 0x8c, 0x65, 0x60,
            0x24, 0xdb
        ])
    );
    assert_eq!(
        vec_bin_to_uuid(&[swapped[0].as_deref()], Some(&[Some(1)])).unwrap(),
        vec![Some(source.to_owned())]
    );
    assert_eq!(
        vec_bin_to_uuid(&[plain[0].as_deref()], Some(&[Some(1)])).unwrap(),
        vec![Some("baba1026-780c-6ccd-9564-5b8c656024db".to_owned())]
    );
    assert!(matches!(
        vec_uuid_to_bin(&[Some(" 6ccd780c-baba-1026-9564-5b8c656024db")], None),
        Err(EvalError::WrongValueForType {
            function: "uuid_to_bin",
            ..
        })
    ));
    assert!(matches!(
        vec_bin_to_uuid(&[Some(&[0_u8; 15])], None),
        Err(EvalError::WrongValueForType {
            function: "bin_to_uuid",
            ..
        })
    ));
    assert_eq!(
        vec_uuid_to_bin(&[None], Some(&[Some(1)])).unwrap(),
        vec![None]
    );
    assert!(matches!(
        vec_uuid_to_bin(&[Some(source), None], Some(&[Some(1)])),
        Err(EvalError::MismatchedColumnLength {
            values: 2,
            flags: 1,
        })
    ));
    assert!(matches!(
        vec_bin_to_uuid(&[plain[0].as_deref()], Some(&[])),
        Err(EvalError::MismatchedColumnLength {
            values: 1,
            flags: 0,
        })
    ));
}

#[test]
/// ANY_VALUE / NAME_CONST 透传，以及 hybrid 整数字段的字符串回退。
fn pass_through_signatures_keep_nulls_and_hybrid_string_behavior() {
    let values = vec![Some(3_i64), None, Some(-9)];
    assert_eq!(vec_any_value(&values), values);
    assert_eq!(vec_name_const(&values), values);

    let argument = vec![Some("enum-a".to_owned()), None];
    let fallback = vec![Some("1".to_owned()), Some("0".to_owned())];
    assert_eq!(
        vec_int_any_value_string(true, &argument, &fallback),
        argument
    );
    assert_eq!(
        vec_int_any_value_string(false, &argument, &fallback),
        fallback
    );
}

#[test]
/// SLEEP：Warning/Error 模式、以及 kill 后后续行填 1 并复位 killer。
fn sleep_handles_warnings_strict_errors_and_kill_propagation() {
    let session = Arc::new(SleepSession::default());
    let warning = vec_sleep(
        &[None, Some(-1.0), Some(0.0), Some(f64::NAN)],
        &session,
        InvalidArgumentMode::Warning,
    )
    .unwrap();
    assert_eq!(warning.values, vec![0, 0, 0, 0]);
    assert_eq!(warning.warnings, 2);

    assert!(matches!(
        vec_sleep(&[Some(-2.5)], &session, InvalidArgumentMode::Error),
        Err(EvalError::IncorrectArguments("sleep"))
    ));
    assert!(matches!(
        vec_sleep(&[None], &session, InvalidArgumentMode::Error),
        Err(EvalError::IncorrectArguments("sleep"))
    ));

    let killer = Arc::clone(&session);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        killer.send_kill_signal();
    });
    let interrupted = vec_sleep(
        &[Some(0.01), Some(0.2), Some(0.2)],
        &session,
        InvalidArgumentMode::Error,
    )
    .unwrap();
    assert_eq!(interrupted.values, vec![0, 1, 1]);
    assert!(
        !session.is_killed(),
        "plain SELECT sleep resets the killer like Go"
    );

    let mutating_session = SleepSession::with_statement_side_effects(true, false, false, false);
    mutating_session.send_kill_signal();
    assert_eq!(
        vec_sleep(
            &[Some(0.02), Some(0.02)],
            &mutating_session,
            InvalidArgumentMode::Error,
        )
        .unwrap()
        .values,
        vec![1, 1]
    );
    assert!(
        mutating_session.is_killed(),
        "statements with table side effects retain the killer like Go"
    );
}

#[test]
/// VITESS_HASH 固定样例与 VECTORIZED_SIGNATURES 清单长度。
fn vitess_hash_and_vectorized_inventory_match_go() {
    assert_eq!(
        vec_vitess_hash(&[Some(30_375_298_039), Some(1123), Some(u64::MAX), None]),
        vec![
            Some(0x0312_6566_1e5f_1133),
            Some(0x031b_565d_41bd_f8ca),
            Some(0x3555_50b2_150e_2451),
            None,
        ]
    );
    assert_eq!(
        VECTORIZED_SIGNATURES,
        [
            "InetNtoa",
            "IsIPv4",
            "JSONAnyValue",
            "RealAnyValue",
            "StringAnyValue",
            "IsIPv6",
            "IsUUID",
            "NameConstString",
            "DecimalAnyValue",
            "UUID",
            "UUIDv4",
            "UUIDv7",
            "UUIDVersion",
            "UUIDTimestamp",
            "NameConstDuration",
            "DurationAnyValue",
            "IntAnyValue",
            "IsIPv4Compat",
            "NameConstInt",
            "NameConstTime",
            "Sleep",
            "IsIPv4Mapped",
            "NameConstDecimal",
            "NameConstJSON",
            "Inet6Aton",
            "TimeAnyValue",
            "InetAton",
            "Inet6Ntoa",
            "NameConstReal",
            "VitessHash",
            "UUIDToBin",
            "BinToUUID",
        ]
    );
}
