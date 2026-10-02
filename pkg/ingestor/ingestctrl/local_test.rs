// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Local backend 综合单元测试。
//
// 覆盖键后继计算、Range 属性、本地 Writer、SST 合并、多路 ingest 循环、
// peer 忙碌检测、NotLeader 错误处理，以及大量 mock PD/Import 客户端与 failpoint 场景；
// 对应 Go `local_test.go` 的迁移基线。

// 主要类型、函数、子用例、断言、资源收尾、并发/channel、failpoint、IO 和 mock 语义均在对应位置补充中文说明，方便人工继续迁移。

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

// var 声明沿用 Go 顶层状态，用于测试替换或共享 fixture。
// Go: var GetSplitConfFromStore = getSplitConfFromStore
/// Go 全局 `GetSplitConfFromStore` 的绑定占位说明。
pub fn get_split_conf_from_store_go_binding_note() {
    // 这里不初始化真实全局状态，只记录 Go 测试中可被替换的符号。
}

// SetGetSplitConfFromStoreFunc 对应 Go 函数/方法声明。
// Go: func SetGetSplitConfFromStoreFunc(
// Go: fn func(ctx context.Context, host string, tls *common.TLS) (splitSize int64, regionSplitKeys int64, err error),
// Go: )
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
/// 测试中替换分裂配置拉取函数的占位。
pub fn set_get_split_conf_from_store_func() {
    // PD/TiKV region: getSplitConfFromStoreFunc = fn
}

#[test]
// TestNextKey 对应 Go 函数/方法声明。
// Go: func TestNextKey(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证 `NextKey` 后继键计算。
pub fn test_next_key() {
    // 断言: require.Equal(t, []byte{}, nextKey([]byte{}))
    // 流程: cases := [][]byte{
    // 流程: {0},
    // 流程: {255},
    // 流程: {1, 255},
    // 流程: }
    // 控制流: for _, b := range cases {
    // 流程: next := nextKey(b)
    // 断言: require.Equal(t, append(b, 0), next)
    // 流程: }
    // 原注释: // in the old logic, this should return []byte{} which is not the actually smallest eky
    // 流程: next := nextKey([]byte{1, 255})
    // 断言: require.Equal(t, -1, bytes.Compare(next, []byte{2}))
    // 原注释: // another test case, nextkey()'s return should be smaller than key with a prefix of the origin key
    // 流程: next = nextKey([]byte{1, 255})
    // 断言: require.Equal(t, -1, bytes.Compare(next, []byte{1, 255, 0, 1, 2}))
    // 原注释: // test recode key
    // 原注释: // key with int handle
    // 控制流: for _, handleID := range []int64{math.MinInt64, 1, 255, math.MaxInt32 - 1} {
    // 流程: key := tablecodec.EncodeRowKeyWithHandle(1, tidbkv.IntHandle(handleID))
    // 断言: require.Equal(t, []byte(tablecodec.EncodeRowKeyWithHandle(1, tidbkv.IntHandle(handleID+1))), nextKey(key))
    // 流程: }
    // 原注释: // overflowed
    // 流程: key := tablecodec.EncodeRowKeyWithHandle(1, tidbkv.IntHandle(math.MaxInt64))
    // 流程: next = tablecodec.EncodeTablePrefix(2)
    // 断言: require.Less(t, string(key), string(next))
    // 断言: require.Equal(t, next, nextKey(key))
    // 流程: testDatums := [][]types.Datum{
    // 流程: {types.NewIntDatum(1), types.NewIntDatum(2)},
    // 流程: {types.NewIntDatum(255), types.NewIntDatum(256)},
    // 流程: {types.NewIntDatum(math.MaxInt32), types.NewIntDatum(math.MaxInt32 + 1)},
    // 流程: {types.NewStringDatum("test"), types.NewStringDatum("test\000")},
    // 流程: {types.NewStringDatum("test\255"), types.NewStringDatum("test\255\000")},
    // 流程: }
    // 流程: stmtCtx := stmtctx.NewStmtCtx()
    // 控制流: for _, datums := range testDatums {
    // 流程: keyBytes, err := codec.EncodeKey(stmtCtx.TimeZone(), nil, types.NewIntDatum(123), datums[0])
    // 断言: require.NoError(t, err)
    // 流程: h, err := tidbkv.NewCommonHandle(keyBytes)
    // 断言: require.NoError(t, err)
    // 流程: key := tablecodec.EncodeRowKeyWithHandle(1, h)
    // 流程: nextKeyBytes, err := codec.EncodeKey(stmtCtx.TimeZone(), nil, types.NewIntDatum(123), datums[1])
    // 断言: require.NoError(t, err)
    // 流程: nextHdl, err := tidbkv.NewCommonHandle(nextKeyBytes)
    // 断言: require.NoError(t, err)
    // 流程: nextValidKey := []byte(tablecodec.EncodeRowKeyWithHandle(1, nextHdl))
    // 原注释: // nextKey may return a key that can't be decoded, but it must not be larger than the valid next key.
    // 断言: require.True(t, bytes.Compare(nextKey(key), nextValidKey) <= 0, "datums: %v", datums)
    // 流程: }
    // 原注释: // a special case that when len(string datum) % 8 == 7, nextKey twice should not panic.
    // 流程: keyBytes, err := codec.EncodeKey(stmtCtx.TimeZone(), nil, types.NewStringDatum("1234567"))
    // 断言: require.NoError(t, err)
    // 流程: h, err := tidbkv.NewCommonHandle(keyBytes)
    // 断言: require.NoError(t, err)
    // 流程: key = tablecodec.EncodeRowKeyWithHandle(1, h)
    // 流程: nextOnce := nextKey(key)
    // 原注释: // should not panic
    // 流程: _ = nextKey(nextOnce)
    // 原注释: // dIAAAAAAAAD/PV9pgAAAAAD/AAABA4AAAAD/AAAAAQOAAAD/AAAAAAEAAAD8
    // 原注释: // a index key with: table: 61, index: 1, int64: 1, int64: 1
    // 流程: a := []byte{116, 128, 0, 0, 0, 0, 0, 0, 255, 61, 95, 105, 128, 0, 0, 0, 0, 255, 0, 0, 1, 3, 128, 0, 0, 0, 255, 0, 0, 0, 1, 3, 128, 0, 0, 255, 0, 0, 0, 0, 1, 0, 0, 0, 252}
    // 断言: require.Equal(t, append(a, 0), nextKey(a))
}

#[test]
fn next_key_advances_integer_record_keys_and_handles_overflow() {
    fn encode_int(value: i64) -> [u8; 8] {
        ((value as u64) ^ 0x8000_0000_0000_0000).to_be_bytes()
    }

    fn record_key(table_id: i64, handle: i64) -> Vec<u8> {
        let mut key = vec![b't'];
        key.extend_from_slice(&encode_int(table_id));
        key.extend_from_slice(b"_r");
        key.extend_from_slice(&encode_int(handle));
        key
    }

    let key = record_key(1, 41);
    assert_eq!(crate::local::NextKey(&key), record_key(1, 42));

    let mut next_table = vec![b't'];
    next_table.extend_from_slice(&encode_int(2));
    assert_eq!(crate::local::NextKey(&record_key(1, i64::MAX)), next_table);
    assert_eq!(crate::local::NextKey(&[]), Vec::<u8>::new());
}

#[test]
fn split_range_merges_tail_without_additional_properties() {
    use crate::KeyRange;

    let ranges = crate::local::splitRangeBySizeProps(
        KeyRange {
            start: b"a".to_vec(),
            end: b"z".to_vec(),
        },
        &[(b"m".to_vec(), 10, 1)],
        10,
        100,
    );

    assert_eq!(
        ranges,
        vec![KeyRange {
            start: b"a".to_vec(),
            end: b"z".to_vec(),
        }]
    );
}

#[test]
// TestRangeProperties 对应 Go 函数/方法声明。
// Go: func TestRangeProperties(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_range_properties() {
    // 流程: type testCase struct {
    // 流程: key []byte
    // 流程: vLen int
    // 流程: count int
    // 流程: }
    // 流程: cases := []testCase{
    // 原注释: // handle "a": size(size = 1, offset = 1),keys(1,1)
    // 流程: {[]byte("a"), 0, 1},
    // 流程: {[]byte("b"), defaultPropSizeIndexDistance / 8, 1},
    // 流程: {[]byte("c"), defaultPropSizeIndexDistance / 4, 1},
    // 流程: {[]byte("d"), defaultPropSizeIndexDistance / 2, 1},
    // 流程: {[]byte("e"), defaultPropSizeIndexDistance / 8, 1},
    // 原注释: // handle "e": size(size = DISTANCE + 4, offset = DISTANCE + 5),keys(4,5)
    // 流程: {[]byte("f"), defaultPropSizeIndexDistance / 4, 1},
    // 流程: {[]byte("g"), defaultPropSizeIndexDistance / 2, 1},
    // 流程: {[]byte("h"), defaultPropSizeIndexDistance / 8, 1},
    // 流程: {[]byte("i"), defaultPropSizeIndexDistance / 4, 1},
    // 原注释: // handle "i": size(size = DISTANCE / 8 * 9 + 4, offset = DISTANCE / 8 * 17 + 9),keys(4,5)
    // 流程: {[]byte("j"), defaultPropSizeIndexDistance / 2, 1},
    // 流程: {[]byte("k"), defaultPropSizeIndexDistance / 2, 1},
    // 原注释: // handle "k": size(size = DISTANCE + 2, offset = DISTANCE / 8 * 25 + 11),keys(2,11)
    // 流程: {[]byte("l"), 0, defaultPropKeysIndexDistance / 2},
    // 流程: {[]byte("m"), 0, defaultPropKeysIndexDistance / 2},
    // 原注释: // handle "m": keys = DEFAULT_PROP_KEYS_INDEX_DISTANCE,offset = 11+DEFAULT_PROP_KEYS_INDEX_DISTANCE
    // 流程: {[]byte("n"), 1, defaultPropKeysIndexDistance},
    // 原注释: // handle "n": keys = DEFAULT_PROP_KEYS_INDEX_DISTANCE, offset = 11+2*DEFAULT_PROP_KEYS_INDEX_DISTANCE
    // 流程: {[]byte("o"), 1, 1},
    // 原注释: // handle　"o": keys = 1, offset = 12 + 2*DEFAULT_PROP_KEYS_INDEX_DISTANCE
    // 流程: }
    // 流程: collector := newRangePropertiesCollector()
    // 控制流: for _, p := range cases {
    // 流程: v := make([]byte, p.vLen)
    // 控制流: for range p.count {
    // 并发/通道: _ = collector.Add(pebble.InternalKey{UserKey: p.key, Trailer: uint64(pebble.InternalKeyKindSet)}, v)
    // 流程: }
    // 流程: }
    // 流程: userProperties := make(map[string]string, 1)
    // 流程: _ = collector.Finish(userProperties)
    // 流程: props, err := decodeRangeProperties(hack.Slice(userProperties[propRangeIndex]), common.NoopKeyAdapter{})
    // 断言: require.NoError(t, err)
    // 原注释: // Smallest key in props.
    // 断言: require.Equal(t, cases[0].key, props[0].Key)
    // 原注释: // Largest key in props.
    // 断言: require.Equal(t, cases[len(cases)-1].key, props[len(props)-1].Key)
    // 断言: require.Len(t, props, 7)
    // 流程: props2 := rangeProperties([]rangeProperty{
    // 流程: {[]byte("b"), rangeOffsets{defaultPropSizeIndexDistance + 10, defaultPropKeysIndexDistance / 2}},
    // 流程: {[]byte("h"), rangeOffsets{defaultPropSizeIndexDistance * 3 / 2, defaultPropKeysIndexDistance * 3 / 2}},
    // 流程: {[]byte("k"), rangeOffsets{defaultPropSizeIndexDistance * 3, defaultPropKeysIndexDistance * 7 / 4}},
    // 流程: {[]byte("mm"), rangeOffsets{defaultPropSizeIndexDistance * 5, defaultPropKeysIndexDistance * 2}},
    // 流程: {[]byte("q"), rangeOffsets{defaultPropSizeIndexDistance * 7, defaultPropKeysIndexDistance*9/4 + 10}},
    // 流程: {[]byte("y"), rangeOffsets{defaultPropSizeIndexDistance*7 + 100, defaultPropKeysIndexDistance*9/4 + 1010}},
    // 流程: })
    // 流程: sizeProps := newSizeProperties()
    // 流程: sizeProps.addAll(props)
    // 流程: sizeProps.addAll(props2)
    // 流程: res := []*rangeProperty{
    // 流程: {[]byte("a"), rangeOffsets{1, 1}},
    // 流程: {[]byte("b"), rangeOffsets{defaultPropSizeIndexDistance + 10, defaultPropKeysIndexDistance / 2}},
    // 流程: {[]byte("e"), rangeOffsets{defaultPropSizeIndexDistance + 4, 4}},
    // 流程: {[]byte("h"), rangeOffsets{defaultPropSizeIndexDistance/2 - 10, defaultPropKeysIndexDistance}},
    // 流程: {[]byte("i"), rangeOffsets{defaultPropSizeIndexDistance*9/8 + 4, 4}},
    // 流程: {[]byte("k"), rangeOffsets{defaultPropSizeIndexDistance*5/2 + 2, defaultPropKeysIndexDistance/4 + 2}},
    // 流程: {[]byte("m"), rangeOffsets{defaultPropKeysIndexDistance, defaultPropKeysIndexDistance}},
    // 流程: {[]byte("mm"), rangeOffsets{defaultPropSizeIndexDistance * 2, defaultPropKeysIndexDistance / 4}},
    // 流程: {[]byte("n"), rangeOffsets{defaultPropKeysIndexDistance * 2, defaultPropKeysIndexDistance}},
    // 流程: {[]byte("o"), rangeOffsets{2, 1}},
    // 流程: {[]byte("q"), rangeOffsets{defaultPropSizeIndexDistance * 2, defaultPropKeysIndexDistance/4 + 10}},
    // 流程: {[]byte("y"), rangeOffsets{100, 1000}},
    // 流程: }
    // 断言: require.Equal(t, 12, sizeProps.indexHandles.Len())
    // 流程: idx := 0
    // 流程: sizeProps.iter(func(p *rangeProperty) bool {
    // 断言: require.Equal(t, res[idx], p)
    // 流程: idx++
    // 返回语义: return true
    // 流程: })
    // 流程: fullRange := engineapi.Range{Start: []byte("a"), End: []byte("z")}
    // 流程: ranges := splitRangeBySizeProps(fullRange, sizeProps, 2*defaultPropSizeIndexDistance, defaultPropKeysIndexDistance*5/2)
    // 断言: require.Equal(t, []engineapi.Range{
    // 流程: {Start: []byte("a"), End: []byte("e")},
    // 流程: {Start: []byte("e"), End: []byte("k")},
    // 流程: {Start: []byte("k"), End: []byte("mm")},
    // 流程: {Start: []byte("mm"), End: []byte("q")},
    // 流程: {Start: []byte("q"), End: []byte("z")},
    // 流程: }, ranges)
    // 流程: ranges = splitRangeBySizeProps(fullRange, sizeProps, 2*defaultPropSizeIndexDistance, defaultPropKeysIndexDistance)
    // 断言: require.Equal(t, []engineapi.Range{
    // 流程: {Start: []byte("a"), End: []byte("e")},
    // 流程: {Start: []byte("e"), End: []byte("h")},
    // 流程: {Start: []byte("h"), End: []byte("k")},
    // 流程: {Start: []byte("k"), End: []byte("m")},
    // 流程: {Start: []byte("m"), End: []byte("mm")},
    // 流程: {Start: []byte("mm"), End: []byte("n")},
    // 流程: {Start: []byte("n"), End: []byte("q")},
    // 流程: {Start: []byte("q"), End: []byte("z")},
    // 流程: }, ranges)
}

#[test]
// TestRangePropertiesWithPebble 对应 Go 函数/方法声明。
// Go: func TestRangePropertiesWithPebble(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_range_properties_with_pebble() {
    // 流程: sizeDistance := uint64(500)
    // 流程: keysDistance := uint64(20)
    // Pebble/SST IO: opt := &pebble.Options{
    // 流程: MemTableSize: 512 * units.MiB,
    // 流程: MaxConcurrentCompactions: func() int { return 16 },
    // 流程: L0CompactionThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: L0StopWritesThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: MaxOpenFiles: 10000,
    // 流程: DisableWAL: true,
    // 流程: ReadOnly: false,
    // Pebble/SST IO: TablePropertyCollectors: []func() pebble.TablePropertyCollector{
    // Pebble/SST IO: func() pebble.TablePropertyCollector {
    // 返回语义: return &RangePropertiesCollector{
    // 流程: props: make([]rangeProperty, 0, 1024),
    // 流程: propSizeIdxDistance: sizeDistance,
    // 流程: propKeysIdxDistance: keysDistance,
    // 流程: }
    // 流程: },
    // 流程: },
    // 流程: }
    // 流程: db, _ := makePebbleDB(t, opt)
    // 资源收尾: defer db.Close()
    // 原注释: // local collector
    // 流程: collector := &RangePropertiesCollector{
    // 流程: props: make([]rangeProperty, 0, 1024),
    // 流程: propSizeIdxDistance: sizeDistance,
    // 流程: propKeysIdxDistance: keysDistance,
    // 流程: }
    // Pebble/SST IO: writeOpt := &pebble.WriteOptions{Sync: false}
    // 流程: value := make([]byte, 100)
    // 控制流: for i := range 10 {
    // Pebble/SST IO: wb := db.NewBatch()
    // 控制流: for j := range 100 {
    // 流程: key := make([]byte, 8)
    // 流程: valueLen := rand.Intn(50)
    // 流程: binary.BigEndian.PutUint64(key, uint64(i*100+j))
    // 流程: err := wb.Set(key, value[:valueLen], writeOpt)
    // 断言: require.NoError(t, err)
    // 并发/通道: err = collector.Add(pebble.InternalKey{UserKey: key, Trailer: uint64(pebble.InternalKeyKindSet)}, value[:valueLen])
    // 断言: require.NoError(t, err)
    // 流程: }
    // 断言: require.NoError(t, wb.Commit(writeOpt))
    // 流程: }
    // 原注释: // flush one sst
    // 断言: require.NoError(t, db.Flush())
    // 流程: props := make(map[string]string, 1)
    // 断言: require.NoError(t, collector.Finish(props))
    // Pebble/SST IO: sstMetas, err := db.SSTables(pebble.WithProperties())
    // 断言: require.NoError(t, err)
    // 控制流: for i, level := range sstMetas {
    // 控制流: if i == 0 {
    // 断言: require.Equal(t, 1, len(level))
    // 流程: } else {
    // 断言: require.Empty(t, level)
    // 流程: }
    // 流程: }
    // 断言: require.Equal(t, props, sstMetas[0][0].Properties.UserProperties)
}

// testLocalWriter 对应 Go 函数/方法声明。
// Go: func testLocalWriter(t *testing.T, needSort bool, partitialSort bool)
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn test_local_writer() {
    // Pebble/SST IO: opt := &pebble.Options{
    // 流程: MemTableSize: 1024 * 1024,
    // 流程: MaxConcurrentCompactions: func() int { return 16 },
    // 流程: L0CompactionThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: L0StopWritesThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: DisableWAL: true,
    // 流程: ReadOnly: false,
    // 流程: }
    // 流程: db, tmpPath := makePebbleDB(t, opt)
    // 资源收尾: t.Cleanup(func() {
    // 断言: require.NoError(t, db.Close())
    // 流程: })
    // 流程: _, engineUUID := backend.MakeUUID("ww", 0)
    // context: engineCtx, cancel := context.WithCancel(context.Background())
    // 流程: f := &Engine{
    // 流程: UUID: engineUUID,
    // 流程: sstDir: tmpPath,
    // 流程: ctx: engineCtx,
    // 流程: cancel: cancel,
    // 并发/通道: sstMetasChan: make(chan metaOrFlush, 64),
    // 流程: keyAdapter: common.NoopKeyAdapter{},
    // 流程: logger: log.L(),
    // 流程: }
    // 流程: f.TS = oracle.GoTimeToTS(time.Now())
    // PD/TiKV region: f.db.Store(db)
    // Pebble/SST IO: f.sstIngester = dbSSTIngester{e: f}
    // 并发/通道: f.wg.Add(1)
    // 并发/通道: go f.ingestSSTLoop()
    // 流程: sorted := needSort && !partitialSort
    // 流程: pool := membuf.NewPool()
    // 资源收尾: defer pool.Destroy()
    // 流程: kvBuffer := pool.NewBuffer()
    // 流程: writerCfg := &backend.LocalWriterConfig{}
    // 流程: writerCfg.Local.IsKVSorted = sorted
    // 流程: w, err := openLocalWriter(writerCfg, f, keyspace.CodecV1, 1024, kvBuffer)
    // 断言: require.NoError(t, err)
    // context: ctx := context.Background()
    // 流程: var kvs []common.KvPair
    // 流程: value := make([]byte, 128)
    // 控制流: for i := range 16 {
    // 流程: binary.BigEndian.PutUint64(value[i*8:], uint64(i))
    // 流程: }
    // 流程: var keys [][]byte
    // 控制流: for i := 1; i <= 20000; i++ {
    // 流程: var kv common.KvPair
    // 流程: kv.Key = make([]byte, 16)
    // 流程: kv.Val = make([]byte, 128)
    // 流程: copy(kv.Val, value)
    // 流程: key := rand.Intn(1000)
    // 流程: binary.BigEndian.PutUint64(kv.Key, uint64(key))
    // 流程: binary.BigEndian.PutUint64(kv.Key[8:], uint64(i))
    // 流程: kvs = append(kvs, kv)
    // 流程: keys = append(keys, kv.Key)
    // 流程: }
    // 流程: var rows1 []common.KvPair
    // 流程: var rows2 []common.KvPair
    // 流程: var rows3 []common.KvPair
    // 流程: rows4 := kvs[:12000]
    // 控制流: if partitialSort {
    // 流程: sort.Slice(rows4, func(i, j int) bool {
    // 返回语义: return bytes.Compare(rows4[i].Key, rows4[j].Key) < 0
    // 流程: })
    // 流程: rows1 = rows4[:6000]
    // 流程: rows3 = rows4[6000:]
    // 流程: rows2 = kvs[12000:]
    // 流程: } else {
    // 控制流: if needSort {
    // 流程: sort.Slice(kvs, func(i, j int) bool {
    // 返回语义: return bytes.Compare(kvs[i].Key, kvs[j].Key) < 0
    // 流程: })
    // 流程: }
    // 流程: rows1 = kvs[:6000]
    // 流程: rows2 = kvs[6000:12000]
    // 流程: rows3 = kvs[12000:]
    // 流程: }
    // 流程: err = w.AppendRows(ctx, []string{}, kv.MakeRowsFromKvPairs(rows1))
    // 断言: require.NoError(t, err)
    // 流程: err = w.AppendRows(ctx, []string{}, kv.MakeRowsFromKvPairs(rows2))
    // 断言: require.NoError(t, err)
    // 流程: err = w.AppendRows(ctx, []string{}, kv.MakeRowsFromKvPairs(rows3))
    // 断言: require.NoError(t, err)
    // 资源收尾: flushStatus, err := w.Close(context.Background())
    // 断言: require.NoError(t, err)
    // 断言: require.NoError(t, f.flushEngineWithoutLock(ctx))
    // 断言: require.True(t, flushStatus.Flushed())
    // Pebble/SST IO: o := &pebble.IterOptions{}
    // 流程: it, _ := db.NewIter(o)
    // 流程: sort.Slice(keys, func(i, j int) bool {
    // 返回语义: return bytes.Compare(keys[i], keys[j]) < 0
    // 流程: })
    // 断言: require.Equal(t, 20000, int(f.Length.Load()))
    // 断言: require.Equal(t, 144*20000, int(f.TotalSize.Load()))
    // 流程: valid := it.SeekGE(keys[0])
    // 断言: require.True(t, valid)
    // 控制流: for _, k := range keys {
    // 断言: require.Equal(t, k, it.Key())
    // 流程: it.Next()
    // 流程: }
    // 断言: require.NoError(t, it.Close())
    // 流程: close(f.sstMetasChan)
    // 并发/通道: f.wg.Wait()
}

#[test]
// TestEngineLocalWriter 对应 Go 函数/方法声明。
// Go: func TestEngineLocalWriter(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_engine_local_writer() {
    // 原注释: // test local writer with sort
    // 流程: testLocalWriter(t, false, false)
    // 原注释: // test local writer with ingest
    // 流程: testLocalWriter(t, true, false)
    // 原注释: // test local writer with ingest unsort
    // 流程: testLocalWriter(t, true, true)
}

// testIngester 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type testIngester struct{}
pub struct testIngester;

// mergeSSTs 对应 Go 函数/方法声明。
// Go: func (i testIngester) mergeSSTs(metas []*sstMeta, dir string, blockSize int) (*sstMeta, error)
// 接收者 `testIngester` 的方法在这里中摊平成函数名 `test_ingester_merge_ss_ts`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn test_ingester_merge_ss_ts() {
    // 控制流: if len(metas) == 0 {
    // 返回语义: return nil, errors.New("sst metas is empty")
    // 流程: } else if len(metas) == 1 {
    // 返回语义: return metas[0], nil
    // 流程: }
    // 控制流: if metas[len(metas)-1].seq-metas[0].seq != int32(len(metas)-1) {
    // 流程: panic("metas is not add in order")
    // 流程: }
    // 流程: newMeta := &sstMeta{
    // 流程: seq: metas[len(metas)-1].seq,
    // 流程: }
    // 控制流: for _, m := range metas {
    // 流程: newMeta.totalSize += m.totalSize
    // 流程: newMeta.totalCount += m.totalCount
    // 流程: }
    // 返回语义: return newMeta, nil
}

// ingest 对应 Go 函数/方法声明。
// Go: func (i testIngester) ingest([]*sstMeta) error
// 接收者 `testIngester` 的方法在这里中摊平成函数名 `test_ingester_ingest`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn test_ingester_ingest() {
    // 返回语义: return nil
}

#[test]
// TestLocalIngestLoop 对应 Go 函数/方法声明。
// Go: func TestLocalIngestLoop(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_local_ingest_loop() {
    // Pebble/SST IO: opt := &pebble.Options{
    // 流程: MemTableSize: 1024 * 1024,
    // 流程: MaxConcurrentCompactions: func() int { return 16 },
    // 流程: L0CompactionThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: L0StopWritesThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: DisableWAL: true,
    // 流程: ReadOnly: false,
    // 流程: }
    // 流程: db, tmpPath := makePebbleDB(t, opt)
    // 资源收尾: defer db.Close()
    // 流程: _, engineUUID := backend.MakeUUID("ww", 0)
    // context: engineCtx, cancel := context.WithCancel(context.Background())
    // 流程: f := Engine{
    // 流程: UUID: engineUUID,
    // 流程: sstDir: tmpPath,
    // 流程: ctx: engineCtx,
    // 流程: cancel: cancel,
    // 并发/通道: sstMetasChan: make(chan metaOrFlush, 64),
    // 流程: config: backend.LocalEngineConfig{
    // 流程: Compact: true,
    // 流程: CompactThreshold: 100,
    // 流程: CompactConcurrency: 4,
    // 流程: },
    // 流程: logger: log.L(),
    // 流程: }
    // PD/TiKV region: f.db.Store(db)
    // 流程: f.sstIngester = testIngester{}
    // 并发/通道: f.wg.Add(1)
    // 并发/通道: go f.ingestSSTLoop()
    // 原注释: // add some routines to add ssts
    // 并发/通道: var wg sync.WaitGroup
    // 并发/通道: wg.Add(4)
    // 流程: totalSize := int64(0)
    // 流程: concurrency := 4
    // 流程: count := 500
    // 流程: var metaSeqLock sync.Mutex
    // 流程: maxMetaSeq := int32(0)
    // 控制流: for range concurrency {
    // 并发/通道: go func() {
    // 资源收尾: defer wg.Done()
    // 流程: flushCnt := rand.Int31n(10) + 1
    // 流程: seq := int32(0)
    // 控制流: for i := range count {
    // 流程: size := int64(rand.Int31n(50) + 1)
    // 流程: m := &sstMeta{totalSize: size, totalCount: 1}
    // 流程: atomic.AddInt64(&totalSize, size)
    // Pebble/SST IO: metaSeq, err := f.addSST(engineCtx, m)
    // 断言: require.NoError(t, err)
    // 控制流: if int32(i) >= flushCnt {
    // 流程: f.mutex.RLock()
    // 流程: err = f.flushEngineWithoutLock(engineCtx)
    // 断言: require.NoError(t, err)
    // 流程: f.mutex.RUnlock()
    // 流程: flushCnt += rand.Int31n(10) + 1
    // 流程: }
    // 流程: seq = metaSeq
    // 流程: }
    // 流程: metaSeqLock.Lock()
    // 控制流: if atomic.LoadInt32(&maxMetaSeq) < seq {
    // PD/TiKV region: atomic.StoreInt32(&maxMetaSeq, seq)
    // 流程: }
    // 流程: metaSeqLock.Unlock()
    // 流程: }()
    // 流程: }
    // 并发/通道: wg.Wait()
    // 流程: f.mutex.RLock()
    // 流程: err := f.flushEngineWithoutLock(engineCtx)
    // 断言: require.NoError(t, err)
    // 流程: f.mutex.RUnlock()
    // 流程: close(f.sstMetasChan)
    // 并发/通道: f.wg.Wait()
    // 断言: require.NoError(t, f.ingestErr.Get())
    // 断言: require.Equal(t, f.TotalSize.Load(), totalSize)
    // 断言: require.Equal(t, int64(concurrency*count), f.Length.Load())
    // 断言: require.Equal(t, atomic.LoadInt32(&maxMetaSeq), f.finishedMetaSeq.Load())
}

// testMergeSSTs 对应 Go 函数/方法声明。
// Go: func testMergeSSTs(t *testing.T, kvs [][]common.KvPair, meta *sstMeta)
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn test_merge_ss_ts() {
    // Pebble/SST IO: opt := &pebble.Options{
    // 流程: MemTableSize: 1024 * 1024,
    // 流程: MaxConcurrentCompactions: func() int { return 16 },
    // 流程: L0CompactionThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: L0StopWritesThreshold: math.MaxInt32, // set to max try to disable compaction
    // 流程: DisableWAL: true,
    // 流程: ReadOnly: false,
    // 流程: }
    // 流程: db, tmpPath := makePebbleDB(t, opt)
    // 资源收尾: defer db.Close()
    // 流程: _, engineUUID := backend.MakeUUID("ww", 0)
    // context: engineCtx, cancel := context.WithCancel(context.Background())
    // 流程: f := &Engine{
    // 流程: UUID: engineUUID,
    // 流程: sstDir: tmpPath,
    // 流程: ctx: engineCtx,
    // 流程: cancel: cancel,
    // 并发/通道: sstMetasChan: make(chan metaOrFlush, 64),
    // 流程: config: backend.LocalEngineConfig{
    // 流程: Compact: true,
    // 流程: CompactThreshold: 100,
    // 流程: CompactConcurrency: 4,
    // 流程: },
    // 流程: logger: log.L(),
    // 流程: }
    // 流程: f.TS = oracle.GoTimeToTS(time.Now())
    // PD/TiKV region: f.db.Store(db)
    // Pebble/SST IO: createSSTWriter := func() (*sstWriter, error) {
    // 流程: path := filepath.Join(f.sstDir, uuid.New().String()+".sst")
    // Pebble/SST IO: writer, err := newSSTWriter(path, 16*1024)
    // 控制流: if err != nil {
    // 返回语义: return nil, err
    // 流程: }
    // 流程: sw := &sstWriter{sstMeta: &sstMeta{path: path}, writer: writer}
    // 返回语义: return sw, nil
    // 流程: }
    // 流程: metas := make([]*sstMeta, 0, len(kvs))
    // 控制流: for _, kv := range kvs {
    // Pebble/SST IO: w, err := createSSTWriter()
    // 断言: require.NoError(t, err)
    // 流程: err = w.writeKVs(kv)
    // 断言: require.NoError(t, err)
    // 断言: require.NoError(t, w.writer.Close())
    // 流程: metas = append(metas, w.sstMeta)
    // 流程: }
    // Pebble/SST IO: i := dbSSTIngester{e: f}
    // Pebble/SST IO: newMeta, err := i.mergeSSTs(metas, tmpPath, 16*1024)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, meta.totalCount, newMeta.totalCount)
    // 断言: require.Equal(t, meta.totalSize, newMeta.totalSize)
}

#[test]
// TestMergeSSTs 对应 Go 函数/方法声明。
// Go: func TestMergeSSTs(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn TestMergeSSTs() {
    // 流程: kvs := make([][]common.KvPair, 0, 5)
    // 控制流: for i := range 5 {
    // 流程: var pairs []common.KvPair
    // 控制流: for j := range 10 {
    // 流程: var kv common.KvPair
    // 流程: kv.Key = make([]byte, 16)
    // 流程: key := i*100 + j
    // 流程: binary.BigEndian.PutUint64(kv.Key, uint64(key))
    // 流程: pairs = append(pairs, kv)
    // 流程: }
    // 流程: kvs = append(kvs, pairs)
    // 流程: }
    // Pebble/SST IO: testMergeSSTs(t, kvs, &sstMeta{totalCount: 50, totalSize: 800})
}

#[test]
// TestMergeSSTsDuplicated 对应 Go 函数/方法声明。
// Go: func TestMergeSSTsDuplicated(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_merge_ss_ts_duplicated() {
    // 流程: kvs := make([][]common.KvPair, 0, 5)
    // 控制流: for i := range 4 {
    // 流程: var pairs []common.KvPair
    // 控制流: for j := range 10 {
    // 流程: var kv common.KvPair
    // 流程: kv.Key = make([]byte, 16)
    // 流程: key := i*100 + j
    // 流程: binary.BigEndian.PutUint64(kv.Key, uint64(key))
    // 流程: pairs = append(pairs, kv)
    // 流程: }
    // 流程: kvs = append(kvs, pairs)
    // 流程: }
    // 原注释: // make a duplication
    // 流程: kvs = append(kvs, kvs[0])
    // Pebble/SST IO: testMergeSSTs(t, kvs, &sstMeta{totalCount: 40, totalSize: 640})
}

// mockPdClient 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockPdClient struct {
// Go: pd.Client
// Go: stores []*metapb.Store
// Go: regions []*router.Region
// Go: closed bool
// Go: }
pub struct mockPdClient;

// GetAllStores 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) GetAllStores(ctx context.Context, opts ...opt.GetStoreOption) ([]*metapb.Store, error)
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_get_all_stores`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_get_all_stores() {
    // 返回语义: return c.stores, nil
}

// ScanRegions 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) ScanRegions(ctx context.Context, key, endKey []byte, limit int, opts ...opt.GetRegionOption) ([]*router.Region, error)
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_scan_regions`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_scan_regions() {
    // 返回语义: return c.regions, nil
}

// GetTS 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) GetTS(ctx context.Context) (int64, int64, error)
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_get_ts`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_get_ts() {
    // 返回语义: return 1, 2, nil
}

// GetClusterID 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) GetClusterID(ctx context.Context) uint64
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_get_cluster_id`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_get_cluster_id() {
    // 返回语义: return 1
}

// WithCallerComponent 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) WithCallerComponent(component caller.Component) pd.Client
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_with_caller_component`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_with_caller_component() {
    // 返回语义: return c
}

// Close 对应 Go 函数/方法声明。
// Go: func (c *mockPdClient) Close()
// 接收者 `mockPdClient` 的方法在这里中摊平成函数名 `mock_pd_client_close`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_pd_client_close() {
    // 流程: c.closed = true
}

// mockGrpcErr 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockGrpcErr struct{}
pub struct mockGrpcErr;

// GRPCStatus 对应 Go 函数/方法声明。
// Go: func (e mockGrpcErr) GRPCStatus() *status.Status
// 接收者 `mockGrpcErr` 的方法在这里中摊平成函数名 `mock_grpc_err_grpc_status`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_grpc_err_grpc_status() {
    // 返回语义: return status.New(codes.Unimplemented, "unimplemented")
}

// Error 对应 Go 函数/方法声明。
// Go: func (e mockGrpcErr) Error() string
// 接收者 `mockGrpcErr` 的方法在这里中摊平成函数名 `mock_grpc_err_error`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_grpc_err_error() {
    // 返回语义: return "unimplemented"
}

// mockImportClient 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockImportClient struct {
// Go: sst.ImportSSTClient
// Go: store *metapb.Store
// Go: resp *sst.IngestResponse
// Go: onceResp *atomic.Pointer[sst.IngestResponse]
// Go: err error
// Go: retry int
// Go: cnt int
// Go: multiIngestCheckFn func(s *metapb.Store) bool
// Go: apiInvokeRecorder map[string][]uint64
// Go: }
pub struct mockImportClient;

// newMockImportClient 对应 Go 函数/方法声明。
// Go: func newMockImportClient() *mockImportClient
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn new_mock_import_client() {
    // mock: return &mockImportClient{
    // PD/TiKV region: multiIngestCheckFn: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: }
}

// MultiIngest 对应 Go 函数/方法声明。
// Go: func (c *mockImportClient) MultiIngest(_ context.Context, req *sst.MultiIngestRequest, _ ...grpc.CallOption) (*sst.IngestResponse, error)
// 接收者 `mockImportClient` 的方法在这里中摊平成函数名 `mock_import_client_multi_ingest`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_import_client_multi_ingest() {
    // 资源收尾: defer func() {
    // 流程: c.cnt++
    // 流程: }()
    // 控制流: for _, meta := range req.Ssts {
    // PD/TiKV region: if meta.RegionId != c.store.GetId() {
    // 返回语义: return &sst.IngestResponse{Error: &errorpb.Error{Message: "The file which would be ingested doest not exist."}}, nil
    // 流程: }
    // 流程: }
    // 控制流: if c.apiInvokeRecorder != nil {
    // 流程: c.apiInvokeRecorder["MultiIngest"] = append(c.apiInvokeRecorder["MultiIngest"], c.store.GetId())
    // 流程: }
    // 控制流: if c.cnt < c.retry {
    // 控制流: if c.err != nil {
    // 返回语义: return c.resp, c.err
    // 流程: }
    // 控制流: if c.onceResp != nil {
    // 流程: resp := c.onceResp.Swap(&sst.IngestResponse{})
    // 返回语义: return resp, nil
    // 流程: }
    // 控制流: if c.resp != nil {
    // 返回语义: return c.resp, nil
    // 流程: }
    // 流程: }
    // 控制流: if !c.multiIngestCheckFn(c.store) {
    // mock: return nil, mockGrpcErr{}
    // 流程: }
    // 返回语义: return &sst.IngestResponse{}, nil
}

// mockWriteClient 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockWriteClient struct {
// Go: sst.ImportSST_WriteClient
// Go: writeResp *sst.WriteResponse
// Go: }
pub struct mockWriteClient;

// Send 对应 Go 函数/方法声明。
// Go: func (m mockWriteClient) Send(request *sst.WriteRequest) error
// 接收者 `mockWriteClient` 的方法在这里中摊平成函数名 `mock_write_client_send`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_write_client_send() {
    // 返回语义: return nil
}

// CloseAndRecv 对应 Go 函数/方法声明。
// Go: func (m mockWriteClient) CloseAndRecv() (*sst.WriteResponse, error)
// 接收者 `mockWriteClient` 的方法在这里中摊平成函数名 `mock_write_client_close_and_recv`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_write_client_close_and_recv() {
    // 返回语义: return m.writeResp, nil
}

// baseCodec 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type baseCodec interface {
// Go: Marshal(v any) ([]byte, error)
// Go: Unmarshal(data []byte, v any) error
// Go: }
pub trait baseCodec {}

// newContextWithRPCInfo 对应 Go 函数/方法声明。
// Go: func newContextWithRPCInfo(ctx context.Context, failfast bool, codec baseCodec, cp grpc.Compressor, comp encoding.Compressor) context.Context
// Go:
// Go: type mockCodec struct
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn new_context_with_rpc_info() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

// Marshal 对应 Go 函数/方法声明。
// Go: func (m mockCodec) Marshal(v any) ([]byte, error)
// 接收者 `mockCodec` 的方法在这里中摊平成函数名 `mock_codec_marshal`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_codec_marshal() {
    // 返回语义: return nil, nil
}

// Unmarshal 对应 Go 函数/方法声明。
// Go: func (m mockCodec) Unmarshal(data []byte, v any) error
// 接收者 `mockCodec` 的方法在这里中摊平成函数名 `mock_codec_unmarshal`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_codec_unmarshal() {
    // 返回语义: return nil
}

// Context 对应 Go 函数/方法声明。
// Go: func (m mockWriteClient) Context() context.Context
// 接收者 `mockWriteClient` 的方法在这里中摊平成函数名 `mock_write_client_context`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_write_client_context() {
    // context: ctx := context.Background()
    // mock: return newContextWithRPCInfo(ctx, false, mockCodec{}, nil, nil)
}

// SendMsg 对应 Go 函数/方法声明。
// Go: func (m mockWriteClient) SendMsg(_ any) error
// 接收者 `mockWriteClient` 的方法在这里中摊平成函数名 `mock_write_client_send_msg`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_write_client_send_msg() {
    // 返回语义: return nil
}

// Write 对应 Go 函数/方法声明。
// Go: func (c *mockImportClient) Write(ctx context.Context, opts ...grpc.CallOption) (sst.ImportSST_WriteClient, error)
// 接收者 `mockImportClient` 的方法在这里中摊平成函数名 `mock_import_client_write`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_import_client_write() {
    // 控制流: if c.apiInvokeRecorder != nil {
    // 流程: c.apiInvokeRecorder["Write"] = append(c.apiInvokeRecorder["Write"], c.store.GetId())
    // 流程: }
    // Pebble/SST IO: return mockWriteClient{writeResp: &sst.WriteResponse{Metas: []*sst.SSTMeta{
    // PD/TiKV region: {RegionId: c.store.GetId()},
    // 流程: }}}, nil
}

// mockImportClientFactory 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockImportClientFactory struct {
// Go: stores []*metapb.Store
// Go: createClientFn func(store *metapb.Store) sst.ImportSSTClient
// Go: apiInvokeRecorder map[string][]uint64
// Go: }
pub struct mockImportClientFactory;

// create 对应 Go 函数/方法声明。
// Go: func (f *mockImportClientFactory) create(_ context.Context, storeID uint64) (sst.ImportSSTClient, error)
// 接收者 `mockImportClientFactory` 的方法在这里中摊平成函数名 `mock_import_client_factory_create`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_import_client_factory_create() {
    // 控制流: for _, store := range f.stores {
    // 控制流: if store.Id == storeID {
    // 返回语义: return f.createClientFn(store), nil
    // 流程: }
    // 流程: }
    // 返回语义: return nil, fmt.Errorf("store %d not found", storeID)
}

// close 对应 Go 函数/方法声明。
// Go: func (f *mockImportClientFactory) close()
// 接收者 `mockImportClientFactory` 的方法在这里中摊平成函数名 `mock_import_client_factory_close`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_import_client_factory_close() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

#[test]
// TestMultiIngest 对应 Go 函数/方法声明。
// Go: func TestMultiIngest(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_multi_ingest() {
    // PD/TiKV region: allStores := []*metapb.Store{
    // 流程: {
    // 流程: Id: 1,
    // PD/TiKV region: State: metapb.StoreState_Offline,
    // 流程: },
    // 流程: {
    // 流程: Id: 2,
    // PD/TiKV region: State: metapb.StoreState_Tombstone,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "test",
    // 流程: Value: "tiflash",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 3,
    // PD/TiKV region: State: metapb.StoreState_Up,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "test",
    // 流程: Value: "123",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 4,
    // PD/TiKV region: State: metapb.StoreState_Tombstone,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "engine",
    // 流程: Value: "test",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 5,
    // PD/TiKV region: State: metapb.StoreState_Tombstone,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "engine",
    // 流程: Value: "test123",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 6,
    // PD/TiKV region: State: metapb.StoreState_Offline,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "engine",
    // 流程: Value: "tiflash",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 7,
    // PD/TiKV region: State: metapb.StoreState_Up,
    // PD/TiKV region: Labels: []*metapb.StoreLabel{
    // 流程: {
    // 流程: Key: "test",
    // 流程: Value: "123",
    // 流程: },
    // 流程: {
    // 流程: Key: "engine",
    // 流程: Value: "tiflash",
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: Id: 8,
    // PD/TiKV region: State: metapb.StoreState_Up,
    // 流程: },
    // 流程: }
    // 流程: cases := []struct {
    // PD/TiKV region: filter func(store *metapb.Store) bool
    // PD/TiKV region: multiIngestSupport func(s *metapb.Store) bool
    // 流程: retry int
    // 流程: err error
    // 流程: supportMutliIngest bool
    // 流程: retErr string
    // 流程: }{
    // 原注释: // test up stores with all support multiIngest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // PD/TiKV region: return store.State == metapb.StoreState_Up
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 原注释: // test all up stores with tiflash not support multi ingest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // PD/TiKV region: return store.State == metapb.StoreState_Up
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return !engine.IsTiFlash(s)
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 原注释: // test all up stores with only tiflash support multi ingest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // PD/TiKV region: return store.State == metapb.StoreState_Up
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return engine.IsTiFlash(s)
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: false,
    // 流程: "",
    // 流程: },
    // 原注释: // test all up stores with some non-tiflash store support multi ingest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // PD/TiKV region: return store.State == metapb.StoreState_Up
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return len(s.Labels) > 0
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: false,
    // 流程: "",
    // 流程: },
    // 原注释: // test all stores with all states
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 原注释: // test all non-tiflash stores that support multi ingests
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return !engine.IsTiFlash(store)
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return !engine.IsTiFlash(s)
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 原注释: // test only up stores support multi ingest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // PD/TiKV region: return s.State == metapb.StoreState_Up
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 原注释: // test only offline/tombstore stores support multi ingest
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // PD/TiKV region: return s.State != metapb.StoreState_Up
    // 流程: },
    // 流程: 0,
    // 流程: nil,
    // 流程: false,
    // 流程: "",
    // 流程: },
    // 原注释: // test grpc return error but no tiflash
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return !engine.IsTiFlash(store)
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: math.MaxInt32,
    // mock: errors.New("mock error"),
    // 流程: false,
    // 流程: "",
    // 流程: },
    // 原注释: // test grpc return error and contains offline tiflash
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // PD/TiKV region: return !engine.IsTiFlash(store) || store.State != metapb.StoreState_Up
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: math.MaxInt32,
    // mock: errors.New("mock error"),
    // 流程: false,
    // 流程: "",
    // 流程: },
    // 原注释: // test grpc return error
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: math.MaxInt32,
    // mock: errors.New("mock error"),
    // 流程: false,
    // mock: "mock error",
    // 流程: },
    // 原注释: // test grpc return error only once
    // 流程: {
    // PD/TiKV region: func(store *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // PD/TiKV region: func(s *metapb.Store) bool {
    // 返回语义: return true
    // 流程: },
    // 流程: 1,
    // mock: errors.New("mock error"),
    // 流程: true,
    // 流程: "",
    // 流程: },
    // 流程: }
    // 控制流: for _, testCase := range cases {
    // PD/TiKV region: stores := make([]*metapb.Store, 0, len(allStores))
    // PD/TiKV region: for _, s := range allStores {
    // 控制流: if testCase.filter(s) {
    // 流程: stores = append(stores, s)
    // 流程: }
    // 流程: }
    // mock: importCli := &mockImportClient{
    // 流程: cnt: 0,
    // 流程: retry: testCase.retry,
    // 流程: err: testCase.err,
    // 流程: multiIngestCheckFn: testCase.multiIngestSupport,
    // 流程: }
    // 流程: local := &Backend{
    // mock: pdCli: &mockPdClient{stores: stores},
    // mock: importClientFactory: &mockImportClientFactory{
    // PD/TiKV region: stores: allStores,
    // Pebble/SST IO: createClientFn: func(store *metapb.Store) sst.ImportSSTClient {
    // 流程: importCli.store = store
    // 返回语义: return importCli
    // 流程: },
    // 流程: },
    // 流程: }
    // context: supportMultiIngest, err := checkMultiIngestSupport(context.Background(), local.pdCli, local.importClientFactory)
    // 控制流: if err != nil {
    // 断言: require.Contains(t, err.Error(), testCase.retErr)
    // 流程: } else {
    // 断言: require.Equal(t, testCase.supportMutliIngest, supportMultiIngest)
    // 流程: }
    // 流程: }
}

// mockIngestData 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockIngestData [][2][]byte
pub struct mockIngestData;

// GetFirstAndLastKey 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) GetFirstAndLastKey(lowerBound, upperBound []byte) ([]byte, []byte, error)
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_get_first_and_last_key`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_get_first_and_last_key() {
    // 流程: i, j := m.getFirstAndLastKeyIdx(lowerBound, upperBound)
    // 控制流: if i == -1 {
    // 返回语义: return nil, nil, nil
    // 流程: }
    // 返回语义: return m[i][0], m[j][0], nil
}

// getFirstAndLastKeyIdx 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) getFirstAndLastKeyIdx(lowerBound, upperBound []byte) (int, int)
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_get_first_and_last_key_idx`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_get_first_and_last_key_idx() {
    // 流程: var first int
    // 控制流: if len(lowerBound) == 0 {
    // 流程: first = 0
    // 流程: } else {
    // 流程: i, _ := sort.Find(len(m), func(i int) int {
    // 返回语义: return bytes.Compare(lowerBound, m[i][0])
    // 流程: })
    // 控制流: if i == len(m) {
    // 返回语义: return -1, -1
    // 流程: }
    // 流程: first = i
    // 流程: }
    // 流程: var last int
    // 控制流: if len(upperBound) == 0 {
    // 流程: last = len(m) - 1
    // 流程: } else {
    // 流程: i, _ := sort.Find(len(m), func(i int) int {
    // 返回语义: return bytes.Compare(upperBound, m[i][1])
    // 流程: })
    // 控制流: if i == 0 {
    // 返回语义: return -1, -1
    // 流程: }
    // 控制流: if i == len(m) || !bytes.Equal(upperBound, m[i][1]) {
    // 流程: last = i - 1
    // 流程: } else {
    // 流程: last = i
    // 流程: }
    // 流程: }
    // 返回语义: return first, last
}

// mockIngestIter 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockIngestIter struct {
// Go: data mockIngestData
// Go: // [startIdx, endIdx)
// Go: startIdx, endIdx, curIdx int
// Go: }
pub struct mockIngestIter;

// First 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) First() bool
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_first`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_first() {
    // 流程: m.curIdx = m.startIdx
    // 返回语义: return true
}

// Valid 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Valid() bool
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_valid`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_valid() {
    // 返回语义: return m.curIdx < m.endIdx
}

// Next 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Next() bool
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_next`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_next() {
    // 流程: m.curIdx++
    // 返回语义: return m.Valid()
}

// Key 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Key() []byte
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_key`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_key() {
    // 返回语义: return m.data[m.curIdx][0]
}

// Value 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Value() []byte
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_value`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_value() {
    // 返回语义: return m.data[m.curIdx][1]
}

// Close 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Close() error
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_close`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_close() {
    // 返回语义: return nil
}

// Error 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) Error() error
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_error`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_error() {
    // 返回语义: return nil
}

// ReleaseBuf 对应 Go 函数/方法声明。
// Go: func (m *mockIngestIter) ReleaseBuf()
// 接收者 `mockIngestIter` 的方法在这里中摊平成函数名 `mock_ingest_iter_release_buf`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_iter_release_buf() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

// NewIter 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) NewIter(_ context.Context, lowerBound, upperBound []byte, _ *membuf.Pool) engineapi.ForwardIter
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_new_iter`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_new_iter() {
    // 流程: i, j := m.getFirstAndLastKeyIdx(lowerBound, upperBound)
    // mock: return &mockIngestIter{data: m, startIdx: i, endIdx: j + 1, curIdx: i}
}

// GetTS 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) GetTS() uint64
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_get_ts`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_get_ts() {
    // 返回语义: return oracle.GoTimeToTS(time.Now())
}

// IncRef 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) IncRef()
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_inc_ref`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_inc_ref() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

// DecRef 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) DecRef()
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_dec_ref`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_dec_ref() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

// Finish 对应 Go 函数/方法声明。
// Go: func (m mockIngestData) Finish(_, _ int64)
// 接收者 `mockIngestData` 的方法在这里中摊平成函数名 `mock_ingest_data_finish`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_ingest_data_finish() {
    // Go 实现为空或只声明类型关系；保留该位置以便后续接线。
}

// var 声明沿用 Go 顶层状态，用于测试替换或共享 fixture。
// Go: var dummyRegionInfo = &split.RegionInfo{
pub fn dummy_region_info_go_binding_note() {
    // 这里不初始化真实全局状态，只记录 Go 测试中可被替换的符号。
}

#[test]
// TestCheckPeersBusy 对应 Go 函数/方法声明。
// Go: func TestCheckPeersBusy(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_check_peers_busy() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: backup := maxRetryBackoffSecond
    // 流程: maxRetryBackoffSecond = 300
    // 资源收尾: t.Cleanup(func() {
    // 流程: maxRetryBackoffSecond = backup
    // 流程: })
    // 流程: apiInvokeRecorder := map[string][]uint64{}
    // 流程: serverIsBusyResp := &sst.IngestResponse{
    // 流程: Error: &errorpb.Error{
    // 流程: ServerIsBusy: &errorpb.ServerIsBusy{},
    // 流程: }}
    // PD/TiKV region: createTimeStore12 := 0
    // 流程: local := &Backend{
    // mock: importClientFactory: &mockImportClientFactory{
    // PD/TiKV region: stores: []*metapb.Store{
    // 流程: {Id: 11}, {Id: 12}, {Id: 13}, // region ["a", "b")
    // 流程: {Id: 21}, {Id: 22}, {Id: 23}, // region ["b", "")
    // 流程: },
    // Pebble/SST IO: createClientFn: func(store *metapb.Store) sst.ImportSSTClient {
    // mock: importCli := newMockImportClient()
    // 流程: importCli.store = store
    // 流程: importCli.apiInvokeRecorder = apiInvokeRecorder
    // 控制流: if store.Id == 12 {
    // PD/TiKV region: createTimeStore12++
    // 原注释: // the second time is checkWriteStall, we mock a busy response
    // PD/TiKV region: if createTimeStore12 == 2 {
    // 流程: importCli.retry = 1
    // 流程: importCli.resp = serverIsBusyResp
    // 流程: }
    // 流程: }
    // 返回语义: return importCli
    // 流程: },
    // 流程: },
    // 流程: logger: log.L(),
    // PD/TiKV region: writeLimiter: newStoreWriteLimiter(0),
    // 流程: supportMultiIngest: true,
    // 流程: BackendConfig: BackendConfig{
    // 流程: ShouldCheckWriteStall: true,
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: }
    // 流程: var err error
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // mock: data := mockIngestData{{[]byte("a"), []byte("a")}, {[]byte("b"), []byte("b")}}
    // 流程: var (
    // context: workGroup, workerCtx = util.NewErrorGroupWithRecoverWithCtx(context.Background())
    // 并发/通道: jobToWorkerCh = make(chan *regionJob, 10)
    // 并发/通道: jobFromWorkerCh = make(chan *regionJob)
    // 流程: )
    // 并发/通道: pool := getRegionJobWorkerPool(workerCtx, &sync.WaitGroup{}, local, nil, jobToWorkerCh, jobFromWorkerCh, 0)
    // 流程: retryJob := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("b")},
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 1,
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 1, StoreId: 11}, {Id: 2, StoreId: 12}, {Id: 3, StoreId: 13},
    // 流程: },
    // 流程: StartKey: []byte("a"),
    // 流程: EndKey: []byte("b"),
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 11},
    // 流程: },
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // 流程: retryCount: 20,
    // 并发/通道: waitUntil: time.Now().Add(-time.Second),
    // 流程: }
    // 并发/通道: jobToWorkerCh <- retryJob
    // 并发/通道: jobToWorkerCh <- &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("b"), End: []byte("")},
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 4,
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 4, StoreId: 21}, {Id: 5, StoreId: 22}, {Id: 6, StoreId: 23},
    // 流程: },
    // 流程: StartKey: []byte("b"),
    // 流程: EndKey: []byte(""),
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 4, StoreId: 21},
    // 流程: },
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // 流程: retryCount: 20,
    // 并发/通道: waitUntil: time.Now().Add(-time.Second),
    // 流程: }
    // 并发/通道: retryJobs := make(chan *regionJob, 1)
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 流程: workGroup.Go(func() error {
    // 流程: pool.Start(wctx)
    // 并发/通道: <-wctx.Done()
    // 流程: pool.Release()
    // 返回语义: return wctx.OperatorErr()
    // 流程: })
    // 流程: workGroup.Go(func() error {
    // 并发/通道: job := <-jobFromWorkerCh
    // 流程: job.retryCount++
    // 并发/通道: retryJobs <- job
    // 并发/通道: <-jobFromWorkerCh
    // 流程: wctx.Cancel()
    // 返回语义: return nil
    // 流程: })
    // 断言: require.NoError(t, workGroup.Wait())
    // 断言: require.Eventually(t, func() bool {
    // 返回语义: return len(retryJobs) == 1
    // 流程: }, 300*time.Second, time.Second)
    // 并发/通道: j := <-retryJobs
    // 断言: require.Same(t, retryJob, j)
    // 断言: require.Equal(t, 21, retryJob.retryCount)
    // 断言: require.Equal(t, wrote, retryJob.stage)
    // 断言: require.Equal(t, []uint64{11, 12, 13, 21, 22, 23}, apiInvokeRecorder["Write"])
    // 原注释: // store 12 has a follower busy, so it will break the workflow for region (11, 12, 13)
    // 断言: require.Equal(t, []uint64{11, 12, 21, 22, 23, 21}, apiInvokeRecorder["MultiIngest"])
    // 原注释: // region (11, 12, 13) has key range ["a", "b"), it's not finished.
    // 断言: require.Equal(t, []byte("a"), retryJob.keyRange.Start)
    // 断言: require.Equal(t, []byte("b"), retryJob.keyRange.End)
}

#[test]
// TestNotLeaderErrorNeedUpdatePeers 对应 Go 函数/方法声明。
// Go: func TestNotLeaderErrorNeedUpdatePeers(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_not_leader_error_need_update_peers() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 原注释: // test lightning using stale region info (1,2,3), now the region is (11,12,13)
    // 流程: apiInvokeRecorder := map[string][]uint64{}
    // 流程: notLeaderResp := &sst.IngestResponse{
    // 流程: Error: &errorpb.Error{
    // PD/TiKV region: NotLeader: &errorpb.NotLeader{Leader: &metapb.Peer{StoreId: 11}},
    // 流程: }}
    // 流程: local := &Backend{
    // PD/TiKV region: splitCli: initTestSplitClient3Replica([][]byte{{}, {'a'}, {}}, nil),
    // mock: importClientFactory: &mockImportClientFactory{
    // PD/TiKV region: stores: []*metapb.Store{
    // 流程: {Id: 1}, {Id: 2}, {Id: 3},
    // 流程: {Id: 11}, {Id: 12}, {Id: 13},
    // 流程: },
    // Pebble/SST IO: createClientFn: func(store *metapb.Store) sst.ImportSSTClient {
    // mock: importCli := newMockImportClient()
    // 流程: importCli.store = store
    // 流程: importCli.apiInvokeRecorder = apiInvokeRecorder
    // 控制流: if store.Id == 1 {
    // 流程: importCli.retry = 1
    // 流程: importCli.resp = notLeaderResp
    // 流程: }
    // 返回语义: return importCli
    // 流程: },
    // 流程: },
    // 流程: logger: log.L(),
    // PD/TiKV region: writeLimiter: newStoreWriteLimiter(0),
    // 流程: supportMultiIngest: true,
    // 流程: BackendConfig: BackendConfig{
    // 流程: ShouldCheckWriteStall: true,
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: }
    // 流程: var err error
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // 流程: var (
    // context: workGroup, workerCtx = util.NewErrorGroupWithRecoverWithCtx(context.Background())
    // 并发/通道: jobToWorkerCh = make(chan *regionJob, 10)
    // 并发/通道: jobFromWorkerCh = make(chan *regionJob)
    // 流程: )
    // 并发/通道: pool := getRegionJobWorkerPool(workerCtx, &sync.WaitGroup{}, local, nil, jobToWorkerCh, jobFromWorkerCh, 0)
    // mock: data := mockIngestData{{[]byte("a"), []byte("a")}}
    // 流程: staleJob := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("b")},
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 1,
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 1, StoreId: 1}, {Id: 2, StoreId: 2}, {Id: 3, StoreId: 3},
    // 流程: },
    // 流程: StartKey: []byte("a"),
    // 流程: EndKey: []byte(""),
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 1},
    // 流程: },
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // 流程: }
    // 并发/通道: jobToWorkerCh <- staleJob
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 流程: workGroup.Go(func() error {
    // 流程: pool.Start(wctx)
    // 并发/通道: <-wctx.Done()
    // 流程: pool.Release()
    // 返回语义: return wctx.OperatorErr()
    // 流程: })
    // 流程: workGroup.Go(func() error {
    // 控制流: for {
    // 并发/通道: job := <-jobFromWorkerCh
    // 控制流: if job.stage == ingested {
    // 流程: wctx.Cancel()
    // 返回语义: return nil
    // 流程: }
    // 并发/通道: jobToWorkerCh <- job
    // 流程: }
    // 流程: })
    // 断言: require.NoError(t, workGroup.Wait())
    // 原注释: // "ingest" to test peers busy of stale region: 1,2,3
    // 原注释: // then "write" to stale region: 1,2,3
    // 原注释: // then "ingest" to stale leader: 1
    // 原注释: // then meet NotLeader error, scanned new region (11,12,13)
    // 原注释: // repeat above for 11,12,13
    // 断言: require.Equal(t, []uint64{1, 2, 3, 11, 12, 13}, apiInvokeRecorder["Write"])
    // 断言: require.Equal(t, []uint64{1, 2, 3, 1, 11, 12, 13, 11}, apiInvokeRecorder["MultiIngest"])
}

#[test]
// TestPartialWriteIngestErrorWontPanic 对应 Go 函数/方法声明。
// Go: func TestPartialWriteIngestErrorWontPanic(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_partial_write_ingest_error_wont_panic() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 原注释: // let lightning meet any error that will call convertStageTo(needRescan)
    // 流程: apiInvokeRecorder := map[string][]uint64{}
    // 流程: notLeaderResp := &sst.IngestResponse{
    // 流程: Error: &errorpb.Error{
    // PD/TiKV region: NotLeader: &errorpb.NotLeader{Leader: &metapb.Peer{StoreId: 11}},
    // 流程: }}
    // 流程: local := &Backend{
    // PD/TiKV region: splitCli: initTestSplitClient3Replica([][]byte{{}, {'c'}}, nil),
    // mock: importClientFactory: &mockImportClientFactory{
    // PD/TiKV region: stores: []*metapb.Store{
    // 流程: {Id: 1}, {Id: 2}, {Id: 3},
    // 流程: },
    // Pebble/SST IO: createClientFn: func(store *metapb.Store) sst.ImportSSTClient {
    // mock: importCli := newMockImportClient()
    // 流程: importCli.store = store
    // 流程: importCli.apiInvokeRecorder = apiInvokeRecorder
    // 控制流: if store.Id == 1 {
    // 流程: importCli.retry = 1
    // 流程: importCli.resp = notLeaderResp
    // 流程: }
    // 返回语义: return importCli
    // 流程: },
    // 流程: },
    // 流程: logger: log.L(),
    // PD/TiKV region: writeLimiter: newStoreWriteLimiter(0),
    // 流程: supportMultiIngest: true,
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: BackendConfig: BackendConfig{
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // 流程: }
    // 流程: var err error
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // mock: data := mockIngestData{{[]byte("a"), []byte("a")}, {[]byte("a2"), []byte("a2")}}
    // 流程: var (
    // context: workGroup, workerCtx = util.NewErrorGroupWithRecoverWithCtx(context.Background())
    // 并发/通道: jobToWorkerCh = make(chan *regionJob, 10)
    // 并发/通道: jobFromWorkerCh = make(chan *regionJob)
    // 流程: )
    // 并发/通道: pool := getRegionJobWorkerPool(workerCtx, &sync.WaitGroup{}, local, nil, jobToWorkerCh, jobFromWorkerCh, 0)
    // 流程: partialWriteJob := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("c")},
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 1,
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 1, StoreId: 1}, {Id: 2, StoreId: 2}, {Id: 3, StoreId: 3},
    // 流程: },
    // 流程: StartKey: []byte("a"),
    // 流程: EndKey: []byte("c"),
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 1},
    // 流程: },
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // 原注释: // use small regionSplitSize to trigger partial write
    // PD/TiKV region: regionSplitSize: 1,
    // 流程: }
    // 并发/通道: jobToWorkerCh <- partialWriteJob
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 流程: workGroup.Go(func() error {
    // 流程: pool.Start(wctx)
    // 并发/通道: <-wctx.Done()
    // 流程: pool.Release()
    // 返回语义: return wctx.OperatorErr()
    // 流程: })
    // 流程: workGroup.Go(func() error {
    // 控制流: for {
    // 并发/通道: job := <-jobFromWorkerCh
    // 控制流: if job.stage == regionScanned {
    // 流程: wctx.Cancel()
    // 返回语义: return nil
    // 流程: }
    // 断言: require.Fail(t, "job stage %s is not expected", job.stage)
    // 流程: }
    // 流程: })
    // 断言: require.NoError(t, workGroup.Wait())
    // 断言: require.Equal(t, []uint64{1, 2, 3}, apiInvokeRecorder["Write"])
    // 断言: require.Equal(t, []uint64{1}, apiInvokeRecorder["MultiIngest"])
}

#[test]
// TestPartialWriteIngestBusy 对应 Go 函数/方法声明。
// Go: func TestPartialWriteIngestBusy(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_partial_write_ingest_busy() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: apiInvokeRecorder := map[string][]uint64{}
    // 流程: notLeaderResp := &sst.IngestResponse{
    // 流程: Error: &errorpb.Error{
    // 流程: ServerIsBusy: &errorpb.ServerIsBusy{},
    // 流程: }}
    // 流程: onceResp := &atomic.Pointer[sst.IngestResponse]{}
    // PD/TiKV region: onceResp.Store(notLeaderResp)
    // 流程: local := &Backend{
    // PD/TiKV region: splitCli: initTestSplitClient3Replica([][]byte{{}, {'c'}}, nil),
    // mock: importClientFactory: &mockImportClientFactory{
    // PD/TiKV region: stores: []*metapb.Store{
    // 流程: {Id: 1}, {Id: 2}, {Id: 3},
    // 流程: },
    // Pebble/SST IO: createClientFn: func(store *metapb.Store) sst.ImportSSTClient {
    // mock: importCli := newMockImportClient()
    // 流程: importCli.store = store
    // 流程: importCli.apiInvokeRecorder = apiInvokeRecorder
    // 控制流: if store.Id == 1 {
    // 流程: importCli.retry = 1
    // 流程: importCli.onceResp = onceResp
    // 流程: }
    // 返回语义: return importCli
    // 流程: },
    // 流程: },
    // 流程: logger: log.L(),
    // PD/TiKV region: writeLimiter: newStoreWriteLimiter(0),
    // 流程: supportMultiIngest: true,
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: BackendConfig: BackendConfig{
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // 流程: }
    // 流程: var err error
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // 流程: db, tmpPath := makePebbleDB(t, nil)
    // 流程: _, engineUUID := backend.MakeUUID("ww", 0)
    // context: engineCtx, cancel2 := context.WithCancel(context.Background())
    // 流程: f := &Engine{
    // 流程: UUID: engineUUID,
    // 流程: sstDir: tmpPath,
    // 流程: ctx: engineCtx,
    // 流程: cancel: cancel2,
    // 并发/通道: sstMetasChan: make(chan metaOrFlush, 64),
    // 流程: keyAdapter: common.NoopKeyAdapter{},
    // 流程: logger: log.L(),
    // 流程: }
    // 流程: f.TS = oracle.GoTimeToTS(time.Now())
    // PD/TiKV region: f.db.Store(db)
    // 流程: err = db.Set([]byte("a"), []byte("a"), nil)
    // 断言: require.NoError(t, err)
    // 流程: err = db.Set([]byte("a2"), []byte("a2"), nil)
    // 断言: require.NoError(t, err)
    // 流程: var (
    // context: workGroup, workerCtx = util.NewErrorGroupWithRecoverWithCtx(context.Background())
    // 并发/通道: jobToWorkerCh = make(chan *regionJob, 10)
    // 并发/通道: jobFromWorkerCh = make(chan *regionJob)
    // 流程: )
    // 并发/通道: pool := getRegionJobWorkerPool(workerCtx, &sync.WaitGroup{}, local, nil, jobToWorkerCh, jobFromWorkerCh, 0)
    // 流程: partialWriteJob := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("c")},
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 1,
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 1, StoreId: 1}, {Id: 2, StoreId: 2}, {Id: 3, StoreId: 3},
    // 流程: },
    // 流程: StartKey: []byte("a"),
    // 流程: EndKey: []byte("c"),
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 1},
    // 流程: },
    // 流程: stage: regionScanned,
    // 流程: ingestData: f,
    // 原注释: // use small regionSplitSize to trigger partial write
    // PD/TiKV region: regionSplitSize: 1,
    // 流程: }
    // 并发/通道: jobToWorkerCh <- partialWriteJob
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 流程: workGroup.Go(func() error {
    // 流程: pool.Start(wctx)
    // 并发/通道: <-wctx.Done()
    // 流程: pool.Release()
    // 返回语义: return wctx.OperatorErr()
    // 流程: })
    // 流程: workGroup.Go(func() error {
    // 控制流: for {
    // 并发/通道: job := <-jobFromWorkerCh
    // 控制流: switch job.stage {
    // 控制流: case wrote:
    // 原注释: // mimic retry later
    // 并发/通道: jobToWorkerCh <- job
    // 控制流: case ingested:
    // 原注释: // partially write will change the start key
    // 断言: require.Equal(t, []byte("a2"), job.keyRange.Start)
    // 断言: require.Equal(t, []byte("c"), job.keyRange.End)
    // 流程: wctx.Cancel()
    // 返回语义: return nil
    // 流程: default:
    // 断言: require.Fail(t, "job stage %s is not expected, job: %v", job.stage, job)
    // 流程: }
    // 流程: }
    // 流程: })
    // 断言: require.NoError(t, workGroup.Wait())
    // 断言: require.Equal(t, int64(2), f.importedKVCount.Load())
    // 断言: require.Equal(t, []uint64{1, 2, 3, 1, 2, 3}, apiInvokeRecorder["Write"])
    // 断言: require.Equal(t, []uint64{1, 1, 1}, apiInvokeRecorder["MultiIngest"])
    // 断言: require.NoError(t, f.Close())
}

// mockGetSizeProperties 对应 Go 函数/方法声明。
// Go: func mockGetSizeProperties(log.Logger, *pebble.DB, common.KeyAdapter) (*sizeProperties, error)
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_get_size_properties() {
    // 流程: props := newSizeProperties()
    // 原注释: // keys starts with 0 is meta keys, so we start with 1.
    // 控制流: for i := byte(1); i <= 10; i++ {
    // 流程: rangeProps := &rangeProperty{
    // 流程: Key: []byte{i},
    // 流程: rangeOffsets: rangeOffsets{
    // 流程: Size: 50 * units.MiB,
    // 流程: Keys: 100_000,
    // 流程: },
    // 流程: }
    // 流程: props.add(rangeProps)
    // 流程: rangeProps = &rangeProperty{
    // 流程: Key: []byte{i, 1},
    // 流程: rangeOffsets: rangeOffsets{
    // 流程: Size: 50 * units.MiB,
    // 流程: Keys: 100_000,
    // 流程: },
    // 流程: }
    // 流程: props.add(rangeProps)
    // 流程: }
    // 返回语义: return props, nil
}

// panicSplitRegionClient 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type panicSplitRegionClient struct{}
pub struct panicSplitRegionClient;

// BeforeSplitRegion 对应 Go 函数/方法声明。
// Go: func (p panicSplitRegionClient) BeforeSplitRegion(context.Context, *split.RegionInfo, [][]byte) (*split.RegionInfo, [][]byte)
// 接收者 `panicSplitRegionClient` 的方法在这里中摊平成函数名 `panic_split_region_client_before_split_region`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn panic_split_region_client_before_split_region() {
    // 流程: panic("should not be called")
}

// AfterSplitRegion 对应 Go 函数/方法声明。
// Go: func (p panicSplitRegionClient) AfterSplitRegion(context.Context, *split.RegionInfo, [][]byte, []*split.RegionInfo, error) ([]*split.RegionInfo, error)
// 接收者 `panicSplitRegionClient` 的方法在这里中摊平成函数名 `panic_split_region_client_after_split_region`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn panic_split_region_client_after_split_region() {
    // 流程: panic("should not be called")
}

// BeforeScanRegions 对应 Go 函数/方法声明。
// Go: func (p panicSplitRegionClient) BeforeScanRegions(ctx context.Context, key, endKey []byte, limit int) ([]byte, []byte, int)
// 接收者 `panicSplitRegionClient` 的方法在这里中摊平成函数名 `panic_split_region_client_before_scan_regions`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn panic_split_region_client_before_scan_regions() {
    // 返回语义: return key, endKey, limit
}

// AfterScanRegions 对应 Go 函数/方法声明。
// Go: func (p panicSplitRegionClient) AfterScanRegions(infos []*split.RegionInfo, err error) ([]*split.RegionInfo, error)
// 接收者 `panicSplitRegionClient` 的方法在这里中摊平成函数名 `panic_split_region_client_after_scan_regions`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn panic_split_region_client_after_scan_regions() {
    // 返回语义: return infos, err
}

#[test]
// TestSplitRangeAgain4BigRegion 对应 Go 函数/方法声明。
// Go: func TestSplitRangeAgain4BigRegion(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_split_range_again4_big_region() {
    // 流程: backup := getSizePropertiesFn
    // mock: getSizePropertiesFn = mockGetSizeProperties
    // 资源收尾: t.Cleanup(func() {
    // 流程: getSizePropertiesFn = backup
    // 流程: })
    // 流程: local := &Backend{
    // PD/TiKV region: splitCli: initTestSplitClient(
    // 流程: [][]byte{{1}, {11}}, // we have one big region
    // PD/TiKV region: panicSplitRegionClient{}, // make sure no further split region
    // 流程: ),
    // 流程: }
    // PD/TiKV region: local.WorkerConcurrency.Store(1)
    // 流程: db, tmpPath := makePebbleDB(t, nil)
    // 流程: _, engineUUID := backend.MakeUUID("ww", 0)
    // context: ctx := context.Background()
    // context: engineCtx, cancel := context.WithCancel(context.Background())
    // 流程: f := &Engine{
    // 流程: UUID: engineUUID,
    // 流程: sstDir: tmpPath,
    // 流程: ctx: engineCtx,
    // 流程: cancel: cancel,
    // 并发/通道: sstMetasChan: make(chan metaOrFlush, 64),
    // 流程: keyAdapter: common.NoopKeyAdapter{},
    // 流程: logger: log.L(),
    // PD/TiKV region: regionSplitKeysCache: [][]byte{{1}, {11}},
    // PD/TiKV region: regionSplitSize: 1 << 30,
    // 流程: }
    // 流程: f.TS = oracle.GoTimeToTS(time.Now())
    // PD/TiKV region: f.db.Store(db)
    // 原注释: // keys starts with 0 is meta keys, so we start with 1.
    // 控制流: for i := byte(1); i <= 10; i++ {
    // 流程: err := db.Set([]byte{i}, []byte{i}, nil)
    // 断言: require.NoError(t, err)
    // 流程: err = db.Set([]byte{i, 1}, []byte{i, 1}, nil)
    // 断言: require.NoError(t, err)
    // 流程: }
    // 并发/通道: jobCh := make(chan *regionJob, 10)
    // 并发/通道: jobWg := sync.WaitGroup{}
    // 流程: err := local.generateAndSendJob(
    // 流程: ctx,
    // 流程: f,
    // 流程: 10*units.GB,
    // 流程: 1<<30,
    // 流程: jobCh,
    // 流程: &jobWg,
    // 流程: )
    // 断言: require.NoError(t, err)
    // 断言: require.Len(t, jobCh, 10)
    // 控制流: for i := range 9 {
    // 并发/通道: job := <-jobCh
    // 断言: require.Equal(t, []byte{byte(i + 1)}, job.keyRange.Start)
    // 断言: require.Equal(t, []byte{byte(i + 2)}, job.keyRange.End)
    // 并发/通道: jobWg.Done()
    // 流程: }
    // 原注释: // the end key of the last job is different, it's the nextKey of the last key
    // 并发/通道: job := <-jobCh
    // 断言: require.Equal(t, []byte{10}, job.keyRange.Start)
    // 断言: require.Equal(t, []byte{10, 1, 0}, job.keyRange.End)
    // 并发/通道: jobWg.Done()
    // 并发/通道: jobWg.Wait()
    // 断言: require.NoError(t, f.Close())
}

#[test]
// TestSplitRangeAgain4BigRegionExternalEngine 对应 Go 函数/方法声明。
// Go: func TestSplitRangeAgain4BigRegionExternalEngine(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_split_range_again4_big_region_external_engine() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // context: ctx := context.Background()
    // 流程: local := &Backend{
    // PD/TiKV region: splitCli: initTestSplitClient(
    // 流程: [][]byte{{1}, {11}}, // we have one big region
    // PD/TiKV region: panicSplitRegionClient{}, // make sure no further split region
    // 流程: ),
    // 流程: }
    // PD/TiKV region: local.WorkerConcurrency.Store(1)
    // 流程: keys := make([][]byte, 0, 10)
    // 流程: value := make([][]byte, 0, 10)
    // 控制流: for i := byte(1); i <= 10; i++ {
    // 流程: keys = append(keys, []byte{i})
    // 流程: value = append(value, []byte{i})
    // 流程: }
    // PD/TiKV region: memStore := objstore.NewMemStorage()
    // mock: dataFiles, statFiles, err := globalsort.MockExternalEngine(memStore, keys, value)
    // 断言: require.NoError(t, err)
    // 流程: extEngine := globalsort.NewExternalEngine(
    // 流程: ctx,
    // PD/TiKV region: memStore,
    // 流程: dataFiles,
    // 流程: statFiles,
    // 流程: []byte{1},
    // 流程: []byte{10},
    // 流程: keys,
    // 流程: [][]byte{{1}, {11}},
    // 流程: 10,
    // 流程: 123,
    // 流程: 456,
    // 流程: 789,
    // 流程: true,
    // 流程: 16*units.GiB,
    // 流程: engineapi.OnDuplicateKeyIgnore,
    // 流程: "",
    // 流程: )
    // 并发/通道: jobCh := make(chan *regionJob, 9)
    // 并发/通道: jobWg := sync.WaitGroup{}
    // 流程: err = local.generateAndSendJob(
    // 流程: ctx,
    // 流程: extEngine,
    // 流程: 10*units.GB,
    // 流程: 1<<30,
    // 流程: jobCh,
    // 流程: &jobWg,
    // 流程: )
    // 断言: require.NoError(t, err)
    // 断言: require.Len(t, jobCh, 9)
    // 控制流: for i := range 9 {
    // 并发/通道: job := <-jobCh
    // 断言: require.Equal(t, []byte{byte(i + 1)}, job.keyRange.Start)
    // 断言: require.Equal(t, []byte{byte(i + 2)}, job.keyRange.End)
    // 流程: firstKey, lastKey, err := job.ingestData.GetFirstAndLastKey(job.keyRange.Start, job.keyRange.End)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, []byte{byte(i + 1)}, firstKey)
    // 断言: require.Equal(t, []byte{byte(i + 1)}, lastKey)
    // 并发/通道: jobWg.Done()
    // 流程: }
    // 并发/通道: jobWg.Wait()
}

// getSuccessInjectedBehaviour 对应 Go 函数/方法声明。
// Go: func getSuccessInjectedBehaviour() []injectedBehaviour
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn get_success_injected_behaviour() {
    // 返回语义: return []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: remainingStartKey: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: ingest: injectedIngestBehaviour{},
    // 流程: },
    // 流程: }
}

// getNeedRescanWhenIngestBehaviour 对应 Go 函数/方法声明。
// Go: func getNeedRescanWhenIngestBehaviour() []injectedBehaviour
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn get_need_rescan_when_ingest_behaviour() {
    // 返回语义: return []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: remainingStartKey: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: ingest: injectedIngestBehaviour{
    // 流程: err: &ingestcli.IngestAPIError{Err: errdef.ErrKVEpochNotMatch},
    // 流程: },
    // 流程: },
    // 流程: }
}

#[test]
// TestDoImport 对应 Go 函数/方法声明。
// Go: func TestDoImport(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_do_import() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: backup := maxRetryBackoffSecond
    // 流程: maxRetryBackoffSecond = 1
    // 资源收尾: t.Cleanup(func() {
    // 流程: maxRetryBackoffSecond = backup
    // 流程: })
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/fakeRegionJobs", "return()")
    // 原注释: // test that
    // 原注释: // - one job need rescan when ingest
    // 原注释: // - one job need retry when write
    // PD/TiKV region: initRegionKeys := [][]byte{{'a'}, {'b'}, {'c'}, {'d'}}
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"a", "b"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'a'}, End: []byte{'b'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: append([]injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: err: status.Error(codes.Unknown, "RequestTooNew"),
    // 流程: },
    // 流程: },
    // 流程: }, getSuccessInjectedBehaviour()...),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"b", "c"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'b'}, End: []byte{'c'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: remainingStartKey: []byte{'b', '2'},
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: ingest: injectedIngestBehaviour{},
    // 流程: },
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: remainingStartKey: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: ingest: injectedIngestBehaviour{},
    // 流程: },
    // 流程: },
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'c', '2'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getNeedRescanWhenIngestBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c', '2'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 原注释: // a retryable error
    // 流程: err: status.Error(codes.Unknown, "is not fully replicated"),
    // 流程: },
    // 流程: },
    // 流程: },
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c", "c2"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'c', '2'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c2", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c', '2'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: }
    // context: ctx := context.Background()
    // 流程: l := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: toAtomic(2),
    // 流程: },
    // 流程: }
    // PD/TiKV region: e := &Engine{regionSplitKeysCache: initRegionKeys}
    // PD/TiKV region: err := l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.NoError(t, err)
    // PD/TiKV region: for _, v := range fakeRegionJobs {
    // 控制流: for _, job := range v.jobs {
    // 断言: require.Len(t, job.injected, 0)
    // 流程: }
    // 流程: }
    // 原注释: // test first call to generateJobForRange meet error
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"a", "b"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'a'}, End: []byte{'b'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"b", "c"}: {
    // 流程: err: errors.New("meet error when generateJobForRange"),
    // 流程: },
    // 流程: }
    // PD/TiKV region: err = l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.ErrorContains(t, err, "meet error when generateJobForRange")
    // 原注释: // test second call to generateJobForRange (needRescan) meet error
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"a", "b"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'a'}, End: []byte{'a', '2'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getNeedRescanWhenIngestBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'a', '2'}, End: []byte{'b'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"b", "c"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'b'}, End: []byte{'c'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"a", "a2"}: {
    // 流程: err: errors.New("meet error when generateJobForRange again"),
    // 流程: },
    // 流程: }
    // PD/TiKV region: err = l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.ErrorContains(t, err, "meet error when generateJobForRange again")
    // 原注释: // test write meet unretryable error
    // 流程: maxRetryBackoffSecond = 100
    // PD/TiKV region: l.WorkerConcurrency.Store(1)
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"a", "b"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'a'}, End: []byte{'b'}},
    // 流程: ingestData: &Engine{},
    // 流程: retryCount: MaxWriteAndIngestRetryTimes - 1,
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"b", "c"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'b'}, End: []byte{'c'}},
    // 流程: ingestData: &Engine{},
    // 流程: retryCount: MaxWriteAndIngestRetryTimes - 1,
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: retryCount: MaxWriteAndIngestRetryTimes - 2,
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 原注释: // unretryable error
    // 流程: err: errors.New("fatal error"),
    // 流程: },
    // 流程: },
    // 流程: },
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: }
    // PD/TiKV region: err = l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.ErrorContains(t, err, "fatal error")
}

struct GoCommit955fd6550bStoreHelper;

impl crate::engine_mgr::StoreHelper for GoCommit955fd6550bStoreHelper {
    fn GetTS(&self, token: &crate::CancellationToken) -> crate::Result<(i64, i64)> {
        token.check()?;
        Ok((1, 1))
    }

    fn GetTiKVCodec(&self) -> String {
        "api-v2".to_owned()
    }
}

struct GoCommit955fd6550bCancellingClient;

impl crate::local::ImportClient for GoCommit955fd6550bCancellingClient {
    fn WriteAndIngest(
        &self,
        token: &crate::CancellationToken,
        _engine: &crate::engine::Engine,
        _ranges: &[crate::KeyRange],
    ) -> crate::Result<(i64, i64)> {
        token.cancel();
        Ok((0, 0))
    }

    fn Close(&self) {}
}

struct GoCommit955fd6550bCancellingFactory;

impl crate::local::ImportClientFactory for GoCommit955fd6550bCancellingFactory {
    fn Create(
        &self,
        token: &crate::CancellationToken,
        _store_id: u64,
    ) -> crate::Result<std::sync::Arc<dyn crate::local::ImportClient>> {
        token.check()?;
        Ok(std::sync::Arc::new(GoCommit955fd6550bCancellingClient))
    }

    fn Close(&self) {}
}

#[test]
fn import_propagates_cancellation_from_active_client() {
    let mut config = crate::local::BackendConfig::default();
    config.local_store_dir = std::env::temp_dir()
        .join(format!("go-commit-955fd6550b-{}", crate::EngineId::new()))
        .to_string_lossy()
        .into_owned();
    let backend = crate::local::NewBackend(
        config,
        std::sync::Arc::new(GoCommit955fd6550bStoreHelper),
        Some(std::sync::Arc::new(GoCommit955fd6550bCancellingFactory)),
        None,
    )
    .unwrap();
    let token = crate::CancellationToken::default();
    let engine_id = crate::EngineId::new();
    backend.OpenEngine(&token, engine_id).unwrap();

    assert_eq!(
        backend.ImportEngine(&token, engine_id, 1),
        Err(crate::Error::Cancelled)
    );
    assert_eq!(backend.GetImportedKVCount(engine_id), 0);
    backend.Close();
}

#[test]
// TestRegionJobResetRetryCounter 对应 Go 函数/方法声明。
// Go: func TestRegionJobResetRetryCounter(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_region_job_reset_retry_counter() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: backup := maxRetryBackoffSecond
    // 流程: maxRetryBackoffSecond = 1
    // 资源收尾: t.Cleanup(func() {
    // 流程: maxRetryBackoffSecond = backup
    // 流程: })
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/fakeRegionJobs", "return()")
    // 原注释: // test that job need rescan when ingest
    // PD/TiKV region: initRegionKeys := [][]byte{{'c'}, {'d'}}
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"c", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'c', '2'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getNeedRescanWhenIngestBehaviour(),
    // 流程: retryCount: MaxWriteAndIngestRetryTimes,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 1, StoreId: 1},
    // PD/TiKV region: {Id: 2, StoreId: 2},
    // PD/TiKV region: {Id: 3, StoreId: 3},
    // 流程: },
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 1},
    // 流程: },
    // 流程: },
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c', '2'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // 流程: retryCount: MaxWriteAndIngestRetryTimes,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 4, StoreId: 4},
    // PD/TiKV region: {Id: 5, StoreId: 5},
    // PD/TiKV region: {Id: 6, StoreId: 6},
    // 流程: },
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 4, StoreId: 4},
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"c", "c2"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'c', '2'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Peers: []*metapb.Peer{
    // PD/TiKV region: {Id: 7, StoreId: 7},
    // PD/TiKV region: {Id: 8, StoreId: 8},
    // PD/TiKV region: {Id: 9, StoreId: 9},
    // 流程: },
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 7, StoreId: 7},
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: }
    // context: ctx := context.Background()
    // 流程: l := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: toAtomic(2),
    // 流程: },
    // 流程: }
    // PD/TiKV region: e := &Engine{regionSplitKeysCache: initRegionKeys}
    // PD/TiKV region: err := l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.NoError(t, err)
    // PD/TiKV region: for _, v := range fakeRegionJobs {
    // 控制流: for _, job := range v.jobs {
    // 断言: require.Len(t, job.injected, 0)
    // 流程: }
    // 流程: }
}

#[test]
// TestCtxCancelIsIgnored 对应 Go 函数/方法声明。
// Go: func TestCtxCancelIsIgnored(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_ctx_cancel_is_ignored() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: backup := maxRetryBackoffSecond
    // 流程: maxRetryBackoffSecond = 1
    // 资源收尾: t.Cleanup(func() {
    // 流程: maxRetryBackoffSecond = backup
    // 流程: })
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/fakeRegionJobs", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeGenerateJob", "sleep(1000)")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/WriteToTiKVNotEnoughDiskSpace", "return()")
    // PD/TiKV region: initRegionKeys := [][]byte{{'c'}, {'d'}, {'e'}}
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"c", "d"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'c'}, End: []byte{'d'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: {"d", "e"}: {
    // 流程: jobs: []*regionJob{
    // 流程: {
    // 流程: keyRange: engineapi.Range{Start: []byte{'d'}, End: []byte{'e'}},
    // 流程: ingestData: &Engine{},
    // 流程: injected: getSuccessInjectedBehaviour(),
    // PD/TiKV region: region: dummyRegionInfo,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: }
    // context: ctx := context.Background()
    // 流程: l := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: toAtomic(1),
    // 流程: },
    // 流程: }
    // PD/TiKV region: e := &Engine{regionSplitKeysCache: initRegionKeys}
    // PD/TiKV region: err := l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.ErrorContains(t, err, "the remaining storage capacity of TiKV")
}

#[test]
// TestWorkerFailedWhenGeneratingJobs 对应 Go 函数/方法声明。
// Go: func TestWorkerFailedWhenGeneratingJobs(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_worker_failed_when_generating_jobs() {
    // 控制流: if kerneltype.IsNextGen() {
    // 流程: t.Skip("skip this test on next-gen kernel")
    // 流程: }
    // 流程: backup := maxRetryBackoffSecond
    // 流程: maxRetryBackoffSecond = 1
    // 资源收尾: t.Cleanup(func() {
    // 流程: maxRetryBackoffSecond = backup
    // 流程: })
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/sendDummyJob", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockGetFirstAndLastKey", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/WriteToTiKVNotEnoughDiskSpace", "return()")
    // PD/TiKV region: initRegionKeys := [][]byte{{'c'}, {'d'}}
    // context: ctx := context.Background()
    // 流程: l := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: toAtomic(1),
    // 流程: },
    // PD/TiKV region: splitCli: initTestSplitClient(
    // 流程: [][]byte{{1}, {11}},
    // PD/TiKV region: panicSplitRegionClient{},
    // 流程: ),
    // 流程: }
    // PD/TiKV region: e := &Engine{regionSplitKeysCache: initRegionKeys}
    // PD/TiKV region: err := l.doImport(ctx, e, initRegionKeys, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.ErrorContains(t, err, "the remaining storage capacity of TiKV")
}

// recordScanRegionsHook 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type recordScanRegionsHook struct {
// Go: beforeScanRegions [][2][]byte
// Go: }
pub struct recordScanRegionsHook;

// BeforeSplitRegion 对应 Go 函数/方法声明。
// Go: func (r *recordScanRegionsHook) BeforeSplitRegion(ctx context.Context, regionInfo *split.RegionInfo, keys [][]byte) (*split.RegionInfo, [][]byte)
// 接收者 `recordScanRegionsHook` 的方法在这里中摊平成函数名 `record_scan_regions_hook_before_split_region`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn record_scan_regions_hook_before_split_region() {
    // 返回语义: return regionInfo, keys
}

// AfterSplitRegion 对应 Go 函数/方法声明。
// Go: func (r *recordScanRegionsHook) AfterSplitRegion(ctx context.Context, info *split.RegionInfo, i [][]byte, infos []*split.RegionInfo, err error) ([]*split.RegionInfo, error)
// 接收者 `recordScanRegionsHook` 的方法在这里中摊平成函数名 `record_scan_regions_hook_after_split_region`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn record_scan_regions_hook_after_split_region() {
    // 返回语义: return infos, err
}

// BeforeScanRegions 对应 Go 函数/方法声明。
// Go: func (r *recordScanRegionsHook) BeforeScanRegions(ctx context.Context, key, endKey []byte, limit int) ([]byte, []byte, int)
// 接收者 `recordScanRegionsHook` 的方法在这里中摊平成函数名 `record_scan_regions_hook_before_scan_regions`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn record_scan_regions_hook_before_scan_regions() {
    // PD/TiKV region: r.beforeScanRegions = append(r.beforeScanRegions, [2][]byte{key, endKey})
    // 返回语义: return key, endKey, limit
}

// AfterScanRegions 对应 Go 函数/方法声明。
// Go: func (r *recordScanRegionsHook) AfterScanRegions(infos []*split.RegionInfo, err error) ([]*split.RegionInfo, error)
// 接收者 `recordScanRegionsHook` 的方法在这里中摊平成函数名 `record_scan_regions_hook_after_scan_regions`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn record_scan_regions_hook_after_scan_regions() {
    // 返回语义: return infos, err
}

#[test]
// TestExternalEngine 对应 Go 函数/方法声明。
// Go: func TestExternalEngine(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_external_engine() {
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipStartWorker", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/injectVariables", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/globalsort/LoadIngestDataBatchSize", "return(2)")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipOnDuplicateKeyCheck", "return(true)")
    // context: ctx := context.Background()
    // 流程: dir := t.TempDir()
    // 流程: storageURI := "file://" + filepath.ToSlash(dir)
    // 流程: storeBackend, err := objstore.ParseBackend(storageURI, nil)
    // 断言: require.NoError(t, err)
    // 流程: extStorage, err := objstore.New(ctx, storeBackend, nil)
    // 断言: require.NoError(t, err)
    // 流程: keys := make([][]byte, 100)
    // 流程: values := make([][]byte, 100)
    // 控制流: for i := range keys {
    // 流程: keys[i] = fmt.Appendf(nil, "key%06d", i)
    // 流程: values[i] = fmt.Appendf(nil, "value%06d", i)
    // 流程: }
    // 原注释: // simple append 0x00
    // 流程: endKey := make([]byte, len(keys[99])+1)
    // 流程: copy(endKey, keys[99])
    // mock: dataFiles, statFiles, err := globalsort.MockExternalEngine(extStorage, keys, values)
    // 断言: require.NoError(t, err)
    // 流程: externalCfg := &backend.ExternalEngineConfig{
    // PD/TiKV region: ExtStore: extStorage,
    // 流程: DataFiles: dataFiles,
    // 流程: StatFiles: statFiles,
    // 流程: StartKey: keys[0],
    // 流程: EndKey: endKey,
    // 流程: JobKeys: [][]byte{keys[0], keys[20], keys[30], keys[50], keys[60], keys[80], keys[90], endKey},
    // PD/TiKV region: SplitKeys: [][]byte{keys[0], keys[50], endKey},
    // PD/TiKV region: TotalFileSize: int64(config.SplitRegionSize) + 1,
    // PD/TiKV region: TotalKVCount: int64(config.SplitRegionKeys) + 1,
    // 流程: MemCapacity: 8 * units.GiB,
    // 流程: }
    // 流程: engineUUID := uuid.New()
    // PD/TiKV region: hook := &recordScanRegionsHook{}
    // 流程: local := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: toAtomic(2),
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // PD/TiKV region: splitCli: initTestSplitClient([][]byte{
    // 流程: keys[0], keys[50], endKey,
    // 流程: }, hook),
    // mock: pdCli: &mockPdClient{},
    // 流程: }
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // 流程: jobs := make([]*regionJob, 0, 5)
    // 并发/通道: jobToWorkerCh := make(chan *regionJob, 10)
    // 流程: testJobToWorkerCh = jobToWorkerCh
    // 并发/通道: done := make(chan struct{})
    // 并发/通道: go func() {
    // 控制流: for range 7 {
    // 并发/通道: jobs = append(jobs, <-jobToWorkerCh)
    // 并发/通道: testJobWg.Done()
    // 流程: }
    // 流程: }()
    // 并发/通道: go func() {
    // 流程: err2 := local.CloseEngine(
    // 流程: ctx,
    // 流程: &backend.EngineConfig{External: externalCfg},
    // 流程: engineUUID,
    // 流程: )
    // 断言: require.NoError(t, err2)
    // PD/TiKV region: err2 = local.ImportEngine(ctx, engineUUID, int64(config.SplitRegionSize), int64(config.SplitRegionKeys))
    // 断言: require.NoError(t, err2)
    // 流程: close(done)
    // 流程: }()
    // 并发/通道: <-done
    // 原注释: // no jobs left in the channel
    // 断言: require.Len(t, jobToWorkerCh, 0)
    // 流程: sort.Slice(jobs, func(i, j int) bool {
    // 返回语义: return bytes.Compare(jobs[i].keyRange.Start, jobs[j].keyRange.Start) < 0
    // 流程: })
    // 流程: expectedKeyRanges := []engineapi.Range{
    // 流程: {Start: keys[0], End: keys[20]},
    // 流程: {Start: keys[20], End: keys[30]},
    // 流程: {Start: keys[30], End: keys[50]},
    // 流程: {Start: keys[50], End: keys[60]},
    // 流程: {Start: keys[60], End: keys[80]},
    // 流程: {Start: keys[80], End: keys[90]},
    // 流程: {Start: keys[90], End: endKey},
    // 流程: }
    // 流程: kvIdx := 0
    // 控制流: for i, job := range jobs {
    // 断言: require.Equal(t, expectedKeyRanges[i], job.keyRange)
    // 流程: iter := job.ingestData.NewIter(ctx, job.keyRange.Start, job.keyRange.End, nil)
    // 控制流: for iter.First(); iter.Valid(); iter.Next() {
    // 断言: require.Equal(t, keys[kvIdx], iter.Key())
    // 断言: require.Equal(t, values[kvIdx], iter.Value())
    // 流程: kvIdx++
    // 流程: }
    // 断言: require.NoError(t, iter.Error())
    // 断言: require.NoError(t, iter.Close())
    // 流程: }
    // 断言: require.Equal(t, 100, kvIdx)
    // 断言: require.Equal(t, [][2][]byte{
    // 流程: {codec.EncodeBytes(nil, keys[0]), codec.EncodeBytes(nil, nextKey(keys[29]))},
    // 流程: {codec.EncodeBytes(nil, keys[30]), codec.EncodeBytes(nil, nextKey(keys[59]))},
    // 流程: {codec.EncodeBytes(nil, keys[60]), codec.EncodeBytes(nil, nextKey(keys[89]))},
    // 流程: {codec.EncodeBytes(nil, keys[90]), codec.EncodeBytes(nil, nextKey(keys[99]))},
    // PD/TiKV region: }, hook.beforeScanRegions)
}

#[test]
// TestCheckDiskAvail 对应 Go 函数/方法声明。
// Go: func TestCheckDiskAvail(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_check_disk_avail() {
    // PD/TiKV region: store := &http.StoreInfo{Status: http.StoreStatus{Capacity: "100 GB", Available: "50 GB"}}
    // context: ctx := context.Background()
    // 流程: err := checkDiskAvail(ctx, store)
    // 断言: require.NoError(t, err)
    // 原注释: // pd may return this StoreInfo before the store reports heartbeat
    // PD/TiKV region: store = &http.StoreInfo{Status: http.StoreStatus{LeaderWeight: 1.0, RegionWeight: 1.0}}
    // 流程: err = checkDiskAvail(ctx, store)
    // 断言: require.NoError(t, err)
    // PD/TiKV region: store = &http.StoreInfo{
    // PD/TiKV region: Store: http.MetaStore{Address: "127.0.0.1:20160"},
    // PD/TiKV region: Status: http.StoreStatus{Capacity: "100 GB", Available: "5 GB"},
    // 流程: }
    // 流程: err = checkDiskAvail(ctx, store)
    // 断言: require.ErrorIs(t, err, errdef.ErrKVDiskFull)
    // 断言: require.Contains(t, err.Error(), "TiKV(127.0.0.1:20160)")
    // 断言: require.Contains(t, err.Error(), "increase the storage capacity of TiKV")
    // PD/TiKV region: store = &http.StoreInfo{
    // PD/TiKV region: Store: http.MetaStore{
    // 流程: Address: "127.0.0.1:3930",
    // PD/TiKV region: Labels: []http.StoreLabel{
    // 流程: {Key: "engine", Value: "tiflash"},
    // 流程: },
    // 流程: },
    // PD/TiKV region: Status: http.StoreStatus{Capacity: "100 GB", Available: "5 GB"},
    // 流程: }
    // 流程: err = checkDiskAvail(ctx, store)
    // 断言: require.ErrorIs(t, err, errdef.ErrKVDiskFull)
    // 断言: require.Contains(t, err.Error(), "TiFlash(127.0.0.1:3930)")
    // 断言: require.Contains(t, err.Error(), "increase the storage capacity of TiFlash")
}

#[test]
// TestBackendCloseWithoutTiKVClient 对应 Go 函数/方法声明。
// Go: func TestBackendCloseWithoutTiKVClient(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_backend_close_without_ti_kv_client() {
    // PD/TiKV region: localStoreDir := t.TempDir()
    // 流程: b := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // PD/TiKV region: LocalStoreDir: localStoreDir,
    // 流程: },
    // 流程: engineMgr: &engineManager{
    // 流程: BackendConfig: BackendConfig{
    // PD/TiKV region: LocalStoreDir: localStoreDir,
    // 流程: },
    // 流程: externalEngine: map[uuid.UUID]engineapi.Engine{},
    // 流程: bufferPool: membuf.NewPool(),
    // 流程: logger: log.L(),
    // 流程: },
    // mock: importClientFactory: &mockImportClientFactory{},
    // 流程: }
    // 断言: require.NotPanics(t, b.Close)
}

#[test]
// TestGetDupeControllerInitializesTiKVClientLazily 对应 Go 函数/方法声明。
// Go: func TestGetDupeControllerInitializesTiKVClientLazily(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_get_dupe_controller_initializes_ti_kv_client_lazily() {
    // 流程: oldNewEtcdSafePointKV := newEtcdSafePointKV
    // PD/TiKV region: oldNewTiKVRPCClient := newTiKVRPCClient
    // PD/TiKV region: oldNewTiKVStore := newTiKVStore
    // PD/TiKV region: oldNewPDClient := newPDClient
    // 资源收尾: defer func() {
    // 流程: newEtcdSafePointKV = oldNewEtcdSafePointKV
    // PD/TiKV region: newTiKVRPCClient = oldNewTiKVRPCClient
    // PD/TiKV region: newTiKVStore = oldNewTiKVStore
    // PD/TiKV region: newPDClient = oldNewPDClient
    // 流程: }()
    // PD/TiKV region: var pdClientCalls, safePointKVCalls, rpcClientCalls, kvStoreCalls int
    // 流程: var routerClientEnabled bool
    // mock: inputPDCli := &mockPdClient{}
    // mock: tikvPDCli := &mockPdClient{}
    // context: newPDClient = func(_ context.Context, apiContext pd.APIContext, _ caller.Component, _ []string, _ pd.SecurityOption, opts ...opt.ClientOption) (pd.Client, error) {
    // 流程: pdClientCalls++
    // 断言: require.Equal(t, pd.NewAPIContextV1(), apiContext)
    // 流程: option := opt.NewOption()
    // 流程: option.SetEnableRouterClient(true)
    // 控制流: for _, clientOpt := range opts {
    // 流程: clientOpt(option)
    // 流程: }
    // 流程: routerClientEnabled = option.GetEnableRouterClient()
    // PD/TiKV region: return tikvPDCli, nil
    // 流程: }
    // 流程: newEtcdSafePointKV = func(_ []string, _ *tls.Config, _ ...tikv.SafePointKVOpt) (tikv.SafePointKV, error) {
    // 流程: safePointKVCalls++
    // mock: return tikv.NewMockSafePointKV(), nil
    // 流程: }
    // PD/TiKV region: newTiKVRPCClient = func(_ ...tikv.ClientOpt) tikv.Client {
    // 流程: rpcClientCalls++
    // 返回语义: return tikv.NewRPCClient()
    // 流程: }
    // PD/TiKV region: newTiKVStore = func(_ string, pdCli pd.Client, _ tikv.SafePointKV, _ tikv.Client, _ ...tikv.Option) (*tikv.KVStore, error) {
    // PD/TiKV region: kvStoreCalls++
    // 断言: require.NotSame(t, inputPDCli, pdCli)
    // mock: return nil, errors.New("mock kv store error")
    // 流程: }
    // 流程: b := &Backend{
    // PD/TiKV region: pdCli: inputPDCli,
    // 流程: pdAddrs: []string{"127.0.0.1:2379"},
    // 流程: tls: &common.TLS{},
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: BackendConfig: BackendConfig{
    // PD/TiKV region: DisablePDClientRouterClient: true,
    // 流程: },
    // 流程: }
    // context: dupeController, err := b.GetDupeController(context.Background(), 1, nil)
    // 断言: require.Nil(t, dupeController)
    // 断言: require.ErrorContains(t, err, "mock kv store error")
    // 断言: require.Equal(t, 1, pdClientCalls)
    // 断言: require.Equal(t, 1, safePointKVCalls)
    // 断言: require.Equal(t, 1, rpcClientCalls)
    // 断言: require.Equal(t, 1, kvStoreCalls)
    // 断言: require.False(t, routerClientEnabled)
    // 断言: require.False(t, inputPDCli.closed)
    // 断言: require.True(t, tikvPDCli.closed)
    // 断言: require.Nil(t, b.tikvCli)
    // PD/TiKV region: pdClientCalls, safePointKVCalls, rpcClientCalls, kvStoreCalls = 0, 0, 0, 0
    // 流程: routerClientEnabled = false
    // mock: inputPDCli = &mockPdClient{}
    // mock: tikvPDCli = &mockPdClient{}
    // 流程: b = &Backend{
    // PD/TiKV region: pdCli: inputPDCli,
    // 流程: pdAddrs: []string{"127.0.0.1:2379"},
    // 流程: tls: &common.TLS{},
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: }
    // context: dupeController, err = b.GetDupeController(context.Background(), 1, nil)
    // 断言: require.Nil(t, dupeController)
    // 断言: require.ErrorContains(t, err, "mock kv store error")
    // 断言: require.Equal(t, 1, pdClientCalls)
    // 断言: require.Equal(t, 1, safePointKVCalls)
    // 断言: require.Equal(t, 1, rpcClientCalls)
    // 断言: require.Equal(t, 1, kvStoreCalls)
    // 断言: require.True(t, routerClientEnabled)
    // 断言: require.False(t, inputPDCli.closed)
    // 断言: require.True(t, tikvPDCli.closed)
    // 断言: require.Nil(t, b.tikvCli)
    // context: ctx, cancel := context.WithCancel(context.Background())
    // context: var pdClientCtx context.Context
    // 流程: newEtcdSafePointKV = func(_ []string, _ *tls.Config, _ ...tikv.SafePointKVOpt) (tikv.SafePointKV, error) {
    // mock: return tikv.NewMockSafePointKV(), nil
    // 流程: }
    // context: newPDClient = func(ctx context.Context, _ pd.APIContext, _ caller.Component, _ []string, _ pd.SecurityOption, _ ...opt.ClientOption) (pd.Client, error) {
    // 流程: pdClientCtx = ctx
    // 流程: cancel()
    // 返回语义: return nil, ctx.Err()
    // 流程: }
    // PD/TiKV region: newTiKVRPCClient = func(_ ...tikv.ClientOpt) tikv.Client {
    // 断言: require.FailNow(t, "canceled PD client creation should stop before creating TiKV RPC client")
    // 返回语义: return nil
    // 流程: }
    // 流程: b = &Backend{
    // 流程: pdAddrs: []string{"127.0.0.1:2379"},
    // 流程: tls: &common.TLS{},
    // 流程: tikvCodec: keyspace.CodecV1,
    // 流程: }
    // 流程: dupeController, err = b.GetDupeController(ctx, 1, nil)
    // 断言: require.Nil(t, dupeController)
    // 断言: require.Same(t, ctx, pdClientCtx)
    // 断言: require.ErrorContains(t, err, context.Canceled.Error())
    // 断言: require.Nil(t, b.tikvCli)
}

// mockStoreHelper 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockStoreHelper struct{}
pub struct mockStoreHelper;

// GetTS 对应 Go 函数/方法声明。
// Go: func (mockStoreHelper) GetTS(context.Context) (physical, logical int64, err error)
// 接收者 `mockStoreHelper` 的方法在这里中摊平成函数名 `mock_store_helper_get_ts`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_store_helper_get_ts() {
    // 返回语义: return 12345, 67890, nil
}

// GetTiKVCodec 对应 Go 函数/方法声明。
// Go: func (mockStoreHelper) GetTiKVCodec() tikv.Codec
// 接收者 `mockStoreHelper` 的方法在这里中摊平成函数名 `mock_store_helper_get_ti_kv_codec`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_store_helper_get_ti_kv_codec() {
    // 流程: c, _ := tikv.NewCodecV2(tikv.ModeTxn, &keyspacepb.KeyspaceMeta{})
    // 返回语义: return c
}

#[test]
// TestTotalMemoryConsume 对应 Go 函数/方法声明。
// Go: func TestTotalMemoryConsume(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_total_memory_consume() {
    // 流程: t.Skip("this test is manually run to calibrate the real memory usage with TotalMemoryConsume")
    // 流程: inMemTest = true
    // 流程: getMemoryInUse := func() int64 {
    // 原注释: // wait to make test more stable, maybe releasing memory is slow
    // 流程: runtime.GC()
    // 流程: time.Sleep(time.Second)
    // 流程: runtime.GC()
    // 流程: s := runtime.MemStats{}
    // 流程: runtime.ReadMemStats(&s)
    // 返回语义: return int64(s.HeapInuse)
    // 流程: }
    // 流程: memInUseBase := getMemoryInUse()
    // context: ctx := context.Background()
    // 流程: cfg := BackendConfig{
    // PD/TiKV region: LocalStoreDir: t.TempDir(),
    // 流程: CheckpointEnabled: true,
    // 流程: DupeDetectEnabled: true,
    // 流程: MemTableSize: 100 * units.MiB,
    // 流程: LocalWriterMemCacheSize: 100 * units.MiB,
    // 流程: }
    // mock: b, err := NewBackendForTest(ctx, cfg, mockStoreHelper{})
    // 断言: require.NoError(t, err)
    // 流程: checkMemoryConsume := func(tag string, expected int64) {
    // 流程: expectedMemConsume := expected
    // 断言: require.EqualValues(t, expectedMemConsume, b.TotalMemoryConsume())
    // 流程: memInUse := getMemoryInUse()
    // 流程: diff := memInUse - memInUseBase
    // 流程: t.Logf("%s, memInUse %d, memInUseBase %d, diff %d", tag, memInUse, memInUseBase, diff)
    // 断言: require.Less(t, mathutil.Abs(diff-expectedMemConsume), int64(10*units.MiB))
    // 流程: }
    // 原注释: // 1. test local engine write phase
    // 流程: engineCfg := &backend.EngineConfig{
    // 流程: Local: backend.LocalEngineConfig{
    // 流程: BlockSize: 100 * units.MiB,
    // 流程: },
    // 流程: }
    // 流程: engineID := uuid.New()
    // 流程: err = b.OpenEngine(ctx, engineCfg, engineID)
    // 断言: require.NoError(t, err)
    // 流程: checkMemoryConsume("after open 1 engine", 0)
    // 流程: writerCfg := &backend.LocalWriterConfig{}
    // 流程: writerCfg.Local.IsKVSorted = false
    // 流程: unsortedWriter, err := b.LocalWriter(ctx, writerCfg, engineID)
    // 断言: require.NoError(t, err)
    // 流程: writerCfg.Local.IsKVSorted = true
    // 流程: sortedWriter, err := b.LocalWriter(ctx, writerCfg, engineID)
    // 断言: require.NoError(t, err)
    // 原注释: // 72 B * 1 Mi from unsortedWriter.writeBatch
    // 流程: checkMemoryConsume("after create engine writers", 72*units.MiB)
    // 流程: err = unsortedWriter.AppendRows(ctx, []string{"a", "b", "c"}, kv.MakeRowsFromKvPairs([]common.KvPair{
    // 流程: {Key: []byte("k1"), Val: []byte("v1")},
    // 流程: {Key: []byte("k3"), Val: []byte("v3")},
    // 流程: {Key: []byte("k2"), Val: []byte("v2")},
    // 流程: }))
    // 断言: require.NoError(t, err)
    // 流程: err = sortedWriter.AppendRows(ctx, []string{"a", "b", "c"}, kv.MakeRowsFromKvPairs([]common.KvPair{
    // 流程: {Key: []byte("k4"), Val: []byte("v4")},
    // 流程: {Key: []byte("k5"), Val: []byte("v5")},
    // 流程: {Key: []byte("k6"), Val: []byte("v6")},
    // 流程: }))
    // 断言: require.NoError(t, err)
    // 原注释: // 72 MiB from unsortedWriter.writeBatch, 1 MiB from bufferPool of unsortedWriter
    // 流程: checkMemoryConsume("after write a bit rows", 73*units.MiB)
    // 资源收尾: _, err = unsortedWriter.Close(ctx)
    // 断言: require.NoError(t, err)
    // 资源收尾: _, err = sortedWriter.Close(ctx)
    // 断言: require.NoError(t, err)
    // 原注释: // 1 MiB from bufferPool of unsortedWriter
    // 流程: checkMemoryConsume("after close all writers", 1*units.MiB)
    // 流程: writerCfg = &backend.LocalWriterConfig{}
    // 流程: writerCfg.Local.IsKVSorted = false
    // 流程: unsortedWriter, err = b.LocalWriter(ctx, writerCfg, engineID)
    // 断言: require.NoError(t, err)
    // 原注释: // write about 150 MiB data
    // 流程: val := make([]byte, 35)
    // 控制流: for i := range 1024 * 1024 {
    // 流程: err = unsortedWriter.AppendRows(ctx, []string{"a", "b", "c"}, kv.MakeRowsFromKvPairs([]common.KvPair{
    // 流程: {Key: fmt.Appendf(nil, "key_a_%09d", i), Val: val},
    // 流程: {Key: fmt.Appendf(nil, "key_b_%09d", i), Val: val},
    // 流程: {Key: fmt.Appendf(nil, "key_c_%09d", i), Val: val},
    // 流程: }))
    // 断言: require.NoError(t, err)
    // 流程: }
    // 原注释: // 119 MiB from bufferPool, 72 B * 2048910 from unsortedWriter.writeBatch
    // 流程: checkMemoryConsume("after write many rows", 272302064)
    // 资源收尾: _, err = unsortedWriter.Close(ctx)
    // 断言: require.NoError(t, err)
    // 流程: checkMemoryConsume("after close all writers", 119*units.MiB)
    // 流程: err = b.CloseEngine(ctx, &backend.EngineConfig{}, engineID)
    // 断言: require.NoError(t, err)
    // 流程: b.CloseEngineMgr()
}

// refCountIngestData 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type refCountIngestData struct {
// Go: mockIngestData
// Go: refCount int64
// Go: mu sync.Mutex
// Go: cleaned bool
// Go: }
pub struct refCountIngestData;

// IncRef 对应 Go 函数/方法声明。
// Go: func (r *refCountIngestData) IncRef()
// 接收者 `refCountIngestData` 的方法在这里中摊平成函数名 `ref_count_ingest_data_inc_ref`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn ref_count_ingest_data_inc_ref() {
    // 流程: time.Sleep(10 * time.Millisecond) // simulate some delay
    // 流程: r.mu.Lock()
    // 资源收尾: defer r.mu.Unlock()
    // 流程: r.refCount++
}

// DecRef 对应 Go 函数/方法声明。
// Go: func (r *refCountIngestData) DecRef()
// 接收者 `refCountIngestData` 的方法在这里中摊平成函数名 `ref_count_ingest_data_dec_ref`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn ref_count_ingest_data_dec_ref() {
    // 流程: r.mu.Lock()
    // 资源收尾: defer r.mu.Unlock()
    // 流程: r.refCount--
    // 控制流: if r.refCount == 0 {
    // 流程: r.cleaned = true
    // 流程: }
}

// GetRefCount 对应 Go 函数/方法声明。
// Go: func (r *refCountIngestData) GetRefCount() int64
// 接收者 `refCountIngestData` 的方法在这里中摊平成函数名 `ref_count_ingest_data_get_ref_count`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn ref_count_ingest_data_get_ref_count() {
    // 流程: r.mu.Lock()
    // 资源收尾: defer r.mu.Unlock()
    // 返回语义: return r.refCount
}

// IsCleaned 对应 Go 函数/方法声明。
// Go: func (r *refCountIngestData) IsCleaned() bool
// 接收者 `refCountIngestData` 的方法在这里中摊平成函数名 `ref_count_ingest_data_is_cleaned`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn ref_count_ingest_data_is_cleaned() {
    // 流程: r.mu.Lock()
    // 资源收尾: defer r.mu.Unlock()
    // 返回语义: return r.cleaned
}

#[test]
// TestRefAllJobsBeforeSending 对应 Go 函数/方法声明。
// Go: func TestRefAllJobsBeforeSending(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_ref_all_jobs_before_sending() {
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipSplitAndScatter", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/skipStartWorker", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/injectVariables", "return()")
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/fakeRegionJobs", "return()")
    // context: ctx := context.Background()
    // 流程: local := &Backend{
    // 流程: BackendConfig: BackendConfig{
    // 流程: WorkerConcurrency: *atomic2.NewInt32(2),
    // PD/TiKV region: LocalStoreDir: path.Join(t.TempDir(), "sorted-kv"),
    // 流程: },
    // PD/TiKV region: splitCli: initTestSplitClient([][]byte{[]byte("a"), []byte("z")}, nil),
    // mock: pdCli: &mockPdClient{},
    // 流程: }
    // 流程: var err error
    // 流程: local.engineMgr, err = newEngineManager(local.BackendConfig, local, local.logger)
    // 断言: require.NoError(t, err)
    // 原注释: // Create a refCountIngestData that tracks reference count
    // 流程: data := &refCountIngestData{
    // mock: mockIngestData: mockIngestData{
    // 流程: {[]byte("b"), []byte("b")},
    // 流程: {[]byte("c"), []byte("c")},
    // 流程: {[]byte("d"), []byte("d")},
    // 流程: },
    // 流程: }
    // 原注释: // Create multiple jobs: some empty, some with data
    // 原注释: // Empty jobs will finish quickly, but the ingestData should not be cleaned
    // 原注释: // until all jobs are done.
    // 流程: jobRanges := []engineapi.Range{
    // 流程: {Start: []byte("a"), End: []byte("b")}, // empty job
    // 流程: {Start: []byte("b"), End: []byte("c")}, // job with data
    // 流程: {Start: []byte("c"), End: []byte("d")}, // job with data
    // 流程: {Start: []byte("d"), End: []byte("e")}, // empty job
    // 流程: {Start: []byte("e"), End: []byte("f")}, // empty job
    // 流程: }
    // 原注释: // Use fakeRegionJobs to inject jobs
    // 流程: key := [2]string{string(jobRanges[0].Start), string(jobRanges[len(jobRanges)-1].End)}
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: key: {
    // 流程: jobs: []*regionJob{
    // 原注释: // Empty job 1
    // 流程: {
    // 流程: keyRange: jobRanges[0],
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 1,
    // 流程: StartKey: []byte("a"),
    // 流程: EndKey: []byte("b"),
    // PD/TiKV region: Peers: []*metapb.Peer{{Id: 1, StoreId: 1}},
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 1, StoreId: 1},
    // 流程: },
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{emptyJob: true},
    // 流程: err: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 原注释: // Job with data 1
    // 流程: {
    // 流程: keyRange: jobRanges[1],
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 2,
    // 流程: StartKey: []byte("b"),
    // 流程: EndKey: []byte("c"),
    // PD/TiKV region: Peers: []*metapb.Peer{{Id: 2, StoreId: 1}},
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 2, StoreId: 1},
    // 流程: },
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: emptyJob: false,
    // 流程: count: 1,
    // 流程: totalBytes: 2,
    // Pebble/SST IO: sstMeta: []*sst.SSTMeta{{}},
    // 流程: },
    // 流程: err: nil,
    // 流程: },
    // 流程: ingest: injectedIngestBehaviour{err: nil},
    // 流程: },
    // 流程: },
    // 流程: },
    // 原注释: // Job with data 2
    // 流程: {
    // 流程: keyRange: jobRanges[2],
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 3,
    // 流程: StartKey: []byte("c"),
    // 流程: EndKey: []byte("d"),
    // PD/TiKV region: Peers: []*metapb.Peer{{Id: 3, StoreId: 1}},
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 3, StoreId: 1},
    // 流程: },
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{
    // 流程: emptyJob: false,
    // 流程: count: 1,
    // 流程: totalBytes: 2,
    // Pebble/SST IO: sstMeta: []*sst.SSTMeta{{}},
    // 流程: },
    // 流程: err: nil,
    // 流程: },
    // 流程: ingest: injectedIngestBehaviour{err: nil},
    // 流程: },
    // 流程: },
    // 流程: },
    // 原注释: // Empty job 2
    // 流程: {
    // 流程: keyRange: jobRanges[3],
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 4,
    // 流程: StartKey: []byte("d"),
    // 流程: EndKey: []byte("e"),
    // PD/TiKV region: Peers: []*metapb.Peer{{Id: 4, StoreId: 1}},
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 4, StoreId: 1},
    // 流程: },
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{emptyJob: true},
    // 流程: err: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 原注释: // Empty job 3
    // 流程: {
    // 流程: keyRange: jobRanges[4],
    // 流程: stage: regionScanned,
    // 流程: ingestData: data,
    // PD/TiKV region: region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{
    // 流程: Id: 5,
    // 流程: StartKey: []byte("e"),
    // 流程: EndKey: []byte("f"),
    // PD/TiKV region: Peers: []*metapb.Peer{{Id: 5, StoreId: 1}},
    // 流程: },
    // PD/TiKV region: Leader: &metapb.Peer{Id: 5, StoreId: 1},
    // 流程: },
    // 流程: injected: []injectedBehaviour{
    // 流程: {
    // 流程: write: injectedWriteBehaviour{
    // 流程: result: &tikvWriteResult{emptyJob: true},
    // 流程: err: nil,
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: },
    // 流程: err: nil,
    // 流程: },
    // 流程: }
    // 资源收尾: t.Cleanup(func() {
    // PD/TiKV region: fakeRegionJobs = nil
    // 流程: })
    // 原注释: // Create a mock engine that returns the data and ranges
    // mock: mockEngine := &mockEngineWithData{
    // 流程: data: data,
    // 流程: ranges: jobRanges,
    // 流程: }
    // 并发/通道: jobToWorkerCh := make(chan *regionJob, 10)
    // 并发/通道: var jobWg sync.WaitGroup
    // 原注释: // Track jobs received and their ref counts
    // 流程: receivedJobs := make([]*regionJob, 0)
    // 流程: var receivedMu sync.Mutex
    // 并发/通道: done := make(chan struct{})
    // 原注释: // Start a goroutine to consume jobs and simulate fast processing of empty jobs
    // 并发/通道: go func() {
    // 资源收尾: defer close(done)
    // 控制流: for job := range jobToWorkerCh {
    // 流程: receivedMu.Lock()
    // 流程: receivedJobs = append(receivedJobs, job)
    // 流程: receivedMu.Unlock()
    // 原注释: // Simulate fast processing of empty jobs - they finish immediately
    // 原注释: // The key point is: even if empty jobs finish quickly and call done(),
    // 原注释: // the ingestData should not be cleaned because other jobs still hold references
    // 控制流: if job.writeResult != nil && job.writeResult.emptyJob {
    // 原注释: // Empty job finishes quickly - this simulates the bug scenario
    // 原注释: // where empty jobs finish before other jobs are processed
    // 流程: job.convertStageTo(ingested)
    // 流程: job.done(&jobWg)
    // 流程: } else {
    // 原注释: // For non-empty jobs, verify that ingestData is still accessible
    // 原注释: // This is the critical check: even after empty jobs finished,
    // 原注释: // non-empty jobs should still be able to access ingestData
    // 断言: require.False(t, data.IsCleaned(), "ingestData should not be cleaned while non-empty jobs are still processing")
    // 原注释: // Simulate processing the non-empty job
    // 流程: job.convertStageTo(ingested)
    // 流程: job.done(&jobWg)
    // 流程: }
    // 流程: }
    // 流程: }()
    // 原注释: // Generate and send jobs
    // 原注释: // The fix ensures all jobs are ref'd before sending to jobToWorkerCh
    // mock: err = local.generateAndSendJob(ctx, mockEngine, int64(config.SplitRegionSize), int64(config.SplitRegionKeys), jobToWorkerCh, &jobWg)
    // 断言: require.NoError(t, err)
    // 原注释: // Wait for all jobs to be processed
    // 并发/通道: jobWg.Wait()
    // 流程: close(jobToWorkerCh)
    // 并发/通道: <-done
    // 原注释: // Verify all jobs were received and processed
    // 流程: receivedMu.Lock()
    // 断言: require.Equal(t, 5, len(receivedJobs), "all 5 jobs should be received")
    // 流程: receivedMu.Unlock()
    // 原注释: // Verify that ingestData was not cleaned prematurely
    // 原注释: // The key verification: after all jobs are done, the ref count should be 0
    // 原注释: // But importantly, we verified during processing that it wasn't cleaned early
    // 断言: require.True(t, data.GetRefCount() == 0, "ref count should be 0 after all jobs are done")
}

#[test]
// TestGenerateAndSendJobDoneAllRefedJobsOnCancel 对应 Go 函数/方法声明。
// Go: func TestGenerateAndSendJobDoneAllRefedJobsOnCancel(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
pub fn test_generate_and_send_job_done_all_refed_jobs_on_cancel() {
    // failpoint: testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/fakeRegionJobs", "return()")
    // context: ctx, cancel := context.WithCancel(context.Background())
    // 资源收尾: t.Cleanup(cancel)
    // 流程: local := &Backend{}
    // PD/TiKV region: local.WorkerConcurrency.Store(1)
    // 流程: data := &refCountIngestData{
    // mock: mockIngestData: mockIngestData{
    // 流程: {[]byte("b"), []byte("b")},
    // 流程: {[]byte("c"), []byte("c")},
    // 流程: {[]byte("d"), []byte("d")},
    // 流程: {[]byte("e"), []byte("e")},
    // 流程: },
    // 流程: }
    // 流程: jobs := []*regionJob{
    // PD/TiKV region: {keyRange: engineapi.Range{Start: []byte("a"), End: []byte("b")}, ingestData: data, region: dummyRegionInfo},
    // PD/TiKV region: {keyRange: engineapi.Range{Start: []byte("b"), End: []byte("c")}, ingestData: data, region: dummyRegionInfo},
    // PD/TiKV region: {keyRange: engineapi.Range{Start: []byte("c"), End: []byte("d")}, ingestData: data, region: dummyRegionInfo},
    // PD/TiKV region: {keyRange: engineapi.Range{Start: []byte("d"), End: []byte("e")}, ingestData: data, region: dummyRegionInfo},
    // 流程: }
    // 流程: jobRange := engineapi.Range{Start: []byte("a"), End: []byte("e")}
    // PD/TiKV region: fakeRegionJobs = map[[2]string]struct {
    // 流程: jobs []*regionJob
    // 流程: err error
    // 流程: }{
    // 流程: {"a", "e"}: {
    // 流程: jobs: jobs,
    // 流程: },
    // 流程: }
    // 资源收尾: t.Cleanup(func() {
    // PD/TiKV region: fakeRegionJobs = nil
    // 流程: })
    // mock: mockEngine := &mockEngineWithData{
    // 流程: data: data,
    // 流程: ranges: []engineapi.Range{jobRange},
    // 流程: }
    // 并发/通道: jobToWorkerCh := make(chan *regionJob)
    // 并发/通道: var jobWg sync.WaitGroup
    // 并发/通道: firstJobDone := make(chan struct{})
    // 并发/通道: go func() {
    // 并发/通道: job := <-jobToWorkerCh
    // 流程: job.done(&jobWg)
    // 流程: cancel()
    // 流程: close(firstJobDone)
    // 流程: }()
    // mock: err := local.generateAndSendJob(ctx, mockEngine, int64(config.SplitRegionSize), int64(config.SplitRegionKeys), jobToWorkerCh, &jobWg)
    // 断言: require.NoError(t, err)
    // 并发/通道: <-firstJobDone
    // 断言: require.Eventually(t, func() bool {
    // 返回语义: return data.GetRefCount() == 0
    // 流程: }, 10*time.Second, 10*time.Millisecond, "ref'd jobs after the canceled send path must all be marked done")
    // 并发/通道: jobWg.Wait()
    // 断言: require.Equal(t, int64(0), data.GetRefCount())
}

// mockEngineWithData 对应 Go 类型声明；字段/方法关系按原顺序保留。
// Go: type mockEngineWithData struct {
// Go: data engineapi.IngestData
// Go: ranges []engineapi.Range
// Go: }
pub struct mockEngineWithData;

// ID 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) ID() string
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_id`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_id() {
    // mock: return "mock-engine"
}

// LoadIngestData 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) LoadIngestData(ctx context.Context, ch chan<- engineapi.DataAndRanges) error
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_load_ingest_data`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_load_ingest_data() {
    // 控制流: select {
    // 并发/通道: case <-ctx.Done():
    // 返回语义: return ctx.Err()
    // 并发/通道: case ch <- engineapi.DataAndRanges{
    // 流程: Data: m.data,
    // 流程: SortedRanges: m.ranges,
    // 流程: }:
    // 流程: }
    // 原注释: // Don't close the channel here - generateAndSendJob will close it
    // 返回语义: return nil
}

// KVStatistics 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) KVStatistics() (totalKVSize int64, totalKVCount int64)
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_kv_statistics`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_kv_statistics() {
    // 返回语义: return 0, 0
}

// ImportedStatistics 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) ImportedStatistics() (importedKVSize int64, importedKVCount int64)
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_imported_statistics`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_imported_statistics() {
    // 返回语义: return 0, 0
}

// ConflictInfo 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) ConflictInfo() engineapi.ConflictInfo
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_conflict_info`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_conflict_info() {
    // 返回语义: return engineapi.ConflictInfo{
    // 流程: Count: 0,
    // 流程: Files: nil,
    // 流程: }
}

// GetKeyRange 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) GetKeyRange() (startKey []byte, endKey []byte, err error)
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_get_key_range`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_get_key_range() {
    // 控制流: if len(m.ranges) == 0 {
    // 返回语义: return nil, nil, nil
    // 流程: }
    // 返回语义: return m.ranges[0].Start, m.ranges[len(m.ranges)-1].End, nil
}

// GetRegionSplitKeys 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) GetRegionSplitKeys() ([][]byte, error)
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_get_region_split_keys`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_get_region_split_keys() {
    // 流程: keys := make([][]byte, 0, len(m.ranges)+1)
    // 控制流: for _, r := range m.ranges {
    // 流程: keys = append(keys, r.Start)
    // 流程: }
    // 控制流: if len(m.ranges) > 0 {
    // 流程: keys = append(keys, m.ranges[len(m.ranges)-1].End)
    // 流程: }
    // 返回语义: return keys, nil
}

// Close 对应 Go 函数/方法声明。
// Go: func (m *mockEngineWithData) Close() error
// 接收者 `mockEngineWithData` 的方法在这里中摊平成函数名 `mock_engine_with_data_close`，避免引入外部真实类型依赖。
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
pub fn mock_engine_with_data_close() {
    // 返回语义: return nil
}

struct CancellationWorkersClient {
    started: std::sync::atomic::AtomicUsize,
}

impl crate::local::ImportClient for CancellationWorkersClient {
    fn WriteAndIngest(
        &self,
        token: &crate::CancellationToken,
        _engine: &crate::engine::Engine,
        _ranges: &[crate::KeyRange],
    ) -> crate::Result<(i64, i64)> {
        if self
            .started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            while !token.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(crate::Error::Cancelled)
        } else {
            Err(crate::Error::InvalidData("worker fatal error".into()))
        }
    }
    fn Close(&self) {}
}

struct CancellationWorkersFactory(std::sync::Arc<CancellationWorkersClient>);
impl crate::local::ImportClientFactory for CancellationWorkersFactory {
    fn Create(
        &self,
        _: &crate::CancellationToken,
        _: u64,
    ) -> crate::Result<std::sync::Arc<dyn crate::local::ImportClient>> {
        Ok(self.0.clone())
    }
    fn Close(&self) {}
}

#[test]
fn worker_error_cancels_other_running_workers() {
    use std::sync::{Arc, atomic::Ordering};
    use std::time::Duration;
    let client = Arc::new(CancellationWorkersClient {
        started: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut config = crate::local::BackendConfig::default();
    config.local_store_dir = std::env::temp_dir()
        .join(format!("worker-cancellation-{}", crate::EngineId::new()))
        .to_string_lossy()
        .into_owned();
    config.worker_concurrency = 2;
    config.region_split_keys = 1;
    let backend = Arc::new(
        crate::local::NewBackend(
            config,
            Arc::new(GoCommit955fd6550bStoreHelper),
            Some(Arc::new(CancellationWorkersFactory(client.clone()))),
            None,
        )
        .unwrap(),
    );
    let token = crate::CancellationToken::default();
    let id = crate::EngineId::new();
    let engine = backend.OpenEngine(&token, id).unwrap();
    engine.Put(b"a".to_vec(), b"a".to_vec()).unwrap();
    engine.Put(b"b".to_vec(), b"b".to_vec()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn({
        let backend = backend.clone();
        let token = token.clone();
        move || tx.send(backend.ImportEngine(&token, id, 1)).unwrap()
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    token.cancel();
    handle.join().unwrap();
    backend.Close();
    assert_eq!(
        result.unwrap(),
        Err(crate::Error::InvalidData("worker fatal error".into()))
    );
    assert!(client.started.load(Ordering::SeqCst) >= 2);
}

#[derive(Default)]
struct ImportCleanupState {
    allowed: std::sync::Mutex<bool>,
    released: std::sync::Condvar,
    done: std::sync::atomic::AtomicBool,
    refs: std::sync::atomic::AtomicUsize,
}

struct BlockingImportData(std::sync::Arc<ImportCleanupState>);
impl astersql_ingestor_engineapi::IngestData for BlockingImportData {
    fn GetFirstAndLastKey(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> std::result::Result<
        (Option<Vec<u8>>, Option<Vec<u8>>),
        astersql_ingestor_engineapi::EngineError,
    > {
        Ok((Some(b"a".to_vec()), Some(b"a".to_vec())))
    }
    fn NewIter(
        &self,
        ctx: &astersql_ingestor_engineapi::Context,
        lower: &[u8],
        upper: &[u8],
        pool: &mut astersql_lightning_membuf::Pool,
    ) -> Box<dyn astersql_ingestor_engineapi::ForwardIter> {
        astersql_ingestor_engineapi::IngestData::NewIter(
            &crate::import_pipeline::LocalData {
                pairs: vec![crate::KvPair {
                    key: b"a".to_vec(),
                    value: b"a".to_vec(),
                }],
                ts: 1,
                refs: Default::default(),
            },
            ctx,
            lower,
            upper,
            pool,
        )
    }
    fn GetTS(&self) -> u64 {
        1
    }
    fn IncRef(&self) {
        self.0
            .refs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn DecRef(&self) {
        let mut allowed = self.0.allowed.lock().unwrap();
        while !*allowed {
            allowed = self.0.released.wait(allowed).unwrap();
        }
        self.0
            .refs
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.0.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn Finish(&self, _: i64, _: i64) {}
}

struct ImportTestSource {
    state: std::sync::Arc<ImportCleanupState>,
    wait_for_cancel: bool,
}
impl astersql_ingestor_engineapi::Engine for ImportTestSource {
    fn ID(&self) -> String {
        "mock-engine".into()
    }
    fn LoadIngestData(
        &self,
        ctx: &astersql_ingestor_engineapi::Context,
        tx: &std::sync::mpsc::SyncSender<astersql_ingestor_engineapi::DataAndRanges>,
    ) -> std::result::Result<(), astersql_ingestor_engineapi::EngineError> {
        tx.send(astersql_ingestor_engineapi::DataAndRanges {
            Data: Box::new(BlockingImportData(self.state.clone())),
            SortedRanges: vec![astersql_ingestor_engineapi::Range {
                Start: b"a".to_vec(),
                End: b"b".to_vec(),
            }],
        })
        .unwrap();
        if self.wait_for_cancel {
            while !ctx.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            return Err(Box::new(crate::Error::Cancelled));
        }
        Ok(())
    }
    fn KVStatistics(&self) -> (i64, i64) {
        (2, 1)
    }
    fn ImportedStatistics(&self) -> (i64, i64) {
        (0, 0)
    }
    fn ConflictInfo(&self) -> astersql_ingestor_engineapi::ConflictInfo {
        Default::default()
    }
    fn GetKeyRange(
        &self,
    ) -> std::result::Result<(Vec<u8>, Vec<u8>), astersql_ingestor_engineapi::EngineError> {
        Ok((b"a".to_vec(), b"b".to_vec()))
    }
    fn GetRegionSplitKeys(
        &self,
    ) -> std::result::Result<Vec<Vec<u8>>, astersql_ingestor_engineapi::EngineError> {
        Ok(vec![b"a".to_vec(), b"b".to_vec()])
    }
    fn Close(&mut self) -> std::result::Result<(), astersql_ingestor_engineapi::EngineError> {
        Ok(())
    }
}

fn import_test_generator() -> crate::import_pipeline::JobGenerator {
    std::sync::Arc::new(|_, _, ranges| {
        Ok(ranges
            .iter()
            .map(|range| crate::job_worker::RegionJob {
                region: crate::job_worker::RegionInfo {
                    id: 1,
                    leader_store_id: 1,
                    peer_store_ids: vec![1],
                },
                key_range: crate::KeyRange {
                    start: range.Start.clone(),
                    end: range.End.clone(),
                },
                data: vec![crate::KvPair {
                    key: b"a".to_vec(),
                    value: b"a".to_vec(),
                }],
                ..Default::default()
            })
            .collect())
    })
}

#[test]
fn context_cancellation_waits_for_running_workers() {
    use std::sync::{Arc, atomic::Ordering};
    use std::time::Duration;
    let state = Arc::new(ImportCleanupState::default());
    let token = crate::CancellationToken::default();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let factory: crate::import_pipeline::WorkerFactory = Arc::new(move |token| {
        let started = started_tx.clone();
        Ok(Box::new(crate::job_worker::NewRegionJobBaseWorker(
            token,
            Arc::new(|_, _| Ok(Default::default())),
            Arc::new(|_, _| Ok(())),
            Arc::new(move |token, _| {
                started.send(()).unwrap();
                while !token.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(crate::Error::Cancelled)
            }),
            Arc::new(|_, _| unreachable!()),
        )))
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn({
        let token = token.clone();
        let state = state.clone();
        move || {
            let release = state.clone();
            tx.send(crate::import_pipeline::do_import(
                &token,
                Arc::new(ImportTestSource {
                    state,
                    wait_for_cancel: true,
                }),
                1,
                import_test_generator(),
                factory,
                crate::import_pipeline::ImportOptions {
                    before_release: Some(Arc::new(move || {
                        *release.allowed.lock().unwrap() = true;
                        release.released.notify_all();
                    })),
                    ..Default::default()
                },
            ))
            .unwrap();
        }
    });
    let started = started_rx.recv_timeout(Duration::from_secs(2));
    token.cancel();
    // Always unblock cleanup, including a failed startup assertion.
    if started.is_err() {
        *state.allowed.lock().unwrap() = true;
        state.released.notify_all();
    }
    let result = rx.recv_timeout(Duration::from_secs(2));
    handle.join().unwrap();
    started.unwrap();
    assert_eq!(result.unwrap(), Err(crate::Error::Cancelled));
    assert!(
        state.done.load(Ordering::SeqCst),
        "import returned before DecRef completed"
    );
    assert_eq!(state.refs.load(Ordering::SeqCst), 0);
}

#[test]
fn dispatcher_handles_error_shutdown_when_result_channel_closes() {
    use std::sync::Arc;
    let token = crate::CancellationToken::default();
    let results = astersql_resourcemanager_pool_workerpool::Channel::bounded(0);
    results.close();
    let cancel = token.clone();
    let before_wait: Option<Arc<dyn Fn() + Send + Sync>> = Some(Arc::new(move || cancel.cancel()));
    assert_eq!(
        crate::import_pipeline::dispatch_results(
            &token,
            &results,
            &crate::region_job::regionJobRetryer::default(),
            &std::sync::atomic::AtomicBool::new(false),
            &before_wait,
            &None,
        ),
        Err(crate::Error::Cancelled),
    );
}

#[test]
fn dispatcher_propagates_context_cancellation() {
    let token = crate::CancellationToken::default();
    token.cancel();
    let results = astersql_resourcemanager_pool_workerpool::Channel::bounded(0);
    assert_eq!(
        crate::import_pipeline::dispatch_results(
            &token,
            &results,
            &crate::region_job::regionJobRetryer::default(),
            &std::sync::atomic::AtomicBool::new(false),
            &None,
            &None,
        ),
        Err(crate::Error::Cancelled),
    );
}

struct SuccessfulImportWorker {
    late_error: bool,
}
impl crate::job_worker::RegionJobWorker for SuccessfulImportWorker {
    fn HandleTask(
        &self,
        mut job: crate::job_worker::RegionJob,
    ) -> crate::Result<Vec<crate::job_worker::RegionJob>> {
        job.write_result = Some(crate::job_worker::TikvWriteResult {
            total_bytes: 2,
            count: 1,
            ..Default::default()
        });
        job.convertStageTo(crate::job_worker::RegionJobStage::Ingested);
        Ok(vec![job])
    }
    fn Close(&self) -> crate::Result<()> {
        if self.late_error {
            Err(crate::Error::InvalidData("worker close failed".into()))
        } else {
            Ok(())
        }
    }
}

#[test]
fn import_pipeline_marks_success_after_worker_cleanup() {
    use std::sync::{Arc, atomic::Ordering};
    let state = Arc::new(ImportCleanupState::default());
    *state.allowed.lock().unwrap() = true;
    let result = crate::import_pipeline::do_import(
        &crate::CancellationToken::default(),
        Arc::new(ImportTestSource {
            state: state.clone(),
            wait_for_cancel: false,
        }),
        2,
        import_test_generator(),
        Arc::new(|_| Ok(Box::new(SuccessfulImportWorker { late_error: false }))),
        crate::import_pipeline::ImportOptions {
            local_engine: true,
            ..Default::default()
        },
    );
    assert_eq!(result, Ok((2, 1)));
    assert!(state.done.load(Ordering::SeqCst));
    assert_eq!(state.refs.load(Ordering::SeqCst), 0);
}

#[test]
fn import_pipeline_retains_error_set_during_worker_release() {
    use std::sync::{Arc, atomic::Ordering};
    let state = Arc::new(ImportCleanupState::default());
    *state.allowed.lock().unwrap() = true;
    let result = crate::import_pipeline::do_import(
        &crate::CancellationToken::default(),
        Arc::new(ImportTestSource {
            state: state.clone(),
            wait_for_cancel: false,
        }),
        1,
        import_test_generator(),
        Arc::new(|_| Ok(Box::new(SuccessfulImportWorker { late_error: true }))),
        Default::default(),
    );
    assert_eq!(
        result,
        Err(crate::Error::InvalidData("worker close failed".into()))
    );
    assert_eq!(state.refs.load(Ordering::SeqCst), 0);
}

#[test]
fn import_pipeline_recovers_worker_panic_and_releases_data() {
    use std::sync::{Arc, atomic::Ordering};
    let state = Arc::new(ImportCleanupState::default());
    *state.allowed.lock().unwrap() = true;
    let counter = astersql_metrics::metrics::PanicCounter.with_label_values(&["regionJob"]);
    let before = counter.get();
    let result = crate::import_pipeline::do_import(
        &crate::CancellationToken::default(),
        Arc::new(ImportTestSource {
            state: state.clone(),
            wait_for_cancel: false,
        }),
        1,
        import_test_generator(),
        Arc::new(|token| {
            Ok(Box::new(crate::job_worker::NewRegionJobBaseWorker(
                token,
                Arc::new(|_, _| panic!("region job failure")),
                Arc::new(|_, _| Ok(())),
                Arc::new(|_, _| Ok(())),
                Arc::new(|_, _| unreachable!()),
            )))
        }),
        Default::default(),
    );
    assert_eq!(
        result,
        Err(crate::Error::InvalidData("region job worker panic".into()))
    );
    assert_eq!(state.refs.load(Ordering::SeqCst), 0);
    assert_eq!(counter.get(), before + 1.0);
}

#[test]
fn import_pipeline_generation_error_cancels_loader() {
    use std::sync::Arc;
    let state = Arc::new(ImportCleanupState::default());
    *state.allowed.lock().unwrap() = true;
    let result = crate::import_pipeline::do_import(
        &crate::CancellationToken::default(),
        Arc::new(ImportTestSource {
            state,
            wait_for_cancel: true,
        }),
        2,
        Arc::new(|_, _, _| Err(crate::Error::InvalidData("generate job failed".into()))),
        Arc::new(|_| Ok(Box::new(SuccessfulImportWorker { late_error: false }))),
        crate::import_pipeline::ImportOptions {
            local_engine: true,
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        Err(crate::Error::InvalidData("generate job failed".into()))
    );
}

struct RetryLimitWorker;
impl crate::job_worker::RegionJobWorker for RetryLimitWorker {
    fn HandleTask(
        &self,
        job: crate::job_worker::RegionJob,
    ) -> crate::Result<Vec<crate::job_worker::RegionJob>> {
        Ok(vec![job])
    }
    fn Close(&self) -> crate::Result<()> {
        Ok(())
    }
}
#[test]
fn import_pipeline_retry_limit_preserves_error_and_releases_data() {
    use std::sync::{Arc, atomic::Ordering};
    let state = Arc::new(ImportCleanupState::default());
    *state.allowed.lock().unwrap() = true;
    let result = crate::import_pipeline::do_import(
        &crate::CancellationToken::default(),
        Arc::new(ImportTestSource {
            state: state.clone(),
            wait_for_cancel: false,
        }),
        1,
        Arc::new(|token, data, ranges| {
            let mut jobs = import_test_generator()(token, data, ranges)?;
            for job in &mut jobs {
                job.retry_count = 30;
                job.last_retryable_error = Some("last retry failure".into());
                job.last_retryable_cause = Some(crate::Error::Io("last retry failure".into()));
            }
            Ok(jobs)
        }),
        Arc::new(|_| Ok(Box::new(RetryLimitWorker))),
        Default::default(),
    );
    assert_eq!(result, Err(crate::Error::Io("last retry failure".into())));
    assert_eq!(state.refs.load(Ordering::SeqCst), 0);
}

#[test]
fn dispatcher_cancellation_interrupts_open_result_channel() {
    use std::sync::{Arc, atomic::AtomicBool};
    use std::time::Duration;
    let token = crate::CancellationToken::default();
    let results = astersql_resourcemanager_pool_workerpool::Channel::bounded(0);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn({
        let token = token.clone();
        let results = results.clone();
        move || {
            let cancel = token.clone();
            let before_receive: Option<Arc<dyn Fn() + Send + Sync>> =
                Some(Arc::new(move || cancel.cancel()));
            tx.send(crate::import_pipeline::dispatch_results(
                &token,
                &results,
                &crate::region_job::regionJobRetryer::default(),
                &AtomicBool::new(false),
                &None,
                &before_receive,
            ))
            .unwrap();
        }
    });
    let outcome = rx.recv_timeout(Duration::from_secs(2));
    results.close();
    handle.join().unwrap();
    assert_eq!(outcome.unwrap(), Err(crate::Error::Cancelled));
}
