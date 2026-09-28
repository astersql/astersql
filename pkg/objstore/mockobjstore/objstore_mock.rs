// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 对象存储 `Storage` 的严格 Mock（对齐 GoMock）。
//
// 通过 `EXPECT` 预先注册每次调用的返回值；实际调用按 FIFO 消费期望，
// 未配置的方法会 panic。`calls`/`verify` 用于断言调用顺序与期望是否耗尽。

#![allow(non_snake_case)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Result, anyhow};
use objectio::{Context, Reader, Writer};

use crate::storeapi::{ReaderOption, Storage, WalkOption, WriterOption};

/// Mock 方法返回值：成功或错误字符串（转为 anyhow）。
type MockResult<T> = std::result::Result<T, String>;

/// One call forwarded through the mock, with the same observable argument
/// order as the generated GoMock implementation.
/// 经 Mock 转发的一次调用，参数顺序与生成的 GoMock 实现可观测顺序一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Call {
    Close,
    Create(String, Option<(i32, i64)>),
    DeleteFile(String),
    DeleteFiles(Vec<String>),
    FileExists(String),
    Open(String, Option<(Option<i64>, Option<i64>, i32)>),
    ReadFile(String),
    Rename(String, String),
    PresignFile(String, Duration),
    URI,
    WalkDir(Option<(String, bool, String, i64, bool, String)>),
    WriteFile(String, Vec<u8>),
}

/// 内部共享状态：已发生调用列表 + 各方法待消费的期望队列。
#[derive(Default)]
struct State {
    calls: Vec<Call>,
    close: usize,
    create: VecDeque<MockResult<Box<dyn Writer + Send>>>,
    delete_file: VecDeque<MockResult<()>>,
    delete_files: VecDeque<MockResult<()>>,
    file_exists: VecDeque<MockResult<bool>>,
    open: VecDeque<MockResult<Box<dyn Reader + Send>>>,
    read_file: VecDeque<MockResult<Vec<u8>>>,
    rename: VecDeque<MockResult<()>>,
    presign_file: VecDeque<MockResult<String>>,
    uri: VecDeque<String>,
    walk_dir: VecDeque<(Vec<(String, i64)>, MockResult<()>)>,
    write_file: VecDeque<MockResult<()>>,
}

/// A strict mock of `storeapi::Storage`.
///
/// Each method consumes one result registered through `EXPECT`, mirroring a
/// single GoMock expected call. Calling an unconfigured method fails loudly.
/// `storeapi::Storage` 的严格 Mock：每次方法调用消费一条 `EXPECT` 注册结果；
/// 未配置则 panic（对齐 GoMock 单次期望调用语义）。
#[derive(Clone, Default)]
pub struct MockStorage {
    state: Arc<Mutex<State>>,
}

/// Recorder returned by `MockStorage::EXPECT`.
/// 由 `MockStorage::EXPECT` 返回的期望录制器。
#[derive(Clone)]
pub struct MockStorageMockRecorder {
    state: Arc<Mutex<State>>,
}

/// 构造默认空期望的 `MockStorage`。
pub fn NewMockStorage() -> MockStorage {
    MockStorage::default()
}

/// 获取状态锁；毒化锁时仍取内层数据以便测试继续。
fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 从期望队列弹出一条；无期望则 panic，Err 转为 anyhow。
fn take<T>(queue: &mut VecDeque<MockResult<T>>, method: &str) -> Result<T> {
    match queue
        .pop_front()
        .unwrap_or_else(|| panic!("unexpected {method} call: no expectation registered"))
    {
        Ok(value) => Ok(value),
        Err(message) => Err(anyhow!(message)),
    }
}

/// 将 WalkOption 展平为可比较的元组，便于记入 `Call::WalkDir`。
fn walk_option(option: Option<&WalkOption>) -> Option<(String, bool, String, i64, bool, String)> {
    option.map(|option| {
        (
            option.SubDir.clone(),
            option.SkipSubDir,
            option.ObjPrefix.clone(),
            option.ListCount,
            option.IncludeTombstone,
            option.StartAfter.clone(),
        )
    })
}

/// 将 WriterOption 展平为可比较快照，保留传给 GoMock 控制器的参数。
fn writer_option(option: Option<&WriterOption>) -> Option<(i32, i64)> {
    option.map(|option| (option.Concurrency, option.PartSize))
}

/// 将 ReaderOption 展平为可比较快照，保留传给 GoMock 控制器的参数。
fn reader_option(option: Option<&ReaderOption>) -> Option<(Option<i64>, Option<i64>, i32)> {
    option.map(|option| (option.StartOffset, option.EndOffset, option.PrefetchSize))
}

impl MockStorage {
    /// 返回期望录制器，用于注册后续调用的返回值。
    pub fn EXPECT(&self) -> MockStorageMockRecorder {
        MockStorageMockRecorder {
            state: Arc::clone(&self.state),
        }
    }

    /// GoMock 兼容空方法（标识本类型为 gomock 风格 Mock）。
    pub fn ISGOMOCK(&self) {}

    /// 已发生调用的快照（顺序与实际调用一致）。
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.state).calls.clone()
    }

    /// Assert that every registered expected call has been consumed.
    /// 断言所有已注册期望均已消费完毕。
    pub fn verify(&self) {
        let state = lock(&self.state);
        assert_eq!(state.close, 0, "unmet Close expectations");
        assert!(state.create.is_empty(), "unmet Create expectations");
        assert!(
            state.delete_file.is_empty(),
            "unmet DeleteFile expectations"
        );
        assert!(
            state.delete_files.is_empty(),
            "unmet DeleteFiles expectations"
        );
        assert!(
            state.file_exists.is_empty(),
            "unmet FileExists expectations"
        );
        assert!(state.open.is_empty(), "unmet Open expectations");
        assert!(state.read_file.is_empty(), "unmet ReadFile expectations");
        assert!(state.rename.is_empty(), "unmet Rename expectations");
        assert!(
            state.presign_file.is_empty(),
            "unmet PresignFile expectations"
        );
        assert!(state.uri.is_empty(), "unmet URI expectations");
        assert!(state.walk_dir.is_empty(), "unmet WalkDir expectations");
        assert!(state.write_file.is_empty(), "unmet WriteFile expectations");
    }
}

impl MockStorageMockRecorder {
    /// 注册一次 Close 期望（用计数表示次数）。
    pub fn Close(&self) {
        lock(&self.state).close += 1;
    }

    /// 注册 Create 的返回值。
    pub fn Create(&self, result: MockResult<Box<dyn Writer + Send>>) {
        lock(&self.state).create.push_back(result);
    }

    /// 注册 DeleteFile 的返回值。
    pub fn DeleteFile(&self, result: MockResult<()>) {
        lock(&self.state).delete_file.push_back(result);
    }

    /// 注册 DeleteFiles 的返回值。
    pub fn DeleteFiles(&self, result: MockResult<()>) {
        lock(&self.state).delete_files.push_back(result);
    }

    /// 注册 FileExists 的返回值。
    pub fn FileExists(&self, result: MockResult<bool>) {
        lock(&self.state).file_exists.push_back(result);
    }

    /// 注册 Open 的返回值。
    pub fn Open(&self, result: MockResult<Box<dyn Reader + Send>>) {
        lock(&self.state).open.push_back(result);
    }

    /// 注册 ReadFile 的返回值。
    pub fn ReadFile(&self, result: MockResult<Vec<u8>>) {
        lock(&self.state).read_file.push_back(result);
    }

    /// 注册 Rename 的返回值。
    pub fn Rename(&self, result: MockResult<()>) {
        lock(&self.state).rename.push_back(result);
    }

    /// 注册 PresignFile 的返回值。
    pub fn PresignFile(&self, result: MockResult<String>) {
        lock(&self.state).presign_file.push_back(result);
    }

    /// 注册 URI 返回字符串。
    pub fn URI(&self, result: String) {
        lock(&self.state).uri.push_back(result);
    }

    /// 注册 WalkDir：回调将收到的条目列表，以及最终 Result。
    pub fn WalkDir(&self, entries: Vec<(String, i64)>, result: MockResult<()>) {
        lock(&self.state).walk_dir.push_back((entries, result));
    }

    /// 注册 WriteFile 的返回值。
    pub fn WriteFile(&self, result: MockResult<()>) {
        lock(&self.state).write_file.push_back(result);
    }
}

impl Storage for MockStorage {
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(Call::WriteFile(name.to_owned(), data.to_vec()));
        take(&mut state.write_file, "WriteFile")
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
        let mut state = lock(&self.state);
        state.calls.push(Call::ReadFile(name.to_owned()));
        take(&mut state.read_file, "ReadFile")
    }

    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
        let mut state = lock(&self.state);
        state.calls.push(Call::FileExists(name.to_owned()));
        take(&mut state.file_exists, "FileExists")
    }

    fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
        let mut state = lock(&self.state);
        state.calls.push(Call::DeleteFile(name.to_owned()));
        take(&mut state.delete_file, "DeleteFile")
    }

    fn Open(
        &self,
        _ctx: &Context,
        path: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(Call::Open(path.to_owned(), reader_option(option)));
        take(&mut state.open, "Open").map(|reader| reader as Box<dyn Reader>)
    }

    fn DeleteFiles(&self, _ctx: &Context, names: &[String]) -> Result<()> {
        let mut state = lock(&self.state);
        state.calls.push(Call::DeleteFiles(names.to_vec()));
        take(&mut state.delete_files, "DeleteFiles")
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        option: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        // 先记录调用并取出期望，再在锁外执行回调，避免重入死锁。
        let (entries, result) = {
            let mut state = lock(&self.state);
            state.calls.push(Call::WalkDir(walk_option(option)));
            state
                .walk_dir
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected WalkDir call: no expectation registered"))
        };
        for (name, size) in entries {
            callback(&name, size)?;
        }
        result.map_err(|message| anyhow!(message))
    }

    fn URI(&self) -> String {
        let mut state = lock(&self.state);
        state.calls.push(Call::URI);
        state
            .uri
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected URI call: no expectation registered"))
    }

    fn Create(
        &self,
        _ctx: &Context,
        path: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(Call::Create(path.to_owned(), writer_option(option)));
        take(&mut state.create, "Create").map(|writer| writer as Box<dyn Writer>)
    }

    fn Rename(&self, _ctx: &Context, old_name: &str, new_name: &str) -> Result<()> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(Call::Rename(old_name.to_owned(), new_name.to_owned()));
        take(&mut state.rename, "Rename")
    }

    fn PresignFile(&self, _ctx: &Context, file_name: &str, expire: Duration) -> Result<String> {
        let mut state = lock(&self.state);
        state
            .calls
            .push(Call::PresignFile(file_name.to_owned(), expire));
        take(&mut state.presign_file, "PresignFile")
    }

    fn Close(&self) {
        let mut state = lock(&self.state);
        state.calls.push(Call::Close);
        assert!(
            state.close > 0,
            "unexpected Close call: no expectation registered"
        );
        state.close -= 1;
    }
}
