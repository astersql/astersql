// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 脱敏模块迁移对齐测试：对照 Go 行为验证核心 API。
//
// 覆盖 MARKER 转义、DeRedact 行分隔、全局 Value/Key/WriteRedact，
// 以及 TaskInfoRedacted 对各存储后端凭证遮盖且不改动原对象。

use crate::REDACT_TEST_LOCK;
use crate::redact::{
    DeRedact, FmtStringer, InitRedact, Key, NeedRedact, String as RedactString, Stringer,
    TaskInfoRedacted, Value, WriteRedact,
};
use kvproto::brpb::{
    AzureBlobStorage, AzureCustomerKey, Gcs, S3, StorageBackend,
    StorageBackend_oneof_backend as Backend, StreamBackupTaskInfo,
};
use std::io::Cursor;

/// 静态字符串的测试用 Stringer。
struct TestStringer(&'static str);

impl FmtStringer for TestStringer {
    fn String(&self) -> String {
        self.0.to_owned()
    }
}

/// 验证 String/Stringer 模式输出及 MARKER 对 ‹› 的转义。
#[test]
fn string_and_stringer_match_go_modes_and_marker_escaping() {
    let cases = [
        ("OFF", "fxcv", "fxcv"),
        ("OFF", "f‹xcv", "f‹xcv"),
        ("ON", "f‹xcv", ""),
        ("MARKER", "f‹xcv", "‹f‹‹xcv›"),
        ("MARKER", "f›xcv", "‹f››xcv›"),
        ("MARKER", "中文", "‹中文›"),
    ];

    for (mode, input, expected) in cases {
        assert_eq!(RedactString(mode, input), expected);
        assert_eq!(Stringer(mode, &TestStringer(input)).String(), expected);
    }
}

/// 验证 DeRedact 的 remove/保留语义、未闭合输入与自定义行分隔符。
#[test]
fn deredact_matches_go_markers_escapes_unclosed_input_and_line_separator() {
    let cases = [
        (true, "‹fxcv›ggg", "?ggg"),
        (false, "‹fxcv›ggg", "fxcvggg"),
        (true, "fxcv", "fxcv"),
        (false, "fxcv", "fxcv"),
        (true, "‹fxcv›ggg‹fxcv›eee", "?ggg?eee"),
        (false, "‹fxcv›ggg‹fxcv›eee", "fxcvgggfxcveee"),
        (true, "‹›", "?"),
        (false, "‹›", ""),
        (true, "gg‹ee", "gg‹ee"),
        (false, "gg‹ee", "gg‹ee"),
        (true, "gg›ee", "gg›ee"),
        (false, "gg›ee", "gg›ee"),
        (true, "gg‹ee‹ee", "gg‹ee‹ee"),
        (false, "gg‹ee‹gg", "gg‹ee‹gg"),
        (true, "gg›ee›gg", "gg›ee›gg"),
        (false, "gg›ee›ee", "gg›ee›ee"),
        (false, "‹f‹‹x››cv›", "f‹x›cv"),
    ];

    for (remove, input, expected) in cases {
        let mut output = Vec::new();
        DeRedact(remove, Cursor::new(input), &mut output, "").unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), expected);
    }

    let mut output = Vec::new();
    DeRedact(false, Cursor::new("a\n‹b›\n"), &mut output, "|").unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "a|b|");
}

/// 验证全局开关下 NeedRedact/Value/Key/WriteRedact 与 Go 一致。
#[test]
fn global_switch_value_key_and_builder_match_go_behavior() {
    let _guard = REDACT_TEST_LOCK.lock().unwrap();
    InitRedact(false);
    assert!(!NeedRedact());
    assert_eq!(Value("secret"), "secret");
    assert_eq!(Key(b"secret"), "736563726574");
    assert_eq!(Key(&[0xab, 0xcd, 0xef]), "ABCDEF");

    let mut builder = String::from("prefix:");
    WriteRedact(&mut builder, "secret", "OFF");
    WriteRedact(&mut builder, "hidden", "ON");
    WriteRedact(&mut builder, "marked", "MARKER");
    assert_eq!(builder, "prefix:secret?‹marked›");

    InitRedact(true);
    assert!(NeedRedact());
    assert_eq!(Value("secret"), "?");
    assert_eq!(Key(b"secret"), "?");

    InitRedact(false);
}

/// 验证 TaskInfoRedacted 遮盖 S3/GCS/Azure 敏感字段且不修改源对象。
#[test]
fn task_info_redacts_each_go_backend_without_mutating_the_source() {
    assert_eq!(TaskInfoRedacted { Info: None }.String(), "nil");

    let mut s3 = S3::default();
    s3.bucket = "diagnostic-bucket".to_owned();
    s3.access_key = "s3-access".to_owned();
    s3.secret_access_key = "s3-secret".to_owned();
    s3.sse_kms_key_id = "s3-kms".to_owned();
    let mut storage = StorageBackend::default();
    storage.backend = Some(Backend::S3(s3));
    let mut info = StreamBackupTaskInfo::default();
    info.name = "backup-task".to_owned();
    info.set_storage(storage);

    let rendered = TaskInfoRedacted { Info: Some(&info) }.String();
    assert!(rendered.contains("diagnostic-bucket"));
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("s3-access"));
    assert!(!rendered.contains("s3-secret"));
    assert!(!rendered.contains("s3-kms"));
    let Some(Backend::S3(original_s3)) = info.storage.as_ref().and_then(|s| s.backend.as_ref())
    else {
        panic!("S3 backend must remain present");
    };
    assert_eq!(original_s3.access_key, "s3-access");

    let mut gcs = Gcs::default();
    gcs.bucket = "gcs-bucket".to_owned();
    gcs.credentials_blob = "gcs-credentials".to_owned();
    let mut storage = StorageBackend::default();
    storage.backend = Some(Backend::Gcs(gcs));
    info.set_storage(storage);
    let rendered = TaskInfoRedacted { Info: Some(&info) }.String();
    assert!(rendered.contains("gcs-bucket"));
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("gcs-credentials"));
    assert_eq!(
        match info.storage.as_ref().and_then(|s| s.backend.as_ref()) {
            Some(Backend::Gcs(original_gcs)) => original_gcs.credentials_blob.as_str(),
            _ => panic!("GCS backend must remain present"),
        },
        "gcs-credentials",
    );

    let mut azure = AzureBlobStorage::default();
    azure.bucket = "azure-bucket".to_owned();
    azure.shared_key = "azure-shared".to_owned();
    azure.access_sig = "azure-signature".to_owned();
    azure.set_encryption_key(AzureCustomerKey {
        encryption_key: "azure-encryption".to_owned(),
        ..Default::default()
    });
    let mut storage = StorageBackend::default();
    storage.backend = Some(Backend::AzureBlobStorage(azure));
    info.set_storage(storage);
    let rendered = TaskInfoRedacted { Info: Some(&info) }.String();
    assert!(rendered.contains("azure-bucket"));
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("azure-shared"));
    assert!(!rendered.contains("azure-signature"));
    assert!(!rendered.contains("azure-encryption"));
    assert_eq!(
        match info.storage.as_ref().and_then(|s| s.backend.as_ref()) {
            Some(Backend::AzureBlobStorage(original_azure)) => original_azure.shared_key.as_str(),
            _ => panic!("Azure backend must remain present"),
        },
        "azure-shared",
    );
}
