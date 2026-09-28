// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `storage` 模块基础测试：默认 HTTP 传输/客户端参数，以及通过 URL 构造内存对象存储。

use crate::hdfs::HDFSStorage;
use crate::memstore::MemStorage;
use crate::parse::ParseBackend;
use crate::storage::{
    CloneDefaultHTTPTransport, Context, GetDefaultHTTPClient, NewFromURL, NewWithDefaultOpt,
};

/// 校验默认 HTTP 传输配置：单主机连接上限为 0（不限制），空闲连接池非空。
#[test]
fn test_default_http_transport() {
    let (transport, ok) = CloneDefaultHTTPTransport();
    assert!(ok);
    assert_eq!(transport.max_connections_per_host, 0);
    assert!(transport.max_idle_connections > 0);
}

/// 按并发度构造默认客户端时，空闲连接与每主机空闲连接应等于该并发度。
#[test]
fn test_default_http_client() {
    let concurrency = 128;
    let transport = GetDefaultHTTPClient(concurrency).transport;
    assert_eq!(transport.max_idle_connections_per_host, concurrency);
    assert_eq!(transport.max_idle_connections, concurrency);
}

/// `memstore://` URL 应解析为进程内内存对象存储实现。
#[test]
fn test_new_mem_storage() {
    let storage = NewFromURL(&Context::background(), "memstore://").unwrap();
    assert!(storage.as_any().is::<MemStorage>());
}

#[test]
/// Go `New` 直接构造 HDFS 后端，不要求注入云存储 external factory。
fn test_new_hdfs_storage() {
    let backend = ParseBackend("hdfs://127.0.0.1:1231/backup", None).unwrap();
    let storage = NewWithDefaultOpt(&Context::background(), &backend).unwrap();

    assert!(storage.as_any().is::<HDFSStorage>());
    assert_eq!(storage.URI(), "hdfs://127.0.0.1:1231/backup");
}

/// Go only passes the context to cloud constructors; local constructors still succeed after
/// cancellation.
#[test]
fn test_new_hdfs_storage_ignores_cancelled_context() {
    let backend = ParseBackend("hdfs://127.0.0.1:1231/backup", None).unwrap();
    let ctx = Context::background();
    ctx.cancel();

    let storage = NewWithDefaultOpt(&ctx, &backend).unwrap();
    assert!(storage.as_any().is::<HDFSStorage>());
}
