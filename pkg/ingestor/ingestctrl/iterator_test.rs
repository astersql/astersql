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

// 迭代器与键适配器相关单元测试。
//
// 覆盖重复键检测迭代器 `DupDetectIter`、重复结果库迭代器 `DupDBIter`，
// 以及 `DupDetectKeyAdapter` 的编解码往返；验证唯一键计数、重复值收集与大规模扫描。

// 主要类型、函数、子用例、断言、资源收尾、并发/channel、failpoint、IO 和 mock 语义均在对应位置补充中文说明，方便人工继续迁移。

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

use std::sync::{Arc, Mutex};

use crate::KvPair;
use crate::iterator::{
    DupDBIter, DupDetectIter, DupDetectKeyAdapter, IngestLocalEngineIter, Iter, KeyAdapter,
};
// randBytes 对应 Go 函数/方法声明。
// Go: func randBytes(n int) []byte
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
/// 确定性伪随机字节（由 seed 派生），便于测试可复现。
pub fn rand_bytes(n: usize, seed: usize) -> Vec<u8> {
    (0..n)
        .map(|index| ((seed * 131 + index * 17) & 0xff) as u8)
        .collect()
}

#[test]
// TestDupDetectIterator 对应 Go 函数/方法声明。
// Go: func TestDupDetectIterator(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证重复键检测与唯一键计数。
pub fn test_dup_detect_iterator() {
    let adapter: Arc<dyn KeyAdapter> = Arc::new(DupDetectKeyAdapter);
    let duplicates = Arc::new(Mutex::new(Vec::new()));
    let mut source = Vec::new();
    let mut expected_duplicate_values = Vec::new();
    for index in 0..20 {
        source.push(KvPair {
            key: adapter.Encode(&rand_bytes(32, index), index as i64),
            value: rand_bytes(128, index),
        });
    }
    for index in 20..40 {
        let key = rand_bytes(32, index);
        for row in 0..2 {
            let value = rand_bytes(128, index * 3 + row);
            expected_duplicate_values.push(value.clone());
            source.push(KvPair {
                key: adapter.Encode(&key, (index * 3 + row) as i64),
                value,
            });
        }
    }
    for index in 40..50 {
        let key = rand_bytes(32, index);
        for row in 0..3 {
            let value = rand_bytes(128, index * 4 + row);
            expected_duplicate_values.push(value.clone());
            source.push(KvPair {
                key: adapter.Encode(&key, (index * 4 + row) as i64),
                value,
            });
        }
    }
    source.reverse();
    let mut iter = DupDetectIter::new(
        source,
        Arc::clone(&adapter),
        Arc::clone(&duplicates),
        b"",
        b"",
    );
    let mut iterated_keys = Vec::new();
    if iter.First() {
        loop {
            iterated_keys.push(iter.Key().to_vec());
            if !iter.Next() {
                break;
            }
        }
    }
    assert!(iter.Error().is_none());
    assert_eq!(50, iterated_keys.len());
    assert!(iterated_keys.windows(2).all(|keys| keys[0] < keys[1]));
    iter.Close().unwrap();
    let detected = duplicates.lock().unwrap().clone();
    assert_eq!(expected_duplicate_values.len(), detected.len());
    let mut detected_values: Vec<_> = detected.iter().map(|pair| pair.value.clone()).collect();
    detected_values.sort();
    expected_duplicate_values.sort();
    assert_eq!(expected_duplicate_values, detected_values);
    let encoded: Vec<_> = detected
        .iter()
        .map(|pair| adapter.Encode(&pair.key, 0))
        .collect();
    let mut duplicate_iter = DupDBIter::new(
        encoded
            .into_iter()
            .zip(detected.iter())
            .map(|(key, pair)| KvPair {
                key,
                value: pair.value.clone(),
            })
            .collect(),
        adapter,
        b"",
        b"",
    );
    let mut decoded_duplicates = Vec::new();
    if duplicate_iter.First() {
        loop {
            decoded_duplicates.push(KvPair {
                key: duplicate_iter.Key().to_vec(),
                value: duplicate_iter.Value().to_vec(),
            });
            if !duplicate_iter.Next() {
                break;
            }
        }
    }
    assert!(duplicate_iter.Error().is_none());
    assert_eq!(detected.len(), decoded_duplicates.len());
    assert!(
        decoded_duplicates
            .iter()
            .zip(detected.iter())
            .all(|(decoded, original)| decoded == original)
    );
    duplicate_iter.Close().unwrap();
    // 流程: pairs := make([]common.KvPair, 0, 20)
    // 流程: prevRowMax := int64(0)
    // 原注释: // Unique pairs.
    // 控制流: for range 20 {
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: randBytes(32),
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: }
    // 原注释: // Duplicate pairs which repeat the same key twice.
    // 控制流: for i := 20; i < 40; i++ {
    // 流程: key := randBytes(32)
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: key,
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: key,
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: }
    // 原注释: // Duplicate pairs which repeat the same key three times.
    // 控制流: for i := 40; i < 50; i++ {
    // 流程: key := randBytes(32)
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: key,
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: key,
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: pairs = append(pairs, common.KvPair{
    // 流程: Key: key,
    // 流程: Val: randBytes(128),
    // 流程: RowID: common.EncodeIntRowID(prevRowMax),
    // 流程: })
    // 流程: prevRowMax++
    // 流程: }
    // 原注释: // Find duplicates from the generated pairs.
    // 流程: var dupPairs []common.KvPair
    // 流程: sort.Slice(pairs, func(i, j int) bool {
    // 返回语义: return bytes.Compare(pairs[i].Key, pairs[j].Key) < 0
    // 流程: })
    // 流程: uniqueKeys := make([][]byte, 0)
    // 控制流: for i := 0; i < len(pairs); {
    // 流程: j := i + 1
    // 控制流: for j < len(pairs) && bytes.Equal(pairs[j-1].Key, pairs[j].Key) {
    // 流程: j++
    // 流程: }
    // 流程: uniqueKeys = append(uniqueKeys, pairs[i].Key)
    // 控制流: if i+1 == j {
    // 流程: i++
    // 流程: continue
    // 流程: }
    // 控制流: for k := i; k < j; k++ {
    // 流程: dupPairs = append(dupPairs, pairs[k])
    // 流程: }
    // 流程: i = j
    // 流程: }
    // 流程: keyAdapter := common.DupDetectKeyAdapter{}
    // 原注释: // Write pairs to db after shuffling the pairs.
    // 流程: rnd := rand.New(rand.NewSource(time.Now().UnixNano()))
    // 流程: rnd.Shuffle(len(pairs), func(i, j int) {
    // 流程: pairs[i], pairs[j] = pairs[j], pairs[i]
    // 流程: })
    // 流程: storeDir := t.TempDir()
    // Pebble/SST IO: db, err := pebble.Open(filepath.Join(storeDir, "kv"), &pebble.Options{})
    // 断言: require.NoError(t, err)
    // Pebble/SST IO: wb := db.NewBatch()
    // 控制流: for _, p := range pairs {
    // 流程: key := keyAdapter.Encode(nil, p.Key, p.RowID)
    // 断言: require.NoError(t, wb.Set(key, p.Val, nil))
    // 流程: }
    // 断言: require.NoError(t, wb.Commit(pebble.Sync))
    // Pebble/SST IO: dupDB, err := pebble.Open(filepath.Join(storeDir, "duplicates"), &pebble.Options{})
    // 断言: require.NoError(t, err)
    // 流程: pool := membuf.NewPool()
    // 资源收尾: defer pool.Destroy()
    // Pebble/SST IO: iter := newDupDetectIter(db, keyAdapter, &pebble.IterOptions{}, dupDB, log.L(), common.DupDetectOpt{}, pool.NewBuffer())
    // 流程: sort.Slice(pairs, func(i, j int) bool {
    // 流程: key1 := keyAdapter.Encode(nil, pairs[i].Key, pairs[i].RowID)
    // 流程: key2 := keyAdapter.Encode(nil, pairs[j].Key, pairs[j].RowID)
    // 返回语义: return bytes.Compare(key1, key2) < 0
    // 流程: })
    // 原注释: // Verify first pair.
    // 断言: require.True(t, iter.First())
    // 断言: require.True(t, iter.Valid())
    // 断言: require.Equal(t, pairs[0].Key, iter.Key())
    // 断言: require.Equal(t, pairs[0].Val, iter.Value())
    // 原注释: // Verify last pair.
    // 断言: require.True(t, iter.Last())
    // 断言: require.True(t, iter.Valid())
    // 断言: require.Equal(t, pairs[len(pairs)-1].Key, iter.Key())
    // 断言: require.Equal(t, pairs[len(pairs)-1].Val, iter.Value())
    // 原注释: // Iterate all keys and check the count of unique keys.
    // 控制流: for iter.First(); iter.Valid(); iter.Next() {
    // 断言: require.Equal(t, uniqueKeys[0], iter.Key())
    // 流程: uniqueKeys = uniqueKeys[1:]
    // 流程: }
    // 断言: require.NoError(t, iter.Error())
    // 断言: require.Equal(t, 0, len(uniqueKeys))
    // 断言: require.NoError(t, iter.Close())
    // 断言: require.NoError(t, db.Close())
    // 原注释: // Check duplicates detected by dupDetectIter.
    // Pebble/SST IO: iter2 := newDupDBIter(dupDB, keyAdapter, &pebble.IterOptions{})
    // 流程: var detectedPairs []common.KvPair
    // 控制流: for iter2.First(); iter2.Valid(); iter2.Next() {
    // 流程: detectedPairs = append(detectedPairs, common.KvPair{
    // 流程: Key: slices.Clone(iter2.Key()),
    // 流程: Val: slices.Clone(iter2.Value()),
    // 流程: })
    // 流程: }
    // 断言: require.NoError(t, iter2.Error())
    // 断言: require.NoError(t, iter2.Close())
    // 断言: require.NoError(t, dupDB.Close())
    // 断言: require.Equal(t, len(dupPairs), len(detectedPairs))
    // 流程: sort.Slice(dupPairs, func(i, j int) bool {
    // 流程: keyCmp := bytes.Compare(dupPairs[i].Key, dupPairs[j].Key)
    // 返回语义: return keyCmp < 0 || keyCmp == 0 && bytes.Compare(dupPairs[i].Val, dupPairs[j].Val) < 0
    // 流程: })
    // 流程: sort.Slice(detectedPairs, func(i, j int) bool {
    // 流程: keyCmp := bytes.Compare(detectedPairs[i].Key, detectedPairs[j].Key)
    // 返回语义: return keyCmp < 0 || keyCmp == 0 && bytes.Compare(detectedPairs[i].Val, detectedPairs[j].Val) < 0
    // 流程: })
    // 控制流: for i := range detectedPairs {
    // 断言: require.Equal(t, dupPairs[i].Key, detectedPairs[i].Key)
    // 断言: require.Equal(t, dupPairs[i].Val, detectedPairs[i].Val)
    // 流程: }
}

#[test]
// TestKeyAdapterEncoding 对应 Go 函数/方法声明。
// Go: func TestKeyAdapterEncoding(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证 DupDetectKeyAdapter 编解码往返与非法键拒绝。
pub fn test_key_adapter_encoding() {
    let adapter = DupDetectKeyAdapter;
    for source in [&[1, 2, 3][..], b"a", b"a\0", b"12345678", b""] {
        for row_id in [1, i64::MIN, i64::MAX] {
            let encoded = adapter.Encode(source, row_id);
            assert_eq!(source, adapter.Decode(&encoded).unwrap().as_slice());
        }
    }
    assert!(adapter.Encode(b"a", i64::MAX) < adapter.Encode(b"a\0", i64::MIN));
    assert!(adapter.Encode(b"k", -1) < adapter.Encode(b"k", 0));
    assert!(adapter.Decode(&[1, 2, 3]).is_err());
    // 流程: keyAdapter := common.DupDetectKeyAdapter{}
    // 流程: srcKey := []byte{1, 2, 3}
    // 流程: v := keyAdapter.Encode(nil, srcKey, common.EncodeIntRowID(1))
    // 流程: resKey, err := keyAdapter.Decode(nil, v)
    // 断言: require.NoError(t, err)
    // 断言: require.EqualValues(t, srcKey, resKey)
    // mock: v = keyAdapter.Encode(nil, srcKey, []byte("mock_common_handle"))
    // 流程: resKey, err = keyAdapter.Decode(nil, v)
    // 断言: require.NoError(t, err)
    // 断言: require.EqualValues(t, srcKey, resKey)
}

// BenchmarkDupDetectIter 对应 Go 函数/方法声明。
// Go: func BenchmarkDupDetectIter(b *testing.B)
// 这是 benchmark ：保留数据规模、循环和资源生命周期，不接入 Rust benchmark harness。
#[test]
/// 大规模重复检测迭代基准（约 20% 重复）。
pub fn benchmark_dup_detect_iter() {
    let adapter: Arc<dyn KeyAdapter> = Arc::new(DupDetectKeyAdapter);
    let pairs = (0..100_000)
        .map(|index| {
            let key_number = if index % 5 == 0 {
                index - usize::from(index > 0)
            } else {
                index
            };
            KvPair {
                key: adapter.Encode(format!("{key_number:09}").as_bytes(), index as i64),
                value: b"value".to_vec(),
            }
        })
        .collect();
    let duplicates = Arc::new(Mutex::new(Vec::new()));
    let mut iter = DupDetectIter::new(pairs, adapter, Arc::clone(&duplicates), b"", b"");
    let mut count = 0;
    if iter.First() {
        loop {
            count += 1;
            if !iter.Next() {
                break;
            }
        }
    }
    assert_eq!(80_001, count);
    assert_eq!(39_998, duplicates.lock().unwrap().len());
    iter.Close().unwrap();
    // 流程: keyAdapter := common.DupDetectKeyAdapter{}
    // Pebble/SST IO: db, _ := pebble.Open(filepath.Join(b.TempDir(), "kv"), &pebble.Options{})
    // Pebble/SST IO: wb := db.NewBatch()
    // 流程: val := []byte("value")
    // 控制流: for i := range 100_000 {
    // 流程: keyNum := i
    // 原注释: // mimic we have 20% duplication
    // 控制流: if keyNum%5 == 0 {
    // 流程: keyNum--
    // 流程: }
    // 流程: keyStr := fmt.Sprintf("%09d", keyNum)
    // 流程: rowID := strconv.Itoa(i)
    // 流程: key := keyAdapter.Encode(nil, []byte(keyStr), []byte(rowID))
    // 流程: wb.Set(key, val, nil)
    // 流程: }
    // Pebble/SST IO: wb.Commit(pebble.Sync)
    // 流程: pool := membuf.NewPool()
    // Pebble/SST IO: dupDB, _ := pebble.Open(filepath.Join(b.TempDir(), "dup"), &pebble.Options{})
    // 流程: b.ResetTimer()
    // 控制流: for i := 0; i < b.N; i++ {
    // 流程: iter := newDupDetectIter(
    // 流程: db,
    // 流程: keyAdapter,
    // Pebble/SST IO: &pebble.IterOptions{},
    // 流程: dupDB,
    // 流程: log.L(),
    // 流程: common.DupDetectOpt{},
    // 流程: pool.NewBuffer(),
    // 流程: )
    // 流程: keyCnt := 0
    // 控制流: for iter.First(); iter.Valid(); iter.Next() {
    // 流程: keyCnt++
    // 流程: }
    // 资源收尾: iter.Close()
    // 流程: }
}
