// Copyright 2026 AsterSQL.

use super::*;
use crate::runtime::CreateAnalyzeSession;
use crate::testutil::TestRecordSet;
use astersql_lightning_mydump::test_support::MemoryStorage;
use astersql_objstore_compressedio::new_buffer;
use std::io::Write;

fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut results = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    assert_eq!(results.len(), 1);
    let mut result = results.pop().unwrap();
    let mut rows = Vec::new();
    while let Some(row) = result.Next().unwrap() {
        rows.push(row);
    }
    result.Close().unwrap();
    rows
}

#[test]
fn canonical_compressed_csv_and_sql_preserve_rows_and_skip_per_file() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("create database compressed_parity")
        .unwrap();
    session.execute("use compressed_parity").unwrap();
    session
        .execute("create table t (i int primary key, s varchar(32))")
        .unwrap();
    let storage = Arc::new(MemoryStorage::default());
    session.SetImportFileStorage(storage.clone());
    for (kind, extension) in [
        (CompressType::Gzip, "gz"),
        (CompressType::Zstd, "zstd"),
        (CompressType::Snappy, "snappy"),
    ] {
        for (format, content) in [
            ("csv", b"1,test1\n2,test2".as_slice()),
            (
                "sql",
                b"INSERT INTO unrelated.other VALUES (1,'test1'),(2,'test2');".as_slice(),
            ),
        ] {
            let mut buffer = new_buffer(64, kind);
            buffer.write_all(content).unwrap();
            buffer.close().unwrap();
            let source = format!("gs://compressed/data.{format}.{extension}");
            storage.insert(&source, buffer.bytes());
            session.execute("truncate table t").unwrap();
            rows(
                &session,
                &format!("import into t from '{source}' with thread=1"),
            );
            assert_eq!(
                rows(&session, "select * from t order by i"),
                vec![vec!["1", "test1"], vec!["2", "test2"]]
            );
        }
    }
    storage.insert("gs://mixed/a.csv", b"1,a\n2,b");
    let mut buffer = new_buffer(64, CompressType::Gzip);
    buffer.write_all(b"3,c\n4,d").unwrap();
    buffer.close().unwrap();
    storage.insert("gs://mixed/b.csv.gzip", buffer.bytes());
    session.execute("truncate table t").unwrap();
    rows(
        &session,
        "import into t from 'gs://mixed/*' with skip_rows=1, thread=1",
    );
    assert_eq!(
        rows(&session, "select * from t order by i"),
        vec![vec!["2", "b"], vec!["4", "d"]]
    );
}

#[test]
fn canonical_corrupt_file_keeps_table_empty() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    session.execute("create database corrupt_parity").unwrap();
    session.execute("use corrupt_parity").unwrap();
    session
        .execute("create table t (i int primary key, s varchar(32))")
        .unwrap();
    let storage = Arc::new(MemoryStorage::default());
    storage.insert("gs://bad/a.csv", b"1,a");
    storage.insert("gs://bad/b.csv.gz", b"broken gzip");
    session.SetImportFileStorage(storage);
    let error = session
        .execute("import into t from 'gs://bad/*' with thread=1")
        .err()
        .unwrap();
    assert!(error.to_string().contains("decompress"), "{error}");
    assert!(rows(&session, "select * from t").is_empty());
}
