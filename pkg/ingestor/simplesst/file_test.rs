// Copyright 2026 AsterSQL.

// KeyValueStore 与 KVReader 联调的单元测试。
//
// 写入有序 KV 时按键数切分 RangeProperty；读回时按编码记录精确解析，
// 耗尽后返回 EOF。SST（Sorted String Table）数据文件线格式为
// `<keyLen><valueLen><key><value>`（长度为大端 uint64）。

/// 属性在达到 prop_keys 阈值时切分；读侧按记录边界推进直至 EOF。
#[test]
fn canonical_kv_file_tracks_properties_and_reads_exact_records() {
    use crate::file::KeyValueStore;
    use crate::kv_reader::KVReader;
    use crate::writer::RangePropertiesCollector;

    // prop_keys=2：每写入 2 个 key 产出一条范围属性。
    let mut store = KeyValueStore::new(Some(RangePropertiesCollector::new(u64::MAX, 2)));
    store.add_raw_kv(b"a", b"one").unwrap();
    assert!(store.collector().unwrap().properties().is_empty());
    store.add_raw_kv(b"b", b"two").unwrap();
    assert_eq!(store.collector().unwrap().properties().len(), 1);
    store.add_raw_kv(b"c", b"three").unwrap();
    let (data, collector) = store.into_parts();
    let properties = collector.unwrap().properties().to_vec();
    // finish 会刷新未满阈值的尾部属性，故共 2 条：[a,b] 与 [c,c]。
    assert_eq!(properties.len(), 2);
    assert_eq!(
        (
            properties[0].FirstKey.as_slice(),
            properties[0].LastKey.as_slice()
        ),
        (&b"a"[..], &b"b"[..])
    );
    assert_eq!(properties[0].Offset, 0);
    assert_eq!(properties[0].Size, 8);
    assert_eq!(properties[0].Keys, 2);
    assert_eq!(properties[1].FirstKey, b"c");
    assert_eq!(properties[1].LastKey, b"c");
    assert_eq!(properties[1].Offset, 40);
    assert_eq!(properties[1].Size, 6);
    assert_eq!(properties[1].Keys, 1);

    // 用同一缓冲构造 KVReader，按顺序读回三条记录后遇 EOF。
    let mut reader = KVReader::new(data, 0, 3).unwrap();
    assert_eq!(reader.next_kv().unwrap(), (b"a".to_vec(), b"one".to_vec()));
    assert_eq!(reader.next_kv().unwrap(), (b"b".to_vec(), b"two".to_vec()));
    assert_eq!(
        reader.next_kv().unwrap(),
        (b"c".to_vec(), b"three".to_vec())
    );
    assert!(reader.next_kv().unwrap_err().is_eof());
}

/// 记录内部的 value 正文被截断时必须返回 UnexpectedEOF，不能伪装成文件正常结束。
#[test]
fn test_kv_reader_rejects_truncated_record_body() {
    use crate::file::encode_kv;
    use crate::kv_reader::KVReader;

    let mut data = encode_kv(b"key", b"value").unwrap();
    data.pop();
    let mut reader = KVReader::new(data, 0, 3).unwrap();
    let error = reader.next_kv().unwrap_err();
    assert!(
        error.to_string().contains("unexpected"),
        "truncated record must be unexpected EOF, got {error}"
    );
}

/// 对照 Go `TestKVReadWrite`：不同缓冲大小下随机形状 KV 均可逐条往返。
#[test]
fn test_kv_read_write() {
    use crate::file::KeyValueStore;
    use crate::kv_reader::KVReader;

    let rows = (0_u8..20)
        .map(|index| {
            (
                vec![index; usize::from(index % 7 + 1)],
                vec![255 - index; usize::from(index % 9 + 1)],
            )
        })
        .collect::<Vec<_>>();
    let mut store = KeyValueStore::new(None);
    for (key, value) in &rows {
        store.add_raw_kv(key, value).unwrap();
    }
    let data = store.into_parts().0;
    for buffer_size in [1, 2, 3, 7, 31] {
        let mut reader = KVReader::new(data.clone(), 0, buffer_size).unwrap();
        for expected in &rows {
            assert_eq!(&reader.next_kv().unwrap(), expected);
        }
        assert!(reader.next_kv().unwrap_err().is_eof());
        reader.close().unwrap();
    }
}

/// `Finish` 只刷新范围属性；与 Go 一样，它不关闭底层数据写入器。
#[test]
fn finish_does_not_close_the_store() {
    use crate::file::{KeyValueStore, encode_kv};

    let mut store = KeyValueStore::new(None);
    store.add_raw_kv(b"a", b"one").unwrap();
    store.finish();
    store.add_raw_kv(b"b", b"two").unwrap();

    let expected = [
        encode_kv(b"a", b"one").unwrap(),
        encode_kv(b"b", b"two").unwrap(),
    ]
    .concat();
    assert_eq!(store.data(), expected);
    assert_eq!(store.offset(), expected.len() as u64);
}
