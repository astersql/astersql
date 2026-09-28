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

// 游标（Cursor）与 RecordSet 的组合包装。
//
// 服务端游标持有打开结果集的会话侧句柄；关闭时必须先关游标再关底层 RecordSet，
// 以匹配 Go API：游标 Close 无返回值，仅 RecordSet Close 的错误对外可见。

use astersql_executor_internal_exec::executor::{Chunk, Executor};

use crate::recordset::{ChunkAllocator, RecordContext, RecordSet, Result, ResultField};

/// 游标句柄：关闭时释放服务端游标资源。
pub trait CursorHandle: Send {
    /// 关闭游标（Go API 无错误返回）。
    fn Close(&mut self);
}

/// 将游标句柄与底层 RecordSet 绑定，使两者生命周期一致。
pub struct cursorRecordSet {
    cursor: Box<dyn CursorHandle>,
    record_set: Box<dyn RecordSet>,
}

impl RecordSet for cursorRecordSet {
    /// 转发到底层结果集的字段列表。
    fn Fields(&self) -> Vec<ResultField> {
        self.record_set.Fields()
    }
    /// 转发拉取下一 chunk。
    fn Next(&mut self, ctx: &RecordContext, req: &mut Chunk) -> Result<()> {
        self.record_set.Next(ctx, req)
    }
    /// 转发创建输出 chunk。
    fn NewChunk(&self, allocator: Option<&dyn ChunkAllocator>) -> Chunk {
        self.record_set.NewChunk(allocator)
    }
    /// 先关游标再关 RecordSet；仅返回后者错误。
    fn Close(&mut self) -> Result<()> {
        // Cursor close is intentionally first and has no result in the Go API;
        // the wrapped record-set close is the only error returned.
        // 必须先关游标，避免会话侧句柄泄漏
        self.cursor.Close();
        self.record_set.Close()
    }
    /// 测试用：转发到底层执行器。
    fn GetExecutor4Test(&self) -> Option<&dyn Executor> {
        Some(
            self.record_set
                .GetExecutor4Test()
                .expect("wrapped record set does not expose GetExecutor4Test"),
        )
    }
}

/// 用游标包装已有 RecordSet，返回统一的结果集接口。
pub fn WrapRecordSetWithCursor(
    cursor: Box<dyn CursorHandle>,
    record_set: Box<dyn RecordSet>,
) -> Box<dyn RecordSet> {
    Box::new(cursorRecordSet { cursor, record_set })
}
