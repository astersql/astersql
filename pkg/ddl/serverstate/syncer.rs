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

// 集群服务器全局状态（server global state）同步器。
//
// 定义 `Syncer` 抽象、基于内存 `StateStore` 的 etcd 风格实现 `EtcdSyncer`，
// 以及状态 JSON 编解码、watch 通道与带超时/取消的 `SyncContext`。
// 全局状态用于协调节点是否处于升级（Upgrading）等运行态。

use std::collections::{HashMap, VecDeque};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// 键读写默认重试次数。
pub const KEY_OP_DEFAULT_RETRY_COUNT: usize = 3;
/// 单次键操作默认超时。
pub const KEY_OP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
/// 键操作重试间隔。
pub const KEY_OP_RETRY_INTERVAL: Duration = Duration::from_millis(30);
/// 日志/会话提示前缀。
pub const STATE_PROMPT: &str = "global-state-syncer";
/// 升级中状态取值。
pub const STATE_UPGRADING: &str = "upgrading";
/// 正常运行状态取值（空串）。
pub const STATE_NORMAL_RUNNING: &str = "";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 状态同步过程中的错误。
pub enum SyncError {
    /// 上下文已取消。
    Cancelled,
    /// 上下文超时。
    Timeout,
    /// 后端存储错误。
    Backend(String),
    /// 状态 JSON 非法。
    InvalidState(String),
    /// get 返回的键值条数不符合预期。
    WrongKeyCount(usize),
    /// 同步器尚未初始化。
    NotInitialized,
    /// watch 通道已关闭。
    WatchClosed,
}

impl Display for SyncError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "context canceled"),
            Self::Timeout => write!(f, "context deadline exceeded"),
            Self::Backend(err) => write!(f, "backend error: {err}"),
            Self::InvalidState(err) => write!(f, "invalid state info: {err}"),
            Self::WrongKeyCount(count) => write!(f, "get key value count:{count} wrong"),
            Self::NotInitialized => write!(f, "state syncer is not initialized"),
            Self::WatchClosed => write!(f, "state watch channel is closed"),
        }
    }
}

impl std::error::Error for SyncError {}

#[derive(Clone, Debug)]
/// 同步操作上下文：支持取消与截止时间（deadline）。
pub struct SyncContext {
    /// 是否已取消。
    cancelled: Arc<AtomicBool>,
    transport: astersql_ddl_schemaver::Context,
    /// 可选截止时间。
    deadline: Option<Instant>,
}

impl Default for SyncContext {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncContext {
    /// 创建无超时、未取消的上下文。
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            transport: astersql_ddl_schemaver::Context::Background(),
            deadline: None,
        }
    }

    /// 派生带超时的子上下文（取父截止与新超时的较早者）。
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        let requested = Instant::now() + timeout;
        Self {
            cancelled: Arc::clone(&self.cancelled),
            transport: self.transport.WithTimeout(timeout),
            deadline: Some(
                self.deadline
                    .map_or(requested, |parent| parent.min(requested)),
            ),
        }
    }

    /// 标记取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.transport.Cancel();
    }

    /// 若已取消或超时则返回对应错误。
    pub fn error(&self) -> Option<SyncError> {
        if self.cancelled.load(Ordering::Acquire) {
            Some(SyncError::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Some(SyncError::Timeout)
        } else if self.transport.Done() {
            Some(SyncError::Cancelled)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 集群全局状态载荷。
pub struct StateInfo {
    /// 状态字符串，例如 upgrading 或空串。
    pub state: String,
}

impl StateInfo {
    /// 用给定状态串构造。
    pub fn new(state: impl Into<String>) -> Self {
        Self {
            state: state.into(),
        }
    }

    /// 序列化为简化 JSON：`{"state":"..."}`。
    pub fn marshal(&self) -> Result<Vec<u8>, SyncError> {
        let mut value = String::with_capacity(self.state.len() + 12);
        value.push_str("{\"state\":\"");
        // 手工转义，避免引入完整 JSON 依赖。
        for character in self.state.chars() {
            match character {
                '"' => value.push_str("\\\""),
                '\\' => value.push_str("\\\\"),
                '\n' => value.push_str("\\n"),
                '\r' => value.push_str("\\r"),
                '\t' => value.push_str("\\t"),
                character if character < '\u{20}' => {
                    value.push_str(&format!("\\u{:04x}", character as u32));
                }
                character => value.push(character),
            }
        }
        value.push_str("\"}");
        Ok(value.into_bytes())
    }

    /// 从简化 JSON 反序列化状态。
    pub fn unmarshal(data: &[u8]) -> Result<Self, SyncError> {
        let input =
            std::str::from_utf8(data).map_err(|err| SyncError::InvalidState(err.to_string()))?;
        let mut cursor = JsonCursor::new(input);
        cursor.space();
        cursor.expect(b'{')?;
        let mut state = None;
        loop {
            cursor.space();
            if cursor.consume(b'}') {
                break;
            }
            let key = cursor.string()?;
            cursor.space();
            cursor.expect(b':')?;
            cursor.space();
            // 仅关心 state 字段，其余值跳过以保持向前兼容。
            if key.eq_ignore_ascii_case("state") {
                if cursor.peek() == Some(b'n') && cursor.consume_literal(b"null") {
                    state = Some(String::new());
                } else {
                    state = Some(cursor.string()?);
                }
            } else {
                cursor.skip_value()?;
            }
            cursor.space();
            if cursor.consume(b'}') {
                break;
            }
            cursor.expect(b',')?;
        }
        cursor.space();
        if cursor.position != cursor.input.len() {
            return Err(cursor.error("trailing JSON data"));
        }
        Ok(Self {
            state: state.unwrap_or_default(),
        })
    }
}

/// 极简 JSON 游标，只够解析 `StateInfo` 所需子集。
struct JsonCursor<'a> {
    /// 输入字节。
    input: &'a [u8],
    /// 当前解析位置。
    position: usize,
}

impl<'a> JsonCursor<'a> {
    /// 从字符串构造游标。
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    /// 解析 JSON 字符串字面量。
    fn string(&mut self) -> Result<String, SyncError> {
        self.expect(b'"')?;
        let mut output = String::new();
        while let Some(byte) = self.next() {
            match byte {
                b'"' => return Ok(output),
                b'\\' => match self.next() {
                    Some(b'"') => output.push('"'),
                    Some(b'\\') => output.push('\\'),
                    Some(b'/') => output.push('/'),
                    Some(b'b') => output.push('\u{08}'),
                    Some(b'f') => output.push('\u{0c}'),
                    Some(b'n') => output.push('\n'),
                    Some(b'r') => output.push('\r'),
                    Some(b't') => output.push('\t'),
                    Some(b'u') => output.push(self.unicode()?),
                    _ => return Err(self.error("invalid string escape")),
                },
                0..=0x1f => return Err(self.error("control character in string")),
                _ => {
                    self.position -= 1;
                    let tail = std::str::from_utf8(&self.input[self.position..])
                        .map_err(|err| SyncError::InvalidState(err.to_string()))?;
                    let character = tail
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("unterminated string"))?;
                    output.push(character);
                    self.position += character.len_utf8();
                }
            }
        }
        Err(self.error("unterminated string"))
    }

    /// 解析 `\uXXXX` 转义，包括 JSON 使用的 UTF-16 代理对。
    fn unicode(&mut self) -> Result<char, SyncError> {
        let high = self.unicode_code_unit()?;
        if (0xd800..=0xdbff).contains(&high) {
            if !self.input[self.position..].starts_with(b"\\u") {
                return Ok(char::REPLACEMENT_CHARACTER);
            }
            self.position += 2;
            let low = self.unicode_code_unit()?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return Ok(char::REPLACEMENT_CHARACTER);
            }
            let code = 0x10000 + (((high as u32 - 0xd800) << 10) | (low as u32 - 0xdc00));
            return char::from_u32(code).ok_or_else(|| self.error("invalid unicode scalar"));
        }
        if (0xdc00..=0xdfff).contains(&high) {
            return Ok(char::REPLACEMENT_CHARACTER);
        }
        char::from_u32(high as u32).ok_or_else(|| self.error("invalid unicode scalar"))
    }

    fn unicode_code_unit(&mut self) -> Result<u16, SyncError> {
        if self.position + 4 > self.input.len() {
            return Err(self.error("short unicode escape"));
        }
        let digits = std::str::from_utf8(&self.input[self.position..self.position + 4])
            .map_err(|err| SyncError::InvalidState(err.to_string()))?;
        self.position += 4;
        u16::from_str_radix(digits, 16).map_err(|_| self.error("invalid unicode escape"))
    }

    /// 跳过任意 JSON 值（对象/数组/标量）。
    fn skip_value(&mut self) -> Result<(), SyncError> {
        self.space();
        match self.peek() {
            Some(b'"') => self.string().map(|_| ()),
            Some(b'{') => self.skip_object(),
            Some(b'[') => self.skip_array(),
            Some(b't') if self.consume_literal(b"true") => Ok(()),
            Some(b'f') if self.consume_literal(b"false") => Ok(()),
            Some(b'n') if self.consume_literal(b"null") => Ok(()),
            Some(b'-' | b'0'..=b'9') => self.skip_number(),
            Some(_) => Err(self.error("invalid JSON value")),
            None => Err(self.error("expected JSON value")),
        }
    }

    fn skip_object(&mut self) -> Result<(), SyncError> {
        self.expect(b'{')?;
        self.space();
        if self.consume(b'}') {
            return Ok(());
        }
        loop {
            self.string()?;
            self.space();
            self.expect(b':')?;
            self.skip_value()?;
            self.space();
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
            self.space();
        }
    }

    fn skip_array(&mut self) -> Result<(), SyncError> {
        self.expect(b'[')?;
        self.space();
        if self.consume(b']') {
            return Ok(());
        }
        loop {
            self.skip_value()?;
            self.space();
            if self.consume(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn consume_literal(&mut self, literal: &[u8]) -> bool {
        if self.input[self.position..].starts_with(literal) {
            self.position += literal.len();
            true
        } else {
            false
        }
    }

    fn skip_number(&mut self) -> Result<(), SyncError> {
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                self.position += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(self.error("invalid JSON number")),
        }
        if self.consume(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("invalid JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("invalid JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        Ok(())
    }

    /// 跳过空白。
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    /// 期望并消费指定字节。
    fn expect(&mut self, byte: u8) -> Result<(), SyncError> {
        if self.consume(byte) {
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", byte as char)))
        }
    }

    /// 若下一字节匹配则消费。
    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    /// 窥视下一字节。
    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    /// 消费并返回下一字节。
    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    /// 构造带位置信息的解析错误。
    fn error(&self, message: &str) -> SyncError {
        SyncError::InvalidState(format!("{message} at byte {}", self.position))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次 watch 事件的键值。
pub struct WatchResponse {
    /// 被观察的键。
    pub key: String,
    /// 新值字节。
    pub value: Vec<u8>,
}

#[derive(Clone)]
/// 对外暴露的 watch 接收通道。
pub struct WatchChannel {
    /// 共享接收端。
    receiver: Arc<Mutex<mpsc::Receiver<WatchResponse>>>,
}

impl WatchChannel {
    /// 阻塞接收下一事件。
    pub fn recv(&self) -> Result<WatchResponse, SyncError> {
        self.receiver
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| SyncError::WatchClosed)
    }

    /// 限时接收下一事件。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<WatchResponse, SyncError> {
        self.receiver
            .lock()
            .unwrap()
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => SyncError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => SyncError::WatchClosed,
            })
    }
}

/// 可替换接收端的 watch 持有者（供 Syncer 实现内部使用）。
pub(crate) struct Watcher {
    /// 当前接收端。
    receiver: Arc<Mutex<mpsc::Receiver<WatchResponse>>>,
}

impl Default for Watcher {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }
}

impl Watcher {
    /// 替换底层接收端（用于 init/rewatch）。
    pub(crate) fn replace(&self, receiver: mpsc::Receiver<WatchResponse>) {
        *self.receiver.lock().unwrap() = receiver;
    }

    /// 导出共享接收通道。
    pub(crate) fn channel(&self) -> WatchChannel {
        WatchChannel {
            receiver: Arc::clone(&self.receiver),
        }
    }
}

/// 全局状态同步器接口。
pub trait Syncer: Send + Sync {
    /// 初始化会话并订阅状态变更。
    fn init(&self, ctx: &SyncContext) -> Result<(), SyncError>;
    /// 写入新的全局状态。
    fn update_global_state(
        &self,
        ctx: &SyncContext,
        state_info: StateInfo,
    ) -> Result<(), SyncError>;
    /// 读取全局状态并刷新本地缓存。
    fn get_global_state(&self, ctx: &SyncContext) -> Result<StateInfo, SyncError>;
    /// 本地缓存是否为升级态。
    fn is_upgrading_state(&self) -> bool;
    /// 取得状态变更 watch 通道。
    fn watch_chan(&self) -> WatchChannel;
    /// 重新订阅 watch（连接中断后恢复）。
    fn rewatch(&self, ctx: &SyncContext);
}

/// 内存存储上的单个 watch 订阅。
struct StoreWatcher {
    /// 订阅 ID。
    id: u64,
    /// 监听的键。
    key: String,
    /// 事件发送端。
    sender: mpsc::Sender<WatchResponse>,
}

#[derive(Default)]
/// `StateStore` 的可变内部状态。
struct StoreState {
    /// 键到多版本/多值列表。
    values: HashMap<String, Vec<Vec<u8>>>,
    /// 活跃订阅。
    watchers: Vec<StoreWatcher>,
    /// 测试注入的下一次失败队列。
    failures: VecDeque<SyncError>,
}

#[derive(Default)]
/// 进程内键值存储，模拟 etcd 的 get/put/watch，便于单测。
pub struct StateStore {
    /// 受保护的存储内容。
    state: Mutex<StoreState>,
    /// 分配订阅 ID 的计数器。
    next_watcher: AtomicU64,
}

impl StateStore {
    /// 注入下一次操作失败（测试用）。
    pub fn fail_next(&self, error: SyncError) {
        self.state.lock().unwrap().failures.push_back(error);
    }

    /// 直接写入原始多值（测试夹具）。
    pub fn set_raw_values(&self, key: impl Into<String>, values: Vec<Vec<u8>>) {
        self.state.lock().unwrap().values.insert(key.into(), values);
    }

    /// 弹出并返回注入的失败；无则 Ok。
    fn take_failure(state: &mut StoreState) -> Result<(), SyncError> {
        state.failures.pop_front().map_or(Ok(()), Err)
    }

    /// 模拟创建会话：检查上下文并消费一次失败注入。
    fn create_session(&self, ctx: &SyncContext, _prompt: &str) -> Result<(), SyncError> {
        if let Some(error) = ctx.error() {
            return Err(error);
        }
        let mut state = self.state.lock().unwrap();
        Self::take_failure(&mut state)
    }

    /// 读取键对应的值列表。
    fn get(&self, ctx: &SyncContext, key: &str) -> Result<Vec<Vec<u8>>, SyncError> {
        if let Some(error) = ctx.error() {
            return Err(error);
        }
        let mut state = self.state.lock().unwrap();
        Self::take_failure(&mut state)?;
        Ok(state.values.get(key).cloned().unwrap_or_default())
    }

    /// 写入键值并通知匹配的 watcher。
    fn put(&self, ctx: &SyncContext, key: &str, value: Vec<u8>) -> Result<(), SyncError> {
        if let Some(error) = ctx.error() {
            return Err(error);
        }
        let mut state = self.state.lock().unwrap();
        Self::take_failure(&mut state)?;
        state.values.insert(key.to_owned(), vec![value.clone()]);
        let response = WatchResponse {
            key: key.to_owned(),
            value,
        };
        // 发送失败的订阅视为已断开并移除。
        state
            .watchers
            .retain(|watcher| watcher.key != key || watcher.sender.send(response.clone()).is_ok());
        Ok(())
    }

    /// 注册 watch；上下文结束后后台线程清理订阅。
    fn watch(self: &Arc<Self>, ctx: SyncContext, key: String) -> mpsc::Receiver<WatchResponse> {
        let (sender, receiver) = mpsc::channel();
        let id = self.next_watcher.fetch_add(1, Ordering::Relaxed);
        self.state
            .lock()
            .unwrap()
            .watchers
            .push(StoreWatcher { id, key, sender });
        let store = Arc::clone(self);
        thread::spawn(move || {
            while ctx.error().is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            store
                .state
                .lock()
                .unwrap()
                .watchers
                .retain(|watcher| watcher.id != id);
        });
        receiver
    }
}

/// 基于 `StateStore` 的 etcd 风格全局状态同步器。
pub struct EtcdSyncer {
    /// 状态键路径。
    etcd_path: String,
    /// 日志前缀。
    prompt: String,
    /// 底层存储。
    store: StateBackend,
    /// 会话是否已建立。
    session_ready: AtomicBool,
    /// 本地缓存的全局状态。
    cluster_state: RwLock<Arc<StateInfo>>,
    /// 全局状态 watch。
    global_state_watcher: Watcher,
    watch_context: Mutex<Option<SyncContext>>,
}

impl EtcdSyncer {
    /// 构造同步器，初始为正常运行态。
    pub fn new(store: Arc<StateStore>, etcd_path: impl Into<String>) -> Self {
        Self {
            etcd_path: etcd_path.into(),
            prompt: STATE_PROMPT.to_owned(),
            store: StateBackend::Memory(store),
            session_ready: AtomicBool::new(false),
            cluster_state: RwLock::new(Arc::new(StateInfo::new(STATE_NORMAL_RUNNING))),
            global_state_watcher: Watcher::default(),
            watch_context: Mutex::new(None),
        }
    }

    /// Use the public etcd transport. GetGlobalState can be called directly,
    /// without allocating a lease or watch, as in Go crossks.
    pub fn with_client(
        client: Arc<dyn astersql_ddl_schemaver::EtcdClient>,
        path: impl Into<String>,
    ) -> Self {
        let mut syncer = Self::new(Arc::new(StateStore::default()), path);
        syncer.store = StateBackend::Etcd {
            client,
            session: Mutex::new(None),
        };
        syncer
    }

    fn start_watch(&self, ctx: &SyncContext) {
        let child = SyncContext {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: ctx.deadline,
            transport: ctx.transport.Child(),
        };
        if let Some(old) = self.watch_context.lock().unwrap().replace(child.clone()) {
            old.cancel();
        }
        self.global_state_watcher
            .replace(self.store.watch(child, self.etcd_path.clone()));
    }

    /// 带重试与子超时地读取键值。
    fn get_key_value(
        &self,
        ctx: &SyncContext,
        key: &str,
        retry_count: usize,
        timeout: Duration,
    ) -> Result<Vec<Vec<u8>>, SyncError> {
        let mut last_error = None;
        for _ in 0..retry_count {
            if let Some(error) = ctx.error() {
                return Err(error);
            }
            let child = ctx.with_timeout(timeout);
            match self.store.get(&child, key) {
                Ok(values) => return Ok(values),
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| SyncError::Backend("get key failed".to_owned())))
    }

    /// 带重试与子超时地写入键值。
    fn put_key_value(&self, ctx: &SyncContext, key: &str, value: Vec<u8>) -> Result<(), SyncError> {
        let mut last_error = None;
        for _ in 0..KEY_OP_DEFAULT_RETRY_COUNT {
            if let Some(error) = ctx.error() {
                return Err(error);
            }
            let child = ctx.with_timeout(KEY_OP_DEFAULT_TIMEOUT);
            match self.store.put(&child, key, value.clone()) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(KEY_OP_RETRY_INTERVAL);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| SyncError::Backend("put key failed".to_owned())))
    }
}

/// 构造装箱后的 `EtcdSyncer`。
pub fn new_etcd_syncer(store: Arc<StateStore>, etcd_path: impl Into<String>) -> Arc<dyn Syncer> {
    Arc::new(EtcdSyncer::new(store, etcd_path))
}

impl Syncer for EtcdSyncer {
    /// 建会话、拉取状态并开始 watch。
    fn init(&self, ctx: &SyncContext) -> Result<(), SyncError> {
        let log_prefix = format!("[{}] {}", self.prompt, self.etcd_path);
        self.store.create_session(ctx, &log_prefix)?;
        self.session_ready.store(true, Ordering::Release);
        let state = self.get_global_state(ctx)?;
        *self.cluster_state.write().unwrap() = Arc::new(state);
        self.start_watch(ctx);
        Ok(())
    }

    /// 将状态写入存储键。
    fn update_global_state(
        &self,
        ctx: &SyncContext,
        state_info: StateInfo,
    ) -> Result<(), SyncError> {
        self.put_key_value(ctx, &self.etcd_path, state_info.marshal()?)
    }

    /// 读取键值并更新本地缓存；0 条视为默认状态，多于 1 条报错。
    fn get_global_state(&self, ctx: &SyncContext) -> Result<StateInfo, SyncError> {
        let values = self.get_key_value(
            ctx,
            &self.etcd_path,
            KEY_OP_DEFAULT_RETRY_COUNT,
            KEY_OP_DEFAULT_TIMEOUT,
        )?;
        let state = match values.as_slice() {
            [] => StateInfo::default(),
            [value] => StateInfo::unmarshal(value)?,
            values => return Err(SyncError::WrongKeyCount(values.len())),
        };
        *self.cluster_state.write().unwrap() = Arc::new(state.clone());
        Ok(state)
    }

    /// 判断本地缓存是否为升级态。
    fn is_upgrading_state(&self) -> bool {
        self.cluster_state.read().unwrap().state == STATE_UPGRADING
    }

    /// 返回全局状态 watch 通道。
    fn watch_chan(&self) -> WatchChannel {
        self.global_state_watcher.channel()
    }

    /// 重新注册对状态键的 watch。
    fn rewatch(&self, ctx: &SyncContext) {
        self.start_watch(ctx);
    }
}

impl Drop for EtcdSyncer {
    fn drop(&mut self) {
        if let Some(ctx) = self.watch_context.lock().unwrap().take() {
            ctx.cancel();
        }
    }
}

/// Both backends execute the same state decoding, cache and retry logic.
enum StateBackend {
    Memory(Arc<StateStore>),
    Etcd {
        client: Arc<dyn astersql_ddl_schemaver::EtcdClient>,
        session: Mutex<Option<astersql_ddl_schemaver::Session>>,
    },
}
impl StateBackend {
    fn create_session(&self, ctx: &SyncContext, prompt: &str) -> Result<(), SyncError> {
        match self {
            Self::Memory(store) => store.create_session(ctx, prompt),
            Self::Etcd { client, session } => {
                for attempt in 0..KEY_OP_DEFAULT_RETRY_COUNT {
                    if let Some(error) = ctx.error() {
                        return Err(error);
                    }
                    match client.NewSession(&ctx.transport, astersql_ddl_schemaver::SessionTTL) {
                        Ok(new_session) => {
                            *session.lock().unwrap() = Some(new_session);
                            return Ok(());
                        }
                        Err(error) if attempt + 1 == KEY_OP_DEFAULT_RETRY_COUNT => {
                            return Err(SyncError::Backend(error.to_string()));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(200)),
                    }
                }

                Ok(())
            }
        }
    }
    fn get(&self, ctx: &SyncContext, key: &str) -> Result<Vec<Vec<u8>>, SyncError> {
        match self {
            Self::Memory(store) => store.get(ctx, key),
            Self::Etcd { client, .. } => client
                .Get(&ctx.transport, key, false)
                .map(|r| r.Kvs.into_iter().map(|kv| kv.Value).collect())
                .map_err(|e| SyncError::Backend(e.to_string())),
        }
    }
    fn put(&self, ctx: &SyncContext, key: &str, value: Vec<u8>) -> Result<(), SyncError> {
        match self {
            Self::Memory(store) => store.put(ctx, key, value),
            Self::Etcd { client, .. } => client
                .Put(
                    &ctx.transport,
                    key,
                    std::str::from_utf8(&value)
                        .map_err(|e| SyncError::InvalidState(e.to_string()))?,
                    None,
                )
                .map_err(|e| SyncError::Backend(e.to_string())),
        }
    }
    fn watch(&self, ctx: SyncContext, key: String) -> mpsc::Receiver<WatchResponse> {
        match self {
            Self::Memory(store) => store.watch(ctx, key),
            Self::Etcd { client, .. } => {
                let watch = client.Watch(&ctx.transport, &key, false, 0);
                let (sender, receiver) = mpsc::channel();
                thread::spawn(move || {
                    while ctx.error().is_none() {
                        match watch.RecvTimeout(Duration::from_millis(20)) {
                            Ok(response) => {
                                if response.Error.is_some() || response.CompactRevision > 0 {
                                    break;
                                }
                                for event in response.Events {
                                    if sender
                                        .send(WatchResponse {
                                            key: String::from_utf8_lossy(&event.Kv.Key)
                                                .into_owned(),
                                            value: event.Kv.Value,
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                });
                receiver
            }
        }
    }
}
impl Drop for StateBackend {
    fn drop(&mut self) {
        if let Self::Etcd { session, .. } = self {
            if let Some(session) = session.lock().unwrap().take() {
                session.Close();
            }
        }
    }
}
