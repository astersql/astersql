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
