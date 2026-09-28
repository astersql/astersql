// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 内存型简单结果集：构造时已持有全部行，按 chunk 大小分批吐出。
//
// 对应 Go `SimpleRecordSet`，实现 `RecordSet` 接口，供测试与无真实执行器场景使用。

#[cfg(feature = "formal-crate")]
use crate::{GoError, RecordChunk, RecordSet, chunk, context, resolve, types};

/// A `RecordSet` whose complete contents are known at construction time.
/// 构造时已完整可知内容的 `RecordSet`（结果集）。
pub struct SimpleRecordSet {
    /// 结果列元信息（ResultField）。
    pub ResultFields: Vec<resolve::ResultField>,
    /// 按行存储的原始值；每行是若干列的 `Any` 装箱。
    pub Rows: Vec<Vec<Box<dyn std::any::Any>>>,
    /// 单次 `Next` 可填充的最大行数（MaxChunkSize）。
    pub MaxChunkSize: usize,
    /// 下一次 `Next` 将读取的行下标。
    idx: usize,
}

impl SimpleRecordSet {
    /// 构造简单结果集；`idx` 从 0 开始。
    pub fn new(
        ResultFields: Vec<resolve::ResultField>,
        Rows: Vec<Vec<Box<dyn std::any::Any>>>,
        MaxChunkSize: usize,
    ) -> Self {
        Self {
            ResultFields,
            Rows,
            MaxChunkSize,
            idx: 0,
        }
    }
}

impl RecordSet for SimpleRecordSet {
    fn Fields(&self) -> &[resolve::ResultField] {
        &self.ResultFields
    }

    /// 将尚未读完的行写入 `req`，直到填满或耗尽；列值经 Datum 转换追加。
    fn Next(&mut self, _ctx: &context::Context, req: &mut RecordChunk) -> Result<(), GoError> {
        req.with_chunk_mut(|req| {
            req.Reset();
            // 逐行填充，直到 chunk 满或没有更多行。
            while self.idx < self.Rows.len() {
                if req.IsFull() {
                    return;
                }
                for column in 0..self.ResultFields.len() {
                    let datum = types::NewDatum(self.Rows[self.idx][column].as_ref());
                    req.AppendDatum(column, &datum);
                }
                self.idx += 1;
            }
        });
        Ok(())
    }

    /// 按结果列类型申请新 chunk；可选使用回收分配器（Allocator）。
    fn NewChunk(&self, alloc: Option<&mut dyn chunk::Allocator>) -> RecordChunk {
        let fields: Vec<Box<types::FieldType>> = self
            .ResultFields
            .iter()
            .map(|field| {
                Box::new(
                    field
                        .column
                        .as_ref()
                        .expect("SimpleRecordSet result field has no column")
                        .FieldType
                        .clone(),
                )
            })
            .collect();

        // 有分配器则走 Allocated 路径，否则直接 New Owned chunk。
        match alloc {
            Some(alloc) => RecordChunk::from_allocated(alloc.Alloc(&fields, 0, self.MaxChunkSize)),
            None => {
                RecordChunk::from_boxed(chunk::New(fields, self.MaxChunkSize, self.MaxChunkSize))
            }
        }
    }

    /// 关闭并重置读指针，允许再次从首行迭代。
    fn Close(&mut self) -> Result<(), GoError> {
        self.idx = 0;
        Ok(())
    }
}
