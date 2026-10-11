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

// 规划器单测：验证 Go planner 的外置元数据、导入、排序、冲突与范围拆分语义。

use crate::{KVGroupConflictInfos, totalConflicts};
use astersql_ingestor_engineapi::ConflictInfo;

#[test]
fn post_process_reads_previous_wire_metas_and_reports_invalid_json() {
    use crate::{PipelineSpec, PlanCtx, PostProcessSpec};
    use astersql_dxf_framework_proto as dxfproto;
    use std::collections::HashMap;

    let import = crate::ImportStepMeta {
        Checksum: HashMap::from([(
            -1,
            crate::Checksum {
                Size: 1,
                KVs: 2,
                Sum: 3,
            },
        )]),
        ..Default::default()
    };
    let mut ctx = PlanCtx {
        NextTaskStep: dxfproto::ImportStepPostProcess,
        PreviousSubtaskMetas: HashMap::from([(
            dxfproto::ImportStepImport,
            vec![import.Marshal().unwrap()],
        )]),
        ..Default::default()
    };
    let spec = PostProcessSpec {
        Schema: String::new(),
        Table: String::new(),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&spec.ToSubtaskMeta(&ctx).unwrap()).unwrap();
    assert_eq!(value["Checksum"]["-1"]["Size"], 1);
    assert_eq!(value["Checksum"]["-1"]["KVs"], 2);
    ctx.PreviousSubtaskMetas
        .insert(dxfproto::ImportStepImport, vec![b"{".to_vec()]);
    assert!(spec.ToSubtaskMeta(&ctx).is_err());
    ctx.PreviousSubtaskMetas.clear();
    ctx.PreviousImportMetas = vec![crate::ImportStepMeta {
        MaxIDs: HashMap::from([(astersql_meta_autoid::AllocatorType::RowId, -1)]),
        ..Default::default()
    }];
    let negative: serde_json::Value =
        serde_json::from_slice(&spec.ToSubtaskMeta(&ctx).unwrap()).unwrap();
    assert!(negative["MaxIDs"].as_object().unwrap().is_empty());
}

#[test]
fn merge_sort_skips_multiple_nonoverlapping_files_like_go() {
    use crate::{
        ImportStepMeta, LogicalPlan, MultipleFilesStat, PlanCtx, SortedKVMeta,
        generateMergeSortSpecs,
    };
    let stat = MultipleFilesStat {
        MinKey: b"a".to_vec(),
        MaxKey: b"z".to_vec(),
        Filenames: vec![
            ["one".into(), "one.stat".into()],
            ["two".into(), "two.stat".into()],
        ],
        MaxOverlappingNum: 1,
    };
    let ctx = PlanCtx {
        ObjectStore: Some(
            astersql_objstore::storage::NewFromURL(
                &astersql_objstore::storage::Context::background(),
                "memstore://planner-merge-overlap",
            )
            .unwrap(),
        ),
        PreviousImportMetas: vec![ImportStepMeta {
            SortedDataMeta: Some(SortedKVMeta {
                StartKey: b"a".to_vec(),
                EndKey: b"z".to_vec(),
                MultipleFilesStats: vec![stat],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ThreadCnt: 16,
        ..Default::default()
    };
    assert_eq!(
        generateMergeSortSpecs(&ctx, &mut LogicalPlan::default())
            .unwrap()
            .len(),
        0
    );
    let mut high_overlap = ctx.clone();
    high_overlap.PreviousImportMetas[0]
        .SortedDataMeta
        .as_mut()
        .unwrap()
        .MultipleFilesStats[0]
        .MaxOverlappingNum = 5000;
    assert_eq!(
        generateMergeSortSpecs(&high_overlap, &mut LogicalPlan::default())
            .unwrap()
            .len(),
        1
    );
    let mut forced_data = ctx.clone();
    forced_data.ForceMergeGroup = Some("data".into());
    forced_data.PreviousImportMetas[0].SortedIndexMetas.insert(
        7,
        SortedKVMeta {
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
            MultipleFilesStats: vec![MultipleFilesStat {
                Filenames: vec![["index".into(), "index.stat".into()]],
                MaxOverlappingNum: 1,
                MinKey: b"a".to_vec(),
                MaxKey: b"z".to_vec(),
            }],
            ..Default::default()
        },
    );
    let forced_specs = generateMergeSortSpecs(&forced_data, &mut LogicalPlan::default()).unwrap();
    assert_eq!(forced_specs.len(), 1);
    assert_eq!(
        forced_specs[0]
            .as_any()
            .downcast_ref::<crate::MergeSortSpec>()
            .unwrap()
            .MergeSortStepMeta
            .KVGroup,
        "data"
    );
    let mut force_all_plan = LogicalPlan::default();
    force_all_plan.Plan.ForceMergeStep = true;
    assert_eq!(
        generateMergeSortSpecs(&forced_data, &mut force_all_plan)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn global_sort_planning_opens_store_before_inspecting_previous_metas() {
    use crate::{
        LogicalPlan, PlanCtx, generateCollectConflictsSpecs, generateConflictResolutionSpecs,
        generateMergeSortSpecs,
    };
    let mut plan = LogicalPlan::default();
    plan.Plan.CloudStorageURI = "unknown://bucket".into();
    let ctx = PlanCtx::default();
    assert!(generateMergeSortSpecs(&ctx, &mut plan).is_err());
    assert!(generateCollectConflictsSpecs(&ctx, &mut plan).is_err());
    assert!(generateConflictResolutionSpecs(&ctx, &mut plan).is_err());
}

#[test]
fn prepared_chunk_map_precedes_inline_map_and_global_plan_persists_external_chunks() {
    use crate::{ImportSpec, LogicalPlan, PlanCtx, PreparedMeta, generateImportSpecs};
    use astersql_dxf_framework_proto as dxfproto;
    use astersql_executor_importer as importer;
    use astersql_objstore as objstore;
    use std::collections::HashMap;
    let storage_ctx = objstore::storage::Context::background();
    let store = objstore::storage::NewFromURL(&storage_ctx, "memstore://planner-prepared").unwrap();
    let prepared = PreparedMeta {
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "prepared.csv".into(),
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    store
        .WriteFile(
            &storage_ctx,
            "prepared/meta.json",
            &prepared.Marshal().unwrap(),
        )
        .unwrap();
    let mut plan = LogicalPlan {
        PreparedChunkMapExternalPath: "prepared/meta.json".into(),
        ChunkMap: HashMap::from([(
            2,
            vec![importer::Chunk {
                Path: "ignored.csv".into(),
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    let ctx = PlanCtx {
        ObjectStore: Some(store.clone()),
        StorageContext: storage_ctx.clone(),
        ..Default::default()
    };
    let specs = generateImportSpecs(&ctx, &mut plan).unwrap();
    let spec = specs[0].as_any().downcast_ref::<ImportSpec>().unwrap();
    assert_eq!(spec.ImportStepMeta.ID, 1);
    assert_eq!(spec.ImportStepMeta.Chunks[0].Path, "prepared.csv");
    store
        .WriteFile(
            &storage_ctx,
            "prepared/empty.json",
            &PreparedMeta::default().Marshal().unwrap(),
        )
        .unwrap();
    plan.PreparedChunkMapExternalPath = "prepared/empty.json".into();
    assert!(generateImportSpecs(&ctx, &mut plan).unwrap().is_empty());
    plan.PreparedChunkMapExternalPath = "prepared/meta.json".into();
    let global_ctx = PlanCtx {
        NextTaskStep: dxfproto::ImportStepEncodeAndSort,
        GlobalSort: true,
        ObjectStore: Some(store.clone()),
        StorageContext: storage_ctx.clone(),
        TaskID: 123,
        ..Default::default()
    };
    let physical = plan.ToPhysicalPlan(global_ctx.clone()).unwrap();
    let envelope = physical
        .ToSubtaskMetas(&global_ctx, dxfproto::ImportStepEncodeAndSort)
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&envelope[0]).unwrap();
    assert_eq!(value["ExternalPath"], "123/plan/encode/1/meta.json");
    assert!(value.get("Chunks").is_none());
    let external: serde_json::Value = serde_json::from_slice(
        &store
            .ReadFile(&storage_ctx, "123/plan/encode/1/meta.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(external["Chunks"][0]["Path"], "prepared.csv");
    let mut invalid = LogicalPlan {
        Plan: importer::Plan {
            CloudStorageURI: "unknown://bucket".into(),
            ..Default::default()
        },
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "a.csv".into(),
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    let invalid_ctx = PlanCtx {
        NextTaskStep: dxfproto::ImportStepEncodeAndSort,
        GlobalSort: true,
        ..Default::default()
    };
    assert!(invalid.ToPhysicalPlan(invalid_ctx).is_err());
}

#[test]
fn ingest_split_reads_real_sort_file_and_preserves_commit_ts() {
    use crate::planner::split_for_one_subtask;
    use crate::{MultipleFilesStat, PlanCtx, SortedKVMeta, WriteIngestSpec};
    use astersql_ingestor_globalsort as globalsort;
    use astersql_objstore as objstore;
    let storage_ctx = objstore::storage::Context::background();
    let store = objstore::storage::NewFromURL(&storage_ctx, "memstore://planner-split").unwrap();
    let pairs: Vec<globalsort::KvPair> = (0..140)
        .map(|index| globalsort::KvPair {
            key: format!("{index:05}").into_bytes(),
            value: vec![0; 1024 * 1024],
        })
        .collect();
    store
        .WriteFile(&storage_ctx, "sort/data", &globalsort::encode_kvs(&pairs))
        .unwrap();
    let props: Vec<_> = (0..140)
        .map(|index| astersql_ingestor_simplesst::codec::RangeProperty {
            FirstKey: format!("{index:05}").into_bytes(),
            LastKey: format!("{index:05}").into_bytes(),
            Size: 1024 * 1024 + 5,
            Keys: 1,
            Offset: index * (1024 * 1024 + 5),
        })
        .collect();
    store
        .WriteFile(
            &storage_ctx,
            "sort/stat",
            &astersql_ingestor_simplesst::codec::encode_multi_props(&props).unwrap(),
        )
        .unwrap();
    let meta = SortedKVMeta {
        StartKey: b"00000".to_vec(),
        EndKey: b"00139\0".to_vec(),
        MultipleFilesStats: vec![MultipleFilesStat {
            Filenames: vec![["sort/data".into(), "sort/stat".into()]],
            ..Default::default()
        }],
        ..Default::default()
    };
    let ctx = PlanCtx {
        StorageContext: storage_ctx,
        ..Default::default()
    };
    let specs =
        split_for_one_subtask(&ctx, store.clone(), "data".into(), meta.clone(), 123).unwrap();
    assert_eq!(specs.len(), 1);
    let spec = specs[0].as_any().downcast_ref::<WriteIngestSpec>().unwrap();
    assert_eq!(spec.WriteIngestStepMeta.TS, 123);
    assert_eq!(spec.WriteIngestStepMeta.DataFiles, ["sort/data"]);
    assert_eq!(
        spec.WriteIngestStepMeta.RangeJobKeys.first().unwrap(),
        b"00000"
    );
    assert_eq!(
        spec.WriteIngestStepMeta.RangeSplitKeys.last().unwrap(),
        b"00139\0"
    );
    assert_eq!(
        spec.WriteIngestStepMeta.RangeSplitKeys.len(),
        if astersql_config_kerneltype::IsNextGen() {
            2
        } else {
            3
        }
    );
    if !astersql_config_kerneltype::IsNextGen() {
        assert_eq!(spec.WriteIngestStepMeta.RangeSplitKeys[1], b"00096");
    }
    let mut missing_stat = meta.clone();
    missing_stat.MultipleFilesStats[0].Filenames[0][1] = "sort/missing.stat".into();
    assert!(split_for_one_subtask(&ctx, store.clone(), "data".into(), missing_stat, 123).is_err());
    let mut invalid_range = meta;
    invalid_range.StartKey = b"z".to_vec();
    let error = split_for_one_subtask(&ctx, store, "data".into(), invalid_range, 123)
        .err()
        .unwrap();
    assert!(error.to_string().contains("invalid kv range"));
}

#[test]
fn raw_encode_meta_drives_merge_and_conflict_plans_with_go_wire_bytes() {
    use crate::{
        ImportStepMeta, LogicalPlan, MultipleFilesStat, PlanCtx, SortedKVMeta,
        collectConflictInfos, generateMergeSortSpecs,
    };
    use astersql_dxf_framework_proto as dxfproto;
    use std::collections::HashMap;
    let sorted = SortedKVMeta {
        StartKey: b"a".to_vec(),
        EndKey: b"z".to_vec(),
        MultipleFilesStats: vec![MultipleFilesStat {
            MinKey: b"a".to_vec(),
            MaxKey: b"z".to_vec(),
            Filenames: vec![
                ["one".into(), "one.stat".into()],
                ["two".into(), "two.stat".into()],
            ],
            MaxOverlappingNum: 5000,
        }],
        ConflictInfo: ConflictInfo {
            Count: 2,
            Files: vec!["conflicts".into()],
        },
        ..Default::default()
    };
    let meta = ImportStepMeta {
        SortedDataMeta: Some(sorted),
        RecordedConflictKVCount: 2,
        ..Default::default()
    };
    let bytes = meta.Marshal().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["SortedDataMeta"]["start-key"], "YQ==");
    assert_eq!(value["SortedDataMeta"]["conflict-info"]["Count"], 2);
    let ctx = PlanCtx {
        ObjectStore: Some(
            astersql_objstore::storage::NewFromURL(
                &astersql_objstore::storage::Context::background(),
                "memstore://planner-raw-wire",
            )
            .unwrap(),
        ),
        PreviousSubtaskMetas: HashMap::from([(dxfproto::ImportStepEncodeAndSort, vec![bytes])]),
        ThreadCnt: 16,
        ..Default::default()
    };
    let mut plan = LogicalPlan::default();
    plan.Plan.ForceMergeStep = true;
    assert_eq!(generateMergeSortSpecs(&ctx, &mut plan).unwrap().len(), 1);
    assert_eq!(
        collectConflictInfos(&ctx, &plan).unwrap().ConflictInfos["data"].Count,
        2
    );
    let bad = PlanCtx {
        PreviousSubtaskMetas: HashMap::from([(
            dxfproto::ImportStepEncodeAndSort,
            vec![b"{".to_vec()],
        )]),
        ..Default::default()
    };
    assert!(collectConflictInfos(&bad, &plan).is_err());
}

#[test]
fn logical_plan_round_trip_and_post_process_processor_match_go() {
    use crate::{LogicalPlan, PlanCtx};
    use astersql_dxf_framework_proto as dxfproto;
    use astersql_executor_importer as importer;
    use std::collections::HashMap;
    let plan = LogicalPlan {
        JobID: 1,
        Stmt: "IMPORT INTO db.tb FROM 'a.csv'".into(),
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "a.csv".into(),
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    let bytes = plan.ToTaskMeta().unwrap();
    let mut restored = LogicalPlan::default();
    restored.FromTaskMeta(&bytes).unwrap();
    assert_eq!(restored.JobID, plan.JobID);
    assert_eq!(restored.Stmt, plan.Stmt);
    assert_eq!(restored.ChunkMap[&1][0].Path, "a.csv");
    let import_ctx = PlanCtx {
        NextTaskStep: dxfproto::ImportStepImport,
        ..Default::default()
    };
    let import_plan = restored.ToPhysicalPlan(import_ctx.clone()).unwrap();
    assert_eq!(import_plan.Processors.len(), 1);
    assert_eq!(import_plan.Processors[0].ID, 0);
    assert_eq!(import_plan.Processors[0].Output.Links[0].ProcessorID, 1);
    let import_metas = import_plan
        .ToSubtaskMetas(&import_ctx, dxfproto::ImportStepImport)
        .unwrap();
    let imported = crate::ImportStepMeta::Unmarshal(&import_metas[0]).unwrap();
    assert_eq!(imported.ID, 1);
    assert_eq!(imported.Chunks[0].Path, "a.csv");
    let import_wire: serde_json::Value = serde_json::from_slice(&import_metas[0]).unwrap();
    assert!(import_wire["Checksum"].is_null());
    assert!(import_wire["MaxIDs"].is_null());
    assert!(import_wire["SortedIndexMetas"].is_null());
    assert!(import_wire.get("RecordedConflictKVCount").is_none());
    let physical = restored
        .ToPhysicalPlan(PlanCtx {
            NextTaskStep: dxfproto::ImportStepPostProcess,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(physical.Processors.len(), 1);
    assert_eq!(
        physical.Processors[0].Input.ColumnTypes,
        [8, 8, 8, 8, 8, 0xf5]
    );
    assert!(physical.Processors[0].Output.Links.is_empty());
}

#[test]
fn empty_chunk_map_runs_real_controller_bridge_and_populates_regions() {
    use crate::{ImportSpec, LogicalPlan, PlanCtx, generateImportSpecs};
    use astersql_executor_importer as importer;
    use astersql_meta_model as model;
    use astersql_planner_core_operator_physicalop::MetadataTableAdapter;
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
    struct Regions;
    impl importer::TableImporterService for Regions {
        fn RuntimeConfig(&self) -> importer::ImportRuntimeConfig {
            unreachable!()
        }
        fn NewEncodingTable(
            &self,
            _: &importer::LoadDataController,
        ) -> Result<Arc<dyn astersql_lightning_backend_encode::Table>, String> {
            unreachable!()
        }
        fn NewBackend(
            &self,
            _: &importer::LoadDataController,
            _: &std::path::Path,
        ) -> Result<Arc<dyn astersql_lightning_backend::Backend>, String> {
            unreachable!()
        }
        fn RegionSplitSizeKeys(&self) -> Result<(i64, i64), String> {
            unreachable!()
        }
        fn NewParser(
            &self,
            _: &importer::LoadDataController,
            _: &importer::Chunk,
        ) -> Result<Box<dyn astersql_lightning_mydump::Parser + Send>, String> {
            unreachable!()
        }
        fn EstimateParquetReaderMemory(
            &self,
            _: &importer::LoadDataController,
            _: &str,
        ) -> Result<i64, String> {
            unreachable!()
        }
        fn MakeTableRegions(
            &self,
            _: &importer::LoadDataController,
            _: i64,
        ) -> Result<Vec<importer::TableRegion>, String> {
            Ok(vec![importer::TableRegion {
                EngineID: 1,
                File: astersql_lightning_mydump::SourceFileMeta {
                    path: "query-row".into(),
                    ..Default::default()
                },
                RowIDMax: 2,
                ..Default::default()
            }])
        }
        fn EstimateCompactionThreshold(&self, _: i64) -> i64 {
            unreachable!()
        }
        fn ImportedKVCount(&self, _: &astersql_lightning_backend::ClosedEngine) -> i64 {
            unreachable!()
        }
        fn DiskCapacity(&self, _: &std::path::Path) -> Result<u64, String> {
            unreachable!()
        }
        fn CheckDiskQuota(
            &self,
            _: &dyn astersql_lightning_backend::Backend,
            _: i64,
        ) -> importer::DiskQuotaState {
            unreachable!()
        }
        fn FlushAndImportLargeEngines(
            &self,
            _: &dyn astersql_lightning_backend::Backend,
            _: &[i32],
        ) -> Result<(), String> {
            unreachable!()
        }
        fn RebaseAllocatorBases(
            &self,
            _: &std::collections::HashMap<astersql_lightning_backend_kv::AllocatorType, i64>,
            _: &importer::Plan,
        ) -> Result<(), String> {
            unreachable!()
        }
        fn RemoteChecksumTableBySQL(
            &self,
            _: &importer::Plan,
            _: usize,
            _: i32,
        ) -> Result<importer::RemoteChecksum, importer::RemoteChecksumError> {
            unreachable!()
        }
        fn FlushTableStats(&self, _: i64, _: i64) -> Result<(), String> {
            unreachable!()
        }
        fn AllocatorMaximums(
            &self,
        ) -> std::collections::HashMap<astersql_lightning_backend_kv::AllocatorType, i64> {
            unreachable!()
        }
    }
    let table_info = model::TableInfo::default();
    let table = Arc::new(MetadataTableAdapter::New(&table_info));
    let ctx = PlanCtx {
        Table: Some(table),
        ControllerServices: Some(Arc::new(services)),
        ImporterService: Some(Arc::new(Regions)),
        ExecuteNodesCnt: 2,
        ..Default::default()
    };
    let mut plan = LogicalPlan {
        Plan: importer::Plan {
            TableInfo: Some(Arc::new(table_info)),
            DataSourceType: importer::DataSourceTypeQuery,
            InImportInto: true,
            ..Default::default()
        },
        Stmt: "IMPORT INTO db.tb FROM 'a.csv'".into(),
        ..Default::default()
    };
    let specs = generateImportSpecs(&ctx, &mut plan).unwrap();
    assert_eq!(specs.len(), 1);
    let meta = &specs[0]
        .as_any()
        .downcast_ref::<ImportSpec>()
        .unwrap()
        .ImportStepMeta;
    assert_eq!(meta.ID, 1);
    assert_eq!(meta.Chunks[0].Path, "query-row");
    assert_eq!(plan.summary.RowCnt, 2);
}

#[test]
fn external_conflict_info_is_loaded_and_missing_object_propagates_error() {
    use crate::{BaseExternalMeta, ImportStepMeta, LogicalPlan, PlanCtx, collectConflictInfos};
    use astersql_dxf_framework_proto as dxfproto;
    use astersql_objstore as objstore;
    use std::collections::HashMap;
    let storage_ctx = objstore::storage::Context::background();
    let store = objstore::storage::NewFromURL(&storage_ctx, "memstore://planner-conflict").unwrap();
    store.WriteFile(&storage_ctx, "conflict/meta.json", br#"{"SortedDataMeta":{"start-key":"YQ==","end-key":"eg==","conflict-info":{"Count":3,"Files":["c1"]}}}"#).unwrap();
    let import = ImportStepMeta {
        BaseExternalMeta: BaseExternalMeta {
            ExternalPath: "conflict/meta.json".into(),
        },
        RecordedConflictKVCount: 3,
        ..Default::default()
    };
    let ctx = PlanCtx {
        PreviousSubtaskMetas: HashMap::from([(
            dxfproto::ImportStepEncodeAndSort,
            vec![import.Marshal().unwrap()],
        )]),
        ObjectStore: Some(store.clone()),
        StorageContext: storage_ctx.clone(),
        ..Default::default()
    };
    assert_eq!(
        collectConflictInfos(&ctx, &LogicalPlan::default())
            .unwrap()
            .ConflictInfos["data"]
            .Count,
        3
    );
    let mut missing = ctx;
    missing.PreviousSubtaskMetas.insert(
        dxfproto::ImportStepEncodeAndSort,
        vec![
            ImportStepMeta {
                BaseExternalMeta: BaseExternalMeta {
                    ExternalPath: "missing.json".into(),
                },
                RecordedConflictKVCount: 3,
                ..Default::default()
            }
            .Marshal()
            .unwrap(),
        ],
    );
    assert!(collectConflictInfos(&missing, &LogicalPlan::default()).is_err());
    missing.PreviousSubtaskMetas.insert(
        dxfproto::ImportStepEncodeAndSort,
        vec![
            ImportStepMeta {
                BaseExternalMeta: BaseExternalMeta {
                    ExternalPath: "missing.json".into(),
                },
                RecordedConflictKVCount: 0,
                ..Default::default()
            }
            .Marshal()
            .unwrap(),
        ],
    );
    assert!(
        collectConflictInfos(&missing, &LogicalPlan::default())
            .unwrap()
            .ConflictInfos
            .is_empty()
    );
}

#[test]
fn conflict_specs_merge_data_index_and_later_step_counts() {
    use crate::{
        ImportStepMeta, LogicalPlan, MergeSortStepMeta, PlanCtx, SortedKVMeta, WriteIngestStepMeta,
        generateCollectConflictsSpecs, generateConflictResolutionSpecs,
    };
    use std::collections::HashMap;
    let ctx = PlanCtx {
        ObjectStore: Some(
            astersql_objstore::storage::NewFromURL(
                &astersql_objstore::storage::Context::background(),
                "memstore://planner-conflict-specs",
            )
            .unwrap(),
        ),
        PreviousImportMetas: vec![ImportStepMeta {
            SortedDataMeta: Some(SortedKVMeta {
                ConflictInfo: ConflictInfo {
                    Count: 2,
                    Files: vec!["data".into()],
                },
                ..Default::default()
            }),
            SortedIndexMetas: HashMap::from([(
                7,
                SortedKVMeta {
                    ConflictInfo: ConflictInfo {
                        Count: 3,
                        Files: vec!["index".into()],
                    },
                    ..Default::default()
                },
            )]),
            RecordedConflictKVCount: 5,
            ..Default::default()
        }],
        PreviousMergeMetas: vec![MergeSortStepMeta {
            KVGroup: "7".into(),
            RecordedConflictKVCount: 4,
            SortedKVMeta: SortedKVMeta {
                ConflictInfo: ConflictInfo {
                    Count: 4,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }],
        PreviousWriteMetas: vec![WriteIngestStepMeta {
            KVGroup: "data".into(),
            RecordedConflictKVCount: 1,
            SortedKVMeta: SortedKVMeta {
                ConflictInfo: ConflictInfo {
                    Count: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut plan = LogicalPlan::default();
    let collect = generateCollectConflictsSpecs(&ctx, &mut plan).unwrap();
    assert_eq!(collect.len(), 1);
    assert_eq!(plan.summary.RowCnt, 10);
    let meta = collect[0]
        .as_any()
        .downcast_ref::<crate::CollectConflictsSpec>()
        .unwrap();
    assert_eq!(meta.CollectConflictsStepMeta.RecordedDataKVConflicts, 3);
    assert_eq!(
        meta.CollectConflictsStepMeta.Infos.ConflictInfos["7"].Count,
        7
    );
    let resolution = generateConflictResolutionSpecs(&ctx, &mut plan).unwrap();
    assert_eq!(resolution.len(), 1);
    assert_eq!(plan.summary.RowCnt, 10);
    let empty_ctx = PlanCtx {
        ObjectStore: ctx.ObjectStore.clone(),
        ..Default::default()
    };
    assert!(
        generateCollectConflictsSpecs(&empty_ctx, &mut plan)
            .unwrap()
            .is_empty()
    );
    assert_eq!(plan.summary.RowCnt, 0);
}

#[test]
fn ingest_uses_merged_data_and_unmerged_index_groups() {
    use crate::{
        ImportStepMeta, LogicalPlan, MergeSortStepMeta, MultipleFilesStat, PlanCtx, SortedKVMeta,
        WriteIngestSpec, generateWriteIngestSpecs,
    };
    use astersql_ingestor_globalsort as globalsort;
    use astersql_objstore as objstore;
    use std::collections::HashMap;
    let storage_ctx = objstore::storage::Context::background();
    let store =
        objstore::storage::NewFromURL(&storage_ctx, "memstore://planner-ingest-groups").unwrap();
    for file in ["merged-data", "encode-index"] {
        store
            .WriteFile(
                &storage_ctx,
                file,
                &globalsort::encode_kvs(&[globalsort::KvPair {
                    key: b"a".to_vec(),
                    value: vec![1],
                }]),
            )
            .unwrap();
        let prop = astersql_ingestor_simplesst::codec::RangeProperty {
            FirstKey: b"a".to_vec(),
            LastKey: b"a".to_vec(),
            Size: 2,
            Keys: 1,
            Offset: 0,
        };
        store
            .WriteFile(
                &storage_ctx,
                &format!("{file}.stat"),
                &astersql_ingestor_simplesst::codec::encode_multi_props(&[prop]).unwrap(),
            )
            .unwrap();
    }
    let sorted = |file: &str, overlap| SortedKVMeta {
        StartKey: b"a".to_vec(),
        EndKey: b"z".to_vec(),
        MultipleFilesStats: vec![MultipleFilesStat {
            Filenames: vec![[file.into(), format!("{file}.stat")]],
            MaxOverlappingNum: overlap,
            MinKey: b"a".to_vec(),
            MaxKey: b"z".to_vec(),
        }],
        ..Default::default()
    };
    let ctx = PlanCtx {
        PreviousImportMetas: vec![ImportStepMeta {
            SortedDataMeta: Some(sorted("old-data", 5000)),
            SortedIndexMetas: HashMap::from([(7, sorted("encode-index", 1))]),
            ..Default::default()
        }],
        PreviousMergeMetas: vec![MergeSortStepMeta {
            KVGroup: "data".into(),
            SortedKVMeta: sorted("merged-data", 1),
            ..Default::default()
        }],
        ObjectStore: Some(store),
        StorageContext: storage_ctx,
        CommitTS: Some(123),
        ThreadCnt: 16,
        ..Default::default()
    };
    let specs = generateWriteIngestSpecs(&ctx, &mut LogicalPlan::default()).unwrap();
    assert_eq!(specs.len(), 2);
    let mut groups: Vec<_> = specs
        .iter()
        .map(|spec| {
            let meta = &spec
                .as_any()
                .downcast_ref::<WriteIngestSpec>()
                .unwrap()
                .WriteIngestStepMeta;
            assert_eq!(meta.TS, 123);
            (meta.KVGroup.clone(), meta.DataFiles[0].clone())
        })
        .collect();
    groups.sort();
    assert_eq!(
        groups,
        [
            ("7".into(), "encode-index".into()),
            ("data".into(), "merged-data".into())
        ]
    );
}

/// 校验 totalConflicts 对各 kv group 计数做饱和求和，与 Go 行为一致。
#[test]
fn planner_saturating_conflict_total_matches_go() {
    let mut infos = KVGroupConflictInfos::default();
    infos.ConflictInfos.insert(
        "data".to_string(),
        ConflictInfo {
            Count: 12,
            ..ConflictInfo::default()
        },
    );
    infos.ConflictInfos.insert(
        "7".to_string(),
        ConflictInfo {
            Count: 5,
            ..ConflictInfo::default()
        },
    );
    assert_eq!(totalConflicts(&infos), 17);
}
