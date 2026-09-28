// Copyright 2026 AsterSQL.

//! 对照 Go `br/pkg/kms` 公开契约的一致性测试。
//! 覆盖密钥包装边界、AWS 错误分类、GCP CRC/配置校验与 Provider 多态调度。
//! 使用注入假客户端，不发起真实云 API；失败表示 Rust/Go 语义漂移。

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    AwsDecryptClient, AwsDecryptError, AwsKmsConfig, CryptographyType, EncryptedKey,
    EncryptionVendorNameAwsKms, GcpDecryptClient, GcpDecryptResponse, GcpKmsConfig, MasterKeyKms,
    NewAwsKmsWithClient, NewEncryptedKey, NewGcpKmsWithClient, NewPlainKey, Provider,
    StorageVendorNameGcp, classifyDecryptError, crc32c,
};

// 成功路径假客户端：固定返回预设明文，忽略密文内容。
// 不校验 key_id，专注验证 AwsKms 对明文结果的透传。
struct OkAwsClient {
    plaintext: Vec<u8>,
}
impl AwsDecryptClient for OkAwsClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        _ciphertext: &[u8],
        _key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError> {
        Ok(self.plaintext.clone())
    }
}

// 失败路径假客户端：按给定 AWS 错误码构造 AwsDecryptError，驱动 classifyDecryptError。
// message 固定为 "boom"，断言时只关心 code→前缀映射是否与 Go 一致。
struct ErrAwsClient {
    code: &'static str,
}

struct RecordingAwsClient {
    request: Arc<Mutex<Option<(Vec<u8>, String)>>>,
}
impl AwsDecryptClient for RecordingAwsClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        ciphertext: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError> {
        *self.request.lock().unwrap() = Some((ciphertext.to_vec(), key_id.to_string()));
        Ok(b"recorded".to_vec())
    }
}
impl AwsDecryptClient for ErrAwsClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        _ciphertext: &[u8],
        _key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError> {
        Err(AwsDecryptError {
            code: self.code.into(),
            message: "boom".into(),
        })
    }
}

// GCP 成功假客户端：明文 CRC 用本地 crc32c 填充，模拟服务端正确响应。
// Close 恒成功，便于测试 Close 委托路径无额外噪音。
struct OkGcpClient {
    plaintext: Vec<u8>,
}
impl GcpDecryptClient for OkGcpClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        _name: &str,
        _ciphertext: &[u8],
        _ciphertext_crc32c: i64,
    ) -> Result<GcpDecryptResponse, String> {
        Ok(GcpDecryptResponse {
            Plaintext: self.plaintext.clone(),
            // 与实现侧校验同一算法，确保正常路径能通过 CRC 门禁。
            PlaintextCrc32C: crc32c(&self.plaintext) as i64,
        })
    }
    fn Close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

struct RecordingGcpClient {
    request: Arc<Mutex<Option<(String, Vec<u8>, i64)>>>,
    response: Result<GcpDecryptResponse, String>,
    closed: Arc<AtomicBool>,
    close_error: bool,
}
impl GcpDecryptClient for RecordingGcpClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        name: &str,
        ciphertext: &[u8],
        ciphertext_crc32c: i64,
    ) -> Result<GcpDecryptResponse, String> {
        *self.request.lock().unwrap() =
            Some((name.to_string(), ciphertext.to_vec(), ciphertext_crc32c));
        self.response.clone()
    }

    fn Close(&mut self) -> Result<(), String> {
        self.closed.store(true, Ordering::SeqCst);
        if self.close_error {
            Err("close failed".into())
        } else {
            Ok(())
        }
    }
}

fn gcp_config(key_id: &str) -> MasterKeyKms {
    MasterKeyKms {
        KeyId: key_id.into(),
        GcpKms: Some(GcpKmsConfig::default()),
        ..Default::default()
    }
}

#[test]
fn unknown_cryptography_type_matches_go_default_branch() {
    let unknown = CryptographyType(99);
    assert_eq!(unknown.TargetKeySize(), 0);
    let key = NewPlainKey(vec![1, 2, 3], unknown).unwrap();
    assert_eq!(key.KeyTag(), unknown);
    assert_eq!(key.Key(), &[1, 2, 3]);
}

#[test]
fn gcp_key_id_trims_exactly_one_trailing_slash() {
    let config = MasterKeyKms {
        KeyId: "projects/p/locations/l/keyRings/r/cryptoKeys/k//".into(),
        GcpKms: Some(GcpKmsConfig::default()),
        ..Default::default()
    };
    let gcp = NewGcpKmsWithClient(
        config,
        OkGcpClient {
            plaintext: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        gcp.config.KeyId,
        "projects/p/locations/l/keyRings/r/cryptoKeys/k/"
    );
}

#[test]
fn common_key_errors_and_plain_key_boundaries_match_go() {
    assert_eq!(
        NewEncryptedKey(Vec::new()).unwrap_err(),
        "encrypted key cannot be empty"
    );
    let mismatch =
        NewPlainKey(vec![0; 31], CryptographyType::CryptographyTypeAesGcm256).unwrap_err();
    assert_eq!(
        mismatch,
        "encryption method and key length mismatch, expect 32 got 31"
    );
    assert!(
        NewPlainKey(Vec::new(), CryptographyType::CryptographyTypePlain).is_ok(),
        "plain keys have no length limitation"
    );
}

#[test]
fn aws_constructor_request_and_all_error_classes_match_go() {
    let config = MasterKeyKms {
        KeyId: "key-id".into(),
        Region: "region".into(),
        Endpoint: "endpoint".into(),
        AwsKms: Some(AwsKmsConfig {
            AccessKey: "access".into(),
            SecretAccessKey: String::new(),
        }),
        GcpKms: None,
    };
    let request = Arc::new(Mutex::new(None));
    let aws = NewAwsKmsWithClient(
        &config,
        RecordingAwsClient {
            request: Arc::clone(&request),
        },
    )
    .unwrap();
    assert_eq!(aws.currentKeyID, "key-id");
    assert_eq!(aws.region, "region");
    assert_eq!(aws.endpoint, "endpoint");
    assert_eq!(aws.DecryptDataKey(b"cipher").unwrap(), b"recorded");
    assert_eq!(
        request.lock().unwrap().as_ref().unwrap(),
        &(b"cipher".to_vec(), "key-id".to_string())
    );

    for (code, prefix) in [
        ("NotFoundException", "wrong master key"),
        ("InvalidKeyUsageException", "wrong master key"),
        ("DependencyTimeoutException", "API timeout"),
        ("KMSInternalException", "API internal error"),
        ("OtherException", "KMS error"),
    ] {
        let error = classifyDecryptError(&AwsDecryptError {
            code: code.into(),
            message: "boom".into(),
        });
        assert_eq!(error, format!("{prefix}: {code}: boom"));
    }
}

#[test]
fn gcp_request_crc_response_crc_and_close_match_go() {
    let request = Arc::new(Mutex::new(None));
    let closed = Arc::new(AtomicBool::new(false));
    let plaintext = b"plain".to_vec();
    let mut gcp = NewGcpKmsWithClient(
        gcp_config("projects/p/locations/l/keyRings/r/cryptoKeys/k"),
        RecordingGcpClient {
            request: Arc::clone(&request),
            response: Ok(GcpDecryptResponse {
                Plaintext: plaintext.clone(),
                PlaintextCrc32C: crc32c(&plaintext) as i64,
            }),
            closed: Arc::clone(&closed),
            close_error: true,
        },
    )
    .unwrap();
    assert_eq!(gcp.DecryptDataKey(b"cipher").unwrap(), plaintext);
    assert_eq!(
        request.lock().unwrap().as_ref().unwrap(),
        &(
            "projects/p/locations/l/keyRings/r/cryptoKeys/k".to_string(),
            b"cipher".to_vec(),
            crc32c(b"cipher") as i64,
        )
    );
    gcp.Close();
    assert!(closed.load(Ordering::SeqCst));
}

#[test]
fn gcp_decrypt_errors_match_go() {
    let request = Arc::new(Mutex::new(None));
    let closed = Arc::new(AtomicBool::new(false));
    let transport_error = NewGcpKmsWithClient(
        gcp_config("projects/p/locations/l/keyRings/r/cryptoKeys/k"),
        RecordingGcpClient {
            request: Arc::clone(&request),
            response: Err("transport".into()),
            closed: Arc::clone(&closed),
            close_error: false,
        },
    )
    .unwrap();
    assert_eq!(
        transport_error.DecryptDataKey(b"cipher").unwrap_err(),
        "gcp kms decrypt request failed: transport"
    );

    let corrupt = NewGcpKmsWithClient(
        gcp_config("projects/p/locations/l/keyRings/r/cryptoKeys/k"),
        RecordingGcpClient {
            request,
            response: Ok(GcpDecryptResponse {
                Plaintext: b"plain".to_vec(),
                PlaintextCrc32C: 0,
            }),
            closed,
            close_error: false,
        },
    )
    .unwrap();
    assert_eq!(
        corrupt.DecryptDataKey(b"cipher").unwrap_err(),
        "response corrupted in-transit"
    );
}

#[test]
fn go_rust_public_contract_matches() {
    // Normal: EncryptedKey / PlainKey
    // 正常路径：密文/明文密钥构造、相等比较与 AES-GCM-256 长度标签。
    // Equal 比较的是内部字节，而非指针身份。
    let ek = NewEncryptedKey(vec![1, 2, 3]).unwrap();
    assert!(ek.Equal(&EncryptedKey(vec![1, 2, 3])));
    let pk = NewPlainKey(vec![0u8; 32], CryptographyType::CryptographyTypeAesGcm256).unwrap();
    assert_eq!(pk.Key().len(), 32);
    assert_eq!(pk.KeyTag(), CryptographyType::CryptographyTypeAesGcm256);
    // Plain 算法 TargetKeySize 为 0，表示不做长度限制。
    assert_eq!(CryptographyType::CryptographyTypePlain.TargetKeySize(), 0);

    // Boundary: empty encrypted / wrong length
    // 边界：空密文密钥与 AES 密钥长度不符均应失败，对齐 Go 校验。
    // 两字节材料对 AesGcm256 必然触发 length mismatch。
    assert!(NewEncryptedKey(vec![]).is_err());
    assert!(NewPlainKey(vec![1, 2], CryptographyType::CryptographyTypeAesGcm256).is_err());

    // AWS: decrypt + error classify
    // AWS：注入成功客户端验证 Name/Decrypt；再直接断言 NotFound 分类文案。
    // 厂商名常量必须为 "AWS"，与 TiKV 加密元数据约定一致。
    let cfg = MasterKeyKms {
        KeyId: "key-1".into(),
        Region: "us-west-2".into(),
        Endpoint: String::new(),
        AwsKms: Some(AwsKmsConfig {
            AccessKey: "ak".into(),
            SecretAccessKey: "sk".into(),
        }),
        GcpKms: None,
    };
    let aws = NewAwsKmsWithClient(
        &cfg,
        OkAwsClient {
            plaintext: b"plain".to_vec(),
        },
    )
    .unwrap();
    assert_eq!(aws.Name(), EncryptionVendorNameAwsKms);
    assert_eq!(aws.DecryptDataKey(b"cipher").unwrap(), b"plain");
    // NotFoundException → "wrong master key"，表示配置了错误的 CMK。
    assert_eq!(
        classifyDecryptError(&AwsDecryptError {
            code: "NotFoundException".into(),
            message: "x".into()
        }),
        "wrong master key: NotFoundException: x"
    );
    // DependencyTimeoutException 必须映射为 "API timeout" 前缀，供上层重试决策。
    let aws_err = NewAwsKmsWithClient(
        &cfg,
        ErrAwsClient {
            code: "DependencyTimeoutException",
        },
    )
    .unwrap();
    let e = aws_err.DecryptDataKey(b"c").unwrap_err();
    assert!(e.contains("API timeout"));

    // GCP: CRC path + missing config error
    // 缺 GcpKms 配置时构造必须失败，避免空凭证静默前进。
    assert!(
        NewGcpKmsWithClient(
            MasterKeyKms {
                GcpKms: None,
                ..Default::default()
            },
            OkGcpClient { plaintext: vec![] }
        )
        .is_err()
    );

    // KeyId 尾斜杠应被剥离；location 取 projects/p/locations/l 四段前缀。
    // Credential 路径仅作配置占位，假客户端不会读取该文件。
    let gcfg = MasterKeyKms {
        KeyId: "projects/p/locations/l/keyRings/r/cryptoKeys/k/".into(),
        GcpKms: Some(GcpKmsConfig {
            Credential: "/tmp/cred.json".into(),
        }),
        ..Default::default()
    };
    let mut gcp = NewGcpKmsWithClient(
        gcfg,
        OkGcpClient {
            plaintext: b"gplain".to_vec(),
        },
    )
    .unwrap();
    // Name 为小写 "gcp"；Decrypt 走 CRC 通过路径返回预设明文。
    assert_eq!(gcp.Name(), StorageVendorNameGcp);
    assert_eq!(gcp.location, "projects/p/locations/l");
    assert_eq!(gcp.DecryptDataKey(b"c").unwrap(), b"gplain");
    // checkCRC32：正确期望值通过，错误期望值报 mismatch。
    gcp.checkCRC32(b"gplain", crc32c(b"gplain") as i64).unwrap();
    assert!(gcp.checkCRC32(b"gplain", 1).is_err());
    // Close 应可安全调用，释放假客户端资源。
    gcp.Close();

    // Resource: Provider trait
    // 通过 trait 对象调用 Name，确认 AWS 后端可作为统一 Provider 调度。
    let p: &dyn Provider = &aws;
    assert_eq!(p.Name(), "AWS");
}
