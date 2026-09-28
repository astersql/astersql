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

// Handshake 迁移对照单元测试。
//
// 校验 `Response41` 默认值与字段宽度、形状与 Go 侧零值及显式赋值一致，
// 避免机械迁移时丢失字段或改变整数类型宽度。

use super::handshake::Response41;
use std::collections::HashMap;

/// 默认构造应对齐 Go 的 Response41 零值（空串、空 map、整型 0）。
#[test]
fn response41_default_matches_go_zero_value() {
    let response = Response41::default();

    assert!(response.attrs.is_empty());
    assert!(response.user.is_empty());
    assert!(response.db_name.is_empty());
    assert!(response.auth_plugin.is_empty());
    assert!(response.auth.is_empty());
    assert_eq!(response.zstd_level, 0);
    assert_eq!(response.capability, 0);
    assert_eq!(response.collation, 0);
}

/// 显式填充全部字段后应原样保留，包括 capability（u32）与 collation（u8）。
#[test]
fn response41_preserves_every_go_field_and_integer_width() {
    // 连接属性（connect attrs）键值对，握手阶段可选元数据。
    let mut attrs = HashMap::new();
    attrs.insert(b"_client_name".to_vec(), b"mysql".to_vec());

    let response = Response41 {
        attrs: attrs.clone(),
        user: b"root".to_vec(),
        db_name: b"test".to_vec(),
        auth_plugin: b"caching_sha2_password".to_vec(),
        auth: vec![1, 2, 3],
        zstd_level: 22_isize,
        capability: 0x0102_0304,
        collation: 45,
    };

    assert_eq!(response.attrs, attrs);
    assert_eq!(response.user, b"root");
    assert_eq!(response.db_name, b"test");
    assert_eq!(response.auth_plugin, b"caching_sha2_password");
    assert_eq!(response.auth, [1, 2, 3]);
    assert_eq!(response.zstd_level, 22);
    assert_eq!(response.capability, 0x0102_0304);
    assert_eq!(response.collation, 45);
}
