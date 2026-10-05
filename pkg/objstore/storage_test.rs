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

// `storage` 模块基础测试：默认 HTTP 传输/客户端参数，以及通过 URL 构造内存对象存储。

use std::any::Any;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::hdfs::HDFSStorage;
use crate::helper::UnmarshalDir;
use crate::memstore::{MemStorage, NewMemStorage};
use crate::parse::ParseBackend;
use crate::storage::{
    CloneDefaultHTTPTransport, Context, CopySpec, GetDefaultHTTPClient, NewFromURL,
    NewWithDefaultOpt, ObjectReader, ObjectWriter, ReaderOption, Storage, StorageRef, WalkOption,
    WriterOption,
};

enum UnmarshalDirWalk {
    Error,
    OneFile,
    OneFileThenError,
}

struct UnmarshalDirTestStorage {
    inner: MemStorage,
    walk: UnmarshalDirWalk,
    started: Option<mpsc::Sender<()>>,
    release: Option<Mutex<mpsc::Receiver<()>>>,
}

impl UnmarshalDirTestStorage {
    fn new(
        walk: UnmarshalDirWalk,
        started: Option<mpsc::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
    ) -> Self {
        let inner = NewMemStorage();
        inner
            .WriteFile(&Context::background(), "meta", b"data")
            .unwrap();
        Self {
            inner,
            walk,
            started,
            release: release.map(Mutex::new),
        }
    }
}

impl Storage for UnmarshalDirTestStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()> {
        self.inner.DeleteFile(ctx, name)
    }

    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        self.inner.WriteFile(ctx, name, data)
    }

    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>> {
        if matches!(self.walk, UnmarshalDirWalk::OneFileThenError) {
            self.started
                .as_ref()
                .expect("started channel must be configured")
                .send(())
                .expect("test must wait for metadata worker");
            self.release
                .as_ref()
                .expect("release channel must be configured")
                .lock()
                .expect("release channel lock poisoned")
                .recv()
                .expect("test must release metadata worker");
        }
        self.inner.ReadFile(ctx, name)
    }

    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool> {
        self.inner.FileExists(ctx, name)
    }

    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>> {
        self.inner.Open(ctx, name, option)
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        _option: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        match &self.walk {
            UnmarshalDirWalk::Error => Err(anyhow!("injected listing failure")),
            UnmarshalDirWalk::OneFile => callback("meta", 4),
            UnmarshalDirWalk::OneFileThenError => {
                callback("meta", 4)?;
                Err(anyhow!("injected listing failure"))
            }
        }
    }

    fn URI(&self) -> String {
        self.inner.URI()
    }

    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        self.inner.Create(ctx, name, option)
    }

    fn Rename(&self, ctx: &Context, old_name: &str, new_name: &str) -> Result<()> {
        self.inner.Rename(ctx, old_name, new_name)
    }

    fn PresignFile(&self, ctx: &Context, name: &str, duration: Duration) -> Result<String> {
        self.inner.PresignFile(ctx, name, duration)
    }

    fn Close(&self) {
        self.inner.Close();
    }

    fn CopyFrom(&self, ctx: &Context, source: StorageRef, spec: &CopySpec) -> Result<()> {
        self.inner.CopyFrom(ctx, source, spec)
    }

    fn is_strong_consistent(&self) -> bool {
        self.inner.is_strong_consistent()
    }
}

#[test]
fn unmarshal_dir_waits_for_workers_after_walk_error() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let storage: StorageRef = Arc::new(UnmarshalDirTestStorage::new(
        UnmarshalDirWalk::OneFileThenError,
        Some(started_tx),
        Some(release_rx),
    ));
    let mut items = UnmarshalDir(
        Context::background(),
        WalkOption::default(),
        storage,
        |_name, content| Ok(String::from_utf8(content.to_vec())?),
    );
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::spawn(move || result_tx.send((items.next(), items.next())).unwrap());

    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("metadata worker must start during directory listing");
    assert!(
        result_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "iterator terminated before its metadata worker completed"
    );
    release_tx.send(()).unwrap();

    let (item, terminal) = result_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(item.unwrap().unwrap(), "data");
    assert!(
        terminal
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("injected listing failure")
    );
}

#[test]
fn unmarshal_dir_returns_walk_error_after_workers_finish() {
    for _ in 0..100 {
        let storage: StorageRef = Arc::new(UnmarshalDirTestStorage::new(
            UnmarshalDirWalk::Error,
            None,
            None,
        ));
        let error = UnmarshalDir(
            Context::background(),
            WalkOption::default(),
            storage,
            |_name, _content| Ok(()),
        )
        .next()
        .expect("walk error must be returned")
        .unwrap_err();
        assert!(error.to_string().contains("injected listing failure"));
    }
}

#[test]
fn unmarshal_dir_returns_worker_error_after_channel_close() {
    for _ in 0..100 {
        let storage: StorageRef = Arc::new(UnmarshalDirTestStorage::new(
            UnmarshalDirWalk::OneFile,
            None,
            None,
        ));
        let error = UnmarshalDir(
            Context::background(),
            WalkOption::default(),
            storage,
            |_name, _content| Err::<(), _>(anyhow!("unsupported metadata version")),
        )
        .next()
        .expect("worker error must be returned")
        .unwrap_err();
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string() == "unsupported metadata version"),
            "worker error must remain in the error chain: {error:#}"
        );
    }
}

/// 校验默认 HTTP 传输配置：单主机连接上限为 0（不限制），空闲连接池非空。
#[test]
fn test_default_http_transport() {
    let (transport, ok) = CloneDefaultHTTPTransport();
    assert!(ok);
    assert_eq!(transport.max_connections_per_host, 0);
    assert!(transport.max_idle_connections > 0);
}

/// 按并发度构造默认客户端时，空闲连接与每主机空闲连接应等于该并发度。
#[test]
fn test_default_http_client() {
    let concurrency = 128;
    let transport = GetDefaultHTTPClient(concurrency).transport;
    assert_eq!(transport.max_idle_connections_per_host, concurrency);
    assert_eq!(transport.max_idle_connections, concurrency);
}

/// `memstore://` URL 应解析为进程内内存对象存储实现。
#[test]
fn test_new_mem_storage() {
    let storage = NewFromURL(&Context::background(), "memstore://").unwrap();
    assert!(storage.as_any().is::<MemStorage>());
}

#[test]
/// Go `New` 直接构造 HDFS 后端，不要求注入云存储 external factory。
fn test_new_hdfs_storage() {
    let backend = ParseBackend("hdfs://127.0.0.1:1231/backup", None).unwrap();
    let storage = NewWithDefaultOpt(&Context::background(), &backend).unwrap();

    assert!(storage.as_any().is::<HDFSStorage>());
    assert_eq!(storage.URI(), "hdfs://127.0.0.1:1231/backup");
}

/// Go only passes the context to cloud constructors; local constructors still succeed after
/// cancellation.
#[test]
fn test_new_hdfs_storage_ignores_cancelled_context() {
    let backend = ParseBackend("hdfs://127.0.0.1:1231/backup", None).unwrap();
    let ctx = Context::background();
    ctx.cancel();

    let storage = NewWithDefaultOpt(&ctx, &backend).unwrap();
    assert!(storage.as_any().is::<HDFSStorage>());
}
