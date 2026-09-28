// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 版本与 MPP / Exchange 压缩相关枚举的单元测试。
//
// 覆盖 Version::Cmp 边界、MPP 版本解析，以及 ExchangeCompressionMode 名称往返。

use kv_dependency as kv;
use vardef_dependency as vardef;

/// 验证 Version 三路比较及 Min/Max 哨兵顺序。
#[test]
fn test_version() {
    assert!(kv::NewVersion(42).Cmp(kv::NewVersion(43)) < 0);
    assert!(kv::NewVersion(42).Cmp(kv::NewVersion(41)) > 0);
    assert_eq!(0, kv::NewVersion(42).Cmp(kv::NewVersion(42)));
    assert!(kv::MinVersion.Cmp(kv::MaxVersion) < 0);
}

/// 验证最新 MPP 版本号与名称到枚举的解析（含 unspecified）。
#[test]
fn test_mpp_version() {
    assert_eq!(3, kv::GetNewestMppVersion().ToInt64());
    for (name, expected) in [
        ("unspecified", kv::MppVersionUnspecified),
        ("-1", kv::MppVersionUnspecified),
        ("0", kv::MppVersionV0),
        ("1", kv::MppVersionV1),
        ("2", kv::MppVersionV2),
        ("3", kv::MppVersionV3),
    ] {
        let (actual, ok) = kv::ToMppVersion(name);
        assert!(ok);
        assert_eq!(expected, actual);
    }
}

/// 验证 Exchange 压缩模式名称往返，以及推荐模式映射到 tipb CompressionMode。
#[test]
fn test_exchange_compression_mode() {
    for (name, expected) in [
        ("UNSPECIFIED", vardef::ExchangeCompressionModeUnspecified),
        ("NONE", vardef::ExchangeCompressionModeNONE),
        ("FAST", vardef::ExchangeCompressionModeFast),
        ("HIGH_COMPRESSION", vardef::ExchangeCompressionModeHC),
    ] {
        assert_eq!(name, expected.Name());
        let (actual, ok) = vardef::ToExchangeCompressionMode(name);
        assert!(ok);
        assert_eq!(expected, actual);
    }
    assert_eq!(
        vardef::ExchangeCompressionModeFast,
        vardef::RecommendedExchangeCompressionMode
    );
    assert_eq!(
        vardef::CompressionMode::Fast,
        vardef::RecommendedExchangeCompressionMode.ToTipbCompressionMode()
    );
}
