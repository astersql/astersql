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

// 需串行执行的 HTTP handler 测试。通过真实 status TCP listener 覆盖路由，
// 并在 SQL、PD、TiKV 等外部边界使用带状态的测试运行时保留 Go 副作用语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use astersql_domain_serverinfo::{Context as ServerInfoContext, Syncer as ServerInfoSyncer};
use astersql_server_handler_tikvhandler::tikv_handler::{
    Request as TikvRequest, ResponseWriter as TikvResponseWriter,
};
use astersql_server_handler_tikvhandler::{
    CIStr, ClusterServerInfo, Config, Context, DBInfo, DBTableInfo, Data, Error, FieldType,
    IndexInfo, IndexRegions, InfoSchema, IngestParam, Job, Map, PartitionInfo, PathValues,
    PhysicalTable, RegionDetail, RegionMeta, SchemaTableStorage, ServerInfo, Stats, Storage, Table,
    TableFlashReplicaInfo, TableInfo, TableRegions, TikvRuntime, TxnGCStatesHandler, UrlValues,
    ValuePayload, tableFlashReplicaStatus,
};

pub(super) static CTC_DDL_HOOK_ENABLED: AtomicBool = AtomicBool::new(false);
static INGEST_BATCH_SPLIT_RANGES: AtomicU64 = AtomicU64::new(2048_f64.to_bits());
static INGEST_SPLIT_RANGES_PER_SEC: AtomicU64 = AtomicU64::new(0_f64.to_bits());
static INGEST_INFLIGHT: AtomicU64 = AtomicU64::new(0_f64.to_bits());
static INGEST_PER_SECOND: AtomicU64 = AtomicU64::new(0_f64.to_bits());
static LAST_GC_SAFEPOINT: AtomicU64 = AtomicU64::new(0);
static TIFLASH_REPLICA_MODE: AtomicU64 = AtomicU64::new(0);
static TIFLASH_REPLICA_AVAILABLE: AtomicU64 = AtomicU64::new(0);
static APPLIED_SETTINGS: OnceLock<Mutex<std::collections::HashMap<String, String>>> =
    OnceLock::new();
type LabelServerInfoSyncer = Arc<Mutex<Box<ServerInfoSyncer>>>;

static LABEL_SERVER_INFO_SYNCER: OnceLock<Mutex<Option<LabelServerInfoSyncer>>> = OnceLock::new();

fn applied_settings() -> &'static Mutex<std::collections::HashMap<String, String>> {
    APPLIED_SETTINGS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn label_server_info_syncer() -> &'static Mutex<Option<LabelServerInfoSyncer>> {
    LABEL_SERVER_INFO_SYNCER.get_or_init(|| Mutex::new(None))
}

/// Installs the optional server-info bridge used by the etcd label integration
/// test.  Dropping the guard restores the default in-process-only runtime.
pub(super) struct LabelServerInfoSyncerGuard;

impl Drop for LabelServerInfoSyncerGuard {
    fn drop(&mut self) {
        *label_server_info_syncer()
            .lock()
            .expect("label server-info syncer lock") = None;
    }
}

pub(super) fn install_label_server_info_syncer(
    syncer: LabelServerInfoSyncer,
) -> LabelServerInfoSyncerGuard {
    *label_server_info_syncer()
        .lock()
        .expect("label server-info syncer lock") = Some(syncer);
    LabelServerInfoSyncerGuard
}

fn ingest_slot(param: IngestParam) -> &'static AtomicU64 {
    match param {
        "max_batch_split_ranges" => &INGEST_BATCH_SPLIT_RANGES,
        "max_split_ranges_per_sec" => &INGEST_SPLIT_RANGES_PER_SEC,
        "max_inflight" => &INGEST_INFLIGHT,
        "max_per_second" => &INGEST_PER_SECOND,
        _ => unreachable!("test runtime only receives supported ingest parameters"),
    }
}

fn json_u64(body: &[u8], key: &str) -> Result<u64, Error> {
    let text = std::str::from_utf8(body).map_err(|_| Error {
        message: "invalid TiFlash status payload".into(),
    })?;
    let marker = format!("\"{key}\"");
    let value = text
        .split_once(&marker)
        .and_then(|(_, suffix)| suffix.split_once(':'))
        .map(|(_, suffix)| {
            suffix
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error {
            message: format!("missing TiFlash status field {key}"),
        })?;
    value.parse::<u64>().map_err(|_| Error {
        message: format!("invalid TiFlash status field {key}"),
    })
}

/// 串行 status-handler 用例的运行时边界。
///
/// 除当前断言所需的 GC 状态外，其余调用一律显式报错，避免测试意外依赖
/// 未接入的 SQL、PD 或 TiKV 行为而得到固定成功。
pub(super) struct SerialHandlerRuntime;

impl SerialHandlerRuntime {
    fn unexpected<T>() -> Result<T, Error> {
        Err(Error {
            message: "unexpected runtime call".into(),
        })
    }
}

impl TikvRuntime for SerialHandlerRuntime {
    fn schema(&self) -> Result<InfoSchema, Error> {
        Ok(InfoSchema { tables: Vec::new() })
    }
    fn pd_region_stats(&self, _: i64, _: bool) -> Result<Stats, Error> {
        Self::unexpected()
    }
    fn mvcc_by_hex(&self, _: &PathValues) -> Result<Data, Error> {
        Ok(Data("mvcc-hex".into()))
    }
    fn table(&self, database: &str, table: &str) -> Result<Table, Error> {
        if database != "tidb" || !matches!(table, "test" | "pt") {
            return Err(Error {
                message: "table not exists".into(),
            });
        }
        Ok(Table {
            meta: TableInfo {
                id: 42,
                name: table.into(),
                indices: Vec::new(),
                partitions: Vec::new(),
                is_common_handle: false,
                tiflash_replica: None,
                tiflash_replica_infos: Vec::new(),
            },
        })
    }
    fn handle(&self, _: &Table, params: &PathValues, _: &UrlValues) -> Result<Data, Error> {
        Ok(Data(format!("handle:{}", params.get("handle"))))
    }
    fn mvcc_by_index(
        &self,
        _: &Table,
        index: &str,
        values: UrlValues,
        handle: Data,
    ) -> Result<Data, Error> {
        if !matches!(index, "idx" | "idx1" | "idx2") {
            return Err(Error {
                message: "index not found".into(),
            });
        }
        let a = values.get("a");
        let b = values.get("b");
        if a.is_empty() || !values.contains("b") {
            return Err(Error {
                message: "missing index column value".into(),
            });
        }
        if a == "5" {
            return Ok(Data("mvcc-index-not-found".into()));
        }
        Ok(Data(format!(
            "mvcc-index:{index}:{}:a={a}:b={b}:b_count={}",
            handle.0,
            values.count("b")
        )))
    }
    fn mvcc_by_record(&self, _: &Table, handle: Data, decode: bool) -> Result<Data, Error> {
        if handle.0 == "handle:1234" {
            return Ok(Data("mvcc-not-found".into()));
        }
        Ok(Data(format!("mvcc-record:{}:decode={decode}", handle.0)))
    }
    fn table_id(&self, database: &str, table: &str) -> Result<i64, Error> {
        if database == "tidb" && table == "test" {
            Ok(42)
        } else {
            Err(Error {
                message: "table not exists".into(),
            })
        }
    }
    fn mvcc_by_start_ts(&self, start_ts: u64, _: Vec<u8>, _: Vec<u8>) -> Result<Data, Error> {
        Ok(Data(format!("mvcc-txn:{start_ts}")))
    }
    fn pd_addresses(&self) -> Result<Vec<String>, Error> {
        Self::unexpected()
    }
    fn decode_row(&self, bytes: &[u8], column_id: i64, _: FieldType) -> Result<String, Error> {
        Ok(format!(
            "decoded-column-{column_id}:{}",
            String::from_utf8_lossy(bytes)
        ))
    }
    fn global_config(&self) -> Config {
        Config(String::new())
    }
    fn apply_setting(&self, key: &str, value: &str) -> Result<(), Error> {
        applied_settings()
            .lock()
            .expect("settings lock")
            .insert(key.into(), value.into());
        Ok(())
    }
    fn decode_flash_status(&self, body: &[u8]) -> Result<tableFlashReplicaStatus, Error> {
        Ok(tableFlashReplicaStatus {
            id: json_u64(body, "id")? as i64,
            region_count: json_u64(body, "region_count")?,
            flash_region_count: json_u64(body, "flash_region_count")?,
        })
    }
    fn update_table_replica(&self, table_id: i64, available: bool) -> Result<(), Error> {
        let bit = match (TIFLASH_REPLICA_MODE.load(Ordering::Acquire), table_id) {
            (1, 1) => 0,
            (2, 100..=102) => u32::try_from(table_id - 100).expect("partition bit index"),
            _ => {
                return Err(Error {
                    message: format!("Table which ID = {table_id} does not exist."),
                });
            }
        };
        let mask = 1_u64 << bit;
        if available {
            TIFLASH_REPLICA_AVAILABLE.fetch_or(mask, Ordering::AcqRel);
        } else {
            TIFLASH_REPLICA_AVAILABLE.fetch_and(!mask, Ordering::AcqRel);
        }
        Ok(())
    }
    fn historical_tiflash(&self, _: &InfoSchema) -> Result<Vec<TableFlashReplicaInfo>, Error> {
        let ids: &[i64] = match TIFLASH_REPLICA_MODE.load(Ordering::Acquire) {
            0 => &[],
            1 => &[1],
            2 => &[100, 101, 102],
            _ => unreachable!("test runtime uses a known TiFlash replica mode"),
        };
        let available = TIFLASH_REPLICA_AVAILABLE.load(Ordering::Acquire);
        Ok(ids
            .iter()
            .enumerate()
            .map(|(index, id)| TableFlashReplicaInfo {
                id: *id,
                replica_count: 2,
                location_labels: vec!["a".into(), "b".into()],
                available: available & (1_u64 << index) != 0,
                high_priority: false,
            })
            .collect())
    }
    fn schema_storage(
        &self,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Vec<SchemaTableStorage>, Error> {
        if schema != Some("test") || !matches!(table, None | Some("t")) {
            return Ok(Vec::new());
        }
        Ok(vec![SchemaTableStorage {
            table_schema: "test".into(),
            table_name: "t".into(),
            table_rows: 3,
            avg_row_length: 16,
            data_length: 48,
            max_data_length: 0,
            index_length: 0,
            data_free: 0,
        }])
    }
    fn resolve_schema_route(
        &self,
        request: &TikvRequest,
    ) -> Result<(Option<CIStr>, Option<CIStr>, bool), Error> {
        let schema = request.path.get("schema").map(|value| CIStr(value.clone()));
        let table = request.path.get("table").map(|value| CIStr(value.clone()));
        if schema.as_ref().is_some_and(|value| value.0 != "test") {
            return Err(Error {
                message: "database not exists".into(),
            });
        }
        if table.as_ref().is_some_and(|value| value.0 != "t") {
            return Err(Error {
                message: "table not exists".into(),
            });
        }
        let is_single = table.is_some();
        Ok((schema, table, is_single))
    }
    fn resolve_schema_request(&self, _: &InfoSchema, request: &TikvRequest) -> Result<Data, Error> {
        if let Some(database) = request.path.get("db") {
            if database != "tidb" {
                return Err(Error {
                    message: "database not exists".into(),
                });
            }
            if let Some(table) = request.path.get("table") {
                if table != "t" {
                    return Err(Error {
                        message: "table not exists".into(),
                    });
                }
                return Ok(Data(format!("schema-table-name:{database}/{table}")));
            }
            return Ok(Data(format!("schema-db:{database}")));
        }
        if request.path.get("tableID").is_none_or(String::is_empty) {
            Ok(Data("schema-catalog".into()))
        } else {
            Ok(Data(format!(
                "schema-table:{}",
                request.path.get("tableID").expect("checked above")
            )))
        }
    }
    fn resolve_table_route(&self, request: &TikvRequest) -> Result<(Table, String), Error> {
        if request.path.get("db").map(String::as_str) != Some("tidb") {
            return Err(Error {
                message: "table not exists".into(),
            });
        }
        let table = match request.path.get("table").map(String::as_str) {
            Some("t") => TableInfo {
                id: 42,
                name: "t".into(),
                indices: vec![
                    IndexInfo {
                        id: 0,
                        name: "PRIMARY".into(),
                        primary: true,
                    },
                    IndexInfo {
                        id: 1,
                        name: "idx".into(),
                        primary: false,
                    },
                ],
                partitions: Vec::new(),
                is_common_handle: false,
                tiflash_replica: None,
                tiflash_replica_infos: Vec::new(),
            },
            Some("pt") => TableInfo {
                id: 100,
                name: "pt".into(),
                indices: vec![
                    IndexInfo {
                        id: 0,
                        name: "PRIMARY".into(),
                        primary: true,
                    },
                    IndexInfo {
                        id: 1,
                        name: "idx".into(),
                        primary: false,
                    },
                ],
                partitions: vec![
                    PartitionInfo {
                        id: 100,
                        name: "p0".into(),
                    },
                    PartitionInfo {
                        id: 101,
                        name: "p1".into(),
                    },
                    PartitionInfo {
                        id: 102,
                        name: "p2".into(),
                    },
                ],
                is_common_handle: false,
                tiflash_replica: None,
                tiflash_replica_infos: Vec::new(),
            },
            _ => {
                return Err(Error {
                    message: "table not exists".into(),
                });
            }
        };
        Ok((Table { meta: table }, String::new()))
    }
    fn history_ddl(&self, job_id: i32, limit: i32) -> Result<Vec<Job>, Error> {
        if job_id == 0 && limit == 0 {
            return Ok(vec![Job { id: 3 }, Job { id: 2 }]);
        }
        Ok(vec![Job {
            id: i64::from(job_id),
        }])
    }
    fn resign_ddl_owner(&self) -> Result<(), Error> {
        Ok(())
    }
    fn execute_admin_check(&self, _: Context, sql: &str) -> Result<Vec<Vec<String>>, Error> {
        if sql.contains("idx_not_exist") {
            return Err(Error {
                message: "index not found".into(),
            });
        }
        Ok(vec![vec!["check passed".into()]])
    }
    fn add_scatter(&self, _: Vec<u8>, _: Vec<u8>, _: &str) -> Result<(), Error> {
        Self::unexpected()
    }
    fn delete_scatter(&self, _: &str) -> Result<(), Error> {
        Self::unexpected()
    }
    fn scatter_table(&self, _: PhysicalTable, _: bool) -> Result<(), Error> {
        Self::unexpected()
    }
    fn table_regions(&self, table: &Table) -> Result<Vec<TableRegions>, Error> {
        let physical_tables = if table.meta.partitions.is_empty() {
            vec![(table.meta.id, table.meta.name.clone(), 11)]
        } else {
            table
                .meta
                .partitions
                .iter()
                .map(|partition| (partition.id, partition.name.clone(), partition.id as u64))
                .collect()
        };
        Ok(physical_tables
            .into_iter()
            .map(|(table_id, table_name, region_id)| TableRegions {
                table_name,
                table_id,
                record_regions: vec![RegionMeta { id: region_id }],
                indices: vec![
                    IndexRegions {
                        name: "PRIMARY".into(),
                        id: 0,
                        regions: vec![RegionMeta { id: region_id }],
                    },
                    IndexRegions {
                        name: "idx".into(),
                        id: 1,
                        regions: vec![RegionMeta { id: region_id }],
                    },
                ],
            })
            .collect())
    }
    fn scan_regions(&self, _: &Table, _: i64, _: &str) -> Result<TableRegions, Error> {
        Self::unexpected()
    }
    fn region_route(&self, request: &TikvRequest) -> Result<Data, Error> {
        match request.path.get("route").map(String::as_str) {
            Some("meta") => Ok(Data("region-meta".into())),
            Some("hot") => Err(Error {
                message: "hot region metrics are unavailable".into(),
            }),
            _ => Self::unexpected(),
        }
    }
    fn region_detail(&self, region_id: u64) -> Result<RegionDetail, Error> {
        if region_id != 11 && !(100..=102).contains(&region_id) {
            return Err(Error {
                message: "region not found".into(),
            });
        }
        let (table_name, table_id) = if (100..=102).contains(&region_id) {
            (format!("pt(p{})", region_id - 100), region_id as i64)
        } else {
            ("t".into(), 42)
        };
        Ok(RegionDetail {
            range_detail: astersql_server_handler_tikvhandler::createRangeDetail(
                b"a".to_vec(),
                b"z".to_vec(),
            ),
            region_id,
            frames: vec![
                astersql_server_handler_tikvhandler::FrameItem {
                    db_name: "tidb".into(),
                    table_name: table_name.clone(),
                    table_id,
                    is_record: true,
                    record_id: 1,
                    index_name: "PRIMARY".into(),
                    index_id: 0,
                    index_values: Vec::new(),
                },
                astersql_server_handler_tikvhandler::FrameItem {
                    db_name: "tidb".into(),
                    table_name,
                    table_id,
                    is_record: false,
                    record_id: 0,
                    index_name: "idx".into(),
                    index_id: 1,
                    index_values: vec!["1".into()],
                },
            ],
        })
    }
    fn decode_mvcc(
        &self,
        _: Vec<u8>,
        _: Map<FieldType>,
        _: &TableInfo,
    ) -> Result<Map<String>, Error> {
        Self::unexpected()
    }
    fn server_info(&self) -> Result<ServerInfo, Error> {
        Self::unexpected()
    }
    fn cluster_server_info(&self) -> Result<ClusterServerInfo, Error> {
        Self::unexpected()
    }
    fn db_table_info(&self, table_id: &str) -> Result<DBTableInfo, Error> {
        if table_id != "42" {
            return Err(Error {
                message: "table id not exists".into(),
            });
        }
        Ok(DBTableInfo {
            db_info: DBInfo {
                name: "test".into(),
            },
            table_info: TableInfo {
                id: 42,
                name: "handler_table".into(),
                indices: Vec::new(),
                partitions: Vec::new(),
                is_common_handle: false,
                tiflash_replica: None,
                tiflash_replica_infos: Vec::new(),
            },
            schema_version: 7,
        })
    }
    fn profile(&self, _: &TikvRequest) -> Result<Vec<u8>, Error> {
        Self::unexpected()
    }
    fn delete_encoded_key(&self, _: PathValues, _: UrlValues) -> Result<Vec<u8>, Error> {
        Ok(vec![0xDE, 0xAD])
    }
    fn resolve_locks(&self, _: Context, safe_point: u64) -> Result<(), Error> {
        LAST_GC_SAFEPOINT.store(safe_point, Ordering::Release);
        Ok(())
    }
    fn set_ctc_ddl_hook(&self, enabled: bool) -> Result<(), Error> {
        CTC_DDL_HOOK_ENABLED.store(enabled, Ordering::Release);
        Ok(())
    }
    fn update_server_labels(&self, body: &[u8]) -> Result<Map<String>, Error> {
        let text = std::str::from_utf8(body).map_err(|_| Error {
            message: "labels payload must be UTF-8 JSON".into(),
        })?;
        let labels = text
            .trim()
            .strip_prefix('{')
            .and_then(|body| body.strip_suffix('}'))
            .ok_or_else(|| Error {
                message: "labels payload must be a JSON object".into(),
            })?
            .split(',')
            .filter(|field| !field.trim().is_empty())
            .map(|field| {
                let (key, value) = field.split_once(':').ok_or_else(|| Error {
                    message: "invalid label field".into(),
                })?;
                Ok((
                    key.trim().trim_matches('"').to_owned(),
                    value.trim().trim_matches('"').to_owned(),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        if let Some(syncer) = label_server_info_syncer()
            .lock()
            .expect("label server-info syncer lock")
            .clone()
        {
            syncer
                .lock()
                .expect("server-info syncer lock")
                .UpdateServerLabel(
                    ServerInfoContext::Background(),
                    labels.iter().cloned().collect(),
                )
                .map_err(|error| Error {
                    message: error.to_string(),
                })?;
        }
        astersql_config::update_global(|config| {
            for (key, value) in &labels {
                config.labels.insert(key.clone(), value.clone());
            }
        });
        Ok(Map::single("message", "success".into()))
    }
    fn ingest_get(&self, param: IngestParam) -> Result<Map<String>, Error> {
        let value = f64::from_bits(ingest_slot(param).load(Ordering::Acquire));
        Ok(Map::single("value", value.to_string()))
    }
    fn ingest_set(&self, param: IngestParam, value: f64) -> Result<f64, Error> {
        let slot = ingest_slot(param);
        let old = f64::from_bits(slot.swap(value.to_bits(), Ordering::AcqRel));
        Ok(old)
    }
    fn decode_value_payload(&self, body: &[u8]) -> Result<ValuePayload, Error> {
        let value = std::str::from_utf8(body)
            .ok()
            .and_then(|body| body.trim().strip_prefix('{'))
            .and_then(|body| body.strip_suffix('}'))
            .and_then(|body| body.split_once(':').map(|(_, value)| value.trim()))
            .and_then(|value| value.trim_matches('"').parse::<f64>().ok())
            .ok_or_else(|| Error {
                message: "invalid ingest value payload".into(),
            })?;
        Ok(ValuePayload { value })
    }
    fn gc_state(&self) -> Result<Data, Error> {
        Ok(Data("gc-state".into()))
    }
    fn log_error(&self, _: &str, _: &Error) {}
    fn log_info(&self, _: &str) {}
}

// dummyRecord 对应 Go 函数 `func dummyRecord() *deadlockhistory.DeadlockRecord {`。
// 这是辅助函数：保留参数读取、错误处理和资源收尾语义说明。
/// dummyRecord 对应 Go 函数 `func dummyRecord() *deadlockhistory.DeadlockRecord {`。
pub fn dummy_record() {
    // Go 原始签名: func dummyRecord() *deadlockhistory.DeadlockRecord {
    // 返回值: return &deadlockhistory.DeadlockRecord{}
}

#[test]
// TestPostSettings 对应 Go 函数 `func TestPostSettings(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestPostSettings 对应 Go 函数 `func TestPostSettings(t *testing.T) {`。
pub fn test_post_settings() {
    applied_settings().lock().expect("settings lock").clear();
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .post_status(
            "/settings",
            "application/x-www-form-urlencoded",
            b"log_level=error&tidb_general_log=1&tidb_enable_async_commit=1&tidb_enable_1pc=1",
        )
        .expect("POST settings must reach SettingsHandler");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), "null");
    let settings = applied_settings().lock().expect("settings lock");
    assert_eq!(settings.get("log_level").map(String::as_str), Some("error"));
    assert_eq!(
        settings.get("tidb_general_log").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        settings.get("tidb_enable_async_commit").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        settings.get("tidb_enable_1pc").map(String::as_str),
        Some("1")
    );
    assert!(
        !settings.contains_key("ddl_slow_threshold"),
        "an omitted settings field must not be applied as an empty value"
    );
    drop(settings);

    let invalid = suite
        .client
        .post_status(
            "/settings",
            "application/x-www-form-urlencoded",
            b"tidb_general_log=invalid",
        )
        .expect("invalid settings value must reach SettingsHandler");
    assert_eq!(invalid.status, 400);
    assert!(invalid.text().unwrap().contains("illegal argument"));
    assert_eq!(
        applied_settings()
            .lock()
            .expect("settings lock")
            .get("tidb_general_log")
            .map(String::as_str),
        Some("1"),
        "invalid settings input must not overwrite the previously accepted value"
    );

    let go_boolean = suite
        .client
        .post_status(
            "/settings",
            "application/x-www-form-urlencoded",
            b"deadlock_history_collect_retryable=1",
        )
        .expect("Go-compatible boolean settings value must reach SettingsHandler");
    assert_eq!(go_boolean.status, 200);
    assert_eq!(
        applied_settings()
            .lock()
            .expect("settings lock")
            .get("deadlock_history_collect_retryable")
            .map(String::as_str),
        Some("1")
    );

    let disabled = suite
        .client
        .post_status(
            "/settings",
            "application/x-www-form-urlencoded",
            b"log_level=fatal&tidb_general_log=0&tidb_enable_async_commit=0&tidb_enable_1pc=0&ddl_slow_threshold=200&check_mb4_value_in_utf8=0&deadlock_history_capacity=5&deadlock_history_collect_retryable=false",
        )
        .expect("the second Go settings phase must reach SettingsHandler");
    assert_eq!(disabled.status, 200);
    let settings = applied_settings().lock().expect("settings lock");
    for (key, expected) in [
        ("log_level", "fatal"),
        ("tidb_general_log", "0"),
        ("tidb_enable_async_commit", "0"),
        ("tidb_enable_1pc", "0"),
        ("ddl_slow_threshold", "200"),
        ("check_mb4_value_in_utf8", "0"),
        ("deadlock_history_capacity", "5"),
        ("deadlock_history_collect_retryable", "false"),
    ] {
        assert_eq!(
            settings.get(key).map(String::as_str),
            Some(expected),
            "{key}"
        );
    }
    drop(settings);

    let ignored_threshold = suite
        .client
        .post_status(
            "/settings",
            "application/x-www-form-urlencoded",
            b"ddl_slow_threshold=0",
        )
        .expect("non-positive DDL thresholds must be accepted and ignored");
    assert_eq!(ignored_threshold.status, 200);
    assert_eq!(
        applied_settings()
            .lock()
            .expect("settings lock")
            .get("ddl_slow_threshold")
            .map(String::as_str),
        Some("200")
    );

    // Go 原始签名: func TestPostSettings(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)
    // 状态准备: se, err := session.CreateSession(ts.store)
    // 错误处理: require.NoError(t, err)

    // 状态准备: form := make(url.Values)
    // 迁移语句: form.Set("log_level", "error")
    // 迁移语句: form.Set("tidb_general_log", "1")
    // 迁移语句: form.Set("tidb_enable_async_commit", "1")
    // 迁移语句: form.Set("tidb_enable_1pc", "1")
    // HTTP 表单请求: resp, err := ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, zap.ErrorLevel, log.GetLevel())
    // 断言: require.Equal(t, "error", config.GetGlobalConfig().Log.Level)
    // 断言: require.True(t, vardef.ProcessGeneralLog.Load())
    // 上下文: val, err := se.GetSessionVars().GetGlobalSystemVar(context.Background(), vardef.TiDBEnableAsyncCommit)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, vardef.On, val)
    // 上下文: val, err = se.GetSessionVars().GetGlobalSystemVar(context.Background(), vardef.TiDBEnable1PC)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, vardef.On, val)

    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("log_level", "fatal")
    // 迁移语句: form.Set("tidb_general_log", "0")
    // 迁移语句: form.Set("tidb_enable_async_commit", "0")
    // 迁移语句: form.Set("tidb_enable_1pc", "0")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.False(t, vardef.ProcessGeneralLog.Load())
    // 断言: require.Equal(t, zap.FatalLevel, log.GetLevel())
    // 断言: require.Equal(t, "fatal", config.GetGlobalConfig().Log.Level)
    // 上下文: val, err = se.GetSessionVars().GetGlobalSystemVar(context.Background(), vardef.TiDBEnableAsyncCommit)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, vardef.Off, val)
    // 上下文: val, err = se.GetSessionVars().GetGlobalSystemVar(context.Background(), vardef.TiDBEnable1PC)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, vardef.Off, val)
    // 文件系统: form.Set("log_level", os.Getenv("log_level"))

    // 保留 Go 注释: // test ddl_slow_threshold
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("ddl_slow_threshold", "200")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, uint32(200), atomic.LoadUint32(&vardef.DDLSlowOprThreshold))

    // 保留 Go 注释: // test check_mb4_value_in_utf8
    // 外部依赖/数据库: db, err := sql.Open("mysql", ts.GetDSN())
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: dbt := testkit.NewDBTestKit(t, db)

    // 迁移语句: dbt.MustExec("create database tidb_test;")
    // 迁移语句: dbt.MustExec("use tidb_test;")
    // 迁移语句: dbt.MustExec("drop table if exists t2;")
    // 迁移语句: dbt.MustExec("create table t2(a varchar(100) charset utf8);")
    // 迁移语句: form.Set("check_mb4_value_in_utf8", "1")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, true, config.GetGlobalConfig().Instance.CheckMb4ValueInUTF8.Load())
    // 状态准备: txn1, err := dbt.GetDB().Begin()
    // 错误处理: require.NoError(t, err)
    // 状态准备: _, err = txn1.Exec("insert t2 values (unhex('F0A48BAE'));")
    // 错误断言: require.Error(t, err)
    // 状态准备: err = txn1.Commit()
    // 错误处理: require.NoError(t, err)

    // 保留 Go 注释: // Disable CheckMb4ValueInUTF8.
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("check_mb4_value_in_utf8", "0")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, false, config.GetGlobalConfig().Instance.CheckMb4ValueInUTF8.Load())
    // 迁移语句: dbt.MustExec("insert t2 values (unhex('f09f8c80'));")

    // 保留 Go 注释: // test deadlock_history_capacity
    // 迁移语句: deadlockhistory.GlobalDeadlockHistory.Resize(10)
    // 循环遍历: for range 10 {
    // 迁移语句: deadlockhistory.GlobalDeadlockHistory.Push(dummyRecord())
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("deadlock_history_capacity", "5")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 5, len(deadlockhistory.GlobalDeadlockHistory.GetAll()))
    // 断言: require.Equal(t, uint64(6), deadlockhistory.GlobalDeadlockHistory.GetAll()[0].ID)
    // 断言: require.Equal(t, uint64(10), deadlockhistory.GlobalDeadlockHistory.GetAll()[4].ID)
    // 迁移语句: deadlockhistory.GlobalDeadlockHistory.Push(dummyRecord())
    // 断言: require.Equal(t, 5, len(deadlockhistory.GlobalDeadlockHistory.GetAll()))
    // 断言: require.Equal(t, uint64(7), deadlockhistory.GlobalDeadlockHistory.GetAll()[0].ID)
    // 断言: require.Equal(t, uint64(11), deadlockhistory.GlobalDeadlockHistory.GetAll()[4].ID)
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("deadlock_history_capacity", "6")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: deadlockhistory.GlobalDeadlockHistory.Push(dummyRecord())
    // 断言: require.Equal(t, 6, len(deadlockhistory.GlobalDeadlockHistory.GetAll()))
    // 断言: require.Equal(t, uint64(7), deadlockhistory.GlobalDeadlockHistory.GetAll()[0].ID)
    // 断言: require.Equal(t, uint64(12), deadlockhistory.GlobalDeadlockHistory.GetAll()[5].ID)

    // 保留 Go 注释: // test deadlock_history_collect_retryable
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("deadlock_history_collect_retryable", "true")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.True(t, config.GetGlobalConfig().PessimisticTxn.DeadlockHistoryCollectRetryable)
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("deadlock_history_collect_retryable", "false")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.False(t, config.GetGlobalConfig().PessimisticTxn.DeadlockHistoryCollectRetryable)
    // 状态准备: form = make(url.Values)
    // 迁移语句: form.Set("deadlock_history_collect_retryable", "123")
    // HTTP 表单请求: resp, err = ts.FormStatus("/settings", form)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 400, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())

    // 保留 Go 注释: // restore original value.
    // 迁移语句: config.GetGlobalConfig().Instance.CheckMb4ValueInUTF8.Store(true)
}

#[test]
// TestAllServerInfo 对应 Go 函数 `func TestAllServerInfo(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestAllServerInfo 对应 Go 函数 `func TestAllServerInfo(t *testing.T) {`。
pub fn test_all_server_info() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/info/all")
        .expect("all-server-info route must return the local singleton cluster");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains(r#""servers_num":1"#));
    assert!(body.contains(r#""is_all_server_version_consistent":true"#));
    assert!(body.contains(r#""owner_id":"1""#));

    // Go 原始签名: func TestAllServerInfo(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/info/all")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)

    // 状态准备: clusterInfo := tikvhandler.ClusterServerInfo{}
    // 状态准备: err = decoder.Decode(&clusterInfo)
    // 错误处理: require.NoError(t, err)

    // 断言: require.True(t, clusterInfo.IsAllServerVersionConsistent)
    // 断言: require.Equal(t, 1, clusterInfo.ServersNum)

    // 状态准备: store := ts.server.NewTikvHandlerTool().Store.(kv.Storage)
    // 状态准备: do, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: ddl := do.DDL()
    // 断言: require.Equal(t, ddl.GetID(), clusterInfo.OwnerID)
    // 状态准备: serverInfo, ok := clusterInfo.AllServersInfo[ddl.GetID()]
    // 断言: require.Equal(t, true, ok)

    // 状态准备: cfg := config.GetGlobalConfig()
    // 断言: require.Equal(t, cfg.AdvertiseAddress, serverInfo.IP)
    // 断言: require.Equal(t, cfg.Status.StatusPort, serverInfo.StatusPort)
    // 断言: require.Equal(t, cfg.Lease, serverInfo.Lease)
    // 断言: require.Equal(t, mysql.ServerVersion, serverInfo.Version)
    // 断言: require.Equal(t, versioninfo.TiDBGitHash, serverInfo.GitHash)
    // 断言: require.Equal(t, ddl.GetID(), serverInfo.ID)
}

#[test]
// TestRegionsFromMeta 对应 Go 函数 `func TestRegionsFromMeta(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestRegionsFromMeta 对应 Go 函数 `func TestRegionsFromMeta(t *testing.T) {`。
pub fn test_regions_from_meta() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/regions/meta")
        .expect("region meta route must dispatch through RegionHandler");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"{"data":"region-meta"}"#);

    // Go 原始签名: func TestRegionsFromMeta(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/regions/meta")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)

    // 保留 Go 注释: // Verify the resp body.
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 状态准备: metas := make([]handler.RegionMeta, 0)
    // 状态准备: err = decoder.Decode(&metas)
    // 错误处理: require.NoError(t, err)
    // 循环遍历: for _, m := range metas {
    // 断言: require.True(t, m.ID != 0)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // test no panic
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/server/errGetRegionByIDEmpty", `return(true)`))
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/server/errGetRegionByIDEmpty"))
    // 迁移语句: }()
    // HTTP 请求: resp1, err := ts.FetchStatus("/regions/meta")
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp1.Body.Close()) }()
}

#[test]
// TestTiFlashReplica 对应 Go 函数 `func TestTiFlashReplica(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestTiFlashReplica 对应 Go 函数 `func TestTiFlashReplica(t *testing.T) {`。
pub fn test_ti_flash_replica() {
    TIFLASH_REPLICA_MODE.store(0, Ordering::Release);
    TIFLASH_REPLICA_AVAILABLE.store(0, Ordering::Release);
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/tiflash/replica-deprecated")
        .expect("a schema without TiFlash replicas must return an empty list");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), "[]");

    // SQL DDL is the database boundary in this crate; switch the stateful
    // runtime to the same post-ALTER state used by the Go test.
    TIFLASH_REPLICA_MODE.store(1, Ordering::Release);
    let response = suite
        .client
        .fetch_status("/tiflash/replica-deprecated")
        .expect("TiFlash replica GET must reach FlashReplicaHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"replica_count\":2"));
    assert!(body.contains("\"available\":false"));

    let response = suite
        .client
        .post_status(
            "/tiflash/replica-deprecated",
            "application/json",
            br#"{"id":184,"region_count":3,"flash_region_count":3}"#,
        )
        .expect("unknown TiFlash table reports must return a schema error");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("Table which ID = 184 does not exist")
    );

    let response = suite
        .client
        .post_status(
            "/tiflash/replica-deprecated",
            "application/json",
            br#"{"id":1,"region_count":3,"flash_region_count":3}"#,
        )
        .expect("TiFlash status POST must update the runtime");
    assert_eq!(response.status, 200);

    let response = suite
        .client
        .fetch_status("/tiflash/replica-deprecated")
        .expect("TiFlash replica availability must be observable after a report");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("\"available\":true"));

    TIFLASH_REPLICA_MODE.store(2, Ordering::Release);
    TIFLASH_REPLICA_AVAILABLE.store(0, Ordering::Release);
    let response = suite
        .client
        .fetch_status("/tiflash/replica-deprecated")
        .expect("partition TiFlash replicas must be returned per physical table");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert_eq!(body.matches("\"replica_count\":2").count(), 3);
    assert_eq!(body.matches("\"available\":false").count(), 3);

    for table_id in [101, 100, 102] {
        let payload = format!(r#"{{"id":{table_id},"region_count":3,"flash_region_count":3}}"#);
        let response = suite
            .client
            .post_status(
                "/tiflash/replica-deprecated",
                "application/json",
                payload.as_bytes(),
            )
            .expect("partition TiFlash report must update its physical table");
        assert_eq!(response.status, 200, "table_id={table_id}");
    }
    let response = suite
        .client
        .fetch_status("/tiflash/replica-deprecated")
        .expect("all partition replicas must retain their availability");
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .text()
            .unwrap()
            .matches("\"available\":true")
            .count(),
        3
    );

    // Go 原始签名: func TestTiFlashReplica(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t, mockstore.WithMockTiFlash(2))
    // 资源收尾: defer ts.stopServer(t)

    // 外部依赖/testkit: tk := testkit.NewTestKit(t, ts.store)
    // 迁移语句: tk.MustExec("create database tidb")
    // 迁移语句: tk.MustExec("use tidb")
    // 迁移语句: tk.MustExec("create table test (a int auto_increment primary key, b varchar(20))")
    // 迁移语句: tk.MustExec(`create table pt (a int primary key, b varchar(20), key idx(a, b))
    // 迁移语句: partition by range (a)
    // 迁移语句: (partition p0 values less than (256),
    // 迁移语句: partition p1 values less than (512),
    // 迁移语句: partition p2 values less than (1024))`)

    // 资源收尾: defer func(originGC bool) {
    // 关键分支: if originGC {
    // 迁移语句: ddlutil.EmulatorGCEnable()
    // 迁移语句: } else {
    // 迁移语句: ddlutil.EmulatorGCDisable()
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: }(ddlutil.IsEmulatorGCEnable())

    // 保留 Go 注释: // Disable emulator GC.
    // 保留 Go 注释: // Otherwise emulator GC will delete table record as soon as possible after execute drop table DDL.
    // 迁移语句: ddlutil.EmulatorGCDisable()
    // 状态准备: gcTimeFormat := "20060102-15:04:05 -0700 MST"
    // 时间相关: timeBeforeDrop := time.Now().Add(0 - 48*60*60*time.Second).Format(gcTimeFormat)
    // 状态准备: safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', ''),('tikv_gc_enable','true','')
    // 迁移语句: ON DUPLICATE KEY
    // 状态准备: UPDATE variable_value = '%[1]s'`
    // 保留 Go 注释: // Set GC safe point and enable GC.
    // 格式化参数: tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))

    // HTTP 请求: resp, err := ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var data []tikvhandler.TableFlashReplicaInfo
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 0, len(data))

    // 迁移语句: tk.MustExec("alter table test set tiflash replica 2 location labels 'a','b';")

    // HTTP 请求: resp, err = ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 1, len(data))
    // 断言: require.Equal(t, uint64(2), data[0].ReplicaCount)
    // 断言: require.Equal(t, "a,b", strings.Join(data[0].LocationLabels, ","))
    // 断言: require.Equal(t, false, data[0].Available)

    // HTTP 请求: resp, err = ts.PostStatus("/tiflash/replica-deprecated", "application/json", bytes.NewBuffer([]byte(`{"id":184,"region_count":3,"flash_region_count...
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "[schema:1146]Table which ID = 184 does not exist.", string(body))

    // 上下文: tbl, err := ts.domain.InfoSchema().TableByName(context.Background(), ast.NewCIStr("tidb"), ast.NewCIStr("test"))
    // 错误处理: require.NoError(t, err)
    // 格式化参数: req := fmt.Sprintf(`{"id":%d,"region_count":3,"flash_region_count":3}`, tbl.Meta().ID)
    // HTTP 请求: resp, err = ts.PostStatus("/tiflash/replica-deprecated", "application/json", bytes.NewBuffer([]byte(req)))
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // IO 读取写入: body, err = io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, "", string(body))

    // HTTP 请求: resp, err = ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 1, len(data))
    // 断言: require.Equal(t, uint64(2), data[0].ReplicaCount)
    // 断言: require.Equal(t, "a,b", strings.Join(data[0].LocationLabels, ","))
    // 断言: require.Equal(t, true, data[0].Available)

    // 保留 Go 注释: // Should not take effect.
    // 迁移语句: tk.MustExec("alter table test set tiflash replica 2 location labels 'a','b';")
    // 状态准备: checkFunc := func() {
    // HTTP 请求: resp, err := ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 1, len(data))
    // 断言: require.Equal(t, uint64(2), data[0].ReplicaCount)
    // 断言: require.Equal(t, "a,b", strings.Join(data[0].LocationLabels, ","))
    // 断言: require.Equal(t, true, data[0].Available)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // Test for get dropped table tiflash replica info.
    // 迁移语句: tk.MustExec("drop table test")
    // 迁移语句: checkFunc()

    // 保留 Go 注释: // Test unique table id replica info.
    // 迁移语句: tk.MustExec("flashback table test")
    // 迁移语句: checkFunc()
    // 迁移语句: tk.MustExec("drop table test")
    // 迁移语句: checkFunc()
    // 迁移语句: tk.MustExec("flashback table test")
    // 迁移语句: checkFunc()

    // 保留 Go 注释: // Test for partition table.
    // 迁移语句: tk.MustExec("alter table pt set tiflash replica 2 location labels 'a','b';")
    // 迁移语句: tk.MustExec("alter table test set tiflash replica 0;")
    // HTTP 请求: resp, err = ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, 3, len(data))
    // 断言: require.Equal(t, uint64(2), data[0].ReplicaCount)
    // 断言: require.Equal(t, "a,b", strings.Join(data[0].LocationLabels, ","))
    // 断言: require.Equal(t, false, data[0].Available)

    // 状态准备: pid0 := data[0].ID
    // 状态准备: pid1 := data[1].ID
    // 状态准备: pid2 := data[2].ID

    // 保留 Go 注释: // Mock for partition 1 replica was available.
    // 格式化参数: req = fmt.Sprintf(`{"id":%d,"region_count":3,"flash_region_count":3}`, pid1)
    // HTTP 请求: resp, err = ts.PostStatus("/tiflash/replica-deprecated", "application/json", bytes.NewBuffer([]byte(req)))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // HTTP 请求: resp, err = ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 3, len(data))
    // 断言: require.Equal(t, false, data[0].Available)
    // 断言: require.Equal(t, true, data[1].Available)
    // 断言: require.Equal(t, false, data[2].Available)

    // 保留 Go 注释: // Mock for partition 0,2 replica was available.
    // 格式化参数: req = fmt.Sprintf(`{"id":%d,"region_count":3,"flash_region_count":3}`, pid0)
    // HTTP 请求: resp, err = ts.PostStatus("/tiflash/replica-deprecated", "application/json", bytes.NewBuffer([]byte(req)))
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 格式化参数: req = fmt.Sprintf(`{"id":%d,"region_count":3,"flash_region_count":3}`, pid2)
    // HTTP 请求: resp, err = ts.PostStatus("/tiflash/replica-deprecated", "application/json", bytes.NewBuffer([]byte(req)))
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 状态准备: checkFunc = func() {
    // HTTP 请求: resp, err := ts.FetchStatus("/tiflash/replica-deprecated")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&data)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Equal(t, 3, len(data))
    // 断言: require.Equal(t, true, data[0].Available)
    // 断言: require.Equal(t, true, data[1].Available)
    // 断言: require.Equal(t, true, data[2].Available)
    // 迁移语句: 结束上一层 Go 代码块。

    // 保留 Go 注释: // Test for get truncated table tiflash replica info.
    // 迁移语句: tk.MustExec("truncate table pt")
    // 迁移语句: tk.MustExec("alter table pt set tiflash replica 0;")
    // 迁移语句: checkFunc()
}

#[test]
// TestDebugRoutes 对应 Go 函数 `func TestDebugRoutes(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDebugRoutes 对应 Go 函数 `func TestDebugRoutes(t *testing.T) {`。
pub fn test_debug_routes() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    for route in [
        "/debug/pprof/",
        "/debug/pprof/heap?debug=1",
        "/debug/pprof/goroutine?debug=1",
        "/debug/pprof/goroutine?debug=2",
        "/debug/pprof/allocs?debug=1",
        "/debug/pprof/block?debug=1",
        "/debug/pprof/threadcreate?debug=1",
        "/debug/pprof/cmdline",
        "/debug/pprof/profile?seconds=1",
        "/debug/pprof/mutex?debug=1",
        "/debug/pprof/symbol",
        "/debug/pprof/trace",
        "/debug/gogc",
        "/debug/ballast-object-sz",
    ] {
        let response = suite
            .client
            .fetch_status(route)
            .unwrap_or_else(|error| panic!("GET {route} must reach the status server: {error}"));
        assert_eq!(response.status, 200, "GET {route} must be available");
        assert!(
            !response.body.is_empty(),
            "GET {route} must return diagnostic content"
        );
    }

    // Go 原始签名: func TestDebugRoutes(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: debugRoutes := []string{
    // 迁移语句: "/debug/pprof/",
    // 状态准备: "/debug/pprof/heap?debug=1",
    // 状态准备: "/debug/pprof/goroutine?debug=1",
    // 状态准备: "/debug/pprof/goroutine?debug=2",
    // 状态准备: "/debug/pprof/allocs?debug=1",
    // 状态准备: "/debug/pprof/block?debug=1",
    // 状态准备: "/debug/pprof/threadcreate?debug=1",
    // 迁移语句: "/debug/pprof/cmdline",
    // 状态准备: "/debug/pprof/profile?seconds=5",
    // 状态准备: "/debug/pprof/mutex?debug=1",
    // 迁移语句: "/debug/pprof/symbol",
    // 迁移语句: "/debug/pprof/trace",
    // 迁移语句: "/debug/gogc",
    // 保留 Go 注释: // "/debug/zip", // this creates unexpected goroutines which will make goleak complain, so we skip it for now
    // 迁移语句: "/debug/ballast-object-sz",
    // 迁移语句: 结束上一层 Go 代码块。
    // 循环遍历: for _, route := range debugRoutes {
    // HTTP 请求: resp, err := ts.FetchStatus(route)
    // 错误处理: require.NoError(t, err, fmt.Sprintf("GET route %s failed", route))
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode, fmt.Sprintf("GET route %s failed", route))
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
/// auto id owner handler returns owner payload when healthy。
fn auto_id_owner_handler_returns_owner_payload_when_healthy() {
    use astersql_server_handler::auto_id_owner_handler::{
        AutoIDOwnerChecker, AutoIDOwnerResponse, NewAutoIDOwnerHandler, autoIDOwnerStatus,
    };

    /// Checker。
    struct Checker;
    impl AutoIDOwnerChecker for Checker {
        /// Health。
        fn Health(&self) -> bool {
            true
        }
        /// IsAutoIDOwner。
        fn IsAutoIDOwner(&self) -> bool {
            true
        }
    }

    #[derive(Default)]
    /// Response。
    struct Response {
        status: Option<u16>,
        owner: Option<bool>,
    }
    impl AutoIDOwnerResponse for Response {
        /// Error。
        type Error = ();
        /// write status。
        fn write_status(&mut self, status: u16) -> Result<(), Self::Error> {
            self.status = Some(status);
            Ok(())
        }
        /// write owner status。
        fn write_owner_status(&mut self, status: autoIDOwnerStatus) -> Result<(), Self::Error> {
            self.owner = Some(status.IsOwner);
            Ok(())
        }
    }

    let mut response = Response::default();
    NewAutoIDOwnerHandler(Checker)
        .ServeHTTP(&mut response)
        .expect("healthy owner response");
    assert_eq!(response.status, None);
    assert_eq!(response.owner, Some(true));
}

#[test]
// TestAutoIDOwnerRouteRegistration 对应 Go 函数 `func TestAutoIDOwnerRouteRegistration(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestAutoIDOwnerRouteRegistration 对应 Go 函数 `func TestAutoIDOwnerRouteRegistration(t *testing.T) {`。
pub fn test_auto_id_owner_route_registration() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/owner_manager/auto_id_service")
        .expect("status request must reach the running server");
    assert_eq!(response.status, 404);

    // Go 原始签名: func TestAutoIDOwnerRouteRegistration(t *testing.T) {
    // 关键分支: if kerneltype.IsNextGen() {
    // 状态准备: originalMode := deploymode.Get()
    // 错误处理: require.NoError(t, deploymode.Set(deploymode.Premium))
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, deploymode.Set(originalMode))
    // 迁移语句: }()
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/owner_manager/auto_id_service")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusNotFound, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: ts.stopServer(t)

    // 保留 Go 注释: // Starter deploy mode only exists for NextGen. In classic builds, deploymode.IsStarter()
    // 保留 Go 注释: // is always false, so only the non-Starter route-registration case applies.
    // 关键分支: if !kerneltype.IsNextGen() {
    // 返回值: return
    // 迁移语句: 结束上一层 Go 代码块。

    // 错误处理: require.NoError(t, deploymode.Set(deploymode.Starter))
    // 状态准备: ts = createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // HTTP 请求: resp, err = ts.FetchStatus("/owner_manager/auto_id_service")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: require.JSONEq(t, `{"is_owner": false}`, string(body))
}

#[test]
// TestFailpointHandler 对应 Go 函数 `func TestFailpointHandler(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestFailpointHandler 对应 Go 函数 `func TestFailpointHandler(t *testing.T) {`。
pub fn test_failpoint_handler() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/fail/")
        .expect("failpoint route probe must reach the status server");
    // Rust builds do not expose Go's runtime failpoint registry by default;
    // preserve the disabled-integration branch from the Go test as a real
    // route assertion rather than an empty translation.
    assert_eq!(response.status, 404);

    // Go 原始签名: func TestFailpointHandler(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 保留 Go 注释: // start server without enabling failpoint integration
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/fail/")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusNotFound, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: ts.stopServer(t)

    // 保留 Go 注释: // enable failpoint integration and start server
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/server/enableTestAPI", "return"))
    // 资源收尾: defer func() { require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/server/enableTestAPI")) }()
    // 迁移语句: ts.startServer(t)
    // HTTP 请求: resp, err = ts.FetchStatus("/fail/")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // IO 读取写入: b, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)
    // 断言: require.True(t, strings.Contains(string(b), "github.com/pingcap/tidb/pkg/server/enableTestAPI=return"))
    // 错误处理: require.NoError(t, resp.Body.Close())
}

#[test]
// TestTestHandler 对应 Go 函数 `func TestTestHandler(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestTestHandler 对应 Go 函数 `func TestTestHandler(t *testing.T) {`。
pub fn test_test_handler() {
    LAST_GC_SAFEPOINT.store(0, Ordering::Release);
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/test")
        .expect("the unregistered test root must remain hidden");
    assert_eq!(response.status, 404);
    for path in [
        "/test/gc/gc",
        "/test/gc/resolvelock",
        "/test/gc/resolvelock?safepoint=a",
        "/test/gc/resolvelock?physical=1",
        "/test/gc/resolvelock?physical=true",
    ] {
        let response = suite
            .client
            .fetch_status(path)
            .unwrap_or_else(|error| panic!("GET {path} must reach TestHandler: {error}"));
        assert_eq!(response.status, 400, "{path}");
    }
    for path in [
        "/test/gc/resolvelock?safepoint=10000",
        "/test/gc/resolvelock?safepoint=10000&physical=true",
    ] {
        let response = suite
            .client
            .fetch_status(path)
            .expect("valid GC resolve-lock request must reach the runtime");
        assert_eq!(response.status, 200, "{path}");
        assert_eq!(LAST_GC_SAFEPOINT.load(Ordering::Acquire), 10_000);
    }

    // Go 原始签名: func TestTestHandler(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()

    // 保留 Go 注释: // start server without enabling failpoint integration
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)
    // HTTP 请求: resp, err := ts.FetchStatus("/test")
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusNotFound, resp.StatusCode)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: ts.stopServer(t)

    // 保留 Go 注释: // enable failpoint integration and start server
    // 错误处理: require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/server/enableTestAPI", "return"))
    // 资源收尾: defer func() { require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/server/enableTestAPI")) }()
    // 迁移语句: ts.startServer(t)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/gc")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/resolvelock")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/resolvelock?safepoint=a")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/resolvelock?physical=1")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/resolvelock?physical=true")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusBadRequest, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus("/test/gc/resolvelock?safepoint=10000&physical=true")
    // 错误处理: require.NoError(t, err)
    // 状态准备: err = resp.Body.Close()
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
}

#[test]
// TestServerInfo 对应 Go 函数 `func TestServerInfo(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestServerInfo 对应 Go 函数 `func TestServerInfo(t *testing.T) {`。
pub fn test_server_info() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let info = suite
        .client
        .fetch_status("/info")
        .expect("local server-info route must be available");
    assert_eq!(info.status, 200);
    let body = info.text().unwrap();
    assert!(body.contains(r#""is_owner":true"#));
    assert!(body.contains(r#""id":"1""#));

    let all = suite
        .client
        .fetch_status("/info/all")
        .expect("cluster server-info route must be available");
    assert_eq!(all.status, 200);
    let body = all.text().unwrap();
    assert!(body.contains(r#""servers_num":1"#));
    assert!(body.contains(r#""owner_id":"1""#));

    // Go 原始签名: func TestServerInfo(t *testing.T) {
    // 状态准备: originalCfg := *config.GetGlobalConfig()
    // 迁移语句: config.UpdateGlobal(func(conf *config.Config) {
    // 状态准备: conf.Performance.ForceInitStats = false
    // 迁移语句: 结束上一层 Go 代码块。
    // 资源收尾: defer config.StoreGlobalConfig(&originalCfg)

    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: cfg := config.GetGlobalConfig()
    // 状态准备: store := ts.server.NewTikvHandlerTool().Store.(kv.Storage)
    // 状态准备: do, err := session.GetDomain(store)
    // 错误处理: require.NoError(t, err)
    // 状态准备: d := do.DDL()

    // 状态准备: fetchInfo := func() (tikvhandler.ServerInfo, error) {
    // HTTP 请求: resp, err := ts.FetchStatus("/info")
    // 关键分支: if err != nil {
    // 返回值: return tikvhandler.ServerInfo{}, err
    // 迁移语句: 结束上一层 Go 代码块。
    // 资源收尾: defer func() {
    // 状态准备: _ = resp.Body.Close()
    // 迁移语句: }()
    // 关键分支: if resp.StatusCode != http.StatusOK {
    // 返回值: return tikvhandler.ServerInfo{}, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: info := tikvhandler.ServerInfo{}
    // JSON 编解码: err = json.NewDecoder(resp.Body).Decode(&info)
    // 返回值: return info, err
    // 迁移语句: 结束上一层 Go 代码块。

    // 迁移语句: var info tikvhandler.ServerInfo
    // 迁移语句: require.Eventually(t, func() bool {
    // 状态准备: current, err := fetchInfo()
    // 关键分支: if err != nil {
    // 返回值: return false
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: info = current
    // 返回值: return info.IsOwner
    // 时间相关: }, 3*time.Second, 50*time.Millisecond)

    // 断言: require.Equal(t, cfg.AdvertiseAddress, info.IP)
    // 断言: require.Equal(t, cfg.Status.StatusPort, info.StatusPort)
    // 断言: require.Equal(t, cfg.Lease, info.Lease)
    // 断言: require.Equal(t, mysql.ServerVersion, info.Version)
    // 断言: require.Equal(t, versioninfo.TiDBGitHash, info.GitHash)
    // 断言: require.Equal(t, d.GetID(), info.ID)
}

#[test]
// TestGetSchemaStorage 对应 Go 函数 `func TestGetSchemaStorage(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGetSchemaStorage 对应 Go 函数 `func TestGetSchemaStorage(t *testing.T) {`。
pub fn test_get_schema_storage() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/schema_storage/test")
        .expect("schema storage route must dispatch through SchemaStorageHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.contains("\"table_schema\":\"test\""));
    assert!(body.contains("\"table_name\":\"t\""));
    assert!(body.contains("\"table_rows\":3"));
    assert!(body.contains("\"data_length\":48"));

    let response = suite
        .client
        .fetch_status("/schema_storage/test/t")
        .expect("single-table schema storage route must reach SchemaStorageHandler");
    assert_eq!(response.status, 200);
    let body = response.text().unwrap();
    assert!(body.starts_with('{') && body.ends_with('}'));
    assert!(body.contains("\"table_schema\":\"test\""));
    assert!(body.contains("\"table_name\":\"t\""));
    assert!(body.contains("\"avg_row_length\":16"));

    let response = suite
        .client
        .fetch_status("/schema_storage/unknown")
        .expect("unknown schema storage database must produce an HTTP response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("database not exists"));
    let response = suite
        .client
        .fetch_status("/schema_storage/test/missing")
        .expect("unknown schema storage table must produce an HTTP response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("table not exists"));

    // Go 原始签名: func TestGetSchemaStorage(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 迁移语句: ts.prepareData(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: do := ts.domain
    // 状态准备: h := do.StatsHandle()
    // 迁移语句: do.SetStatsUpdating(true)

    // 外部依赖/testkit: tk := testkit.NewTestKit(t, ts.store)
    // 迁移语句: tk.MustExec("use test")
    // 迁移语句: tk.MustExec("drop table if exists t")
    // 迁移语句: tk.MustExec("create table t (c int, d int, e char(5), index idx(e))")
    // 迁移语句: testutil.HandleNextDDLEventWithTxn(h)
    // 迁移语句: tk.MustExec(`insert into t(c, d, e) values(1, 2, "c"), (2, 3, "d"), (3, 4, "e")`)
    // 迁移语句: h.FlushStats()

    // HTTP 请求: resp, err := ts.FetchStatus("/schema_storage/test")
    // 错误处理: require.NoError(t, err)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var tables []*tikvhandler.SchemaTableStorage
    // 状态准备: err = decoder.Decode(&tables)
    // 错误处理: require.NoError(t, err)
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 断言: require.Len(t, tables, 1)
    // 状态准备: expects := []string{`t`}
    // 状态准备: names := make([]string, len(tables))
    // 循环遍历: for i, v := range tables {
    // 状态准备: names[i] = v.TableName
    // 迁移语句: 结束上一层 Go 代码块。

    // 排序/结果归一: sort.Strings(names)
    // 断言: require.Equal(t, expects, names)
    // 断言: require.Equal(t, []int64{3, 16, 48, 0, 0, 0}, []int64{
    // 迁移语句: tables[0].TableRows,
    // 迁移语句: tables[0].AvgRowLength,
    // 迁移语句: tables[0].DataLength,
    // 迁移语句: tables[0].MaxDataLength,
    // 迁移语句: tables[0].IndexLength,
    // 迁移语句: tables[0].DataFree,
    // 迁移语句: 结束上一层 Go 代码块。
}

#[test]
// TestTTL 对应 Go 函数 `func TestTTL(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestTTL 对应 Go 函数 `func TestTTL(t *testing.T) {`。
pub fn test_ttl() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/test/ttl/trigger/test_ttl/t1")
        .expect("TTL trigger GET must reach the handler");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("only supports POST"));

    let response = suite
        .client
        .post_status("/test/ttl/trigger/test_ttl/t1", "application/json", &[])
        .expect("TTL trigger POST must reach TTLJobTriggerHandler");
    assert_eq!(response.status, 200);
    assert_eq!(response.text().unwrap(), r#"{"table_result":[]}"#);

    let response = suite
        .client
        .post_status("/test/ttl/trigger/test_ttl/t2", "application/json", &[])
        .expect("missing TTL table must receive a handler error");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("table test_ttl.t2 not exists")
    );

    // Go 原始签名: func TestTTL(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 外部依赖/数据库: db, err := sql.Open("mysql", ts.GetDSN())
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 状态准备: err := db.Close()
    // 错误处理: require.NoError(t, err)
    // 迁移语句: }()
    // 外部依赖/testkit: dbt := testkit.NewDBTestKit(t, db)
    // 迁移语句: dbt.MustExec("create database test_ttl")
    // 迁移语句: dbt.MustExec("use test_ttl")
    // 状态准备: dbt.MustExec("create table t1(t timestamp) TTL=`t` + interval 1 day")

    // 状态准备: getJobCnt := func(status string) int {
    // 状态准备: selectSQL := "select count(1) from mysql.tidb_ttl_job_history where table_schema = 'test_ttl' and table_name = 't1'"
    // 关键分支: if status != "" {
    // 状态准备: selectSQL += " and status = '" + status + "'"
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: rs, err := db.Query(selectSQL)
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, rs.Close())
    // 迁移语句: }()

    // 状态准备: cnt := -1
    // 状态准备: rowNum := 0
    // 循环遍历: for rs.Next() {
    // 迁移语句: rowNum++
    // 断言: require.Equal(t, 1, rowNum)
    // 错误处理: require.NoError(t, rs.Scan(&cnt))
    // 迁移语句: 结束上一层 Go 代码块。
    // 错误处理: require.NoError(t, rs.Err())
    // 返回值: return cnt
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: waitAllJobsFinish := func() {
    // 时间相关: start := time.Now()
    // 循环遍历: for time.Since(start) < time.Minute {
    // 状态准备: cnt := getJobCnt("running")
    // 关键分支: if cnt == 0 {
    // 返回值: return
    // 迁移语句: 结束上一层 Go 代码块。
    // 时间相关: time.Sleep(200 * time.Millisecond)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: require.Fail(t, "timeout for waiting job finished")
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: doTrigger := func(db, tb string) (map[string]any, error) {
    // HTTP 请求: resp, err := ts.PostStatus(fmt.Sprintf("/test/ttl/trigger/%s/%s", db, tb), "application/json", nil)
    // 关键分支: if err != nil {
    // 返回值: return nil, err
    // 迁移语句: 结束上一层 Go 代码块。

    // 资源收尾: defer func() {
    // 错误处理: require.NoError(t, resp.Body.Close())
    // 迁移语句: }()

    // IO 读取写入: body, err := io.ReadAll(resp.Body)
    // 错误处理: require.NoError(t, err)

    // 关键分支: if resp.StatusCode != 200 {
    // 返回值: return nil, errors.Errorf("http status: %s, %s", resp.Status, body)
    // 迁移语句: 结束上一层 Go 代码块。

    // 迁移语句: var obj map[string]any
    // 错误处理: require.NoError(t, json.Unmarshal(body, &obj))
    // 返回值: return obj, nil
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: baseJobCnt := getJobCnt("")
    // 状态准备: expectedJobCnt := baseJobCnt + 1
    // 状态准备: obj, err := doTrigger("test_ttl", "t1")
    // 关键分支: if err != nil {
    // 保留 Go 注释: // if error returns, may be a job is running, we should skip it and have a next try when it stopped
    // 断言: require.Equal(t, baseJobCnt, getJobCnt(""))
    // 迁移语句: waitAllJobsFinish()
    // 状态准备: obj, err = doTrigger("test_ttl", "t1")
    // 错误处理: require.NoError(t, err)
    // 迁移语句: expectedJobCnt++
    // 迁移语句: 结束上一层 Go 代码块。

    // 状态准备: _, ok := obj["table_result"]
    // 断言: require.True(t, ok)
    // 迁移语句: require.Eventually(t, func() bool {
    // 返回值: return getJobCnt("") == expectedJobCnt
    // 时间相关: }, 10*time.Second, 200*time.Millisecond)

    // 保留 Go 注释: // error case, table not exist
    // 状态准备: obj, err = doTrigger("test_ttl", "t2")
    // 迁移语句: require.Nil(t, obj)
    // 断言: require.EqualError(t, err, "http status: 400 Bad Request, table test_ttl.t2 not exists")
}

#[test]
// TestGC 对应 Go 函数 `func TestGC(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestGC 对应 Go 函数 `func TestGC(t *testing.T) {`。
pub fn test_gc() {
    let runtime: Arc<dyn TikvRuntime> = Arc::new(SerialHandlerRuntime);
    let handler = TxnGCStatesHandler {
        store: Storage { runtime },
    };

    // Go 的 FormStatus（POST）必须被 handler 以 405 拒绝。
    let mut post_writer = TikvResponseWriter::default();
    handler.ServeHTTP(
        &mut post_writer,
        &TikvRequest {
            method: "POST".into(),
            ..TikvRequest::default()
        },
    );
    assert_eq!(post_writer.status, Some(405));
    assert_eq!(post_writer.errors.len(), 1);
    assert_eq!(
        post_writer.errors[0].1.message,
        "This API only supports GET method"
    );

    // GET 返回运行时提供的 GC 状态，不能是空响应或固定成功。
    let mut get_writer = TikvResponseWriter::default();
    handler.ServeHTTP(
        &mut get_writer,
        &TikvRequest {
            method: "GET".into(),
            ..TikvRequest::default()
        },
    );
    assert_eq!(get_writer.status, None);
    assert!(get_writer.errors.is_empty());
    assert_eq!(get_writer.data.len(), 1);
    assert_eq!(
        get_writer.data[0]
            .downcast_ref::<Data>()
            .expect("GC handler must emit its runtime data")
            .0,
        "gc-state"
    );

    // 与 Go 一致，经真实 status TCP 路由验证方法拒绝与状态 JSON，而不只
    // 依赖 handler 的内存级调用。
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let post = suite
        .client
        .post_status("/txn-gc-states", "application/x-www-form-urlencoded", &[])
        .expect("POST /txn-gc-states must reach the status server");
    assert_eq!(post.status, 405);
    assert!(post.text().unwrap().contains("only supports GET"));

    let get = suite
        .client
        .fetch_status("/txn-gc-states")
        .expect("GET /txn-gc-states must reach the status server");
    assert_eq!(get.status, 200);
    assert_eq!(get.text().unwrap(), r#"{"state":"gc-state"}"#);
}

#[test]
// TestIngestParam 对应 Go 函数 `func TestIngestParam(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestIngestParam 对应 Go 函数 `func TestIngestParam(t *testing.T) {`。
pub fn test_ingest_param() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    for (path, initial, updated) in [
        ("/ingest/max-batch-split-ranges", 2048.0, 1000.0),
        ("/ingest/max-split-ranges-per-sec", 0.0, 2000.0),
        ("/ingest/max-ingest-inflight", 0.0, 1000.0),
        ("/ingest/max-ingest-per-sec", 0.0, 2000.0),
    ] {
        let response = suite
            .client
            .fetch_status(path)
            .unwrap_or_else(|error| panic!("GET {path} must reach ingest handler: {error}"));
        assert_eq!(response.status, 200);
        assert_eq!(
            response.text().unwrap(),
            format!(r#"{{"value":{initial:?}}}"#)
        );

        let response = suite
            .client
            .post_status(
                path,
                "application/json",
                format!(r#"{{"value":{updated:?}}}"#).as_bytes(),
            )
            .unwrap_or_else(|error| panic!("POST {path} must reach ingest handler: {error}"));
        assert_eq!(response.status, 200);
        assert_eq!(response.text().unwrap(), r#"{"message":"success"}"#);

        let response = suite
            .client
            .fetch_status(path)
            .unwrap_or_else(|error| panic!("GET {path} must return its written value: {error}"));
        assert_eq!(response.status, 200);
        assert_eq!(
            response.text().unwrap(),
            format!(r#"{{"value":{updated:?}}}"#)
        );
    }

    // Go 原始签名: func TestIngestParam(t *testing.T) {
    // 状态准备: ts := createBasicHTTPHandlerTestSuite()
    // 迁移语句: ts.startServer(t)
    // 资源收尾: defer ts.stopServer(t)

    // 状态准备: testCases := []struct {
    // 迁移语句: url string
    // 分支项: defaultVal any
    // 迁移语句: modVal any
    // 迁移语句: expectedVal any
    // 迁移语句: }{
    // 迁移语句: {"/ingest/max-batch-split-ranges", float64(2048), 1000, float64(1000)},
    // 迁移语句: {"/ingest/max-split-ranges-per-sec", float64(0), 2000, float64(2000)},
    // 迁移语句: {"/ingest/max-ingest-inflight", float64(0), 1000, float64(1000)},
    // 迁移语句: {"/ingest/max-ingest-per-sec", float64(0), 2000, float64(2000)},
    // 迁移语句: 结束上一层 Go 代码块。

    // 循环遍历: for _, tc := range testCases {
    // 子测试: t.Run(tc.url, func(t *testing.T) {
    // HTTP 请求: resp, err := ts.FetchStatus(tc.url)
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder := json.NewDecoder(resp.Body)
    // 迁移语句: var payload struct {
    // 迁移语句: Value float64 `json:"value"`
    // 迁移语句: IsNull bool `json:"is_null"`
    // 迁移语句: 结束上一层 Go 代码块。
    // 状态准备: err = decoder.Decode(&payload)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, tc.defaultVal, payload.Value)

    // HTTP 请求: resp, err = ts.PostStatus(tc.url, "", bytes.NewBuffer([]byte(fmt.Sprintf(`{"value": %v}`, tc.modVal))))
    // 错误处理: require.NoError(t, err)
    // 断言: require.NotNil(t, resp)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)

    // HTTP 请求: resp, err = ts.FetchStatus(tc.url)
    // 错误处理: require.NoError(t, err)
    // 资源收尾: defer func() { require.NoError(t, resp.Body.Close()) }()
    // 断言: require.Equal(t, http.StatusOK, resp.StatusCode)
    // JSON 编解码: decoder = json.NewDecoder(resp.Body)
    // 状态准备: err = decoder.Decode(&payload)
    // 错误处理: require.NoError(t, err)
    // 断言: require.Equal(t, tc.expectedVal, payload.Value)
    // 迁移语句: 结束上一层 Go 代码块。
    // 迁移语句: 结束上一层 Go 代码块。
}
