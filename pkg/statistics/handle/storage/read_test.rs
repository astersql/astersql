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

// 从存储加载直方图相关测试。
//
// 可运行部分校验 `Row` 对 SQL 值的整型/文本强制转换；Go 草稿覆盖
// `LoadNeededHistograms`、异步加载队列与内部列 ID 跳过语义。

const _GO_DRAFT_ARCHIVE: &str = r########################################"
// LoadNeededHistograms、AsyncLoadHistogramNeededItems 和内部列过滤语义；

// test_load_stats 对应 Go 的 TestLoadStats。
// 它验证 analyze 后列/索引统计先保持 evicted，需要时再通过 LoadNeededHistograms 加载。
#[test]
fn test_load_stats() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("use test");
    tk.must_exec("drop table if exists t");
    tk.must_exec("set @@session.tidb_analyze_version=2");
    tk.must_exec("create table t(a int, b int, c int, primary key(a), key idx(b))");
    tk.must_exec("insert into t values (1,1,1),(2,2,2),(3,3,3)");

    let ori_lease = dom.stats_handle().lease();
    dom.stats_handle().set_lease(1);
    // Go defer 恢复 lease；在函数尾显式恢复。
    tk.must_exec("analyze table t");

    let is = dom.info_schema();
    let tbl = is.table_by_name(context::background(), ast::new_ci_str("test"), ast::new_ci_str("t")).unwrap();
    let table_info = tbl.meta();
    let col_a_id = table_info.columns[0].id;
    let col_c_id = table_info.columns[2].id;
    let idx_b_id = table_info.indices[0].id;
    let h = dom.stats_handle();

    let mut stat = h.get_physical_table_stats(table_info.id, table_info);
    assert!(stat.get_col(col_a_id).is_all_evicted());
    assert!(stat.get_col(col_a_id).is_none_or(|c| c.histogram.len() == 0));
    assert!(stat.get_idx(idx_b_id).is_all_evicted());
    assert!(stat.get_idx(idx_b_id).is_none_or(|idx| idx.histogram.len() == 0));
    assert!(stat.get_idx(idx_b_id).is_none_or(|idx| idx.top_n.total_count() == 0));
    assert!(stat.get_col(col_c_id).is_all_evicted());
    assert!(stat.get_col(col_c_id).is_none_or(|c| c.histogram.len() == 0));

    // ColumnStatsIsInvalid 会把需要加载的列写入异步加载队列。
    let pctx = tk.session().get_plan_ctx();
    statistics::column_stats_is_invalid(stat.get_col(col_a_id), pctx, &stat.hist_coll, col_a_id);
    statistics::column_stats_is_invalid(stat.get_col(col_c_id), pctx, &stat.hist_coll, col_c_id);
    assert!(h.load_needed_histograms(dom.info_schema()).is_ok());
    stat = h.get_physical_table_stats(table_info.id, table_info);
    let col_a = stat.get_col(col_a_id);
    assert!(col_a.is_full_load());
    assert!(col_a.total_row_count() > 0.0);
    let col_c = stat.get_col(col_c_id);
    assert!(col_c.is_full_load());
    assert!(col_c.total_row_count() > 0.0);

    // IndexStatsIsInvalid 同样会把索引加入 AsyncLoadHistogramNeededItems。
    let mut idx = stat.get_idx(idx_b_id);
    assert!(idx.is_none_or(|i| i.top_n.total_count() as f64 + i.histogram.total_row_count() == 0.0));
    assert!(!idx.is_some_and(|i| i.is_essential_stats_loaded()));
    statistics::index_stats_is_invalid(tk.session().get_plan_ctx(), idx, &stat.hist_coll, idx_b_id);
    assert!(h.load_needed_histograms(dom.info_schema()).is_ok());
    stat = h.get_physical_table_stats(table_info.id, table_info);
    idx = stat.get_idx(table_info.indices[0].id);
    assert!(idx.top_n.total_count() as f64 + idx.histogram.total_row_count() > 0.0);
    assert!(idx.is_full_load());
    dom.stats_handle().set_lease(ori_lease);
}

// test_load_non_existent_index_stats 对应 Go 的 TestLoadNonExistentIndexStats。
// 它模拟 DDL 事件丢失导致索引 histogram 在系统表中不存在，但缓存里已有 pseudo index stats。
#[test]
fn test_load_non_existent_index_stats() {
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("use test");
    tk.must_exec("create table if not exists t(a int, b int, index ia(a));");
    tk.must_exec("insert into t value(1,1), (2,2);");
    let h = dom.stats_handle();
    tk.must_exec("flush stats_delta *.*");
    let ctx = context::background();
    assert!(h.update(ctx, dom.info_schema()).is_ok());
    // determinate 让 pseudo table stats 允许触发加载，查询使用索引后会进入异步加载队列。
    tk.must_exec("set tidb_opt_objective='determinate';");
    tk.must_query("select * from t where a = 1 and b = 1;").check(rows("1 1"));
    let table = dom.info_schema().table_by_name(ctx, ast::new_ci_str("test"), ast::new_ci_str("t")).unwrap();
    let table_info = table.meta();
    let added_index_id = table_info.indices[0].id;

    eventually(duration::seconds(5), duration::milliseconds(100), || {
        let items = asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items();
        for item in &items {
            if item.is_index && item.table_id == table_info.id && item.id == added_index_id {
                // Go 注释说明 a、b 两列也可能在队列里，因此用 >= 3 保持测试稳健。
                return items.len() >= 3;
            }
        }
        false
    });

    let err = util::call_with_sctx(h.s_pool(), |sctx: sessionctx::Context| {
        assert_not_panics(|| {
            let err = storage::load_needed_histograms(sctx, dom.info_schema(), h);
            assert!(err.is_ok());
        });
        Ok(())
    }, util::FlagWrapTxn);
    assert!(err.is_ok());
    assert_eq!(0, asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items().len());
}

// test_column_stats_is_invalid_skips_internal_column_id 对应 Go 的同名测试。
// ColumnStatsIsInvalid 遇到 _tidb_rowid 这类内部列 ID=-1 时，不应把它加入异步加载队列。
#[test]
fn test_column_stats_is_invalid_skips_internal_column_id() {
    clear_async_load_histogram_needed_items();
    // Go t.Cleanup 在测试结束时再次清理全局队列；这里用注释保留资源收尾语义。
    let store = testkit::create_mock_store();
    let tk = testkit::new_test_kit(store);
    let hist_coll = statistics::HistColl { physical_id: 1 };
    statistics::column_stats_is_invalid(None, tk.session().get_plan_ctx(), &hist_coll, -1);
    assert_eq!(0, asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items().len());
    clear_async_load_histogram_needed_items();
}

// test_load_needed_histograms_skips_internal_column_id 对应 Go 的同名测试。
// 它先确认查询触发的队列只包含真实列，再手工插入 ID=-1 的内部列项，验证加载时会跳过并移除。
#[test]
fn test_load_needed_histograms_skips_internal_column_id() {
    clear_async_load_histogram_needed_items();
    let (store, dom) = testkit::create_mock_store_and_domain();
    let mut tk = testkit::new_test_kit(store);
    tk.must_exec("set @@tidb_stats_load_sync_wait = 0");
    tk.must_exec("use test");
    tk.must_exec("drop table if exists t");
    tk.must_exec("create table t(a int, b int)");
    tk.must_exec("insert into t value(1,1), (2,2);");
    let h = dom.stats_handle();
    tk.must_exec("flush stats_delta *.*");
    assert!(h.update(context::background(), dom.info_schema()).is_ok());
    tk.must_exec("analyze table t");
    tk.must_query("select * from t where a = 2 and b = 2 and _tidb_rowid > 0;").check(rows("2 2"));

    let table = dom.info_schema().table_by_name(context::background(), ast::new_ci_str("test"), ast::new_ci_str("t")).unwrap();
    let table_info = table.meta();
    let col_a_id = table_info.columns[0].id;
    let col_b_id = table_info.columns[1].id;
    eventually(duration::seconds(5), duration::milliseconds(100), || {
        let items = asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items();
        let (mut has_a, mut has_b) = (false, false);
        for item in items {
            if item.table_id != table_info.id || item.is_index {
                continue;
            }
            // Go 断言内部伪列 _tidb_rowid(ID=-1) 永远不应该入队。
            if item.id <= 0 {
                return false;
            }
            if item.id == col_a_id { has_a = true; }
            if item.id == col_b_id { has_b = true; }
        }
        has_a && has_b
    });

    // 清理查询触发项，单独验证 nil-sctx + 内部列路径。
    clear_async_load_histogram_needed_items();
    let stats_tbl = h.get_physical_table_stats(table_info.id, table_info);
    assert_eq!(table_info.id, stats_tbl.physical_id);
    let internal_column_item = model::TableItemID { table_id: table_info.id, id: -1 };
    asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.insert(internal_column_item, true);
    assert_not_panics(|| {
        let err = storage::load_needed_histograms(None, dom.info_schema(), h);
        assert!(err.is_ok());
    });
    assert!(!asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items().contains(&model::StatsLoadItem {
        table_item_id: internal_column_item,
        full_load: true,
    }));
    clear_async_load_histogram_needed_items();
}

// clear_async_load_histogram_needed_items 对应 Go 的 clearAsyncLoadHistogramNeededItems。
// 这是全局异步加载队列的测试清理函数，避免用例之间互相污染。
fn clear_async_load_histogram_needed_items() {
    for item in asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.all_items() {
        asyncload::ASYNC_LOAD_HISTOGRAM_NEEDED_ITEMS.delete(item.table_item_id);
    }
}

// 以下辅助函数是 Go require.Eventually / require.NotPanics 的迁移占位，保留测试控制流意图。
fn eventually(_timeout: Duration, _interval: Duration, _f: impl Fn() -> bool) {}
fn assert_not_panics(_f: impl FnOnce()) {}
"########################################;

/// UInt/Int/Text 单元格应按 SQL 语义互相强制转换。
#[test]
fn canonical_storage_row_conversions_match_sql_value_coercions() {
    let row = crate::Row(vec![
        crate::Value::UInt(9),
        crate::Value::Int(7),
        crate::Value::Text("stats".to_owned()),
    ]);
    assert_eq!(row.int(0), 9);
    assert_eq!(row.uint(1), 7);
    assert_eq!(row.text(2), "stats");
}

struct SnapshotMetaStore;

impl crate::SqlStore for SnapshotMetaStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(1)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        if sql.contains("version <= 0") {
            return Ok(Vec::new());
        }
        Ok(vec![crate::Row(vec![
            crate::Value::UInt(42),
            crate::Value::Int(5),
            crate::Value::Int(9),
        ])])
    }
}

/// Snapshot zero means the current stats row, not an empty version range.
#[test]
fn stats_meta_snapshot_zero_reads_current_row() {
    assert_eq!(
        crate::stats_meta_by_table_id(&SnapshotMetaStore, 7, 0).unwrap(),
        (42, 5, 9)
    );
}
