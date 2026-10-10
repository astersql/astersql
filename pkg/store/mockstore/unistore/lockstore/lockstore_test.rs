// Copyright 2019-present PingCAP, Inc.
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

// lockstore MemStore 单元与并发测试。
//
// 覆盖插入/查询/删除、值替换、迭代器定位，以及单写多读并发下的数据完整性；
// 同时保留对应 Go benchmark 工作负载的编译期桩函数。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
use crate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use rand::seq::SliceRandom;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, RwLock};
    use std::thread;
    use std::time::Duration;

    /// 测试键前缀。
    const KEY_PREFIX: &str = "ls";
    /// 大批量插入/校验的键数量。
    const TEST_KEY_COUNT: usize = 30_000;

    /// 将序号格式化为固定宽度键。
    fn num_to_key_with_prefix(prefix: &str, n: usize) -> Vec<u8> {
        format!("{prefix}{n:020}").into_bytes()
    }

    /// 使用默认前缀生成键。
    fn numToKey(n: usize) -> Vec<u8> {
        num_to_key_with_prefix(KEY_PREFIX, n)
    }

    /// 乱序插入 n 个键，值为 val_prefix+key。
    fn insertMemStore<'a>(
        ls: &'a mut MemStore,
        prefix: &str,
        val_prefix: &str,
        n: usize,
    ) -> &'a mut MemStore {
        let mut permutation: Vec<_> = (0..n).collect();
        permutation.shuffle(&mut rand::thread_rng());
        let mut hint = Hint::new();
        for value in permutation {
            let key = num_to_key_with_prefix(prefix, value);
            let mut stored = val_prefix.as_bytes().to_vec();
            stored.extend_from_slice(&key);
            ls.PutWithHint(&key, &stored, Some(&mut hint));
        }
        ls
    }

    /// 乱序校验 n 个键的值内容。
    fn checkMemStore(ls: &MemStore, prefix: &str, val_prefix: &str, n: usize) {
        let mut permutation: Vec<_> = (0..n).collect();
        permutation.shuffle(&mut rand::thread_rng());
        let mut buf = Vec::new();
        for value in permutation {
            let key = num_to_key_with_prefix(prefix, value);
            let stored = ls.Get(&key, &mut buf).expect("inserted key must exist");
            assert_eq!(&stored[..val_prefix.len()], val_prefix.as_bytes());
            assert_eq!(&stored[val_prefix.len()..], key);
        }
    }

    /// 乱序删除 n 个键。
    fn deleteMemStore(ls: &mut MemStore, prefix: &str, n: usize) {
        let mut permutation: Vec<_> = (0..n).collect();
        permutation.shuffle(&mut rand::thread_rng());
        for value in permutation {
            assert!(ls.Delete(&num_to_key_with_prefix(prefix, value)));
        }
    }

    #[test]
    /// 覆盖插入、查询、删除及延迟复用后块数稳定性。
    fn test_mem_store() {
        let mut ls = MemStore::NewMemStore(1 << 10);
        let mut buf = Vec::new();
        assert_eq!(ls.Get(b"a", &mut buf), None);

        insertMemStore(&mut ls, KEY_PREFIX, "", TEST_KEY_COUNT);
        let num_blocks = ls.getArena().blocks.len();
        checkMemStore(&ls, KEY_PREFIX, "", TEST_KEY_COUNT);
        deleteMemStore(&mut ls, KEY_PREFIX, TEST_KEY_COUNT);
        assert_eq!(ls.getArena().blocks.len(), num_blocks);

        // 等待 reuseSafeDuration 后再插入，验证块延迟复用效果。
        thread::sleep(reuseSafeDuration);
        insertMemStore(&mut ls, KEY_PREFIX, "", TEST_KEY_COUNT);
        let diff = ls.getArena().blocks.len().abs_diff(num_blocks);
        assert!(
            diff < num_blocks / 100,
            "block difference {diff}/{num_blocks}"
        );
        assert_eq!(ls.Get(&numToKey(TEST_KEY_COUNT), &mut buf), None);
        assert_eq!(ls.Get(b"abc", &mut buf), None);
    }

    // Go 以不稳定为由跳过本测试（#26235）；保留编译与 ignore 行为。
    // The Go source skips this exact test as unstable (#26235). Keep the body
    // compiled and preserve the same test-runner behavior.
    #[test]
    #[ignore = "Skip this unstable test(#26235) and bring it back before 2021-07-29."]
    /// 迭代器 Seek/Next/Prev 定位行为（Go 侧标为不稳定而 ignore）。
    fn test_iterator() {
        let mut ls = MemStore::NewMemStore(1 << 10);
        let mut hint = Hint::new();
        for i in (10..1000).step_by(10) {
            let key = numToKey(i);
            ls.PutWithHint(&key, &key.repeat(10), Some(&mut hint));
        }
        assert_eq!(ls.getArena().blocks.len(), 33);
        let mut it = ls.NewIterator();
        it.SeekToFirst();
        checkKey(&it, 10);
        it.Next();
        checkKey(&it, 20);
        it.SeekToFirst();
        checkKey(&it, 10);
        it.SeekToLast();
        checkKey(&it, 990);
        it.Seek(&numToKey(11));
        checkKey(&it, 20);
        it.Seek(&numToKey(989));
        checkKey(&it, 990);
        it.Seek(&numToKey(0));
        checkKey(&it, 10);
        it.Seek(&numToKey(2000));
        assert!(!it.Valid());
        it.Seek(&numToKey(500));
        checkKey(&it, 500);
        it.Prev();
        checkKey(&it, 490);
        it.SeekForPrev(&numToKey(100));
        checkKey(&it, 100);
        it.SeekForPrev(&numToKey(99));
        checkKey(&it, 90);
        it.SeekForPrev(&numToKey(2000));
        checkKey(&it, 990);
    }

    /// 断言迭代器当前键值对应序号 n。
    fn checkKey(it: &iterator::Iterator<'_>, n: usize) {
        assert!(it.Valid());
        assert_eq!(it.Key(), numToKey(n));
        assert_eq!(it.Value(), it.Key().repeat(10));
    }

    #[test]
    /// 同键二次插入应覆盖旧值。
    fn test_replace() {
        let mut ls = MemStore::NewMemStore(1 << 10);
        insertMemStore(&mut ls, KEY_PREFIX, "old", TEST_KEY_COUNT);
        checkMemStore(&ls, KEY_PREFIX, "old", TEST_KEY_COUNT);
        insertMemStore(&mut ls, KEY_PREFIX, "new", TEST_KEY_COUNT);
        checkMemStore(&ls, KEY_PREFIX, "new", TEST_KEY_COUNT);
    }

    /// 并发读者：反复 Get 指定键直到 stop。
    fn runReader(
        ls: Arc<RwLock<Box<MemStore>>>,
        stop: Arc<AtomicBool>,
        started: Arc<Barrier>,
        i: usize,
    ) -> usize {
        let key = numToKey(i);
        let mut reads = 0;
        let mut buf = Vec::with_capacity(100);
        started.wait();
        while !stop.load(Ordering::Relaxed) {
            reads += 1;
            let result = ls.read().unwrap().Get(&key, &mut buf);
            if let Some(result) = result {
                assert_eq!(result, key, "data corruption");
            }
        }
        reads
    }

    #[test]
    /// 单写多读并发：写端 Put/Delete，读端校验无损坏。
    fn test_mem_store_concurrent() {
        const KEY_RANGE: usize = 10;
        const WRITE_ROUNDS: usize = 50_000;
        let concurrent_keys: Vec<_> = (0..KEY_RANGE).map(numToKey).collect();
        let ls = Arc::new(RwLock::new(MemStore::NewMemStore(1 << 20)));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(Barrier::new(KEY_RANGE + 1));
        let readers: Vec<_> = (0..KEY_RANGE)
            .map(|i| {
                let ls = Arc::clone(&ls);
                let stop = Arc::clone(&stop);
                let started = Arc::clone(&started);
                thread::spawn(move || runReader(ls, stop, started, i))
            })
            .collect();

        let mut rng = rand::thread_rng();
        let mut total_insert = 0;
        let mut total_delete = 0;
        let mut hint = Hint::new();
        started.wait();
        for _ in 0..WRITE_ROUNDS {
            let key = &concurrent_keys[rng.gen_range(0..KEY_RANGE)];
            if ls.write().unwrap().PutWithHint(key, key, Some(&mut hint)) {
                total_insert += 1;
            }
            let key = &concurrent_keys[rng.gen_range(0..KEY_RANGE)];
            if ls.write().unwrap().DeleteWithHint(key, Some(&mut hint)) {
                total_delete += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        let read_count: usize = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(read_count > 0);
        assert!(total_insert > 0);
        assert!(total_delete > 0);
    }

    // Stable Rust has no built-in benchmark harness. These functions retain
    // each Go benchmark workload and are compiled as part of this test crate.
    #[allow(dead_code)]
    /// 对应 Go benchmark：Delete+Put+Get 循环。
    fn benchmark_mem_store_delete_insert_get(iterations: usize) {
        let mut ls = MemStore::NewMemStore(1 << 23);
        let keys: Vec<_> = (0..10_000).map(numToKey).collect();
        for key in &keys {
            ls.Put(key, key);
        }
        let mut rng = rand::thread_rng();
        let mut buf = Vec::with_capacity(100);
        for _ in 0..iterations {
            let key = &keys[rng.gen_range(0..keys.len())];
            ls.Delete(key);
            ls.Put(key, key);
            ls.Get(key, &mut buf);
        }
    }

    #[allow(dead_code)]
    /// 对应 Go benchmark：全表迭代。
    fn benchmark_mem_store_iterate(iterations: usize) {
        let mut ls = MemStore::NewMemStore(1 << 23);
        let keys: Vec<_> = (0..10_000).map(numToKey).collect();
        for key in &keys {
            ls.Put(key, key);
        }
        for _ in 0..iterations {
            let mut it = ls.NewIterator();
            it.SeekToFirst();
            while it.Valid() {
                it.Next();
            }
        }
    }

    #[allow(dead_code)]
    /// 对应 Go benchmark：带 hint 的 Put。
    fn benchmark_put_with_hint(iterations: usize) {
        let mut ls = MemStore::NewMemStore(1 << 20);
        let keys: Vec<_> = (0..100_000).map(numToKey).collect();
        let mut hint = Hint::new();
        for i in 0..iterations {
            let key = &keys[i % keys.len()];
            ls.PutWithHint(key, key, Some(&mut hint));
        }
    }

    #[allow(dead_code)]
    /// 对应 Go benchmark：无 hint 的 Put。
    fn benchmark_put(iterations: usize) {
        let mut ls = MemStore::NewMemStore(1 << 20);
        let keys: Vec<_> = (0..100_000).map(numToKey).collect();
        for i in 0..iterations {
            let key = &keys[i % keys.len()];
            ls.Put(key, key);
        }
    }
}
