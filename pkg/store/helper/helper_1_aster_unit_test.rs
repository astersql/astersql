// Copyright 2026 AsterSQL.
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

// Store Helper 单元测试（Aster 迁移补充集）。
//
// 覆盖 TiFlash 状态解析、表/索引键范围与 keyspace Codec、Region↔表半开区间
// 相交、Frame 边界、分区/全局索引收集，以及 MVCC Get 遇锁重试与 HTTP 状态采集路径。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{BufReader, Cursor, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use crate::*;

/// 构造大小写不敏感标识符（CIStr）。
fn ci(value: &str) -> CIStr {
    CIStr::new(value)
}

#[test]
/// 校验 TiFlash 副本状态文本累加与非法输入报错。
fn compute_tiflash_status_matches_go_accumulation_and_errors() {
    let mut replicas = HashMap::new();
    ComputeTiFlashStatus(&mut BufReader::new(Cursor::new("0\n\n")), &mut replicas).unwrap();
    ComputeTiFlashStatus(
        &mut BufReader::new(Cursor::new("2\n1009 1010 \n")),
        &mut replicas,
    )
    .unwrap();
    ComputeTiFlashStatus(&mut BufReader::new(Cursor::new("1\n1009\n")), &mut replicas).unwrap();

    // 同一 Region 多次出现时副本数应累加。
    assert_eq!(replicas, HashMap::from([(1009, 2), (1010, 1)]));
    assert!(
        ComputeTiFlashStatus(
            &mut BufReader::new(Cursor::new("not-a-count\n\n")),
            &mut HashMap::new(),
        )
        .is_err()
    );
    assert!(
        ComputeTiFlashStatus(
            &mut BufReader::new(Cursor::new("1\nnot-a-region\n")),
            &mut HashMap::new(),
        )
        .is_err()
    );
}

#[test]
/// 校验表/索引键范围在 v1/v2 Codec 下与 Go 契约一致。
fn table_ranges_and_keyspace_codec_match_go_contract() {
    let db = Arc::new(DatabaseInfo {
        Name: ci("test"),
        ..Default::default()
    });
    let table = Arc::new(TableMeta {
        ID: 41,
        Name: ci("t"),
        ..Default::default()
    });
    let index = Arc::new(IndexMeta {
        ID: 1,
        Name: ci("i"),
        ..Default::default()
    });

    let v1_table = NewTableWithKeyRange(db.clone(), table.clone(), Codec::v1());
    let v1_index = NewIndexWithKeyRange(db.clone(), table.clone(), index.clone(), Codec::v1());
    let v2_table = NewTableWithKeyRange(db.clone(), table.clone(), Codec::v2(1).unwrap());
    let v2_index = NewIndexWithKeyRange(db, table, index, Codec::v2(1).unwrap());

    assert!(v1_index.StartKey < v1_table.StartKey);
    assert_ne!(v1_table.StartKey, v2_table.StartKey);
    assert_ne!(v1_index.StartKey, v2_index.StartKey);
    assert!(v2_table.StartKey.starts_with("78000001"));
}

#[test]
/// 校验 Region↔表交集保持半开区间语义。
fn parse_regions_table_infos_preserves_half_open_intersections() {
    let db = Arc::new(DatabaseInfo {
        Name: ci("test"),
        ..Default::default()
    });
    let table = Arc::new(TableMeta {
        ID: 41,
        Name: ci("t"),
        ..Default::default()
    });
    let index = Arc::new(IndexMeta {
        ID: 1,
        Name: ci("i"),
        ..Default::default()
    });
    let mut tables = vec![
        NewTableWithKeyRange(db.clone(), table.clone(), Codec::v1()),
        NewIndexWithKeyRange(db, table, index, Codec::v1()),
    ];
    tables.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));

    let regions = vec![
        RegionInfo {
            ID: 1,
            StartKey: String::new(),
            EndKey: tables[0].StartKey.clone(),
        },
        RegionInfo {
            ID: 2,
            StartKey: tables[0].StartKey.clone(),
            EndKey: tables[1].EndKey.clone(),
        },
    ];
    let result = ParseRegionsTableInfos(regions, tables);

    assert!(result[&1].is_empty());
    assert_eq!(result[&2].len(), 2);
    assert!(result[&2][0].IsIndex);
    assert!(!result[&2][1].IsIndex);
}

#[test]
/// 校验 Region 帧在记录/索引边界上与 Go 一致。
fn region_frame_range_matches_go_record_and_index_boundaries() {
    let mut range = RegionFrameRange {
        First: FrameItem {
            TableID: 41,
            IndexID: 1,
            ..Default::default()
        },
        Last: FrameItem {
            TableID: 41,
            IsRecord: true,
            ..Default::default()
        },
        region: KeyLocation::default(),
    };

    let index = range.GetIndexFrame(41, 1, "test", "t", "i").unwrap();
    assert_eq!(index.IndexName, "i");
    let record = range.GetRecordFrame(41, "test", "t", true).unwrap();
    assert_eq!(record.IndexName, "PRIMARY");
    assert_eq!(
        range
            .GetIndexFrame(41, 2, "test", "t", "i2")
            .unwrap()
            .IndexID,
        2
    );
    assert!(range.GetIndexFrame(40, 2, "test", "t", "i2").is_none());
}

#[test]
/// 校验帧键解码与 TiFlash end key 使用真实 tablecodec。
fn frame_key_decoding_and_tiflash_end_key_use_real_tablecodec() {
    let record_key =
        tablecodec::EncodeRowKeyWithHandle(41, Box::new(tablecodec::kv::IntHandle(7))).0;
    let record = NewFrameItemFromRegionKey(record_key).unwrap();
    assert_eq!(record.TableID, 41);
    assert!(record.IsRecord);
    assert_eq!(record.RecordID, 7);

    let split_key = tablecodec::EncodeTablePrefix(63).0;
    let split = NewFrameItemFromRegionKey(split_key).unwrap();
    assert_eq!(split.TableID, 63);

    let before = NewFrameItemFromRegionKey(vec![b'a']).unwrap();
    let after = NewFrameItemFromRegionKey(vec![b'z']).unwrap();
    assert_eq!(before.TableID, i64::MIN);
    assert_eq!(after.TableID, i64::MAX);

    let end_key = tablecodec::codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(42).0);
    assert_eq!(GetTiFlashTableIDFromEndKey(&hex::encode(end_key)), 41);
}

#[derive(Clone)]
/// Schema 替身。
struct MockSchema {
    database: Arc<DatabaseInfo>,
}

impl SchemaAndTable for MockSchema {
    fn AllSchemas(&self) -> Vec<Arc<DatabaseInfo>> {
        vec![self.database.clone()]
    }

    fn SchemaTableInfos(&self, _: &CIStr) -> anyhow::Result<Vec<Arc<TableMeta>>> {
        Ok(self.database.Tables.clone())
    }
}

#[test]
/// 校验分区表与全局索引收集分支对齐 Go。
fn table_collection_keeps_partition_and_global_index_go_branches() {
    let local = Arc::new(IndexMeta {
        ID: 1,
        Name: ci("local"),
        Global: false,
    });
    let global = Arc::new(IndexMeta {
        ID: 2,
        Name: ci("global"),
        Global: true,
    });
    let table = Arc::new(TableMeta {
        ID: 41,
        Name: ci("pt"),
        Indices: vec![local, global],
        Partition: Some(PartitionInfo {
            Definitions: vec![
                PartitionDefinition {
                    ID: 101,
                    Name: ci("p0"),
                },
                PartitionDefinition {
                    ID: 102,
                    Name: ci("p1"),
                },
            ],
        }),
        IsCommonHandle: false,
    });
    let database = Arc::new(DatabaseInfo {
        Name: ci("test"),
        Tables: vec![table],
    });
    let schema = MockSchema { database };
    let ranges = Helper::default().GetTablesInfoWithKeyRange(&schema, None);

    assert_eq!(ranges.len(), 5); // 2 records + 2 local indexes + 1 global index.
    assert_eq!(
        ranges
            .iter()
            .filter(|range| range.TableInfo.IsPartition)
            .count(),
        4
    );
    assert_eq!(
        ranges
            .iter()
            .filter(|range| range.TableInfo.IsIndex)
            .count(),
        3
    );

    let databases = vec![
        Arc::new(DatabaseInfo {
            Name: ci("INFORMATION_SCHEMA"),
            ..Default::default()
        }),
        Arc::new(DatabaseInfo {
            Name: ci("test"),
            ..Default::default()
        }),
    ];
    let filtered = Helper::default().FilterMemDBs(databases);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].Name.L, "test");
}

#[derive(Clone)]
/// 固定 Region 定位的缓存替身。
struct StaticRegionCache;

impl RegionCache for StaticRegionCache {
    fn LocateKey(&self, _: &mut Backoffer, _: &[u8]) -> anyhow::Result<KeyLocation> {
        Ok(KeyLocation {
            Region: RegionVerID { ID: 9 },
            StartKey: vec![],
            EndKey: vec![],
        })
    }

    fn LocateRegionByID(&self, _: &mut Backoffer, id: u64) -> anyhow::Result<KeyLocation> {
        Ok(KeyLocation {
            Region: RegionVerID { ID: id },
            StartKey: vec![],
            EndKey: vec![],
        })
    }
}

/// 固定低分辨率时间戳的 Oracle 替身。
struct FixedOracle(u64);

impl Oracle for FixedOracle {
    fn GetLowResolutionTimestamp(&self) -> anyhow::Result<u64> {
        Ok(self.0)
    }
}

#[derive(Default)]
/// 计数 ResolveLocks 调用次数的解析器替身。
struct CountingResolver(AtomicUsize);

impl LockResolver for CountingResolver {
    fn ResolveLocksWithOpts(
        &self,
        _: &mut Backoffer,
        options: ResolveLocksOptions,
    ) -> anyhow::Result<ResolveLockResult> {
        assert_eq!(options.CallerStartTS, 10);
        assert!(options.Lite);
        assert!(!options.ForRead);
        assert_eq!(options.Locks.len(), 1);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ResolveLockResult { TTL: 0 })
    }
}

/// 按队列吐出 KvResponse 的 Storage 替身，用于 MVCC Get 遇锁重试。
struct MvccStore {
    responses: Mutex<VecDeque<KvResponse>>,
    oracle: Arc<FixedOracle>,
    resolver: Arc<CountingResolver>,
}

impl Storage for MvccStore {
    fn GetRegionCache(&self) -> Arc<dyn RegionCache> {
        Arc::new(StaticRegionCache)
    }

    fn SendReq(
        &self,
        _: &mut Backoffer,
        _: KvRequest,
        _: RegionVerID,
        _: Duration,
    ) -> anyhow::Result<KvResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("no response"))
    }

    fn GetOracle(&self) -> anyhow::Result<Arc<dyn Oracle>> {
        Ok(self.oracle.clone())
    }

    fn GetLockResolver(&self) -> anyhow::Result<Arc<dyn LockResolver>> {
        Ok(self.resolver.clone())
    }
}

/// 构造带事务锁的 MvccGetByKey 响应。
fn locked_response() -> KvResponse {
    KvResponse::MvccGetByKey(MvccGetByKeyResponse {
        Info: Some(MvccInfo {
            Lock: Some(LockInfo {
                Primary: b"primary".to_vec(),
                StartTS: 8,
                TTL: 100,
                TxnSize: 2,
                ..Default::default()
            }),
        }),
        ..Default::default()
    })
}

#[test]
/// 校验遇锁解析重试，并拒绝超过最新 ts 的快照。
fn mvcc_get_resolves_lock_retries_and_rejects_future_snapshot() {
    let resolver = Arc::new(CountingResolver::default());
    let store = Arc::new(MvccStore {
        responses: Mutex::new(VecDeque::from([
            locked_response(),
            KvResponse::MvccGetByKey(MvccGetByKeyResponse {
                Info: Some(MvccInfo::default()),
                ..Default::default()
            }),
        ])),
        oracle: Arc::new(FixedOracle(20)),
        resolver: resolver.clone(),
    });
    let helper = NewHelper(store);
    let response = helper
        .GetMvccByEncodedKeyWithTS(b"key".to_vec(), 10)
        .unwrap();
    assert_eq!(response.Info, Some(MvccInfo::default()));
    assert_eq!(resolver.0.load(Ordering::SeqCst), 1);

    let resolver = Arc::new(CountingResolver::default());
    let store = Arc::new(MvccStore {
        responses: Mutex::new(VecDeque::from([locked_response()])),
        oracle: Arc::new(FixedOracle(20)),
        resolver: resolver.clone(),
    });
    let error = NewHelper(store)
        .GetMvccByEncodedKeyWithTS(b"key".to_vec(), 21)
        .unwrap_err();
    assert!(error.to_string().contains("larger than latest allocated"));
    assert_eq!(resolver.0.load(Ordering::SeqCst), 0);
}

/// 一次性本地 HTTP 服务：回放状态码与 body，并把请求原文发回 channel。
fn one_shot_http_server(
    status: u16,
    body: &'static str,
) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") || count == 0 {
                break;
            }
        }
        let _ = sender.send(String::from_utf8(request).unwrap());
        let reason = if status == 200 {
            "OK"
        } else {
            "Internal Server Error"
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (address, receiver, handle)
}

#[test]
/// 校验 TiFlash/Columnar HTTP 路径与 Go 一致。
fn tiflash_and_columnar_http_paths_match_go() {
    let (address, request, server) = one_shot_http_server(200, "2\n1009 1010 \n");
    let mut replicas = HashMap::new();
    CollectTiFlashStatus(&address, 7, 41, &mut replicas).unwrap();
    assert_eq!(replicas, HashMap::from([(1009, 1), (1010, 1)]));
    assert!(
        request
            .recv()
            .unwrap()
            .starts_with("GET /tiflash/sync-status/keyspace/7/table/41 HTTP/1.1")
    );
    server.join().unwrap();

    let (address, request, server) =
        one_shot_http_server(200, r#"{"ready":2,"vector-index-ready":1,"total":3}"#);
    let status = CollectColumnarStatus(&address, 7, 41, Some(9)).unwrap();
    assert_eq!(status.Ready, 2);
    assert_eq!(status.VectorIndexReady, 1);
    assert_eq!(status.FtsIndexReady, 0);
    assert!(!status.HasFtsIndexReady);
    assert_eq!(status.Total, 3);
    assert!(request.recv().unwrap().starts_with(
        "GET /kvengine/columnar_status?keyspace_id=7&table_id=41&index_id=9 HTTP/1.1"
    ));
    server.join().unwrap();

    for (body, expected) in [
        (
            r#"{"ready":3,"vector-index-ready":2,"fts-index-ready":1,"total":4}"#,
            1,
        ),
        (
            r#"{"ready":3,"vector-index-ready":2,"fts-index-ready":0,"total":4}"#,
            0,
        ),
    ] {
        let (address, _, server) = one_shot_http_server(200, body);
        let status = CollectColumnarStatus(&address, 7, 41, Some(9)).unwrap();
        assert_eq!(status.FtsIndexReady, expected);
        assert!(status.HasFtsIndexReady);
        server.join().unwrap();
    }

    let (address, _, server) = one_shot_http_server(500, "bad status");
    let error = CollectColumnarStatus(&address, 7, 41, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("returned status 500: bad status")
    );
    server.join().unwrap();

    let canceled = RequestContext::background();
    canceled.cancel();
    assert!(CollectColumnarStatusWithCtx(&canceled, "127.0.0.1:1", 1, 1, None).is_err());
}

#[test]
fn storage_class_status_path_and_validation_match_go() {
    let (address, request, server) = one_shot_http_server(200, r#"{"ready":2,"total":3}"#);
    let status = CollectStorageClassStatus(&address, 7, 41, "IA").unwrap();
    assert_eq!(status, StorageClassStatusResp { Ready: 2, Total: 3 });
    assert!(request.recv().unwrap().starts_with(
        "GET /kvengine/storage_class_status?keyspace_id=7&table_id=41&target=IA HTTP/1.1"
    ));
    server.join().unwrap();

    for body in [r#"{"ready":1}"#, r#"{"total":1}"#, r#"{"ready":2,"total":1}"#] {
        let (address, _, server) = one_shot_http_server(200, body);
        assert!(CollectStorageClassStatus(&address, 7, 41, "STANDARD").is_err());
        server.join().unwrap();
    }

    let (address, _, server) = one_shot_http_server(500, "bad status");
    assert!(
        CollectStorageClassStatus(&address, 7, 41, "IA")
            .unwrap_err()
            .to_string()
            .contains("returned status 500: bad status")
    );
    server.join().unwrap();

    let canceled = RequestContext::background();
    canceled.cancel();
    assert!(CollectStorageClassStatusWithCtx(&canceled, "127.0.0.1:1", 1, 1, "IA").is_err());
}
