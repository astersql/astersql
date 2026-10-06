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

// Store Helper 行为测试。
//
// 覆盖 Region↔表映射、热点采集、PD Region 统计的 keyspace 编码、
// TiFlash 状态解析以及表键范围构造等。

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use crate::*;

/// 测试辅助：构造 CIStr。
fn ci(value: &str) -> CIStr {
    CIStr::new(value)
}

#[derive(Clone)]
/// 固定返回单一数据库的 Schema 替身。
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

/// 构造含多表/索引的 mock schema，供 Region 映射测试。
fn getMockRegionsTableInfoSchema() -> Arc<DatabaseInfo> {
    Arc::new(DatabaseInfo {
        Name: ci("test"),
        Tables: vec![
            Arc::new(TableMeta {
                ID: 41,
                Name: ci("t41"),
                Indices: vec![Arc::new(IndexMeta {
                    ID: 1,
                    Name: ci("i1"),
                    Global: false,
                })],
                ..Default::default()
            }),
            Arc::new(TableMeta {
                ID: 63,
                Name: ci("t63"),
                Indices: vec![
                    Arc::new(IndexMeta {
                        ID: 1,
                        Name: ci("i1"),
                        Global: false,
                    }),
                    Arc::new(IndexMeta {
                        ID: 2,
                        Name: ci("i2"),
                        Global: false,
                    }),
                ],
                ..Default::default()
            }),
            Arc::new(TableMeta {
                ID: 66,
                Name: ci("t66"),
                Indices: vec![
                    Arc::new(IndexMeta {
                        ID: 1,
                        Name: ci("i1"),
                        Global: false,
                    }),
                    Arc::new(IndexMeta {
                        ID: 2,
                        Name: ci("i2"),
                        Global: false,
                    }),
                    Arc::new(IndexMeta {
                        ID: 3,
                        Name: ci("i3"),
                        Global: false,
                    }),
                ],
                ..Default::default()
            }),
        ],
    })
}

/// 构造一组 mock Region 信息。
fn getMockTiKVRegionsInfo() -> RegionsInfo {
    RegionsInfo {
        Count: 8,
        Regions: vec![
            RegionInfo {
                ID: 1,
                StartKey: String::new(),
                EndKey: "12341234".into(),
            },
            RegionInfo {
                ID: 2,
                StartKey: "7480000000000000FF295F698000000000FF0000010000000000FA".into(),
                EndKey: "7480000000000000FF2B5F698000000000FF0000010000000000FA".into(),
            },
            RegionInfo {
                ID: 3,
                StartKey: "7480000000000000FF3F5F698000000000FF0000010000000000FA".into(),
                EndKey: "7480000000000000FF425F698000000000FF0000010000000000FA".into(),
            },
            RegionInfo {
                ID: 4,
                StartKey: "7480000000000000FF425F72C000000000FF0000000000000000FA".into(),
                EndKey: String::new(),
            },
            RegionInfo {
                ID: 5,
                StartKey: "7480000000000000FF425F698000000000FF0000030000000000FA".into(),
                EndKey: "7480000000000000FF425F72C000000000FF0000000000000000FA".into(),
            },
            RegionInfo {
                ID: 6,
                StartKey: "7480000000000000FF425F698000000000FF0000010000000000FA".into(),
                EndKey: "7480000000000000FF425F698000000000FF0000020000000000FA".into(),
            },
            RegionInfo {
                ID: 7,
                StartKey: "7480000000000000FF425F698000000000FF0000020000000000FA".into(),
                EndKey: "7480000000000000FF425F698000000000FF0000030000000000FA".into(),
            },
            RegionInfo {
                ID: 8,
                StartKey: "7480000000000000FF425F698000000000FF0000020000000000FA".into(),
                EndKey: "7480000000000000FF425F72C000000000FF0000000000000000FA".into(),
            },
        ],
    }
}

/// 字典序下一前缀，用于半开区间上界。
fn prefix_next(key: &[u8]) -> Vec<u8> {
    let mut next = key.to_vec();
    for index in (0..next.len()).rev() {
        if next[index] != u8::MAX {
            next[index] += 1;
            next.truncate(index + 1);
            return next;
        }
    }
    next.push(0);
    next
}

#[test]
/// 验证 Region 与表信息映射的基本正确性。
fn TestGetRegionsTableInfo() {
    let h = Helper::default();
    // 使用 mock Region 与 schema 做交集映射断言。
    let regions = getMockTiKVRegionsInfo();
    let schema = MockSchema {
        database: getMockRegionsTableInfoSchema(),
    };
    let table_infos = h.GetRegionsTableInfo(regions, &schema, None);
    let frames = |region_id| {
        table_infos[&region_id]
            .iter()
            .map(|info| {
                (
                    info.Table.ID,
                    info.IsIndex,
                    info.Index.as_ref().map(|index| index.ID),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(frames(1), vec![]);
    assert_eq!(frames(2), vec![(41, true, Some(1)), (41, false, None)]);
    assert_eq!(
        frames(3),
        vec![(63, true, Some(1)), (63, true, Some(2)), (63, false, None)]
    );
    assert_eq!(frames(4), vec![(66, false, None)]);
    assert_eq!(frames(5), vec![(66, true, Some(3)), (66, false, None)]);
    assert_eq!(frames(6), vec![(66, true, Some(1))]);
    assert_eq!(frames(7), vec![(66, true, Some(2))]);
    assert_eq!(
        frames(8),
        vec![(66, true, Some(2)), (66, true, Some(3)), (66, false, None)]
    );
}

#[test]
/// 验证带 keyspace 编码时 Region↔表映射仍正确。
fn TestGetRegionsTableInfoWithKeyspace() {
    let keyspace_id = 1_u32;
    let codec_v2 = Codec::v2(keyspace_id).unwrap();
    let db = getMockRegionsTableInfoSchema();
    let mut tables = Vec::new();
    for table in &db.Tables {
        tables.push(NewTableWithKeyRange(
            db.clone(),
            table.clone(),
            codec_v2.clone(),
        ));
        for index in &table.Indices {
            tables.push(NewIndexWithKeyRange(
                db.clone(),
                table.clone(),
                index.clone(),
                codec_v2.clone(),
            ));
        }
    }
    tables.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));

    let tbl41 = NewTableWithKeyRange(db.clone(), db.Tables[0].clone(), codec_v2.clone());
    let tbl41_idx1 = NewIndexWithKeyRange(
        db.clone(),
        db.Tables[0].clone(),
        db.Tables[0].Indices[0].clone(),
        codec_v2.clone(),
    );
    let tbl63 = NewTableWithKeyRange(db.clone(), db.Tables[1].clone(), codec_v2.clone());
    let tbl66 = NewTableWithKeyRange(db.clone(), db.Tables[2].clone(), codec_v2.clone());
    let regions = vec![
        RegionInfo {
            ID: 1,
            StartKey: String::new(),
            EndKey: tbl41_idx1.StartKey.clone(),
        },
        RegionInfo {
            ID: 2,
            StartKey: tbl41_idx1.StartKey.clone(),
            EndKey: tbl41.EndKey.clone(),
        },
        RegionInfo {
            ID: 3,
            StartKey: tbl63.StartKey.clone(),
            EndKey: tbl63.EndKey.clone(),
        },
        RegionInfo {
            ID: 4,
            StartKey: tbl66.StartKey.clone(),
            EndKey: tbl66.EndKey.clone(),
        },
    ];

    let h = Helper::default();
    let table_infos = h.ParseRegionsTableInfos(regions.clone(), tables);
    assert!(table_infos[&1].is_empty());
    assert_eq!(table_infos[&2].len(), 2);
    assert_eq!(table_infos[&2][0].Table.ID, 41);
    assert!(!table_infos[&3].is_empty());
    assert_eq!(table_infos[&3][0].Table.ID, 63);
    assert!(!table_infos[&4].is_empty());
    assert_eq!(table_infos[&4][0].Table.ID, 66);

    let tbl41_v1 = NewTableWithKeyRange(db.clone(), db.Tables[0].clone(), Codec::v1());
    assert_ne!(tbl41.StartKey, tbl41_v1.StartKey);

    let regions_info = RegionsInfo {
        Count: regions.len() as i64,
        Regions: regions,
    };
    let store = Arc::new(HotStore {
        pd: Arc::new(CapturingPdClient {
            hot: StoreHotPeersInfos::default(),
            regions: RegionsInfo::default(),
            captured: Arc::new(Mutex::new(None)),
            stats: RegionStats::default(),
        }),
        cache: Arc::new(StaticRegionCache::default()),
        codec: codec_v2,
    });
    let h_v2 = NewHelper(store);
    let via_api = h_v2.GetRegionsTableInfo(regions_info, &MockSchema { database: db }, None);
    assert_eq!(table_infos, via_api);
}

/// 记录请求参数的 PD 客户端替身。
struct CapturingPdClient {
    hot: StoreHotPeersInfos,
    regions: RegionsInfo,
    captured: Arc<Mutex<Option<(Vec<u8>, Vec<u8>)>>>,
    stats: RegionStats,
}

impl PdClient for CapturingPdClient {
    fn WithCallerID(&self, _: &str) -> Arc<dyn PdClient> {
        Arc::new(Self {
            hot: self.hot.clone(),
            regions: self.regions.clone(),
            captured: self.captured.clone(),
            stats: self.stats.clone(),
        })
    }

    fn GetRegionsByKeyRange(
        &self,
        _: &RequestContext,
        _: &[u8],
        _: &[u8],
        _: i32,
    ) -> anyhow::Result<RegionsInfo> {
        Ok(self.regions.clone())
    }

    fn GetHotReadRegions(&self, _: &RequestContext) -> anyhow::Result<StoreHotPeersInfos> {
        Ok(self.hot.clone())
    }

    fn GetHotWriteRegions(&self, _: &RequestContext) -> anyhow::Result<StoreHotPeersInfos> {
        Ok(self.hot.clone())
    }

    fn GetRegionStatusByKeyRange(
        &self,
        _: &RequestContext,
        start_key: &[u8],
        end_key: &[u8],
        _: bool,
    ) -> anyhow::Result<RegionStats> {
        *self.captured.lock().unwrap() = Some((start_key.to_vec(), end_key.to_vec()));
        Ok(self.stats.clone())
    }
}

#[derive(Default)]
/// 返回固定 Region 定位结果的缓存替身。
struct StaticRegionCache {
    locations: HashMap<u64, KeyLocation>,
}

impl RegionCache for StaticRegionCache {
    fn LocateKey(&self, _: &mut Backoffer, _: &[u8]) -> anyhow::Result<KeyLocation> {
        Ok(KeyLocation {
            Region: RegionVerID { ID: 1 },
            StartKey: vec![],
            EndKey: vec![],
        })
    }

    fn LocateRegionByID(&self, _: &mut Backoffer, id: u64) -> anyhow::Result<KeyLocation> {
        Ok(self.locations.get(&id).cloned().unwrap_or(KeyLocation {
            Region: RegionVerID { ID: id },
            StartKey: vec![],
            EndKey: vec![],
        }))
    }
}

/// 提供热点相关依赖的 Storage 替身。
struct HotStore {
    pd: Arc<dyn PdClient>,
    cache: Arc<dyn RegionCache>,
    codec: Codec,
}

impl Storage for HotStore {
    fn GetRegionCache(&self) -> Arc<dyn RegionCache> {
        self.cache.clone()
    }
    fn GetCodec(&self) -> Codec {
        self.codec.clone()
    }
    fn GetPDHTTPClient(&self) -> Option<Arc<dyn PdClient>> {
        Some(self.pd.clone())
    }
    fn GetPDAddrs(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec!["127.0.0.1:2379".into()])
    }
}

#[test]
/// 验证热点 Region 采集与表索引关联。
fn TestHotRegion() {
    let hot = StoreHotPeersInfos {
        AsLeader: HashMap::from([
            (
                1,
                HotPeersStat {
                    Stats: vec![HotPeerStat {
                        RegionID: 2,
                        ByteRate: 100.0,
                        HotDegree: 1,
                    }],
                },
            ),
            (
                2,
                HotPeersStat {
                    Stats: vec![HotPeerStat {
                        RegionID: 4,
                        ByteRate: 200.0,
                        HotDegree: 2,
                    }],
                },
            ),
        ]),
    };
    let pd = Arc::new(CapturingPdClient {
        hot,
        regions: RegionsInfo::default(),
        captured: Arc::new(Mutex::new(None)),
        stats: RegionStats::default(),
    });
    let cache = Arc::new(StaticRegionCache {
        locations: HashMap::from([
            (
                2,
                KeyLocation {
                    Region: RegionVerID { ID: 2 },
                    StartKey: vec![],
                    EndKey: vec![],
                },
            ),
            (
                4,
                KeyLocation {
                    Region: RegionVerID { ID: 4 },
                    StartKey: vec![],
                    EndKey: vec![],
                },
            ),
        ]),
    });
    let store = Arc::new(HotStore {
        pd,
        cache,
        codec: Codec::v1(),
    });
    let mut h = NewHelper(store);
    let metrics = h
        .FetchHotRegion(&RequestContext::background(), "read")
        .unwrap();
    assert_eq!(
        metrics,
        HashMap::from([
            (
                2,
                RegionMetric {
                    FlowBytes: 100,
                    MaxHotDegree: 1,
                    Count: 0
                }
            ),
            (
                4,
                RegionMetric {
                    FlowBytes: 200,
                    MaxHotDegree: 2,
                    Count: 0
                }
            ),
        ])
    );

    let schema = MockSchema {
        database: Arc::new(DatabaseInfo {
            Name: ci("test"),
            Tables: vec![],
        }),
    };
    let res = h.FetchRegionTableIndex(metrics, &schema, None).unwrap();
    assert_eq!(res.len(), 2);
    assert_ne!(res[0].RegionMetric, res[1].RegionMetric);
}

#[test]
/// 验证向 PD 查询 Region 统计时使用正确的 keyspace 编码。
fn TestGetPDRegionStatsKeyspaceEncoding() {
    let keyspace_id = 1_u32;
    let codec_v2 = Codec::v2(keyspace_id).unwrap();
    let captured = Arc::new(Mutex::new(None));
    let pd = Arc::new(CapturingPdClient {
        hot: StoreHotPeersInfos::default(),
        regions: RegionsInfo::default(),
        captured: captured.clone(),
        stats: RegionStats { Count: 0 },
    });
    let store = Arc::new(HotStore {
        pd,
        cache: Arc::new(StaticRegionCache::default()),
        codec: codec_v2.clone(),
    });
    let mut h = NewHelper(store);
    h.GetPDRegionStats(&RequestContext::background(), 41, false)
        .unwrap();

    let start = tablecodec::EncodeTablePrefix(41).0;
    let end = prefix_next(&start);
    let (expected_start, expected_end) = codec_v2.EncodeRegionRange(&start, &end);
    let keys = captured.lock().unwrap().clone().expect("keys captured");
    assert_eq!(keys.0, expected_start);
    assert_eq!(keys.1, expected_end);
}

#[test]
/// 验证 TiKV RegionsInfo 解析/序列化相关行为。
fn TestTiKVRegionsInfo() {
    let regions = getMockTiKVRegionsInfo();
    let pd = Arc::new(CapturingPdClient {
        hot: StoreHotPeersInfos::default(),
        regions: regions.clone(),
        captured: Arc::new(Mutex::new(None)),
        stats: RegionStats::default(),
    });
    let store = Arc::new(HotStore {
        pd,
        cache: Arc::new(StaticRegionCache::default()),
        codec: Codec::v1(),
    });
    let mut h = NewHelper(store);
    let got = h.GetRegions(&RequestContext::background()).unwrap();
    assert_eq!(got, regions);
}

#[test]
/// 验证 TiFlash 状态文本解析与副本计数累加。
fn TestComputeTiFlashStatus() {
    let mut replicas = HashMap::new();
    ComputeTiFlashStatus(
        &mut BufReader::new(std::io::Cursor::new("0\n\n")),
        &mut replicas,
    )
    .unwrap();
    ComputeTiFlashStatus(
        &mut BufReader::new(std::io::Cursor::new("2\n1009 1010 \n")),
        &mut replicas,
    )
    .unwrap();
    assert_eq!(replicas.len(), 2);
    assert_eq!(replicas[&1009], 1);
    assert_eq!(replicas[&1010], 1);

    let mut replicas2 = HashMap::new();
    let mut body = String::from("2000\n");
    for i in 1000..3000 {
        body.push_str(&format!("{i} "));
    }
    body.push('\n');
    ComputeTiFlashStatus(
        &mut BufReader::new(std::io::Cursor::new(body)),
        &mut replicas2,
    )
    .unwrap();
    assert_eq!(replicas2.len(), 2000);
    for i in 1000..3000 {
        assert!(replicas2.contains_key(&i));
    }
}

#[test]
fn collect_columnar_status_cancels_pending_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_started_tx, request_started_rx) = mpsc::channel();
    let (request_cancelled_tx, request_cancelled_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let size = stream.read(&mut request).unwrap();
        assert!(
            String::from_utf8_lossy(&request[..size])
                .starts_with("GET /kvengine/columnar_status?keyspace_id=7&table_id=9 HTTP/1.1")
        );
        request_started_tx.send(()).unwrap();

        while stream.read(&mut request).unwrap_or_default() != 0 {}
        request_cancelled_tx.send(()).unwrap();
    });

    let ctx = RequestContext::background();
    let request_ctx = ctx.clone();
    let request = thread::spawn(move || {
        CollectColumnarStatusWithCtx(&request_ctx, &address.to_string(), 7, 9, None)
    });

    request_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("columnar status request must start");
    ctx.cancel();

    let error = request
        .join()
        .unwrap()
        .expect_err("cancelling the context must stop the pending request");
    assert!(error.to_string().contains("context canceled"));
    request_cancelled_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pending HTTP request must observe cancellation");
    server.join().unwrap();
}

#[test]
/// 验证表键范围构造。
fn TestTableRange() {
    let start_key = tablecodec::GenTableRecordPrefix(1);
    let end_key = prefix_next(&start_key.0);
    assert_eq!(hex::encode(&start_key.0), "7480000000000000015f72");
    assert_eq!(hex::encode(&end_key), "7480000000000000015f73");

    let start_key = tablecodec::EncodeTablePrefix(1);
    let end_key = prefix_next(&start_key.0);
    assert_eq!(hex::encode(&start_key.0), "748000000000000001");
    assert_eq!(hex::encode(&end_key), "748000000000000002");
}

#[test]
/// 验证 keyspace 感知键编码走 Store Codec。
fn getKeyspaceAwareKey_uses_store_codec() {
    let raw = b"abc".to_vec();
    assert_eq!(Codec::v1().EncodeKey(&raw), raw);
    let encoded = Codec::v2(1).unwrap().EncodeKey(&raw);
    assert_ne!(encoded, raw);
}
