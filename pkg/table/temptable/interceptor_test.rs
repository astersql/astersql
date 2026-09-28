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

// 临时表 Snapshot 拦截器单元测试。
//
// 覆盖 table_id/范围解析、会话键读取、Get/BatchGet/Iter 拦截，
// 以及 UnionIter 创建失败时子迭代器关闭等行为。

use std::sync::Arc;

use crate::main_test::{
    InvokeArg, MOCK_COMMIT_TS, MockedIterHandle, MockedRetriever, MockedSnapshot,
    new_mocked_info_schema, new_mocked_retriever, new_mocked_snapshot,
};
use crate::{
    EmptyIterator, InfoSchema, Key, KvIterator, Retriever, Snapshot, SnapshotInterceptor,
    TempTableError, TempTableType, TemporaryTableSnapshotInterceptor, UnionIter, ValueEntry,
    create_union_iter, encode_table_prefix, get_key_accessed_table_id, get_range_accessed_table_id,
    get_session_key, not_table_range,
};

/// 末字节 +1，用于构造边界键。
fn inc_last_byte(key: &[u8]) -> Key {
    let mut out = key.to_vec();
    if let Some(last) = out.last_mut() {
        *last = last.wrapping_add(1);
    }
    out
}

/// 末字节 -1，用于构造边界键。
fn dec_last_byte(key: &[u8]) -> Key {
    let mut out = key.to_vec();
    if let Some(last) = out.last_mut() {
        *last = last.wrapping_sub(1);
    }
    out
}

/// 表前缀后追加后缀字节。
fn encode_table_key(tbl_id: i64, suffix: &[u8]) -> Key {
    let mut key = encode_table_prefix(tbl_id);
    key.extend_from_slice(suffix);
    key
}

/// 与表 ID 相同的有符号大端编码。
fn encode_int(v: i64) -> Vec<u8> {
    ((v as u64) ^ (1_u64 << 63)).to_be_bytes().to_vec()
}

/// 字面量表前缀字节 `t`。
fn table_prefix() -> Key {
    b"t".to_vec()
}

/// 构造 MockedRetriever.set_data 用的条目。
fn entry(key: Key, value: Option<&[u8]>) -> (Key, Option<Vec<u8>>) {
    (key, value.map(|v| v.to_vec()))
}

/// 耗尽迭代器并收集键值。
fn collect_iter(mut iter: Box<dyn KvIterator>) -> Vec<(Key, Vec<u8>)> {
    let mut result = Vec::new();
    while iter.valid() {
        result.push((iter.key().to_vec(), iter.value().value.clone()));
        iter.next().unwrap();
    }
    result
}

/// 断言创建迭代器失败并返回错误。
fn expect_iter_err(result: Result<Box<dyn KvIterator>, TempTableError>) -> TempTableError {
    match result {
        Ok(_) => panic!("expected iterator error"),
        Err(err) => err,
    }
}

#[test]
/// 解析单键所属 table_id 的边界与合法前缀用例。
fn test_get_key_accessed_table_id() {
    let tb_prefix = table_prefix();
    let prefix0 = encode_table_key(0, &[]);
    let prefix_max = encode_table_key(i64::MAX, &[]);
    let prefix_negative = encode_table_key(-1, &[]);
    let prefix1 = encode_table_key(1, &[]);
    let prefix_a = encode_table_key(i64::MAX / 2, &[]);
    let prefix_b = encode_table_key(i64::MAX - 1, &[]);

    struct Case {
        name: &'static str,
        key: Key,
        ok: bool,
        test_suffix: bool,
        tb_id: i64,
    }

    let cases = [
        Case {
            name: "empty",
            key: vec![],
            ok: false,
            test_suffix: false,
            tb_id: 0,
        },
        Case {
            name: "replace1",
            key: inc_last_byte(&tb_prefix),
            ok: false,
            test_suffix: false,
            tb_id: 0,
        },
        Case {
            name: "replace2",
            key: dec_last_byte(&tb_prefix),
            ok: false,
            test_suffix: false,
            tb_id: 0,
        },
        Case {
            name: "tbPrefix",
            key: tb_prefix.clone(),
            ok: false,
            test_suffix: false,
            tb_id: 0,
        },
        Case {
            name: "back1",
            key: prefix1[..prefix1.len() - 1].to_vec(),
            ok: false,
            test_suffix: false,
            tb_id: 1,
        },
        Case {
            name: "back2",
            key: prefix1[..tb_prefix.len() + 1].to_vec(),
            ok: false,
            test_suffix: false,
            tb_id: 1,
        },
        Case {
            name: "prefix0",
            key: prefix0,
            ok: false,
            test_suffix: true,
            tb_id: 0,
        },
        Case {
            name: "prefixMax",
            key: prefix_max,
            ok: false,
            test_suffix: true,
            tb_id: 0,
        },
        Case {
            name: "prefixNegative",
            key: prefix_negative,
            ok: false,
            test_suffix: true,
            tb_id: 0,
        },
        Case {
            name: "prefix1",
            key: prefix1.clone(),
            ok: true,
            test_suffix: true,
            tb_id: 1,
        },
        Case {
            name: "prefixA",
            key: prefix_a,
            ok: true,
            test_suffix: true,
            tb_id: i64::MAX / 2,
        },
        Case {
            name: "prefixB",
            key: prefix_b,
            ok: true,
            test_suffix: true,
            tb_id: i64::MAX - 1,
        },
    ];

    for c in cases {
        let mut keys = vec![c.key.clone()];
        if c.test_suffix {
            for s in [
                vec![0],
                vec![1],
                vec![0xFF],
                encode_int(0),
                encode_int(i64::MAX / 2),
                encode_int(i64::MAX),
            ] {
                let mut new_key = c.key.clone();
                new_key.extend_from_slice(&s);
                keys.push(new_key);
            }
        }
        for (i, key) in keys.iter().enumerate() {
            let got = get_key_accessed_table_id(key);
            assert_eq!(got.is_some(), c.ok, "{} {}", c.name, i);
            if c.ok {
                assert_eq!(got, Some(c.tb_id), "{} {}", c.name, i);
            } else {
                assert_eq!(got, None, "{} {}", c.name, i);
            }
        }
    }
}

#[test]
/// 判断范围是否落在单一表前缀内。
fn test_get_range_accessed_table_id() {
    let cases: Vec<(Key, Key, bool, i64)> = vec![
        (encode_table_key(1, &[]), encode_table_key(1, &[]), true, 1),
        (
            encode_table_key(1, &[]),
            {
                let mut k = encode_table_key(1, &[]);
                k.push(0);
                k
            },
            true,
            1,
        ),
        (
            encode_table_key(1, &[]),
            {
                let mut k = encode_table_key(1, &[]);
                k.push(0xFF);
                k
            },
            true,
            1,
        ),
        (encode_table_key(1, &[]), encode_table_key(2, &[]), true, 1),
        (table_prefix(), encode_table_key(1, &[]), false, 0),
        (table_prefix(), vec![], false, 0),
        (encode_table_key(0, &[]), encode_table_key(1, &[]), false, 0),
        (encode_table_key(0, &[]), vec![], false, 0),
        (encode_table_key(1, &[]), encode_table_key(5, &[]), false, 0),
        (encode_table_key(1, &[]), vec![], false, 0),
        (
            encode_table_key(1, &[]),
            inc_last_byte(&table_prefix()),
            false,
            0,
        ),
        (
            encode_table_key(1, &[]),
            encode_table_key(1, &[])[..encode_table_key(1, &[]).len() - 1].to_vec(),
            false,
            0,
        ),
        (
            encode_table_key(1, &[])[..encode_table_key(1, &[]).len() - 1].to_vec(),
            encode_table_key(1, &[]),
            false,
            0,
        ),
        (
            encode_table_key(i64::MAX, &[]),
            encode_table_key(i64::MAX, &[0]),
            false,
            0,
        ),
        (vec![], vec![], false, 0),
        (vec![], encode_table_key(2, &[]), false, 0),
    ];
    for (i, (start, end, ok, tb_id)) in cases.into_iter().enumerate() {
        let got = get_range_accessed_table_id(&start, &end);
        assert_eq!(got.is_some(), ok, "case {i}");
        if ok {
            assert_eq!(got, Some(tb_id), "case {i}");
        } else {
            assert_eq!(got, None, "case {i}");
        }
    }
}

#[test]
/// 非表键空间范围判定。
fn test_not_table_range() {
    let false_cases = [
        (vec![], vec![]),
        (vec![], encode_table_key(1, &[0])),
        (vec![], encode_table_key(1, &[1])),
        (encode_table_key(1, &[]), vec![]),
        (encode_table_key(1, &[0]), vec![]),
        (encode_table_key(1, &[]), encode_table_key(1, &[])),
        (encode_table_key(1, &[]), encode_table_key(1, &[0])),
        (encode_table_key(1, &[]), encode_table_key(1, &[1])),
        (encode_table_key(1, &[]), encode_table_key(2, &[])),
        (encode_table_key(1, &[]), encode_table_key(2, &[0])),
        (encode_table_key(1, &[]), encode_table_key(2, &[1])),
        (encode_table_key(1, &[0]), encode_table_key(1, &[1])),
        (encode_table_key(1, &[]), inc_last_byte(&table_prefix())),
    ];
    let tp = table_prefix();
    let true_cases = [
        (vec![], dec_last_byte(&tp)),
        (dec_last_byte(&tp), {
            let mut k = dec_last_byte(&tp);
            k.push(1);
            k
        }),
        (inc_last_byte(&tp), vec![]),
        (inc_last_byte(&tp), {
            let mut k = inc_last_byte(&tp);
            k.push(1);
            k
        }),
    ];
    for c in false_cases {
        assert!(!not_table_range(&c.0, &c.1));
    }
    for c in true_cases {
        assert!(not_table_range(&c.0, &c.1));
    }
}

#[test]
/// get_session_key：本地/全局/普通表与空值删除语义。
fn test_get_session_temporary_table_key() {
    let local_temp_table_data = vec![
        entry(encode_table_key(5, &[]), Some(b"v5")),
        entry(encode_table_key(5, &[0]), Some(b"v50")),
        entry(encode_table_key(5, &[1]), Some(b"v51")),
        entry(encode_table_key(5, &[0, 1]), Some(b"v501")),
        entry(encode_table_key(5, &[2]), Some(b"")),
        entry(encode_table_key(5, &[3]), None),
    ];
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let normal_tb = is.table_by_id(1).unwrap();
    let global_tb = is.table_by_id(3).unwrap();
    let local_tb = is.table_by_id(5).unwrap();
    assert_eq!(normal_tb.metadata().temp_table_type, TempTableType::None);
    assert_eq!(global_tb.metadata().temp_table_type, TempTableType::Global);
    assert_eq!(local_tb.metadata().temp_table_type, TempTableType::Local);

    let retriever = new_mocked_retriever()
        .set_allowed_method(&["Get"])
        .set_data(local_temp_table_data.clone());

    let mut cases = local_temp_table_data;
    cases.push(entry(encode_table_key(5, &[b'n']), Some(b"non-exist-key")));
    for (i, (key, value)) in cases.iter().enumerate() {
        let val = get_session_key(local_tb.metadata().as_ref(), Some(retriever.as_ref()), key);
        let missing = value
            .as_ref()
            .map(|v| v.is_empty() || v == b"non-exist-key")
            .unwrap_or(true)
            || value
                .as_ref()
                .map(|v| v.as_slice() == b"non-exist-key")
                .unwrap_or(false);
        // empty or nil or non-exist marker
        let expect_missing = match value {
            None => true,
            Some(v) if v.is_empty() || v.as_slice() == b"non-exist-key" => true,
            _ => false,
        };
        let _ = missing;
        if expect_missing {
            assert!(matches!(val, Err(TempTableError::KeyNotExist)), "{i}");
        } else {
            assert_eq!(val.unwrap().value, value.clone().unwrap(), "{i}");
        }
        let invokes = retriever.get_invokes();
        assert_eq!(invokes.len(), 1, "{i}");
        assert_eq!(invokes[0].method, "Get");
        assert_eq!(invokes[0].args, vec![InvokeArg::Key(key.clone())]);
        retriever.reset_invokes();

        let val = get_session_key(local_tb.metadata().as_ref(), None, key);
        assert!(matches!(val, Err(TempTableError::KeyNotExist)), "{i}");
        assert!(retriever.get_invokes().is_empty(), "{i}");
    }

    let val = get_session_key(
        global_tb.metadata().as_ref(),
        Some(retriever.as_ref()),
        &encode_table_key(3, &[]),
    );
    assert!(matches!(val, Err(TempTableError::KeyNotExist)));
    assert!(retriever.get_invokes().is_empty());

    let val = get_session_key(
        normal_tb.metadata().as_ref(),
        Some(retriever.as_ref()),
        &encode_table_key(1, &[]),
    );
    assert!(matches!(
        val,
        Err(TempTableError::NormalTableSessionRead(_))
    ));
    assert!(retriever.get_invokes().is_empty());

    let injected = TempTableError::Store("err".into());
    retriever.inject_method_error("Get", Some(injected.clone()));
    let val = get_session_key(
        local_tb.metadata().as_ref(),
        Some(retriever.as_ref()),
        &encode_table_key(5, &[]),
    );
    assert_eq!(val.unwrap_err(), injected);
}

#[test]
/// temporary_table_info_by_id 仅对临时表返回元数据。
fn test_interceptor_temporary_table_info_by_id() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1, 5])
        .add_table(TempTableType::Global, &[2, 6])
        .add_table(TempTableType::Local, &[3, 7]);
    let interceptor = TemporaryTableSnapshotInterceptor::new(is, Some(new_mocked_retriever()));

    assert!(interceptor.temporary_table_info_by_id(1).is_none());
    assert!(interceptor.temporary_table_info_by_id(5).is_none());

    let info = interceptor.temporary_table_info_by_id(2).unwrap();
    assert_eq!(info.name.original(), "tb2");
    assert_eq!(info.temp_table_type, TempTableType::Global);
    let info = interceptor.temporary_table_info_by_id(6).unwrap();
    assert_eq!(info.name.original(), "tb6");
    assert_eq!(info.temp_table_type, TempTableType::Global);

    let info = interceptor.temporary_table_info_by_id(3).unwrap();
    assert_eq!(info.name.original(), "tb3");
    assert_eq!(info.temp_table_type, TempTableType::Local);
    let info = interceptor.temporary_table_info_by_id(7).unwrap();
    assert_eq!(info.name.original(), "tb7");
    assert_eq!(info.temp_table_type, TempTableType::Local);

    assert!(interceptor.temporary_table_info_by_id(4).is_none());
    assert!(interceptor.temporary_table_info_by_id(8).is_none());
}

/// 本地临时表夹具数据。
fn local_temp_data() -> Vec<(Key, Option<Vec<u8>>)> {
    vec![
        entry(encode_table_key(5, &[]), Some(b"v5")),
        entry(encode_table_key(5, &[0]), Some(b"v50")),
        entry(encode_table_key(5, &[1]), Some(b"v51")),
        entry(encode_table_key(5, &[0, 1]), Some(b"v501")),
        entry(encode_table_key(5, &[2]), Some(b"")),
        entry(encode_table_key(5, &[3]), None),
    ]
}

/// 无临时表相关键的夹具。
fn no_temp_data() -> Vec<(Key, Option<Vec<u8>>)> {
    vec![
        entry(encode_table_key(1, &[]), Some(b"v1")),
        entry(encode_table_key(1, &[1]), Some(b"v11")),
        entry(encode_table_key(2, &[]), Some(b"v2")),
        entry(encode_table_key(2, &[1]), Some(b"v21")),
        entry(b"s".to_vec(), Some(b"vs")),
        entry(b"s0".to_vec(), Some(b"vs0")),
        entry(b"u".to_vec(), Some(b"vu")),
        entry(b"u0".to_vec(), Some(b"vu0")),
        entry(table_prefix(), Some(b"vt")),
        entry(encode_table_key(0, &[]), Some(b"v0")),
        entry(encode_table_key(0, &[1]), Some(b"v01")),
        entry(encode_table_key(i64::MAX, &[]), Some(b"vm")),
        entry(encode_table_key(i64::MAX, &[1]), Some(b"vm1")),
    ]
}

#[test]
/// on_get：临时表走会话，普通表走 Snapshot，并校验调用白名单。
fn test_interceptor_on_get() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_allowed_method(&["Get"])
            .set_data(no_temp_data()),
    );
    let retriever = new_mocked_retriever()
        .set_allowed_method(&["Get"])
        .set_data(local_temp_data());
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty_interceptor = TemporaryTableSnapshotInterceptor::new(is, None);

    let mut cases = no_temp_data();
    cases.extend([
        entry(encode_table_key(1, &[b'n']), Some(b"non-exist-key")),
        entry(encode_table_key(2, &[b'n']), Some(b"non-exist-key")),
        entry(b"sn".to_vec(), Some(b"non-exist-key")),
        entry(b"un".to_vec(), Some(b"non-exist-key")),
    ]);

    for (i, (key, value)) in cases.iter().enumerate() {
        for empty_retriever in [false, true] {
            for return_commit_ts in [false, true] {
                let inter = if empty_retriever {
                    &empty_interceptor
                } else {
                    &interceptor
                };
                snap.set_return_commit_ts(return_commit_ts);
                let entry = inter.on_get(snap.as_ref(), key);
                if value.as_ref().map(|v| v.as_slice()) == Some(b"non-exist-key") {
                    assert!(matches!(entry, Err(TempTableError::KeyNotExist)), "{i}");
                } else {
                    let commit_ts = if return_commit_ts { MOCK_COMMIT_TS } else { 0 };
                    assert_eq!(
                        entry.unwrap(),
                        ValueEntry::new(value.clone().unwrap(), commit_ts),
                        "{i} empty={empty_retriever} cts={return_commit_ts}"
                    );
                }
                assert!(retriever.get_invokes().is_empty());
                let invokes = snap.get_invokes();
                assert_eq!(invokes.len(), 1, "{i}");
                assert_eq!(invokes[0].method, "Get");
                assert_eq!(invokes[0].args, vec![InvokeArg::Key(key.clone())]);
                snap.reset_invokes();
            }
        }
    }

    for key in [encode_table_key(3, &[]), encode_table_key(3, &[1])] {
        assert!(matches!(
            interceptor.on_get(snap.as_ref(), &key),
            Err(TempTableError::KeyNotExist)
        ));
        assert!(retriever.get_invokes().is_empty());
        assert!(snap.get_invokes().is_empty());
    }
    assert!(matches!(
        empty_interceptor.on_get(snap.as_ref(), &encode_table_key(3, &[1])),
        Err(TempTableError::KeyNotExist)
    ));

    let mut cases = local_temp_data();
    cases.push(entry(encode_table_key(5, &[b'n']), Some(b"non-exist-key")));
    for (i, (key, value)) in cases.iter().enumerate() {
        for return_commit_ts in [false, true] {
            snap.set_return_commit_ts(return_commit_ts);
            let entry = interceptor.on_get(snap.as_ref(), key);
            let expect_missing = match value {
                None => true,
                Some(v) if v.is_empty() || v.as_slice() == b"non-exist-key" => true,
                _ => false,
            };
            if expect_missing {
                assert!(matches!(entry, Err(TempTableError::KeyNotExist)), "{i}");
            } else {
                assert_eq!(
                    entry.unwrap(),
                    ValueEntry::new(value.clone().unwrap(), 0),
                    "{i}"
                );
            }
            assert!(snap.get_invokes().is_empty(), "{i}");
            let invokes = retriever.get_invokes();
            assert_eq!(invokes.len(), 1, "{i}");
            assert_eq!(invokes[0].method, "Get");
            assert_eq!(invokes[0].args, vec![InvokeArg::Key(key.clone())]);
            retriever.reset_invokes();

            assert!(matches!(
                empty_interceptor.on_get(snap.as_ref(), key),
                Err(TempTableError::KeyNotExist)
            ));
            assert!(snap.get_invokes().is_empty());
            assert!(retriever.get_invokes().is_empty());
        }
    }

    let injected = TempTableError::Store("err1".into());
    snap.inject_method_error("Get", Some(injected.clone()));
    assert_eq!(
        interceptor
            .on_get(snap.as_ref(), &encode_table_key(1, &[]))
            .unwrap_err(),
        injected
    );
    assert!(retriever.get_invokes().is_empty());
    assert_eq!(snap.get_invokes().len(), 1);
    assert_eq!(
        interceptor.on_get(snap.as_ref(), b"s").unwrap_err(),
        injected
    );
    assert_eq!(snap.get_invokes().len(), 2);
    snap.reset_invokes();
    snap.inject_method_error("Get", None);

    let injected = TempTableError::Store("err2".into());
    retriever.inject_method_error("Get", Some(injected.clone()));
    assert_eq!(
        interceptor
            .on_get(snap.as_ref(), &encode_table_key(5, &[]))
            .unwrap_err(),
        injected
    );
    assert!(snap.get_invokes().is_empty());
    assert_eq!(retriever.get_invokes().len(), 1);
}

#[test]
/// batch_get_temporary_table_keys 拆分与会话命中。
fn test_interceptor_batch_get_temporary_table_keys() {
    let mut local = local_temp_data();
    local.extend([
        entry(encode_table_key(8, &[]), Some(b"v8")),
        entry(encode_table_key(8, &[0]), Some(b"v80")),
        entry(encode_table_key(8, &[1]), Some(b"v81")),
        entry(encode_table_key(8, &[0, 1]), Some(b"v801")),
        entry(encode_table_key(8, &[2]), Some(b"")),
        entry(encode_table_key(8, &[3]), None),
    ]);
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1, 4])
        .add_table(TempTableType::Global, &[3, 6])
        .add_table(TempTableType::Local, &[5, 8]);
    let retriever = new_mocked_retriever()
        .set_allowed_method(&["Get"])
        .set_data(local);
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty = TemporaryTableSnapshotInterceptor::new(is, None);

    struct Case {
        keys: Vec<Key>,
        snap_keys: Option<Vec<Key>>,
        nil_session: bool,
        result: Option<Vec<(Key, Vec<u8>)>>,
    }
    let cases = [
        Case {
            keys: vec![],
            snap_keys: Some(vec![]),
            nil_session: false,
            result: None,
        },
        Case {
            keys: vec![
                encode_table_key(3, &[]),
                encode_table_key(3, &[1]),
                encode_table_key(6, &[]),
            ],
            snap_keys: Some(vec![]),
            nil_session: false,
            result: None,
        },
        Case {
            keys: vec![encode_table_key(3, &[]), encode_table_key(5, &[b'n'])],
            snap_keys: Some(vec![]),
            nil_session: false,
            result: None,
        },
        Case {
            keys: vec![encode_table_key(5, &[b'n'])],
            snap_keys: Some(vec![]),
            nil_session: false,
            result: None,
        },
        Case {
            keys: vec![
                encode_table_key(0, &[]),
                encode_table_key(1, &[]),
                encode_table_key(2, &[]),
                encode_table_key(i64::MAX, &[]),
                table_prefix(),
                b"s".to_vec(),
                b"v".to_vec(),
            ],
            snap_keys: Some(vec![
                encode_table_key(0, &[]),
                encode_table_key(1, &[]),
                encode_table_key(2, &[]),
                encode_table_key(i64::MAX, &[]),
                table_prefix(),
                b"s".to_vec(),
                b"v".to_vec(),
            ]),
            nil_session: false,
            result: None,
        },
        Case {
            keys: vec![
                encode_table_key(5, &[]),
                encode_table_key(5, &[2]),
                encode_table_key(5, &[b'n']),
                encode_table_key(8, &[1]),
            ],
            snap_keys: Some(vec![]),
            nil_session: false,
            result: Some(vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(8, &[1]), b"v81".to_vec()),
            ]),
        },
        Case {
            keys: vec![
                encode_table_key(5, &[]),
                encode_table_key(1, &[]),
                encode_table_key(5, &[b'n']),
                encode_table_key(8, &[1]),
            ],
            snap_keys: Some(vec![encode_table_key(1, &[])]),
            nil_session: false,
            result: Some(vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(8, &[1]), b"v81".to_vec()),
            ]),
        },
        Case {
            keys: vec![
                table_prefix(),
                encode_table_key(5, &[]),
                encode_table_key(1, &[]),
                encode_table_key(5, &[2]),
                encode_table_key(5, &[b'n']),
                encode_table_key(8, &[1]),
            ],
            snap_keys: Some(vec![table_prefix(), encode_table_key(1, &[])]),
            nil_session: false,
            result: Some(vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(8, &[1]), b"v81".to_vec()),
            ]),
        },
        Case {
            keys: vec![
                table_prefix(),
                encode_table_key(5, &[]),
                encode_table_key(1, &[]),
                encode_table_key(5, &[2]),
                encode_table_key(5, &[b'n']),
                encode_table_key(8, &[1]),
            ],
            snap_keys: Some(vec![table_prefix(), encode_table_key(1, &[])]),
            nil_session: true,
            result: None,
        },
    ];

    for (i, c) in cases.iter().enumerate() {
        let inter = if c.nil_session { &empty } else { &interceptor };
        let (snap_keys, result) = inter.batch_get_temporary_table_keys(&c.keys).unwrap();
        assert_eq!(snap_keys, c.snap_keys.clone().unwrap_or_default(), "{i}");
        match &c.result {
            None => assert!(result.is_none(), "{i}"),
            Some(expected) => {
                let result = result.expect("result");
                assert_eq!(result.len(), expected.len(), "{i}");
                for (k, v) in expected {
                    assert_eq!(
                        result.get(k).unwrap(),
                        &ValueEntry::new(v.clone(), 0),
                        "{i}"
                    );
                }
            }
        }
        if c.nil_session {
            assert!(retriever.get_invokes().is_empty());
        }
        for invoke in retriever.get_invokes() {
            assert_eq!(invoke.method, "Get");
        }
        retriever.reset_invokes();
    }

    let injected = TempTableError::Store("err".into());
    retriever.inject_method_error("Get", Some(injected.clone()));
    let err = interceptor
        .batch_get_temporary_table_keys(&[
            table_prefix(),
            encode_table_key(5, &[]),
            encode_table_key(1, &[]),
            encode_table_key(5, &[b'n']),
            encode_table_key(8, &[1]),
        ])
        .unwrap_err();
    assert_eq!(err, injected);
}

#[test]
/// on_batch_get：合并会话结果与 Snapshot，临时表值优先。
fn test_interceptor_on_batch_get() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_allowed_method(&["BatchGet"])
            .set_data(no_temp_data()),
    );
    let retriever = new_mocked_retriever()
        .set_allowed_method(&["Get"])
        .set_data(local_temp_data());
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty = TemporaryTableSnapshotInterceptor::new(is, None);

    struct Case {
        keys: Vec<Key>,
        snap_keys: Vec<Key>,
        nil_session: bool,
        result: Vec<(Key, Vec<u8>)>,
    }
    let cases = [
        Case {
            keys: vec![],
            snap_keys: vec![],
            nil_session: false,
            result: vec![],
        },
        Case {
            keys: vec![encode_table_key(3, &[]), encode_table_key(5, &[b'n'])],
            snap_keys: vec![],
            nil_session: false,
            result: vec![],
        },
        Case {
            keys: vec![
                encode_table_key(7, &[]),
                encode_table_key(1, &[b'n']),
                b"o".to_vec(),
            ],
            snap_keys: vec![
                encode_table_key(7, &[]),
                encode_table_key(1, &[b'n']),
                b"o".to_vec(),
            ],
            nil_session: false,
            result: vec![],
        },
        Case {
            keys: vec![
                encode_table_key(3, &[]),
                encode_table_key(5, &[]),
                encode_table_key(5, &[1]),
                encode_table_key(5, &[2]),
            ],
            snap_keys: vec![],
            nil_session: false,
            result: vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
            ],
        },
        Case {
            keys: vec![
                encode_table_key(0, &[]),
                encode_table_key(1, &[]),
                encode_table_key(2, &[]),
                encode_table_key(i64::MAX, &[]),
                table_prefix(),
                b"s".to_vec(),
                b"u".to_vec(),
                encode_table_key(9, &[]),
            ],
            snap_keys: vec![
                encode_table_key(0, &[]),
                encode_table_key(1, &[]),
                encode_table_key(2, &[]),
                encode_table_key(i64::MAX, &[]),
                table_prefix(),
                b"s".to_vec(),
                b"u".to_vec(),
                encode_table_key(9, &[]),
            ],
            nil_session: false,
            result: vec![
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s".to_vec(), b"vs".to_vec()),
                (b"u".to_vec(), b"vu".to_vec()),
            ],
        },
        Case {
            keys: vec![
                table_prefix(),
                encode_table_key(5, &[]),
                encode_table_key(1, &[]),
                encode_table_key(5, &[2]),
                encode_table_key(5, &[b'n']),
                encode_table_key(1, &[b'n']),
            ],
            snap_keys: vec![
                table_prefix(),
                encode_table_key(1, &[]),
                encode_table_key(1, &[b'n']),
            ],
            nil_session: false,
            result: vec![
                (table_prefix(), b"vt".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
            ],
        },
        Case {
            keys: vec![
                table_prefix(),
                encode_table_key(5, &[]),
                encode_table_key(1, &[]),
                encode_table_key(5, &[2]),
                encode_table_key(5, &[b'n']),
                encode_table_key(1, &[b'n']),
            ],
            snap_keys: vec![
                table_prefix(),
                encode_table_key(1, &[]),
                encode_table_key(1, &[b'n']),
            ],
            nil_session: true,
            result: vec![
                (table_prefix(), b"vt".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
            ],
        },
    ];

    for (i, c) in cases.iter().enumerate() {
        for return_commit_ts in [false, true] {
            let inter = if c.nil_session { &empty } else { &interceptor };
            snap.set_return_commit_ts(return_commit_ts);
            let result = inter.on_batch_get(snap.as_ref(), &c.keys).unwrap();
            assert_eq!(result.len(), c.result.len(), "{i}");
            for (k, v) in &c.result {
                let commit_ts = if return_commit_ts && c.snap_keys.iter().any(|sk| sk == k) {
                    MOCK_COMMIT_TS
                } else {
                    0
                };
                assert_eq!(
                    result.get(k).unwrap(),
                    &ValueEntry::new(v.clone(), commit_ts),
                    "{i}"
                );
            }
            if c.nil_session {
                assert!(retriever.get_invokes().is_empty());
            }
            if c.snap_keys.is_empty() {
                assert!(snap.get_invokes().is_empty(), "{i}");
            } else {
                assert_eq!(snap.get_invokes().len(), 1, "{i}");
                assert_eq!(snap.get_invokes()[0].method, "BatchGet");
                assert_eq!(
                    snap.get_invokes()[0].args,
                    vec![InvokeArg::Keys(c.snap_keys.clone())]
                );
            }
            retriever.reset_invokes();
            snap.reset_invokes();
        }
    }

    let session_err = TempTableError::Store("errSession".into());
    retriever.inject_method_error("Get", Some(session_err.clone()));
    assert_eq!(
        interceptor
            .on_batch_get(snap.as_ref(), &[encode_table_key(5, &[])])
            .unwrap_err(),
        session_err
    );
    retriever.inject_method_error("Get", None);

    let snap_err = TempTableError::Store("errSnap".into());
    snap.inject_method_error("BatchGet", Some(snap_err.clone()));
    assert_eq!(
        interceptor
            .on_batch_get(snap.as_ref(), &[encode_table_key(1, &[])])
            .unwrap_err(),
        snap_err
    );
}

#[test]
/// create_union_iter 正常合并正/反向扫描结果。
fn test_create_union_iter() {
    let retriever = new_mocked_retriever().set_data(vec![
        entry(b"k1".to_vec(), Some(b"v1")),
        entry(b"k10".to_vec(), Some(b"")),
        entry(b"k11".to_vec(), Some(b"v11")),
        entry(b"k5".to_vec(), Some(b"v5")),
    ]);
    let snap = new_mocked_snapshot(new_mocked_retriever().set_data(vec![
        entry(b"k2".to_vec(), Some(b"v2")),
        entry(b"k20".to_vec(), Some(b"v20")),
        entry(b"k21".to_vec(), Some(b"v21")),
    ]));
    retriever.set_allowed_method(&["Iter", "IterReverse"]);
    snap.set_allowed_method(&["Iter", "IterReverse"]);

    struct Case {
        args: Vec<Key>,
        reverse: bool,
        nil_sess: bool,
        nil_snap: bool,
        result: Vec<(Key, Vec<u8>)>,
    }
    let cases = [
        Case {
            args: vec![b"k1".to_vec(), b"k21".to_vec()],
            reverse: false,
            nil_sess: false,
            nil_snap: false,
            result: vec![
                (b"k1".to_vec(), b"v1".to_vec()),
                (b"k11".to_vec(), b"v11".to_vec()),
                (b"k2".to_vec(), b"v2".to_vec()),
                (b"k20".to_vec(), b"v20".to_vec()),
            ],
        },
        Case {
            args: vec![b"k21".to_vec()],
            reverse: true,
            nil_sess: false,
            nil_snap: false,
            result: vec![
                (b"k20".to_vec(), b"v20".to_vec()),
                (b"k2".to_vec(), b"v2".to_vec()),
                (b"k11".to_vec(), b"v11".to_vec()),
                (b"k1".to_vec(), b"v1".to_vec()),
            ],
        },
        Case {
            args: vec![b"k1".to_vec(), b"k21".to_vec()],
            reverse: false,
            nil_sess: false,
            nil_snap: true,
            result: vec![
                (b"k1".to_vec(), b"v1".to_vec()),
                (b"k11".to_vec(), b"v11".to_vec()),
            ],
        },
        Case {
            args: vec![b"k21".to_vec()],
            reverse: true,
            nil_sess: false,
            nil_snap: true,
            result: vec![
                (b"k11".to_vec(), b"v11".to_vec()),
                (b"k1".to_vec(), b"v1".to_vec()),
            ],
        },
        Case {
            args: vec![b"k1".to_vec(), b"k21".to_vec()],
            reverse: false,
            nil_sess: true,
            nil_snap: false,
            result: vec![
                (b"k2".to_vec(), b"v2".to_vec()),
                (b"k20".to_vec(), b"v20".to_vec()),
            ],
        },
        Case {
            args: vec![b"k21".to_vec()],
            reverse: true,
            nil_sess: true,
            nil_snap: false,
            result: vec![
                (b"k20".to_vec(), b"v20".to_vec()),
                (b"k2".to_vec(), b"v2".to_vec()),
            ],
        },
    ];

    for (i, c) in cases.iter().enumerate() {
        let sess: Option<&dyn Retriever> = if c.nil_sess {
            None
        } else {
            Some(retriever.as_ref())
        };
        let snap_arg: Option<&dyn Snapshot> = if c.nil_snap {
            None
        } else {
            Some(snap.as_ref())
        };
        let iter = if c.reverse {
            create_union_iter(sess, snap_arg, &[], &c.args[0], true).unwrap()
        } else {
            create_union_iter(sess, snap_arg, &c.args[0], &c.args[1], false).unwrap()
        };
        if c.nil_sess && c.nil_snap {
            assert!(iter.as_any().is::<EmptyIterator>(), "{i}");
        } else if !c.nil_sess {
            assert!(iter.as_any().is::<UnionIter>(), "{i}");
        } else {
            assert!(iter.as_any().is::<MockedIterHandle>(), "{i}");
        }
        if !c.nil_sess {
            assert_eq!(retriever.get_invokes().len(), 1, "{i}");
            assert_eq!(
                retriever.get_invokes()[0].method,
                if c.reverse { "IterReverse" } else { "Iter" }
            );
        }
        if !c.nil_snap {
            assert_eq!(snap.get_invokes().len(), 1, "{i}");
        }
        assert_eq!(collect_iter(iter), c.result, "{i}");
        retriever.reset_invokes();
        snap.reset_invokes();
    }
}

/// 创建失败后断言两侧迭代器均已关闭。
fn check_created_iter_closed(retriever: &MockedRetriever, snap: &MockedSnapshot, reverse: bool) {
    let method = if reverse { "IterReverse" } else { "Iter" };
    assert!(retriever.get_invokes().len() <= 1);
    for invoke in retriever.get_invokes() {
        assert_eq!(invoke.method, method);
        if invoke.ret_err.is_none() {
            let iter = invoke.ret_iter.as_ref().unwrap();
            assert!(iter.lock().unwrap().closed());
        }
    }
    assert!(snap.get_invokes().len() <= 1);
    for invoke in snap.get_invokes() {
        assert_eq!(invoke.method, method);
        if invoke.ret_err.is_none() {
            let iter = invoke.ret_iter.as_ref().unwrap();
            assert!(iter.lock().unwrap().closed());
        }
    }
}

#[test]
/// 会话或快照 Iter 失败时的资源清理。
fn test_error_create_union_iter() {
    let retriever = new_mocked_retriever()
        .set_allowed_method(&["Iter", "IterReverse"])
        .set_data(vec![entry(b"k1".to_vec(), Some(b""))]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_allowed_method(&["Iter", "IterReverse"])
            .set_data(vec![entry(b"k1".to_vec(), Some(b"v1"))]),
    );

    let iter_next_err = TempTableError::Store("iterNextErr".into());
    retriever.inject_method_error("IterNext", Some(iter_next_err.clone()));
    let err = expect_iter_err(create_union_iter(
        Some(retriever.as_ref()),
        Some(snap.as_ref()),
        b"k1",
        b"k2",
        false,
    ));
    assert_eq!(err, iter_next_err);
    check_created_iter_closed(&retriever, &snap, false);
    retriever.reset_invokes();
    snap.reset_invokes();
    retriever.inject_method_error("IterNext", None);

    let iter_reverse_next_err = TempTableError::Store("iterReverseNextErr".into());
    retriever.inject_method_error("IterReverseNext", Some(iter_reverse_next_err.clone()));
    let err = expect_iter_err(create_union_iter(
        Some(retriever.as_ref()),
        Some(snap.as_ref()),
        &[],
        b"k2",
        true,
    ));
    assert_eq!(err, iter_reverse_next_err);
    check_created_iter_closed(&retriever, &snap, true);
    retriever.reset_invokes();
    snap.reset_invokes();
    retriever.inject_method_error("IterReverseNext", None);

    let session_iter_err = TempTableError::Store("sessionIterErr".into());
    retriever.inject_method_error("Iter", Some(session_iter_err.clone()));
    for snap_arg in [Some(snap.as_ref() as &dyn Snapshot), None] {
        let err = expect_iter_err(create_union_iter(
            Some(retriever.as_ref()),
            snap_arg,
            b"k1",
            b"k2",
            false,
        ));
        assert_eq!(err, session_iter_err);
        check_created_iter_closed(&retriever, &snap, false);
        retriever.reset_invokes();
        snap.reset_invokes();
    }
    retriever.inject_method_error("Iter", None);

    let session_iter_reverse_err = TempTableError::Store("sessionIterReverseErr".into());
    retriever.inject_method_error("IterReverse", Some(session_iter_reverse_err.clone()));
    for _ in 0..2 {
        let err = expect_iter_err(create_union_iter(
            Some(retriever.as_ref()),
            Some(snap.as_ref()),
            &[],
            b"k2",
            true,
        ));
        assert_eq!(err, session_iter_reverse_err);
        check_created_iter_closed(&retriever, &snap, true);
        retriever.reset_invokes();
        snap.reset_invokes();
    }
    retriever.inject_method_error("IterReverse", None);

    let snap_iter_err = TempTableError::Store("snapIterError".into());
    snap.inject_method_error("Iter", Some(snap_iter_err.clone()));
    for _ in 0..2 {
        let err = expect_iter_err(create_union_iter(
            None,
            Some(snap.as_ref()),
            b"k1",
            b"k2",
            false,
        ));
        assert_eq!(err, snap_iter_err);
        check_created_iter_closed(&retriever, &snap, false);
        retriever.reset_invokes();
        snap.reset_invokes();
    }
    snap.inject_method_error("Iter", None);

    let snap_iter_reverse_err = TempTableError::Store("snapIterError".into());
    snap.inject_method_error("IterReverse", Some(snap_iter_reverse_err.clone()));
    for _ in 0..2 {
        let err = expect_iter_err(create_union_iter(
            None,
            Some(snap.as_ref()),
            &[],
            b"k2",
            true,
        ));
        assert_eq!(err, snap_iter_reverse_err);
        check_created_iter_closed(&retriever, &snap, true);
        retriever.reset_invokes();
        snap.reset_invokes();
    }
}

#[test]
/// iter_table：非临时/Global/Local 三条路径。
fn test_iter_table() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let retriever = new_mocked_retriever()
        .set_data(local_temp_data())
        .set_allowed_method(&["Iter"]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_data(vec![
                entry(encode_table_key(1, &[]), Some(b"v1")),
                entry(encode_table_key(1, &[1]), Some(b"v11")),
                entry(encode_table_key(2, &[]), Some(b"v2")),
                entry(encode_table_key(2, &[1]), Some(b"v21")),
            ])
            .set_allowed_method(&["Iter"]),
    );
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty = TemporaryTableSnapshotInterceptor::new(Arc::clone(&is) as _, None);

    struct Case {
        tbl_id: i64,
        nil_session: bool,
        args: [Key; 2],
        result: Vec<(Key, Vec<u8>)>,
    }
    let cases = [
        Case {
            tbl_id: 1,
            nil_session: false,
            args: [encode_table_key(1, &[]), encode_table_key(2, &[])],
            result: vec![
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
            ],
        },
        Case {
            tbl_id: 1,
            nil_session: false,
            args: [encode_table_key(1, &[]), encode_table_key(1, &[1])],
            result: vec![(encode_table_key(1, &[]), b"v1".to_vec())],
        },
        Case {
            tbl_id: 2,
            nil_session: false,
            args: [encode_table_key(2, &[]), encode_table_key(3, &[])],
            result: vec![
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
            ],
        },
        Case {
            tbl_id: 3,
            nil_session: false,
            args: [encode_table_key(3, &[]), encode_table_key(4, &[])],
            result: vec![],
        },
        Case {
            tbl_id: 4,
            nil_session: false,
            args: [encode_table_key(4, &[]), encode_table_key(5, &[])],
            result: vec![],
        },
        Case {
            tbl_id: 5,
            nil_session: false,
            args: [encode_table_key(5, &[]), encode_table_key(6, &[])],
            result: vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
            ],
        },
        Case {
            tbl_id: 5,
            nil_session: false,
            args: [encode_table_key(5, &[]), encode_table_key(5, &[1])],
            result: vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
            ],
        },
        Case {
            tbl_id: 5,
            nil_session: true,
            args: [encode_table_key(5, &[]), encode_table_key(5, &[1])],
            result: vec![],
        },
    ];

    for (i, c) in cases.iter().enumerate() {
        let inter = if c.nil_session { &empty } else { &interceptor };
        let iter = inter
            .iter_table(c.tbl_id, snap.as_ref(), &c.args[0], &c.args[1])
            .unwrap();
        assert_eq!(collect_iter(iter), c.result, "{i}");

        let tbl = is.table_by_id(c.tbl_id);
        if tbl.as_ref().map(|t| t.metadata().temp_table_type) != Some(TempTableType::Local)
            && tbl.as_ref().map(|t| t.metadata().temp_table_type) != Some(TempTableType::Global)
        {
            // normal / missing table reads snapshot
            if tbl.is_none()
                || tbl.as_ref().unwrap().metadata().temp_table_type == TempTableType::None
            {
                assert!(retriever.get_invokes().is_empty(), "{i}");
                assert_eq!(snap.get_invokes().len(), 1, "{i}");
                assert_eq!(snap.get_invokes()[0].method, "Iter");
            }
        }
        if let Some(tbl) = &tbl {
            if tbl.metadata().temp_table_type == TempTableType::Global {
                assert!(retriever.get_invokes().is_empty(), "{i}");
                assert!(snap.get_invokes().is_empty(), "{i}");
            }
            if tbl.metadata().temp_table_type == TempTableType::Local {
                assert!(snap.get_invokes().is_empty(), "{i}");
                if c.nil_session {
                    assert!(retriever.get_invokes().is_empty(), "{i}");
                } else {
                    assert_eq!(retriever.get_invokes().len(), 1, "{i}");
                    assert_eq!(retriever.get_invokes()[0].method, "Iter");
                }
            }
        }
        snap.reset_invokes();
        retriever.reset_invokes();
    }

    let snap_err = TempTableError::Store("snapErr".into());
    snap.inject_method_error("Iter", Some(snap_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.iter_table(
            1,
            snap.as_ref(),
            &encode_table_key(1, &[]),
            &encode_table_key(2, &[])
        )),
        snap_err
    );
    snap.inject_method_error("Iter", None);

    let retriever_err = TempTableError::Store("retrieverErr".into());
    retriever.inject_method_error("Iter", Some(retriever_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.iter_table(
            5,
            snap.as_ref(),
            &encode_table_key(5, &[]),
            &encode_table_key(6, &[])
        )),
        retriever_err
    );
}

#[test]
/// on_iter：非表范围、单表范围与跨表 union。
fn test_on_iter() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let retriever = new_mocked_retriever()
        .set_data(local_temp_data())
        .set_allowed_method(&["Iter"]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_data(no_temp_data())
            .set_allowed_method(&["Iter"]),
    );
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty = TemporaryTableSnapshotInterceptor::new(is, None);

    let cases: Vec<(bool, Key, Key, Vec<(Key, Vec<u8>)>)> = vec![
        (
            false,
            vec![],
            vec![],
            vec![
                (b"s".to_vec(), b"vs".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (b"u".to_vec(), b"vu".to_vec()),
                (b"u0".to_vec(), b"vu0".to_vec()),
            ],
        ),
        (
            false,
            vec![],
            b"s0".to_vec(),
            vec![(b"s".to_vec(), b"vs".to_vec())],
        ),
        (
            false,
            b"u".to_vec(),
            vec![],
            vec![
                (b"u".to_vec(), b"vu".to_vec()),
                (b"u0".to_vec(), b"vu0".to_vec()),
            ],
        ),
        (
            false,
            encode_table_key(1, &[]),
            vec![],
            vec![
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (b"u".to_vec(), b"vu".to_vec()),
                (b"u0".to_vec(), b"vu0".to_vec()),
            ],
        ),
        (
            false,
            vec![],
            encode_table_key(1, &[1]),
            vec![
                (b"s".to_vec(), b"vs".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
            ],
        ),
        (
            false,
            encode_table_key(1, &[]),
            encode_table_key(2, &[]),
            vec![
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
            ],
        ),
        (
            false,
            encode_table_key(1, &[]),
            encode_table_key(3, &[]),
            vec![
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
            ],
        ),
        (
            false,
            encode_table_key(3, &[]),
            encode_table_key(4, &[]),
            vec![],
        ),
        (
            false,
            encode_table_key(5, &[]),
            encode_table_key(5, &[3]),
            vec![
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
            ],
        ),
        (
            true,
            vec![],
            vec![],
            vec![
                (b"s".to_vec(), b"vs".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (b"u".to_vec(), b"vu".to_vec()),
                (b"u0".to_vec(), b"vu0".to_vec()),
            ],
        ),
        (
            true,
            encode_table_key(5, &[]),
            encode_table_key(5, &[3]),
            vec![],
        ),
    ];

    for (i, (nil_session, start, end, expected)) in cases.iter().enumerate() {
        let inter = if *nil_session { &empty } else { &interceptor };
        let iter = inter.on_iter(snap.as_ref(), start, end).unwrap();
        assert_eq!(collect_iter(iter), *expected, "{i}");
    }

    let snap_err = TempTableError::Store("snapErr".into());
    snap.inject_method_error("Iter", Some(snap_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.on_iter(snap.as_ref(), &[], &[])),
        snap_err
    );
    assert_eq!(
        expect_iter_err(interceptor.on_iter(snap.as_ref(), &[], b"o")),
        snap_err
    );
    assert_eq!(
        expect_iter_err(interceptor.on_iter(
            snap.as_ref(),
            &encode_table_key(4, &[]),
            &encode_table_key(5, &[])
        )),
        snap_err
    );
    snap.inject_method_error("Iter", None);

    let retriever_err = TempTableError::Store("retrieverErr".into());
    retriever.inject_method_error("Iter", Some(retriever_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.on_iter(snap.as_ref(), &[], &[])),
        retriever_err
    );
    assert_eq!(
        expect_iter_err(interceptor.on_iter(
            snap.as_ref(),
            &encode_table_key(5, &[]),
            &encode_table_key(6, &[])
        )),
        retriever_err
    );
}

#[test]
/// on_iter_reverse：反向扫描拦截路径。
fn test_on_iter_reverse() {
    let is = new_mocked_info_schema()
        .add_table(TempTableType::None, &[1])
        .add_table(TempTableType::Global, &[3])
        .add_table(TempTableType::Local, &[5]);
    let retriever = new_mocked_retriever()
        .set_data(local_temp_data())
        .set_allowed_method(&["IterReverse"]);
    let snap = new_mocked_snapshot(
        new_mocked_retriever()
            .set_data(no_temp_data())
            .set_allowed_method(&["IterReverse"]),
    );
    let interceptor = TemporaryTableSnapshotInterceptor::new(
        Arc::clone(&is) as _,
        Some(Arc::clone(&retriever) as _),
    );
    let empty = TemporaryTableSnapshotInterceptor::new(is, None);

    let cases: Vec<(bool, Key, Key, Vec<(Key, Vec<u8>)>)> = vec![
        (
            false,
            vec![],
            vec![],
            vec![
                (b"u0".to_vec(), b"vu0".to_vec()),
                (b"u".to_vec(), b"vu".to_vec()),
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (b"s".to_vec(), b"vs".to_vec()),
            ],
        ),
        (
            false,
            b"u".to_vec(),
            vec![],
            vec![
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (b"s".to_vec(), b"vs".to_vec()),
            ],
        ),
        (
            false,
            encode_table_key(5, &[0, 1]),
            vec![],
            vec![
                (encode_table_key(5, &[0]), b"v50".to_vec()),
                (encode_table_key(5, &[]), b"v5".to_vec()),
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (b"s".to_vec(), b"vs".to_vec()),
            ],
        ),
        (
            false,
            b"s0".to_vec(),
            vec![],
            vec![(b"s".to_vec(), b"vs".to_vec())],
        ),
        (
            true,
            encode_table_key(5, &[0, 1]),
            vec![],
            vec![
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
                (b"s".to_vec(), b"vs".to_vec()),
            ],
        ),
        (
            true,
            encode_table_key(5, &[0, 1]),
            b"s0".to_vec(),
            vec![
                (encode_table_key(2, &[1]), b"v21".to_vec()),
                (encode_table_key(2, &[]), b"v2".to_vec()),
                (encode_table_key(1, &[1]), b"v11".to_vec()),
                (encode_table_key(1, &[]), b"v1".to_vec()),
                (encode_table_key(0, &[1]), b"v01".to_vec()),
                (encode_table_key(0, &[]), b"v0".to_vec()),
                (table_prefix(), b"vt".to_vec()),
                (b"s0".to_vec(), b"vs0".to_vec()),
            ],
        ),
        (
            false,
            b"u".to_vec(),
            encode_table_key(5, &[0, 1]),
            vec![
                (encode_table_key(i64::MAX, &[1]), b"vm1".to_vec()),
                (encode_table_key(i64::MAX, &[]), b"vm".to_vec()),
                (encode_table_key(5, &[1]), b"v51".to_vec()),
                (encode_table_key(5, &[0, 1]), b"v501".to_vec()),
            ],
        ),
    ];

    for (i, (nil_session, end, lower, expected)) in cases.iter().enumerate() {
        let inter = if *nil_session { &empty } else { &interceptor };
        let iter = inter.on_iter_reverse(snap.as_ref(), end, lower).unwrap();
        assert_eq!(collect_iter(iter), *expected, "{i}");
    }

    let snap_err = TempTableError::Store("snapErr".into());
    snap.inject_method_error("IterReverse", Some(snap_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.on_iter_reverse(snap.as_ref(), &[], &[])),
        snap_err
    );
    assert_eq!(
        expect_iter_err(interceptor.on_iter_reverse(snap.as_ref(), b"o", &[])),
        snap_err
    );
    snap.inject_method_error("IterReverse", None);

    let retriever_err = TempTableError::Store("retrieverErr".into());
    retriever.inject_method_error("IterReverse", Some(retriever_err.clone()));
    assert_eq!(
        expect_iter_err(interceptor.on_iter_reverse(snap.as_ref(), &[], &[])),
        retriever_err
    );
}
