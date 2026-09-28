// Copyright 2026 AsterSQL.

// TiKV 客户端适配层的契约测试。
//
// 覆盖存储 trait 接入、版本与选项转换、按需分页及 Region 边界路由；末尾的忽略测试
// 连接真实集群，验证乐观/悲观事务、回滚和 MVCC 快照的一致性。

use std::sync::{Arc, Mutex};

use astersql_kv as kv;
use tikv_client::TimestampExt;

use crate::kv_adapter::{
    CLIENT_SCAN_PAGE_SIZE, ClientIterator, configured_scan_batch_size, option_enabled,
    scan_region_page, snapshot_timestamp,
};
use crate::{TiKVDriver, TikvStore};

fn assert_canonical_storage<T: kv::Storage>() {}

// 编译期约束：适配后的 TiKV 存储必须完整实现项目统一的存储接口。
#[test]
fn kv_adapter_implements_canonical_storage_contract() {
    assert_canonical_storage::<TikvStore>();
}

#[test]
fn latest_snapshot_sentinel_uses_current_pd_timestamp() {
    let calls = std::cell::Cell::new(0);
    let timestamp = snapshot_timestamp(kv::MaxVersion.Ver, || {
        calls.set(calls.get() + 1);
        Ok(42)
    })
    .unwrap();
    assert_eq!(
        timestamp.version(),
        42,
        "never send i64::MAX as a normal TiKV scan timestamp"
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn historical_snapshot_does_not_contact_pd() {
    assert_eq!(
        snapshot_timestamp(42, || panic!("historical read must keep its timestamp"))
            .unwrap()
            .version(),
        42
    );
}

#[test]
fn latest_snapshot_propagates_pd_failure() {
    assert!(
        snapshot_timestamp(kv::MaxVersion.Ver, || Err(kv::errors::New(
            "PD unavailable"
        )))
        .unwrap_err()
        .to_string()
        .contains("PD unavailable")
    );
}

#[test]
fn unrepresentable_snapshot_is_rejected_without_clamping() {
    assert!(snapshot_timestamp(i64::MAX as u64 + 1, || panic!("not a latest sentinel")).is_err());
}

#[test]
fn oracle_failure_is_returned_by_all_snapshot_read_paths() {
    use kv::{Getter, Retriever, Snapshot};
    let mut snapshot = crate::kv_adapter::FailedSnapshot(kv::errors::New("PD unavailable"));
    snapshot.SetOption(kv::KeyOnly, Some(Box::new(true)));
    let context = kv::Context::default();
    let key = kv::Key(b"key".to_vec());
    assert!(
        snapshot
            .Get(&context, key.clone(), &[])
            .unwrap_err()
            .to_string()
            .contains("PD unavailable")
    );
    assert!(
        snapshot
            .BatchGet(&context, &[key.clone()], &[])
            .unwrap_err()
            .to_string()
            .contains("PD unavailable")
    );
    assert!(
        snapshot
            .Iter(key, None)
            .err()
            .unwrap()
            .to_string()
            .contains("PD unavailable")
    );
    assert!(
        snapshot
            .IterReverse(None, None)
            .err()
            .unwrap()
            .to_string()
            .contains("PD unavailable")
    );
}

#[test]
fn scan_batch_size_matches_client_go_scanner_rules() {
    // 零散小批量沿用 Go 扫描器的默认页大小，合法显式值则原样传给客户端。
    assert_eq!(
        configured_scan_batch_size(&1_usize),
        Some(CLIENT_SCAN_PAGE_SIZE)
    );
    assert_eq!(configured_scan_batch_size(&512_usize), Some(512));
    assert_eq!(configured_scan_batch_size(&512_i32), Some(512));
    assert_eq!(configured_scan_batch_size(&u64::MAX), None);
}

#[test]
fn key_only_option_requires_an_explicit_true_boolean() {
    let mut options = std::collections::HashMap::<i32, Box<dyn std::any::Any>>::new();
    assert!(!option_enabled(&options, kv::KeyOnly));
    options.insert(kv::KeyOnly, Box::new(true));
    assert!(option_enabled(&options, kv::KeyOnly));
    options.insert(kv::KeyOnly, Box::new(false));
    assert!(!option_enabled(&options, kv::KeyOnly));
}

#[test]
fn client_iterator_fetches_remote_scan_pages_on_demand() {
    let rows = Arc::new(
        (0..(CLIENT_SCAN_PAGE_SIZE as usize * 3))
            .map(|index| {
                let key = format!("{index:08}").into_bytes();
                (kv::Key(key), vec![b'v'])
            })
            .collect::<Vec<_>>(),
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let scan_rows = Arc::clone(&rows);
    let scan_calls = Arc::clone(&calls);
    let scan_page = Box::new(
        move |start: Vec<u8>, end: Option<Vec<u8>>, reverse: bool, limit: u32| {
            scan_region_page(
                |_, _| Ok(astersql_store_copr::KeyLocation::default()),
                |start, end, reverse, limit| {
                    assert!(!reverse);
                    scan_calls.lock().expect("scan call lock").push((
                        start.clone(),
                        end.clone(),
                        limit,
                    ));
                    Ok(scan_rows
                        .iter()
                        .filter(|(key, _)| {
                            key.as_ref() >= start.as_slice()
                                && end.as_ref().is_none_or(|end| key.as_ref() < end.as_slice())
                        })
                        .take(limit as usize)
                        .cloned()
                        .collect())
                },
                start,
                end,
                reverse,
                limit,
            )
        },
    );

    // 构造迭代器时只允许拉取首个有界页，避免把整个远端范围预读到内存。
    let mut iterator =
        ClientIterator::paged(scan_page, Vec::new(), None, false, CLIENT_SCAN_PAGE_SIZE)
            .expect("open paged client iterator");
    assert_eq!(
        calls.lock().expect("scan call lock").len(),
        1,
        "opening an iterator must fetch only its first bounded page"
    );

    // 消费完当前页后才能触发下一次请求；提前关闭不得继续访问剩余范围。
    for expected in 0..CLIENT_SCAN_PAGE_SIZE as usize {
        assert!(kv::Iterator::Valid(&iterator));
        assert_eq!(
            kv::Iterator::Key(&iterator).0,
            format!("{expected:08}").into_bytes()
        );
        kv::Iterator::Next(&mut iterator).expect("advance first page");
    }
    assert_eq!(
        calls.lock().expect("scan call lock").len(),
        2,
        "the second page must not be fetched before the first is consumed"
    );

    kv::Iterator::Close(&mut iterator);
    assert!(!kv::Iterator::Valid(&iterator));
    assert_eq!(
        calls.lock().expect("scan call lock").len(),
        2,
        "closing early must not fetch the rest of the range"
    );
}

#[test]
fn client_iterator_pages_reverse_scans_without_duplicate_boundaries() {
    let row_count = CLIENT_SCAN_PAGE_SIZE as usize * 2 + 1;
    let rows = Arc::new(
        (0..row_count)
            .map(|index| {
                let key = format!("{index:08}").into_bytes();
                (kv::Key(key), vec![b'v'])
            })
            .collect::<Vec<_>>(),
    );
    let scan_rows = Arc::clone(&rows);
    let scan_page = Box::new(
        move |start: Vec<u8>, end: Option<Vec<u8>>, reverse: bool, limit: u32| {
            scan_region_page(
                |_, _| Ok(astersql_store_copr::KeyLocation::default()),
                |start, end, reverse, limit| {
                    assert!(reverse);
                    Ok(scan_rows
                        .iter()
                        .filter(|(key, _)| {
                            key.as_ref() >= start.as_slice()
                                && end.as_ref().is_none_or(|end| key.as_ref() < end.as_slice())
                        })
                        .rev()
                        .take(limit as usize)
                        .cloned()
                        .collect())
                },
                start,
                end,
                reverse,
                limit,
            )
        },
    );
    let mut iterator =
        ClientIterator::paged(scan_page, Vec::new(), None, true, CLIENT_SCAN_PAGE_SIZE)
            .expect("open reverse paged client iterator");
    let mut scanned = Vec::new();
    while kv::Iterator::Valid(&iterator) {
        scanned.push(kv::Iterator::Key(&iterator).0);
        kv::Iterator::Next(&mut iterator).expect("advance reverse page");
    }

    // 反向翻页使用排他边界续扫，跨页结果应严格递减且不重不漏。
    let expected = (0..row_count)
        .rev()
        .map(|index| format!("{index:08}").into_bytes())
        .collect::<Vec<_>>();
    assert_eq!(scanned, expected);
}

#[test]
fn region_scanner_never_sends_one_request_across_region_boundaries() {
    let regions = [
        (Vec::new(), b"m".to_vec()),
        (b"m".to_vec(), b"t".to_vec()),
        (b"t".to_vec(), Vec::new()),
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let scan_page = Box::new(
        move |start: Vec<u8>, end: Option<Vec<u8>>, reverse: bool, limit: u32| {
            scan_region_page(
                |key, reverse| {
                    let (start_key, end_key) = if reverse {
                        regions
                            .iter()
                            .rev()
                            .find(|(start, _)| start.as_slice() < key)
                            .unwrap_or(&regions[0])
                    } else {
                        regions
                            .iter()
                            .find(|(start, end)| {
                                key >= start.as_slice() && (end.is_empty() || key < end.as_slice())
                            })
                            .expect("key belongs to a Region")
                    };
                    Ok(astersql_store_copr::KeyLocation {
                        start_key: start_key.clone(),
                        end_key: end_key.clone(),
                        ..astersql_store_copr::KeyLocation::default()
                    })
                },
                |start, end, reverse, limit| {
                    recorded.lock().expect("request lock").push((
                        start.clone(),
                        end.clone(),
                        reverse,
                        limit,
                    ));
                    let row = if reverse {
                        match end.as_deref() {
                            Some(b"z") => b"y".to_vec(),
                            Some(b"t") => b"s".to_vec(),
                            _ => b"l".to_vec(),
                        }
                    } else {
                        match start.as_slice() {
                            b"a" => b"l".to_vec(),
                            b"m" => b"s".to_vec(),
                            _ => b"y".to_vec(),
                        }
                    };
                    Ok(vec![(kv::Key(row), vec![b'v'])])
                },
                start,
                end,
                reverse,
                limit,
            )
        },
    );
    let mut iterator = ClientIterator::paged(
        scan_page,
        b"a".to_vec(),
        Some(b"z".to_vec()),
        false,
        CLIENT_SCAN_PAGE_SIZE,
    )
    .expect("open Region scanner");
    let mut keys = Vec::new();
    while kv::Iterator::Valid(&iterator) {
        keys.push(kv::Iterator::Key(&iterator).0);
        kv::Iterator::Next(&mut iterator).expect("advance Region scanner");
    }
    assert_eq!(keys, vec![b"l".to_vec(), b"s".to_vec(), b"y".to_vec()]);
    // 每次请求都被裁剪到当前 Region，抵达边界后再从下一个 Region 继续。
    assert_eq!(
        *requests.lock().expect("request lock"),
        vec![
            (
                b"a".to_vec(),
                Some(b"m".to_vec()),
                false,
                CLIENT_SCAN_PAGE_SIZE,
            ),
            (
                b"m".to_vec(),
                Some(b"t".to_vec()),
                false,
                CLIENT_SCAN_PAGE_SIZE,
            ),
            (
                b"t".to_vec(),
                Some(b"z".to_vec()),
                false,
                CLIENT_SCAN_PAGE_SIZE,
            ),
        ]
    );
}

#[test]
fn reverse_region_scanner_uses_exclusive_end_key_routing() {
    let regions = [
        (Vec::new(), b"m".to_vec()),
        (b"m".to_vec(), b"t".to_vec()),
        (b"t".to_vec(), Vec::new()),
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let scan_page = Box::new(
        move |start: Vec<u8>, end: Option<Vec<u8>>, reverse: bool, limit: u32| {
            scan_region_page(
                |key, reverse| {
                    assert!(reverse);
                    let (start_key, end_key) = regions
                        .iter()
                        .rev()
                        .find(|(start, _)| start.as_slice() < key)
                        .unwrap_or(&regions[0]);
                    Ok(astersql_store_copr::KeyLocation {
                        start_key: start_key.clone(),
                        end_key: end_key.clone(),
                        ..astersql_store_copr::KeyLocation::default()
                    })
                },
                |start, end, reverse, limit| {
                    assert!(reverse);
                    recorded.lock().expect("request lock").push((
                        start.clone(),
                        end.clone(),
                        limit,
                    ));
                    let row = match end.as_deref() {
                        Some(b"z") => b"y".to_vec(),
                        Some(b"t") => b"s".to_vec(),
                        _ => b"l".to_vec(),
                    };
                    Ok(vec![(kv::Key(row), vec![b'v'])])
                },
                start,
                end,
                reverse,
                limit,
            )
        },
    );
    let mut iterator = ClientIterator::paged(
        scan_page,
        b"a".to_vec(),
        Some(b"z".to_vec()),
        true,
        CLIENT_SCAN_PAGE_SIZE,
    )
    .expect("open reverse Region scanner");
    let mut keys = Vec::new();
    while kv::Iterator::Valid(&iterator) {
        keys.push(kv::Iterator::Key(&iterator).0);
        kv::Iterator::Next(&mut iterator).expect("advance reverse Region scanner");
    }
    assert_eq!(keys, vec![b"y".to_vec(), b"s".to_vec(), b"l".to_vec()]);
    // 反向定位以排他结束键所属的前一 Region 为起点，随后逐段向低键空间推进。
    assert_eq!(
        *requests.lock().expect("request lock"),
        vec![
            (b"t".to_vec(), Some(b"z".to_vec()), CLIENT_SCAN_PAGE_SIZE,),
            (b"m".to_vec(), Some(b"t".to_vec()), CLIENT_SCAN_PAGE_SIZE,),
            (b"a".to_vec(), Some(b"m".to_vec()), CLIENT_SCAN_PAGE_SIZE,),
        ]
    );
}

#[test]
#[ignore = "requires REAL_TIKV_PD and a running PD/TiKV cluster"]
fn real_client_rust_mvcc_commit_rollback_and_snapshot() {
    // 使用唯一前缀隔离共享集群中的测试数据，并以提交前快照验证 MVCC 可见性。
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");
    let path = format!("tikv://{pd}?disableGC=true");
    let mut driver = TiKVDriver::default();
    let mut store = driver.Open(&path).expect("real TiKV store must open");
    let ctx = kv::Context::todo();

    let suffix = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("real PD must allocate a timestamp")
        .Ver;
    let prefix = format!("astersql/client-rust/mvcc/{suffix}/").into_bytes();
    let committed_key = [prefix.as_slice(), b"committed"].concat();
    let rolled_back_key = [prefix.as_slice(), b"rolled-back"].concat();
    let pessimistic_key = [prefix.as_slice(), b"pessimistic"].concat();
    let scan_end = [prefix.as_slice(), &[0xff]].concat();

    let old_version = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("old snapshot timestamp must be allocated");
    let old_snapshot = kv::Storage::GetSnapshot(&store, old_version);

    let mut optimistic =
        kv::Storage::Begin(&store, &[]).expect("optimistic transaction must begin");
    kv::Mutator::Set(
        optimistic.as_mut(),
        kv::Key(committed_key.clone()),
        b"committed-value".to_vec(),
    )
    .expect("optimistic put must succeed");
    optimistic
        .Commit(&ctx)
        .expect("optimistic commit must succeed");
    println!(
        "optimistic start_ts={} commit_ts={} committed={}",
        optimistic.StartTS(),
        optimistic.CommitTS(),
        String::from_utf8_lossy(b"committed-value")
    );

    let old_value = kv::Getter::Get(
        old_snapshot.as_ref(),
        &ctx,
        kv::Key(committed_key.clone()),
        &[],
    );
    assert!(
        old_value.is_err(),
        "snapshot taken before commit must not see the committed key"
    );

    let new_version = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("new snapshot timestamp must be allocated");
    let new_snapshot = kv::Storage::GetSnapshot(&store, new_version);
    let committed = kv::Getter::Get(
        new_snapshot.as_ref(),
        &ctx,
        kv::Key(committed_key.clone()),
        &[],
    )
    .expect("new snapshot must see committed value");
    assert_eq!(committed.Value, b"committed-value");

    // 回滚写入在新快照与最终范围扫描中都必须保持不可见。
    let mut rolled_back = kv::Storage::Begin(&store, &[]).expect("rollback transaction must begin");
    kv::Mutator::Set(
        rolled_back.as_mut(),
        kv::Key(rolled_back_key.clone()),
        b"must-disappear".to_vec(),
    )
    .expect("rollback put must succeed");
    rolled_back.Rollback().expect("rollback must succeed");

    let after_rollback = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("post-rollback timestamp must be allocated"),
    );
    assert!(
        kv::Getter::Get(
            after_rollback.as_ref(),
            &ctx,
            kv::Key(rolled_back_key.clone()),
            &[],
        )
        .is_err(),
        "rolled-back key must remain absent"
    );
    println!("rollback missing=true");

    // 悲观事务走独立入口，但提交后的值必须遵守相同的快照读取语义。
    let mut pessimistic = store
        .BeginPessimistic()
        .expect("pessimistic transaction must begin");
    kv::Mutator::Set(
        pessimistic.as_mut(),
        kv::Key(pessimistic_key.clone()),
        b"pessimistic-value".to_vec(),
    )
    .expect("pessimistic put must succeed");
    pessimistic
        .Commit(&ctx)
        .expect("pessimistic commit must succeed");
    assert!(pessimistic.IsPessimistic());
    println!(
        "pessimistic start_ts={} commit_ts={} mode={}",
        pessimistic.StartTS(),
        pessimistic.CommitTS(),
        pessimistic.IsPessimistic()
    );

    let scan_snapshot = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("scan timestamp must be allocated"),
    );
    let mut iterator = kv::Retriever::Iter(
        scan_snapshot.as_ref(),
        kv::Key(prefix.clone()),
        Some(kv::Key(scan_end.clone())),
    )
    .expect("snapshot scan must start");
    let mut scanned = Vec::new();
    while iterator.Valid() {
        scanned.push((iterator.Key().0, iterator.Value()));
        iterator.Next().expect("snapshot scan must advance");
    }
    iterator.Close();
    assert!(
        scanned
            .iter()
            .any(|(key, value)| key == &committed_key && value == b"committed-value")
    );
    assert!(
        scanned
            .iter()
            .any(|(key, value)| key == &pessimistic_key && value == b"pessimistic-value")
    );
    assert!(
        scanned.iter().all(|(key, _)| key != &rolled_back_key),
        "snapshot scan must omit rolled-back key"
    );
    println!(
        "snapshot old=missing new={} scan_count={}",
        String::from_utf8_lossy(&committed.Value),
        scanned.len()
    );

    // MaxVersion must also be safe on a real scan, not just point reads.
    let latest = kv::Storage::GetSnapshot(&store, kv::MaxVersion);
    let mut iterator =
        kv::Retriever::Iter(latest.as_ref(), kv::Key(prefix), Some(kv::Key(scan_end)))
            .expect("latest snapshot must scan using a valid PD TSO");
    let mut latest_rows = Vec::new();
    while iterator.Valid() {
        latest_rows.push((iterator.Key().0, iterator.Value()));
        iterator.Next().expect("latest snapshot page must advance");
    }
    iterator.Close();
    assert_eq!(latest_rows, scanned);
    assert!(kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope).is_ok());
    println!(
        "latest MaxVersion scan_count={} PD still available",
        latest_rows.len()
    );

    kv::Storage::Close(&mut store).expect("real TiKV store must close");
}
