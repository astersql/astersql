// Copyright 2026 AsterSQL.

// `binaryplan` casetest crate 入口。
//
// Binary Plan 会同时写入 slow log 与 statement summary。这里的公共模型保留 Go
// 用例所验证的状态转换，测试文件负责构造用例和断言，避免把测试逻辑藏在测试
// 文件里的私有玩具函数中。

#![allow(dead_code)]

/// Go `stmtsummary.MaxEncodedPlanSizeInBytes`。
pub const MAX_ENCODED_PLAN_SIZE_IN_BYTES: usize = 1024 * 1024;

/// 执行计划算子的任务类型；它决定 runtime stats 应位于哪个字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskType {
    Root,
    Cop,
    Unknown,
}

/// Go `tipb.ExplainOperator` 中本任务会清理或校验的字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryPlanOperator {
    pub name: String,
    pub task_type: TaskType,
    pub root_basic_exec_info: Option<String>,
    pub root_group_exec_info: Option<Vec<String>>,
    pub cop_exec_info: Option<String>,
    pub access_objects: Option<Vec<String>>,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub children: Vec<BinaryPlanOperator>,
}

impl BinaryPlanOperator {
    /// 对齐 Go `simplifyAndCheckBinaryOperator` 的稳定化规则。
    pub fn simplify_for_comparison(&mut self, with_runtime_stats: bool) -> Result<(), String> {
        if with_runtime_stats {
            match self.task_type {
                TaskType::Root
                    if self
                        .root_basic_exec_info
                        .as_deref()
                        .unwrap_or_default()
                        .is_empty() =>
                {
                    return Err(format!("root operator {} has no runtime stats", self.name));
                }
                TaskType::Cop if self.cop_exec_info.as_deref().unwrap_or_default().is_empty() => {
                    return Err(format!("cop operator {} has no runtime stats", self.name));
                }
                TaskType::Unknown | TaskType::Root | TaskType::Cop => {}
            }
        }

        self.root_basic_exec_info = None;
        self.root_group_exec_info = None;
        self.cop_exec_info = None;

        if is_access_operator(&self.name) && self.access_objects.is_none() {
            return Err(format!(
                "access operator {} has no access objects",
                self.name
            ));
        }
        // AccessObject is an interface in Go and is intentionally excluded from the
        // JSON comparison after the presence check above.
        self.access_objects = None;
        // These values are explicitly unstable in the Go test.
        self.memory_bytes = 0;
        self.disk_bytes = 0;

        for child in &mut self.children {
            child.simplify_for_comparison(with_runtime_stats)?;
        }
        Ok(())
    }
}

/// Go 的 `((Table|Index).*Scan)|CTEFullScan|Point_Get` 匹配规则。
pub fn is_access_operator(name: &str) -> bool {
    ["Table", "Index"].iter().any(|prefix| {
        name.find(prefix)
            .is_some_and(|start| name[start + prefix.len()..].contains("Scan"))
    }) || name.contains("CTEFullScan")
        || name.contains("Point_Get")
}

/// `tipb.ExplainData` 的本任务相关字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryPlan {
    pub main: Option<BinaryPlanOperator>,
    pub ctes: Vec<BinaryPlanOperator>,
    pub with_runtime_stats: bool,
    pub discarded_due_to_too_long: bool,
}

impl BinaryPlan {
    /// 清理不稳定字段并保留 Go 测试中的必需字段断言。
    pub fn simplify_for_comparison(&mut self) -> Result<(), String> {
        if let Some(main) = &mut self.main {
            main.simplify_for_comparison(self.with_runtime_stats)?;
        }
        for cte in &mut self.ctes {
            cte.simplify_for_comparison(self.with_runtime_stats)?;
        }
        Ok(())
    }

    /// statement summary 超过上限时只保留 discarded 标记，和 Go 的 nil Main/Ctes 对齐。
    pub fn discarded_for_summary() -> Self {
        Self {
            main: None,
            ctes: Vec::new(),
            with_runtime_stats: false,
            discarded_due_to_too_long: true,
        }
    }
}

/// slow log 与 statement summary 的写入结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryPlanDestinations {
    pub slow_log: Option<BinaryPlan>,
    pub statement_summary: Option<BinaryPlan>,
}

/// 对齐 Go `tidb_generate_binary_plan` 和 statement-summary 大小分支。
pub fn route_binary_plan(
    plan: &BinaryPlan,
    generate_binary_plan: bool,
    encoded_size: usize,
    max_encoded_size: usize,
) -> BinaryPlanDestinations {
    if !generate_binary_plan {
        return BinaryPlanDestinations {
            slow_log: None,
            statement_summary: None,
        };
    }

    BinaryPlanDestinations {
        slow_log: Some(plan.clone()),
        statement_summary: Some(if encoded_size > max_encoded_size {
            BinaryPlan::discarded_for_summary()
        } else {
            plan.clone()
        }),
    }
}

/// 编解码错误；Go 测试链路中的所有解析错误都必须可报告，不能 panic。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryPlanCodecError {
    Truncated,
    InvalidOperatorLength,
    InvalidUtf8,
    InvalidRuntimeRows,
}

/// 测试用的稳定摘要载荷，保留 Go binary-plan 核心测试验证的 operator/rows 信息。
pub fn encode_binary_plan(operator: &str, rows: u64) -> Result<Vec<u8>, BinaryPlanCodecError> {
    let len =
        u32::try_from(operator.len()).map_err(|_| BinaryPlanCodecError::InvalidOperatorLength)?;
    let mut payload = len.to_be_bytes().to_vec();
    payload.extend_from_slice(operator.as_bytes());
    payload.extend_from_slice(&rows.to_be_bytes());
    Ok(payload)
}

/// 解析摘要载荷，并显式校验长度、UTF-8 和尾部的 u64。
pub fn decode_binary_plan(payload: &[u8]) -> Result<(String, u64), BinaryPlanCodecError> {
    if payload.len() < 4 {
        return Err(BinaryPlanCodecError::Truncated);
    }
    let len = u32::from_be_bytes(
        payload[..4]
            .try_into()
            .map_err(|_| BinaryPlanCodecError::Truncated)?,
    ) as usize;
    let operator_end = 4usize
        .checked_add(len)
        .ok_or(BinaryPlanCodecError::InvalidOperatorLength)?;
    let rows_end = operator_end
        .checked_add(8)
        .ok_or(BinaryPlanCodecError::Truncated)?;
    if payload.len() < rows_end {
        return Err(BinaryPlanCodecError::Truncated);
    }
    let operator = String::from_utf8(payload[4..operator_end].to_vec())
        .map_err(|_| BinaryPlanCodecError::InvalidUtf8)?;
    let rows = u64::from_be_bytes(
        payload[operator_end..rows_end]
            .try_into()
            .map_err(|_| BinaryPlanCodecError::InvalidRuntimeRows)?,
    );
    if payload.len() != rows_end {
        return Err(BinaryPlanCodecError::InvalidRuntimeRows);
    }
    Ok((operator, rows))
}

/// Binary Plan 核心编解码往返测试。
#[cfg(test)]
mod binary_plan_core_test;
/// Binary Plan 在 statement summary 中的大小裁剪测试。
#[cfg(test)]
mod binary_plan_test;
/// 对应 Go TestMain 的初始化顺序断言。
#[cfg(test)]
mod main_test;
