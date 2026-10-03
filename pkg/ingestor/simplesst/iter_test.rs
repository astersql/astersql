// Copyright 2026 AsterSQL.

// MergeKVIter（多路 KV 归并迭代器）的单元测试。
//
// 从多个有序 SST 数据文件归并键值，空文件在首读 EOF 时关闭并不参与输出。
// 归并用最小堆按 key 选路，保证全局有序。

/// 左、空、右三路输入归并后应按 a、b、c、d 顺序产出。
#[test]
fn canonical_merge_kv_iter_orders_all_inputs_and_closes_empty_readers() {
    use crate::MemoryStorage;
    use crate::file::{KeyValueStore, encode_kv};
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    let mut left = KeyValueStore::new(None);
    left.add_raw_kv(b"a", b"1").unwrap();
    left.add_raw_kv(b"d", b"4").unwrap();
    storage.write("left", left.into_parts().0).unwrap();
    // 空对象：打开后立即 EOF，对应 reader 槽位置空。
    storage.write("empty", Vec::new()).unwrap();
    storage
        .write(
            "right",
            [
                encode_kv(b"b", b"2").unwrap(),
                encode_kv(b"c", b"3").unwrap(),
            ]
            .concat(),
        )
        .unwrap();

    let paths = vec!["left".to_string(), "empty".to_string(), "right".to_string()];
    let mut iter = MergeKVIter::new(&paths, &storage, None, 2).unwrap();
    let mut output = Vec::new();
    while iter.next() {
        output.push((iter.key().to_vec(), iter.value().to_vec()));
    }
    assert_eq!(
        output,
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"2".to_vec()),
            (b"c".to_vec(), b"3".to_vec()),
            (b"d".to_vec(), b"4".to_vec()),
        ]
    );
    assert!(iter.error().is_none());
    iter.close().unwrap();
}

/// 对照 Go `TestCorruptContent`：完整 KV 后的损坏尾部必须通过 `Error` 暴露。
#[test]
fn test_corrupt_content() {
    use crate::MemoryStorage;
    use crate::file::KeyValueStore;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    let mut left = KeyValueStore::new(None);
    left.add_raw_kv(b"key1", b"value1").unwrap();
    left.add_raw_kv(b"key3", b"value3").unwrap();
    let mut left_data = left.into_parts().0;
    left_data.extend_from_slice(b"corrupt");
    storage.write("left-corrupt", left_data).unwrap();

    let mut right = KeyValueStore::new(None);
    right.add_raw_kv(b"key2", b"value2").unwrap();
    storage.write("right-valid", right.into_parts().0).unwrap();

    let paths = vec!["left-corrupt".to_string(), "right-valid".to_string()];
    let mut iter = MergeKVIter::new(&paths, &storage, None, 5).unwrap();
    let mut keys = Vec::new();
    while iter.next() {
        keys.push(iter.key().to_vec());
    }
    assert_eq!(
        keys,
        vec![b"key1".to_vec(), b"key2".to_vec(), b"key3".to_vec()]
    );
    let error = iter.error().expect("corrupt suffix must be reported");
    assert!(
        error.to_string().contains("unexpected"),
        "corrupt suffix must be unexpected EOF, got {error}"
    );
}

fn write_rows(storage: &crate::MemoryStorage, path: &str, rows: &[(&[u8], &[u8])]) {
    let mut store = crate::file::KeyValueStore::new(None);
    for (key, value) in rows {
        store.add_raw_kv(key, value).unwrap();
    }
    storage.write(path, store.into_parts().0).unwrap();
}

/// 对照 Go `TestOneUpstream`。
#[test]
fn test_one_upstream() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    write_rows(
        &storage,
        "single",
        &[
            (b"key1", b"value1"),
            (b"key2", b"value2"),
            (b"key3", b"value3"),
        ],
    );
    let mut iter = MergeKVIter::new(&["single".into()], &storage, None, 5).unwrap();
    let mut keys = Vec::new();
    while iter.next() {
        keys.push(iter.key().to_vec());
    }
    assert_eq!(keys, [b"key1", b"key2", b"key3"].map(Vec::from));
    assert!(iter.error().is_none());
    assert_eq!(iter.open_reader_count(), 0);
}

/// 对照 Go `TestAllEmpty`。
#[test]
fn test_all_empty() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    storage.write("empty-1", Vec::new()).unwrap();
    storage.write("empty-2", Vec::new()).unwrap();
    let mut iter =
        MergeKVIter::new(&["empty-1".into(), "empty-2".into()], &storage, None, 5).unwrap();
    assert_eq!(iter.open_reader_count(), 0);
    assert!(!iter.next());
    assert!(iter.error().is_none());
    iter.close().unwrap();
}

/// 对照 Go `TestMergeIterSwitchMode`：热点检测开启时完整归并不丢数据。
#[test]
fn test_merge_iter_switch_mode() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    for file in 0..4_u8 {
        let rows = (0..50_u8)
            .filter(|value| value % 4 == file)
            .map(|value| (vec![value], vec![value.wrapping_mul(value)]))
            .collect::<Vec<_>>();
        let refs = rows
            .iter()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
            .collect::<Vec<_>>();
        write_rows(&storage, &format!("switch-{file}"), &refs);
    }
    let paths = (0..4).map(|i| format!("switch-{i}")).collect::<Vec<_>>();
    let mut iter = MergeKVIter::new_with_options(&paths, &storage, None, 2, true, 1).unwrap();
    iter.set_hotspot_check_period(3);
    let mut output = Vec::new();
    while iter.next() {
        output.push(iter.key()[0]);
    }
    assert_eq!(output, (0..50_u8).collect::<Vec<_>>());
    assert!(iter.error().is_none());
}

/// 对照 Go `TestReadAfterCloseConnReader`：退出并发模式后文件尾仍是正常 EOF。
#[test]
fn test_read_after_close_conn_reader() {
    use crate::ByteReader;

    let mut reader = ByteReader::new(vec![42], 0, 1).unwrap();
    reader.enable_concurrent_read(1, 1).unwrap();
    reader.switch_concurrent_mode(true).unwrap();
    assert_eq!(reader.read_n_bytes(1).unwrap(), vec![42]);
    reader.switch_concurrent_mode(false).unwrap();
    assert!(reader.read_n_bytes(1).unwrap_err().is_eof());
}

/// 对照 Go `TestHotspot`：唯一多数 reader 启用并发，热点消失时立即释放。
#[test]
fn test_hotspot() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    write_rows(
        &storage,
        "hot-0",
        &[
            (b"key00", b"v"),
            (b"key01", b"v"),
            (b"key02", b"v"),
            (b"key06", b"v"),
            (b"key07", b"v"),
        ],
    );
    write_rows(
        &storage,
        "hot-1",
        &[
            (b"key03", b"v"),
            (b"key04", b"v"),
            (b"key05", b"v"),
            (b"key08", b"v"),
            (b"key09", b"v"),
        ],
    );
    let mut iter = MergeKVIter::new_with_options(
        &["hot-0".into(), "hot-1".into()],
        &storage,
        None,
        2,
        true,
        1,
    )
    .unwrap();
    iter.set_hotspot_check_period(2);
    assert!(iter.next());
    assert!(iter.next());
    assert_eq!(iter.reader_concurrent_mode(0), Some((true, false)));
    assert!(iter.next());
    assert_eq!(iter.reader_concurrent_mode(0), Some((true, true)));
    assert!(iter.next());
    assert_eq!(iter.reader_concurrent_mode(0), Some((false, false)));
    while iter.next() {}
    assert_eq!(iter.open_reader_count(), 0);
}

/// 对照 Go `TestMemoryUsageWhenHotspotChange`：热点轮换后所有 reader 与预取状态均释放。
#[test]
fn test_memory_usage_when_hotspot_change() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let storage = MemoryStorage::default();
    let mut paths = Vec::new();
    for file in 0..10_u8 {
        let path = format!("memory-hot-{file}");
        let rows = (0..20_u8)
            .map(|row| (vec![file, row], vec![row; 1024]))
            .collect::<Vec<_>>();
        let refs = rows
            .iter()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
            .collect::<Vec<_>>();
        write_rows(&storage, &path, &refs);
        paths.push(path);
    }
    let mut iter = MergeKVIter::new_with_options(&paths, &storage, None, 64, true, 16).unwrap();
    iter.set_hotspot_check_period(5);
    let mut count = 0;
    while iter.next() {
        count += 1;
    }
    assert_eq!(count, 200);
    assert_eq!(iter.open_reader_count(), 0);
    iter.close().unwrap();
}

/// 对照 Go `TestLimitSizeMergeIter`：任意限额下全局有序且错误可传播。
#[test]
fn test_limit_size_merge_iter() {
    use crate::Error;
    use crate::iter::LimitSizeMergeIter;

    for limit in 1..=4 {
        let sources = vec![
            vec![Ok(1), Ok(2), Ok(3)],
            vec![Ok(4), Ok(5), Ok(6)],
            vec![Ok(7), Ok(8), Ok(9)],
        ];
        let mut iter = LimitSizeMergeIter::new(sources, vec![1, 1, 1], limit).unwrap();
        let mut output = Vec::new();
        while iter.next() {
            output.push(*iter.current().unwrap());
            assert!(iter.active_weight() <= limit);
        }
        assert_eq!(output, (1..=9).collect::<Vec<_>>());
        assert!(iter.error().is_none());

        let error = Error::InvalidData("mock error".into());
        let mut failed = LimitSizeMergeIter::new(
            vec![vec![Ok(1), Err(error.clone())], vec![Ok(2), Ok(3)]],
            vec![1, 1],
            limit,
        )
        .unwrap();
        while failed.next() {}
        assert_eq!(failed.error(), Some(&error));
    }
}

/// 对照 Go `TestLimitSizeMergeIterDiffWeight`。
#[test]
fn test_limit_size_merge_iter_diff_weight() {
    use crate::iter::LimitSizeMergeIter;

    let sources = vec![
        vec![Ok(1), Ok(4), Ok(7)],
        vec![Ok(2), Ok(5)],
        vec![Ok(3)],
        vec![Ok(10)],
        vec![Ok(11), Ok(14), Ok(17)],
        vec![Ok(12), Ok(15)],
        vec![Ok(13)],
    ];
    let mut iter = LimitSizeMergeIter::new(sources, vec![1, 1, 1, 3, 1, 1, 1], 3).unwrap();
    let mut output = Vec::new();
    while iter.next() {
        output.push(*iter.current().unwrap());
        assert!(iter.active_weight() <= 3);
    }
    assert_eq!(output, vec![1, 2, 3, 4, 5, 7, 10, 11, 12, 13, 14, 15, 17]);
}

/// 对照 Go `TestMergePropBaseIter`：多属性文件惰性耗尽标志与顺序正确。
#[test]
fn test_merge_prop_base_iter() {
    use crate::MemoryStorage;
    use crate::codec::{RangeProperty, encode_multi_props};
    use crate::iter::MergePropIter;
    use crate::writer::MultipleFilesStat;

    let storage = MemoryStorage::default();
    let mut filenames = Vec::new();
    for index in 0..16_u8 {
        let path = format!("prop-{index}");
        storage
            .write(
                &path,
                encode_multi_props(&[RangeProperty {
                    FirstKey: vec![index],
                    ..RangeProperty::default()
                }])
                .unwrap(),
            )
            .unwrap();
        filenames.push(["".into(), path]);
    }
    let mut iter = MergePropIter::new(
        vec![MultipleFilesStat {
            Filenames: filenames,
            ..MultipleFilesStat::default()
        }],
        &storage,
    )
    .unwrap();
    for expected in 0..16_u8 {
        assert!(iter.next());
        assert_eq!(iter.current_property().unwrap().FirstKey, vec![expected]);
        assert!(iter.GetBaseIterCloseReaderFlag());
    }
    assert!(!iter.next());
}

/// 对照 Go `TestEmptyBaseReader4LimitSizeMergeIter`。
#[test]
fn test_empty_base_reader_for_limit_size_merge_iter() {
    use crate::MemoryStorage;
    use crate::iter::MergePropIter;
    use crate::writer::MultipleFilesStat;

    let storage = MemoryStorage::default();
    let mut filenames = Vec::new();
    for index in 0..100 {
        let path = format!("empty-prop-{index}");
        storage.write(&path, Vec::new()).unwrap();
        filenames.push(["".into(), path]);
    }
    let mut iter = MergePropIter::new(
        vec![MultipleFilesStat {
            Filenames: filenames,
            ..MultipleFilesStat::default()
        }],
        &storage,
    )
    .unwrap();
    assert!(!iter.next());
    assert!(iter.Error().is_none());
}

/// 对照 Go `TestCloseLimitSizeMergeIterHalfway`：半途关闭将活动权重归零。
#[test]
fn test_close_limit_size_merge_iter_halfway() {
    use crate::iter::LimitSizeMergeIter;

    let sources = (0..100)
        .map(|index| vec![Ok(index), Ok(index + 100)])
        .collect::<Vec<_>>();
    let mut iter = LimitSizeMergeIter::new(sources, vec![1; 100], 8).unwrap();
    assert!(iter.next());
    assert!(iter.active_weight() > 0);
    iter.close();
    assert_eq!(iter.active_weight(), 0);
    assert!(!iter.next());
}

/// 对照 Go `TestMergeKVIterPassWrongParam`。
#[test]
fn test_merge_kv_iter_pass_wrong_param() {
    use crate::MemoryStorage;
    use crate::iter::MergeKVIter;

    let error = MergeKVIter::new_with_options(&[], &MemoryStorage::default(), None, 1, true, 0)
        .err()
        .unwrap();
    assert!(error.to_string().contains("outerConcurrency"));
}

#[test]
fn merge_prop_close_with_async_open_error() {
    close_with_pending_property_opens(true, 64);
}

#[test]
fn merge_prop_close_with_async_open_success() {
    close_with_pending_property_opens(false, 64);
}

#[test]
fn merge_prop_close_drains_full_preopen_queue() {
    close_with_pending_property_opens(false, 100);
}

fn close_with_pending_property_opens(fail: bool, count: usize) {
    use crate::{
        MemoryStorage,
        codec::{RangeProperty, encode_multi_props},
        iter::MergePropIter,
        writer::MultipleFilesStat,
    };
    use std::{sync::mpsc, time::Duration};
    let storage = MemoryStorage::default();
    let paths = (0..count)
        .map(|i| format!("/test{i:06}"))
        .collect::<Vec<_>>();
    for (i, path) in paths[..if fail { 32 } else { count }].iter().enumerate() {
        storage
            .write(
                path,
                encode_multi_props(&[RangeProperty {
                    FirstKey: vec![i as u8],
                    ..Default::default()
                }])
                .unwrap(),
            )
            .unwrap();
    }
    let (gate, started) = storage.gate_reads(paths[32..].to_vec(), fail);
    let stat = MultipleFilesStat {
        MaxOverlappingNum: 31,
        Filenames: paths
            .into_iter()
            .map(|path| [String::new(), path])
            .collect(),
        ..Default::default()
    };
    let (send, receive) = mpsc::channel();
    let constructor = std::thread::spawn(move || {
        let _ = send.send(MergePropIter::new(vec![stat], &storage));
    });
    let result = receive.recv_timeout(Duration::from_secs(3));
    // Always unblock and join the constructor even when the pre-fix synchronous path fails.
    if result.is_err() {
        gate.release();
        constructor.join().unwrap();
        panic!("constructor waited for asynchronous opens");
    }
    let mut iter = result.unwrap().unwrap();
    for _ in 0..(count - 32).min(33) {
        if started.recv_timeout(Duration::from_secs(3)).is_err() {
            gate.release();
            panic!("not all 32 asynchronous opens started");
        }
    }
    let signal = iter.close_signal();
    let closer = std::thread::spawn(move || iter.close());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !signal.load(std::sync::atomic::Ordering::Acquire) && std::time::Instant::now() < deadline
    {
        std::thread::yield_now();
    }
    let signaled = signal.load(std::sync::atomic::Ordering::Acquire);
    let waited = !closer.is_finished();
    gate.release();
    assert!(signaled, "close did not signal pending opens");
    assert!(waited, "close returned before blocked opens finished");
    closer.join().unwrap().unwrap();
    constructor.join().unwrap();
}

#[test]
fn merge_prop_preopened_readers_preserve_order_and_surface_errors() {
    use crate::{
        MemoryStorage,
        codec::{RangeProperty, encode_multi_props},
        iter::MergePropIter,
        writer::MultipleFilesStat,
    };
    for missing in [false, true] {
        let storage = MemoryStorage::default();
        let mut filenames = Vec::new();
        for i in 0..64_u8 {
            let path = format!("prop-{i:06}");
            if !missing || i < 32 {
                storage
                    .write(
                        &path,
                        encode_multi_props(&[RangeProperty {
                            FirstKey: vec![i],
                            ..Default::default()
                        }])
                        .unwrap(),
                    )
                    .unwrap();
            }
            filenames.push([String::new(), path]);
        }
        let mut iter = MergePropIter::new(
            vec![MultipleFilesStat {
                Filenames: filenames,
                MaxOverlappingNum: 31,
                ..Default::default()
            }],
            &storage,
        )
        .unwrap();
        let mut keys = Vec::new();
        while iter.next() {
            keys.push(iter.current_property().unwrap().FirstKey[0]);
        }
        if missing {
            assert_eq!(keys, vec![0]);
            assert!(matches!(iter.Error(), Some(crate::Error::NotFound(_))));
        } else {
            assert_eq!(keys, (0..64_u8).collect::<Vec<_>>());
            assert!(iter.Error().is_none());
        }
        iter.close().unwrap();
        iter.close().unwrap();
        assert!(!iter.next());
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ReadGate {
    paths: std::collections::HashSet<String>,
    started: std::sync::mpsc::Sender<String>,
    released: (std::sync::Mutex<bool>, std::sync::Condvar),
    fail: bool,
}
#[cfg(test)]
impl ReadGate {
    pub(crate) fn before_read(&self, path: &str) -> crate::Result<()> {
        if !self.paths.contains(path) {
            return Ok(());
        }
        let _ = self.started.send(path.to_owned());
        let mut released = self.released.0.lock().map_err(|_| crate::Error::Poisoned)?;
        while !*released {
            released = self
                .released
                .1
                .wait(released)
                .map_err(|_| crate::Error::Poisoned)?;
        }
        if self.fail {
            return Err(crate::Error::Io(
                std::io::ErrorKind::Other,
                "async open failed".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}

impl crate::MemoryStorage {
    /// Pause selected object opens at the storage boundary until explicitly released.
    #[cfg(test)]
    pub(crate) fn gate_reads(
        &self,
        paths: Vec<String>,
        fail: bool,
    ) -> (std::sync::Arc<ReadGate>, std::sync::mpsc::Receiver<String>) {
        let (started, receiver) = std::sync::mpsc::channel();
        let gate = std::sync::Arc::new(ReadGate {
            paths: paths.into_iter().collect(),
            started,
            released: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
            fail,
        });
        *self.read_gate.lock().unwrap() = Some(gate.clone());
        (gate, receiver)
    }
}
