// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// etcd 删除重试与命名空间的 Aster 补充单元测试。
//
// 用脚本化/卡住客户端验证：常量与 Go 一致、前缀拼接、错误重试、超时隔离与零重试空操作。

use async_trait::async_trait;
use std::collections::VecDeque;
use std::future::pending;
use std::io;
use std::time::Duration;
use util_etcd::{
    DeleteClient, KEY_OP_DEFAULT_RETRY_COUNT, KEY_OP_DEFAULT_TIMEOUT, KEY_OP_RETRY_INTERVAL,
    NamespacedClient, delete_key_from_etcd, set_etcd_client_namespace,
};

#[derive(Default)]
/// 按队列依次返回预置 delete 结果，并记录实际 key。
struct ScriptedClient {
    results: VecDeque<io::Result<()>>,
    keys: Vec<Vec<u8>>,
}

#[async_trait]
impl DeleteClient for ScriptedClient {
    type Error = io::Error;

    async fn delete(&mut self, key: Vec<u8>) -> Result<(), Self::Error> {
        self.keys.push(key);
        self.results.pop_front().unwrap_or(Ok(()))
    }
}

#[derive(Default)]
/// delete 永远挂起，用于验证每次尝试独立超时。
struct StalledClient {
    attempts: usize,
}

#[async_trait]
impl DeleteClient for StalledClient {
    type Error = io::Error;

    async fn delete(&mut self, _key: Vec<u8>) -> Result<(), Self::Error> {
        self.attempts += 1;
        pending().await
    }
}

#[test]
/// 默认超时、重试次数与间隔应与 Go 常量一致。
fn constants_match_go_durations_and_retry_count() {
    assert_eq!(KEY_OP_DEFAULT_TIMEOUT, Duration::from_secs(2));
    assert_eq!(KEY_OP_DEFAULT_RETRY_COUNT, 5);
    assert_eq!(KEY_OP_RETRY_INTERVAL, Duration::from_millis(30));
}

#[tokio::test]
/// 删除前应先拼接 namespace，与 Go namespaced 包装一致。
async fn namespace_is_applied_before_delete_like_go_client_wrappers() {
    let backend = ScriptedClient::default();
    let mut client = NamespacedClient::new(backend);
    set_etcd_client_namespace(&mut client, "testNamespace/");

    delete_key_from_etcd("testkey", &mut client, 1, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(client.inner().keys, vec![b"testNamespace/testkey"]);
}

#[tokio::test]
/// 遇错重试，首次成功即停止，不再继续尝试。
async fn delete_retries_each_error_and_stops_on_first_success() {
    let backend = ScriptedClient {
        results: VecDeque::from([
            Err(io::Error::other("first")),
            Err(io::Error::other("second")),
            Ok(()),
        ]),
        keys: Vec::new(),
    };
    let mut client = NamespacedClient::new(backend);

    delete_key_from_etcd("key", &mut client, 5, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(client.inner().keys, vec![b"key", b"key", b"key"]);
}

#[tokio::test]
/// 重试耗尽后返回最后一次错误。
async fn delete_returns_the_last_error_after_retry_budget_is_exhausted() {
    let backend = ScriptedClient {
        results: VecDeque::from([
            Err(io::Error::other("first")),
            Err(io::Error::other("last")),
        ]),
        keys: Vec::new(),
    };
    let mut client = NamespacedClient::new(backend);

    let error = delete_key_from_etcd("key", &mut client, 2, Duration::from_secs(1))
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "last");
    assert_eq!(client.inner().keys.len(), 2);
}

#[tokio::test]
/// 每次尝试各自超时；卡住客户端应被尝试次数次打断。
async fn every_delete_attempt_has_its_own_timeout() {
    let mut client = StalledClient::default();

    let error = delete_key_from_etcd("key", &mut client, 2, Duration::from_millis(1))
        .await
        .unwrap_err();

    assert_eq!(client.attempts, 2);
    assert_eq!(error.to_string(), "etcd delete timed out after 1ms");
}

#[tokio::test]
/// retry_count=0 时不发起删除，直接成功（Go 空操作语义）。
async fn zero_retry_count_matches_go_noop_result() {
    let mut client = NamespacedClient::new(ScriptedClient::default());
    delete_key_from_etcd("key", &mut client, 0, Duration::ZERO)
        .await
        .unwrap();
    assert!(client.inner().keys.is_empty());
}
