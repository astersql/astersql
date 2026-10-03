// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Meta Service 迁移对照测试：URL 解析、PD 客户端发现与分组校验对齐 Go 行为。

use super::{
    Context, GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL, GC_MANAGEMENT_TYPE_UNIFIED,
    GLOBAL_GROUP_ID, GROUP_ADDRS_KEY, GROUP_ID_KEY, KeyspaceMeta, MetaServiceError, PdClient,
    PdMember, ServiceClient, get_group, get_info, get_info_and_group_addrs, new_client, parse_url,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// 按调用顺序弹出预设响应的假 PD 客户端，用于模拟成员发现与重试。
#[derive(Default)]
struct MockPdClient {
    /// 队列中的 `GetAllMembers` 返回值；每次调用弹出队首。
    responses: Mutex<VecDeque<Result<Vec<PdMember>, MetaServiceError>>>,
}

impl MockPdClient {
    /// 用给定响应序列构造 mock。
    fn with_responses(responses: Vec<Result<Vec<PdMember>, MetaServiceError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl PdClient for MockPdClient {
    fn get_all_members(&self, _ctx: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        self.responses
            .lock()
            .expect("mock response lock")
            .pop_front()
            .expect("unexpected GetAllMembers call")
    }
}

/// 构造仅含 client URL 列表的 [`PdMember`]。
fn member(urls: &[&str]) -> PdMember {
    PdMember {
        client_urls: urls.iter().map(|url| (*url).to_owned()).collect(),
    }
}

/// 按名称与配置项构造 [`KeyspaceMeta`]。
fn keyspace(name: &str, entries: &[(&str, &str)]) -> KeyspaceMeta {
    KeyspaceMeta {
        name: name.to_owned(),
        config: entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>(),
    }
}

/// 对照 Go 合法/非法 URL 表，校验 `parse_url` 拆分前缀与 host:port。
#[test]
fn migration_parse_url_matches_go_table() {
    let valid = [
        ("http://example.com:8080", "http://", "example.com:8080"),
        ("https://localhost:443", "https://", "localhost:443"),
        ("http://[2001:db8::1]:2379", "http://", "[2001:db8::1]:2379"),
        ("https://[2001:db8::1]:443", "https://", "[2001:db8::1]:443"),
    ];
    for (raw, prefix, host_port) in valid {
        assert_eq!(
            parse_url(raw).unwrap(),
            (prefix.to_owned(), host_port.to_owned())
        );
    }

    for raw in [
        "ftp://example.com",
        "http://example.com:8080:extra",
        "https://:8080",
        "http://",
        "https://example.com",
        "http://localhost",
        "https://[2001:db8::1]",
        "http://2001:db8::1:2379",
        "https://[2001:db8::1",
    ] {
        assert!(parse_url(raw).is_err(), "{raw} should be rejected");
    }
}

/// 仅 PD 客户端时：裸地址与 HTTP 地址均应正确；无 PD 时 `new_client` 返回 None。
#[test]
fn migration_pd_only_client_returns_bare_and_http_addresses() {
    let pd = Arc::new(MockPdClient::with_responses(vec![
        Ok(vec![member(&["http://127.0.0.1:1111"])]),
        Ok(vec![member(&["http://127.0.0.1:1111"])]),
    ]));
    let client = new_client(None, Some(pd)).expect("PD-only client");

    assert_eq!(
        client.get_pd_addrs(&Context::default()).unwrap(),
        ["127.0.0.1:1111"]
    );
    assert_eq!(
        client.get_pd_http_addrs(&Context::default()).unwrap(),
        ["http://127.0.0.1:1111"]
    );
    assert!(client.keyspace_etcd_client().is_none());
    assert!(new_client(None, None).is_none());
}

/// 成员发现遇临时错误会重试；全部成员无可用 URL 时报告与 Go 一致的错误文案。
#[test]
fn migration_member_discovery_retries_and_reports_go_errors() {
    let pd = Arc::new(MockPdClient::with_responses(vec![
        Err(MetaServiceError::Pd("temporary region miss".into())),
        Ok(vec![member(&[]), member(&["https://pd.example:2379"])]),
    ]));
    let client = new_client(None, Some(pd)).unwrap();
    assert_eq!(
        client.get_pd_addrs(&Context::default()).unwrap(),
        ["pd.example:2379"]
    );

    let empty = Arc::new(MockPdClient::with_responses(vec![Ok(vec![
        member(&[]),
        member(&[]),
    ])]));
    let error = new_client(None, Some(empty))
        .unwrap()
        .get_pd_addrs(&Context::default())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "no usable PD client URL found in PD members"
    );
}

/// `get_group`：空 meta 失败、地址 trim、默认 meta 回退全局分组。
#[test]
fn migration_get_group_matches_go_validation_and_fallbacks() {
    let pd_addrs = vec!["127.0.0.1:2379".to_owned()];
    assert!(matches!(
        get_group(None, &pd_addrs),
        Err(MetaServiceError::NilKeyspaceMeta)
    ));

    let meta = keyspace(
        "ks1",
        &[
            (GROUP_ID_KEY, "group1"),
            (GROUP_ADDRS_KEY, " 127.0.0.1:2388, ,127.0.0.1:2389,  "),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ],
    );
    let group = get_group(Some(&meta), &pd_addrs).unwrap();
    assert_eq!(group.group_id, "group1");
    assert_eq!(group.addrs, ["127.0.0.1:2388", "127.0.0.1:2389"]);

    let default_meta = KeyspaceMeta::default();
    let group = get_group(Some(&default_meta), &pd_addrs).unwrap();
    assert_eq!(group.group_id, GLOBAL_GROUP_ID);
    assert_eq!(group.addrs, pd_addrs);
}

/// 非法 group ID、统一 GC、缺失/空白地址均应拒绝，对齐 Go 校验。
#[test]
fn migration_get_group_rejects_invalid_or_incomplete_configuration() {
    let pd_addrs = vec!["127.0.0.1:2379".to_owned()];
    for group_id in ["1", "group 1", "group.1", "1-2_3"] {
        let meta = keyspace(
            "ks1",
            &[
                (GROUP_ID_KEY, group_id),
                (GROUP_ADDRS_KEY, "127.0.0.1:2388"),
                (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
            ],
        );
        assert!(matches!(
            get_group(Some(&meta), &pd_addrs),
            Err(MetaServiceError::InvalidGroupId)
        ));
    }

    let unified = keyspace(
        "ks-unified-gc",
        &[
            (GROUP_ID_KEY, "group1"),
            (GROUP_ADDRS_KEY, "127.0.0.1:2388"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_UNIFIED),
        ],
    );
    let error = get_group(Some(&unified), &pd_addrs).unwrap_err();
    assert!(matches!(
        error,
        MetaServiceError::KeyspaceLevelGcRequired { .. }
    ));
    assert!(error.to_string().contains("requires keyspace-level GC"));

    for addresses in [None, Some(" , \t,  ")] {
        let mut entries = vec![
            (GROUP_ID_KEY, "group1"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ];
        if let Some(addresses) = addresses {
            entries.push((GROUP_ADDRS_KEY, addresses));
        }
        let meta = keyspace("ks1", &entries);
        assert!(matches!(
            get_group(Some(&meta), &pd_addrs),
            Err(MetaServiceError::GroupNotMatch)
        ));
    }
}

/// `get_info` 无 meta 回退，以及经 PD mock 的 `get_info_and_group_addrs` 组合查询。
#[test]
fn migration_get_info_and_combined_lookup_match_go() {
    let pd_addrs = vec!["127.0.0.1:2379".to_owned()];
    let info = get_info(None, &pd_addrs).unwrap();
    assert_eq!(info.pd_addrs, pd_addrs);
    assert_eq!(info.group.group_id, GLOBAL_GROUP_ID);
    assert_eq!(info.group_addrs(), pd_addrs.as_slice());

    let pd = Arc::new(MockPdClient::with_responses(vec![Ok(vec![member(&[
        "http://127.0.0.1:2379",
    ])])]));
    let meta = keyspace(
        "ks1",
        &[
            (GROUP_ID_KEY, "group2"),
            (GROUP_ADDRS_KEY, "127.0.0.1:2388,127.0.0.1:2389"),
            (GC_MANAGEMENT_TYPE_KEY, GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL),
        ],
    );
    let (info, group_addrs) =
        get_info_and_group_addrs(&Context::default(), pd.as_ref(), Some(&meta)).unwrap();
    assert_eq!(info.pd_addrs, ["127.0.0.1:2379"]);
    assert_eq!(group_addrs, ["127.0.0.1:2388", "127.0.0.1:2389"]);
}

#[test]
fn member_discovery_skips_invalid_urls_and_preserves_unix_schemes() {
    for (with_scheme, expected) in [
        (
            false,
            vec![
                "pd.example:2379",
                "unix:///tmp/pd.sock",
                "unixs://localhost:m0",
            ],
        ),
        (
            true,
            vec![
                "https://pd.example:2379",
                "unix:///tmp/pd.sock",
                "unixs://localhost:m0",
            ],
        ),
    ] {
        let pd = MockPdClient::with_responses(vec![Ok(vec![
            member(&[
                "http://invalid",
                "https://pd.example:2379",
                "unix:///tmp/pd.sock",
            ]),
            member(&["ftp://invalid:2379", "unixs://localhost:m0"]),
        ])]);
        assert_eq!(
            super::get_pd_addrs(&Context::default(), &pd, with_scheme).unwrap(),
            expected
        );
    }
    let pd = MockPdClient::with_responses(vec![Ok(vec![member(&[
        "http://invalid",
        "ftp://invalid:2379",
    ])])]);
    assert!(matches!(
        super::get_pd_addrs(&Context::default(), &pd, false),
        Err(MetaServiceError::NoUsablePdUrl)
    ));
}

#[test]
fn service_url_alias_supports_unix_and_rejects_paths() {
    for (raw, scheme, address) in [
        ("unix://localhost:m0", "unix://", "localhost:m0"),
        ("unix:///tmp/pd.sock", "unix://", "/tmp/pd.sock"),
        ("unixs:///tmp/pd.sock", "unixs://", "/tmp/pd.sock"),
    ] {
        assert_eq!(parse_url(raw).unwrap(), (scheme.into(), address.into()));
        let pd = Arc::new(MockPdClient::with_responses(vec![Ok(vec![member(&[raw])])]));
        assert_eq!(
            new_client(None, Some(pd))
                .unwrap()
                .GetPDServiceURLs(&Context::default())
                .unwrap(),
            [raw]
        );
    }
    assert!(parse_url("http://localhost:2379/path").is_err());
}
