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

// 窗口执行器构建器。
//
// 根据物理窗口计划（分区、排序、帧类型、是否启用流水线）选择
// `PipelinedWindowExec` 或缓冲式 `WindowExec`，并装配对应的 WindowProcessor。

use std::collections::VecDeque;

use crate::pipelined_window::{OrderedWindowExec, PipelinedWindowExec};
use crate::window::{
    AggWindowProcessor, BoundType, ChildExecutor, Chunk, Error, ExecContext, FrameBound, FrameType,
    GroupChecker, OrderBy, RangeFrameWindowProcessor, Result, RowFrameWindowProcessor, WindowExec,
    WindowFrame, WindowFunction, WindowMemoryTracker,
};

/// 物理窗口计划片段：schema、分区键、排序键、窗口函数与帧定义。
pub struct PhysicalWindowPlan {
    /// 输出总列数（输入列 + 窗口函数结果列）。
    pub schema_columns: usize,
    /// PARTITION BY 列下标。
    pub partition_by: Vec<usize>,
    /// ORDER BY 键（列与升降序）。
    pub order_by: Vec<OrderBy>,
    /// 本节点要计算的窗口函数列表。
    pub window_functions: Vec<Box<dyn WindowFunction>>,
    /// 窗口帧；None 表示整分区聚合。
    pub frame: Option<WindowFrame>,
    /// 是否优先构建流水线窗口执行器。
    pub pipelined_enabled: bool,
}

/// 构建结果：流水线或缓冲式窗口执行器。
pub enum WindowExecutor {
    /// 流水线路径。
    Pipelined(PipelinedWindowExec),
    /// 缓冲整分区后再计算的路径。
    Buffered(WindowExec),
}

impl WindowExecutor {
    /// 打开所选执行器。
    pub fn open(&mut self, context: &ExecContext) -> Result<()> {
        match self {
            Self::Pipelined(executor) => executor.open(context),
            Self::Buffered(executor) => executor.open(context),
        }
    }

    /// 拉取下一批带窗口结果的行。
    pub fn next(&mut self, context: &ExecContext, output: &mut Chunk) -> Result<()> {
        match self {
            Self::Pipelined(executor) => executor.next(context, output),
            Self::Buffered(executor) => executor.next(context, output),
        }
    }

    /// 关闭所选执行器。
    pub fn close(&mut self) -> Result<()> {
        match self {
            Self::Pipelined(executor) => executor.close(),
            Self::Buffered(executor) => executor.close(),
        }
    }

    pub fn memory_bytes(&self) -> i64 {
        match self {
            Self::Pipelined(executor) => executor.memory_bytes(),
            Self::Buffered(executor) => executor.memory_bytes(),
        }
    }
}

/// 构建有序流水线窗口（OrderedWindowExec）；非流水线结果视为错误。
pub fn build_ordered(
    plan: PhysicalWindowPlan,
    child: Box<dyn ChildExecutor>,
) -> Result<OrderedWindowExec> {
    match build(plan, child, true)? {
        WindowExecutor::Pipelined(inner) => Ok(OrderedWindowExec { inner }),
        WindowExecutor::Buffered(_) => Err(Error::new(
            "ordered window must be built with pipelined window executor",
        )),
    }
}

/// 按计划构建窗口执行器；`force_pipelined` 可强制走流水线路径。
pub fn build(
    mut plan: PhysicalWindowPlan,
    child: Box<dyn ChildExecutor>,
    force_pipelined: bool,
) -> Result<WindowExecutor> {
    // 输入列数 = schema 总列 - 窗口函数结果列。
    let function_count = plan.window_functions.len();
    let memory_tracker = WindowMemoryTracker::new(
        plan.window_functions
            .iter()
            .map(|function| function.initial_partial_result_memory_usage())
            .collect(),
    );
    let input_columns = plan
        .schema_columns
        .checked_sub(function_count)
        .ok_or_else(|| Error::new("window function count exceeds schema columns"))?;
    let pipelined = force_pipelined || plan.pipelined_enabled;

    // 流水线：默认帧为无界 PRECEDING..FOLLOWING；RANGE 需绑定比较列。
    if pipelined {
        let (mut start, mut end, range_frame) = match plan.frame.take() {
            None => (
                FrameBound::unbounded(BoundType::Preceding),
                FrameBound::unbounded(BoundType::Following),
                false,
            ),
            Some(frame) => {
                let range = frame.frame_type == FrameType::Range;
                (frame.start, frame.end, range)
            }
        };
        // RANGE 帧按 ORDER BY 值比较边界，需先填充 compare_columns。
        if range_frame {
            start.update_compare_cols(&plan.order_by)?;
            end.update_compare_cols(&plan.order_by)?;
        }
        return Ok(WindowExecutor::Pipelined(PipelinedWindowExec {
            child,
            input_columns,
            window_functions: plan.window_functions,
            start,
            end,
            group_checker: GroupChecker::new(plan.partition_by),
            child_result: None,
            data: VecDeque::new(),
            data_index: 0,
            done: false,
            accumulated: 0,
            dropped: 0,
            rows_to_consume: 0,
            new_partition: false,
            current_row: 0,
            last_start_row: 0,
            last_end_row: 0,
            staged_start_row: 0,
            staged_end_row: 0,
            row_start: 0,
            order_by: plan.order_by,
            rows: Vec::new(),
            row_count: 0,
            whole: false,
            range_frame,
            empty_frame: false,
            initialized_sliding_window: false,
            memory_tracker,
            data_memory: 0,
            rows_memory: 0,
        }));
    }

    // 缓冲路径：按帧类型选择聚合/ROWS/RANGE 处理器。
    let processor: Box<dyn crate::window::WindowProcessor> = match plan.frame {
        // 无帧：整分区一次聚合。
        None => Box::new(AggWindowProcessor {
            window_functions: plan.window_functions,
            memory_tracker: memory_tracker.clone(),
        }),
        // ROWS 帧：按行偏移滑动窗口。
        Some(frame) if frame.frame_type == FrameType::Rows => Box::new(RowFrameWindowProcessor {
            window_functions: plan.window_functions,
            start: frame.start,
            end: frame.end,
            current_row: 0,
            initialized_sliding_window: false,
            memory_tracker: memory_tracker.clone(),
        }),
        // RANGE 帧：按排序键值域滑动窗口。
        Some(mut frame) => {
            frame.start.update_compare_cols(&plan.order_by)?;
            frame.end.update_compare_cols(&plan.order_by)?;
            Box::new(RangeFrameWindowProcessor {
                window_functions: plan.window_functions,
                start: frame.start,
                end: frame.end,
                current_row: 0,
                last_start_offset: 0,
                last_end_offset: 0,
                order_by: plan.order_by,
                initialized_sliding_window: false,
                memory_tracker: memory_tracker.clone(),
            })
        }
    };
    Ok(WindowExecutor::Buffered(WindowExec {
        child,
        group_checker: GroupChecker::new(plan.partition_by),
        child_result: None,
        executed: false,
        result_chunks: VecDeque::new(),
        remaining_rows_in_chunk: VecDeque::new(),
        input_columns,
        processor,
        memory_tracker,
        result_queue_memory: 0,
    }))
}
