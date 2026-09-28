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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use task_ossstore::{CredentialRefresher, CredentialsProvider, ProviderCredentials};

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
