// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Resource Group Tag 编码与解码的单元测试。
//
// Resource Group Tag 是附在 KV 请求上的标签，用于资源管控与可观测性：
// 其中可携带 SQL digest（语句摘要）与 keyspace（键空间，多租户隔离单元）等信息。
// 本测试覆盖空标签、仅 digest、带 keyspace，以及 NextGen 内核下全局 keyspace 配置的编码行为。

use kv_dependency as kv;
use protobuf::Message;
use rand::Rng;
use resourcegrouptag_dependency::resource_group_tag::DecodeResourceGroupTag;

#[test]
fn go_merge_4_request_limiter_capacity_and_store_identity() {
    assert!(kv::NewCoprRequestLimiter(0).is_none());
    assert!(kv::NewCoprRequestLimiter(-1).is_none());
    let limiter = kv::NewCoprRequestLimiter(1).unwrap();
    assert_eq!(limiter.Capacity(), 1);
    assert!(limiter.TryAcquire());
    assert!(!limiter.TryAcquire());
    limiter.Release();
    assert!(limiter.TryAcquire());
    limiter.Release();

    assert!(kv::NewQueryCopStoreLimiter(0).is_none());
    assert!(kv::NewQueryCopStoreLimiter(-1).is_none());
    let stores = kv::NewQueryCopStoreLimiter(1).unwrap();
    assert_eq!(stores.Capacity(), 1);
    assert!(stores.GetStoreLimiter(0).is_none());
    let first = stores.GetStoreLimiter(1).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &stores.GetStoreLimiter(1).unwrap()
    ));
    assert!(!std::sync::Arc::ptr_eq(
        &first,
        &stores.GetStoreLimiter(2).unwrap()
    ));
}

#[test]
fn go_merge_4_limiter_waits_releases_and_cancels() {
    let limiter = kv::NewCoprRequestLimiter(1).unwrap();
    let done = tokio_util::sync::CancellationToken::new();
    assert!(limiter.TryAcquire());
    let (sender, receiver) = std::sync::mpsc::channel();
    let waiting_limiter = limiter.clone();
    let waiting_done = done.clone();
    let worker = std::thread::spawn(move || {
        let ctx = kv::Context::new();
        let exit = waiting_limiter.AcquireWithContext(&ctx, &waiting_done);
        sender.send(exit).unwrap();
        if !exit {
            waiting_limiter.Release();
        }
    });
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err()
    );
    limiter.Release();
    assert!(
        !receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
    );
    worker.join().unwrap();
    assert!(limiter.TryAcquire());
    limiter.Release();

    assert!(limiter.TryAcquire());
    done.cancel();
    assert!(limiter.AcquireWithContext(&kv::Context::new(), &done));
    limiter.Release();

    assert!(limiter.TryAcquire());
    let cancelled = kv::Context::new();
    cancelled.cancel();
    assert!(limiter.AcquireWithContext(&cancelled, &tokio_util::sync::CancellationToken::new()));
    limiter.Release();
}

#[test]
#[should_panic(expected = "release a redundant cop request token")]
fn go_merge_4_redundant_release_panics() {
    kv::NewCoprRequestLimiter(1).unwrap().Release();
}

#[test]
fn go_merge_4_per_store_limiters_isolate_capacity() {
    let stores = kv::NewQueryCopStoreLimiter(1).unwrap();
    let first = stores.GetStoreLimiter(1).unwrap();
    let second = stores.GetStoreLimiter(2).unwrap();
    assert!(first.TryAcquire());
    assert!(!first.TryAcquire());
    assert!(second.TryAcquire());
    second.Release();
    first.Release();
}

#[test]
fn go_merge_4_concurrent_attempts_respect_capacity() {
    let limiter = kv::NewCoprRequestLimiter(3).unwrap();
    let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..32 {
        let limiter = limiter.clone();
        let active = active.clone();
        let peak = peak.clone();
        workers.push(std::thread::spawn(move || {
            let ctx = kv::Context::new();
            let done = tokio_util::sync::CancellationToken::new();
            for _ in 0..20 {
                assert!(!limiter.AcquireWithContext(&ctx, &done));
                let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                std::thread::yield_now();
                active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                limiter.Release();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(peak.load(std::sync::atomic::Ordering::SeqCst) <= 3);
}

/// 生成指定长度的随机十六进制字节序列，用作变长 SQL digest 测试数据。
fn gen_rand_hex(length: usize) -> Vec<u8> {
    const CHARS: &[u8] = b"0123456789abcdef";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())])
        .collect()
}

/// 用给定 digest 与 keyspace 名构造 Resource Group Tag 的 protobuf 字节串。
fn encode(digest: Vec<u8>, keyspace_name: Vec<u8>) -> Vec<u8> {
    let mut builder = kv::NewResourceGroupTagBuilder(keyspace_name);
    builder.SetSQLDigest(kv::parser::Digest::new(digest));
    builder
        .EncodeTagWithKey(&[])
        .expect("resource group tag should encode")
}

/// 验证 Resource Group Tag 在多种 digest/keyspace 组合下的编解码正确性。
#[test]
fn test_resource_group_tag_encoding() {
    // 空 digest + 空 keyspace：标签仅含最小 protobuf 开销。
    let tag = encode(Vec::new(), Vec::new());
    assert_eq!(2, tag.len());
    assert_eq!(
        0,
        DecodeResourceGroupTag(&tag)
            .unwrap()
            .unwrap_or_default()
            .len()
    );
    let mut resource_tag = kv::tipb::ResourceGroupTag::new();
    resource_tag.merge_from_bytes(&tag).unwrap();
    assert!(!resource_tag.has_keyspace_name());

    // 仅有短 digest、无 keyspace。
    let digest = b"aa".to_vec();
    let tag = encode(digest.clone(), Vec::new());
    assert_eq!(6, tag.len());
    assert_eq!(Some(digest), DecodeResourceGroupTag(&tag).unwrap());

    // 同时携带 digest 与 keyspace 名。
    let keyspace_name = b"123".to_vec();
    let digest = gen_rand_hex(64);
    let tag = encode(digest.clone(), keyspace_name.clone());
    assert_eq!(Some(digest), DecodeResourceGroupTag(&tag).unwrap());
    resource_tag = kv::tipb::ResourceGroupTag::new();
    resource_tag.merge_from_bytes(&tag).unwrap();
    assert!(resource_tag.has_keyspace_name());
    assert_eq!(keyspace_name.as_slice(), resource_tag.get_keyspace_name());

    // 长 digest + 全局配置中的 keyspace；NextGen 与经典内核分支不同。
    let digest = gen_rand_hex(510);
    if kerneltype::IsNextGen() {
        // Go 的 config.initByLDFlags 在 `intest.InTest && kerneltype.IsNextGen()` 时
        // 会把全局 KeyspaceName 置为 "SYSTEM"（见 pkg/config/config.go）。Rust 端没有等价的
        // 进程启动期 init()，这里显式补齐同样的前置状态，保证下面的分支断言与 Go 行为一致。
        kv::config::update_global(|c| c.keyspace_name = keyspace::System.to_owned());
    }
    let configured_keyspace = keyspace::GetKeyspaceNameBytesBySettings();
    let tag = encode(digest.clone(), configured_keyspace.to_vec());
    assert_eq!(Some(digest), DecodeResourceGroupTag(&tag).unwrap());
    resource_tag = kv::tipb::ResourceGroupTag::new();
    resource_tag.merge_from_bytes(&tag).unwrap();
    if kerneltype::IsNextGen() {
        assert!(resource_tag.has_keyspace_name());
        assert_eq!(configured_keyspace, resource_tag.get_keyspace_name());
    } else {
        assert!(!resource_tag.has_keyspace_name());
    }
}
