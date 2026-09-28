// Copyright 2026 AsterSQL.

//! 主密钥后端公开契约测试，对照 Go `br/pkg/encryption/master_key`。
//! 覆盖 IV 长度、内存/文件 AES-GCM 加解密、CreateBackend 拒绝明文、
//! MultiMasterKey 聚合，以及 Azure 云后端仍未实现的错误路径。
//! 测试使用临时目录写密钥文件，结束时清理；不依赖真实云厂商 SDK。

use crate::{
    Backend, CreateBackend, CtrIv16, GcmIv12, MasterKey, MasterKeyBackend, MasterKeyFile,
    MasterKeyKms, MetadataKeyMethod, MetadataMethodAes256Gcm, NewIVFromSlice, NewIVGcm,
    NewMemAesGcmBackend, NewMultiMasterKeyBackend, StorageVendorNameAzure, createCloudBackend,
    createFileBackend,
};
use astersql_br_pkg_kms::AwsKmsConfig;
use std::io::Write;

#[test]
fn go_rust_public_contract_matches() {
    // GCM IV 固定 12 字节；CTR 允许 16 字节切片，过短拒绝。
    assert_eq!(NewIVGcm().unwrap().Data.len(), GcmIv12);
    assert!(NewIVFromSlice(&[0u8; CtrIv16]).is_ok());
    assert!(NewIVFromSlice(&[0u8; 5]).is_err());

    // 内存 AES-GCM：密钥须 32 字节；密文元数据标注 aes256-gcm；往返明文一致。
    let key = [7u8; 32];
    let backend = NewMemAesGcmBackend(&key).unwrap();
    let enc = backend
        .EncryptContent(b"hello-master-key", &NewIVGcm().unwrap())
        .unwrap();
    // 元数据 method 字段须为 aes256-gcm，供解密路径选型。
    assert_eq!(
        enc.Metadata.get(MetadataKeyMethod).unwrap().as_slice(),
        MetadataMethodAes256Gcm.as_bytes()
    );
    assert_eq!(backend.DecryptContent(&enc).unwrap(), b"hello-master-key");
    // 非 32 字节密钥必须拒绝。
    assert!(NewMemAesGcmBackend(&[1, 2, 3]).is_err());

    // 文件后端：十六进制密钥文件；错误长度密钥文件创建失败。
    let dir = std::env::temp_dir().join(format!("mk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("key.txt");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        // 每字节两位 hex，凑齐 64 字符 = 32 字节密钥。
        write!(
            f,
            "{}\n",
            key.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
        .unwrap();
    }
    let fb = createFileBackend(path.to_str().unwrap()).unwrap();
    let enc2 = fb.Encrypt(b"file-plain").unwrap();
    // 文件后端加解密须往返一致。
    assert_eq!(fb.Decrypt(&enc2).unwrap(), b"file-plain");
    fb.Close();
    std::fs::write(dir.join("bad.txt"), b"short\n").unwrap();
    assert!(createFileBackend(dir.join("bad.txt").to_str().unwrap()).is_err());

    // CreateBackend：None / Plaintext 均拒绝；File 变体可解密既有密文。
    assert!(CreateBackend(None).is_err());
    assert!(
        CreateBackend(Some(&MasterKey {
            Backend: MasterKeyBackend::Plaintext
        }))
        .is_err()
    );
    let file_mk = MasterKey {
        Backend: MasterKeyBackend::File(MasterKeyFile {
            Path: path.to_str().unwrap().into(),
        }),
    };
    let mut any = CreateBackend(Some(&file_mk)).unwrap();
    // 工厂产物应能解密同一文件后端生成的密文。
    assert_eq!(any.Decrypt(&enc2).unwrap(), b"file-plain");
    any.Close();

    // MultiMasterKey：Go 的 `nil && len == 0` 只拒绝 nil，非 nil 空切片仍构造成功。
    let mut multi = NewMultiMasterKeyBackend(Some(&[file_mk])).unwrap();
    assert_eq!(multi.Decrypt(&enc2).unwrap(), b"file-plain");
    multi.Close();
    assert!(NewMultiMasterKeyBackend(None).is_err());
    assert!(NewMultiMasterKeyBackend(Some(&[])).is_ok());

    // protobuf oneof 未设置时，Go 工厂返回 unknown backend，Rust 也必须可表达该状态。
    let unset_err = match CreateBackend(Some(&MasterKey {
        Backend: MasterKeyBackend::Unset,
    })) {
        Ok(_) => panic!("unset backend must fail"),
        Err(err) => err,
    };
    assert!(unset_err.contains("unknown master key backend type"));

    // Azure 云后端尚为桩：错误消息须含 "not implemented Azure"。
    assert!(
        createCloudBackend(&MasterKeyKms {
            Vendor: StorageVendorNameAzure.into(),
            ..Default::default()
        })
        .err()
        .unwrap()
        .contains("not implemented Azure")
    );
    // 清理临时密钥目录，避免残留敏感材料。
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cloud_backends_use_production_kms_constructors() {
    // 构造过程必须完全离线：内置 WebPKI roots 不读取 native trust store，静态凭据
    // 阻止默认凭据链访问元数据服务，自定义 HTTP endpoint 也须原样保留。
    let aws = createCloudBackend(&MasterKeyKms {
        KeyId: "arn:aws:kms:us-east-1:123456789012:key/test".into(),
        Region: "us-east-1".into(),
        Endpoint: "http://127.0.0.1:1".into(),
        Vendor: crate::StorageVendorNameAWS.into(),
        AwsKms: Some(AwsKmsConfig {
            AccessKey: "test-access-key".into(),
            SecretAccessKey: "test-secret-key".into(),
        }),
        ..Default::default()
    });
    assert!(
        aws.is_ok(),
        "AWS cloud backend must be constructed like Go; got {:?}",
        aws.err()
    );

    let gcp = createCloudBackend(&MasterKeyKms {
        KeyId: "projects/test/locations/global/keyRings/ring/cryptoKeys/key".into(),
        Vendor: crate::StorageVendorNameGCP.into(),
        GcpKms: Some(Default::default()),
        ..Default::default()
    });
    assert!(
        gcp.is_ok(),
        "GCP cloud backend must be constructed like Go; got {:?}",
        gcp.err()
    );
}
