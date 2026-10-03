// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use super::precheck::{check_import_size_limit, display_bytes, is_supported_cloud_uri};

#[test]
fn starter_import_real_size_limit_matches_go_boundary() {
    assert!(check_import_size_limit(100, 200, false, 100).is_ok());
    assert!(check_import_size_limit(100, 200, true, 0).is_ok());
    assert!(check_import_size_limit(100, 100, true, 100).is_ok());
    let error = check_import_size_limit(50, 200, true, 100).unwrap_err();
    assert!(error.contains("200B exceeds maximum import size limit 100B"));
    assert!(error.contains("total file size 50B"));
}

#[test]
fn starter_limit_sizes_use_go_units_format() {
    assert_eq!(display_bytes(0), "0B");
    assert_eq!(display_bytes(2), "2B");
    assert_eq!(display_bytes(1024), "1KiB");
    assert_eq!(display_bytes(1536), "1.5KiB");
    assert_eq!(display_bytes(123_456), "120.6KiB");
    assert_eq!(display_bytes(1 << 20), "1MiB");
}

#[test]
fn global_sort_uri_accepts_only_go_cloud_backends_with_a_bucket() {
    for uri in [
        "s3://bucket/path",
        "gcs://bucket/path",
        "gs://bucket/path",
        "azure://container/path",
        "azblob://container/path",
    ] {
        assert!(is_supported_cloud_uri(uri), "expected supported URI: {uri}");
    }

    for uri in [
        ":",
        "s3://",
        "s3:///path",
        "local:///tmp",
        "unknown://bucket",
    ] {
        assert!(!is_supported_cloud_uri(uri), "expected rejected URI: {uri}");
    }
}

#[test]
fn global_sort_missing_bucket_propagates_redacted_invalid_uri() {
    let uri =
        "s3:///path?access-key=secret-id&secret-access-key=secret-key&session-token=secret-token";
    let error = super::precheck::validate_global_sort_uri(uri).unwrap_err();
    let reason = "please specify the bucket for s3 in s3:///path?access-key=xxxxxx&secret-access-key=xxxxxx&session-token=xxxxxx";
    let expected = astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataInvalidURI
        .GenWithStackByArgs(&["cloud storage".into(), reason.into()])
        .to_string();
    assert_eq!(error, expected);
    for secret in ["secret-id", "secret-key", "secret-token"] {
        assert!(!error.contains(secret));
    }
}

use crate as importer;
use std::sync::Arc;
struct ControllerServices;
impl importer::ColumnAssignmentFactory for ControllerServices {
    fn BuildAssignment(
        &self,
        _: &astersql_parser_ast::Assignment,
    ) -> Result<Arc<dyn importer::ColAssignExpressionBuilder>, String> {
        unreachable!()
    }
}
impl importer::ImportDatumConverter for ControllerServices {
    fn CastColumnValue(
        &self,
        _: astersql_lightning_backend_encode::Datum,
        _: &astersql_table::Column,
    ) -> Result<astersql_lightning_backend_encode::Datum, String> {
        unreachable!()
    }
    fn CurrentTime(
        &self,
        _: &astersql_table::Column,
    ) -> Result<astersql_lightning_backend_encode::Datum, String> {
        unreachable!()
    }
}
impl importer::ImportParserFactory for ControllerServices {
    fn NewParser(
        &self,
        _: &str,
        _: Box<dyn astersql_lightning_mydump::ReadSeekCloser>,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &importer::Plan,
    ) -> Result<Box<dyn astersql_lightning_mydump::Parser>, String> {
        unreachable!()
    }
}
impl importer::ImportSizeEstimator for ControllerServices {
    fn EstimateRealSize(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &dyn astersql_objstore_storeapi::Storage,
    ) -> Result<i64, String> {
        unreachable!()
    }
    fn ParquetExpansionRatio(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &str,
        _: i64,
        _: &dyn astersql_objstore_storeapi::Storage,
    ) -> Result<f64, String> {
        unreachable!()
    }
}
impl importer::ImportStorageFactory for ControllerServices {
    fn Open(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &str,
        _: &str,
    ) -> Result<importer::SharedStorage, String> {
        unreachable!()
    }
}
impl importer::TiKVConfigProbe for ControllerServices {
    fn IsRaftKV2(&self) -> Result<bool, String> {
        unreachable!()
    }
}
impl importer::ImportResourceCalculator for ControllerServices {
    fn TargetNodeCPUCnt(&self) -> Result<usize, String> {
        unreachable!()
    }
    fn ScheduleTuneFactors(&self, _: &str) -> Result<importer::ScheduleTuneFactors, String> {
        unreachable!()
    }
    fn SampleIndexSizeRatio(
        &self,
        _: &importer::LoadDataController,
        _: &[u8],
    ) -> Result<f64, String> {
        unreachable!()
    }
    fn Calculate(
        &self,
        _: i64,
        _: usize,
        _: f64,
        _: importer::ScheduleTuneFactors,
    ) -> importer::ResourceParams {
        unreachable!()
    }
}
fn services() -> importer::LoadDataControllerServices {
    let mock = Arc::new(ControllerServices);
    importer::LoadDataControllerServices {
        DatumConverter: mock.clone(),
        AssignmentFactory: mock.clone(),
        ParserFactory: mock.clone(),
        SizeEstimator: mock.clone(),
        StorageFactory: mock.clone(),
        TiKVConfigProbe: mock.clone(),
        ResourceCalculator: mock,
    }
}

struct PrecheckBoundary {
    calls: Vec<&'static str>,
    session: astersql_session::runtime::system_session::SystemSessionLease,
}
impl importer::ImportPrecheckService for PrecheckBoundary {
    fn ActiveJobCount(&mut self, database: &str, table: &str) -> Result<i64, String> {
        self.calls.push("jobs");
        let rows = self.session.query(format!("SELECT COUNT(1) FROM mysql.tidb_import_jobs WHERE status IN ('pending','running') AND table_schema = '{database}' AND table_name = '{table}'"))?;
        rows[0][0]
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())
    }
    fn TableHasRows(&mut self, database: &str, table: &str) -> Result<bool, String> {
        self.calls.push("rows");
        Ok(!self
            .session
            .query(format!("SELECT 1 FROM `{database}`.`{table}` LIMIT 1"))?
            .is_empty())
    }
    fn IsStarterDeployment(&self) -> bool {
        false
    }
    fn StarterMaxImportDataSize(&self) -> u64 {
        0
    }
    fn PiTRTaskNames(&mut self) -> Result<Vec<String>, String> {
        panic!("DisablePrecheck should skip PiTR")
    }
    fn RunningCDCChangefeedsMessage(&mut self) -> Result<Option<String>, String> {
        panic!("DisablePrecheck should skip CDC")
    }
    fn CheckGlobalSortStorePrivileges(
        &mut self,
        _: &str,
        _: &[importer::GlobalSortPermission],
    ) -> Result<(), String> {
        panic!("no global sort")
    }
}
#[test]
fn requirements_reject_enabled_ttl_before_external_checks_in_both_entrypoints() {
    use astersql_planner_core_operator_physicalop::MetadataTableAdapter;
    for source in [importer::DataSourceTypeQuery, importer::DataSourceTypeFile] {
        for enabled in [Some(true), Some(false), None] {
            let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
            let pool =
                astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
            let session = pool.acquire().unwrap();
            session.query("CREATE TABLE IF NOT EXISTS mysql.tidb_import_jobs (table_schema varchar(64), table_name varchar(64), status varchar(64))").unwrap();
            let ttl = match enabled {
                Some(true) => " TTL = `created_at` + INTERVAL 1 DAY",
                Some(false) => " TTL = `created_at` + INTERVAL 1 DAY TTL_ENABLE='OFF'",
                None => "",
            };
            session
                .query(format!(
                    "CREATE TABLE test.t (id int primary key, created_at datetime){ttl}"
                ))
                .unwrap();
            if enabled == Some(true) {
                // TTL must win over the nonempty-table and zero-file errors.
                session
                    .query("INSERT INTO test.t VALUES (17,'2026-10-04 01:02:03')")
                    .unwrap();
            }
            let meta = domain.table_by_name("test", "t").unwrap();
            assert_eq!(meta.TTLInfo.as_ref().map(|ttl| ttl.Enable), enabled);
            let controller = importer::NewLoadDataController(
                importer::Plan {
                    DBName: "test".into(),
                    DataSourceType: source,
                    DisablePrecheck: true,
                    TableInfo: Some(meta.clone()),
                    InImportInto: true,
                    Path: "/file.csv".into(),
                    TotalFileSize: if enabled == Some(true) { 0 } else { 1 },
                    ..Default::default()
                },
                Arc::new(MetadataTableAdapter::New(&meta)),
                importer::ASTArgs::default(),
                services(),
                vec![],
            )
            .unwrap();
            for before_files in [false, true] {
                let mut service = PrecheckBoundary {
                    calls: vec![],
                    session: pool.acquire().unwrap(),
                };
                let result = if before_files {
                    controller.CheckRequirementsBeforeInitDataFiles(&mut service)
                } else {
                    controller.CheckRequirements(&mut service)
                };
                if enabled == Some(true) {
                    let typed = importer::CheckImportTableTTL(&meta).unwrap_err();
                    assert!(
                        astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataPreCheckFailed
                            .Equal(Some(&typed))
                    );
                    assert_eq!(
                        result.unwrap_err(),
                        "[executor:8173]PreCheck failed: target table has TTL enabled, please disable TTL before IMPORT INTO"
                    );
                    assert!(service.calls.is_empty());
                } else {
                    result.unwrap();
                    assert_eq!(
                        service.calls,
                        if source == importer::DataSourceTypeFile {
                            vec!["jobs", "rows"]
                        } else {
                            vec!["rows"]
                        }
                    );
                }
            }
        }
    }
}
