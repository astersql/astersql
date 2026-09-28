// Copyright 2023 PingCAP, Inc.
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

// ExternalSorter 公共测试辅助。
//
// 提供随机 KV 生成、单线程与多 Writer 并行写入后校验有序去重结果的例程，
// 供磁盘排序等具体实现复用。

use crate::external_sorter::ExternalSorter;
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};
use std::sync::{Arc, Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 测试用键值对。
pub(crate) struct TestKeyValue {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Vec<u8>,
}

/// 生成 `n` 条随机 KV；key 末 4 字节写入序号以保证唯一。
pub(crate) fn gen_random_kvs(
    rng: &mut StdRng,
    n: usize,
    key_size_range: usize,
    value_size_range: usize,
) -> Vec<TestKeyValue> {
    (0..n)
        .map(|index| {
            // 前缀随机，末尾序号保证 key 唯一且可复现。
            let key_size = rng.gen_range(4..key_size_range);
            let mut key = vec![0; key_size];
            let mut value = vec![0; rng.gen_range(0..value_size_range)];
            rng.fill_bytes(&mut key[..key_size - 4]);
            rng.fill_bytes(&mut value);
            key[key_size - 4..].copy_from_slice(&(index as u32).to_be_bytes());
            TestKeyValue { key, value }
        })
        .collect()
}

/// 单 Writer 写入、排序、迭代校验与期望排序结果一致。
pub(crate) fn run_common_test<S: ExternalSorter>(sorter: &S) {
    const NUM_KEYS: usize = 1000;
    let ctx = CancellationToken::new();
    let mut writer = sorter.new_writer(&ctx).unwrap();
    let mut rng = StdRng::seed_from_u64(0);
    // 排序前不应能创建迭代器。
    let mut kvs = gen_random_kvs(&mut rng, NUM_KEYS, 256, 1024);
    for kv in &kvs {
        writer.put(&kv.key, &kv.value).unwrap();
    }
    writer.close().unwrap();
    assert!(sorter.new_iterator(&ctx).is_err());
    sorter.sort(&ctx).unwrap();
    let mut iter = sorter.new_iterator(&ctx).unwrap();
    // 期望结果按 key 字典序。
    kvs.sort_by(|left, right| left.key.cmp(&right.key));
    let mut count = 0;
    if iter.first() {
        while iter.valid() {
            assert_eq!(kvs[count].key, iter.unsafe_key());
            assert_eq!(kvs[count].value, iter.unsafe_value());
            count += 1;
            iter.next();
        }
    }
    assert_eq!(kvs.len(), count);
    iter.close().unwrap();
}

/// 多 Writer 并发写入后排序，校验全局有序且条数正确。
pub(crate) fn run_common_parallel_test<S>(sorter: &S)
where
    S: ExternalSorter + Clone + Send + Sync,
{
    const NUM_KEYS: usize = 10_000;
    const NUM_WRITERS: usize = 10;
    let ctx = CancellationToken::new();
    let (sender, receiver) = mpsc::sync_channel::<TestKeyValue>(16);
    let receiver = Arc::new(Mutex::new(receiver));
    let mut rng = StdRng::seed_from_u64(0);
    let mut kvs = gen_random_kvs(&mut rng, NUM_KEYS, 256, 1024);

    // 通道分发 KV；发送端 drop 后各 Writer 退出并 close。
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..NUM_WRITERS {
            let receiver = Arc::clone(&receiver);
            let sorter = sorter.clone();
            let ctx = ctx.clone();
            handles.push(scope.spawn(move || {
                let mut writer = sorter.new_writer(&ctx).unwrap();
                loop {
                    let next = receiver.lock().unwrap().recv();
                    let Ok(kv) = next else { break };
                    writer.put(&kv.key, &kv.value).unwrap();
                }
                writer.close().unwrap();
            }));
        }
        for kv in kvs.iter().cloned() {
            sender.send(kv).unwrap();
        }
        drop(sender);
        for handle in handles {
            handle.join().unwrap();
        }
    });

    kvs.sort_by(|left, right| left.key.cmp(&right.key));
    sorter.sort(&ctx).unwrap();
    let mut iter = sorter.new_iterator(&ctx).unwrap();
    let mut count = 0;
    if iter.first() {
        while iter.valid() {
            assert_eq!(kvs[count].key, iter.unsafe_key());
            assert_eq!(kvs[count].value, iter.unsafe_value());
            count += 1;
            iter.next();
        }
    }
    assert_eq!(kvs.len(), count);
    iter.close().unwrap();
}
