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

// OSS 访问凭证：静态密钥、reqsign 默认链，以及并发安全的周期刷新器。
//
// 备份/导入访问阿里云 OSS 时需要 AccessKey；STS（Security Token Service）
// 场景下密钥会过期，`CredentialRefresher` 在后台线程整快照替换，读者无锁读取。

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use arc_swap::ArcSwapOption;
use reqsign_core::ProvideCredential;

/// 一次完整的凭证快照：AK/SK、可选 STS Token，以及来源名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderCredentials {
    /// Access Key ID。
    pub access_key_id: String,
    /// Access Key Secret。
    pub access_key_secret: String,
    /// STS 临时安全令牌；静态密钥时通常为空。
    pub security_token: String,
    /// 提供者标识（如 `static`、`reqsign_default`）。
    pub provider_name: String,
}

/// 凭证提供者抽象：按需返回一整份 `ProviderCredentials`。
pub trait CredentialsProvider: Send + Sync {
    /// 拉取当前可用凭证；失败时返回错误。
    fn get_credentials(&self) -> Result<ProviderCredentials>;
}

/// 固定不变的静态凭证提供者。
#[derive(Clone)]
pub struct StaticCredentialsProvider {
    credentials: ProviderCredentials,
}

impl StaticCredentialsProvider {
    /// 用 AK/SK 与可选 STS Token 构造静态提供者。
    pub fn new(access_key_id: String, access_key_secret: String, security_token: String) -> Self {
        Self {
            credentials: ProviderCredentials {
                access_key_id,
                access_key_secret,
                security_token,
                provider_name: "static".to_owned(),
            },
        }
    }
}

impl CredentialsProvider for StaticCredentialsProvider {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        Ok(self.credentials.clone())
    }
}

/// 基于 reqsign 阿里云默认凭证链的提供者（环境变量、AssumeRole 等）。
pub struct ReqsignCredentialsProvider {
    /// 阻塞调用 async 凭证链所需的 tokio 运行时。
    runtime: tokio::runtime::Runtime,
    /// reqsign 执行上下文（环境、文件读、HTTP）。
    context: reqsign_core::Context,
    /// 阿里云默认凭证链实现。
    provider: reqsign_aliyun_oss::DefaultCredentialProvider,
}

impl ReqsignCredentialsProvider {
    /// 构造默认链；`role_arn` 非空时启用 AssumeRole（可带 external_id）。
    pub fn new(role_arn: &str, external_id: &str) -> Result<Self> {
        let mut builder = reqsign_aliyun_oss::DefaultCredentialProvider::builder();
        if !role_arn.is_empty() {
            // STS AssumeRole：用角色 ARN 换取临时凭证。
            let mut assume_role = reqsign_aliyun_oss::AssumeRoleCredentialProvider::new()
                .with_role_arn(role_arn)
                .with_role_session_name("tidb-ossstore");
            if !external_id.is_empty() {
                assume_role = assume_role.with_external_id(external_id);
            }
            builder = builder.assume_role(assume_role);
        }
        let context = reqsign_core::Context::new()
            .with_env(reqsign_core::OsEnv)
            .with_file_read(reqsign_file_read_tokio::TokioFileRead)
            .with_http_send(reqsign_http_send_reqwest::ReqwestHttpSend::default());
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
            context,
            provider: builder.build(),
        })
    }
}

impl CredentialsProvider for ReqsignCredentialsProvider {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        let credential = self
            .runtime
            .block_on(self.provider.provide_credential(&self.context))?
            .ok_or_else(|| {
                anyhow!("no credentials found in the Aliyun default credential chain")
            })?;
        Ok(ProviderCredentials {
            access_key_id: credential.access_key_id,
            access_key_secret: credential.access_key_secret,
            security_token: credential.security_token.unwrap_or_default(),
            provider_name: "reqsign_default".to_owned(),
        })
    }
}

/// Concurrency-safe credential snapshot refresher. The provider itself is only
/// called by this object, while readers load immutable whole snapshots.
/// 并发安全的凭证快照刷新器：仅本对象调用底层 provider，读者加载不可变整快照。
pub struct CredentialRefresher {
    /// 真正向云端/本地链拉取凭证的提供者。
    provider: Arc<dyn CredentialsProvider>,
    /// 当前已发布的整快照；`None` 表示尚未初始化。
    credentials: ArcSwapOption<ProviderCredentials>,
    /// 停止标志 + Condvar，用于唤醒后台刷新线程退出。
    stop: Arc<(Mutex<bool>, Condvar)>,
    /// 后台刷新线程句柄；`None` 表示未启动。
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CredentialRefresher {
    /// 创建尚未拉取任何快照的刷新器。
    pub fn new(provider: Arc<dyn CredentialsProvider>) -> Self {
        Self {
            provider,
            credentials: ArcSwapOption::empty(),
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            worker: Mutex::new(None),
        }
    }

    /// 同步拉取一次并原子替换快照。
    pub fn refresh_once(&self) -> Result<()> {
        let credentials = self.provider.get_credentials()?;
        self.credentials.store(Some(Arc::new(credentials)));
        Ok(())
    }

    /// 以默认 5 秒间隔启动后台刷新。
    pub fn start_refresh(self: &Arc<Self>) -> Result<()> {
        self.start_refresh_with_interval(Duration::from_secs(5))
    }

    /// 先 `refresh_once`，再按 `interval` 启动唯一后台线程周期刷新。
    pub fn start_refresh_with_interval(self: &Arc<Self>, interval: Duration) -> Result<()> {
        self.refresh_once()?;
        let mut worker = self
            .worker
            .lock()
            .expect("credential worker mutex poisoned");
        // 已有后台线程则幂等返回。
        if worker.is_some() {
            return Ok(());
        }
        *self.stop.0.lock().expect("credential stop mutex poisoned") = false;
        let this = Arc::clone(self);
        *worker = Some(thread::spawn(move || {
            loop {
                let (lock, wake) = &*this.stop;
                let stopped = lock.lock().expect("credential stop mutex poisoned");
                // 等待 interval 或被 close 唤醒；stopped=true 时退出。
                let (stopped, _) = wake
                    .wait_timeout_while(stopped, interval, |stopped| !*stopped)
                    .expect("credential wait mutex poisoned");
                if *stopped {
                    return;
                }
                drop(stopped);
                if let Err(error) = this.refresh_once() {
                    log::warn!("failed to refresh OSS credentials: {error:#}");
                }
            }
        }));
        Ok(())
    }

    /// 通知后台线程停止并 join；Drop 时也会调用。
    pub fn close(&self) {
        {
            let (lock, wake) = &*self.stop;
            *lock.lock().expect("credential stop mutex poisoned") = true;
            wake.notify_all();
        }
        if self
            .worker
            .lock()
            .expect("credential worker mutex poisoned")
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            log::warn!("OSS credential refresher thread panicked while stopping");
        }
    }
}

impl CredentialsProvider for CredentialRefresher {
    fn get_credentials(&self) -> Result<ProviderCredentials> {
        self.credentials
            .load_full()
            .map(|credential| (*credential).clone())
            .context("credentials not initialized")
    }
}

impl Drop for CredentialRefresher {
    fn drop(&mut self) {
        self.close();
    }
}
