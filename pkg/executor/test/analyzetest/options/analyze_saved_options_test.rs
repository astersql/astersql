// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! `analyze_saved_options_test.go` 中 ANALYZE 选项持久化语义的行为测试。
//!
//! 本测试 crate 刻意不依赖数据库，因此用下方的小型状态机模拟
//! `mysql.analyze_options` 的持久化记录和逐列统计信息刷新版本。模型保留 Go 用例的
//! 关键语义：语句选项与已保存选项合并、分区选项覆盖表选项、自动分析复用已保存的
//! 列选择，并且关闭持久化后既不读取也不写入保存记录。

#![allow(non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// ANALYZE 选择待分析列的方式。
enum ColumnChoice {
    All,
    List,
    Predicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 对应一条持久化 ANALYZE 选项记录。
struct AnalyzeOptions {
    sample_rate: u32,
    buckets: u32,
    topn: i32,
    choice: ColumnChoice,
    column_ids: Vec<u32>,
}

impl AnalyzeOptions {
    /// 生成未显式指定任何选项时的初始记录。
    fn defaults() -> Self {
        Self {
            sample_rate: 0,
            buckets: 0,
            topn: -1,
            choice: ColumnChoice::All,
            column_ids: Vec::new(),
        }
    }

    /// 删除已不在表结构中的列，模拟 DDL 后对持久化列 ID 的清理。
    fn filtered_to(&self, columns: &[u32]) -> Self {
        let valid = columns.iter().copied().collect::<BTreeSet<_>>();
        let mut result = self.clone();
        result.column_ids.retain(|id| valid.contains(id));
        result
    }
}

#[derive(Default)]
/// 单次 ANALYZE 语句显式携带的选项；`None` 表示应从保存记录或默认值继承。
struct AnalyzeRequest {
    sample_rate: Option<u32>,
    buckets: Option<u32>,
    topn: Option<i32>,
    choice: Option<ColumnChoice>,
    column_ids: Option<Vec<u32>>,
}

impl AnalyzeRequest {
    fn with_buckets_topn(buckets: u32, topn: i32) -> Self {
        Self {
            buckets: Some(buckets),
            topn: Some(topn),
            ..Self::default()
        }
    }

    fn columns(column_ids: &[u32]) -> Self {
        Self {
            choice: Some(ColumnChoice::List),
            column_ids: Some(column_ids.to_vec()),
            ..Self::default()
        }
    }

    fn predicate() -> Self {
        Self {
            choice: Some(ColumnChoice::Predicate),
            ..Self::default()
        }
    }

    fn all_columns() -> Self {
        Self {
            choice: Some(ColumnChoice::All),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 测试断言所需的最小列统计信息。
struct ColumnStats {
    buckets: u32,
    last_update_version: u64,
}

#[derive(Clone, Debug)]
/// 一个物理表或分区各自保存的选项与列统计信息。
struct PhysicalTable {
    options: Option<AnalyzeOptions>,
    stats: BTreeMap<u32, ColumnStats>,
}

impl PhysicalTable {
    fn new(columns: &[u32]) -> Self {
        Self {
            options: None,
            stats: columns
                .iter()
                .copied()
                .map(|id| (id, ColumnStats::default()))
                .collect(),
        }
    }
}

/// 表级状态机；空字符串键表示逻辑表，其余键表示各物理分区。
struct AnalyzeTable {
    columns: Vec<u32>,
    table_options: Option<AnalyzeOptions>,
    physical_tables: BTreeMap<String, PhysicalTable>,
    predicate_columns: BTreeSet<u32>,
    version: u64,
    persist_options: bool,
}

impl AnalyzeTable {
    fn new(columns: &[u32], partitions: &[&str]) -> Self {
        let mut physical_tables = BTreeMap::new();
        physical_tables.insert(String::new(), PhysicalTable::new(columns));
        for partition in partitions {
            physical_tables.insert((*partition).to_owned(), PhysicalTable::new(columns));
        }
        Self {
            columns: columns.to_vec(),
            table_options: if partitions.is_empty() {
                None
            } else {
                Some(AnalyzeOptions::defaults())
            },
            physical_tables,
            predicate_columns: BTreeSet::new(),
            version: 0,
            persist_options: true,
        }
    }

    fn add_partition(&mut self, name: &str) {
        self.physical_tables
            .insert(name.to_owned(), PhysicalTable::new(&self.columns));
    }

    fn set_persist_options(&mut self, enabled: bool) {
        self.persist_options = enabled;
    }

    fn record_predicate_column(&mut self, column_id: u32) {
        self.predicate_columns.insert(column_id);
    }

    /// 按“语句选项 > 保存记录 > 默认值”的优先级解析最终选项。
    ///
    /// LIST 模式会合并已保存和本次指定的列，再过滤已被 DDL 删除的列。
    fn resolve(&self, saved: Option<&AnalyzeOptions>, request: &AnalyzeRequest) -> AnalyzeOptions {
        let defaults = AnalyzeOptions::defaults();
        let base = saved.unwrap_or(&defaults);
        let mut result = AnalyzeOptions {
            sample_rate: request.sample_rate.unwrap_or(base.sample_rate),
            buckets: request.buckets.unwrap_or(base.buckets),
            topn: request.topn.unwrap_or(base.topn),
            choice: request.choice.unwrap_or(base.choice),
            column_ids: base.column_ids.clone(),
        };

        if let Some(column_ids) = &request.column_ids {
            result.choice = ColumnChoice::List;
            result.column_ids = if base.choice == ColumnChoice::List {
                base.column_ids
                    .iter()
                    .chain(column_ids.iter())
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            } else {
                column_ids.clone()
            };
        } else if matches!(result.choice, ColumnChoice::All | ColumnChoice::Predicate) {
            result.column_ids.clear();
        }
        result.filtered_to(&self.columns)
    }

    fn target_names(&self, target: Option<&str>) -> Vec<String> {
        match target {
            Some(name) => vec![name.to_owned()],
            None => self.physical_tables.keys().cloned().collect(),
        }
    }

    /// 执行一次模型化分析，并更新目标物理表的选项记录和逐列刷新版本。
    fn analyze(&mut self, target: Option<&str>, request: AnalyzeRequest) {
        let target_names = self.target_names(target);
        assert!(!target_names.is_empty(), "analyze target must exist");
        self.version += 1;
        let version = self.version;

        for name in target_names {
            let physical = self
                .physical_tables
                .get(&name)
                .unwrap_or_else(|| panic!("unknown analyze target {name}"));
            let saved = if name.is_empty() {
                self.table_options.clone()
            } else {
                physical
                    .options
                    .clone()
                    .or_else(|| self.table_options.clone())
            };
            let resolved = if self.persist_options {
                self.resolve(saved.as_ref(), &request)
            } else {
                self.resolve(None, &request)
            };
            let analyzed_columns = match request.column_ids.as_ref() {
                Some(ids) => ids.clone(),
                None => match resolved.choice {
                    ColumnChoice::All => self.columns.clone(),
                    ColumnChoice::List => resolved.column_ids.clone(),
                    ColumnChoice::Predicate => self.predicate_columns.iter().copied().collect(),
                },
            };

            if self.persist_options {
                if name.is_empty() {
                    self.table_options = Some(resolved.clone());
                } else if let Some(physical) = self.physical_tables.get_mut(&name) {
                    physical.options = Some(resolved.clone());
                }
            }

            let physical = self
                .physical_tables
                .get_mut(&name)
                .unwrap_or_else(|| panic!("unknown analyze target {name}"));
            for column_id in analyzed_columns {
                if let Some(stats) = physical.stats.get_mut(&column_id) {
                    stats.buckets = resolved.buckets;
                    stats.last_update_version = version;
                }
            }

            // 分析逻辑表时，表级选项会下发给没有独立记录的分区；已有分区记录中的
            // 显式列选择仍会保留，这与 Go 用例中的删列场景一致。
            if name.is_empty() && !self.physical_tables.is_empty() {
                let partition_names = self
                    .physical_tables
                    .keys()
                    .filter(|partition_name| !partition_name.is_empty())
                    .cloned()
                    .collect::<Vec<_>>();
                for partition_name in partition_names {
                    if !self.persist_options {
                        continue;
                    }
                    let partition_saved = self
                        .physical_tables
                        .get(&partition_name)
                        .and_then(|partition| partition.options.clone())
                        .or_else(|| self.table_options.clone());
                    let partition_options = self.resolve(partition_saved.as_ref(), &request);
                    self.physical_tables
                        .get_mut(&partition_name)
                        .expect("partition collected above")
                        .options = Some(partition_options);
                }
            }
        }
    }

    fn stat(&self, target: &str, column_id: u32) -> ColumnStats {
        self.physical_tables
            .get(target)
            .and_then(|table| table.stats.get(&column_id))
            .copied()
            .unwrap_or_default()
    }

    fn options(&self, target: &str) -> Option<&AnalyzeOptions> {
        if target.is_empty() {
            self.table_options.as_ref()
        } else {
            self.physical_tables
                .get(target)
                .and_then(|physical| physical.options.as_ref())
        }
    }

    fn drop_column(&mut self, column_id: u32) {
        self.columns.retain(|id| *id != column_id);
        for physical in self.physical_tables.values_mut() {
            physical.stats.remove(&column_id);
            if let Some(options) = &mut physical.options {
                options.column_ids.retain(|id| *id != column_id);
            }
        }
        if let Some(options) = &mut self.table_options {
            options.column_ids.retain(|id| *id != column_id);
        }
    }

    fn drop_partition(&mut self, name: &str) {
        self.physical_tables.remove(name);
    }

    fn drop_table(&mut self) {
        self.physical_tables.retain(|name, _| name.is_empty());
    }
}

#[test]
fn TestSavedAnalyzeOptions() {
    let mut table = AnalyzeTable::new(&[1, 2, 3], &[]);
    table.analyze(None, AnalyzeRequest::with_buckets_topn(2, 1));
    assert_eq!(table.stat("", 1).buckets, 2);
    assert_eq!(table.stat("", 2).buckets, 2);
    let options = table.options("").unwrap();
    assert_eq!(options.sample_rate, 0);
    assert_eq!(options.topn, 1);

    // 行数变化触发自动分析时复用表级保存选项。
    let previous_version = table.stat("", 1).last_update_version;
    table.analyze(None, AnalyzeRequest::default());
    assert!(table.stat("", 1).last_update_version > previous_version);
    assert_eq!(table.stat("", 1).buckets, 2);

    // 显式指定列时，新语句选项与已保存的 topn 合并。
    let previous_version = table.stat("", 1).last_update_version;
    let mut request = AnalyzeRequest::columns(&[1, 2]);
    request.sample_rate = Some(1);
    request.buckets = Some(3);
    table.analyze(None, request);
    assert!(table.stat("", 1).last_update_version > previous_version);
    assert_eq!(table.stat("", 1).buckets, 3);
    assert_eq!(table.stat("", 2).buckets, 3);
    assert_eq!(table.stat("", 3).buckets, 2);
    let options = table.options("").unwrap();
    assert_eq!(
        (options.sample_rate, options.buckets, options.topn),
        (1, 3, 1)
    );
    assert_eq!(options.column_ids, vec![1, 2]);

    // 关闭持久化后，既不读取旧记录，也不以本次选项覆盖它。
    table.set_persist_options(false);
    table.analyze(
        None,
        AnalyzeRequest {
            topn: Some(2),
            ..AnalyzeRequest::default()
        },
    );
    assert_ne!(table.stat("", 1).buckets, 3);
    assert_ne!(table.options("").unwrap().topn, 2);
}

#[test]
fn TestSavedPartitionAnalyzeOptions() {
    let mut table = AnalyzeTable::new(&[1, 2, 3], &["p0", "p1"]);

    // 只分析一个分区时，仅为该分区建立独立记录。
    table.analyze(Some("p0"), AnalyzeRequest::with_buckets_topn(3, 1));
    assert_eq!(table.stat("p0", 1).buckets, 3);
    assert_eq!(table.options("p0").unwrap().sample_rate, 0);
    assert_eq!(table.options("p0").unwrap().topn, 1);
    assert_eq!(table.options("").unwrap().buckets, 0);

    // 表级分析将选项合并到每个分区，并保留列清单。
    table.analyze(
        None,
        AnalyzeRequest {
            buckets: Some(2),
            topn: Some(0),
            column_ids: Some(vec![1, 2]),
            choice: Some(ColumnChoice::List),
            ..AnalyzeRequest::default()
        },
    );
    assert_eq!(table.stat("p0", 1).buckets, 2);
    assert_eq!(table.stat("p1", 1).buckets, 2);
    assert!(table.stat("p0", 3).last_update_version < table.stat("p0", 1).last_update_version);
    for target in ["", "p0", "p1"] {
        let options = table.options(target).unwrap();
        assert_eq!(options.sample_rate, 0);
        assert_eq!((options.buckets, options.topn), (2, 0));
        assert_eq!(options.column_ids, vec![1, 2]);
    }

    // 分区可以扩展自身保存的列清单，而不影响 p0。
    let p0_version = table.stat("p0", 1).last_update_version;
    table.analyze(
        Some("p1"),
        AnalyzeRequest {
            buckets: Some(1),
            column_ids: Some(vec![1, 3]),
            choice: Some(ColumnChoice::List),
            ..AnalyzeRequest::default()
        },
    );
    assert_eq!(table.stat("p0", 1).last_update_version, p0_version);
    assert_eq!(table.stat("p1", 1).buckets, 1);
    assert_eq!(table.stat("p1", 3).buckets, 1);
    assert_eq!(table.options("p1").unwrap().column_ids, vec![1, 2, 3]);

    // 未给语句选项时复用 p0 的保存记录；显式 buckets 则覆盖对应值。
    let previous_version = table.stat("p0", 1).last_update_version;
    table.analyze(Some("p0"), AnalyzeRequest::default());
    assert!(table.stat("p0", 1).last_update_version > previous_version);
    assert_eq!(table.stat("p0", 1).buckets, 2);
    table.analyze(
        Some("p0"),
        AnalyzeRequest {
            buckets: Some(3),
            ..AnalyzeRequest::default()
        },
    );
    assert_eq!(table.stat("p0", 1).buckets, 3);
    assert_eq!(table.options("p0").unwrap().column_ids, vec![1, 2]);

    // 新增分区以表级记录作为默认选项。
    table.add_partition("p2");
    table.analyze(Some("p2"), AnalyzeRequest::default());
    assert_eq!(table.stat("p2", 1).buckets, 2);
    assert_eq!(table.options("p2").unwrap().column_ids, vec![1, 2]);

    // 删列后，下一次分析前会从每条持久化记录中滤除该列。
    table.drop_column(2);
    table.analyze(None, AnalyzeRequest::default());
    assert_eq!(table.options("").unwrap().column_ids, vec![1]);
    assert_eq!(table.options("p0").unwrap().column_ids, vec![1]);
    assert_eq!(table.options("p1").unwrap().column_ids, vec![1, 3]);

    table.drop_partition("p1");
    assert!(table.options("p1").is_none());
    table.drop_table();
    assert!(table.options("p0").is_none());
}

#[test]
fn TestSavedAnalyzeOptionsForMultipleTables() {
    let mut t1 = AnalyzeTable::new(&[1, 2, 3], &[]);
    let mut t2 = AnalyzeTable::new(&[1, 2, 3], &[]);
    t1.analyze(None, AnalyzeRequest::with_buckets_topn(3, 1));
    t2.analyze(None, AnalyzeRequest::with_buckets_topn(2, 0));

    let mut update_topn = AnalyzeRequest::default();
    update_topn.topn = Some(2);
    t1.analyze(None, update_topn);
    let mut update_topn = AnalyzeRequest::default();
    update_topn.topn = Some(2);
    t2.analyze(None, update_topn);

    assert_eq!(t1.stat("", 1).buckets, 3);
    assert_eq!(t2.stat("", 1).buckets, 2);
    assert_eq!(t1.options("").unwrap().topn, 2);
    assert_eq!(t2.options("").unwrap().topn, 2);
}

#[test]
fn TestSavedAnalyzeColumnOptions() {
    let mut table = AnalyzeTable::new(&[1, 2, 3], &[]);
    table.record_predicate_column(2);
    table.analyze(Some(""), AnalyzeRequest::predicate());
    let predicate_version = table.stat("", 2).last_update_version;
    assert_eq!(table.stat("", 1).last_update_version, 0);
    assert_eq!(table.stat("", 3).last_update_version, 0);
    assert_eq!(table.options("").unwrap().choice, ColumnChoice::Predicate);
    assert!(table.options("").unwrap().column_ids.is_empty());

    // 手动分析和自动分析都会复用保存的 PREDICATE 列选择。
    table.record_predicate_column(3);
    table.analyze(None, AnalyzeRequest::default());
    assert!(table.stat("", 2).last_update_version > predicate_version);
    assert!(table.stat("", 3).last_update_version > predicate_version);
    assert_eq!(table.stat("", 1).last_update_version, 0);

    let mut list_a = AnalyzeRequest::columns(&[1]);
    list_a.buckets = Some(2);
    table.analyze(None, list_a);
    let list_version = table.stat("", 1).last_update_version;
    assert_eq!(table.stat("", 1).last_update_version, list_version);
    assert!(table.stat("", 2).last_update_version < list_version);
    assert!(table.stat("", 3).last_update_version < list_version);
    assert_eq!(table.options("").unwrap().choice, ColumnChoice::List);
    assert_eq!(table.options("").unwrap().column_ids, vec![1]);

    table.analyze(None, AnalyzeRequest::all_columns());
    let all_version = table.stat("", 1).last_update_version;
    assert_eq!(table.stat("", 2).last_update_version, all_version);
    assert_eq!(table.stat("", 3).last_update_version, all_version);
    assert_eq!(table.options("").unwrap().choice, ColumnChoice::All);
    assert!(table.options("").unwrap().column_ids.is_empty());
}
