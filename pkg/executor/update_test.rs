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
// Update 执行器辅助逻辑的单元测试。
//
// 覆盖重复键检查模式选择、外连接未匹配行判定，以及 UpdateRuntimeStats 的合并与格式化。

use crate::update::{
    UpdateDupKeyCheckMode, UpdateExec, UpdateRuntime, UpdateRuntimeStats,
    optimizeDupKeyCheckForUpdate, unmatchedOuterRow,
};
use std::time::Duration;
#[test]
/// 验证 Lazy/InPlace 选择、unmatchedOuterRow 与运行时统计 Merge/String。
fn update_modes_and_runtime_stats_cover_lazy_and_merge_paths() {
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, false, false),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(true, false, false),
        UpdateDupKeyCheckMode::Lazy
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, false, true),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(true, false, true),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, true, true),
        UpdateDupKeyCheckMode::Lazy
    );
    assert!(unmatchedOuterRow(true));
    let mut stats = UpdateRuntimeStats {
        fetch: Duration::from_secs(1),
        compose: Duration::from_secs(2),
        check_and_update: Duration::from_secs(3),
    };
    stats.Merge(&stats.Clone());
    assert_eq!(stats.fetch, Duration::from_secs(2));
    assert!(stats.String().contains("check-and-update:6s"));
}

#[derive(Default)]
struct WriteRuntime {
    drained: bool,
    resets: usize,
    write_rows: Vec<usize>,
}

impl UpdateRuntime for WriteRuntime {
    type Context = ();
    type Request = ();
    type Row = ();
    type Schema = ();
    type ForeignKeyCheck = ();
    type ForeignKeyCascade = ();
    type Error = &'static str;

    fn prepare_row(&mut self, _: &Self::Row) -> Result<(), Self::Error> {
        Ok(())
    }
    fn merge_non_generated(&mut self, _: &Self::Row, _: &mut Self::Row) -> Result<(), Self::Error> {
        Ok(())
    }
    fn merge_generated(
        &mut self,
        _: &Self::Row,
        _: &mut Self::Row,
        _: usize,
        _: bool,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn execute_prepared_row(
        &mut self,
        _: &mut Self::Context,
        _: &Self::Schema,
        _: usize,
        _: Self::Row,
        _: Self::Row,
        _: UpdateDupKeyCheckMode,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn reset_request(&self, _: &mut Self::Request) {}
    fn drained(&self) -> bool {
        self.drained
    }
    fn set_drained(&mut self, drained: bool) {
        self.drained = drained;
    }
    fn update_rows(&mut self, _: &mut Self::Context) -> Result<(usize, i64), Self::Error> {
        Ok((3, 7))
    }
    fn update_rows_column_multiply_for_prepared_row(&self) -> i64 {
        0
    }
    fn handle_update_error(&mut self, _: usize, error: Self::Error) -> Self::Error {
        error
    }
    fn fast_compose_new_row(&mut self, _: usize, _: &Self::Row) -> Result<Self::Row, Self::Error> {
        Ok(())
    }
    fn compose_new_row(&mut self, _: usize, _: &Self::Row) -> Result<Self::Row, Self::Error> {
        Ok(())
    }
    fn record_rows_column_multiply(&mut self, _: i64) {}
    fn reset_write_runtime_stats(&mut self) {
        self.resets += 1;
    }
    fn record_write_cpu_work(&mut self, rows: usize) {
        self.write_rows.push(rows);
    }
    fn set_message(&mut self) {}
    fn register_runtime_stats(&mut self) {}
    fn collect_runtime_stats_enabled(&self) -> bool {
        true
    }
    fn reset_memory_usage(&mut self) {}
    fn close_child(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn open_child(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn initialize_evaluation_buffer(&mut self) {}
    fn all_assignments_are_constant(&self) -> bool {
        true
    }
    fn foreign_key_checks(&self) -> Vec<&Self::ForeignKeyCheck> {
        Vec::new()
    }
    fn foreign_key_cascades(&self) -> Vec<&Self::ForeignKeyCascade> {
        Vec::new()
    }
}

#[test]
fn update_resets_write_stats_and_counts_first_matched_rows() {
    let mut executor = UpdateExec {
        runtime: WriteRuntime::default(),
    };

    executor.Open(&mut ()).unwrap();
    executor.Next(&mut (), &mut ()).unwrap();
    executor.Next(&mut (), &mut ()).unwrap();

    assert_eq!(executor.runtime.resets, 1);
    assert_eq!(executor.runtime.write_rows, vec![3]);
}
