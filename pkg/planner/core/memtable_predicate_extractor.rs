// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 集群诊断类内存表的谓词抽取器。
//
// 从 WHERE 中提取节点类型、实例、时间窗、日志级别、指标标签、
// 热点 Region（Region 为 TiKV 数据分片）等过滤条件，缩小内存表扫描范围；
// `SkipRequest` 表示条件无解、可跳过请求。

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
/// 谓词字面量取值。
pub enum PredicateValue {
    String(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq)]
/// 简化谓词：等值、IN、LIKE 与比较。
pub enum Predicate {
    Eq(String, PredicateValue),
    In(String, Vec<PredicateValue>),
    Like(String, String),
    LikeWithEscape(String, String, LikeEscape),
    Ilike(String, String, LikeEscape),
    Or(Vec<Predicate>),
    Regexp(String, String),
    Ge(String, PredicateValue),
    Gt(String, PredicateValue),
    Le(String, PredicateValue),
    Lt(String, PredicateValue),
}

/// Only a plan-time constant ESCAPE can be used to build a scan pattern.
#[derive(Clone, Debug, PartialEq)]
pub enum LikeEscape {
    Constant(u8),
    Missing,
    Dynamic,
    Deferred,
    Parameter,
}

/// Build the scan pattern and report whether scalar evaluation must be retained.
/// An inexact OR branch makes the complete disjunction a prefilter.
pub(crate) fn extract_like_pattern(
    predicate: &Predicate,
    column: &str,
    to_lower: bool,
    need_regexp: bool,
) -> Option<(String, bool)> {
    use stringutil_dependency::string_util::CompileLike2Regexp;
    if let Predicate::Or(branches) = predicate {
        if to_lower || branches.is_empty() {
            return None;
        }
        let mut patterns = Vec::with_capacity(branches.len());
        let mut prefilter = false;
        for branch in branches {
            let (pattern, branch_prefilter) =
                extract_like_pattern(branch, column, to_lower, need_regexp)?;
            patterns.push(pattern);
            prefilter |= branch_prefilter;
        }
        return Some((patterns.join("|"), prefilter));
    }
    let (field, pattern, escape, ilike) = match predicate {
        Predicate::Like(field, pattern) => (field, pattern, b'\\', false),
        Predicate::LikeWithEscape(field, pattern, LikeEscape::Constant(escape)) => {
            (field, pattern, *escape, false)
        }
        Predicate::Ilike(field, pattern, LikeEscape::Constant(escape)) => {
            (field, pattern, *escape, true)
        }
        Predicate::Eq(field, PredicateValue::String(value))
            if field.eq_ignore_ascii_case(column) =>
        {
            let mut quoted = String::from("^");
            for character in value.chars() {
                if "\\.+*?()|[]{}^$".contains(character) {
                    quoted.push('\\');
                }
                quoted.push(character);
            }
            quoted.push('$');
            return Some((
                if to_lower {
                    quoted.to_lowercase()
                } else {
                    quoted
                },
                false,
            ));
        }
        Predicate::Regexp(field, pattern) if field.eq_ignore_ascii_case(column) => {
            return Some((
                if to_lower {
                    pattern.to_lowercase()
                } else {
                    pattern.clone()
                },
                false,
            ));
        }
        _ => return None,
    };
    if !field.eq_ignore_ascii_case(column) {
        return None;
    }
    if !need_regexp {
        return Some((
            if to_lower {
                pattern.to_lowercase()
            } else {
                pattern.clone()
            },
            false,
        ));
    }
    let mut pattern = CompileLike2Regexp(pattern, escape);
    let prefilter = if ilike && !to_lower {
        pattern = format!("(?i:{pattern})");
        true
    } else {
        // Preserve the later correction in ece360bd: folded LIKE needs recheck.
        !ilike && to_lower
    };
    Some((
        if to_lower {
            pattern.to_lowercase()
        } else {
            pattern
        },
        prefilter,
    ))
}

/// 将谓词值列表转为字符串集合；含非字符串则返回 None。
fn strings_with_case(values: &[PredicateValue], lowercase: bool) -> Option<BTreeSet<String>> {
    values
        .iter()
        .map(|value| match value {
            PredicateValue::String(value) if lowercase => Some(value.to_lowercase()),
            PredicateValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn strings(values: &[PredicateValue]) -> Option<BTreeSet<String>> {
    strings_with_case(values, true)
}
/// 将谓词值列表转为 i64 集合（含可安全转换的 u64）。
fn i64s(values: &[PredicateValue]) -> Option<BTreeSet<i64>> {
    values
        .iter()
        .map(|value| match value {
            PredicateValue::I64(value) => Some(*value),
            PredicateValue::U64(value) => i64::try_from(*value).ok(),
            _ => None,
        })
        .collect()
}
/// 将谓词值列表转为 bool 集合。
fn bools(values: &[PredicateValue]) -> Option<BTreeSet<bool>> {
    values
        .iter()
        .map(|value| match value {
            PredicateValue::Bool(value) => Some(*value),
            PredicateValue::I64(value) => Some(*value == 1),
            PredicateValue::U64(value) => Some(*value == 1),
            _ => None,
        })
        .collect()
}
/// 若谓词作用于指定字段（忽略大小写），取出等值/IN 的值列表。
fn values<'a>(predicate: &'a Predicate, field: &str) -> Option<Vec<&'a PredicateValue>> {
    match predicate {
        Predicate::Eq(name, value) if name.eq_ignore_ascii_case(field) => Some(vec![value]),
        Predicate::In(name, values) if name.eq_ignore_ascii_case(field) => {
            Some(values.iter().collect())
        }
        _ => None,
    }
}
/// 集合并入：首次赋值，再次则求交集。
fn intersect<T: Ord + Clone>(
    target: &mut BTreeSet<T>,
    incoming: BTreeSet<T>,
    initialized: &mut bool,
) {
    if *initialized {
        *target = target.intersection(&incoming).cloned().collect();
    } else {
        *target = incoming;
        *initialized = true;
    }
}

fn time_value(value: &PredicateValue) -> Option<i64> {
    match value {
        PredicateValue::I64(value) => Some(*value),
        PredicateValue::U64(value) => i64::try_from(*value).ok(),
        _ => None,
    }
}

fn extract_time_range(predicates: &[Predicate], field_name: &str) -> (Vec<Predicate>, i64, i64) {
    let mut remaining = Vec::new();
    let mut start_time = 0;
    let mut end_time = 0;
    for predicate in predicates {
        let (field, value, lower, strict) = match predicate {
            Predicate::Eq(field, value) => (field, value, true, false),
            Predicate::Ge(field, value) => (field, value, true, false),
            Predicate::Gt(field, value) => (field, value, true, true),
            Predicate::Le(field, value) => (field, value, false, false),
            Predicate::Lt(field, value) => (field, value, false, true),
            _ => {
                remaining.push(predicate.clone());
                continue;
            }
        };
        if !field.eq_ignore_ascii_case(field_name) {
            remaining.push(predicate.clone());
            continue;
        }
        let Some(mut value) = time_value(value) else {
            remaining.push(predicate.clone());
            continue;
        };
        if strict {
            value = if lower {
                value.saturating_add(1)
            } else {
                value.saturating_sub(1)
            };
        }
        if matches!(predicate, Predicate::Eq(_, _)) {
            start_time = start_time.max(value);
            end_time = if end_time == 0 {
                value
            } else {
                end_time.min(value)
            };
        } else if lower {
            start_time = start_time.max(value);
        } else {
            end_time = if end_time == 0 {
                value
            } else {
                end_time.min(value)
            };
        }
    }
    (remaining, start_time, end_time)
}

fn join_set(values: &BTreeSet<String>) -> String {
    values.iter().cloned().collect::<Vec<_>>().join(",")
}

fn extract_string_set_field(
    predicates: &[Predicate],
    field: &str,
    lowercase: bool,
) -> (Vec<Predicate>, BTreeSet<String>, bool) {
    let mut remaining = Vec::new();
    let mut result = BTreeSet::new();
    let mut initialized = false;
    for predicate in predicates {
        if let Some(raw) = values(predicate, field)
            && let Some(set) =
                strings_with_case(&raw.into_iter().cloned().collect::<Vec<_>>(), lowercase)
        {
            intersect(&mut result, set, &mut initialized);
            continue;
        }
        remaining.push(predicate.clone());
    }
    let skip = initialized && result.is_empty();
    (remaining, result, skip)
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 集群表通用过滤：节点类型与实例地址。
pub struct ClusterTableExtractor {
    pub NodeTypes: BTreeSet<String>,
    pub Instances: BTreeSet<String>,
    pub SkipRequest: bool,
}
impl ClusterTableExtractor {
    /// 抽取 type/instance 过滤，返回未消费谓词。
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut node = false;
        let mut instance = false;
        let mut remaining = Vec::new();
        for predicate in predicates {
            if let Some(raw) = values(predicate, "type") {
                if let Some(set) = strings(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                    intersect(&mut self.NodeTypes, set, &mut node);
                    continue;
                }
            }
            if let Some(raw) = values(predicate, "instance") {
                if let Some(set) =
                    strings_with_case(&raw.into_iter().cloned().collect::<Vec<_>>(), false)
                {
                    intersect(&mut self.Instances, set, &mut instance);
                    continue;
                }
            }
            remaining.push(predicate.clone())
        }
        self.SkipRequest =
            (node && self.NodeTypes.is_empty()) || (instance && self.Instances.is_empty());
        remaining
    }
    /// Explain 摘要。
    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request:true".to_owned();
        }
        let mut parts = Vec::new();
        if !self.NodeTypes.is_empty() {
            parts.push(format!("node_types:[{}]", join_set(&self.NodeTypes)));
        }
        if !self.Instances.is_empty() {
            parts.push(format!("instances:[{}]", join_set(&self.Instances)));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 集群日志表：在通用过滤上增加时间窗、消息 LIKE 与日志级别。
pub struct ClusterLogTableExtractor {
    pub NodeTypes: BTreeSet<String>,
    pub Instances: BTreeSet<String>,
    pub StartTime: i64,
    pub EndTime: i64,
    pub Patterns: Vec<String>,
    pub LogLevels: BTreeSet<String>,
    pub SkipRequest: bool,
}
impl ClusterLogTableExtractor {
    /// 抽取 type/instance 过滤，返回未消费谓词。
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        // 先复用集群表 type/instance 抽取，再处理 time/message/level。
        let mut base = ClusterTableExtractor::default();
        let mut remaining = base.ExtractPredicates(predicates);
        self.NodeTypes = base.NodeTypes;
        self.Instances = base.Instances;
        self.SkipRequest = base.SkipRequest;
        let mut levels = false;
        let (time_remaining, start_time, end_time) = extract_time_range(&remaining, "time");
        remaining = time_remaining;
        self.StartTime = start_time;
        self.EndTime = end_time;
        remaining.retain(|predicate| {
            if let Some((pattern, prefilter)) =
                extract_like_pattern(predicate, "message", false, true)
            {
                self.Patterns.push(pattern);
                return prefilter;
            }
            if let Some(raw) = values(predicate, "level") {
                if let Some(set) = strings(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                    intersect(&mut self.LogLevels, set, &mut levels);
                    return false;
                }
            }
            true
        });
        self.SkipRequest |= (levels && self.LogLevels.is_empty())
            || (self.EndTime != 0 && self.StartTime > self.EndTime);
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }
    /// Explain 摘要。
    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request: true".to_owned();
        }
        let mut parts = Vec::new();
        if self.StartTime > 0 {
            parts.push(format!("start_time:{}", self.StartTime));
        }
        if self.EndTime > 0 {
            parts.push(format!("end_time:{}", self.EndTime));
        }
        if !self.NodeTypes.is_empty() {
            parts.push(format!("node_types:[{}]", join_set(&self.NodeTypes)));
        }
        if !self.Instances.is_empty() {
            parts.push(format!("instances:[{}]", join_set(&self.Instances)));
        }
        if !self.LogLevels.is_empty() {
            parts.push(format!("log_levels:[{}]", join_set(&self.LogLevels)));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 指标表：标签条件、分位数、时间窗。
pub struct MetricTableExtractor {
    pub LabelConditions: BTreeMap<String, BTreeSet<String>>,
    pub Quantiles: BTreeSet<String>,
    pub StartTime: i64,
    pub EndTime: i64,
    pub SkipRequest: bool,
}
impl MetricTableExtractor {
    /// 抽取 type/instance 过滤，返回未消费谓词。
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut remaining = Vec::new();
        let mut initialized = BTreeSet::new();
        for predicate in predicates {
            let field = match predicate {
                Predicate::Eq(field, _) | Predicate::In(field, _) => field,
                _ => {
                    remaining.push(predicate.clone());
                    continue;
                }
            };
            let Some(raw) = values(predicate, field) else {
                remaining.push(predicate.clone());
                continue;
            };
            // quantile 与其它标签分开存储；非法分位数会使 SkipRequest。
            if field.eq_ignore_ascii_case("quantile") {
                let set = raw
                    .into_iter()
                    .filter_map(|value| match value {
                        PredicateValue::F64(v) => Some(v.to_string()),
                        PredicateValue::String(v) => Some(v.clone()),
                        _ => None,
                    })
                    .collect();
                let mut seen = initialized.contains("quantile");
                intersect(&mut self.Quantiles, set, &mut seen);
                initialized.insert("quantile".into());
            } else if field.eq_ignore_ascii_case("time") || field.eq_ignore_ascii_case("value") {
                remaining.push(predicate.clone());
            } else {
                let Some(set) =
                    strings_with_case(&raw.into_iter().cloned().collect::<Vec<_>>(), false)
                else {
                    remaining.push(predicate.clone());
                    continue;
                };
                let entry = self
                    .LabelConditions
                    .entry(field.to_lowercase())
                    .or_default();
                let mut seen = initialized.contains(field);
                intersect(entry, set, &mut seen);
                initialized.insert(field.to_lowercase());
                // Metric readers may not be able to apply every label filter.
                // Keep it for the SQL layer, as Go's Extract does.
                remaining.push(predicate.clone());
            }
        }
        let (time_remaining, start_time, end_time) = extract_time_range(&remaining, "time");
        remaining = time_remaining;
        self.StartTime = start_time;
        self.EndTime = end_time;
        self.SkipRequest = self.LabelConditions.values().any(BTreeSet::is_empty)
            || (self.EndTime != 0 && self.StartTime > self.EndTime);
        remaining
    }
    /// 将标签条件拼成 Prometheus 风格选择器。
    pub fn GetMetricTablePromQL(&self, table: &str) -> String {
        let labels = self
            .LabelConditions
            .iter()
            .flat_map(|(name, values)| {
                values
                    .iter()
                    .map(move |value| format!(r#"{name}=\"{value}\""#))
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{table}{{{labels}}}")
    }

    pub fn ExplainInfo(&self, table: &str) -> String {
        if self.SkipRequest {
            "skip_request: true".to_owned()
        } else {
            format!(
                "PromQL:{}, start_time:{}, end_time:{}",
                self.GetMetricTablePromQL(table),
                self.StartTime,
                self.EndTime
            )
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 指标摘要表：复用 MetricTableExtractor，抽出 metrics_name。
pub struct MetricSummaryTableExtractor {
    pub Quantiles: BTreeSet<String>,
    pub MetricsNames: BTreeSet<String>,
    pub SkipRequest: bool,
}
impl MetricSummaryTableExtractor {
    pub fn ExtractPredicates(&mut self, p: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut quantile_initialized = false;
        for predicate in p {
            if let Some(raw) = values(predicate, "quantile") {
                let quantiles = raw
                    .into_iter()
                    .filter_map(|value| match value {
                        PredicateValue::F64(value) => Some(value.to_string()),
                        PredicateValue::String(value) => {
                            value.parse::<f64>().ok().map(|value| value.to_string())
                        }
                        _ => None,
                    })
                    .collect();
                intersect(&mut self.Quantiles, quantiles, &mut quantile_initialized);
            }
        }
        // Go intentionally runs the metrics_name extraction against the original
        // predicate list, so the returned predicates still contain quantile filters.
        let (remaining, metrics_names, metrics_skip) =
            extract_string_set_field(p, "metrics_name", true);
        self.MetricsNames = metrics_names;
        self.SkipRequest = (quantile_initialized && self.Quantiles.is_empty()) || metrics_skip;
        remaining
    }

    pub fn ExplainInfo(&self) -> String {
        String::new()
    }
}

/// 生成双字符串字段过滤抽取器（如 inspection 规则/项）。
macro_rules! string_filter_extractor {
    ($name:ident,$first:ident,$first_field:literal,$second:ident,$second_field:literal,$skip:ident) => {
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name {
            pub $first: BTreeSet<String>,
            pub $second: BTreeSet<String>,
            pub $skip: bool,
        }
        impl $name {
            pub fn ExtractPredicates(&mut self, p: &[Predicate]) -> Vec<Predicate> {
                *self = Self::default();
                let mut a = false;
                let mut b = false;
                let mut rest = Vec::new();
                for pred in p {
                    if let Some(raw) = values(pred, $first_field) {
                        if let Some(set) = strings(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                            intersect(&mut self.$first, set, &mut a);
                            continue;
                        }
                    }
                    if let Some(raw) = values(pred, $second_field) {
                        if let Some(set) = strings(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                            intersect(&mut self.$second, set, &mut b);
                            continue;
                        }
                    }
                    rest.push(pred.clone())
                }
                self.$skip = (a && self.$first.is_empty()) || (b && self.$second.is_empty());
                rest
            }
        }
    };
}
string_filter_extractor!(
    InspectionResultTableExtractor,
    Rules,
    "rule",
    Items,
    "item",
    SkipInspection
);
impl InspectionResultTableExtractor {
    pub fn ExplainInfo(&self) -> String {
        if self.SkipInspection {
            "skip_inspection:true".to_owned()
        } else {
            format!(
                "rules:[{}], items:[{}]",
                join_set(&self.Rules),
                join_set(&self.Items)
            )
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectionSummaryTableExtractor {
    pub Rules: BTreeSet<String>,
    pub MetricNames: BTreeSet<String>,
    pub Quantiles: BTreeSet<String>,
    pub SkipInspection: bool,
}

impl InspectionSummaryTableExtractor {
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let (_, rules, rule_skip) = extract_string_set_field(predicates, "rule", true);
        let (_, metric_names, metric_skip) =
            extract_string_set_field(predicates, "metrics_name", true);
        let mut quantile_initialized = false;
        let mut remaining = Vec::new();
        for predicate in predicates {
            if let Some(raw) = values(predicate, "quantile") {
                let set = raw
                    .into_iter()
                    .filter_map(|value| match value {
                        PredicateValue::F64(value) => Some(value.to_string()),
                        PredicateValue::String(value) => {
                            value.parse::<f64>().ok().map(|value| value.to_string())
                        }
                        _ => None,
                    })
                    .collect();
                intersect(&mut self.Quantiles, set, &mut quantile_initialized);
                continue;
            }
            remaining.push(predicate.clone());
        }
        self.Rules = rules;
        self.MetricNames = metric_names;
        self.SkipInspection =
            rule_skip || metric_skip || (quantile_initialized && self.Quantiles.is_empty());
        if self.SkipInspection {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipInspection {
            return "skip_inspection: true".to_owned();
        }
        let mut parts = Vec::new();
        if !self.Rules.is_empty() {
            parts.push(format!("rules:[{}]", join_set(&self.Rules)));
        }
        if !self.MetricNames.is_empty() {
            parts.push(format!("metric_names:[{}]", join_set(&self.MetricNames)));
        }
        if !self.Quantiles.is_empty() {
            parts.push(format!(
                "quantiles:[{}]",
                self.Quantiles
                    .iter()
                    .filter_map(|value| value.parse::<f64>().ok())
                    .map(|value| format!("{value:.6}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectionRuleTableExtractor {
    pub Types: BTreeSet<String>,
    pub SkipRequest: bool,
}

impl InspectionRuleTableExtractor {
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let (remaining, types, skip) = extract_string_set_field(predicates, "type", true);
        self.Types = types;
        self.SkipRequest = skip;
        if skip { Vec::new() } else { remaining }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            "skip_request: true".to_owned()
        } else if self.Types.is_empty() {
            String::new()
        } else {
            format!("node_types:[{}]", join_set(&self.Types))
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 热点 Region 历史：时间窗、learner/leader、类型与 ID 集合。
pub struct HotRegionsHistoryTableExtractor {
    pub StartTime: i64,
    pub EndTime: i64,
    pub IsLearners: BTreeSet<bool>,
    pub IsLeaders: BTreeSet<bool>,
    pub HotRegionTypes: BTreeSet<String>,
    pub RegionIDs: BTreeSet<i64>,
    pub StoreIDs: BTreeSet<i64>,
    pub PeerIDs: BTreeSet<i64>,
    pub SkipRequest: bool,
}
pub const HotRegionTypeRead: &str = "read";
pub const HotRegionTypeWrite: &str = "write";
impl HotRegionsHistoryTableExtractor {
    pub fn ExtractPredicates(&mut self, p: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut remaining = Vec::new();
        let mut region_initialized = false;
        let mut store_initialized = false;
        let mut peer_initialized = false;
        let mut learner_initialized = false;
        let mut leader_initialized = false;
        let mut type_initialized = false;
        for pred in p {
            let mut consumed = false;
            for (field, target, initialized) in [
                ("region_id", &mut self.RegionIDs, &mut region_initialized),
                ("store_id", &mut self.StoreIDs, &mut store_initialized),
                ("peer_id", &mut self.PeerIDs, &mut peer_initialized),
            ] {
                if let Some(raw) = values(pred, field)
                    && let Some(set) = i64s(&raw.into_iter().cloned().collect::<Vec<_>>())
                {
                    intersect(target, set, initialized);
                    consumed = true;
                    break;
                }
            }
            if !consumed
                && let Some(raw) = values(pred, "type")
                && let Some(set) =
                    strings_with_case(&raw.into_iter().cloned().collect::<Vec<_>>(), false)
            {
                intersect(&mut self.HotRegionTypes, set, &mut type_initialized);
                consumed = true;
            }
            if !consumed
                && let Some(raw) = values(pred, "is_learner")
                && let Some(set) = bools(&raw.into_iter().cloned().collect::<Vec<_>>())
            {
                intersect(&mut self.IsLearners, set, &mut learner_initialized);
                consumed = true;
            }
            if !consumed
                && let Some(raw) = values(pred, "is_leader")
                && let Some(set) = bools(&raw.into_iter().cloned().collect::<Vec<_>>())
            {
                intersect(&mut self.IsLeaders, set, &mut leader_initialized);
                consumed = true;
            }
            if !consumed {
                remaining.push(pred.clone())
            }
        }
        let (remaining, start_time, end_time) = extract_time_range(&remaining, "update_time");
        self.StartTime = start_time;
        self.EndTime = end_time;
        if !learner_initialized {
            self.IsLearners = [false, true].into_iter().collect();
        }
        if !leader_initialized {
            self.IsLeaders = [false, true].into_iter().collect();
        }
        if !type_initialized {
            self.HotRegionTypes = [HotRegionTypeRead.into(), HotRegionTypeWrite.into()]
                .into_iter()
                .collect();
        }
        self.SkipRequest = (region_initialized && self.RegionIDs.is_empty())
            || (store_initialized && self.StoreIDs.is_empty())
            || (peer_initialized && self.PeerIDs.is_empty())
            || (learner_initialized && self.IsLearners.is_empty())
            || (leader_initialized && self.IsLeaders.is_empty())
            || (type_initialized && self.HotRegionTypes.is_empty())
            || (self.EndTime != 0 && self.StartTime > self.EndTime);
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request: true".to_owned();
        }
        let mut parts = Vec::new();
        if self.StartTime > 0 {
            parts.push(format!("start_time:{}", self.StartTime));
        }
        if self.EndTime > 0 {
            parts.push(format!("end_time:{}", self.EndTime));
        }
        for (name, values) in [
            ("region_ids", &self.RegionIDs),
            ("store_ids", &self.StoreIDs),
            ("peer_ids", &self.PeerIDs),
        ] {
            if !values.is_empty() {
                parts.push(format!(
                    "{name}:[{}]",
                    values
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
        }
        if !self.IsLearners.is_empty() {
            parts.push(format!(
                "learner_roles:[{}]",
                self.IsLearners
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !self.IsLeaders.is_empty() {
            parts.push(format!(
                "leader_roles:[{}]",
                self.IsLeaders
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !self.HotRegionTypes.is_empty() {
            parts.push(format!(
                "hot_region_types:[{}]",
                join_set(&self.HotRegionTypes)
            ));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 抽取出的闭区间时间范围；本 crate 使用毫秒时间戳。
pub struct TimeRange {
    pub StartTime: i64,
    pub EndTime: i64,
}

impl TimeRange {
    pub fn new(start_time: i64, end_time: i64) -> Self {
        Self {
            StartTime: start_time,
            EndTime: end_time,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// slow_query 文件扫描提示与时间范围。
pub struct SlowQueryExtractor {
    pub SkipRequest: bool,
    pub TimeRanges: Vec<TimeRange>,
    pub Enable: bool,
    pub Desc: bool,
    pub Limit: u64,
}

impl SlowQueryExtractor {
    pub fn SetRowLimitHint(&mut self, limit: u64) {
        if limit != 0 && (self.Limit == 0 || limit < self.Limit) {
            self.Limit = limit;
        }
    }

    pub fn SetDesc(&mut self, desc: bool) {
        self.Desc = desc;
    }

    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        self.SkipRequest = false;
        self.TimeRanges.clear();
        self.Enable = false;
        let (remaining, start_time, end_time) = extract_time_range(predicates, "time");
        if start_time == 0 && end_time == 0 {
            return remaining;
        }
        let range = TimeRange::new(
            if start_time == 0 {
                i64::MIN
            } else {
                start_time
            },
            if end_time == 0 { i64::MAX } else { end_time },
        );
        self.SkipRequest = range.StartTime > range.EndTime;
        self.Enable = true;
        self.TimeRanges.push(range);
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self, slow_query_file: &str) -> String {
        if self.SkipRequest {
            return "skip_request: true".to_owned();
        }
        if !self.Enable {
            return format!("only search in the current '{slow_query_file}' file");
        }
        let range = self.TimeRanges[0];
        format!("start_time:{}, end_time:{}", range.StartTime, range.EndTime)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// information_schema.TABLE_STORAGE_STATS 的 schema/table 过滤。
pub struct TableStorageStatsExtractor {
    pub SkipRequest: bool,
    pub TableSchema: BTreeSet<String>,
    pub TableName: BTreeSet<String>,
}

impl TableStorageStatsExtractor {
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let (remaining, schemas, schema_skip) =
            extract_string_set_field(predicates, "table_schema", true);
        let (remaining, tables, table_skip) =
            extract_string_set_field(&remaining, "table_name", true);
        self.TableSchema = schemas;
        self.TableName = tables;
        self.SkipRequest = schema_skip || table_skip;
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request: true".to_owned();
        }
        let mut parts = Vec::new();
        if !self.TableSchema.is_empty() {
            parts.push(format!("schema:[{}]", join_set(&self.TableSchema)));
        }
        if !self.TableName.is_empty() {
            parts.push(format!("table:[{}]", join_set(&self.TableName)));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// TiFlash system 表的实例、数据库与表过滤。
pub struct TiFlashSystemTableExtractor {
    pub SkipRequest: bool,
    pub TiFlashInstances: BTreeSet<String>,
    pub TiDBDatabases: String,
    pub TiDBTables: String,
}

impl TiFlashSystemTableExtractor {
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let (remaining, instances, instance_skip) =
            extract_string_set_field(predicates, "tiflash_instance", false);
        let (remaining, databases, database_skip) =
            extract_string_set_field(&remaining, "tidb_database", true);
        let (remaining, tables, table_skip) =
            extract_string_set_field(&remaining, "tidb_table", true);
        self.TiFlashInstances = instances;
        self.TiDBDatabases = join_set(&databases);
        self.TiDBTables = join_set(&tables);
        self.SkipRequest = instance_skip || database_skip || table_skip;
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request:true".to_owned();
        }
        let mut parts = Vec::new();
        if !self.TiFlashInstances.is_empty() {
            parts.push(format!(
                "tiflash_instances:[{}]",
                join_set(&self.TiFlashInstances)
            ));
        }
        if !self.TiDBDatabases.is_empty() {
            parts.push(format!("tidb_databases:[{}]", self.TiDBDatabases));
        }
        if !self.TiDBTables.is_empty() {
            parts.push(format!("tidb_tables:[{}]", self.TiDBTables));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// statements_summary 的 digest 与粗粒度时间范围过滤。
pub struct StatementsSummaryExtractor {
    pub SkipRequest: bool,
    pub Digests: BTreeSet<String>,
    pub CoarseTimeRange: Option<TimeRange>,
}

impl StatementsSummaryExtractor {
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let (remaining, digests, digest_skip) =
            extract_string_set_field(predicates, "digest", false);
        self.Digests = digests;
        if digest_skip {
            self.SkipRequest = true;
            return Vec::new();
        }

        let (_, _, end_time) = extract_time_range(&remaining, "summary_begin_time");
        let (_, start_time, _) = extract_time_range(&remaining, "summary_end_time");
        if start_time != 0 || end_time != 0 {
            const DEFAULT_STATEMENTS_DURATION_MS: i64 = 60 * 60 * 1000;
            let start_time = if start_time == 0 {
                end_time.saturating_sub(DEFAULT_STATEMENTS_DURATION_MS)
            } else {
                start_time
            };
            let end_time = if end_time == 0 {
                start_time.saturating_add(DEFAULT_STATEMENTS_DURATION_MS)
            } else {
                end_time
            };
            self.CoarseTimeRange = Some(TimeRange::new(start_time, end_time));
            self.SkipRequest = start_time > end_time;
        }
        if self.SkipRequest {
            Vec::new()
        } else {
            remaining
        }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request: true".to_owned();
        }
        let mut parts = Vec::new();
        if !self.Digests.is_empty() {
            parts.push(format!("digests: [{}]", join_set(&self.Digests)));
        }
        if let Some(range) = self.CoarseTimeRange {
            parts.push(format!(
                "start_time: {}, end_time: {}",
                range.StartTime, range.EndTime
            ));
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// TiKV Region Peer 表：按 region_id / store_id 过滤。
pub struct TikvRegionPeersExtractor {
    pub RegionIDs: BTreeSet<i64>,
    pub StoreIDs: BTreeSet<i64>,
    pub SkipRequest: bool,
}
impl TikvRegionPeersExtractor {
    pub fn ExtractPredicates(&mut self, p: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut rest = Vec::new();
        let mut region_initialized = false;
        let mut store_initialized = false;
        for pred in p {
            let mut consumed = false;
            for (field, target, initialized) in [
                ("region_id", &mut self.RegionIDs, &mut region_initialized),
                ("store_id", &mut self.StoreIDs, &mut store_initialized),
            ] {
                if let Some(raw) = values(pred, field) {
                    if let Some(set) = i64s(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                        intersect(target, set, initialized);
                        consumed = true;
                        break;
                    }
                }
            }
            if !consumed {
                rest.push(pred.clone())
            }
        }
        self.SkipRequest = (region_initialized && self.RegionIDs.is_empty())
            || (store_initialized && self.StoreIDs.is_empty());
        if self.SkipRequest { Vec::new() } else { rest }
    }

    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request:true".to_owned();
        }
        let mut parts = Vec::new();
        for (name, values) in [
            ("region_ids", &self.RegionIDs),
            ("store_ids", &self.StoreIDs),
        ] {
            if !values.is_empty() {
                parts.push(format!(
                    "{name}:[{}]",
                    values
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// TiKV Region 状态表：按 table_id 过滤。
pub struct TiKVRegionStatusExtractor {
    pub TableIDs: BTreeSet<i64>,
}
impl TiKVRegionStatusExtractor {
    pub fn ExtractPredicates(&mut self, p: &[Predicate]) -> Vec<Predicate> {
        *self = Self::default();
        let mut rest = Vec::new();
        let mut initialized = false;
        for pred in p {
            if let Some(raw) = values(pred, "table_id") {
                if let Some(set) = i64s(&raw.into_iter().cloned().collect::<Vec<_>>()) {
                    intersect(&mut self.TableIDs, set, &mut initialized);
                    continue;
                }
            }
            rest.push(pred.clone())
        }
        rest
    }
    /// 返回已抽取的表 ID 列表。
    pub fn GetTablesID(&self) -> Vec<i64> {
        self.TableIDs.iter().copied().collect()
    }

    pub fn ExplainInfo(&self) -> String {
        if self.TableIDs.is_empty() {
            String::new()
        } else {
            format!(
                "table_id in {{{}}}",
                self.TableIDs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}
