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

// etcd MetaService 客户端单元测试。
//
// 覆盖仅 PD 客户端时的地址解析、双客户端为空返回 None、无可用 URL 时报错，
// 以及 `parse_url` 对 http(s)/IPv6 合法与非法输入的矩阵校验。

use super::{Context, MetaServiceError, PdClient, PdMember, get_pd_addrs, new_client, parse_url};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// 固定返回预设成员列表的 Mock PD 客户端。
struct MockPdClient {
    members: Vec<PdMember>,
}

impl PdClient for MockPdClient {
    fn get_all_members(&self, _ctx: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        Ok(self.members.clone())
    }
}

/// 始终返回 region miss，并记录调用次数，用于核对 TiKV BoRegionMiss 预算。
struct AlwaysFailPdClient {
    calls: Arc<AtomicUsize>,
}

impl PdClient for AlwaysFailPdClient {
    fn get_all_members(&self, _ctx: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(MetaServiceError::Pd("temporary region miss".to_owned()))
    }
}

/// 由 URL 字符串切片构造 PdMember。
fn member(urls: &[&str]) -> PdMember {
    PdMember {
        client_urls: urls.iter().map(|url| (*url).to_owned()).collect(),
    }
}

/// 仅提供 PD 客户端时，GetPDAddrs / GetPDHttpAddrs 应分别去掉与保留 scheme。
#[test]
fn test_get_pd_addrs_pd_only_client() {
    let pd_client = Arc::new(MockPdClient {
        members: vec![member(&["http://127.0.0.1:1111"])],
    });
    let service_client = new_client(None, Some(pd_client)).expect("PD client creates service");
    let ctx = Context::default();

    assert_eq!(
        service_client.GetPDAddrs(&ctx).unwrap(),
        vec!["127.0.0.1:1111"]
    );
    assert_eq!(
        service_client.GetPDHttpAddrs(&ctx).unwrap(),
        vec!["http://127.0.0.1:1111"]
    );
}

/// etcd 与 PD 皆未传入时 new_client 必须返回 None。
#[test]
fn test_new_client_returns_nil_without_clients() {
    assert!(new_client(None, None).is_none());
}

/// 占位 etcd 客户端类型，仅用于验证 Client 能持有并回传引用。
#[derive(Clone)]
struct TestEtcdClient;

/// 同时持有 etcd 与 PD 时能取回 etcd；成员无 URL 时应得到 NoUsablePdUrl。
#[test]
fn test_get_pd_addrs_with_real_client() {
    let etcd_client = Arc::new(TestEtcdClient);
    let pd_client = Arc::new(MockPdClient {
        members: vec![member(&["http://127.0.0.1:1111"])],
    });
    let service_client = new_client(Some(etcd_client.clone()), Some(pd_client)).unwrap();

    assert!(service_client.GetKeyspaceEtcdCli().is_some());
    assert_eq!(
        service_client.GetPDAddrs(&Context::default()).unwrap(),
        vec!["127.0.0.1:1111"]
    );

    // 成员存在但 client_urls 为空：无法解析出可用地址。
    let empty_urls = Arc::new(MockPdClient {
        members: vec![member(&[]), member(&[])],
    });
    let service_client = new_client(Some(etcd_client), Some(empty_urls)).unwrap();
    let error = service_client
        .GetPDAddrs(&Context::default())
        .expect_err("members without client URLs must fail");
    assert!(matches!(&error, MetaServiceError::NoUsablePdUrl));
    assert_eq!(
        error.to_string(),
        "no usable PD client URL found in PD members"
    );
}

/// Go 使用 BoRegionMiss(2ms, 500ms, no jitter)，累计休眠超过 5000ms 后
/// 返回 region unavailable；慢 PD 调用本身不消耗该休眠预算。
#[test]
fn test_get_pd_addrs_uses_go_region_miss_backoff() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pd_client = AlwaysFailPdClient {
        calls: calls.clone(),
    };

    let error = get_pd_addrs(&Context::default(), &pd_client, false)
        .expect_err("exhausted region-miss backoff must fail");

    assert!(
        matches!(&error, MetaServiceError::Pd(message) if message == "region unavailable"),
        "unexpected exhausted-backoff error: {error}"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 18);
}

/// parse_url 合法/非法用例矩阵（含 IPv6 方括号形式与缺端口等）。
#[test]
fn test_parse_url() {
    let cases = [
        (
            "http://example.com:8080",
            Some(("http://", "example.com:8080")),
        ),
        ("https://localhost:443", Some(("https://", "localhost:443"))),
        (
            "http://[2001:db8::1]:2379",
            Some(("http://", "[2001:db8::1]:2379")),
        ),
        (
            "https://[2001:db8::1]:443",
            Some(("https://", "[2001:db8::1]:443")),
        ),
        // Go's net/url + net.SplitHostPort accepts a decimal port without
        // constraining it to the TCP u16 range.
        (
            "http://example.com:65536",
            Some(("http://", "example.com:65536")),
        ),
        ("ftp://example.com", None),
        ("unix://localhost:m0", Some(("unix://", "localhost:m0"))),
        ("unix://localhost", Some(("unix://", "localhost"))),
        ("http://example.com:8080:extra", None),
        ("https://:8080", None),
        ("http://", None),
        ("https://example.com", None),
        ("http://localhost", None),
        ("https://[2001:db8::1]", None),
        ("http://2001:db8::1:2379", None),
        ("https://[2001:db8::1", None),
    ];

    for (raw_url, expected) in cases {
        match expected {
            Some((prefix, host_port)) => assert_eq!(
                parse_url(raw_url).unwrap_or_else(|error| panic!("{raw_url}: {error}")),
                (prefix.to_owned(), host_port.to_owned()),
                "input: {raw_url}"
            ),
            None => assert!(parse_url(raw_url).is_err(), "input should fail: {raw_url}"),
        }
    }
}
