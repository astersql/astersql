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
// 杂项内建函数（网络地址、UUID、ANY_VALUE、分片哈希等）的常规单元测试。
//
// 对应 Go `builtin_miscellaneous_test.go`：覆盖 INET*、IPv4/IPv6 判定、
// UUID 校验/生成/二进制往返，以及透传与 TIDB_SHARD/Vitess 哈希边界。

use crate::expression_builtin_miscellaneous::*;
use uuid::Uuid;

/// INET_ATON/INET_NTOA：合法地址、短格式、NULL，以及非法字符串错误。
#[test]
fn inet_functions_cover_normal_null_boundary_and_invalid_inputs() {
    assert_eq!(inet_aton(Some("127.0.0.1")).unwrap(), Some(2_130_706_433));
    assert_eq!(inet_aton(Some("127.2.1")).unwrap(), Some(2_130_837_505));
    assert_eq!(
        inet_aton(Some("255.255.255.255")).unwrap(),
        Some(u32::MAX as u64)
    );
    assert_eq!(inet_aton(None).unwrap(), None);
    for invalid in ["", "0.0.0.256", "123.2.1.", "127.0.0.1.1"] {
        assert!(inet_aton(Some(invalid)).is_err(), "{invalid}");
    }

    assert_eq!(inet_ntoa(Some(167_773_449)), Some("10.0.5.9".into()));
    assert_eq!(
        inet_ntoa(Some(u32::MAX as i64)),
        Some("255.255.255.255".into())
    );
    assert_eq!(inet_ntoa(Some(-1)), None);
    assert_eq!(inet_ntoa(None), None);
}

/// INET6 二进制互转与 IS_IPV4/IS_IPV6/兼容/映射判定。
#[test]
fn ipv4_ipv6_conversion_and_predicates_match_go_cases() {
    let mapped = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 1, 2, 3, 4];
    let compat = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4];
    assert_eq!(
        inet6_aton(Some("10.0.5.9")).unwrap(),
        Some(vec![10, 0, 5, 9])
    );
    assert_eq!(
        inet6_aton(Some("::ffff:1.2.3.4")).unwrap(),
        Some(mapped.to_vec())
    );
    assert!(inet6_aton(Some("1.2.256")).is_err());
    assert_eq!(inet6_aton(None).unwrap(), None);
    assert_eq!(inet6_ntoa(Some(&mapped)), Some("::ffff:1.2.3.4".into()));
    assert_eq!(inet6_ntoa(Some(&[1, 2, 3])), None);

    assert_eq!(is_ipv4(Some("192.168.1.1")), Some(true));
    assert_eq!(is_ipv4(Some("::ffff:1.2.3.4")), Some(false));
    assert_eq!(is_ipv6(Some("2001:db8::68")), Some(true));
    assert_eq!(is_ipv6(Some("192.168.1.1")), Some(false));
    assert_eq!(is_ipv4_mapped(Some(&mapped)), Some(true));
    assert_eq!(is_ipv4_compat(Some(&compat)), Some(true));
    assert_eq!(is_ipv4(None), None);
}

/// IS_UUID、版本号、UUID_TO_BIN/BIN_TO_UUID（含字节序交换）往返。
#[test]
fn uuid_validation_generation_and_binary_round_trip_match_go() {
    let source = "6ccd780c-baba-1026-9564-5b8c656024db";
    assert_eq!(is_uuid(Some(source)), Some(true));
    assert_eq!(is_uuid(Some(" bad uuid")), Some(false));
    assert_eq!(is_uuid(None), None);
    for (value, version) in [(uuid_v1(), 1), (uuid_v4(), 4), (uuid_v7(), 7)] {
        assert_eq!(Uuid::parse_str(&value).unwrap().get_version_num(), version);
    }
    assert_eq!(uuid_version(Some(source)).unwrap(), Some(1));
    assert!(uuid_version(Some("bad uuid")).is_err());
    assert_eq!(uuid_version(None).unwrap(), None);

    let plain = uuid_to_bin(Some(source), Some(0)).unwrap().unwrap();
    let swapped = uuid_to_bin(Some(source), Some(1)).unwrap().unwrap();
    assert_ne!(plain, swapped);
    assert_eq!(
        bin_to_uuid(Some(&plain), Some(0)).unwrap().as_deref(),
        Some(source)
    );
    assert_eq!(
        bin_to_uuid(Some(&swapped), Some(1)).unwrap().as_deref(),
        Some(source)
    );
    assert!(bin_to_uuid(Some(&plain[..15]), None).is_err());
    assert_eq!(uuid_to_bin(None, Some(1)).unwrap(), None);
}

/// ANY_VALUE/NAME_CONST 透传，以及 Vitess 哈希与 TIDB_SHARD 边界值。
#[test]
fn pass_through_and_shard_helpers_preserve_values_and_boundaries() {
    assert_eq!(any_value(Some(1234)), Some(1234));
    assert_eq!(any_value::<i32>(None), None);
    assert_eq!(name_const("answer", Some("TiDB")), Some("TiDB"));
    assert_eq!(vitess_hash_u64(u64::MAX), 0x3555_50b2_150e_2451);
    assert_eq!(tidb_shard(-1), 81);
    assert_eq!(tidb_shard(0), 167);
}
