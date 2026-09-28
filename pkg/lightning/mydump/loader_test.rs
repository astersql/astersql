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
// Copyright 2026 AsterSQL.

// MDLoader 单元测试：目录扫描、路由、过滤、视图占位剔除与压缩体积估算。
//
// 使用 MemoryStorage / 自定义 FileIterator 覆盖重复库表、缺失 schema、
// 特殊字符路径、并行处理与扫描选项。

use crate::test_support::MemoryStorage;
use crate::*;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 测试用 LoaderConfig：auto 字符集与 *.* 过滤器。
fn newConfigWithSourceDir() -> LoaderConfig {
    LoaderConfig {
        char_set: "auto".into(),
        filter: vec!["*.*".into()],
        ..Default::default()
    }
}

/// 构造配置与预置文件的 MemoryStorage。
fn newTestMydumpLoaderSuite(files: &[(&str, &[u8])]) -> (LoaderConfig, Arc<MemoryStorage>) {
    (
        newConfigWithSourceDir(),
        Arc::new(MemoryStorage::with(files)),
    )
}

/// 写入空文件。
fn touch(storage: &MemoryStorage, path: &str) {
    storage.insert(path, Vec::new());
}

/// 写入文本文件。
fn writeFile(storage: &MemoryStorage, path: &str, content: &str) {
    storage.insert(path, content.as_bytes().to_vec());
}

/// 占位：模拟创建目录成功。
fn mkdir(_name: &str) -> bool {
    true
}

/// 用给定文件集构造 MDLoader。
fn load(files: &[(&str, &[u8])]) -> Result<MDLoader, MydumpError> {
    let (cfg, storage) = newTestMydumpLoaderSuite(files);
    NewLoaderWithStore(cfg, storage, vec![])
}

/// 基本加载：库、表、视图与 CSV 分片按 sort_key 排序。
#[test]
fn TestLoader() {
    let loader = load(&[
        ("test-schema-create.sql", b"CREATE DATABASE test;"),
        ("test.t-schema.sql", b"CREATE TABLE t(id INT);"),
        ("test.t.0002.csv", b"2\n"),
        ("test.t.0001.csv", b"1\n"),
        (
            "test.v-schema-view.sql",
            b"CREATE VIEW v AS SELECT id FROM t;",
        ),
    ])
    .unwrap();
    assert_eq!(loader.GetDatabases().len(), 1);
    let database = &loader.GetDatabases()[0];
    assert_eq!(database.name, "test");
    assert_eq!(database.tables.len(), 1);
    assert_eq!(database.views.len(), 1);
    assert_eq!(database.tables[0].data_files.len(), 2);
    assert_eq!(database.tables[0].data_files[0].file_meta.sort_key, "0001");
    assert_eq!(loader.GetAllFiles().len(), 3);
}

/// 仅有建库文件的空库。
#[test]
fn TestEmptyDB() {
    let loader = load(&[("empty-schema-create.sql", b"CREATE DATABASE empty;")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].name, "empty");
    assert!(loader.GetDatabases()[0].tables.is_empty());
}

/// 同名库 schema 与 Go 一致报重复错误。
#[test]
fn TestDuplicatedDB() {
    let storage = Arc::new(MemoryStorage::default());
    storage.insert("a/db-schema-create.sql", b"CREATE DATABASE db;".to_vec());
    storage.insert("b/db-schema-create.sql", b"CREATE DATABASE db;".to_vec());
    let error = match NewLoaderWithStore(newConfigWithSourceDir(), storage, vec![]) {
        Ok(_) => panic!("duplicated database schema must fail"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("invalid database schema file, duplicated item - b/db-schema-create.sql")
    );
}

/// 无独立建库文件时从表路径推断库名。
#[test]
fn TestTableNoHostDB() {
    let loader = load(&[("db.t-schema.sql", b"CREATE TABLE t(id INT);")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].name, "db");
    assert_eq!(loader.GetDatabases()[0].tables[0].name, "t");
}

/// 同名表 schema 与 Go 一致报重复错误。
#[test]
fn TestDuplicatedTable() {
    let storage = Arc::new(MemoryStorage::default());
    writeFile(&storage, "a/db.t-schema.sql", "CREATE TABLE t(a INT);");
    writeFile(&storage, "b/db.t-schema.sql", "CREATE TABLE t(b INT);");
    let error = match NewLoaderWithStore(newConfigWithSourceDir(), storage, vec![]) {
        Ok(_) => panic!("duplicated table schema must fail"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("invalid table schema file, duplicated item - b/db.t-schema.sql")
    );
}

/// 表按总数据大小稳定升序排列，与 Go setup 后处理一致。
#[test]
fn tables_are_sorted_by_total_size() {
    let loader = load(&[
        ("db.large.csv", b"123456"),
        ("db.small.csv", b"1"),
        ("db.medium.csv", b"123"),
    ])
    .unwrap();
    let tables = &loader.GetDatabases()[0].tables;
    assert_eq!(
        tables
            .iter()
            .map(|table| table.name.as_str())
            .collect::<Vec<_>>(),
        vec!["small", "medium", "large"]
    );
}

/// 仅有数据文件时 GetSchema 失败。
#[test]
fn TestTableInfoNotFound() {
    let loader = load(&[("db.t.csv", b"1\n")]).unwrap();
    assert!(
        loader.GetDatabases()[0].tables[0]
            .GetSchema(loader.GetStore().as_ref())
            .is_err()
    );
}

/// schema 路径不存在时 GetSchema 报错。
#[test]
fn TestTableUnexpectedError() {
    let storage = MemoryStorage::default();
    let mut table = NewMDTableMeta("auto");
    table.db = "db".into();
    table.name = "t".into();
    table.schema_file.file_meta.path = "missing.sql".into();
    assert!(table.GetSchema(&storage).is_err());
}

/// 仅有 SQL 数据分片、无 schema 文件。
#[test]
fn TestMissingTableSchema() {
    let loader = load(&[("db.t.0001.sql", b"INSERT INTO t VALUES (1);")]).unwrap();
    let table = &loader.GetDatabases()[0].tables[0];
    assert!(table.schema_file.file_meta.path.is_empty());
    assert_eq!(table.data_files.len(), 1);
}

/// 数据文件路径推断库名。
#[test]
fn TestDataNoHostDB() {
    let loader = load(&[("db.t.csv", b"1\n")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].name, "db");
}

/// 数据文件路径推断表名。
#[test]
fn TestDataNoHostTable() {
    let loader = load(&[("db.t.csv", b"1\n")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].tables[0].name, "t");
}

/// 仅有视图 schema 时可接受。
#[test]
fn TestViewWithoutHostDBIsAccepted() {
    let loader = load(&[("db.v-schema-view.sql", b"CREATE VIEW v AS SELECT 1;")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].views[0].name, "v");
}

/// 同名表占位被视图 prune 掉。
#[test]
fn TestViewWithoutHostTableIsAccepted() {
    let loader = load(&[
        ("db.v-schema.sql", b"CREATE TABLE v(id INT);"),
        ("db.v-schema-view.sql", b"CREATE VIEW v AS SELECT 1;"),
    ])
    .unwrap();
    assert!(loader.GetDatabases()[0].tables.is_empty());
    assert_eq!(loader.GetDatabases()[0].views.len(), 1);
}

/// 无 schema 时仍累计 data 文件大小。
#[test]
fn TestDataWithoutSchema() {
    let loader = load(&[("db.t.csv", b"1\n2\n")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].tables[0].total_size, 4);
}

/// 表名含多个点号时正确解析。
#[test]
fn TestTablesWithDots() {
    let loader = load(&[("db.table.with.dots.0001.csv", b"1\n")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].tables[0].name, "table.with.dots");
}

/// filter 包含/排除规则。
#[test]
fn TestRouter() {
    let mut cfg = newConfigWithSourceDir();
    cfg.filter = vec!["db.*".into(), "!db.secret".into()];
    let storage = Arc::new(MemoryStorage::with(&[
        ("db.keep.csv", b"1\n"),
        ("db.secret.csv", b"2\n"),
        ("other.t.csv", b"3\n"),
    ]));
    let loader = NewLoaderWithStore(cfg, storage, vec![]).unwrap();
    assert_eq!(loader.GetDatabases().len(), 1);
    assert_eq!(loader.GetDatabases()[0].tables[0].name, "keep");
    assert!(loader.shouldSkip("db", "secret"));
}

/// 非法正则路由规则导致构造失败。
#[test]
fn TestRoutesPanic() {
    let mut cfg = newConfigWithSourceDir();
    cfg.file_routes = vec![FileRouteRule {
        pattern: "(".into(),
        schema: "db".into(),
        table: "t".into(),
        type_name: TYPE_CSV.into(),
        ..Default::default()
    }];
    assert!(NewLoaderWithStore(cfg, Arc::new(MemoryStorage::default()), vec![]).is_err());
}

/// 空路由规则非法。
#[test]
fn TestBadRouterRule() {
    let mut cfg = newConfigWithSourceDir();
    cfg.file_routes = vec![FileRouteRule::default()];
    assert!(NewLoaderWithStore(cfg, Arc::new(MemoryStorage::default()), vec![]).is_err());
}

/// 精确 path 路由覆盖默认规则。
#[test]
fn TestFileRouting() {
    let mut cfg = newConfigWithSourceDir();
    cfg.file_routes.insert(
        0,
        FileRouteRule {
            path: "external.data".into(),
            schema: "target".into(),
            table: "table".into(),
            type_name: TYPE_CSV.into(),
            key: "99".into(),
            ..Default::default()
        },
    );
    let storage = Arc::new(MemoryStorage::with(&[("external.data", b"1\n")]));
    let loader = NewLoaderWithStore(cfg, storage, vec![]).unwrap();
    let file = &loader.GetDatabases()[0].tables[0].data_files[0];
    assert_eq!(file.file_meta.sort_key, "99");
}

/// URL 编码路径还原库表名。
#[test]
fn TestInputWithSpecialChars() {
    let loader = load(&[("db%20name.table%2Ename.csv", b"1\n")]).unwrap();
    assert_eq!(loader.GetDatabases()[0].name, "db name");
    assert_eq!(loader.GetDatabases()[0].tables[0].name, "table.name");
}

/// 各 SetupOption 正确写入配置。
#[test]
fn TestMDLoaderSetupOption() {
    let mut config = DefaultMDLoaderSetupConfig();
    WithMaxScanFiles(10)(&mut config);
    WithScanFileConcurrency(3)(&mut config);
    WithSkipRealSizeEstimation(true)(&mut config);
    ReturnPartialResultOnError(false)(&mut config);
    assert_eq!(config.max_scan_files, 10);
    assert_eq!(config.scan_file_concurrency, 3);
    assert!(config.skip_real_size_estimation);
    assert!(!config.support_partial_result);
}

/// 自定义 FileIterator 注入外部文件列表。
#[test]
fn TestExternalDataRoutes() {
    let iterator = Arc::new(AllFileIterator {
        files: vec![RawFile {
            path: "db.t.csv".into(),
            size: 123,
        }],
    });
    let loader = NewLoaderWithStore(
        newConfigWithSourceDir(),
        Arc::new(MemoryStorage::default()),
        vec![WithFileIterator(iterator)],
    )
    .unwrap();
    assert_eq!(loader.GetDatabases()[0].tables[0].total_size, 123);
}

/// 统计 open 次数的 Storage，用于压缩采样测试。
struct OpenCounterStorage {
    data: Vec<u8>,
    opens: AtomicUsize,
}
impl OpenCounterStorage {
    fn Open(&self) -> Box<dyn Read + Send> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Box::new(Cursor::new(self.data.clone()))
    }
}
impl Storage for OpenCounterStorage {
    fn open(
        &self,
        _path: &str,
        _compression: Compression,
    ) -> Result<Box<dyn Read + Send>, MydumpError> {
        Ok(self.Open())
    }
}

/// 采样解压比 = 采样字节 / 压缩文件大小。
#[test]
fn TestSampleFileCompressRatio() {
    let storage = OpenCounterStorage {
        data: vec![0; 1000],
        opens: AtomicUsize::new(0),
    };
    let source = SourceFileMeta {
        path: "data.gz".into(),
        compression: Compression::Gz,
        file_size: 100,
        ..Default::default()
    };
    assert_eq!(SampleFileCompressRatio(&source, &storage).unwrap(), 10.0);
    assert_eq!(storage.opens.load(Ordering::SeqCst), 1);
}

/// 压缩与未压缩文件的真实大小估算。
#[test]
fn TestEstimateFileSize() {
    let storage = MemoryStorage::with(&[("data.gz", &[0; 200])]);
    let source = SourceFileMeta {
        path: "data.gz".into(),
        compression: Compression::Gz,
        file_size: 100,
        ..Default::default()
    };
    assert_eq!(EstimateRealSizeForFile(&source, &storage), 200);
    let plain = SourceFileMeta {
        compression: Compression::None,
        file_size: 77,
        ..Default::default()
    };
    assert_eq!(EstimateRealSizeForFile(&plain, &storage), 77);
}

/// WithSkipRealSizeEstimation 开关。
#[test]
fn TestMDLoaderSkipRealSizeEstimation() {
    let mut config = DefaultMDLoaderSetupConfig();
    WithSkipRealSizeEstimation(true)(&mut config);
    assert!(config.skip_real_size_estimation);
}

/// 返回样例 Parquet SourceFileMeta 的 real_size。
fn testSampleParquetDataSize() -> i64 {
    SourceFileMeta {
        source_type: SourceType::Parquet,
        file_size: 40,
        real_size: 400,
        rows: 100,
        ..Default::default()
    }
    .real_size
}

/// 断言 Parquet 样例 real_size。
#[test]
fn TestSampleParquetDataSize() {
    assert_eq!(testSampleParquetDataSize(), 400);
}

/// FileIterator + MaxScanFiles 截断扫描。
#[test]
fn TestSetupOptions() {
    let iterator = Arc::new(AllFileIterator {
        files: vec![
            RawFile {
                path: "db.a.csv".into(),
                size: 1,
            },
            RawFile {
                path: "db.b.csv".into(),
                size: 1,
            },
        ],
    });
    let loader = NewLoaderWithStore(
        newConfigWithSourceDir(),
        Arc::new(MemoryStorage::default()),
        vec![WithFileIterator(iterator), WithMaxScanFiles(1)],
    )
    .unwrap();
    assert_eq!(loader.GetAllFiles().len(), 1);
    assert!(mkdir("virtual"));
}

/// 并行 map 保持顺序，错误提前终止。
#[test]
fn TestParallelProcess() {
    let output = ParallelProcess((0..100).collect(), 8, |value| Ok(value * value)).unwrap();
    assert_eq!(
        output,
        (0..100).map(|value| value * value).collect::<Vec<_>>()
    );
    let error = ParallelProcess(vec![1, 2, 3], 2, |value| {
        if value == 2 {
            Err(MydumpError::Io("stop".into()))
        } else {
            Ok(value)
        }
    })
    .unwrap_err();
    assert!(error.to_string().contains("stop"));
}
