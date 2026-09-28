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

// 会话状态与令牌行为的迁移期综合单元测试。
//
// 对照 Go：状态类型常量、PreparedStmtInfo JSON/base64、缺证书错误、
// 令牌签发/校验/伪造/过期、证书宽限期与多算法、Starter 模式更长有效期、并发验签。

use super::*;
use chrono::Duration;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ED25519,
    PKCS_RSA_SHA256, PKCS_RSA_SHA384, PKCS_RSA_SHA512, SignatureAlgorithm,
};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;

/// 生成自签名证书与私钥 PEM，写入给定路径。
fn write_self_signed_cert(
    cert_path: &Path,
    key_path: &Path,
    algorithm: &'static SignatureAlgorithm,
) {
    let key = KeyPair::generate_for(algorithm).unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "session-token-test");
    let cert = params.self_signed(&key).unwrap();

    fs::write(cert_path, cert.pem()).unwrap();
    fs::write(key_path, key.serialize_pem()).unwrap();
}

/// 按指定 RSA 签名算法写测试证书对。
fn create_rsa_cert(cert_path: &Path, key_path: &Path, algorithm: &'static SignatureAlgorithm) {
    write_self_signed_cert(cert_path, key_path, algorithm);
}

/// 写 ECDSA P-256 测试证书对。
fn create_ec_cert(cert_path: &Path, key_path: &Path) {
    write_self_signed_cert(cert_path, key_path, &PKCS_ECDSA_P256_SHA256);
}

/// 写 Ed25519 测试证书对。
fn create_ed25519_cert(cert_path: &Path, key_path: &Path) {
    write_self_signed_cert(cert_path, key_path, &PKCS_ED25519);
}

/// 综合断言：状态常量、JSON 形状、令牌生命周期、宽限期、多算法与并发校验均与 Go 一致。
#[test]
fn session_state_and_token_behaviors_match_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(StatePrepareStmt, 0);
    assert_eq!(StateBinding, 1);

    let prepared = PreparedStmtInfo {
        StmtText: "select ?".to_owned(),
        ParamTypes: vec![3, 8],
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(&prepared).unwrap(),
        serde_json::json!({"text":"select ?","types":"Awg="})
    );
    assert_eq!(SessionStates::default().CurrentDB, "");

    // 无证书时 CreateSessionToken 应失败。
    ResetSigningCertForTest();
    let err = CreateSessionToken("test_user").unwrap_err();
    assert_eq!(err.code(), 8146);
    assert!(err.to_string().contains("no certificate or key file"));

    let first_dir = tempfile::tempdir().unwrap();
    let first_cert = first_dir.path().join("cert.pem");
    let first_key = first_dir.path().join("key.pem");
    create_rsa_cert(&first_cert, &first_key, &PKCS_RSA_SHA256);
    SetKeyPath(first_key.to_string_lossy().into_owned());
    SetCertPath(first_cert.to_string_lossy().into_owned());

    let token = CreateSessionToken("Test_User").unwrap();
    assert_eq!(token.ExpireTime - token.SignTime, Duration::minutes(1));
    let token_bytes = serde_json::to_vec(&token).unwrap();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&token_bytes).unwrap()["signature"].is_string()
    );
    ValidateSessionToken(&token_bytes, "test_user").unwrap();
    assert!(
        ValidateSessionToken(&token_bytes, "another_user")
            .unwrap_err()
            .to_string()
            .contains("username does not match")
    );

    // 篡改用户名后签名应失效。
    let mut forged = token.clone();
    forged.Username = "another_user".to_owned();
    let forged_bytes = serde_json::to_vec(&forged).unwrap();
    assert!(
        ValidateSessionToken(&forged_bytes, "another_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );

    // 时间偏移超过有效期应报 token expired。
    SetMockNowOffset(Duration::minutes(2));
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("token expired")
    );
    SetMockNowOffset(Duration::zero());

    let second_dir = tempfile::tempdir().unwrap();
    let second_cert = second_dir.path().join("cert.pem");
    let second_key = second_dir.path().join("key.pem");
    create_rsa_cert(&second_cert, &second_key, &PKCS_RSA_SHA256);
    SetKeyPath(second_key.to_string_lossy().into_owned());
    CreateSessionToken("test_user").unwrap(); // mismatched pair keeps the old certificate usable
    SetCertPath(second_cert.to_string_lossy().into_owned());
    ValidateSessionToken(&token_bytes, "test_user").unwrap(); // old cert remains in the grace window

    // 多线程并发校验同一令牌。
    let shared = Arc::new(serde_json::to_vec(&CreateSessionToken("worker").unwrap()).unwrap());
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                for _ in 0..20 {
                    ValidateSessionToken(&shared, "WORKER").unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    // 超出宽限期后旧令牌验签失败。
    SetMockNowOffset(LoadCertInterval + oldCertValidTime + Duration::minutes(1));
    ReloadSigningCert();
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    SetMockNowOffset(Duration::zero());

    for (name, create) in [
        ("rsa-sha384", create_rsa_sha384 as fn(&Path, &Path)),
        ("rsa-sha512", create_rsa_sha512 as fn(&Path, &Path)),
        ("ecdsa", create_ec_cert as fn(&Path, &Path)),
        ("ed25519", create_ed25519_cert as fn(&Path, &Path)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join(format!("{name}-cert.pem"));
        let key = dir.path().join(format!("{name}-key.pem"));
        create(&cert, &key);
        SetKeyPath(key.to_string_lossy().into_owned());
        SetCertPath(cert.to_string_lossy().into_owned());
        let bytes = serde_json::to_vec(&CreateSessionToken(name).unwrap()).unwrap();
        ValidateSessionToken(&bytes, &name.to_ascii_uppercase()).unwrap();
    }

    // Starter 模式使用更长令牌与证书重载间隔。
    if config::kerneltype::IsNextGen() {
        let original_mode = config::deploymode::Get();
        config::deploymode::Set(config::deploymode::Starter).unwrap();
        let starter_token = CreateSessionToken("starter").unwrap();
        assert_eq!(
            starter_token.ExpireTime - starter_token.SignTime,
            starterTokenLifetime
        );
        assert_eq!(GetLoadCertInterval(), starterLoadCertInterval);
        config::deploymode::Set(original_mode).unwrap();
    } else {
        assert!(config::deploymode::Set(config::deploymode::Starter).is_err());
    }

    assert!(ValidateSessionToken(b"not-json", "worker").is_err());
    ResetSigningCertForTest();
}

/// RSA-SHA384 证书辅助。
fn create_rsa_sha384(cert_path: &Path, key_path: &Path) {
    create_rsa_cert(cert_path, key_path, &PKCS_RSA_SHA384);
}

/// RSA-SHA512 证书辅助。
fn create_rsa_sha512(cert_path: &Path, key_path: &Path) {
    create_rsa_cert(cert_path, key_path, &PKCS_RSA_SHA512);
}
