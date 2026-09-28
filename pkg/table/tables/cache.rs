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

// 缓存表（Cached Table）的内存缓冲与租约（lease）协作逻辑。
//
// 缓存表将整表数据加载到进程内 mem buffer，用读/写租约与远程状态服务
// （`StateRemote`）协调多实例一致性；`TokenLimit` 串行化非线程安全的远程句柄。
// 时间戳与租约使用 TiDB 习惯的「物理时间 + 逻辑位」编码。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

/// 单张缓存表允许占用的内存上限（字节），默认 64 MiB。
pub const CACHED_TABLE_SIZE_LIMIT: i64 = 64 * (1 << 20);
/// 写锁租约默认时长（对应 Go `cacheTableWriteLease`）。
pub const CACHE_TABLE_WRITE_LEASE: Duration = Duration::from_secs(5);
/// TiDB 时间戳中逻辑部分占用的低位比特数，用于把毫秒时长编码进租约截止时间。
const LOGICAL_BITS: u32 = 18;

/// 进程内有序 KV 缓冲，键值均为原始字节。
pub type MemBuffer = BTreeMap<Vec<u8>, Vec<u8>>;

/// 一份已加载（或正在加载）的缓存数据快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CacheData {
    /// 缓存生效的起始时间戳（含）。
    pub start: u64,
    /// 读租约截止时间戳（不含）；超过后缓存视为失效。
    pub lease: u64,
    /// `None` while a background load is in progress.
    /// 内存缓冲；`None` 表示后台加载尚未完成。
    pub mem_buffer: Option<Arc<MemBuffer>>,
}

/// 由当前时间戳与租约时长计算出租约截止时间戳（物理毫秒左移逻辑位后相加）。
pub fn lease_from_ts(timestamp: u64, lease_duration: Duration) -> u64 {
    let physical_timestamp = timestamp & !((1_u64 << LOGICAL_BITS) - 1);
    physical_timestamp.saturating_add(
        u64::try_from(lease_duration.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_mul(1_u64 << LOGICAL_BITS),
    )
}

/// 远程缓存表状态服务：读写锁与租约续期。
pub trait StateRemote: Send {
    /// 远程调用错误类型。
    type Error;
    /// 为读路径申请/刷新读租约；返回是否成功拿到锁。
    fn lock_for_read(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error>;
    /// 续期读租约，返回新的租约截止时间。
    fn renew_read_lease(
        &mut self,
        table_id: i64,
        old_lease: u64,
        new_lease: u64,
    ) -> Result<u64, Self::Error>;
    /// 申请写锁并返回写租约截止时间。
    fn lock_for_write(
        &mut self,
        table_id: i64,
        lease_duration: Duration,
    ) -> Result<u64, Self::Error>;
    /// 续期写租约；返回是否成功。
    fn renew_write_lease(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error>;
}

/// Capacity-one channel semantics used to serialize the non-thread-safe remote
/// handle. `take` blocks and `try_take` implements Go's select/default branch.
///
/// 容量为 1 的令牌通道：串行化非线程安全的远程句柄。
/// `take` 阻塞等待，`try_take` 对应 Go `select` 的 default 分支。
pub struct TokenLimit<R> {
    /// 可选的远程句柄；`None` 表示令牌已被借出。
    value: Mutex<Option<R>>,
    /// 句柄归还时唤醒阻塞在 `take` 上的等待者。
    ready: Condvar,
}

impl<R> TokenLimit<R> {
    /// 用初始远程句柄构造令牌通道。
    pub fn new(value: R) -> Self {
        Self {
            value: Mutex::new(Some(value)),
            ready: Condvar::new(),
        }
    }

    /// 阻塞取出远程句柄；无可用令牌时等待归还。
    pub fn take(&self) -> R {
        let mut value = self.value.lock().expect("cache token lock poisoned");
        loop {
            if let Some(handle) = value.take() {
                return handle;
            }
            value = self.ready.wait(value).expect("cache token lock poisoned");
        }
    }

    /// 非阻塞尝试取出远程句柄；已被占用时返回 `None`。
    pub fn try_take(&self) -> Option<R> {
        self.value.lock().expect("cache token lock poisoned").take()
    }

    /// 归还远程句柄并唤醒一个等待者；重复归还会断言失败。
    pub fn put(&self, handle: R) {
        let mut value = self.value.lock().expect("cache token lock poisoned");
        assert!(value.is_none(), "cache remote token returned twice");
        *value = Some(handle);
        self.ready.notify_one();
    }
}

/// 单张缓存表的本地状态：表 ID、缓存快照、累计字节数与远程令牌。
pub struct CachedTable<R> {
    /// 表的物理/逻辑 ID，用于远程锁与租约调用。
    table_id: i64,
    /// 当前缓存数据；`None` 表示尚未安装缓存。
    cache_data: RwLock<Option<Arc<CacheData>>>,
    /// 当前 mem buffer 占用的近似字节数。
    total_size: AtomicI64,
    /// 串行化访问的远程状态句柄。
    remote: TokenLimit<R>,
}

impl<R> CachedTable<R> {
    /// 构造尚未加载数据的缓存表包装。
    pub fn new(table_id: i64, remote: R) -> Self {
        Self {
            table_id,
            cache_data: RwLock::new(None),
            total_size: AtomicI64::new(0),
            remote: TokenLimit::new(remote),
        }
    }

    /// 安装一份缓存快照，并按键值长度之和更新 `total_size`。
    pub fn install_cache(&self, data: CacheData) {
        // 近似估算占用：各键值字节长度之和。
        let size = data.mem_buffer.as_ref().map_or(0, |buffer| {
            buffer
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>() as i64
        });
        self.total_size.store(size, Ordering::Release);
        *self.cache_data.write().expect("cache data lock poisoned") = Some(Arc::new(data));
    }

    /// 返回当前缓存占用的近似字节数。
    pub fn total_size(&self) -> i64 {
        self.total_size.load(Ordering::Acquire)
    }

    /// Returns `(buffer, loading, should_renew)`. The last flag is the exact Go
    /// half-lease trigger and lets the caller schedule renewal asynchronously.
    ///
    /// 尝试在租约窗口内读取缓存，返回 `(缓冲, 是否加载中, 是否应续租)`。
    /// 最后一项对应 Go 的半租约触发条件，供调用方异步续期。
    pub fn try_read_from_cache(
        &self,
        timestamp: u64,
        lease_duration: Duration,
    ) -> (Option<Arc<MemBuffer>>, bool, bool) {
        let data = self
            .cache_data
            .read()
            .expect("cache data lock poisoned")
            .clone();
        let Some(data) = data else {
            return (None, false, false);
        };
        // 时间戳落在 [start, lease) 之外则缓存不可用。
        if timestamp < data.start || timestamp >= data.lease {
            return (None, false, false);
        }
        // 剩余租约不超过一半时长时提示调用方续期。
        let half_lease = u64::try_from((lease_duration / 2).as_millis())
            .unwrap_or(u64::MAX)
            .saturating_mul(1_u64 << LOGICAL_BITS);
        let should_renew = data.lease.saturating_sub(timestamp) <= half_lease;
        (
            data.mem_buffer.clone(),
            data.mem_buffer.is_none(),
            should_renew,
        )
    }

    /// 判断当前已加载缓存是否仍允许执行一次写变更。
    ///
    /// 与 Go 的 `AddRecord`/`UpdateRecord` 一致，本次写入可以使表跨过上限；
    /// 超限状态会在缓存重新加载并更新 `total_size` 后阻止后续写入。
    pub fn can_apply_mutation(&self, _size_delta: i64) -> bool {
        self.total_size() <= CACHED_TABLE_SIZE_LIMIT
    }
}

impl<R: StateRemote> CachedTable<R> {
    /// 向远程申请读锁/读租约；通过令牌通道串行化远程调用。
    pub fn update_lock_for_read(
        &self,
        timestamp: u64,
        duration: Duration,
    ) -> Result<bool, R::Error> {
        let mut remote = self.remote.take();
        let result = remote.lock_for_read(self.table_id, lease_from_ts(timestamp, duration));
        self.remote.put(remote);
        result
    }

    /// 用当前缓存中的旧租约向远程续期读租约。
    pub fn renew_lease(&self, timestamp: u64, duration: Duration) -> Result<u64, R::Error> {
        let data = self
            .cache_data
            .read()
            .expect("cache data lock poisoned")
            .clone();
        let old_lease = data.as_ref().map_or(0, |data| data.lease);
        let mut remote = self.remote.take();
        let result =
            remote.renew_read_lease(self.table_id, old_lease, lease_from_ts(timestamp, duration));
        self.remote.put(remote);
        if let (Ok(new_lease), Some(data)) = (&result, data)
            && *new_lease > 0
        {
            *self.cache_data.write().expect("cache data lock poisoned") =
                Some(Arc::new(CacheData {
                    start: data.start,
                    lease: *new_lease,
                    mem_buffer: data.mem_buffer.clone(),
                }));
        }
        result
    }

    /// 向远程申请写锁，使用默认写租约时长。
    pub fn lock_for_write(&self) -> Result<u64, R::Error> {
        let mut remote = self.remote.take();
        let result = remote.lock_for_write(self.table_id, CACHE_TABLE_WRITE_LEASE);
        self.remote.put(remote);
        result
    }
}
