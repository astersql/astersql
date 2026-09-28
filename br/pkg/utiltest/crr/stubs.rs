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

//! Local stand-ins for objstore / storeapi / objectio / context boundaries
//! 本地替身：模拟对象存储与 Context，避免依赖真实 kv/objstore/gRPC。
//! 供 CRR harness 在 darwin/arm64 上跑内存与本地文件系统路径。
//! MemStorage/ArcMemStorage 适合纯内存事件复制；LocalStorage 对齐 objstore 本地实现。
//! Context/CancelHandle 提供可取消等待；Error.is_not_exist 区分缺失文件。
//! 本文件是桩边界：Create 在裸 MemStorage 上故意不可用，须走 ArcMemStorage。
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio/objstore).

// HashMap 承载内存文件表；fs/io 用于 LocalStorage。
use std::collections::HashMap;
// 本地存储依赖标准库文件系统 API。
use std::fs;
// IoWrite 别名避免与本包 Writer trait 冲突。
use std::io::{Read, Write as IoWrite};
// PathBuf 保存根目录与对象全路径。
use std::path::{Path, PathBuf};
// AtomicBool 标记取消与 Close 状态。
use std::sync::atomic::{AtomicBool, Ordering};
// Arc 共享 Context/Storage；Mutex 保护文件表。
use std::sync::{Arc, Condvar, Mutex};
// PresignFile 过期参数占位，本地实现忽略。
use std::time::Duration;

#[derive(Clone, Debug)]
/// 轻量错误：携带消息与 is_not_exist，对齐对象存储 not-found 语义。
pub struct Error {
    /// 人类可读错误说明，用于断言 contains。
    pub message: String,
    /// 为 true 时表示文件不存在，对应 Go os.IsNotExist。
    pub is_not_exist: bool,
}

impl Error {
    /// 构造普通错误（非 not-exist）。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            is_not_exist: false,
        }
    }

    /// 构造 not-exist 错误，供 Read/Open 缺失路径使用。
    pub fn not_exist(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            is_not_exist: true,
        }
    }
}

// Display 只输出 message，便于 format!/to_string 断言。
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// 接入 std::error::Error，便于 ? 与 anyhow 互操作。
impl std::error::Error for Error {}

// 本包统一 Result 别名，错误类型固定为 stubs::Error。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
/// 可取消上下文替身：AtomicBool + Condvar，非 tokio Cancel。
pub struct Context {
    inner: Arc<ContextInner>,
}

// 内部共享状态；CancelHandle 与 Context 共享同一 Arc。
struct ContextInner {
    // 取消标志，SeqCst 保证跨线程可见。
    cancelled: AtomicBool,
    // 首次取消时写入的错误消息，后续取消不覆盖。
    err: Mutex<Option<String>>,
    // 供等待方在 cancel 时被唤醒（本文件未直接 wait）。
    pair: (Mutex<()>, Condvar),
}

// Default 等同 background，方便结构体填充。
impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// 永不取消的后台上下文。
    pub fn background() -> Self {
        Self {
            inner: Arc::new(ContextInner {
                cancelled: AtomicBool::new(false),
                err: Mutex::new(None),
                pair: (Mutex::new(()), Condvar::new()),
            }),
        }
    }

    /// 返回可取消对：Context + CancelHandle。
    pub fn with_cancel() -> (Self, CancelHandle) {
        let ctx = Self::background();
        let handle = CancelHandle {
            inner: Arc::clone(&ctx.inner),
        };
        (ctx, handle)
    }

    /// 是否已取消。
    pub fn is_done(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 取消时附带的错误文案（若有）。
    pub fn err_message(&self) -> Option<String> {
        self.inner.err.lock().unwrap().clone()
    }
}

#[derive(Clone)]
/// 取消句柄：与 Context 共享 inner，可独立 clone。
pub struct CancelHandle {
    inner: Arc<ContextInner>,
}

impl CancelHandle {
    /// 以默认 "context canceled" 消息取消。
    pub fn cancel(&self) {
        self.cancel_with("context canceled");
    }

    /// 写入消息并置位 cancelled，再 notify_all。
    pub fn cancel_with(&self, msg: impl Into<String>) {
        {
            let mut err = self.inner.err.lock().unwrap();
            // 首次取消写入消息，后续 cancel_with 不覆盖。
            if err.is_none() {
                *err = Some(msg.into());
            }
        }
        // 先写 err 再置位，读者看到 done 时消息已就绪。
        self.inner.cancelled.store(true, Ordering::SeqCst);
        // 唤醒可能在 Condvar 上等待的方。
        self.inner.pair.1.notify_all();
    }
}

#[derive(Clone, Debug, Default)]
/// 打开读选项占位；当前实现忽略具体字段。
pub struct ReaderOption {}

#[derive(Clone, Debug, Default)]
/// 创建写选项占位；当前实现忽略具体字段。
pub struct WriterOption {}

#[derive(Clone, Debug, Default)]
/// 目录遍历选项；SubDir 限制前缀范围。
pub struct WalkOption {
    /// 子目录前缀；空表示遍历全部。
    pub SubDir: String,
}

/// 流式读接口，对齐 objectio.Reader。
pub trait Reader: Send {
    /// 读入 buf，返回字节数；EOF 返回 0。
    fn Read(&mut self, ctx: &Context, buf: &mut [u8]) -> Result<usize>;
    /// 关闭读端；内存实现为 no-op。
    fn Close(&mut self, ctx: &Context) -> Result<()>;
}

/// 流式写接口；Close 时提交缓冲。
pub trait Writer: Send {
    /// 追加写入，返回接受的字节数。
    fn Write(&mut self, ctx: &Context, p: &[u8]) -> Result<usize>;
    fn Close(&mut self, ctx: &Context) -> Result<()>;
}

/// 对象存储表面，覆盖 CRR/flush 测试所需方法子集。
pub trait Storage: Send + Sync {
    /// 原子写整文件（内存直接 insert；本地写盘）。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()>;
    /// 读整文件；缺失返回 is_not_exist。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>>;
    /// 判断对象是否存在。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool>;
    /// 删除单文件；本地缺失视为成功。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()>;
    /// 批量删除，遇错即停。
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()>;
    /// 按 SubDir 前缀遍历，回调 (相对路径, 大小)。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    /// 返回 mem:// 或 file:// 形式的 URI。
    fn URI(&self) -> String;
    /// 打开只读流。
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>>;
    /// 创建写入流；Close 时落盘/提交。
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>>;
    /// 重命名；内存实现为读-写-删。
    fn Rename(&self, ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()>;
    /// 预签名 URL 占位，返回 scheme+路径，非真实签名。
    fn PresignFile(&self, _ctx: &Context, file_name: &str, _expire: Duration) -> Result<String>;
    /// 标记存储关闭；部分实现此后拒绝写入。
    fn Close(&self);
}

#[derive(Default)]
/// 进程内 HashMap 存储；适合事件复制单测，无磁盘 IO。
pub struct MemStorage {
    // 路径 → 字节内容；Mutex 保护并发写。
    files: Mutex<HashMap<String, Vec<u8>>>,
    // Close 置位；本 MemStorage 实现不强制检查。
    closed: AtomicBool,
}

impl MemStorage {
    /// 空内存存储工厂。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回排序后的全部路径，便于断言。
    pub fn paths(&self) -> Vec<String> {
        let mut v: Vec<_> = self.files.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }
}

// 内存读游标：持有快照字节与当前位置。
struct MemReader {
    // Open 时拷贝的文件快照。
    data: Vec<u8>,
    // 下一次 Read 的起始偏移。
    pos: usize,
}

// MemReader：EOF 返回 Ok(0)，不返回错误。
impl Reader for MemReader {
    fn Read(&mut self, _ctx: &Context, buf: &mut [u8]) -> Result<usize> {
        // 已到末尾：返回 0 表示 EOF。
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        // 本次最多填满 buf 或读完剩余数据。
        let n = std::cmp::min(buf.len(), self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }

    fn Close(&mut self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

// 内存写缓冲：Close 时 WriteFile 提交到 MemStorage。
struct MemWriter {
    // 需 Arc 才能在 Close 时写回。
    storage: Arc<MemStorage>,
    // 目标对象名。
    name: String,
    // Close 前累积的写入内容。
    buf: Vec<u8>,
}

// MemWriter：Write 只扩缓冲，Close 才可见。
impl Writer for MemWriter {
    fn Write(&mut self, _ctx: &Context, p: &[u8]) -> Result<usize> {
        // 延迟提交：内容仅在 Close 进入存储。
        self.buf.extend_from_slice(p);
        Ok(p.len())
    }

    fn Close(&mut self, ctx: &Context) -> Result<()> {
        // Close 一次性写入，模拟对象存储最终一致。
        self.storage.WriteFile(ctx, &self.name, &self.buf)
    }
}

// 裸 MemStorage：Create 故意失败，迫使使用 ArcMemStorage。
impl Storage for MemStorage {
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            // 覆盖同名对象，无版本历史。
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            // 缺失统一 not_exist，便于上层分支。
            .ok_or_else(|| Error::not_exist(format!("file not found: {name}")))
    }

    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(name))
    }

    fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
        self.files.lock().unwrap().remove(name);
        Ok(())
    }

    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        for n in names {
            self.DeleteFile(ctx, n)?;
        }
        Ok(())
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        // 去掉首尾斜杠，统一前缀匹配形态。
        let prefix = opt.SubDir.trim_matches('/');
        let entries: Vec<(String, i64)> = {
            let files = self.files.lock().unwrap();
            let mut paths: Vec<_> = files.keys().cloned().collect();
            paths.sort();
            paths
                .into_iter()
                .filter(|path| {
                    // 空前缀匹配全部键。
                    if prefix.is_empty() {
                        true
                    } else {
                        // 精确目录名或目录/子路径。
                        path == prefix || path.starts_with(&format!("{prefix}/"))
                    }
                })
                .map(|path| {
                    let size = files.get(&path).map(|b| b.len() as i64).unwrap_or(0);
                    (path, size)
                })
                .collect()
        };
        for (path, size) in entries {
            fn_(&path, size)?;
        }
        Ok(())
    }

    fn URI(&self) -> String {
        // 固定 scheme，测试只校验前缀。
        "mem://".into()
    }

    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        // Open 读出快照，后续 Read 不受并发写影响。
        let data = self.ReadFile(ctx, name)?;
        Ok(Box::new(MemReader { data, pos: 0 }))
    }

    fn Create(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        // Self is not Arc here; Create for bare MemStorage is unused by CRR
        // worker paths that go through Arc wrappers. Keep a local copy path.
        // 无 Arc 无法安全共享给 Writer，明确拒绝。
        Err(Error::new(
            "MemStorage::Create requires Arc; use LocalStorage or ArcMemStorage",
        ))
    }

    fn Rename(&self, ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()> {
        // Rename：拷贝后删旧，非原子但测试可接受。
        let data = self.ReadFile(ctx, old_file_name)?;
        self.WriteFile(ctx, new_file_name, &data)?;
        self.DeleteFile(ctx, old_file_name)
    }

    fn PresignFile(&self, _ctx: &Context, file_name: &str, _expire: Duration) -> Result<String> {
        // 伪预签名：原样拼 URL，无签名参数。
        Ok(format!("mem://{file_name}"))
    }

    fn Close(&self) {
        // 仅置标志；内存表内容保留供排查。
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// Arc 包装的内存存储，支持 Create 写入流。
/// Arc-backed mem storage that supports Create writers.
#[derive(Clone, Default)]
/// 可 Clone 的内存存储句柄，内部共享同一 MemStorage。
pub struct ArcMemStorage {
    // 真正的文件表；Clone 只增加引用计数。
    inner: Arc<MemStorage>,
}

impl ArcMemStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// 升为 trait 对象，供 CRRUpstreamStorage 等注入。
    pub fn as_storage(&self) -> Arc<dyn Storage> {
        Arc::new(self.clone())
    }

    pub fn paths(&self) -> Vec<String> {
        self.inner.paths()
    }
}

// 方法全部委托 inner；Create 返回持有 Arc 的 MemWriter。
impl Storage for ArcMemStorage {
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        self.inner.WriteFile(ctx, name, data)
    }
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>> {
        self.inner.ReadFile(ctx, name)
    }
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool> {
        self.inner.FileExists(ctx, name)
    }
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()> {
        self.inner.DeleteFile(ctx, name)
    }
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        self.inner.DeleteFiles(ctx, names)
    }
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.inner.WalkDir(ctx, opt, fn_)
    }
    fn URI(&self) -> String {
        self.inner.URI()
    }
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        self.inner.Open(ctx, name, option)
    }
    fn Create(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        // ArcMemStorage::Create：Writer 持有 inner Arc。
        Ok(Box::new(MemWriter {
            storage: Arc::clone(&self.inner),
            name: name.to_string(),
            buf: Vec::new(),
        }))
    }
    fn Rename(&self, ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()> {
        self.inner.Rename(ctx, old_file_name, new_file_name)
    }
    fn PresignFile(&self, ctx: &Context, file_name: &str, expire: Duration) -> Result<String> {
        self.inner.PresignFile(ctx, file_name, expire)
    }
    fn Close(&self) {
        self.inner.Close();
    }
}

/// 本地文件系统存储，对齐 Go `objstore.NewLocalStorage`。
/// Local filesystem storage matching Go `objstore.NewLocalStorage`.
/// 根目录下的相对路径对象存储；URI 为 file://。
pub struct LocalStorage {
    // 存储根目录，创建时 mkdir -p。
    root: PathBuf,
    /// Whether deleting a missing file is treated as success, matching Go's
    /// exported `IgnoreEnoentForDelete` switch.
    pub IgnoreEnoentForDelete: bool,
    closed: AtomicBool,
}

impl LocalStorage {
    /// 确保根目录存在后构造。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        // 构造时确保根存在，失败映射为 Error。
        fs::create_dir_all(&root)
            .map_err(|e| Error::new(format!("mkdir {}: {e}", root.display())))?;
        Ok(Self {
            root,
            IgnoreEnoentForDelete: false,
            closed: AtomicBool::new(false),
        })
    }

    /// 将逻辑名（/ 分隔）拼到 root 下，过滤空段。
    fn full_path(&self, name: &str) -> PathBuf {
        let mut p = self.root.clone();
        // 按 / 分段，忽略空段，防止绝对路径逃逸 root。
        for part in name.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p
    }
}

// 磁盘只读句柄。
struct FileReader {
    file: fs::File,
}

impl Reader for FileReader {
    fn Read(&mut self, _ctx: &Context, buf: &mut [u8]) -> Result<usize> {
        // 透传 std Read/Write 错误。
        self.file
            .read(buf)
            .map_err(|e| Error::new(format!("read: {e}")))
    }
    fn Close(&mut self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

// 先写临时文件，Close 时 rename 到最终路径，降低半写风险。
struct FileWriter {
    // 最终目标路径。
    path: PathBuf,
    // 同目录 .tmp 临时文件。
    tmp: PathBuf,
    file: fs::File,
}

// FileWriter：Flush+rename 提交；失败保留 tmp 便于排查。
impl Writer for FileWriter {
    fn Write(&mut self, _ctx: &Context, p: &[u8]) -> Result<usize> {
        self.file
            .write(p)
            .map_err(|e| Error::new(format!("write: {e}")))
    }
    fn Close(&mut self, _ctx: &Context) -> Result<()> {
        self.file
            .flush()
            .map_err(|e| Error::new(format!("flush: {e}")))?;
        // 临时文件提交为最终对象。
        fs::rename(&self.tmp, &self.path).map_err(|e| Error::new(format!("rename tmp: {e}")))
    }
}

// LocalStorage：路径穿越依赖 full_path 分段拼接，不含 URL 解析。
impl Storage for LocalStorage {
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        let path = self.full_path(name);
        // 写入前创建父目录，支持嵌套逻辑路径。
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir {}: {e}", parent.display())))?;
        }
        fs::write(&path, data).map_err(|e| Error::new(format!("write {}: {e}", path.display())))
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
        let path = self.full_path(name);
        fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                // NotFound → is_not_exist，其它 IO 错误保留原文。
                Error::not_exist(format!("file not found: {name}"))
            } else {
                Error::new(format!("read {}: {e}", path.display()))
            }
        })
    }

    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
        // exists 不区分文件/目录；测试路径皆为文件。
        Ok(self.full_path(name).exists())
    }

    fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
        let path = self.full_path(name);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && self.IgnoreEnoentForDelete => {
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::not_exist(format!(
                "failed to delete file {name}: {e}"
            ))),
            Err(e) => Err(Error::new(format!("delete {}: {e}", path.display()))),
        }
    }

    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        for n in names {
            self.DeleteFile(ctx, n)?;
        }
        Ok(())
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        // Walk 起点：根或 SubDir 对应目录。
        let base = if opt.SubDir.is_empty() {
            self.root.clone()
        } else {
            self.full_path(&opt.SubDir)
        };
        // 子目录不存在时 WalkDir 空成功，不报错。
        if !base.exists() {
            return Ok(());
        }
        walk_collect(&self.root, &base, fn_)
    }

    fn URI(&self) -> String {
        format!("file://{}", self.root.display())
    }

    fn Open(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        let path = self.full_path(name);
        // Open 映射 NotFound 为 not_exist。
        let file = fs::File::open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::not_exist(format!("file not found: {name}"))
            } else {
                Error::new(format!("open {}: {e}", path.display()))
            }
        })?;
        Ok(Box::new(FileReader { file }))
    }

    fn Create(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        let path = self.full_path(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir {}: {e}", parent.display())))?;
        }
        // 用扩展名 .tmp 作 staging，Close 再原子替换。
        let tmp = path.with_extension("tmp");
        let file = fs::File::create(&tmp)
            .map_err(|e| Error::new(format!("create {}: {e}", tmp.display())))?;
        Ok(Box::new(FileWriter { path, tmp, file }))
    }

    fn Rename(&self, _ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()> {
        let old = self.full_path(old_file_name);
        let new = self.full_path(new_file_name);
        if let Some(parent) = new.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir {}: {e}", parent.display())))?;
        }
        fs::rename(&old, &new).map_err(|e| Error::new(format!("rename: {e}")))
    }

    fn PresignFile(&self, _ctx: &Context, file_name: &str, _expire: Duration) -> Result<String> {
        Ok(Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string())
    }

    fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

// 深度优先收集文件；相对路径统一为正斜杠。
fn walk_collect(
    root: &Path,
    dir: &Path,
    fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| Error::new(format!("readdir: {e}")))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        // 递归进入子目录。
        if path.is_dir() {
            walk_collect(root, &path, fn_)?;
        } else {
            // strip_prefix 失败则退回绝对路径字符串。
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                // Windows 分隔符归一化为 /。
                .replace('\\', "/");
            let meta = fs::metadata(&path).map_err(|e| Error::new(format!("stat: {e}")))?;
            // 回调相对路径与字节大小。
            fn_(&rel, meta.len() as i64)?;
        }
    }
    Ok(())
}
