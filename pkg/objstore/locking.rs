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

// 基于对象存储的远程分布式锁。
//
// 通过条件写入 INTENT + 锁文件实现互斥锁与读写锁（`.WRIT` / `.READ.*`），
// 元数据以 JSON 落在对象内容中；弱一致存储并发使用不安全。对应 Go `locking.go`。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{Context, StorageRef, WalkOption};

/// 错误信息中最多展示的冲突 blocker 数。
const LOCK_BLOCKER_ERROR_LIMIT: usize = 3;
/// 收集冲突对象元数据时的上限。
const LOCK_BLOCKER_META_LIMIT: usize = 3;
/// 日志字段中最多输出的远程 blocker 数。
const LOCK_BLOCKER_LOG_LIMIT: usize = 3;
/// `LockWithRetry` 最大重试次数。
const LOCK_RETRY_TIMES: usize = 60;

/// 按事务 ID 生成锁文件内容的回调。
type ContentFn = Arc<dyn Fn(Uuid) -> Vec<u8> + Send + Sync>;
/// 条件写前的冲突校验回调。
type VerifyFn = Arc<dyn Fn(&VerifyWriteContext) -> Result<()> + Send + Sync>;

/// 条件写入：先写 INTENT 再写目标，并校验无其它冲突对象。
struct ConditionalPut {
    target: String,
    content: ContentFn,
    verify: Option<VerifyFn>,
    local: LockMetaInput,
}

/// 条件写校验上下文：持有目标路径、存储与事务 ID。
struct VerifyWriteContext {
    context: Context,
    target: String,
    storage: StorageRef,
    txn_id: Uuid,
}

impl VerifyWriteContext {
    /// INTENT 临时对象名：`{target}.INTENT.{txn}`。
    fn intent_file_name(&self) -> String {
        format!("{}.INTENT.{}", self.target, self.txn_id.simple())
    }

    /// 枚举同前缀冲突对象（可含 tombstone），排除 expected 自身。
    fn conflicting_objects_of_prefix_expect(
        &self,
        prefix: &str,
        expected: &str,
    ) -> Result<(Vec<LockBlocker>, usize)> {
        let (directory, file_name) = split_object_path(prefix);
        let mut blockers = Vec::new();
        let mut blocker_count = 0_usize;
        self.storage.WalkDir(
            &self.context,
            Some(&WalkOption {
                sub_dir: directory,
                obj_prefix: file_name,
                include_tombstone: true,
                ..WalkOption::default()
            }),
            &mut |object_path, _size| {
                if object_path != expected {
                    blocker_count += 1;
                    if blockers.len() < LOCK_BLOCKER_META_LIMIT {
                        blockers.push(lock_blocker_from_path(
                            &self.context,
                            self.storage.clone(),
                            object_path,
                        ));
                    }
                }
                Ok(())
            },
        )?;
        Ok((blockers, blocker_count))
    }

    /// 若存在其它冲突对象则返回 `ErrLocked`。
    fn assert_no_other_of_prefix_expect(&self, prefix: &str, expected: &str) -> Result<()> {
        let (blockers, blocker_count) =
            self.conflicting_objects_of_prefix_expect(prefix, expected)?;
        if blocker_count == 0 {
            Ok(())
        } else {
            Err(anyhow!(ErrLocked {
                path: self.target.clone(),
                blocker_count,
                blockers,
                ..ErrLocked::default()
            }))
        }
    }

    /// 断言目标前缀下仅有本事务的 INTENT。
    fn assert_only_my_intent(&self) -> Result<()> {
        self.assert_no_other_of_prefix_expect(&self.target, &self.intent_file_name())
    }
}

impl ConditionalPut {
    /// 执行条件提交：预检 → 写 INTENT → 再检 → 写目标 → 删 INTENT。
    fn commit_to(&self, ctx: &Context, storage: StorageRef) -> Result<Uuid> {
        // 弱一致存储并发不安全，仅警告。
        if !storage.is_strong_consistent() {
            eprintln!(
                "object storage does not guarantee strong consistency; avoid concurrent access"
            );
        }
        let txn_id = Uuid::new_v4();
        let verify_context = VerifyWriteContext {
            context: ctx.clone(),
            target: self.target.clone(),
            storage: storage.clone(),
            txn_id,
        };
        let check_conflict = || -> Result<()> {
            if let Some(verify) = &self.verify {
                verify(&verify_context)?;
            }
            verify_context
                .assert_only_my_intent()
                .map_err(|error| with_lock_context(error, &self.target, &self.local))
        };

        // 先冲突检测，再落 INTENT，再二次检测后写真正锁文件。
        check_conflict().context("during initial check")?;
        let intent = verify_context.intent_file_name();
        storage
            .WriteFile(ctx, &intent, &[])
            .context("during writing intention file")?;

        let commit_result = (|| -> Result<()> {
            check_conflict().context("during checking whether there are other intentions")?;
            storage.WriteFile(ctx, &self.target, &(self.content)(txn_id))
        })();
        // INTENT 清理失败不掩盖提交结果，仅打印提示。
        if let Err(error) = storage.DeleteFile(ctx, &intent) {
            eprintln!("cannot delete intention file {intent}; delete it manually: {error:#}");
        }
        commit_result?;
        Ok(txn_id)
    }
}

/// 拆成 (目录, 文件名)；无 `/` 时目录为空。
fn split_object_path(path: &str) -> (String, String) {
    path.rsplit_once('/')
        .map(|(directory, file)| (directory.to_owned(), file.to_owned()))
        .unwrap_or_else(|| (String::new(), path.to_owned()))
}

/// 读取路径上的锁元数据，失败则记录 error 字符串。
fn lock_blocker_from_path(ctx: &Context, storage: StorageRef, path: &str) -> LockBlocker {
    match get_lock_meta(ctx, storage, path) {
        Ok(meta) => LockBlocker {
            path: path.to_owned(),
            meta,
            error: None,
        },
        Err(error) => LockBlocker {
            path: path.to_owned(),
            error: Some(error.to_string()),
            ..LockBlocker::default()
        },
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 加锁请求侧本地信息：owner、类型与提示。
pub struct LockMetaInput {
    pub owner_id: String,
    pub lock_type: String,
    pub hint: String,
}

impl fmt::Display for LockMetaInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fields = vec![format!("hint: {}", self.hint)];
        if !self.owner_id.is_empty() {
            fields.push(format!("owner_id: {}", self.owner_id));
        }
        if !self.lock_type.is_empty() {
            fields.push(format!("lock_type: {}", self.lock_type));
        }
        write!(formatter, "LockMetaInput({})", fields.join(", "))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
/// 锁文件持久化元数据（JSON）：时间、主机、PID、事务 ID 等。
pub struct LockMeta {
    #[serde(rename = "locked_at")]
    pub locked_at: DateTime<Utc>,
    #[serde(rename = "locker_host")]
    pub locker_host: String,
    #[serde(rename = "locker_pid")]
    pub locker_pid: u32,
    #[serde(rename = "txn_id", with = "base64_bytes")]
    pub txn_id: Vec<u8>,
    #[serde(rename = "owner_id", default, skip_serializing_if = "String::is_empty")]
    pub owner_id: String,
    #[serde(
        rename = "lock_type",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub lock_type: String,
    pub hint: String,
}

impl fmt::Display for LockMeta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fields = vec![
            format!("at: {}", self.locked_at.format("%Y-%m-%d %H:%M:%S")),
            format!("host: {}", self.locker_host),
            format!("pid: {}", self.locker_pid),
            format!("hint: {}", self.hint),
        ];
        if !self.owner_id.is_empty() {
            fields.push(format!("owner_id: {}", self.owner_id));
        }
        if !self.lock_type.is_empty() {
            fields.push(format!("lock_type: {}", self.lock_type));
        }
        write!(formatter, "Locked({})", fields.join(", "))
    }
}

/// `txn_id` 字段的 base64 编解码。
mod base64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 阻碍加锁的远程对象描述。
pub struct LockBlocker {
    pub path: String,
    pub meta: LockMeta,
    pub error: Option<String>,
}

impl fmt::Display for LockBlocker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error) = &self.error {
            write!(formatter, "Blocker(path: {}, err: {})", self.path, error)
        } else {
            write!(
                formatter,
                "Blocker(path: {}, meta: {})",
                self.path, self.meta
            )
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 加锁失败错误：路径、本地/远程元数据与冲突列表。
pub struct ErrLocked {
    pub path: String,
    pub meta: LockMeta,
    pub local: LockMetaInput,
    pub blocker_count: usize,
    pub blockers: Vec<LockBlocker>,
}

impl ErrLocked {
    /// 远程冲突总数（以 blocker_count 与 blockers 长度取较大）。
    fn remote_blocker_count(&self) -> usize {
        self.blocker_count.max(self.blockers.len())
    }
}

impl fmt::Display for ErrLocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fields = vec!["locked".to_owned()];
        if !self.path.is_empty() {
            fields.push(format!("path = {}", self.path));
        }
        if self.meta != LockMeta::default() {
            fields.push(format!("meta = {}", self.meta));
        }
        if self.local != LockMetaInput::default() {
            fields.push(format!("local = {}", self.local));
        }
        for blocker in self.blockers.iter().take(LOCK_BLOCKER_ERROR_LIMIT) {
            fields.push(format!("conflict file {}", blocker.path));
            if let Some(error) = &blocker.error {
                fields.push(format!("blocker_error = {error}"));
            } else {
                fields.push(format!("blocker_meta = {}", blocker.meta));
            }
        }
        let omitted = self
            .remote_blocker_count()
            .saturating_sub(self.blockers.len().min(LOCK_BLOCKER_ERROR_LIMIT));
        if omitted > 0 {
            fields.push(format!("omitted_conflict_files = {omitted}"));
        }
        formatter.write_str(&fields.join(", "))
    }
}

impl std::error::Error for ErrLocked {}

/// 为 `ErrLocked` 补全 path/local 上下文。
fn with_lock_context(error: anyhow::Error, path: &str, local: &LockMetaInput) -> anyhow::Error {
    if let Some(locked) = error.downcast_ref::<ErrLocked>() {
        let mut locked = locked.clone();
        if locked.path.is_empty() {
            locked.path = path.to_owned();
        }
        if locked.local == LockMetaInput::default() {
            locked.local = local.clone();
        }
        anyhow!(locked)
    } else {
        error
    }
}

/// 根据本机主机名与 PID 构造锁元数据。
pub fn MakeLockMeta(input: LockMetaInput) -> LockMeta {
    let hostname = hostname::get()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|error| format!("UnknownHost(err={error})"));
    LockMeta {
        locked_at: Utc::now(),
        locker_host: hostname,
        locker_pid: std::process::id(),
        owner_id: input.owner_id,
        lock_type: input.lock_type,
        hint: input.hint,
        ..LockMeta::default()
    }
}

/// 读取并解析锁文件 JSON。
fn get_lock_meta(ctx: &Context, storage: StorageRef, path: &str) -> Result<LockMeta> {
    let bytes = storage
        .ReadFile(ctx, path)
        .with_context(|| format!("failed to read existed lock file {path}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse lock file {path}"))
}

/// 已持有的远程锁句柄：解锁时校验 txn_id。
pub struct RemoteLock {
    txn_id: Uuid,
    storage: StorageRef,
    path: String,
}

impl fmt::Debug for RemoteLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteLock")
            .field("path", &self.path)
            .finish()
    }
}

impl fmt::Display for RemoteLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{{path={},uuid={},storage_uri={}}}",
            self.path,
            self.txn_id,
            self.storage.URI()
        )
    }
}

impl RemoteLock {
    /// 校验事务 ID 后删除锁文件。
    pub fn Unlock(&self, ctx: &Context) -> Result<()> {
        let meta = get_lock_meta(ctx, self.storage.clone(), &self.path)?;
        if self.txn_id.as_bytes() != meta.txn_id.as_slice() {
            return Err(anyhow!(
                "Txn ID mismatch: remote is {:?}, our is {}",
                meta.txn_id,
                self.txn_id
            ));
        }
        self.storage
            .DeleteFile(ctx, &self.path)
            .with_context(|| format!("failed to delete lock file {}", self.path))
    }

    /// 清理路径解锁：Context 已取消时换 background 再删，失败仅打日志。
    pub fn UnlockOnCleanUp(&self, ctx: &Context) {
        let cleanup = if ctx.is_cancelled() {
            Context::background()
        } else {
            ctx.clone()
        };
        if let Err(error) = self.Unlock(&cleanup) {
            eprintln!("failed to unlock {self}; manual deletion may be required: {error:#}");
        }
    }
}

/// 将带 txn_id 的 LockMeta 序列化为 JSON 字节。
fn serialize_lock_meta(input: LockMetaInput, txn_id: Uuid) -> Vec<u8> {
    let mut meta = MakeLockMeta(input);
    meta.txn_id = txn_id.as_bytes().to_vec();
    serde_json::to_vec(&meta).expect("plain lock metadata must serialize")
}

/// 丰富加锁错误：补全 ErrLocked 字段，或附上远程锁元数据。
fn annotate_lock_attempt_error(
    ctx: &Context,
    storage: StorageRef,
    path: &str,
    local: &LockMetaInput,
    error: anyhow::Error,
    message: String,
) -> anyhow::Error {
    if let Some(locked) = error.downcast_ref::<ErrLocked>() {
        let mut locked = locked.clone();
        if locked.path.is_empty() {
            locked.path = path.to_owned();
        }
        if locked.local == LockMetaInput::default() {
            locked.local = local.clone();
        }
        if locked.meta == LockMeta::default() {
            if let Some(blocker) = locked.blockers.first() {
                locked.meta = blocker.meta.clone();
            } else {
                let blocker = lock_blocker_from_path(ctx, storage, path);
                locked.meta = blocker.meta.clone();
                if blocker.error.is_some() {
                    locked.blockers.push(blocker);
                }
            }
        }
        return anyhow!(locked).context(format!("{message}: {error:#}"));
    }
    if let Ok(meta) = get_lock_meta(ctx, storage, path) {
        error.context(format!("{message}; remote_lock_meta = {meta}"))
    } else {
        error.context(message)
    }
}

/// 尝试获取互斥远程锁（目标路径即锁文件）。
pub fn TryLockRemote(
    ctx: &Context,
    storage: StorageRef,
    path: &str,
    input: LockMetaInput,
) -> Result<RemoteLock> {
    let content_input = input.clone();
    let writer = ConditionalPut {
        target: path.to_owned(),
        local: input.clone(),
        verify: None,
        content: Arc::new(move |txn_id| serialize_lock_meta(content_input.clone(), txn_id)),
    };
    let txn_id = writer.commit_to(ctx, storage.clone()).map_err(|error| {
        annotate_lock_attempt_error(
            ctx,
            storage.clone(),
            path,
            &input,
            error,
            format!("failed to acquire lock on '{path}'"),
        )
    })?;
    Ok(RemoteLock {
        txn_id,
        storage,
        path: path.to_owned(),
    })
}

/// 写锁对象名：`{path}.WRIT`。
fn write_lock_name(path: &str) -> String {
    format!("{path}.WRIT")
}

/// 读锁对象名：`{path}.READ.{随机}`，允许多读共存。
fn new_read_lock_name(path: &str) -> String {
    format!(
        "{path}.READ.{:016x}",
        rand::thread_rng().r#gen::<u64>() & i64::MAX as u64
    )
}

/// 尝试获取写锁：要求同前缀无其它 INTENT/读锁冲突。
pub fn TryLockRemoteWrite(
    ctx: &Context,
    storage: StorageRef,
    path: &str,
    input: LockMetaInput,
) -> Result<RemoteLock> {
    let target = write_lock_name(path);
    let verify_path = path.to_owned();
    let verify_target = target.clone();
    let verify_input = input.clone();
    let content_input = input.clone();
    let writer = ConditionalPut {
        target: target.clone(),
        local: input.clone(),
        content: Arc::new(move |txn_id| serialize_lock_meta(content_input.clone(), txn_id)),
        // 写锁校验：目标前缀下除本 INTENT 外不得有其它对象（含读锁）。
        verify: Some(Arc::new(move |context| {
            let (blockers, blocker_count) = context
                .conflicting_objects_of_prefix_expect(&verify_path, &context.intent_file_name())?;
            if blocker_count == 0 {
                Ok(())
            } else {
                Err(anyhow!(ErrLocked {
                    path: verify_target.clone(),
                    local: verify_input.clone(),
                    blocker_count,
                    blockers,
                    ..ErrLocked::default()
                }))
            }
        })),
    };
    let txn_id = writer.commit_to(ctx, storage.clone()).map_err(|error| {
        annotate_lock_attempt_error(
            ctx,
            storage.clone(),
            &target,
            &input,
            error,
            "something wrong about the lock".to_owned(),
        )
    })?;
    Ok(RemoteLock {
        txn_id,
        storage,
        path: target,
    })
}

/// 尝试获取读锁：要求不存在写锁（`.WRIT`）。
pub fn TryLockRemoteRead(
    ctx: &Context,
    storage: StorageRef,
    path: &str,
    input: LockMetaInput,
) -> Result<RemoteLock> {
    let target = new_read_lock_name(path);
    let write_lock = write_lock_name(path);
    let verify_target = target.clone();
    let verify_input = input.clone();
    let content_input = input.clone();
    let writer = ConditionalPut {
        target: target.clone(),
        local: input.clone(),
        content: Arc::new(move |txn_id| serialize_lock_meta(content_input.clone(), txn_id)),
        verify: Some(Arc::new(move |context| {
            let (blockers, blocker_count) =
                context.conflicting_objects_of_prefix_expect(&write_lock, "")?;
            if blocker_count == 0 {
                Ok(())
            } else {
                Err(anyhow!(ErrLocked {
                    path: verify_target.clone(),
                    local: verify_input.clone(),
                    blocker_count,
                    blockers,
                    ..ErrLocked::default()
                }))
            }
        })),
    };
    let txn_id = writer.commit_to(ctx, storage.clone()).map_err(|error| {
        annotate_lock_attempt_error(
            ctx,
            storage.clone(),
            &write_lock_name(path),
            &input,
            error,
            "failed to commit the lock due to existing lock: something wrong about the lock"
                .to_owned(),
        )
    })?;
    Ok(RemoteLock {
        txn_id,
        storage,
        path: target,
    })
}

/// 带指数退避与抖动的加锁重试封装。
pub fn LockWithRetry<F>(
    ctx: &Context,
    locker: F,
    storage: StorageRef,
    path: &str,
    input: LockMetaInput,
) -> Result<RemoteLock>
where
    F: Fn(&Context, StorageRef, &str, LockMetaInput) -> Result<RemoteLock>,
{
    // 指数退避上限 60s，并叠加 2.5–7.5s 抖动。
    let jitter = Duration::from_millis(rand::thread_rng().gen_range(2500..7500));
    let mut last_error = None;
    for attempt in 0..=LOCK_RETRY_TIMES {
        match locker(ctx, storage.clone(), path, input.clone()) {
            Ok(lock) => return Ok(lock),
            Err(error) => last_error = Some(error),
        }
        if attempt == LOCK_RETRY_TIMES {
            break;
        }
        let seconds = 1_u64
            .checked_shl(attempt.min(6) as u32)
            .unwrap_or(60)
            .min(60);
        if let Err(wait_error) = ctx.wait_timeout(Duration::from_secs(seconds) + jitter) {
            if let Some(error) = last_error.take() {
                return Err(error).context(wait_error.to_string());
            }
            return Err(wait_error);
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("lock failed"))).context(format!(
        "failed to acquire lock after {LOCK_RETRY_TIMES} retries"
    ))
}

/// 从加锁错误提取结构化日志字段（本地与远程 blocker）。
pub fn LockConflictLogFields(
    path: &str,
    input: &LockMetaInput,
    error: &anyhow::Error,
) -> Vec<(String, String)> {
    let mut fields = vec![
        ("error".to_owned(), error.to_string()),
        ("path".to_owned(), path.to_owned()),
        ("local_owner_id".to_owned(), input.owner_id.clone()),
        ("local_lock_type".to_owned(), input.lock_type.clone()),
        ("local_hint".to_owned(), input.hint.clone()),
    ];
    if let Some(locked) = error.downcast_ref::<ErrLocked>() {
        fields.push((
            "remote_blocker_count".to_owned(),
            locked.remote_blocker_count().to_string(),
        ));
        for (index, blocker) in locked
            .blockers
            .iter()
            .take(LOCK_BLOCKER_LOG_LIMIT)
            .enumerate()
        {
            fields.push((format!("remote_blocker_{index}_path"), blocker.path.clone()));
            fields.push((
                format!("remote_blocker_{index}_owner_id"),
                blocker.meta.owner_id.clone(),
            ));
            fields.push((
                format!("remote_blocker_{index}_lock_type"),
                blocker.meta.lock_type.clone(),
            ));
            fields.push((
                format!("remote_blocker_{index}_hint"),
                blocker.meta.hint.clone(),
            ));
        }
    }
    fields
}
