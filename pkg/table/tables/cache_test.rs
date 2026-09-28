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

// 缓存表核心状态机的单元测试。
//
// 这里用可记录调用的远端状态桩，验证租约时间戳换算、缓存可读窗口、容量统计，
// 以及读写锁请求的参数和串行令牌回收；不涉及 SQL 层的缓存表集成流程。

use crate::cache::{
    CACHE_TABLE_WRITE_LEASE, CACHED_TABLE_SIZE_LIMIT, CacheData, CachedTable, MemBuffer,
    StateRemote, TokenLimit, lease_from_ts,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn lease_from_ts_matches_oracle_physical_time_conversion() {
    const LOGICAL_BITS: u32 = 18;
    let timestamp = (1_000_u64 << LOGICAL_BITS) | 123;

    assert_eq!(
        lease_from_ts(timestamp, Duration::from_millis(25)),
        1_025_u64 << LOGICAL_BITS,
        "oracle.GetTimeFromTS followed by oracle.GoTimeToTS discards logical bits"
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 记录缓存表向远端状态存储发出的锁与续租请求，便于核对参数契约。
enum Call {
    Read(i64, u64),
    RenewRead(i64, u64, u64),
    Write(i64, Duration),
    RenewWrite(i64, u64),
}

#[derive(Clone)]
/// 可配置返回值的远端状态桩；每次调用都会先写入共享调用日志。
struct MockRemote {
    calls: Arc<Mutex<Vec<Call>>>,
    read_result: Result<bool, &'static str>,
    renew_result: Result<u64, &'static str>,
    write_result: Result<u64, &'static str>,
}

impl StateRemote for MockRemote {
    type Error = &'static str;

    fn lock_for_read(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error> {
        self.calls.lock().unwrap().push(Call::Read(table_id, lease));
        self.read_result
    }

    fn renew_read_lease(
        &mut self,
        table_id: i64,
        old_lease: u64,
        new_lease: u64,
    ) -> Result<u64, Self::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::RenewRead(table_id, old_lease, new_lease));
        self.renew_result
    }

    fn lock_for_write(
        &mut self,
        table_id: i64,
        lease_duration: Duration,
    ) -> Result<u64, Self::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Write(table_id, lease_duration));
        self.write_result
    }

    fn renew_write_lease(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::RenewWrite(table_id, lease));
        Ok(true)
    }
}

fn remote() -> (MockRemote, Arc<Mutex<Vec<Call>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        MockRemote {
            calls: calls.clone(),
            read_result: Ok(true),
            renew_result: Ok(99),
            write_result: Ok(123),
        },
        calls,
    )
}

#[test]
fn token_limit_matches_capacity_one_channel() {
    let token = TokenLimit::new(7);
    assert_eq!(token.try_take(), Some(7));
    assert_eq!(token.try_take(), None);
    token.put(9);
    assert_eq!(token.take(), 9);
}

#[test]
fn cache_window_reports_loading_and_half_lease_renewal() {
    let (remote, _) = remote();
    let table = CachedTable::new(5, remote);
    assert_eq!(
        table.try_read_from_cache(10, Duration::from_millis(10)),
        (None, false, false)
    );

    table.install_cache(CacheData {
        start: 100_u64 << 18,
        lease: 120_u64 << 18,
        mem_buffer: None,
    });
    // 依次覆盖租约开始前、有效期前半段、后半段和到期边界：只有有效期内可读，
    // 后半段还需提示调用方发起续租。
    assert_eq!(
        table.try_read_from_cache(99_u64 << 18, Duration::from_millis(20)),
        (None, false, false)
    );
    assert_eq!(
        table.try_read_from_cache(105_u64 << 18, Duration::from_millis(20)),
        (None, true, false)
    );
    assert_eq!(
        table.try_read_from_cache(110_u64 << 18, Duration::from_millis(20)),
        (None, true, true)
    );
    assert_eq!(
        table.try_read_from_cache(120_u64 << 18, Duration::from_millis(20)),
        (None, false, false)
    );
}

#[test]
fn installing_cache_updates_size_and_exposes_buffer() {
    let (remote, _) = remote();
    let table = CachedTable::new(5, remote);
    let buffer: MemBuffer = [
        (b"a".to_vec(), b"12".to_vec()),
        (b"bbb".to_vec(), b"4567".to_vec()),
    ]
    .into_iter()
    .collect();
    table.install_cache(CacheData {
        start: 10,
        lease: 20,
        mem_buffer: Some(Arc::new(buffer.clone())),
    });

    let (loaded, loading, renew) = table.try_read_from_cache(15, Duration::from_nanos(1));
    assert_eq!(&*loaded.unwrap(), &buffer);
    assert!(!loading);
    assert!(!renew);
    // 缓存大小同时计入键和值的字节数，并据此严格执行全局容量上限。
    assert_eq!(table.total_size(), 10);
    assert!(table.can_apply_mutation(CACHED_TABLE_SIZE_LIMIT));
    assert!(
        table.can_apply_mutation(CACHED_TABLE_SIZE_LIMIT + 1),
        "Go checks the already-loaded size before AddRecord/UpdateRecord and allows the current mutation to cross the limit"
    );
}

#[test]
fn remote_calls_use_table_id_old_lease_and_go_durations() {
    let (remote, calls) = remote();
    let table = CachedTable::new(42, remote);
    table.install_cache(CacheData {
        start: 10,
        lease: 77,
        mem_buffer: None,
    });

    assert_eq!(
        table.update_lock_for_read(1_000_u64 << 18, Duration::from_millis(25)),
        Ok(true)
    );
    assert_eq!(
        table.renew_lease(2_000_u64 << 18, Duration::from_millis(30)),
        Ok(99)
    );
    assert_eq!(table.lock_for_write(), Ok(123));

    // 读锁和续租把物理时长换算进 TSO，续租还必须携带当前旧租约；
    // 写锁则直接沿用与 Go 实现一致的固定租约时长。
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            Call::Read(42, 1_025_u64 << 18),
            Call::RenewRead(42, 77, 2_030_u64 << 18),
            Call::Write(42, CACHE_TABLE_WRITE_LEASE),
        ]
    );
}

#[test]
fn remote_error_does_not_lose_the_serialization_token() {
    let (mut remote, calls) = remote();
    remote.read_result = Err("read failed");
    let table = CachedTable::new(8, remote);

    assert_eq!(
        table.update_lock_for_read(0, Duration::from_secs(1)),
        Err("read failed")
    );
    // 第二次请求仍能到达远端，证明首次错误返回时串行令牌已归还，而非永久阻塞后续请求。
    assert_eq!(
        table.update_lock_for_read(0, Duration::from_secs(1)),
        Err("read failed")
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn renew_lease_updates_the_cached_snapshot_only_for_a_positive_lease() {
    let (remote, _) = remote();
    let table = CachedTable::new(42, remote);
    let buffer: MemBuffer = [(b"key".to_vec(), b"value".to_vec())].into_iter().collect();
    table.install_cache(CacheData {
        start: 10,
        lease: 77,
        mem_buffer: Some(Arc::new(buffer.clone())),
    });

    assert_eq!(table.renew_lease(0, Duration::from_millis(1)), Ok(99));
    let (loaded, loading, _) = table.try_read_from_cache(80, Duration::ZERO);
    assert_eq!(
        &*loaded.expect("renewed cache should remain readable"),
        &buffer
    );
    assert!(!loading);
}
