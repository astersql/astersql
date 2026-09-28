// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 逻辑算子：内存表扫描（LogicalMemTable / MemTableScan）。
//
// 对应 INFORMATION_SCHEMA / 性能相关虚拟表（如 statements_summary、slow_query）的
// 逻辑计划节点。谓词下推（Predicate PushDown）经 Extractor 抽取为表侧过滤提示；
// 列裁剪（PruneColumns）仅对部分可裁剪表生效。统计信息（Stats）使用固定行数估计。

use crate::*;
use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

/// 内存表谓词抽取器：从下推谓词中提取可在表侧应用的过滤条件。
pub trait MemTablePredicateExtractor {
    /// 克隆为新的 trait 对象。
    fn CloneBox(&self) -> Box<dyn MemTablePredicateExtractor>;
    /// 抽取谓词；返回未能下推、需保留在上层的表达式。
    fn Extract(
        &mut self,
        schema: &Schema,
        names: &NameSlice,
        predicates: Vec<Expression>,
    ) -> Vec<Expression>;
    /// 设置行数上限提示（Offset+Count），默认空实现。
    fn SetRowLimitHint(&mut self, _limit: u64) {}
    /// 设置是否按降序读取，默认空实现。
    fn SetDesc(&mut self, _desc: bool) {}
}

impl Clone for Box<dyn MemTablePredicateExtractor> {
    fn clone(&self) -> Self {
        self.CloneBox()
    }
}

/// 逻辑内存表扫描算子：扫描不落盘的系统/诊断虚拟表。
pub struct LogicalMemTable {
    /// 产出 schema 与基类逻辑计划的公共字段。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 可选谓词抽取器；无则谓词原样返回。
    pub Extractor: Option<Box<dyn MemTablePredicateExtractor>>,
    /// 库名（大小写不敏感字符串 CIStr）。
    pub DBName: parser_ast::CIStr,
    /// 表元信息。
    pub TableInfo: model::TableInfo,
    /// 当前投影保留的列元数据。
    pub Columns: Vec<model::ColumnInfo>,
    /// 查询时间范围（起止时间戳），用于慢查询等按时间过滤。
    pub QueryTimeRange: Option<(i64, i64)>,
}

impl Default for LogicalMemTable {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            Extractor: None,
            DBName: parser_ast::CIStr::default(),
            TableInfo: model::TableInfo::default(),
            Columns: Vec::new(),
            QueryTimeRange: None,
        }
    }
}

impl LogicalMemTable {
    /// 初始化为 MemTableScan，绑定计划上下文与查询块偏移。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            NewBaseLogicalPlan(ctx, "MemTableScan", offset);
        self
    }
    /// 谓词下推：有 Extractor 时抽取，否则原样返回谓词。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let Some(extractor) = self.Extractor.as_mut() else {
            return Ok(predicates);
        };
        Ok(extractor.Extract(
            self.LogicalSchemaProducer.Schema(),
            self.LogicalSchemaProducer.OutputNames(),
            predicates,
        ))
    }
    /// 列裁剪：仅对白名单内的诊断/系统表按父节点用列精简 schema。
    pub fn PruneColumns(&mut self, parent: &[Column]) -> Result<()> {
        // 只有这些内存表支持按需裁列，其余表保持全列输出。
        let prunable = matches!(
            self.TableInfo.Name.L.as_str(),
            "statements_summary"
                | "statements_summary_history"
                | "tidb_statements_stats"
                | "cluster_statements_summary"
                | "cluster_statements_summary_history"
                | "cluster_tidb_statements_stats"
                | "slow_query"
                | "cluster_slow_query"
                | "tidb_trx"
                | "cluster_tidb_trx"
                | "data_lock_waits"
                | "deadlocks"
                | "cluster_deadlocks"
                | "tables"
        );
        if !prunable {
            return Ok(());
        }
        let used: HashSet<_> = parent.iter().map(|column| column.UniqueID).collect();
        let old_schema = self.Schema().Columns.clone();
        let old_names = self.OutputNames().Shallow();
        let old_columns = self.Columns.clone();
        let mut keep = old_schema
            .iter()
            .enumerate()
            .filter_map(|(i, c)| used.contains(&c.UniqueID).then_some(i))
            .collect::<Vec<_>>();
        // 父节点未引用任何列时至少保留第 0 列，避免空 schema。
        if keep.is_empty() && !old_schema.is_empty() {
            keep.push(0);
        }
        self.Schema_mut().Columns = keep.iter().map(|i| old_schema[*i].clone()).collect();
        self.SetOutputNames(NameSlice(
            keep.iter()
                .filter_map(|i| old_names.0.get(*i).cloned())
                .collect(),
        ));
        self.Columns = keep
            .iter()
            .filter_map(|i| old_columns.get(*i).cloned())
            .collect();
        Ok(())
    }
    /// TopN 保留在内存表之上，同时把可安全消费的 limit/方向提示交给 Extractor。
    pub fn PushDownTopN(&mut self, mut top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        let Some(plan) = top_n.as_mut() else {
            return None;
        };
        let Some(top_n_plan) = plan.as_any_mut().downcast_mut::<LogicalTopN>() else {
            return top_n;
        };
        if !top_n_plan.PartitionBy.is_empty() {
            return top_n;
        }
        if top_n_plan.IsLimit() {
            self.pushDownRowLimit(top_n_plan.Offset, top_n_plan.Count);
        } else if top_n_plan.ByItems.len() == 1
            && top_n_plan.ByItems[0]
                .Expr
                .as_any()
                .downcast_ref::<Column>()
                .is_some_and(|column| self.isSlowLogTopNByTime(column))
        {
            if let Some(extractor) = self.Extractor.as_mut() {
                extractor.SetDesc(top_n_plan.ByItems[0].Desc);
            }
            self.pushDownRowLimit(top_n_plan.Offset, top_n_plan.Count);
        }
        top_n
    }
    /// 将 Limit 的 offset+count 作为行数提示传给 Extractor。
    pub fn pushDownRowLimit(&mut self, offset: u64, count: u64) {
        if let Some(extractor) = self.Extractor.as_mut() {
            extractor.SetRowLimitHint(offset.saturating_add(count));
        }
    }
    /// 判断是否为慢日志表且按 time 列做 TopN 排序。
    pub fn isSlowLogTopNByTime(&self, column: &Column) -> bool {
        matches!(
            self.TableInfo.Name.L.as_str(),
            "slow_query" | "cluster_slow_query"
        ) && self
            .TableInfo
            .Columns
            .iter()
            .any(|info| info.ID == column.ID && info.Name.L == "time")
    }
    /// 推导统计信息：固定估计 10000 行，各列 NDV 同为行数。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let pseudo = statistics::PseudoTable(self.TableInfo.ID);
        let row_count = pseudo.HistColl.RealtimeCount as f64;
        let mut stats = StatsInfo {
            RowCount: row_count,
            HistColl: Some(Arc::new(pseudo.HistColl.clone())),
            StatsVersion: statistics::PseudoVersion,
            ..Default::default()
        };
        for column in &self.Schema().Columns {
            stats.ColNDVs.insert(column.UniqueID, row_count);
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }
}
impl LogicalPlan for LogicalMemTable {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn PredicatePushDown(&mut self, p: Vec<Expression>) -> Result<Vec<Expression>> {
        LogicalMemTable::PredicatePushDown(self, p)
    }
    fn PruneColumns(&mut self, c: &[Column]) -> Result<()> {
        LogicalMemTable::PruneColumns(self, c)
    }
    fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        LogicalMemTable::PushDownTopN(self, top_n)
    }
    fn DeriveStats(&mut self, r: bool) -> Result<(StatsInfo, bool)> {
        LogicalMemTable::DeriveStats(self, r)
    }
}
