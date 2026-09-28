// Copyright 2026 AsterSQL.

//! Real SQL/TiKV fixture for compression parity. Object storage is the sole
//! simulated service, matching the Go suite's fake GCS boundary.
#![allow(non_snake_case)]
use astersql_lightning_mydump::test_support::MemoryStorage;
use astersql_session::runtime::{BootstrapCanonicalDomain, ConcreteSession};
use astersql_session::testutil::TestRecordSet;
use astersql_tests_realtikvtest_importintotest3::harness::{
    compress_framed, fakestorage, gcs_endpoint, mydump,
};
use std::sync::Arc;

pub struct MockGCSSuite {
    pub tk: TestKit,
    pub server: Server,
    _domain: DomainGuard,
}
struct DomainGuard(Arc<astersql_domain::Domain>);
impl Drop for DomainGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}
pub struct Server(Arc<MemoryStorage>);
impl Server {
    pub fn CreateObject(&self, object: fakestorage::Object) {
        let path = format!(
            "gs://{}/{}?endpoint={}",
            object.ObjectAttrs.BucketName,
            object.ObjectAttrs.Name,
            gcs_endpoint()
        );
        self.0.insert(&path, object.Content);
    }
}
pub struct TestKit(ConcreteSession);
#[derive(Debug)]
pub struct ResultSet(Vec<Vec<String>>);
impl ResultSet {
    pub fn Check(&self, expected: &[Vec<&str>]) {
        let actual: Vec<Vec<&str>> = self
            .0
            .iter()
            .map(|row| row.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(actual, expected);
    }
}
impl TestKit {
    pub fn MustExec(&self, sql: &str) {
        self.0.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    pub fn MustQuery(&self, sql: &str) -> ResultSet {
        self.QueryToErr(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }
    pub fn QueryToErr(&self, sql: &str) -> Result<ResultSet, String> {
        let mut result = self.0.execute(sql).map_err(|e| e.to_string())?;
        if result.len() != 1 {
            return Err(format!("expected one record set for {sql}"));
        }
        let mut result = result.pop().unwrap();
        let mut rows = Vec::new();
        while let Some(row) = result.Next().map_err(|e| e.to_string())? {
            rows.push(row);
        }
        result.Close().map_err(|e| e.to_string())?;
        Ok(ResultSet(rows))
    }
}
impl MockGCSSuite {
    pub fn setup() -> Self {
        let pd = std::env::var("REAL_TIKV_PD").unwrap_or_else(|_| "127.0.0.1:2379".into());
        let store = astersql_store_driver::TiKVDriver::default()
            .Open(&format!("tikv://{pd}?disableGC=true"))
            .expect("compression parity requires real TiKV, as in Go");
        let domain = Arc::new(astersql_domain::Domain::new(
            store,
            Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
            astersql_domain::DomainConfig::default(),
        ));
        domain.init().unwrap();
        let session = BootstrapCanonicalDomain(domain.clone()).unwrap();
        let storage = Arc::new(MemoryStorage::default());
        session.SetImportFileStorage(storage.clone());
        Self {
            tk: TestKit(session),
            server: Server(storage),
            _domain: DomainGuard(domain),
        }
    }
    pub fn prepare_and_use_db(&self, name: &str) {
        self.tk
            .MustExec(&format!("drop database if exists `{name}`"));
        self.tk.MustExec(&format!("create database `{name}`"));
        self.tk.MustExec(&format!("use `{name}`"));
    }
    pub fn get_compressed_data(&self, kind: mydump::Compression, data: &[u8]) -> Vec<u8> {
        let compressed = compress_framed(kind, data);
        assert_ne!(compressed, data);
        compressed
    }
    pub fn tear_down(self) {
        drop(self);
    }
}
