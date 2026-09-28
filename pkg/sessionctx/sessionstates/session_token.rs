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

// 会话令牌（SessionToken）：代理在节点间迁移会话时的鉴权凭证。
//
// 对应 Go `session_token.go`。令牌含用户名、签发/过期时间与证书签名；
// 支持 RSA/ECDSA/Ed25519，证书可热加载并在宽限期内保留旧证书验签。
// Starter 部署模式下使用更长的令牌与证书有效期。

#![allow(non_snake_case, non_upper_case_globals)]

use crate::session_states::SessionStateError;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use config::deploymode;
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey, Private, Public};
use openssl::rsa::Padding;
use openssl::sign::{RsaPssSaltlen, Signer, Verifier};
use openssl::x509::X509;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fs;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, RwLock};

/// A token needs a short lifetime to limit brute-force attacks.
/// 普通模式下令牌有效期（1 分钟），缩短窗口以限制暴力尝试。
pub const tokenLifetime: Duration = Duration::minutes(1);
/// Starter 模式下令牌有效期（8 小时）。
pub const starterTokenLifetime: Duration = Duration::hours(8);
/// 普通模式下证书重载检查间隔（10 分钟）。
pub const LoadCertInterval: Duration = Duration::minutes(10);
/// Starter 模式下证书重载检查间隔（24 小时）。
pub const starterLoadCertInterval: Duration = Duration::hours(24);
/// 普通模式下旧证书验签宽限期（15 分钟）。
pub const oldCertValidTime: Duration = Duration::minutes(15);
/// Starter 模式下旧证书验签宽限期（36 小时）。
pub const starterOldCertValidTime: Duration = Duration::hours(36);

/// 按当前部署模式返回令牌有效期。
fn currentTokenLifetime() -> Duration {
    if deploymode::IsStarter() {
        starterTokenLifetime
    } else {
        tokenLifetime
    }
}

/// Returns the active interval for reloading session-token certificates.
/// 返回当前部署模式下的证书重载间隔。
pub fn GetLoadCertInterval() -> Duration {
    if deploymode::IsStarter() {
        starterLoadCertInterval
    } else {
        LoadCertInterval
    }
}

/// 按当前部署模式返回旧证书宽限期。
fn currentOldCertValidTime() -> Duration {
    if deploymode::IsStarter() {
        starterOldCertValidTime
    } else {
        oldCertValidTime
    }
}

/// 与 Go 一致：签名字节在 JSON 中以标准 base64 字符串表示。
mod go_signature_bytes {
    use super::*;

    /// 将签名字节编码为 base64 字符串。
    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(value))
    }

    /// 从 base64 字符串还原签名字节。
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)
    }
}

/// Token used by a proxy to authenticate a migrated session at another server.
/// 代理用于在目标节点鉴权已迁移会话的令牌。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionToken {
    #[serde(rename = "username")]
    /// 签发时的用户名（校验时大小写不敏感）。
    pub Username: String,
    #[serde(rename = "sign-time")]
    /// 签发时间（UTC）。
    pub SignTime: DateTime<Utc>,
    #[serde(rename = "expire-time")]
    /// 过期时间（UTC）。
    pub ExpireTime: DateTime<Utc>,
    #[serde(
        rename = "signature",
        default,
        with = "go_signature_bytes",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// 对（去掉 signature 字段后的）JSON 内容的数字签名；JSON 中为 base64。
    pub Signature: Vec<u8>,
}

/// 已加载的一张证书及其私钥与本地缓存过期时刻。
struct CertInfo {
    cert: X509,
    private_key: PKey<Private>,
    expire_time: DateTime<Utc>,
}

/// 进程内签名证书状态：路径、当前与宽限期内的旧证书列表。
#[derive(Default)]
struct SigningCert {
    cert_path: String,
    key_path: String,
    certs: Vec<CertInfo>,
}

impl SigningCert {
    /// 更新证书路径；路径变化时尝试重新加载。
    fn set_cert_path(&mut self, cert_path: String) {
        if cert_path != self.cert_path {
            self.cert_path = cert_path;
            let _ = self.check_and_load_cert();
        }
    }

    /// 更新私钥路径；路径变化时尝试重新加载。
    fn set_key_path(&mut self, key_path: String) {
        if key_path != self.key_path {
            self.key_path = key_path;
            let _ = self.check_and_load_cert();
        }
    }

    /// 证书与私钥路径均非空时才真正加载。
    fn check_and_load_cert(&mut self) -> Result<(), String> {
        if self.cert_path.is_empty() || self.key_path.is_empty() {
            return Ok(());
        }
        self.load_cert()
    }

    /// 从路径读取 PEM，校验公私钥匹配，并将新证书置于缓存头部、保留未过期的旧证书。
    fn load_cert(&mut self) -> Result<(), String> {
        let cert_pem = fs::read(&self.cert_path).map_err(|err| {
            format!(
                "load x509 failed, cert path: {}, key path: {}: {err}",
                self.cert_path, self.key_path
            )
        })?;
        let key_pem = fs::read(&self.key_path).map_err(|err| {
            format!(
                "load x509 failed, cert path: {}, key path: {}: {err}",
                self.cert_path, self.key_path
            )
        })?;
        let cert =
            X509::from_pem(&cert_pem).map_err(|err| format!("parse x509 cert failed: {err}"))?;
        let private_key = PKey::private_key_from_pem(&key_pem)
            .map_err(|err| format!("parse private key failed: {err}"))?;
        let public_key = cert.public_key().map_err(|err| err.to_string())?;
        // 公私钥不匹配时拒绝加载，保留原有缓存（调用方仍可用旧对）。
        if !private_key.public_eq(&public_key) {
            return Err("private key does not match public key".to_owned());
        }

        let now = get_now();
        // 过期时刻 = 当前 + 重载间隔 + 旧证宽限期，用于轮换后仍能验旧签名。
        let expire_time = now + GetLoadCertInterval() + currentOldCertValidTime();
        let old_certs = std::mem::take(&mut self.certs);
        let mut certs = Vec::with_capacity(old_certs.len() + 1);
        certs.push(CertInfo {
            cert,
            private_key,
            expire_time,
        });
        // 仅保留尚未超过本地 expire_time 的旧证书。
        certs.extend(
            old_certs
                .into_iter()
                .take_while(|info| now <= info.expire_time),
        );
        self.certs = certs;
        Ok(())
    }

    /// 用缓存中最新（首张）证书对内容签名。
    fn sign(&self, content: &[u8]) -> Result<Vec<u8>, String> {
        let info = self
            .certs
            .first()
            .ok_or_else(|| "no certificate or key file to sign the data".to_owned())?;
        sign_with_key(&info.cert, &info.private_key, content)
    }

    /// 按缓存顺序尝试验签，跳过已过本地宽限期的证书。
    fn check_signature(&self, content: &[u8], signature: &[u8]) -> Result<(), String> {
        let now = get_now();
        let mut last_error = None;
        for info in &self.certs {
            if now > info.expire_time {
                break;
            }
            let public_key = info.cert.public_key().map_err(|err| err.to_string())?;
            match verify_with_key(&info.cert, &public_key, content, signature) {
                Ok(()) => return Ok(()),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            format!(
                "no valid certificate to check the signature, cached certificates: {}",
                self.certs.len()
            )
        }))
    }
}

/// 全局签名证书状态（读写锁保护）。
static GLOBAL_SIGNING_CERT: LazyLock<RwLock<SigningCert>> =
    LazyLock::new(|| RwLock::new(SigningCert::default()));
/// 测试用“当前时间”相对真实时间的毫秒偏移（对应 Go failpoint mockNowOffset）。
static MOCK_NOW_OFFSET_MILLIS: AtomicI64 = AtomicI64::new(0);

/// 根据证书签名算法选出摘要算法（默认 SHA256）。
fn digest_for_certificate(cert: &X509) -> Result<MessageDigest, String> {
    let nid = cert.signature_algorithm().object().nid();
    if nid == Nid::RSASSAPSS {
        // OpenSSL exposes the outer RSASSA-PSS OID here; the selected hash lives
        // in the algorithm parameters. Its stable text form preserves that
        // parameter and avoids silently falling back to SHA-256.
        let text = String::from_utf8(cert.to_text().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        return if text.contains("Hash Algorithm: sha256") {
            Ok(MessageDigest::sha256())
        } else if text.contains("Hash Algorithm: sha384") {
            Ok(MessageDigest::sha384())
        } else if text.contains("Hash Algorithm: sha512") {
            Ok(MessageDigest::sha512())
        } else {
            Err("not supported RSA-PSS digest for signing".to_owned())
        };
    }
    let digest_nid = nid
        .signature_algorithms()
        .map(|algorithms| algorithms.digest)
        .unwrap_or(Nid::SHA256);
    match digest_nid {
        Nid::SHA256 => Ok(MessageDigest::sha256()),
        Nid::SHA384 => Ok(MessageDigest::sha384()),
        Nid::SHA512 => Ok(MessageDigest::sha512()),
        _ => Err(format!(
            "not supported private key type '{}' for signing",
            nid.long_name().unwrap_or("unknown")
        )),
    }
}

/// 若证书为 RSA-PSS，则为 Signer 配置 PSS padding 与 salt 长度。
fn configure_rsa_signer(signer: &mut Signer<'_>, cert: &X509) -> Result<(), String> {
    if cert.signature_algorithm().object().nid() == Nid::RSASSAPSS {
        signer
            .set_rsa_padding(Padding::PKCS1_PSS)
            .map_err(|err| err.to_string())?;
        signer
            .set_rsa_pss_saltlen(RsaPssSaltlen::DIGEST_LENGTH)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// 若证书为 RSA-PSS，则为 Verifier 配置与签名侧一致的 PSS 参数。
fn configure_rsa_verifier(verifier: &mut Verifier<'_>, cert: &X509) -> Result<(), String> {
    if cert.signature_algorithm().object().nid() == Nid::RSASSAPSS {
        verifier
            .set_rsa_padding(Padding::PKCS1_PSS)
            .map_err(|err| err.to_string())?;
        verifier
            .set_rsa_pss_saltlen(RsaPssSaltlen::DIGEST_LENGTH)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// 按私钥类型（Ed25519 / EC / RSA）对内容签名。
fn sign_with_key(cert: &X509, key: &PKey<Private>, content: &[u8]) -> Result<Vec<u8>, String> {
    match key.id() {
        Id::ED25519 => {
            let mut signer = Signer::new_without_digest(key).map_err(|err| err.to_string())?;
            signer
                .sign_oneshot_to_vec(content)
                .map_err(|err| err.to_string())
        }
        Id::EC => {
            let ec_key = key.ec_key().map_err(|err| err.to_string())?;
            EcdsaSig::sign(content, &ec_key)
                .and_then(|signature| signature.to_der())
                .map_err(|err| err.to_string())
        }
        Id::RSA | Id::RSA_PSS => {
            let digest = digest_for_certificate(cert)?;
            let mut signer = Signer::new(digest, key).map_err(|err| err.to_string())?;
            configure_rsa_signer(&mut signer, cert)?;
            signer.update(content).map_err(|err| err.to_string())?;
            signer.sign_to_vec().map_err(|err| err.to_string())
        }
        _ => Err(format!(
            "not supported private key type '{:?}' for signing",
            key.id()
        )),
    }
}

/// 按公钥类型验证签名；失败返回 `"verification error"` 等可读消息。
fn verify_with_key(
    cert: &X509,
    key: &PKey<Public>,
    content: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    let valid = match key.id() {
        Id::ED25519 => {
            let mut verifier = Verifier::new_without_digest(key).map_err(|err| err.to_string())?;
            verifier
                .verify_oneshot(signature, content)
                .map_err(|err| err.to_string())?
        }
        Id::EC => {
            let ec_key = key.ec_key().map_err(|err| err.to_string())?;
            EcdsaSig::from_der(signature)
                .and_then(|parsed| parsed.verify(content, &ec_key))
                .map_err(|err| err.to_string())?
        }
        Id::RSA | Id::RSA_PSS => {
            let digest = digest_for_certificate(cert)?;
            let mut verifier = Verifier::new(digest, key).map_err(|err| err.to_string())?;
            configure_rsa_verifier(&mut verifier, cert)?;
            verifier.update(content).map_err(|err| err.to_string())?;
            verifier.verify(signature).map_err(|err| err.to_string())?
        }
        _ => return Err(format!("not supported public key type '{:?}'", key.id())),
    };
    if valid {
        Ok(())
    } else {
        Err("verification error".to_owned())
    }
}

/// Compares strings using Unicode simple case folding, matching Go
/// `strings.EqualFold` without applying length-changing full case mappings.
fn equal_fold(left: &str, right: &str) -> bool {
    fn simple_fold(character: char) -> char {
        let mut lowercase = character.to_lowercase();
        let lowered = match (lowercase.next(), lowercase.next()) {
            (Some(value), None) => value,
            _ => character,
        };
        let mut uppercase = lowered.to_uppercase();
        let uppered = match (uppercase.next(), uppercase.next()) {
            (Some(value), None) => value,
            _ => lowered,
        };
        let mut canonical = uppered.to_lowercase();
        match (canonical.next(), canonical.next()) {
            (Some(value), None) => value,
            _ => uppered,
        }
    }

    left.chars()
        .map(simple_fold)
        .eq(right.chars().map(simple_fold))
}

/// Creates and signs a token for the proxy.
/// 为代理创建并签名会话令牌：先序列化无签名字段，再填入 Signature。
pub fn CreateSessionToken(username: impl Into<String>) -> Result<SessionToken, SessionStateError> {
    let now = get_now();
    let mut token = SessionToken {
        Username: username.into(),
        SignTime: now,
        ExpireTime: now + currentTokenLifetime(),
        Signature: Vec::new(),
    };
    // 签名对象是不含 signature 字段的 JSON（空 signature 被 skip）。
    let token_bytes = serde_json::to_vec(&token)?;
    token.Signature = GLOBAL_SIGNING_CERT
        .read()
        .expect("session signing certificate lock poisoned")
        .sign(&token_bytes)
        .map_err(SessionStateError::cannot_migrate)?;
    Ok(token)
}

/// Validates JSON, signature, expiry, lifetime, and case-insensitive username.
/// 校验 JSON、签名、过期时间、生命周期上限，以及用户名（忽略大小写）。
pub fn ValidateSessionToken(token_bytes: &[u8], username: &str) -> Result<(), SessionStateError> {
    let mut token: SessionToken = serde_json::from_slice(token_bytes)?;
    // 取出签名后重新序列化无签名内容，再验签。
    let signature = std::mem::take(&mut token.Signature);
    let unsigned = serde_json::to_vec(&token)?;
    GLOBAL_SIGNING_CERT
        .read()
        .expect("session signing certificate lock poisoned")
        .check_signature(&unsigned, &signature)
        .map_err(SessionStateError::cannot_migrate)?;

    let now = get_now();
    if now > token.ExpireTime {
        return Err(SessionStateError::cannot_migrate(format!(
            "token expired, {}",
            token.ExpireTime
        )));
    }
    // 签发时刻到现在不得超过当前模式允许的令牌生命周期。
    if token.SignTime + currentTokenLifetime() < now {
        return Err(SessionStateError::cannot_migrate(format!(
            "token lifetime is too long, {}",
            token.SignTime
        )));
    }
    if !equal_fold(username, &token.Username) {
        return Err(SessionStateError::cannot_migrate(format!(
            "username does not match, {username}, {}",
            token.Username
        )));
    }
    Ok(())
}

/// Sets the private-key path and immediately attempts to reload the pair.
/// 设置私钥路径并立即尝试重载证书对。
pub fn SetKeyPath(key_path: String) {
    GLOBAL_SIGNING_CERT
        .write()
        .expect("session signing certificate lock poisoned")
        .set_key_path(key_path);
}

/// Sets the certificate path and immediately attempts to reload the pair.
/// 设置证书路径并立即尝试重载证书对。
pub fn SetCertPath(cert_path: String) {
    GLOBAL_SIGNING_CERT
        .write()
        .expect("session signing certificate lock poisoned")
        .set_cert_path(cert_path);
}

/// Periodically reloads the configured certificate and rotates the cache.
/// 周期性重载配置的证书并轮换缓存（保留宽限期内旧证）。
pub fn ReloadSigningCert() {
    let _ = GLOBAL_SIGNING_CERT
        .write()
        .expect("session signing certificate lock poisoned")
        .check_and_load_cert();
}

/// Mirrors the Go failpoint used by token lifetime and certificate rotation tests.
/// 镜像 Go failpoint：为令牌生命周期与证书轮换测试注入时间偏移。
#[doc(hidden)]
pub fn SetMockNowOffset(offset: Duration) {
    MOCK_NOW_OFFSET_MILLIS.store(offset.num_milliseconds(), Ordering::SeqCst);
}

/// Clears process-global signing state so isolated harnesses do not leak state.
/// 清空进程级签名状态，避免测试间泄漏。
#[doc(hidden)]
pub fn ResetSigningCertForTest() {
    *GLOBAL_SIGNING_CERT
        .write()
        .expect("session signing certificate lock poisoned") = SigningCert::default();
    SetMockNowOffset(Duration::zero());
}

/// 返回带 mock 偏移的当前 UTC 时间。
fn get_now() -> DateTime<Utc> {
    Utc::now() + Duration::milliseconds(MOCK_NOW_OFFSET_MILLIS.load(Ordering::SeqCst))
}
