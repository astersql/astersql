// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// mock Coprocessor 聚合执行器：HashAgg 与 StreamAgg。
//
// 聚合指按分组键累积 COUNT/SUM/MIN/MAX/FIRST 等状态；HashAgg 用哈希表缓冲全量分组，
// StreamAgg 要求输入已按分组键有序，按流式切换分组输出。

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use crate::copr_handler::{AggCall, AggKind, CopError, Datum, ExecDetail, Expr, Row};
use crate::executor::{NextRow, executor};

/// 单个聚合函数的运行时状态（计数、求和、最值、首值）。
#[derive(Clone, Debug)]
pub enum AggState {
    Count(u64),
    Sum(Option<Datum>),
    Min(Option<Datum>),
    Max(Option<Datum>),
    First(Option<Datum>),
}

impl AggState {
    /// 按聚合种类初始化空状态。
    fn new(call: &AggCall) -> Self {
        match call.kind {
            AggKind::Count => Self::Count(0),
            AggKind::Sum => Self::Sum(None),
            AggKind::Min => Self::Min(None),
            AggKind::Max => Self::Max(None),
            AggKind::First => Self::First(None),
        }
    }

    /// 用一行的求值结果更新本聚合状态；NULL 在 COUNT/SUM/MIN/MAX 中按 SQL 语义跳过。
    fn update(&mut self, value: Datum) -> Result<(), CopError> {
        match self {
            Self::Count(count) => {
                if !matches!(value, Datum::Null) {
                    *count += 1;
                }
            }
            Self::First(slot) => {
                if slot.is_none() {
                    *slot = Some(value);
                }
            }
            Self::Min(slot) => {
                if !matches!(value, Datum::Null)
                    && slot.as_ref().is_none_or(|current| value < *current)
                {
                    *slot = Some(value);
                }
            }
            Self::Max(slot) => {
                if !matches!(value, Datum::Null)
                    && slot.as_ref().is_none_or(|current| value > *current)
                {
                    *slot = Some(value);
                }
            }
            Self::Sum(slot) => {
                if matches!(value, Datum::Null) {
                    return Ok(());
                }
                *slot = Some(match (slot.take(), value) {
                    (None, value) => value,
                    (Some(Datum::Int(left)), Datum::Int(right)) => {
                        Datum::Int(left.saturating_add(right))
                    }
                    (Some(Datum::Uint(left)), Datum::Uint(right)) => {
                        Datum::Uint(left.saturating_add(right))
                    }
                    (Some(Datum::Real(left)), Datum::Real(right)) => Datum::Real(left + right),
                    // 类型不一致时拒绝求和，与严格数值类型预期一致。
                    _ => return Err(CopError::Type("sum expects one numeric type".into())),
                });
            }
        }
        Ok(())
    }

    /// 取出最终聚合结果；未收到有效值时返回 NULL。
    fn result(&self) -> Datum {
        match self {
            Self::Count(value) => Datum::Uint(*value),
            Self::Sum(value) | Self::Min(value) | Self::Max(value) | Self::First(value) => {
                value.clone().unwrap_or(Datum::Null)
            }
        }
    }
}

/// 分组键 → 该组聚合状态向量；用 BTreeMap 保证确定性顺序。
pub type aggCtxsMapper = BTreeMap<Vec<Datum>, Vec<AggState>>;

/// 对分组表达式求值，得到当前行的分组键。
fn eval_group_key(expressions: &[Expr], row: &[Datum]) -> Result<Vec<Datum>, CopError> {
    expressions
        .iter()
        .map(|expression| expression.eval(row))
        .collect()
}

/// 用当前行更新一组聚合状态；无表达式时 COUNT 默认计 1。
fn update_aggregates(
    states: &mut [AggState],
    calls: &[AggCall],
    row: &[Datum],
) -> Result<(), CopError> {
    for (state, call) in states.iter_mut().zip(calls) {
        let value = call
            .expr
            .as_ref()
            .map(|expression| expression.eval(row))
            .transpose()?
            .unwrap_or(Datum::Int(1));
        state.update(value)?;
    }
    Ok(())
}

/// 组装输出行：先写各聚合结果，再追加分组键列。
fn output_row(states: &[AggState], group_key: &[Datum]) -> Row {
    states
        .iter()
        .map(AggState::result)
        .chain(group_key.iter().cloned())
        .collect()
}

/// 合并上游执行明细与本算子自身的 ExecDetail。
fn append_details(source: Option<&dyn executor>, own: &ExecDetail) -> Vec<ExecDetail> {
    let mut details = source.map(executor::ExecDetails).unwrap_or_default();
    details.push(own.clone());
    details
}

/// Hash 聚合执行器：消费完上游后按分组键输出聚合行。
pub struct hashAggExec {
    pub aggExprs: Vec<AggCall>,
    pub groupByExprs: Vec<Expr>,
    pub aggCtxsMap: aggCtxsMapper,
    pub groupKeys: Vec<Vec<Datum>>,
    pub rows: VecDeque<Row>,
    pub executed: bool,
    pub count: i64,
    pub execDetail: ExecDetail,
    pub src: Option<Box<dyn executor>>,
}

impl hashAggExec {
    /// 构造尚未绑定上游的 HashAgg。
    pub fn new(aggregates: Vec<AggCall>, group_by: Vec<Expr>) -> Self {
        Self {
            aggExprs: aggregates,
            groupByExprs: group_by,
            aggCtxsMap: BTreeMap::new(),
            groupKeys: Vec::new(),
            rows: VecDeque::new(),
            executed: false,
            count: 0,
            execDetail: ExecDetail::default(),
            src: None,
        }
    }

    /// 从上游取一行并聚合；上游结束返回 false。
    pub fn innerNext(&mut self) -> Result<bool, CopError> {
        let row = self
            .src
            .as_mut()
            .ok_or_else(|| CopError::InvalidRequest("hash aggregate has no source".into()))?
            .Next()?;
        let Some(row) = row else {
            return Ok(false);
        };
        self.aggregate(&row)?;
        Ok(true)
    }

    /// 计算当前行的分组键。
    pub fn getGroupKey(&self, row: &[Datum]) -> Result<Vec<Datum>, CopError> {
        eval_group_key(&self.groupByExprs, row)
    }

    /// 将一行并入对应分组的聚合状态；新分组会登记到 `groupKeys`。
    pub fn aggregate(&mut self, row: &[Datum]) -> Result<(), CopError> {
        let group_key = self.getGroupKey(row)?;
        if !self.aggCtxsMap.contains_key(&group_key) {
            self.groupKeys.push(group_key.clone());
        }
        let states = self
            .aggCtxsMap
            .entry(group_key)
            .or_insert_with(|| self.aggExprs.iter().map(AggState::new).collect());
        update_aggregates(states, &self.aggExprs, row)
    }

    /// 按分组键查询已累积的聚合状态切片。
    pub fn getContexts(&self, group_key: &[Datum]) -> Option<&[AggState]> {
        self.aggCtxsMap.get(group_key).map(Vec::as_slice)
    }
}

impl executor for hashAggExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        if let Some(source) = self.src.as_mut() {
            source.ResetCounts();
        }
    }
    fn Counts(&self) -> Vec<i64> {
        self.src
            .as_ref()
            .map(|source| source.Counts())
            .unwrap_or_default()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.execDetail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        self.count += 1;
        let result = (|| {
            // 首次调用时排空上游，物化全部分组结果到 rows 队列。
            if !self.executed {
                while self.innerNext()? {}
                // 无 GROUP BY 且无输入时仍输出一行空分组的聚合结果（如 COUNT(*)=0）。
                if self.groupKeys.is_empty() && self.groupByExprs.is_empty() {
                    self.groupKeys.push(Vec::new());
                    self.aggCtxsMap.insert(
                        Vec::new(),
                        self.aggExprs.iter().map(AggState::new).collect(),
                    );
                }
                for key in &self.groupKeys {
                    self.rows.push_back(output_row(&self.aggCtxsMap[key], key));
                }
                self.executed = true;
            }
            Ok(self.rows.pop_front())
        })();
        match &result {
            Ok(row) => self.execDetail.update(begin, row),
            Err(_) => self.execDetail.update(begin, &None),
        }
        result
    }
}

/// Stream 聚合执行器：输入须已按分组键有序，遇到新分组则输出上一组结果。
pub struct streamAggExec {
    pub aggExprs: Vec<AggCall>,
    pub groupByExprs: Vec<Expr>,
    pub current_key: Option<Vec<Datum>>,
    pub current_states: Vec<AggState>,
    pub pending: Option<Row>,
    pub finished: bool,
    pub count: i64,
    pub execDetail: ExecDetail,
    pub src: Option<Box<dyn executor>>,
}

impl streamAggExec {
    /// 构造尚未绑定上游的 StreamAgg。
    pub fn new(aggregates: Vec<AggCall>, group_by: Vec<Expr>) -> Self {
        Self {
            current_states: aggregates.iter().map(AggState::new).collect(),
            aggExprs: aggregates,
            groupByExprs: group_by,
            current_key: None,
            pending: None,
            finished: false,
            count: 0,
            execDetail: ExecDetail::default(),
            src: None,
        }
    }

    /// 将当前分组的聚合状态格式化为输出行。
    pub fn getPartialResult(&self) -> Row {
        output_row(
            &self.current_states,
            self.current_key.as_deref().unwrap_or(&[]),
        )
    }
    /// 判断当前行是否开启了新的分组（相对已记录的 `current_key`）。
    pub fn meetNewGroup(&self, row: &[Datum]) -> Result<bool, CopError> {
        let next_key = eval_group_key(&self.groupByExprs, row)?;
        Ok(self
            .current_key
            .as_ref()
            .is_some_and(|key| *key != next_key))
    }
}

impl executor for streamAggExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        if let Some(source) = self.src.as_mut() {
            source.ResetCounts();
        }
    }
    fn Counts(&self) -> Vec<i64> {
        self.src
            .as_ref()
            .map(|source| source.Counts())
            .unwrap_or_default()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.execDetail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        self.count += 1;
        let result = (|| {
            if self.finished {
                return Ok(None);
            }
            // 优先消费上一轮因分组切换而暂存的 pending 行。
            let first = match self.pending.take() {
                Some(row) => Some(row),
                None => self
                    .src
                    .as_mut()
                    .ok_or_else(|| {
                        CopError::InvalidRequest("stream aggregate has no source".into())
                    })?
                    .Next()?,
            };
            let Some(first) = first else {
                self.finished = true;
                // 无 GROUP BY 且从未见过行：仍输出一行空分组聚合结果。
                if self.current_key.is_none() && self.groupByExprs.is_empty() {
                    self.current_key = Some(Vec::new());
                }
                let row = self.current_key.as_ref().map(|_| self.getPartialResult());
                self.current_key = None;
                return Ok(row);
            };
            let key = eval_group_key(&self.groupByExprs, &first)?;
            self.current_key = Some(key.clone());
            self.current_states = self.aggExprs.iter().map(AggState::new).collect();
            update_aggregates(&mut self.current_states, &self.aggExprs, &first)?;
            // 持续消费同组行，直到上游结束或遇到新分组键。
            loop {
                let next = self
                    .src
                    .as_mut()
                    .ok_or_else(|| {
                        CopError::InvalidRequest("stream aggregate has no source".into())
                    })?
                    .Next()?;
                let Some(next) = next else {
                    self.finished = true;
                    break;
                };
                if eval_group_key(&self.groupByExprs, &next)? != key {
                    self.pending = Some(next);
                    break;
                }
                update_aggregates(&mut self.current_states, &self.aggExprs, &next)?;
            }
            Ok(Some(self.getPartialResult()))
        })();
        match &result {
            Ok(row) => self.execDetail.update(begin, row),
            Err(_) => self.execDetail.update(begin, &None),
        }
        result
    }
}
