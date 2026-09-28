// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 异步数据流管道：按算子顺序 Open，逆序/全量 Close，并标记启动状态。
//
// `AsyncPipeline` 将多个 `Operator` 串成流水线。`Execute` 依次打开；
// 中途失败则对已打开算子逆序 Close 回滚。`GetReaderAndWriter` 约定
// 四算子管道中第 2、3 个为可调的 reader/writer（对应导入等典型拓扑）。

#![allow(dead_code, non_snake_case)]

use crate::operator::{Operator, TunableOperator};
use crate::workerpool::Error;
use std::sync::atomic::{AtomicBool, Ordering};

/// An asynchronous dataflow ordered from the first operator to the last.
///
/// 按从首到尾顺序排列的异步数据流管道。
pub struct AsyncPipeline {
    /// 管道内算子列表（顺序即数据流方向）。
    ops: Vec<Box<dyn Operator>>,
    /// 是否已成功 Execute（全部 Open 完成）。
    started: AtomicBool,
}

impl AsyncPipeline {
    /// 依次 Open 各算子；任一失败则逆序 Close 已打开者并返回错误。
    pub fn Execute(&mut self) -> Result<(), Error> {
        for index in 0..self.ops.len() {
            if let Err(error) = self.ops[index].Open() {
                // 回滚：对已成功 Open 的算子按逆序 Close，避免资源泄漏。
                for opened in (0..index).rev() {
                    let _ = self.ops[opened].Close();
                }
                return Err(error);
            }
        }
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 是否已成功启动（全部算子 Open 完成）。
    pub fn IsStarted(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// 关闭全部算子；若多个 Close 失败，仅返回第一个错误。
    pub fn Close(&mut self) -> Result<(), Error> {
        let mut first_error = None;
        for operator in &mut self.ops {
            // 继续关闭后续算子，但只保留首次遇到的错误。
            if let Err(error) = operator.Close()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.started.store(false, Ordering::SeqCst);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 以 `A -> B -> ...` 形式描述管道拓扑。
    pub fn String(&self) -> String {
        let names = self
            .ops
            .iter()
            .map(|operator| operator.String())
            .collect::<Vec<_>>();
        format!("AsyncPipeline[{}]", names.join(" -> "))
    }

    /// 仅当恰好 4 个算子时，返回第 2 个（reader）与第 3 个（writer）的可调接口。
    ///
    /// 对应 Go 侧固定拓扑：source → reader → writer → sink。
    pub fn GetReaderAndWriter(
        &mut self,
    ) -> (
        Option<&mut dyn TunableOperator>,
        Option<&mut dyn TunableOperator>,
    ) {
        if self.ops.len() != 4 {
            return (None, None);
        }
        // split_at_mut(2)：前半含 source/reader，后半含 writer/sink。
        let (through_reader, from_writer) = self.ops.split_at_mut(2);
        let reader = through_reader[1].as_tunable_operator_mut();
        let writer = from_writer[0].as_tunable_operator_mut();
        (reader, writer)
    }
}

/// 由算子列表构造尚未启动的 AsyncPipeline。
pub fn NewAsyncPipeline(ops: Vec<Box<dyn Operator>>) -> AsyncPipeline {
    AsyncPipeline {
        ops,
        started: AtomicBool::new(false),
    }
}
