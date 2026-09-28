// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 非分区表（普通表）的自动 ANALYZE 作业实现。
//
// 对应 Go `NonPartitionedTableAnalysisJob`：可整表分析或仅分析新增索引。
// ANALYZE 用于收集优化器所需的列/索引统计信息（直方图、NDV 等）。

use crate::job::*;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

/// 作业类型标记：整表分析。
pub const ANALYZE_TABLE: &str = "analyzeTable";
/// 作业类型标记：仅分析指定索引。
pub const ANALYZE_INDEX: &str = "analyzeIndex";

/// 非分区表分析作业：持有表 ID、待分析索引、权重指标与 SQL 标识符参数。
pub struct NonPartitionedTableAnalysisJob {
    /// 成功完成后回调（通常从 running_jobs 移除）。
    success_hook: Option<SuccessJobHook>,
    /// 失败后回调；must_retry 为 true 时进入 must_retry 集合等待重入队。
    failure_hook: Option<FailureJobHook>,
    /// 物理表 ID（非分区表即全局表 ID）。
    pub TableID: i64,
    /// 待分析索引 ID 集合；空表示整表分析。
    pub IndexIDs: HashMap<i64, ()>,
    /// 变更比例、表大小、上次分析间隔等优先级指标。
    pub indicators: Indicators,
    /// 统计版本（影响 ANALYZE 执行语义）。
    pub TableStatsVer: i32,
    /// 是否需要旧版本统计重写告警。
    pub NeedVersionRewriteWarn: bool,
    /// 由 PriorityCalculator 计算的调度权重（越大越优先）。
    pub Weight: f64,
    /// Schema（库）名，ValidateAndPrepare 时从元数据填充。
    pub SchemaName: String,
    /// 表名，ValidateAndPrepare 时从元数据填充。
    pub TableName: String,
    /// 索引名列表，由 IndexIDs 经元数据解析得到。
    pub IndexNames: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
/// 构造非分区表分析作业；名称字段初始为空，执行前由 ValidateAndPrepare 填充。
pub fn NewNonPartitionedTableAnalysisJob(
    table_id: i64,
    index_ids: HashMap<i64, ()>,
    table_stats_ver: i32,
    need_version_rewrite_warn: bool,
    change_percentage: f64,
    table_size: f64,
    last_analysis_duration: impl Into<AnalysisDuration>,
) -> NonPartitionedTableAnalysisJob {
    NonPartitionedTableAnalysisJob {
        success_hook: None,
        failure_hook: None,
        TableID: table_id,
        IndexIDs: index_ids,
        indicators: Indicators {
            ChangePercentage: change_percentage,
            TableSize: table_size,
            LastAnalysisDuration: last_analysis_duration.into(),
        },
        TableStatsVer: table_stats_ver,
        NeedVersionRewriteWarn: need_version_rewrite_warn,
        Weight: 0.0,
        SchemaName: String::new(),
        TableName: String::new(),
        IndexNames: Vec::new(),
    }
}

impl NonPartitionedTableAnalysisJob {
    /// 返回作业类型字符串：无索引则整表，否则索引分析。
    pub fn GetAnalyzeType(&self) -> &'static str {
        if self.IndexIDs.is_empty() {
            ANALYZE_TABLE
        } else {
            ANALYZE_INDEX
        }
    }

    /// 生成整表 ANALYZE SQL 模板与 `%n` 占位参数（schema、table）。
    pub fn GenSQLForAnalyzeTable(&self) -> (String, Vec<String>) {
        (
            "analyze table %n.%n".to_owned(),
            vec![self.SchemaName.clone(), self.TableName.clone()],
        )
    }

    /// 生成单索引 ANALYZE SQL 模板与参数（schema、table、index）。
    pub fn GenSQLForAnalyzeIndex(&self, index: &str) -> (String, Vec<String>) {
        (
            "analyze table %n.%n index %n".to_owned(),
            vec![
                self.SchemaName.clone(),
                self.TableName.clone(),
                index.to_owned(),
            ],
        )
    }

    /// 通过 AnalysisRuntime 执行整表 ANALYZE。
    pub fn AnalyzeTable(&self, runtime: &dyn AnalysisRuntime) -> Result<bool, String> {
        let (sql, params) = self.GenSQLForAnalyzeTable();
        runtime.execute_analyze(
            &sql,
            &params,
            self.TableStatsVer,
            self.NeedVersionRewriteWarn,
        )
    }

    /// Statistics version 2 refreshes columns and all indexes, so Go deliberately
    /// analyzes only the first index to avoid redundant ANALYZE statements.
    pub fn AnalyzeIndexes(&self, runtime: &dyn AnalysisRuntime) -> Result<bool, String> {
        if let Some(index) = self.IndexNames.first() {
            let (sql, params) = self.GenSQLForAnalyzeIndex(index);
            return runtime.execute_analyze(
                &sql,
                &params,
                self.TableStatsVer,
                self.NeedVersionRewriteWarn,
            );
        }
        Ok(true)
    }

    /// 根据成功/失败触发对应钩子。
    fn finish(&mut self, success: bool) {
        if success {
            if let Some(hook) = self.success_hook.clone() {
                hook(self);
            }
        } else if let Some(hook) = self.failure_hook.clone() {
            // must_retry=true：失败后允许后台重入队。
            hook(self, true);
        }
    }
}

impl AnalysisJob for NonPartitionedTableAnalysisJob {
    /// 校验表存在、解析索引名，并检查是否处于失败冷却窗口内。
    fn ValidateAndPrepare(&mut self, runtime: &dyn AnalysisRuntime) -> (bool, String) {
        let Some(metadata) = runtime.table_by_id(self.TableID) else {
            if let Some(hook) = self.failure_hook.clone() {
                hook(self, false);
            }
            return (false, TABLE_NOT_EXIST.to_owned());
        };
        self.IndexNames = index_names(&metadata, &self.IndexIDs);
        self.SchemaName = metadata.schema_name;
        self.TableName = metadata.table_name;
        // 非分区表无 partition 参数传入 IsValidToAnalyze。
        let result = IsValidToAnalyze(runtime, &self.SchemaName, &self.TableName, &[]);
        if !result.0
            && let Some(hook) = self.failure_hook.clone()
        {
            hook(self, true);
        }
        result
    }

    /// 按 IndexIDs 是否为空选择整表或索引分析路径，并调用 finish。
    fn Analyze(&mut self, runtime: &dyn AnalysisRuntime) -> Result<(), String> {
        let result = match self.GetAnalyzeType() {
            ANALYZE_TABLE => self.AnalyzeTable(runtime),
            ANALYZE_INDEX => self.AnalyzeIndexes(runtime),
            _ => unreachable!(),
        };
        match result {
            Ok(success) => {
                self.finish(success);
                Ok(())
            }
            Err(error) => {
                self.finish(false);
                Err(error)
            }
        }
    }

    fn SetWeight(&mut self, weight: f64) {
        self.Weight = weight;
    }
    fn GetWeight(&self) -> f64 {
        self.Weight
    }
    /// 存在待分析 IndexIDs 视为“新加索引”特殊事件，提升优先级。
    fn HasNewlyAddedIndex(&self) -> bool {
        !self.IndexIDs.is_empty()
    }
    fn GetIndicators(&self) -> Indicators {
        self.indicators.clone()
    }
    fn SetIndicators(&mut self, indicators: Indicators) {
        self.indicators = indicators;
    }
    fn GetTableID(&self) -> i64 {
        self.TableID
    }
    fn RegisterSuccessHook(&mut self, hook: SuccessJobHook) {
        self.success_hook = Some(hook);
    }
    fn RegisterFailureHook(&mut self, hook: FailureJobHook) {
        self.failure_hook = Some(hook);
    }
    /// 序列化为可观测/调试用 JSON 结构。
    fn AsJSON(&self) -> AnalysisJobJSON {
        AnalysisJobJSON {
            Type: self.GetAnalyzeType().to_owned(),
            TableID: self.TableID,
            SchemaName: self.SchemaName.clone(),
            TableName: self.TableName.clone(),
            IndexNames: self.IndexNames.clone(),
            Indicators: AsJSONIndicators(&self.indicators),
            Weight: format!("{:.6}", self.Weight),
            ..AnalysisJobJSON::default()
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl fmt::Display for NonPartitionedTableAnalysisJob {
    /// 人类可读的作业摘要，便于日志与测试输出。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "NonPartitionedTableAnalysisJob:\n\tAnalyzeType: {}\n\tIndexes: {}\n\tSchema: {}\n\tTable: {}\n\tTableID: {}\n\tTableStatsVer: {}\n\tChangePercentage: {:.6}\n\tTableSize: {:.2}\n\tLastAnalysisDuration: {}\n\tWeight: {:.6}\n",
            self.GetAnalyzeType(),
            self.IndexNames.join(", "),
            self.SchemaName,
            self.TableName,
            self.TableID,
            self.TableStatsVer,
            self.indicators.ChangePercentage,
            self.indicators.TableSize,
            crate::job::format_go_duration(self.indicators.LastAnalysisDuration),
            self.Weight
        )
    }
}
