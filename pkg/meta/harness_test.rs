// Copyright 2026 AsterSQL.

// Harness 与 Go `structure` / `codec` / `bytes` / `metadef` 的聚焦契约测试。

use astersql_meta::{ast, bytes, codec, json, kv, metadef, model, structure};

#[test]
fn codec_and_structure_keys_match_go_layout() {
    let encoded_a = [b'a', 0, 0, 0, 0, 0, 0, 0, 0xf8];
    assert_eq!(
        codec::encode_bytes(b"m", b"a"),
        [b"m".as_slice(), encoded_a.as_slice()].concat()
    );

    let transaction = kv::Transaction::default();
    let structure = structure::new_structure(transaction, b"m");

    let mut expected_string = [b"m".as_slice(), encoded_a.as_slice()].concat();
    expected_string.extend_from_slice(&(b's' as u64).to_be_bytes());
    assert_eq!(
        structure.encode_string_data_key(b"a"),
        kv::Key(expected_string)
    );

    let encoded_f = [b'f', 0, 0, 0, 0, 0, 0, 0, 0xf8];
    let mut expected_hash = [b"m".as_slice(), encoded_a.as_slice()].concat();
    expected_hash.extend_from_slice(&(b'h' as u64).to_be_bytes());
    expected_hash.extend_from_slice(&encoded_f);
    assert_eq!(structure.encode_hash_data_key(b"a", b"f"), expected_hash);
}

#[test]
fn prefixes_are_isolated_and_snapshot_is_read_only() {
    let transaction = kv::Transaction::default();
    let first = structure::new_structure(transaction.clone(), b"first");
    let second = structure::new_structure(transaction.clone(), b"second");

    first.set(b"key", b"one").unwrap();
    second.set(b"key", b"two").unwrap();
    first.hset(b"hash", b"field", b"one").unwrap();
    second.hset(b"hash", b"field", b"two").unwrap();

    assert_eq!(first.get(b"key").unwrap(), Some(b"one".to_vec()));
    assert_eq!(second.get(b"key").unwrap(), Some(b"two".to_vec()));
    assert_eq!(
        first.hget(b"hash", b"field").unwrap(),
        Some(b"one".to_vec())
    );
    assert_eq!(
        second.hget(b"hash", b"field").unwrap(),
        Some(b"two".to_vec())
    );

    let snapshot = structure::new_snapshot_structure(transaction.snapshot(), b"first");
    assert!(snapshot.set(b"new", b"value").is_err());
    assert!(snapshot.hset(b"hash", b"new", b"value").is_err());
    assert!(snapshot.clear(b"key").is_err());
    assert!(snapshot.hclear(b"hash").is_err());
}

#[test]
fn integer_and_byte_edges_match_go() {
    let structure = structure::new_structure(kv::Transaction::default(), b"m");
    structure.set(b"empty", b"").unwrap();
    assert!(structure.get_i64(b"empty").is_err());

    structure
        .set(b"max", i64::MAX.to_string().as_bytes())
        .unwrap();
    assert_eq!(structure.inc(b"max", 1).unwrap(), i64::MIN);

    structure
        .hset(b"hash", b"max", i64::MAX.to_string().as_bytes())
        .unwrap();
    assert_eq!(structure.hinc(b"hash", b"max", 1).unwrap(), i64::MIN);

    assert_eq!(bytes::index(b"abc", b""), Some(0));
}

#[test]
fn reverse_iterator_empty_start_and_meta_constants_match_go() {
    let structure = structure::new_structure(kv::Transaction::default(), b"m");
    structure.hset(b"hash", b"a", b"one").unwrap();
    structure.hset(b"hash", b"z", b"two").unwrap();
    let iterator = structure::new_hash_reverse_iter_from(&structure, b"hash", b"").unwrap();
    assert!(iterator.valid());
    assert_eq!(iterator.value(), b"two");

    const RESERVED_GLOBAL_ID_UPPER_BOUND: i64 = 0x0000_FFFF_FFFF_FFFF;
    assert_eq!(
        metadef::MAX_USER_GLOBAL_ID,
        RESERVED_GLOBAL_ID_UPPER_BOUND - 1000
    );
    assert_eq!(metadef::SYSTEM_DATABASE_ID, RESERVED_GLOBAL_ID_UPPER_BOUND);
    assert_eq!(structure::STRING_DATA, b's');
}

#[test]
fn ci_string_and_model_json_match_go_compatibility() {
    let name: ast::CiString = json::unmarshal(br#""TeSt""#).unwrap();
    assert_eq!(name.original, "TeSt");
    assert_eq!(name.lower, "test");

    let database = model::DbInfo::public_system(7, "App", "utf8mb4", "utf8mb4_bin");
    let encoded = json::marshal(&database).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["db_name"]["O"], "App");
    assert!(value.get("name").is_none());

    let ids = model::AutoIdGroup {
        row_id: 1,
        increment_id: 2,
        random_id: 3,
    };
    let value: serde_json::Value = serde_json::from_slice(&json::marshal(&ids).unwrap()).unwrap();
    assert_eq!(value["RowID"], 1);
    assert_eq!(value["IncrementID"], 2);
    assert_eq!(value["RandomID"], 3);
}

#[test]
fn increments_are_atomic_across_clones() {
    let structure = structure::new_structure(kv::Transaction::default(), b"m");
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let structure = structure.clone();
            std::thread::spawn(move || {
                for _ in 0..1_000 {
                    structure.inc(b"counter", 1).unwrap();
                    structure.hinc(b"hash", b"counter", 1).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(structure.get_i64(b"counter").unwrap(), 8_000);
    assert_eq!(structure.hget_i64(b"hash", b"counter").unwrap(), 8_000);
}
