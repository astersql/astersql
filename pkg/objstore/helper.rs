// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 对象存储辅助：云 URI 连通性校验、上传 worker 计数与目录元数据并行反序列化。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, unbounded};

use crate::parse::ParseBackend;
use crate::storage::{
    Context, HttpClient, HttpTransport, New, Options, Permission, StorageRef, WalkOption,
};

/// 云存储 URI 校验函数类型：在给定 Context 下探测 URI 是否可用。
type Validator = fn(&Context, &str) -> Result<()>;
static CLOUD_STORAGE_URI_VALIDATOR: OnceLock<Validator> = OnceLock::new();

/// Rust has no package init hook, so module integration calls this explicit registration.
/// Rust 无包级 init，由集成代码显式调用以注册默认校验器。
pub fn init() {
    let _ = CLOUD_STORAGE_URI_VALIDATOR.set(ValidateCloudStorageURI);
}

/// 返回已注册的云 URI 校验函数（若有）。
pub fn registered_cloud_storage_uri_validator() -> Option<Validator> {
    CLOUD_STORAGE_URI_VALIDATOR.get().copied()
}

/// 解析后端、以关闭 keep-alive 的 HTTP 客户端打开存储并检查 List/Get/AccessBuckets 权限。
pub fn ValidateCloudStorageURI(ctx: &Context, uri: &str) -> Result<()> {
    let backend = ParseBackend(uri, None)?;
    let http_client = HttpClient {
        transport: HttpTransport {
            disable_keep_alives: true,
            ..HttpTransport::default()
        },
    };
    let storage = New(
        ctx,
        &backend,
        Some(&Options {
            http_client: Some(http_client),
            check_permissions: vec![
                Permission::ListObjects,
                Permission::GetObject,
                Permission::AccessBuckets,
            ],
            ..Options::default()
        }),
    )?;
    storage.Close();
    Ok(())
}

/// The active upload worker count is observable for GCS, matching helper.go.
/// 活跃上传 worker 计数（供 GCS 观测），对齐 Go `helper.go`。
pub static activeUploadWorkerCnt: AtomicI64 = AtomicI64::new(0);

/// 读取当前活跃上传 worker 数。
pub fn GetActiveUploadWorkerCount() -> i64 {
    activeUploadWorkerCnt.load(Ordering::SeqCst)
}

/// 异步目录反序列化结果迭代器：从通道拉取 `Result<T>`。
pub struct UnmarshalDirIter<T> {
    receiver: Receiver<Result<T>>,
}

impl<T> Iterator for UnmarshalDirIter<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

/// UnmarshalDir starts metadata workers while walking and joins every submitted worker before
/// publishing the terminal listing or worker error. Results are deliberately unordered, as in
/// the Go channel-based implementation.
/// WalkDir 遍历期间即启动至多 128 个 worker；发布 listing/worker 终止错误前先等待已提交 worker。
pub fn UnmarshalDir<T, F>(
    ctx: Context,
    walk_option: WalkOption,
    storage: StorageRef,
    unmarshal: F,
) -> UnmarshalDirIter<T>
where
    T: Send + 'static,
    F: Fn(&str, &[u8]) -> Result<T> + Send + Sync + 'static,
{
    let (sender, receiver) = unbounded();
    let unmarshal = Arc::new(unmarshal);
    std::thread::spawn(move || {
        let failed = Arc::new(AtomicBool::new(false));
        let worker_error = Arc::new(Mutex::new(None));
        let (work_sender, work_receiver) = unbounded::<String>();
        let mut workers = Vec::new();
        let walk_result = storage.WalkDir(&ctx, Some(&walk_option), &mut |path, _size| {
            if failed.load(Ordering::Acquire) {
                return Ok(());
            }

            if workers.len() < 128 {
                let work_receiver = work_receiver.clone();
                let failed = failed.clone();
                let worker_error = worker_error.clone();
                let sender = sender.clone();
                let storage = storage.clone();
                let unmarshal = unmarshal.clone();
                let ctx = ctx.clone();
                workers.push(std::thread::spawn(move || {
                    while let Ok(path) = work_receiver.recv() {
                        if failed.load(Ordering::Acquire) {
                            break;
                        }
                        if let Err(error) = ctx.check_cancelled() {
                            if !failed.swap(true, Ordering::AcqRel) {
                                *worker_error.lock().expect("metadata error lock poisoned") =
                                    Some(error);
                            }
                            break;
                        }
                        let result = storage
                            .ReadFile(&ctx, &path)
                            .with_context(|| {
                                format!("during reading meta file {path} from storage")
                            })
                            .and_then(|bytes| {
                                unmarshal(&path, &bytes)
                                    .with_context(|| format!("failed to unmarshal file {path}"))
                            });
                        match result {
                            Ok(value) => {
                                let _ = sender.send(Ok(value));
                            }
                            Err(error) => {
                                if !failed.swap(true, Ordering::AcqRel) {
                                    *worker_error.lock().expect("metadata error lock poisoned") =
                                        Some(error);
                                }
                                break;
                            }
                        }
                    }
                }));
            }
            work_sender
                .send(path.to_owned())
                .map_err(|_| anyhow::anyhow!("metadata worker queue closed"))
        });
        drop(work_sender);
        for worker in workers {
            let _ = worker.join();
        }
        let terminal_error = walk_result.err().or_else(|| {
            worker_error
                .lock()
                .expect("metadata error lock poisoned")
                .take()
        });
        if let Some(error) = terminal_error {
            let _ = sender.send(Err(error));
        }
    });
    UnmarshalDirIter { receiver }
}
