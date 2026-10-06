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

// 静态 RecordSet：把底层 Executor 包装成可按 Chunk 拉取的结果集。
//
// RecordSet 是 SQL 执行层向会话/协议层交付查询结果的统一抽象：
// 调用方通过 [`RecordSet::Next`] 填满一个 Chunk（列式批处理缓冲），
// 再经 [`RecordSet::Close`] 释放执行器资源。本模块提供不依赖游标状态机的
// “静态”实现，直接委托给内部 Executor。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use astersql_executor_internal_exec::executor::{
    self as exec, Chunk, ExecContext, Executor, FieldType,
};

/// 与内部执行器共用的错误类型别名。
pub type Error = exec::Error;
/// 与内部执行器共用的 Result 别名。
pub type Result<T> = exec::Result<T>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 结果集列元信息（当前仅保留列名）。
pub struct ResultField {
    pub name: String,
}

#[derive(Clone, Debug, Default)]
/// 资源使用量（RU，Request Unit）读写明细，用于计量与限流。
pub struct RUDetails {
    pub read_ru: f64,
    pub write_ru: f64,
}

#[derive(Clone, Default)]
/// 拉取下一 Chunk 时携带的上下文：执行上下文 + 可选 RU 明细。
pub struct RecordContext {
    pub exec_context: ExecContext,
    pub ru_details: Option<Arc<RUDetails>>,
}
impl RecordContext {
    /// 从 source 继承 RU 明细，覆盖到当前上下文的克隆上。
    fn inherit(&self, source: &Self) -> Self {
        let mut inherited = self.clone();
        // 优先保留来源侧已绑定的 RU，避免会话侧空值冲掉。
        if let Some(details) = &source.ru_details {
            inherited.ru_details = Some(details.clone());
        }
        inherited
    }
}

/// Chunk 分配器：按列类型与容量预分配结果缓冲。
pub trait ChunkAllocator: Send + Sync {
    fn Alloc(&self, fields: &[FieldType], capacity: usize, max_size: usize) -> Chunk;
}

/// 结果集接口：列定义、按批拉取、建 Chunk、关闭。
pub trait RecordSet: Send {
    fn Fields(&self) -> Vec<ResultField>;
    fn Next(&mut self, ctx: &RecordContext, req: &mut Chunk) -> Result<()>;
    fn NewChunk(&self, allocator: Option<&dyn ChunkAllocator>) -> Chunk;
    fn Close(&mut self) -> Result<()>;
    /// 测试钩子：暴露内部 Executor；默认实现返回 None。
    fn GetExecutor4Test(&self) -> Option<&dyn Executor> {
        None
    }
}

/// 静态结果集：持有列定义、可选 Executor、SQL 原文与来源上下文。
pub struct staticRecordSet {
    fields: Vec<ResultField>,
    /// Close 后置为 None，防止重复拉取。
    executor: Option<Box<dyn Executor>>,
    sql_text: String,
    source_ctx: Option<RecordContext>,
}

/// 构造包装给定 Executor 的静态 RecordSet。
pub fn New(
    fields: Vec<ResultField>,
    executor: Box<dyn Executor>,
    sql_text: String,
    source_ctx: Option<RecordContext>,
) -> Box<dyn RecordSet> {
    Box::new(staticRecordSet {
        fields,
        executor: Some(executor),
        sql_text,
        source_ctx,
    })
}

impl RecordSet for staticRecordSet {
    fn Fields(&self) -> Vec<ResultField> {
        self.fields.clone()
    }

    fn Next(&mut self, ctx: &RecordContext, req: &mut Chunk) -> Result<()> {
        // 若构造时带了 source_ctx，则把其 RU 明细继承到本次调用上下文。
        let context = self
            .source_ctx
            .as_ref()
            .map_or_else(|| ctx.clone(), |source| ctx.inherit(source));
        let Some(executor) = self.executor.as_mut() else {
            return Err(Error::Other("record set is closed".into()));
        };
        // exec::Next already converts executor panics; this outer catch preserves the
        // record-set boundary for panics in adapters surrounding the executor.
        // 外层 catch_unwind：适配器层 panic 也统一映射为 Error::Panic。
        catch_unwind(AssertUnwindSafe(|| {
            exec::Next(&context.exec_context, executor.as_mut(), req)
        }))
        .unwrap_or(Err(Error::Panic))
    }

    fn NewChunk(&self, allocator: Option<&dyn ChunkAllocator>) -> Chunk {
        let Some(executor) = self.executor.as_ref() else {
            return Chunk::default();
        };
        // 无自定义分配器时走执行器默认首块；否则按返回列类型分配。
        match allocator {
            None => exec::NewFirstChunk(executor.as_ref()),
            Some(allocator) => allocator.Alloc(
                &executor.RetFieldTypes(),
                executor.InitCap(),
                executor.MaxChunkSize(),
            ),
        }
    }

    fn Close(&mut self) -> Result<()> {
        // take 掉 executor，保证幂等关闭。
        let Some(mut executor) = self.executor.take() else {
            return Ok(());
        };
        exec::Close(executor.as_mut())
    }

    fn GetExecutor4Test(&self) -> Option<&dyn Executor> {
        self.executor.as_deref()
    }
}
