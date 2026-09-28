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
// Copyright 2026 AsterSQL.

// TiDB Auth Token（基于 JWT）与 JWKS 加载/验签实现。
//
// Auth Token 用 JSON Web Token 代替传统密码登录：服务端用 JWKS
// （JSON Web Key Set，公钥集合）校验签名，再校验 claims（如 sub、email、
// iat/exp）。`JWKSImpl` 支持从文件加载密钥并在后台按间隔热更新。

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use serde_json::Value;

use crate::PrivilegeError;

/// 可协作取消的令牌，供 JWKS 热更新后台线程退出。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// 请求取消（置位标志）。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// JWKS 内存缓存与文件路径；负责加载、验签与可选的周期刷新。
#[derive(Debug, Default)]
pub struct JWKSImpl {
    /// 当前已加载的 JWK 集合；`None` 表示尚未成功加载。
    set: Arc<RwLock<Option<JwkSet>>>,
    /// JWKS JSON 文件路径。
    pub(crate) filepath: String,
}

impl JWKSImpl {
    /// 构造空的 JWKS 实现。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `filepath` 读取并解析 JWKS，替换内存中的密钥集。
    pub fn load(&self) -> Result<(), PrivilegeError> {
        let bytes = fs::read(&self.filepath).map_err(|err| PrivilegeError::Io(err.to_string()))?;
        let current: JwkSet = serde_json::from_slice(&bytes)
            .map_err(|err| PrivilegeError::InvalidJson(err.to_string()))?;
        *self.set.write().unwrap() = Some(current);
        Ok(())
    }

    /// 用当前 JWKS 验证 JWT 签名，成功则返回 claims 的 JSON 字节。
    ///
    /// 按 header 中的 `kid` 选钥；无 kid 且仅一把钥时回退到该钥。
    /// 此处关闭库内置的 exp/nbf 校验，寿命/过期由上层 claims 逻辑处理。
    pub fn verify(&self, token_bytes: &[u8]) -> Result<Vec<u8>, PrivilegeError> {
        let token = std::str::from_utf8(token_bytes)
            .map_err(|err| PrivilegeError::Authentication(err.to_string()))?;
        let header =
            decode_header(token).map_err(|err| PrivilegeError::Authentication(err.to_string()))?;
        let set = self.set.read().unwrap();
        let set = set
            .as_ref()
            .ok_or_else(|| PrivilegeError::Authentication("No valid JWKS yet".into()))?;
        // 按 kid 匹配；无 kid 时仅在单钥场景允许隐式选用。
        let jwk = match header.kid.as_deref() {
            Some(kid) => set.find(kid),
            None if set.keys.len() == 1 => set.keys.first(),
            None => None,
        }
        .ok_or_else(|| PrivilegeError::Authentication("no matching key in JWKS".into()))?;
        let key = DecodingKey::from_jwk(jwk)
            .map_err(|err| PrivilegeError::Authentication(err.to_string()))?;
        let mut validation = Validation::new(header.alg);
        // 过期/生效时间由 checkAuthTokenClaims 等上层逻辑校验。
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.required_spec_claims.clear();
        let data = decode::<Value>(token, &key, &validation)
            .map_err(|err| PrivilegeError::Authentication(err.to_string()))?;
        serde_json::to_vec(&data.claims).map_err(|err| PrivilegeError::InvalidJson(err.to_string()))
    }

    /// 加载 JWKS；若提供取消令牌则启动后台线程按 `interval` 热更新。
    ///
    /// 返回后台线程句柄；未提供取消令牌时只做一次加载并返回 `None`。
    pub fn LoadJWKS4AuthToken(
        &mut self,
        cancellation: Option<CancellationToken>,
        jwks_path: impl Into<String>,
        interval: Duration,
    ) -> Result<Option<JoinHandle<()>>, PrivilegeError> {
        self.filepath = jwks_path.into();
        // 与 Go 一致：先启动刷新任务，再执行首次同步加载。这样即使首次加载
        // 失败，调用方仍可修复文件，并由已启动的后台任务自动恢复。
        let refresh_handle = cancellation.map(|cancellation| {
            let set = Arc::clone(&self.set);
            let path = self.filepath.clone();
            thread::spawn(move || {
                // 周期读文件；解析成功才替换内存 JWKS，失败则保留旧集。
                while !cancellation.is_cancelled() {
                    thread::park_timeout(interval);
                    if cancellation.is_cancelled() {
                        break;
                    }
                    if let Ok(bytes) = fs::read(&path) {
                        if let Ok(current) = serde_json::from_slice::<JwkSet>(&bytes) {
                            *set.write().unwrap() = Some(current);
                        }
                    }
                }
            })
        });
        self.load()?;
        Ok(refresh_handle)
    }

    /// 验签并解析 claims；失败时重新 load JWKS 后重试，直至次数耗尽。
    pub fn checkSigWithRetry(
        &self,
        token_string: &str,
        mut retry_time: i32,
    ) -> Result<HashMap<String, Value>, PrivilegeError> {
        // JWT 必须是 header.payload.signature 三段。
        if token_string.split('.').count() != 3 {
            return Err(PrivilegeError::Authentication("Invalid JWT".into()));
        }
        let mut last_error = PrivilegeError::Authentication("Retry time has been spent out".into());
        while retry_time >= 0 {
            retry_time -= 1;
            match self.verify(token_string.as_bytes()) {
                Ok(payload) => {
                    let claims: HashMap<String, Value> = serde_json::from_slice(&payload)
                        .map_err(|err| PrivilegeError::InvalidJson(err.to_string()))?;
                    return Ok(claims);
                }
                Err(err) => {
                    // 验签失败可能因密钥刚轮换：重新加载后再试。
                    last_error = err;
                    self.load()?;
                }
            }
        }
        Err(PrivilegeError::Authentication(format!(
            "Retry time has been spent out: {last_error}"
        )))
    }
}

/// 进程级全局 JWKS 实例（惰性初始化）。
pub fn GlobalJWKS() -> &'static Mutex<JWKSImpl> {
    static GLOBAL: OnceLock<Mutex<JWKSImpl>> = OnceLock::new();
    GLOBAL.get_or_init(|| Mutex::new(JWKSImpl::default()))
}
