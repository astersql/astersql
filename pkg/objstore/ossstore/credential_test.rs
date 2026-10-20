// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// `CredentialRefresher` 单元测试：整快照发布、周期刷新与停止后不再拉取。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use task_ossstore::{CredentialRefresher, CredentialsProvider, ProviderCredentials};

struct SequenceProvider(Mutex<VecDeque<Result<ProviderCredentials, String>>>);

impl CredentialsProvider for SequenceProvider {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .expect("credential result sequence exhausted")
            .map_err(anyhow::Error::msg)
    }
}

fn credentials(access_key_id: &str) -> ProviderCredentials {
    ProviderCredentials {
        access_key_id: access_key_id.to_owned(),
        access_key_secret: "sk".to_owned(),
        security_token: String::new(),
        provider_name: "mock".to_owned(),
    }
}

/// 每次 `get_credentials` 递增序列号，用序列生成 AK/SK/Token。
struct CountingProvider(AtomicUsize);

impl CredentialsProvider for CountingProvider {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        let sequence = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ProviderCredentials {
            access_key_id: sequence.to_string(),
            access_key_secret: format!("secret-{sequence}"),
            security_token: format!("token-{sequence}"),
            provider_name: "mock".to_owned(),
        })
    }
}

/// 验证：未初始化报错 → 手动 refresh → 后台刷新到 ≥3 → close 后计数冻结。
#[test]
fn test_credential_refresher() {
    let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
    let refresher = Arc::new(CredentialRefresher::new(provider.clone()));

    assert!(
        refresher
            .get_credentials()
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    refresher.refresh_once().unwrap();
    let first = refresher.get_credentials().unwrap();
    assert_eq!(first.access_key_id, "1");

    // 5ms 间隔后台刷新，等待 provider 至少被调用 3 次。
    refresher
        .start_refresh_with_interval(Duration::from_millis(5))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while provider.0.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let refreshed = refresher.get_credentials().unwrap();
    assert!(refreshed.access_key_id.parse::<usize>().unwrap() >= 3);
    assert_eq!(
        refreshed.access_key_secret,
        format!("secret-{}", refreshed.access_key_id)
    );
    assert_eq!(
        refreshed.security_token,
        format!("token-{}", refreshed.access_key_id)
    );

    // close 后短暂等待，确认不再继续拉取。
    refresher.close();
    let stopped_at = provider.0.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(15));
    assert_eq!(provider.0.load(Ordering::SeqCst), stopped_at);
}

#[test]
fn fetch_credentials_retries_only_transient_ecs_metadata_failures() {
    let transient = concat!(
        "unable to get credentials from any of the providers in the chain: ",
        "get role name failed: Get http://100.100.100.200/latest/meta-data/ram/",
        "security-credentials/: dial tcp 100.100.100.200:80: i/o timeout"
    );
    let raw_transient = concat!(
        "get role name failed: Get http://100.100.100.200/latest/meta-data/ram/",
        "security-credentials/: context deadline exceeded"
    );
    let permanent = "open /home/pingcap/.aliyun/config.json: no such file or directory";

    let provider = SequenceProvider(Mutex::new(VecDeque::from([
        Err(transient.to_owned()),
        Err(raw_transient.to_owned()),
        Ok(credentials("ak")),
    ])));
    let credential = task_ossstore::fetch_credentials_with_policy(
        &storeapi::Context::default(),
        &provider,
        30,
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(credential.access_key_id, "ak");

    let provider = SequenceProvider(Mutex::new(VecDeque::from([
        Err(permanent.to_owned()),
        Ok(credentials("must-not-be-read")),
    ])));
    let error = task_ossstore::fetch_credentials_with_policy(
        &storeapi::Context::default(),
        &provider,
        30,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(error.to_string().contains("no such file or directory"));
    assert_eq!(provider.0.lock().unwrap().len(), 1);

    let provider = SequenceProvider(Mutex::new(VecDeque::from_iter(
        (0..30).map(|_| Err(raw_transient.to_owned())),
    )));
    let error = task_ossstore::fetch_credentials_with_policy(
        &storeapi::Context::default(),
        &provider,
        30,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(error.to_string().contains("context deadline exceeded"));
    assert!(provider.0.lock().unwrap().is_empty());

    let context = storeapi::Context::default();
    context.cancel();
    let provider = SequenceProvider(Mutex::new(VecDeque::from([Err(transient.to_owned())])));
    let error = task_ossstore::fetch_credentials_with_policy(
        &context,
        &provider,
        30,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
}

#[test]
fn transient_no_credentials_error_requires_ecs_metadata_timeout() {
    assert!(!task_ossstore::IsTransientNoCredentialsError(None));
    assert!(!task_ossstore::IsTransientNoCredentialsError(Some(
        "open /home/pingcap/.aliyun/config.json: no such file or directory"
    )));
    assert!(!task_ossstore::IsTransientNoCredentialsError(Some(
        "dial tcp 100.100.100.200:80: connect: connection refused"
    )));
    assert!(task_ossstore::IsTransientNoCredentialsError(Some(
        "Get http://100.100.100.200/latest/meta-data/: i/o timeout"
    )));
    assert!(task_ossstore::IsTransientNoCredentialsError(Some(
        "GET HTTP://100.100.100.200/latest/meta-data/: CONTEXT DEADLINE EXCEEDED"
    )));
}
