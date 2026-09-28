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
// Copyright 2026 AsterSQL.

// 杂项内建函数的 Aster 集成单元测试（expression group 18）。
//
// 对齐 Go 表：INET*/IPv6、UUID 校验/版本/时间戳/二进制往返、Vitess 哈希与
// TIDB_SHARD、咨询锁名称规范化与错误映射，以及 SLEEP/未支持函数契约。

use std::collections::HashMap;

use crate::expression_builtin_miscellaneous::*;
use uuid::Uuid;

/// INET_ATON/NTOA 表驱动：短格式、合法边界与非法输入。
#[test]
fn inet_aton_and_ntoa_match_go_tables() {
    let cases = [
        ("255.255.255.255", 4_294_967_295),
        ("0.0.0.0", 0),
        ("127.0.0.1", 2_130_706_433),
        ("113.14.22.3", 1_896_748_547),
        ("127", 127),
        ("127.255", 2_130_706_687),
        ("127.2.1", 2_130_837_505),
    ];
    for (input, expected) in cases {
        assert_eq!(inet_aton(Some(input)).unwrap(), Some(expected));
    }
    assert_eq!(inet_aton(None).unwrap(), None);
    for input in ["", "0.0.0.256", "127,256", "123.2.1.", "127.0.0.1.1"] {
        assert!(matches!(
            inet_aton(Some(input)),
            Err(MiscError::WrongValue { .. })
        ));
    }

    assert_eq!(inet_ntoa(Some(167_773_449)), Some("10.0.5.9".into()));
    assert_eq!(inet_ntoa(Some(2_063_728_641)), Some("123.2.0.1".into()));
    assert_eq!(inet_ntoa(Some(0)), Some("0.0.0.0".into()));
    assert_eq!(
        inet_ntoa(Some(u32::MAX as i64)),
        Some("255.255.255.255".into())
    );
    assert_eq!(inet_ntoa(Some(-1)), None);
    assert_eq!(inet_ntoa(Some(545_460_846_593)), None);
    assert_eq!(inet_ntoa(None), None);
}

/// INET6 互转与 IS_IPV4/IS_IPV6/compat/mapped 判定。
#[test]
fn ip_binary_conversions_and_predicates_match_go() {
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
    assert_eq!(
        inet6_aton(Some("fdfe::5a55:caff:fefa:9089")).unwrap(),
        Some(vec![
            0xfd, 0xfe, 0, 0, 0, 0, 0, 0, 0x5a, 0x55, 0xca, 0xff, 0xfe, 0xfa, 0x90, 0x89
        ])
    );
    for invalid in ["", "Not IP address", "1.0002.3.4", "1.2.256"] {
        assert!(inet6_aton(Some(invalid)).is_err());
    }
    assert_eq!(inet6_aton(None).unwrap(), None);

    assert_eq!(inet6_ntoa(Some(&[10, 0, 5, 9])), Some("10.0.5.9".into()));
    assert_eq!(inet6_ntoa(Some(&[0, 0, 0, 0])), Some("0.0.0.0".into()));
    assert_eq!(inet6_ntoa(Some(&mapped)), Some("::ffff:1.2.3.4".into()));
    assert_eq!(
        inet6_ntoa(Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ])),
        Some("::ffff:255.255.255.255".into())
    );
    assert_eq!(inet6_ntoa(Some(&[])), None);
    assert_eq!(inet6_ntoa(Some(&[10, 0, 5])), None);
    assert_eq!(inet6_ntoa(Some(&mapped[..15])), None);
    assert_eq!(inet6_ntoa(None), None);

    for valid in ["192.168.1.1", "255.255.255.255"] {
        assert_eq!(is_ipv4(Some(valid)), Some(true));
    }
    for invalid in [
        "10.t.255.255",
        "10.1.2.3.4",
        "::ffff:1.2.3.4",
        "1...1",
        "192.168.1.",
        ".168.1.2",
        "168.1.2",
        "1.2.3.4.5",
    ] {
        assert_eq!(is_ipv4(Some(invalid)), Some(false));
    }
    assert_eq!(is_ipv4(None), None);
    assert_eq!(is_ipv6(Some("2001:250:207:0:0:eef2::1")), Some(true));
    assert_eq!(
        is_ipv6(Some("2001:0250:0207:0001:0000:0000:0000:ff02")),
        Some(true)
    );
    assert_eq!(is_ipv6(Some("::ffff:1.2.3.4")), Some(true));
    assert_eq!(is_ipv6(Some("2001:250:207::eff2::1，")), Some(false));
    assert_eq!(is_ipv6(Some("192.168.1.1")), Some(false));
    assert_eq!(is_ipv6(None), None);
    assert_eq!(is_ipv4_mapped(Some(&mapped)), Some(true));
    assert_eq!(is_ipv4_mapped(Some(&compat)), Some(false));
    assert_eq!(is_ipv4_mapped(Some(&[])), Some(false));
    assert_eq!(is_ipv4_mapped(Some(&[0x10; 4])), Some(false));
    let mut almost_mapped = mapped;
    almost_mapped[9] = 1;
    assert_eq!(is_ipv4_mapped(Some(&almost_mapped)), Some(false));
    assert_eq!(is_ipv4_mapped(Some(&[0, 1, 2, 3, 4, 5, 6])), Some(false));
    assert_eq!(is_ipv4_compat(Some(&compat)), Some(true));
    assert_eq!(is_ipv4_compat(Some(&mapped)), Some(false));
    assert_eq!(is_ipv4_compat(Some(&[])), Some(false));
    assert_eq!(is_ipv4_compat(Some(&[0x10; 4])), Some(false));
    let mut almost_compat = compat;
    almost_compat[9] = 1;
    assert_eq!(is_ipv4_compat(Some(&almost_compat)), Some(false));
    assert_eq!(is_ipv4_compat(Some(&almost_mapped)), Some(false));
    assert_eq!(is_ipv4_compat(Some(&[0, 1, 2, 3, 4, 5, 6])), Some(false));
    assert_eq!(is_ipv4_mapped(None), None);
    assert_eq!(is_ipv4_compat(None), None);
}

/// IS_UUID 多格式、生成函数版本号，以及 UUID_VERSION。
#[test]
fn uuid_validation_generation_and_versions_match_go() {
    for valid in [
        "6ccd780c-baba-1026-9564-5b8c656024db",
        "6CCD780C-BABA-1026-9564-5B8C656024DB",
        "6ccd780cbaba102695645b8c656024db",
        "{6ccd780c-baba-1026-9564-5b8c656024db}",
        "{99a9ad03-5298-11ec-8f5c-00ff90147ac3*",
        "urn:uuid:99a9ad03-5298-11ec-8f5c-00ff90147ac3",
    ] {
        assert_eq!(is_uuid(Some(valid)), Some(true), "{valid}");
    }
    for invalid in [
        "6ccd780c-baba-1026-9564-5b8c6560",
        "6CCD780C-BABA-1026-9564-5B8C656024DQ",
        " 6ccd780c-baba-1026-9564-5b8c656024db",
        "6ccd780c-baba-1026-9564-5b8c656024db ",
        " 6ccd780c-baba-1026-9564-5b8c656024db ",
    ] {
        assert_eq!(is_uuid(Some(invalid)), Some(false), "{invalid}");
    }
    assert_eq!(is_uuid(None), None);

    let generated = [(uuid_v1(), 1), (uuid_v4(), 4), (uuid_v7(), 7)];
    for (value, version) in generated {
        let parsed = Uuid::parse_str(&value).unwrap();
        assert_eq!(parsed.get_version_num(), version);
        assert_eq!(
            value.split('-').map(str::len).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
    }

    let versions = [
        ("5f13f854-d74a-11f0-9b7a-0ae0156bd76b", 1),
        ("c6437ef1-5b86-3a4e-a071-c2d4ad414e65", 3),
        ("a3e3b4a1-ea6d-471e-9860-8303a8b261f6", 4),
        ("271a8175-dadd-5df9-b0bd-20a4a0b441e6", 5),
        ("1f0e48c1-7860-69cc-9b3f-35f89c103d4d", 6),
        ("019b1440-87b7-7380-ab00-ce413e795004", 7),
    ];
    for (value, version) in versions {
        assert_eq!(uuid_version(Some(value)).unwrap(), Some(version));
    }
    assert!(uuid_version(Some("bad uuid")).is_err());
    assert_eq!(uuid_version(None).unwrap(), None);
}

/// UUID_TIMESTAMP 仅对 v1/v6/v7 返回微秒时间，其余为 NULL。
#[test]
fn uuid_timestamp_matches_go_microsecond_results() {
    let cases = [
        (
            "5f13f854-d74a-11f0-9b7a-0ae0156bd76b",
            Some("1765537487.118139"),
        ),
        ("c6437ef1-5b86-3a4e-a071-c2d4ad414e65", None),
        ("a3e3b4a1-ea6d-471e-9860-8303a8b261f6", None),
        ("271a8175-dadd-5df9-b0bd-20a4a0b441e6", None),
        (
            "1f0e48c1-7860-69cc-9b3f-35f89c103d4d",
            Some("1766995078.970004"),
        ),
        (
            "019b1440-87b7-7380-ab00-ce413e795004",
            Some("1765571332.023000"),
        ),
        ("00000000-0000-0000-0000-000000000000", None),
        ("ffffffff-ffff-ffff-ffff-ffffffffffff", None),
    ];
    for (value, expected) in cases {
        let actual = uuid_timestamp(Some(value)).unwrap();
        assert_eq!(
            actual
                .as_ref()
                .map(UuidTimestamp::decimal_string)
                .as_deref(),
            expected
        );
    }
    assert_eq!(uuid_timestamp(None).unwrap(), None);
}

/// UUID_TO_BIN/BIN_TO_UUID 往返与 swap 标志字节序。
#[test]
fn uuid_binary_round_trip_and_swap_match_go() {
    let text = "6ccd780c-baba-1026-9564-5b8c656024db";
    let regular = [
        0x6c, 0xcd, 0x78, 0x0c, 0xba, 0xba, 0x10, 0x26, 0x95, 0x64, 0x5b, 0x8c, 0x65, 0x60, 0x24,
        0xdb,
    ];
    let swapped = [
        0x10, 0x26, 0xba, 0xba, 0x6c, 0xcd, 0x78, 0x0c, 0x95, 0x64, 0x5b, 0x8c, 0x65, 0x60, 0x24,
        0xdb,
    ];
    assert_eq!(uuid_to_bin(Some(text), Some(0)).unwrap(), Some(regular));
    assert_eq!(uuid_to_bin(Some(text), Some(1)).unwrap(), Some(swapped));
    assert_eq!(uuid_to_bin(Some(text), None).unwrap(), Some(regular));
    assert_eq!(
        uuid_to_bin(Some("6CCD780C-BABA-1026-9564-5B8C656024DB"), None).unwrap(),
        Some(regular)
    );
    assert_eq!(
        uuid_to_bin(Some("6ccd780cbaba102695645b8c656024db"), None).unwrap(),
        Some(regular)
    );
    assert_eq!(
        uuid_to_bin(Some("{6ccd780c-baba-1026-9564-5b8c656024db}"), None).unwrap(),
        Some(regular)
    );
    assert_eq!(uuid_to_bin(None, Some(1)).unwrap(), None);
    assert!(uuid_to_bin(Some("6ccd780c-baba-1026-9564-5b8c6560"), None).is_err());
    assert!(uuid_to_bin(Some(&format!(" {text}")), None).is_err());
    assert!(uuid_to_bin(Some(&format!("{text} ")), None).is_err());
    assert!(uuid_to_bin(Some(&format!(" {text} ")), None).is_err());
    assert_eq!(
        bin_to_uuid(Some(&regular), Some(0)).unwrap().as_deref(),
        Some(text)
    );
    assert_eq!(
        bin_to_uuid(Some(&regular), Some(1)).unwrap().as_deref(),
        Some("baba1026-780c-6ccd-9564-5b8c656024db")
    );
    assert_eq!(bin_to_uuid(None, Some(1)).unwrap(), None);
    assert!(bin_to_uuid(Some(&regular[..15]), None).is_err());
}

/// ANY_VALUE/NAME_CONST 透传与 Vitess/TIDB_SHARD 哈希表。
#[test]
fn pass_through_hash_and_shard_behaviors_match_go() {
    assert_eq!(any_value(Some(1234)), Some(1234));
    assert_eq!(any_value(Some(-0x99)), Some(-0x99));
    assert_eq!(any_value(Some(3.1415926)), Some(3.1415926));
    assert_eq!(any_value(Some("Hello, World")), Some("Hello, World"));
    assert_eq!(any_value::<i32>(None), None);
    assert_eq!(name_const("answer", Some("TiDB")), Some("TiDB"));
    assert_eq!(name_const::<_, i32>("empty", None), None);

    let hashes = [
        (30_375_298_039_u64, 0x0312_6566_1e5f_1133),
        (1_123, 0x031b_565d_41bd_f8ca),
        (30_573_721_600, 0x1efd_6439_f205_0ffd),
        (116, 0x1e17_88ff_0fde_093c),
        (u64::MAX, 0x3555_50b2_150e_2451),
    ];
    for (input, expected) in hashes {
        assert_eq!(vitess_hash_u64(input), expected);
    }
    assert_eq!(tidb_shard(-1), 81);
    assert_eq!(tidb_shard(0), 167);
    assert_eq!(tidb_shard(1), 214);
    assert_eq!(tidb_shard(9_999_999_999_999_999), 63);
}

/// 咨询锁测试替身：内存 HashMap 模拟占用连接 ID，并可注入下一次错误。
#[derive(Default)]
struct MockLocks {
    locks: HashMap<String, u64>,
    next_error: Option<AdvisoryLockError>,
}

impl AdvisoryLockContext for MockLocks {
    fn get_advisory_lock(
        &mut self,
        name: &str,
        _timeout_secs: i64,
    ) -> Result<(), AdvisoryLockError> {
        if let Some(error) = self.next_error.take() {
            return Err(error);
        }
        self.locks.insert(name.to_owned(), 42);
        Ok(())
    }

    fn is_used_advisory_lock(&self, name: &str) -> u64 {
        self.locks.get(name).copied().unwrap_or(0)
    }

    fn release_advisory_lock(&mut self, name: &str) -> bool {
        self.locks.remove(name).is_some()
    }

    fn release_all_advisory_locks(&mut self) -> u64 {
        let count = self.locks.len() as u64;
        self.locks.clear();
        count
    }
}

/// 锁名大小写归一、超时钳制告警、非法名与超时/死锁/其它错误映射。
#[test]
fn advisory_locks_preserve_normalization_limits_and_error_mapping() {
    let mut locks = MockLocks::default();
    let acquired = get_lock(&mut locks, Some("MiXeD"), Some(5), 50).unwrap();
    assert_eq!(
        acquired,
        LockOutcome {
            value: 1,
            warning: None
        }
    );
    assert_eq!(is_used_lock(&locks, Some("mixed")).unwrap(), Some(42));
    assert_eq!(is_free_lock(&locks, Some("MIXED")).unwrap(), 0);
    assert_eq!(release_lock(&mut locks, Some("Mixed")).unwrap(), 1);
    assert_eq!(release_lock(&mut locks, Some("Mixed")).unwrap(), 0);

    let clipped = get_lock(&mut locks, Some("one"), Some(-1), 50).unwrap();
    assert_eq!(clipped.value, 1);
    assert_eq!(
        clipped.warning,
        Some(LockWarning {
            supplied_timeout: -1,
            effective_timeout: 50
        })
    );
    get_lock(&mut locks, Some("two"), None, 50).unwrap();
    assert_eq!(release_all_locks(&mut locks), 2);

    // NULL、空串、超长 Unicode 名均应报 UserLockWrongName。
    for invalid in [
        None,
        Some(""),
        Some(
            "界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界界",
        ),
    ] {
        assert!(matches!(
            get_lock(&mut locks, invalid, Some(0), 50),
            Err(MiscError::UserLockWrongName(_))
        ));
    }

    locks.next_error = Some(AdvisoryLockError::Timeout);
    assert_eq!(
        get_lock(&mut locks, Some("wait"), Some(1), 50)
            .unwrap()
            .value,
        0
    );
    locks.next_error = Some(AdvisoryLockError::Deadlock);
    assert_eq!(
        get_lock(&mut locks, Some("dead"), Some(1), 50),
        Err(MiscError::UserLockDeadlock)
    );
    locks.next_error = Some(AdvisoryLockError::Other("backend".into()));
    assert_eq!(
        get_lock(&mut locks, Some("other"), Some(1), 50),
        Err(MiscError::Lock("backend".into()))
    );
}

/// SLEEP 零秒/被 kill，以及 DEFAULT/UUID_SHORT/tidb_row_checksum 的固定错误。
#[test]
fn sleep_and_unsupported_functions_keep_go_contracts() {
    assert_eq!(sleep_builtin(Some(0.0), || false).unwrap(), 0);
    assert_eq!(sleep_builtin(Some(0.1), || true).unwrap(), 1);
    assert!(matches!(
        sleep_builtin(None, || false),
        Err(MiscError::IncorrectArguments("sleep"))
    ));
    assert!(matches!(
        sleep_builtin(Some(-0.1), || false),
        Err(MiscError::IncorrectArguments("sleep"))
    ));
    assert_eq!(
        default_function(),
        Err(MiscError::FunctionNotExists("DEFAULT"))
    );
    assert_eq!(
        uuid_short(),
        Err(MiscError::FunctionNotExists("UUID_SHORT"))
    );
    assert_eq!(
        tidb_row_checksum(),
        Err(MiscError::NotSupported(
            "FUNCTION tidb_row_checksum can only be used as a select field in a fast point plan"
        ))
    );
}
