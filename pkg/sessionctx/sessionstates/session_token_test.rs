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

// 对应 Go `session_token_test.go`：覆盖 JSON/base64、缺失证书、RSA/RSA-PSS/ECDSA/Ed25519、
// 验签失败、Unicode 大小写折叠、有效期、旧证宽限、重载与并发读写。

use super::*;
use chrono::{Duration, TimeZone, Utc};
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey, Private};
use openssl::pkey_ctx::PkeyCtx;
use openssl::rsa::Rsa;
use openssl::x509::{X509, X509NameBuilder};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;

/// 校验 SessionToken JSON 字段名与 signature 的 base64 往返与 Go 一致。
#[test]
fn session_token_json_matches_go_field_names_and_base64_signature() {
    let sign_time = Utc.with_ymd_and_hms(2026, 7, 23, 8, 9, 10).unwrap();
    let token = SessionToken {
        Username: "Test_User".to_owned(),
        SignTime: sign_time,
        ExpireTime: sign_time + Duration::minutes(1),
        Signature: vec![0, 1, 2, 0xff],
    };

    let value = serde_json::to_value(&token).unwrap();
    assert_eq!(value["username"], "Test_User");
    assert!(value.get("sign-time").is_some());
    assert!(value.get("expire-time").is_some());
    assert_eq!(value["signature"], "AAEC/w==");

    let decoded: SessionToken = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.Username, token.Username);
    assert_eq!(decoded.SignTime, token.SignTime);
    assert_eq!(decoded.ExpireTime, token.ExpireTime);
    assert_eq!(decoded.Signature, token.Signature);
}

/// 无签名证书时 CreateSessionToken 应返回 ErrCannotMigrateSession。
#[test]
fn signing_without_certificate_returns_cannot_migrate() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let error = CreateSessionToken("test_user").unwrap_err();
    assert_eq!(error.code(), errno::errcode::ErrCannotMigrateSession);
    assert!(error.to_string().contains("no certificate or key file"));
}

fn write_x509_pair(cert_path: &Path, key_path: &Path, key: &PKey<Private>, digest: MessageDigest) {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "session-token-test")
        .unwrap();
    let name = name.build();
    let serial = BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap();
    let not_before = Asn1Time::days_from_now(0).unwrap();
    let not_after = Asn1Time::days_from_now(1).unwrap();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&name).unwrap();
    builder.set_issuer_name(&name).unwrap();
    builder.set_pubkey(key).unwrap();
    builder.set_not_before(&not_before).unwrap();
    builder.set_not_after(&not_after).unwrap();
    builder.sign(key, digest).unwrap();
    fs::write(cert_path, builder.build().to_pem().unwrap()).unwrap();
    fs::write(key_path, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
}

fn write_rsa_x509_pair(cert_path: &Path, key_path: &Path, bits: u32, digest: MessageDigest) {
    let key = PKey::from_rsa(Rsa::generate(bits).unwrap()).unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_rsa_pss_x509_pair(cert_path: &Path, key_path: &Path, bits: u32, digest: MessageDigest) {
    let mut context = PkeyCtx::new_id(Id::RSA_PSS).unwrap();
    context.keygen_init().unwrap();
    context.set_rsa_keygen_bits(bits).unwrap();
    let key = context.keygen().unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_ecdsa_x509_pair(cert_path: &Path, key_path: &Path, curve: Nid, digest: MessageDigest) {
    let group = EcGroup::from_curve_name(curve).unwrap();
    let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
    write_x509_pair(cert_path, key_path, &key, digest);
}

fn write_ed25519_x509_pair(cert_path: &Path, key_path: &Path) {
    let key = PKey::generate_ed25519().unwrap();
    write_x509_pair(cert_path, key_path, &key, MessageDigest::null());
}

fn assert_token_round_trip(username: &str) {
    ReloadSigningCert();
    let token = CreateSessionToken(username).unwrap();
    ValidateSessionToken(&serde_json::to_vec(&token).unwrap(), username).unwrap();
}

/// 对应 Go TestSetCertAndKey：缺路径失败、完整路径成功、不匹配密钥保留旧证书。
#[test]
fn set_cert_and_key_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());

    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(String::new());
    SetCertPath(cert_path.to_string_lossy().into_owned());
    assert!(CreateSessionToken("test_user").is_err());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    CreateSessionToken("test_user").unwrap();

    let other_key_path = directory.path().join("other-key.pem");
    let other_cert_path = directory.path().join("other-cert.pem");
    write_rsa_x509_pair(
        &other_cert_path,
        &other_key_path,
        4096,
        MessageDigest::sha256(),
    );
    SetKeyPath(other_key_path.to_string_lossy().into_owned());
    CreateSessionToken("test_user").unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestSignAlgo：覆盖 RSA PKCS#1/PSS、ECDSA、Ed25519 与密钥尺寸矩阵。
#[test]
fn signing_algorithm_matrix_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    for bits in [2048, 4096] {
        for (name, digest) in [
            ("sha256", MessageDigest::sha256()),
            ("sha384", MessageDigest::sha384()),
            ("sha512", MessageDigest::sha512()),
        ] {
            write_rsa_x509_pair(&cert_path, &key_path, bits, digest);
            assert_token_round_trip(&format!("rsa-{name}-{bits}"));
            write_rsa_pss_x509_pair(&cert_path, &key_path, bits, digest);
            assert_token_round_trip(&format!("rsa-pss-{name}-{bits}"));
        }
    }
    for (name, curve, digest) in [
        ("sha256", Nid::X9_62_PRIME256V1, MessageDigest::sha256()),
        ("sha384", Nid::SECP384R1, MessageDigest::sha384()),
        ("sha512", Nid::SECP521R1, MessageDigest::sha512()),
    ] {
        write_ecdsa_x509_pair(&cert_path, &key_path, curve, digest);
        assert_token_round_trip(&format!("ecdsa-{name}"));
    }
    write_ed25519_x509_pair(&cert_path, &key_path);
    assert_token_round_trip("ed25519");
    ResetSigningCertForTest();
}

/// 对应 Go TestVerifyToken：正常、过期、用户名不匹配及字段伪造。
#[test]
fn token_verification_failures_match_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 4096, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("test_user").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    ValidateSessionToken(&token_bytes, "test_user").unwrap();
    SetMockNowOffset(tokenLifetime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("token expired")
    );
    SetMockNowOffset(Duration::zero());
    assert!(
        ValidateSessionToken(&token_bytes, "another_user")
            .unwrap_err()
            .to_string()
            .contains("username does not match")
    );

    let mut forged = token.clone();
    forged.Username = "another_user".to_owned();
    assert!(
        ValidateSessionToken(&serde_json::to_vec(&forged).unwrap(), "another_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    forged.Username = "test_user".to_owned();
    forged.ExpireTime = Utc::now() - Duration::minutes(1);
    assert!(
        ValidateSessionToken(&serde_json::to_vec(&forged).unwrap(), "test_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    ResetSigningCertForTest();
}

/// Go `strings.EqualFold` uses Unicode simple case folding, including the
/// ordinary and final lowercase forms of Greek sigma.
#[test]
fn token_username_uses_go_unicode_case_folding() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("Σ").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    ValidateSessionToken(&token_bytes, "ς").unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestStarterSessionTokenLifetime。
#[test]
fn starter_token_lifetime_matches_go() {
    // Go TestStarterSessionTokenLifetime only runs in NextGen.
    if !config::kerneltype::IsNextGen() {
        return;
    }
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let original_mode = config::deploymode::Get();
    config::deploymode::Set(config::deploymode::Starter).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    let token = CreateSessionToken("test_user").unwrap();
    let token_bytes = serde_json::to_vec(&token).unwrap();
    assert_eq!(token.ExpireTime - token.SignTime, starterTokenLifetime);
    assert_eq!(GetLoadCertInterval(), starterLoadCertInterval);
    SetMockNowOffset(tokenLifetime + Duration::minutes(1));
    ValidateSessionToken(&token_bytes, "test_user").unwrap();
    SetMockNowOffset(starterTokenLifetime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&token_bytes, "test_user")
            .unwrap_err()
            .to_string()
            .contains("token expired")
    );

    config::deploymode::Set(original_mode).unwrap();
    ResetSigningCertForTest();
}

/// 对应 Go TestCertExpire：轮换后旧证书先可用，超过宽限期后失效。
#[test]
fn certificate_grace_period_matches_go() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());
    let old_token = serde_json::to_vec(&CreateSessionToken("test_user").unwrap()).unwrap();

    let new_cert_path = directory.path().join("new-cert.pem");
    let new_key_path = directory.path().join("new-key.pem");
    write_rsa_x509_pair(&new_cert_path, &new_key_path, 2048, MessageDigest::sha256());
    SetKeyPath(new_key_path.to_string_lossy().into_owned());
    SetCertPath(new_cert_path.to_string_lossy().into_owned());
    ValidateSessionToken(&old_token, "test_user").unwrap();

    SetMockNowOffset(LoadCertInterval);
    ReloadSigningCert();
    SetMockNowOffset(LoadCertInterval + oldCertValidTime + Duration::minutes(1));
    assert!(
        ValidateSessionToken(&old_token, "test_user")
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    ResetSigningCertForTest();
}

/// 对应 Go TestLoadAndReadConcurrently：并发写证书、重载、签发和验签。
#[test]
fn certificate_reload_and_reads_are_concurrent_safe() {
    let _guard = SESSION_TOKEN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ResetSigningCertForTest();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = Arc::new(directory.path().join("cert.pem"));
    let key_path = Arc::new(directory.path().join("key.pem"));
    write_rsa_x509_pair(&cert_path, &key_path, 2048, MessageDigest::sha256());
    SetKeyPath(key_path.to_string_lossy().into_owned());
    SetCertPath(cert_path.to_string_lossy().into_owned());

    // Key generation is not part of the concurrency contract. Build two
    // immutable fixtures once, then race only file replacement, reloads and reads.
    let fixture_directory = tempfile::tempdir().unwrap();
    let fixtures = Arc::new(
        (0..2)
            .map(|id| {
                let fixture_cert = fixture_directory.path().join(format!("cert-{id}.pem"));
                let fixture_key = fixture_directory.path().join(format!("key-{id}.pem"));
                write_ecdsa_x509_pair(
                    &fixture_cert,
                    &fixture_key,
                    Nid::X9_62_PRIME256V1,
                    MessageDigest::sha256(),
                );
                (
                    fs::read(fixture_cert).unwrap(),
                    fs::read(fixture_key).unwrap(),
                )
            })
            .collect::<Vec<_>>(),
    );
    let barrier = Arc::new(std::sync::Barrier::new(7));

    let writer_cert = Arc::clone(&cert_path);
    let writer_key = Arc::clone(&key_path);
    let writer_fixtures = Arc::clone(&fixtures);
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        writer_barrier.wait();
        for id in 0..16 {
            let (cert, key) = &writer_fixtures[id % writer_fixtures.len()];
            fs::write(&*writer_cert, cert).unwrap();
            fs::write(&*writer_key, key).unwrap();
        }
        16
    });
    let loaders: Vec<_> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..32 {
                    ReloadSigningCert();
                }
                32
            })
        })
        .collect();
    let readers: Vec<_> = (0..3)
        .map(|id| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let username = format!("test_user_{id}");
                for _ in 0..32 {
                    let bytes =
                        serde_json::to_vec(&CreateSessionToken(&username).unwrap()).unwrap();
                    ValidateSessionToken(&bytes, &username).unwrap();
                }
                32
            })
        })
        .collect();

    barrier.wait();
    assert_eq!(writer.join().unwrap(), 16);
    for loader in loaders {
        assert_eq!(loader.join().unwrap(), 32);
    }
    for reader in readers {
        assert_eq!(reader.join().unwrap(), 32);
    }
    ResetSigningCertForTest();
}
