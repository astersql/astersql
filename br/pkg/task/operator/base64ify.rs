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

//! Base64ify operator — mirrors `br/pkg/task/operator/base64ify.go`.
//! 将外部存储 URI（及可选凭证）解析为 backend protobuf，再标准 Base64 打印到 stdout，
//! 供后续 BR 命令以 `--storage` 编码串复用；与 Go `Base64ify`/`runEncode` 数据流一致。

use base64::Engine;

use crate::config::Base64ifyConfig;
use crate::stubs::{BackendOptions, Context, Error, Result, color_hi_red};

use astersql_objstore::parse::{
    AzureBlobStorage, BackendOptions as ObjstoreBackendOptions, Gcs, S3, StorageBackend,
};
use astersql_objstore::storage::{Context as StorageContext, New, Options as StorageOptions};

/// CLI 入口：委托 `runEncode`，保持与 Go 同名公开函数签名语义。
pub fn Base64ify(ctx: Context, cfg: Base64ifyConfig) -> Result<()> {
    runEncode(ctx, cfg)
}

/// 解析 backend → 试连存储（校验 URI/凭证）→ Marshal → Base64 输出。
/// `LoadCerd` 为真时会把环境凭证打进序列化结果，故 stderr 红色警告不可省略。
pub fn runEncode(ctx: Context, cfg: Base64ifyConfig) -> Result<()> {
    let storage_ctx = StorageContext::from_cancellation_flag(ctx.cancellation_flag());
    let backend_options = to_objstore_backend_options(&cfg.BackendOptions);
    // 与 Go `objstore.ParseBackend` 对齐：只解析，不立刻读对象。
    let s = parse_backend(&cfg.StorageURI, &backend_options)?;

    // New 用于验证后端可达；选项与 Go storeapi.Options 一致。
    let store = New(
        &storage_ctx,
        &s,
        Some(&StorageOptions {
            send_credentials: cfg.LoadCerd,
            check_s3_object_lock_options: true,
            ..StorageOptions::default()
        }),
    )
    .map_err(objstore_error)?;
    store.Close();

    if cfg.LoadCerd {
        eprintln!(
            "{}",
            color_hi_red(
                "Credientials are encoded to the base64 string. DON'T share this with untrusted people!"
            )
        );
    }

    // Marshal 后的字节即 Go `s.Marshal()` 的 protobuf wire 结果。
    let sBytes = marshal_backend(&s);
    println!(
        "{}",
        base64::engine::general_purpose::STANDARD.encode(sBytes)
    );
    Ok(())
}

fn objstore_error(error: impl std::fmt::Display) -> Error {
    Error::new(error.to_string())
}

fn parse_backend(uri: &str, options: &ObjstoreBackendOptions) -> Result<StorageBackend> {
    let backend =
        astersql_objstore::parse::ParseBackend(uri, Some(options)).map_err(objstore_error)?;
    // Go Base64ify 调用 ParseBackend，不走 NewFromURL 的 Rust-only memstore 捷径。
    if matches!(backend, StorageBackend::MemStore) {
        return Err(Error::new("storage memstore not support yet"));
    }
    Ok(backend)
}

fn to_objstore_backend_options(options: &BackendOptions) -> ObjstoreBackendOptions {
    let mut converted = ObjstoreBackendOptions::default();
    let value = |name: &str| options.S3.get(name).cloned().unwrap_or_default();
    converted.s3.endpoint = value("endpoint");
    converted.s3.region = value("region");
    converted.s3.storage_class = value("storage-class");
    converted.s3.sse = value("sse");
    converted.s3.sse_kms_key_id = value("sse-kms-key-id");
    converted.s3.acl = value("acl");
    converted.s3.access_key = value("access-key");
    converted.s3.secret_access_key = value("secret-access-key");
    converted.s3.session_token = value("session-token");
    converted.s3.provider = value("provider");
    converted.s3.role_arn = value("role-arn");
    converted.s3.external_id = value("external-id");
    converted.s3.profile = value("profile");
    converted.s3.force_path_style = options
        .S3
        .get("force-path-style")
        .and_then(|value| value.parse().ok())
        .unwrap_or(true);
    converted
}

fn push_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_bytes(field: u32, value: &[u8], output: &mut Vec<u8>) {
    push_varint(u64::from(field << 3 | 2), output);
    push_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn push_string(field: u32, value: &str, output: &mut Vec<u8>) {
    if !value.is_empty() {
        push_bytes(field, value.as_bytes(), output);
    }
}

fn push_bool(field: u32, value: bool, output: &mut Vec<u8>) {
    if value {
        push_varint(u64::from(field << 3), output);
        output.push(1);
    }
}

fn marshal_s3(value: &S3) -> Vec<u8> {
    let mut output = Vec::new();
    push_string(1, &value.endpoint, &mut output);
    push_string(2, &value.region, &mut output);
    push_string(3, &value.bucket, &mut output);
    push_string(4, &value.prefix, &mut output);
    push_string(5, &value.storage_class, &mut output);
    push_string(6, &value.sse, &mut output);
    push_string(7, &value.acl, &mut output);
    push_string(8, &value.access_key, &mut output);
    push_string(9, &value.secret_access_key, &mut output);
    push_bool(10, value.force_path_style, &mut output);
    push_string(11, &value.sse_kms_key_id, &mut output);
    push_string(12, &value.role_arn, &mut output);
    push_string(13, &value.external_id, &mut output);
    push_string(15, &value.session_token, &mut output);
    push_string(16, &value.provider, &mut output);
    push_string(17, &value.profile, &mut output);
    output
}

fn marshal_gcs(value: &Gcs) -> Vec<u8> {
    let mut output = Vec::new();
    push_string(1, &value.endpoint, &mut output);
    push_string(2, &value.bucket, &mut output);
    push_string(3, &value.prefix, &mut output);
    push_string(4, &value.storage_class, &mut output);
    push_string(5, &value.predefined_acl, &mut output);
    push_string(6, &value.credentials_blob, &mut output);
    output
}

fn marshal_azure(value: &AzureBlobStorage) -> Vec<u8> {
    let mut output = Vec::new();
    push_string(1, &value.endpoint, &mut output);
    push_string(2, &value.bucket, &mut output);
    push_string(3, &value.prefix, &mut output);
    push_string(4, &value.storage_class, &mut output);
    push_string(5, &value.account_name, &mut output);
    push_string(6, &value.shared_key, &mut output);
    push_string(8, &value.access_sig, &mut output);
    push_string(9, &value.encryption_scope, &mut output);
    if let Some(key) = &value.encryption_key {
        let mut encoded_key = Vec::new();
        push_string(1, &key.encryption_key, &mut encoded_key);
        push_string(2, &key.encryption_key_sha256, &mut encoded_key);
        push_bytes(10, &encoded_key, &mut output);
    }
    output
}

fn marshal_backend(backend: &StorageBackend) -> Vec<u8> {
    let (field, nested) = match backend {
        StorageBackend::Noop => (1, Vec::new()),
        StorageBackend::Local(local) => {
            let mut nested = Vec::new();
            push_string(1, &local.path, &mut nested);
            (2, nested)
        }
        StorageBackend::S3(s3) => (3, marshal_s3(s3)),
        StorageBackend::Gcs(gcs) => (4, marshal_gcs(gcs)),
        StorageBackend::Hdfs(hdfs) => {
            let mut nested = Vec::new();
            push_string(1, &hdfs.remote, &mut nested);
            (6, nested)
        }
        StorageBackend::AzureBlobStorage(azure) => (7, marshal_azure(azure)),
        StorageBackend::MemStore => unreachable!("memstore is rejected by parse_backend"),
    };
    let mut output = Vec::new();
    push_bytes(field, &nested, &mut output);
    output
}

pub(crate) fn encode_backend_for_test(uri: &str) -> Result<String> {
    let backend = parse_backend(uri, &ObjstoreBackendOptions::default())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(marshal_backend(&backend)))
}
