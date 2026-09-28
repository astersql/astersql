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

// 惰性游标测试：使用真实 LazyCursor 和可观察关闭状态的 ResultSet 边界。
//
// Lazy Cursor 按需从底层 ResultSet 拉取 Chunk，避免一次物化全部行。

#![allow(non_snake_case)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 按固定步长产出 Int64 行的测试 ResultSet，用于跨 Chunk 边界验证。
struct BatchResultSet {
    next: i64,
    end: i64,
    batch_size: usize,
    closed: Arc<AtomicBool>,
}

impl astersql_server_internal_resultset::ResultSet for BatchResultSet {
    fn Columns(&mut self) -> Vec<std::sync::Arc<astersql_server_internal_column::Info>> {
        Vec::new()
    }

    fn NewChunk(
        &mut self,
        _allocator: Option<&mut dyn astersql_util_chunk::Allocator>,
    ) -> astersql_util_sqlexec::RecordChunk {
        astersql_util_sqlexec::RecordChunk::from_boxed(astersql_util_chunk::NewChunkWithCapacity(
            vec![astersql_util_chunk::types::NewFieldType(
                astersql_parser_mysql::r#type::TypeLonglong,
            )],
            2,
        ))
    }

    fn Next(
        &mut self,
        _ctx: &astersql_util_sqlexec::context::Context,
        request: &mut astersql_util_sqlexec::RecordChunk,
    ) -> Result<(), astersql_util_sqlexec::GoError> {
        request.with_chunk_mut(|chunk| {
            chunk.Reset();
            for _ in 0..self.batch_size {
                if self.next >= self.end {
                    break;
                }
                chunk.AppendInt64(0, self.next);
                self.next += 1;
            }
        });
        Ok(())
    }

    fn Close(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn IsClosed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn FieldTypes(&self) -> Vec<Box<astersql_util_chunk::types::FieldType>> {
        vec![astersql_util_chunk::types::NewFieldType(
            astersql_parser_mysql::r#type::TypeLonglong,
        )]
    }

    fn SetPreparedStmt(
        &mut self,
        _stmt: Option<astersql_server_internal_resultset::PreparedStmtRef>,
    ) {
    }

    fn Finish(&mut self) -> Result<(), astersql_util_sqlexec::GoError> {
        Ok(())
    }

    fn TryDetach(
        &mut self,
    ) -> Result<
        (
            Option<Box<dyn astersql_server_internal_resultset::ResultSet>>,
            bool,
        ),
        astersql_util_sqlexec::GoError,
    > {
        Ok((None, false))
    }

    fn OnFetchReturned(&mut self) {}

    fn SetCursorRUV2Tracker(
        &mut self,
        _tracker: Option<std::sync::Arc<astersql_server_internal_resultset::CursorRUV2Tracker>>,
    ) {
    }

    fn ReportCursorRUV2Delta(&mut self, _result_chunk_cells_delta: i64) {}
}

#[test]
/// 对齐 Go `TestLazyRowIterator`：四种 Chunk 配置均逐行拉取 0..1000。
fn lazy_cursor_fetches_across_real_chunk_boundaries() {
    use astersql_server_internal_resultset::WrapWithLazyCursor;

    for (initial_size, maximum_size) in [(1024, 1024), (512, 512), (256, 256), (100, 100)] {
        let closed = Arc::new(AtomicBool::new(false));
        let source = BatchResultSet {
            next: 0,
            end: 1000,
            batch_size: maximum_size,
            closed: closed.clone(),
        };
        let mut cursor = WrapWithLazyCursor(Box::new(source), initial_size, maximum_size);
        let context = astersql_util_sqlexec::context::Context::new();
        let iterator = cursor.GetRowIterator();

        for expected in 0_i64..1000 {
            let row = iterator.Current(&context);
            assert_eq!(row.GetInt64(0), expected);
            let next = iterator.Next(&context);
            if expected == 999 {
                assert!(next.IsEmpty());
            } else {
                assert_eq!(next.GetInt64(0), expected + 1);
            }
        }
        assert!(iterator.Current(&context).IsEmpty());
        assert!(iterator.Error().is_none());
        iterator.Close();
        assert!(closed.load(Ordering::SeqCst));
    }
}
