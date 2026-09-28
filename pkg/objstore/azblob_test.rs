// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `AzureBlobStorage` 外部行为测试。
//
// 使用内存/`LocalFileSystem` 注入与本地截断 HTTP 服务，验证读写、Seek、
// 认证选择、下载重试与跨前缀 copy。

use std::io::{Read, Seek, SeekFrom};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use objstore::azblob::*;
use objstore::objectio::Context;
use objstore::storeapi::{CopySpec, Storage, WalkOption, WriterOption};
use rand::RngCore;
use serial_test::serial;
use sha2::{Digest, Sha256};

/// 构造注入给定 ObjectStore 的测试用 AzureBlobStorage。
fn in_memory_azure(
    bucket: &str,
    prefix: &str,
    store: Arc<dyn object_store::ObjectStore>,
) -> AzureBlobStorage {
    AzureBlobStorage::with_store(
        AzureBlobStorageConfig {
            bucket: bucket.into(),
            prefix: prefix.into(),
            ..Default::default()
        },
        store,
        "devstoreaccount1",
        "http://127.0.0.1:10000/devstoreaccount1",
    )
    .unwrap()
}

#[test]
/// 基本写读、存在性、删除、WalkDir、Seek/Read 与 URI。
fn test_azblob() {
    let ctx = Context::default();
    let storage = in_memory_azure("test", "a/b/", Arc::new(InMemory::new()));
    storage.WriteFile(&ctx, "key", b"data").unwrap();
    storage.WriteFile(&ctx, "key1", b"data1").unwrap();
    storage
        .WriteFile(&ctx, "key2", b"data22223346757222222222289722222")
        .unwrap();
    assert_eq!(storage.ReadFile(&ctx, "key").unwrap(), b"data");
    assert!(storage.FileExists(&ctx, "key").unwrap());
    assert!(!storage.FileExists(&ctx, "key_not_exist").unwrap());

    storage.WriteFile(&ctx, "key_delete", b"data").unwrap();
    storage.DeleteFile(&ctx, "key_delete").unwrap();
    assert!(!storage.FileExists(&ctx, "key_delete").unwrap());

    let mut listed = Vec::new();
    storage
        .WalkDir(&ctx, None, &mut |name, size| {
            listed.push((name.to_owned(), size));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        listed,
        vec![("key".into(), 4), ("key1".into(), 5), ("key2".into(), 33)]
    );

    let mut reader = storage.Open(&ctx, "key2", None).unwrap();
    assert_eq!(reader.file_size().unwrap(), 33);
    let mut bytes = [0; 10];
    assert_eq!(reader.read(&mut bytes).unwrap(), 10);
    assert_eq!(&bytes, b"data222233");
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"46757222222222289722222");
    assert_eq!(reader.seek(SeekFrom::Start(3)).unwrap(), 3);
    assert_eq!(reader.read(&mut bytes[..5]).unwrap(), 5);
    assert_eq!(&bytes[..5], b"a2222");
    assert_eq!(reader.seek(SeekFrom::Current(3)).unwrap(), 11);
    assert_eq!(reader.read(&mut bytes[..5]).unwrap(), 5);
    assert_eq!(&bytes[..5], b"67572");
    assert_eq!(reader.seek(SeekFrom::End(-7)).unwrap(), 26);
    assert_eq!(reader.read(&mut bytes[..5]).unwrap(), 5);
    assert_eq!(&bytes[..5], b"97222");
    reader.close().unwrap();
    assert_eq!(storage.URI(), "azure://test/a/b/");
}

#[test]
/// Azure 的 Go 实现不会在客户端应用 `StartAfter`，Rust 保持相同回调集合。
fn test_azblob_walk_dir_does_not_apply_start_after() {
    let ctx = Context::default();
    let storage = in_memory_azure("test", "a/b/", Arc::new(InMemory::new()));
    storage.WriteFile(&ctx, "key", b"data").unwrap();
    storage.WriteFile(&ctx, "key1", b"data1").unwrap();

    let mut listed = Vec::new();
    storage
        .WalkDir(
            &ctx,
            Some(&WalkOption {
                StartAfter: "key".into(),
                ..Default::default()
            }),
            &mut |name, _| {
                listed.push(name.to_owned());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(listed, vec!["key", "key1"]);
}

/// 测试结束时清理指定环境变量，避免串扰。
struct EnvGuard(Vec<&'static str>);

impl EnvGuard {
    /// 先移除变量并记录名单，Drop 时再次清除。
    fn clean(names: Vec<&'static str>) -> Self {
        for name in &names {
            unsafe { std::env::remove_var(name) };
        }
        Self(names)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for name in &self.0 {
            unsafe { std::env::remove_var(name) };
        }
    }
}

#[test]
#[serial]
/// 覆盖显式 SharedKey、环境账户、Client Secret 与优先级回退。
fn test_new_azblob_storage() {
    let _env = EnvGuard::clean(vec![
        "AZURE_STORAGE_ACCOUNT",
        "AZURE_STORAGE_KEY",
        "AZURE_CLIENT_ID",
        "AZURE_TENANT_ID",
        "AZURE_CLIENT_SECRET",
    ]);
    let mut explicit = AzureBlobStorageConfig {
        endpoint: "http://127.0.0.1:1000".into(),
        bucket: "test".into(),
        account_name: "user".into(),
        shared_key: "cGFzc3dk".into(),
        ..Default::default()
    };
    let selected = select_azure_client(&mut explicit, &StorageOptions::default()).unwrap();
    assert_eq!(selected.auth, AzureAuth::SharedKey);
    assert_eq!(selected.account_name, "user");
    assert_eq!(selected.service_url, "http://127.0.0.1:1000");
    explicit.endpoint.clear();
    assert_eq!(
        select_azure_client(&mut explicit, &StorageOptions::default())
            .unwrap()
            .service_url,
        "https://user.blob.core.windows.net"
    );

    unsafe { std::env::set_var("AZURE_STORAGE_ACCOUNT", "env_user") };
    assert_eq!(
        select_azure_client(&mut explicit, &StorageOptions::default())
            .unwrap()
            .account_name,
        "user"
    );
    let mut from_env = AzureBlobStorageConfig {
        bucket: "test".into(),
        shared_key: "explicit-key".into(),
        ..Default::default()
    };
    assert_eq!(
        select_azure_client(&mut from_env, &StorageOptions::default())
            .unwrap()
            .auth,
        AzureAuth::Default
    );

    unsafe { std::env::set_var("AZURE_STORAGE_KEY", "cGFzc3dk") };
    let selected = select_azure_client(&mut from_env, &StorageOptions::default()).unwrap();
    assert_eq!(selected.auth, AzureAuth::SharedKey);
    assert_eq!(selected.account_name, "env_user");

    for name in ["AZURE_CLIENT_ID", "AZURE_TENANT_ID", "AZURE_CLIENT_SECRET"] {
        unsafe { std::env::set_var(name, "321") };
    }
    let mut token = AzureBlobStorageConfig {
        endpoint: "http://127.0.0.1:1000".into(),
        bucket: "test".into(),
        ..Default::default()
    };
    let selected = select_azure_client(&mut token, &StorageOptions::default()).unwrap();
    assert_eq!(selected.auth, AzureAuth::ClientSecret);
    assert_eq!(selected.account_name, "env_user");
    assert_eq!(selected.service_url, "http://127.0.0.1:1000");
    token.shared_key = "explicit-key-without-explicit-account".into();
    assert_eq!(
        select_azure_client(&mut token, &StorageOptions::default())
            .unwrap()
            .auth,
        AzureAuth::ClientSecret
    );
    token.account_name = "user".into();
    assert_eq!(
        select_azure_client(&mut token, &StorageOptions::default())
            .unwrap()
            .auth,
        AzureAuth::SharedKey
    );
    token.shared_key.clear();
    assert_eq!(
        select_azure_client(&mut token, &StorageOptions::default())
            .unwrap()
            .auth,
        AzureAuth::ClientSecret
    );
}

/// 故意返回截断正文的本地 HTTP 服务，用于触发下载重试。
struct TruncatedServer {
    endpoint: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TruncatedServer {
    /// 启动监听：完整 GET 返回残缺 body，Range 请求返回 206 头。
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { break };
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                thread_requests.fetch_add(1, Ordering::AcqRel);
                let mut request = [0; 8192];
                let count = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
                // Range 请求只回 206 头无 body；普通 GET 声明 7 字节却只给 "12345"。
                let response: &[u8] = if request.contains("range:") {
                    b"HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nContent-Range: bytes 5-6/7\r\nETag: \"0x1\"\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nx-ms-request-id: test\r\nConnection: close\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nETag: \"0x1\"\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\nx-ms-request-id: test\r\nConnection: close\r\n\r\n12345"
                };
                let _ = std::io::Write::write_all(&mut stream, response);
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for TruncatedServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
/// 真实客户端读截断响应时应重试超过 `AZBLOB_RETRY_TIMES`。
fn test_download_retry() {
    let server = TruncatedServer::start();
    let storage = new_azure_blob_storage(AzureBlobStorageConfig {
        endpoint: server.endpoint.clone(), bucket: "test".into(), prefix: "a/b/".into(),
        account_name: "devstoreaccount1".into(), shared_key: "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into(),
        ..Default::default()
    }, &StorageOptions::default()).unwrap();
    let error = storage.ReadFile(&Context::default(), "c").unwrap_err();
    let requests = server.requests.load(Ordering::Acquire);
    assert!(
        requests > AZBLOB_RETRY_TIMES as usize,
        "requests={requests}, error={error:#}"
    );
}

#[test]
/// Seek 到文件末尾应成功且后续 read 返回 0。
fn test_azblob_seek_to_end_should_not_error() {
    let ctx = Context::default();
    let storage = in_memory_azure("test", "a/b/", Arc::new(InMemory::new()));
    storage.WriteFile(&ctx, "c", &[0; 16]).unwrap();
    let mut reader = storage.Open(&ctx, "c", None).unwrap();
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 16);
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    reader.close().unwrap();
}

#[test]
/// 大对象跨 bucket/prefix copy，并用 SHA-256 校验内容一致。
fn test_copy_object() {
    const SIZE: usize = 300 * 1024 * 1024;
    let temp = tempfile::tempdir().unwrap();
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(LocalFileSystem::new_with_prefix(temp.path()).unwrap());
    let source = in_memory_azure("alice", "somewhat/", store.clone());
    let target = in_memory_azure("bob", "complex/prefix/", store);
    let ctx = Context::default();
    let mut writer = source
        .Create(&ctx, "test.bin", Some(&WriterOption::default()))
        .unwrap();
    let mut chunk = vec![0_u8; 1024 * 1024];
    let mut rng = rand::thread_rng();
    // 写入约 300 MiB 随机数据后再 copy_from。
    for _ in 0..300 {
        rng.fill_bytes(&mut chunk);
        writer.write(&ctx, &chunk).unwrap();
    }
    writer.close(&ctx).unwrap();
    drop(writer);
    target
        .copy_from(
            &source,
            &CopySpec {
                From: "test.bin".into(),
                To: "somewhere/test.bin".into(),
            },
        )
        .unwrap();
    let source_bytes = source.ReadFile(&ctx, "test.bin").unwrap();
    assert_eq!(source_bytes.len(), SIZE);
    let source_hash = Sha256::digest(&source_bytes);
    drop(source_bytes);
    let target_bytes = target.ReadFile(&ctx, "somewhere/test.bin").unwrap();
    assert_eq!(target_bytes.len(), SIZE);
    assert_eq!(Sha256::digest(&target_bytes), source_hash);
}
