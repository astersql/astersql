// Copyright 2026 AsterSQL.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct DistributionRuntime {
    partitions: BTreeMap<String, i64>,
    ranges: std::sync::Mutex<Vec<(Vec<u8>, Vec<u8>)>>,
    jobs: Vec<DistributionJob>,
    keyspace_id: Option<u32>,
    limit: AtomicU64,
}

impl ShowRuntimeContext for DistributionRuntime {
    fn MaxChunkSize(&self) -> usize {
        32
    }
    fn SelectLimit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    fn SetSelectLimit(&self, value: u64) {
        self.limit.store(value, Ordering::Relaxed);
    }
    fn FetchRows(&self, _: ShowOperation, _: &ShowRequest) -> ShowResult<Vec<Vec<ShowValue>>> {
        unreachable!()
    }
    fn AllSchemaNames(&self) -> ShowResult<Vec<String>> {
        unreachable!()
    }
    fn DatabaseVisible(&self, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn SchemaExists(&self, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn Tables(&self, _: &str, _: bool) -> ShowResult<Vec<TableInfo>> {
        unreachable!()
    }
    fn TableByName(&self, _: &str, _: &str) -> ShowResult<Option<TableInfo>> {
        unreachable!()
    }
    fn TableVisible(&self, _: &str, _: &str) -> ShowResult<bool> {
        unreachable!()
    }
    fn TableRegions(&self, _: &TableName, _: &[i64]) -> ShowResult<Vec<regionMeta>> {
        unreachable!()
    }
    fn IndexRegions(&self, _: &TableName, _: &str, _: &[i64]) -> ShowResult<Vec<regionMeta>> {
        unreachable!()
    }
    fn SchedulingInfo(
        &self,
        _: &[regionMeta],
        _: &TableName,
    ) -> ShowResult<Vec<showTableRegionRowItem>> {
        unreachable!()
    }
    fn ImportJobs(&self, _: &ShowRequest) -> ShowResult<Vec<ImportJobInfo>> {
        unreachable!()
    }
    fn ImportRuntime(&self, _: i64) -> ShowResult<Option<ImportRuntimeInfo>> {
        unreachable!()
    }
    fn DistributionJobs(&self, _: &ShowRequest) -> ShowResult<Vec<DistributionJob>> {
        Ok(self.jobs.clone())
    }
    fn DistributionKeyspaceID(&self) -> Option<u32> {
        self.keyspace_id
    }
    fn DistributionCodec(&self) -> astersql_store_helper::Codec {
        match self.keyspace_id {
            Some(id) => astersql_store_helper::Codec::v2(id).unwrap(),
            None => astersql_store_helper::Codec::v1(),
        }
    }
    fn DistributionTable(
        &self,
        _: &ShowRequest,
    ) -> ShowResult<crate::distribute::DistributionTable> {
        Ok(crate::distribute::DistributionTable {
            database_name: "test".into(),
            table_name: "orders".into(),
            table_id: 9,
            partitions: self.partitions.clone(),
        })
    }
    fn RegionDistributions(&self, start: &[u8], end: &[u8]) -> ShowResult<Vec<RegionDistribution>> {
        self.ranges
            .lock()
            .unwrap()
            .push((start.to_vec(), end.to_vec()));
        Ok(vec![RegionDistribution {
            store_id: 1,
            engine_type: "tikv".into(),
            region_leader_count: 1,
            region_peer_count: 3,
            ..Default::default()
        }])
    }
    fn RunWithSystemSession(
        &self,
        action: &mut dyn FnMut(&dyn ShowRuntimeContext) -> ShowResult,
    ) -> ShowResult {
        action(self)
    }
    fn FillViewColumnType(&self, _: &str, _: &TableInfo) -> ShowResult {
        unreachable!()
    }
    fn FormatSplitExpression(&self, _: &str) -> ShowResult<String> {
        unreachable!()
    }
}

#[test]
fn distribution_job_keyspace_filter_preserves_legacy_aliases_and_job_id_lookup() {
    let aliases = [
        "2:test.orders.",
        "3:test.orders.",
        "test.legacy.partition(p0)",
        "bad:test.orders.",
        ":test.orders.",
        "4294967296:test.orders.",
        "-1:test.orders.",
        "+2:test.orders.",
        "02:test.orders.",
    ];
    for keyspace_id in [None, Some(2)] {
        for requested in [None, Some(1), Some(2), Some(3)] {
            let runtime = Arc::new(DistributionRuntime {
                partitions: Default::default(),
                ranges: Default::default(),
                jobs: aliases
                    .iter()
                    .enumerate()
                    .map(|(i, alias)| DistributionJob {
                        job_id: i as u64 + 1,
                        alias: (*alias).into(),
                        ..Default::default()
                    })
                    .collect(),
                keyspace_id,
                limit: AtomicU64::new(1),
            });
            let mut show = ShowExec::new(runtime, ShowStmtType::DistributionJobs);
            show.DistributionJobID = requested;
            show.fetchShowDistributionJobs().unwrap();
            let rows = show.result.as_deref().unwrap_or_default();
            let expected: Vec<_> = aliases
                .iter()
                .enumerate()
                .filter(|(i, _)| requested.is_none_or(|id| id == *i as i64 + 1))
                .filter(|(i, _)| keyspace_id.is_none() || *i != 1)
                .collect();
            assert_eq!(rows.len(), expected.len());
            for (row, (i, alias)) in rows.iter().zip(expected) {
                let db = if keyspace_id.is_some() && [0, 8].contains(&i) {
                    "test"
                } else {
                    alias.split('.').next().unwrap()
                };
                assert_eq!(
                    row[1],
                    ShowValue::String(db.into()),
                    "alias={alias}, keyspace={keyspace_id:?}"
                );
            }
        }
    }
}

#[test]
fn distribution_job_show_table_uses_store_codec_range() {
    let runtime = Arc::new(DistributionRuntime {
        jobs: Vec::new(),
        keyspace_id: Some(2),
        limit: AtomicU64::new(1),
        partitions: Default::default(),
        ranges: Default::default(),
    });
    let mut show = ShowExec::new(runtime.clone(), ShowStmtType::Distributions);
    show.Table = Some(TableName {
        schema: "test".into(),
        name: "orders".into(),
        partition_names: Vec::new(),
    });
    show.fetchShowDistributions().unwrap();
    let expected_start = vec![
        b'x', 0, 0, 2, b't', 0x80, 0, 0, 0xff, 0, 0, 0, 0, 9, 0, 0, 0, 0xfc,
    ];
    let expected_end = vec![
        b'x', 0, 0, 2, b't', 0x80, 0, 0, 0xff, 0, 0, 0, 0, 10, 0, 0, 0, 0xfc,
    ];
    assert_eq!(
        *runtime.ranges.lock().unwrap(),
        vec![(expected_start, expected_end)]
    );
    assert_eq!(
        show.result.as_ref().unwrap()[0][0],
        ShowValue::String("orders".into())
    );
    assert_eq!(show.result.as_ref().unwrap()[0][1], ShowValue::Uint64(1));
}

#[test]
fn distribution_job_show_partition_ranges_stay_separate_in_both_api_versions() {
    for keyspace_id in [None, Some(2)] {
        for names in [vec![], vec!["P1"], vec!["p1", "p0"], vec!["PÅ"]] {
            let runtime = Arc::new(DistributionRuntime {
                jobs: Vec::new(),
                keyspace_id,
                limit: AtomicU64::new(1),
                ranges: Default::default(),
                partitions: BTreeMap::from([
                    ("p0".into(), 10),
                    ("p1".into(), 11),
                    ("pÅ".into(), 13),
                ]),
            });
            let mut show = ShowExec::new(runtime.clone(), ShowStmtType::Distributions);
            show.Table = Some(TableName {
                schema: "test".into(),
                name: "orders".into(),
                partition_names: names.iter().map(|name| (*name).into()).collect(),
            });
            show.fetchShowDistributions().unwrap();
            let ids = if names.is_empty() {
                vec![10, 11, 13]
            } else {
                names
                    .iter()
                    .map(|name| {
                        if name.eq_ignore_ascii_case("p0") {
                            10
                        } else if name.eq_ignore_ascii_case("p1") {
                            11
                        } else {
                            13
                        }
                    })
                    .collect()
            };
            let expected: Vec<_> = ids
                .iter()
                .map(|id| {
                    (
                        expected_region_key(*id, keyspace_id),
                        expected_region_key(*id + 1, keyspace_id),
                    )
                })
                .collect();
            assert_eq!(*runtime.ranges.lock().unwrap(), expected);
            assert_eq!(show.result.as_ref().unwrap().len(), ids.len());
            if names == ["PÅ"] {
                assert_eq!(
                    show.result.as_ref().unwrap()[0][0],
                    ShowValue::String("på".into())
                );
            }
            if names == ["P1"] {
                assert_eq!(
                    show.result.as_ref().unwrap()[0][0],
                    ShowValue::String("p1".into())
                );
            }
        }
    }
}

// Independent of the store codec: the wire prefix precedes memcomparable encoding.
fn expected_region_key(id: i64, keyspace_id: Option<u32>) -> Vec<u8> {
    let mut raw = Vec::new();
    if let Some(id) = keyspace_id {
        raw.extend_from_slice(&[b'x', (id >> 16) as u8, (id >> 8) as u8, id as u8]);
    }
    raw.push(b't');
    raw.extend_from_slice(&((id as u64) ^ (1 << 63)).to_be_bytes());
    astersql_util_codec::EncodeBytes(Vec::new(), &raw)
}

#[test]
fn distribution_job_invalid_visible_alias_still_errors_after_keyspace_filter() {
    for (alias, errors) in [("3:invalid", false), ("2:invalid", true), ("invalid", true)] {
        let runtime = Arc::new(DistributionRuntime {
            partitions: Default::default(),
            ranges: Default::default(),
            keyspace_id: Some(2),
            limit: AtomicU64::new(1),
            jobs: vec![DistributionJob {
                job_id: 1,
                alias: alias.into(),
                ..Default::default()
            }],
        });
        let mut show = ShowExec::new(runtime, ShowStmtType::DistributionJobs);
        assert_eq!(show.fetchShowDistributionJobs().is_err(), errors);
    }
}
