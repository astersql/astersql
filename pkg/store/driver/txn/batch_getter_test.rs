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

// `BufferBatchGetter` 三层批量读取的单元测试与 mock 存储。
//
// 验证缓冲优先、中间缓存次之、快照兜底，以及删除墓碑（空值）不出现在结果中；
// 开启 `WithReturnCommitTSBatch` 时 commit_ts 取自命中层的基准时间戳。

use std::collections::HashMap;
use std::sync::Arc;

use crate::*;

/// 覆盖缓冲覆盖、删除墓碑、中间缓存与快照回落，以及可选返回 commit_ts。
#[test]
fn TestBufferBatchGetter() {
    // 快照层：a/b/c/d 均有原始值，commitTSBase=1000。
    let mut snap = newMockStore();
    snap.commitTSBase = 1000;
    let ka = b"a".to_vec();
    let kb = b"b".to_vec();
    let kc = b"c".to_vec();
    let kd = b"d".to_vec();
    snap.Set(ka.clone(), ka.clone()).unwrap();
    snap.Set(kb.clone(), kb.clone()).unwrap();
    snap.Set(kc.clone(), kc.clone()).unwrap();
    snap.Set(kd.clone(), kd.clone()).unwrap();

    // 中间缓存：覆盖 a、c。
    let mut middle = newMockStore();
    middle.commitTSBase = 2000;
    middle.Set(ka.clone(), b"a1".to_vec()).unwrap();
    middle.Set(kc.clone(), b"c1".to_vec()).unwrap();

    // 缓冲：覆盖 a，删除 b（空值墓碑）。
    let mut buffer = newMockStore();
    buffer.commitTSBase = 3000;
    buffer.Set(ka.clone(), b"a2".to_vec()).unwrap();
    buffer.Delete(kb.clone()).unwrap();

    let batchGetter = NewBufferBatchGetter(
        Arc::new(mockBufferBatchGetterStore { inner: buffer }),
        Some(Arc::new(middle)),
        Arc::new(snap),
    );
    // 无 commit_ts：a 来自缓冲，b 被墓碑抑制，c 来自中间缓存，d 来自快照。
    let result = batchGetter
        .BatchGet(&[ka.clone(), kb.clone(), kc.clone(), kd.clone()], &[])
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(
        result.get(&ka).unwrap(),
        &ValueEntry::new(b"a2".to_vec(), 0)
    );
    assert_eq!(
        result.get(&kc).unwrap(),
        &ValueEntry::new(b"c1".to_vec(), 0)
    );
    assert_eq!(result.get(&kd).unwrap(), &ValueEntry::new(b"d".to_vec(), 0));

    // 请求返回 commit_ts：各层 base + 键首字节，缺失键 "xx" 不出现。
    let result = batchGetter
        .BatchGet(
            &[
                ka.clone(),
                kb.clone(),
                kc.clone(),
                kd.clone(),
                b"xx".to_vec(),
            ],
            &[WithReturnCommitTSBatch()],
        )
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(
        result.get(&ka).unwrap(),
        &ValueEntry::new(b"a2".to_vec(), 3000 + b'a' as u64)
    );
    assert_eq!(
        result.get(&kc).unwrap(),
        &ValueEntry::new(b"c1".to_vec(), 2000 + b'c' as u64)
    );
    assert_eq!(
        result.get(&kd).unwrap(),
        &ValueEntry::new(b"d".to_vec(), 1000 + b'd' as u64)
    );
}

/// 简易内存 mock：并行的键列表与值列表，以及合成 commit_ts 的基准。
#[derive(Clone, Default)]
struct mockBatchGetterStore {
    /// 键索引列表（与 `value` 下标对齐）。
    index: Vec<Key>,
    /// 对应值列表；空切片表示删除。
    value: Vec<Vec<u8>>,
    /// 合成 commit_ts = base + key[0]（当要求返回时）。
    commitTSBase: u64,
}

/// 创建空的 mock 存储。
fn newMockStore() -> mockBatchGetterStore {
    mockBatchGetterStore::default()
}

impl mockBatchGetterStore {
    /// 当前键条目数。
    fn Len(&self) -> usize {
        self.index.len()
    }

    /// 写入或覆盖键值。
    fn Set(&mut self, k: Key, v: Vec<u8>) -> Result<(), DriverError> {
        for (i, key) in self.index.iter().enumerate() {
            if key == &k {
                self.value[i] = v;
                return Ok(());
            }
        }
        self.index.push(k);
        self.value.push(v);
        Ok(())
    }

    /// 删除：写入空值作为墓碑。
    fn Delete(&mut self, k: Key) -> Result<(), DriverError> {
        self.Set(k, Vec::new())
    }

    /// 点查；未命中返回 `NotFound`。
    fn lookup(&self, k: &[u8], return_commit_ts: bool) -> Result<ValueEntry, DriverError> {
        let commit_ts = if return_commit_ts {
            self.commitTSBase + u64::from(k[0])
        } else {
            0
        };
        for (i, key) in self.index.iter().enumerate() {
            if key.as_slice() == k {
                return Ok(ValueEntry::new(self.value[i].clone(), commit_ts));
            }
        }
        Err(DriverError::NotFound)
    }
}

impl Getter for mockBatchGetterStore {
    fn get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        self.lookup(key, wants_return_commit_ts_get(options))
    }
}

impl BatchGetter for mockBatchGetterStore {
    fn batch_get(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        let mut map = HashMap::new();
        let return_commit_ts = wants_return_commit_ts_batch(options);
        for key in keys {
            match self.lookup(key, return_commit_ts) {
                Ok(value) => {
                    map.insert(key.clone(), value);
                }
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(map)
    }
}

/// 将 `mockBatchGetterStore` 包装为 `BatchBufferGetter`。
struct mockBufferBatchGetterStore {
    inner: mockBatchGetterStore,
}

impl Getter for mockBufferBatchGetterStore {
    fn get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        self.inner.get(key, options)
    }
}

impl BatchBufferGetter for mockBufferBatchGetterStore {
    fn len(&self) -> usize {
        self.inner.Len()
    }

    fn batch_get_bytes(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        self.inner.batch_get(keys, options)
    }
}
