// Copyright 2026 AsterSQL.

// `interfaces` 类型默认值与字段语义单元测试。

use super::*;
use std::collections::HashMap;
use std::time::Duration;

#[test]
/// 默认 UpdateOptions 不跳过版本前移（SkipMoveForward=false）。
fn update_options_default_preserves_version_progression() {
    assert!(!UpdateOptions::default().SkipMoveForward);
}

#[test]
/// StatsLockTable 保留分区名映射与全限定表名。
fn stats_lock_table_keeps_partition_names_and_full_name() {
    let lock = StatsLockTable {
        PartitionInfo: HashMap::from([(7, "p0".to_owned())]),
        FullName: "db.table".to_owned(),
    };
    assert_eq!(lock.PartitionInfo.get(&7).map(String::as_str), Some("p0"));
    assert_eq!(lock.FullName, "db.table");
}

#[test]
/// GlobalStatsInfo 可区分列/索引合并输入并携带直方图 ID 列表。
fn global_stats_info_distinguishes_column_and_index_inputs() {
    let info = GlobalStatsInfo {
        HistIDs: vec![1, 2],
        IsIndex: 0,
        StatsVersion: 2,
    };
    assert_eq!(info.HistIDs, [1, 2]);
    assert_eq!(info.IsIndex, 0);
    assert_eq!(info.StatsVersion, 2);
}

#[test]
fn public_dtos_use_the_same_package_types_as_go() {
    let column_time = ColStatsTimeInfo::default();
    let _: Option<aster_sql_types::time::Time> = column_time.LastUsedAt;
    let _: Option<aster_sql_types::time::Time> = column_time.LastAnalyzedAt;

    let task = PartitionStatisticLoadTask {
        JSONTable: None,
        PhysicalID: 42,
    };
    let _: Option<&aster_sql_statistics_util::JSONTable> = task.JSONTable.as_deref();

    fn assert_needed_item_result(
        task: NeededItemTask,
    ) -> std::sync::mpsc::Sender<aster_sql_sessionctx_stmtctx::StatsLoadResult> {
        task.ResultCh
    }
    let _ = assert_needed_item_result;
}

#[test]
fn public_traits_accept_the_real_dependency_contracts() {
    fn assert_gc<T: StatsGC + ?Sized>(gc: &T, info_schema: &dyn aster_sql_infoschema::InfoSchema) {
        let _ = gc.GCStats(info_schema, Duration::ZERO);
    }

    fn assert_index_usage<T: IndexUsage + ?Sized>(usage: &T) {
        let _: aster_sql_statistics_handle_usage_indexusage::SessionIndexUsageCollector =
            usage.NewSessionIndexUsageCollector();
        let _: aster_sql_statistics_handle_usage_indexusage::Sample = usage.GetIndexUsage(1, 2);
    }

    fn assert_handle_util_supertraits<T: StatsHandle + ?Sized>() {
        fn require<
            U: aster_sql_statistics_handle_util::Pool
                + aster_sql_statistics_handle_util::AutoAnalyzeProcIdGenerator
                + aster_sql_statistics_handle_util::LeaseGetter
                + aster_sql_statistics_handle_util::TableInfoGetter
                + ?Sized,
        >() {
        }
        require::<T>();
    }

    fn assert_analyze_accepts_any_error<T: StatsAnalyze + ?Sized>(
        analyze: &T,
        job: &aster_sql_statistics::AnalyzeJob,
        analyze_type: aster_sql_statistics::JobType,
        error: &std::io::Error,
    ) {
        analyze.FinishAnalyzeJob(job, Some(error), analyze_type);
    }

    let _ = assert_gc::<dyn StatsGC>;
    let _ = assert_index_usage::<dyn IndexUsage>;
    let _ = assert_handle_util_supertraits::<dyn StatsHandle>;
    let _ = assert_analyze_accepts_any_error::<dyn StatsAnalyze>;
}

#[test]
fn context_and_callback_signatures_preserve_go_cancellation_and_nil_json() {
    fn assert_cache_update<T: StatsCache + ?Sized>(
        cache: &T,
        context: &ExecutionContext,
        info_schema: &dyn aster_sql_infoschema::InfoSchema,
    ) {
        let _ = cache.Update(context, info_schema, &[1, 2]);
    }

    fn assert_sync_load<T: StatsSyncLoad + ?Sized>(
        loader: &T,
        statement_context: &mut aster_sql_sessionctx_stmtctx::StatementContext,
        wait_group: &aster_sql_util::wait_group_wrapper::WaitGroupEnhancedWrapper,
        exit: std::sync::mpsc::Receiver<()>,
    ) {
        let _ = loader.SendLoadRequests(statement_context, &[], Duration::ZERO);
        let _ = loader.SyncWaitStatsLoad(statement_context);
        loader.SubLoadWorker(exit, wait_group);
    }

    let _ = assert_cache_update::<dyn StatsCache>;
    let _ = assert_sync_load::<dyn StatsSyncLoad>;

    let context = ExecutionContext::new();
    let saw_nil = std::sync::atomic::AtomicBool::new(false);
    let persist = |_: &ExecutionContext,
                   json_table: Option<&aster_sql_statistics_util::JSONTable>,
                   physical_id: i64| {
        saw_nil.store(
            json_table.is_none() && physical_id == 42,
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(())
    };
    let persist: &PersistFunc<'_> = &persist;
    persist(&context, None, 42).expect("nil JSON callback must remain valid");
    assert!(saw_nil.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn priority_queue_json_field_names_match_go_tags() {
    let snapshot = PriorityQueueSnapshot {
        CurrentJobs: vec![AnalysisJobJSON {
            Type: "dynamic".to_owned(),
            TableID: 7,
            Weight: 1.5,
            PartitionIDs: vec![8],
            IndexIDs: vec![9],
            PartitionIndexIDs: HashMap::from([(8, vec![9])]),
            Indicators: IndicatorsJSON {
                ChangePercentage: "10%".to_owned(),
                TableSize: "1 MiB".to_owned(),
                LastAnalysisDuration: "1h".to_owned(),
            },
            HasNewlyAddedIndex: true,
        }],
        MustRetryTables: vec![11],
    };

    let value = serde_json::to_value(snapshot).expect("priority queue snapshot must serialize");
    assert_eq!(value["must_retry_tables"], serde_json::json!([11]));
    assert_eq!(value["current_jobs"][0]["type"], "dynamic");
    assert_eq!(value["current_jobs"][0]["table_id"], 7);
    assert_eq!(
        value["current_jobs"][0]["partition_index_ids"]["8"],
        serde_json::json!([9])
    );
    assert_eq!(
        value["current_jobs"][0]["indicators"]["change_percentage"],
        "10%"
    );
    assert_eq!(value["current_jobs"][0]["has_newly_added_index"], true);
}
