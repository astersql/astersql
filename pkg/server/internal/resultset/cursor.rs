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

// 游标结果集封装：将 ResultSet 适配为逐行迭代器。
//
// 提供基于 RowContainer 的预物化游标，以及按需从底层 ResultSet 拉取
// chunk 的 lazy 游标；两者均通过宏转发 ResultSet 接口，供 COM_STMT_FETCH
// / 服务端游标场景使用。

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;

use astersql_server_internal_column::Info;
use astersql_util_chunk as chunk;
use astersql_util_sqlexec as sqlexec;

use crate::{CursorRUV2Tracker, PreparedStmtRef, ResultSet};

/// 行迭代器错误类型（可跨线程共享）。
pub type RowIteratorError = Arc<dyn Error + Send + Sync>;
/// 共享的 ResultSet（Rc+RefCell，供游标与迭代器共同持有）。
type SharedResultSet = Rc<RefCell<Box<dyn ResultSet>>>;

/// 结果集行迭代器：支持 Next/Current/End，以及错误查询与关闭。
pub trait RowIterator {
    /// 前进到下一行并返回；耗尽时返回 End 行。
    fn Next(&mut self, ctx: &sqlexec::context::Context) -> chunk::Row;
    /// 返回当前行；尚未开始迭代时等价于首次 Next。
    fn Current(&mut self, ctx: &sqlexec::context::Context) -> chunk::Row;
    /// 表示迭代结束的哨兵空行。
    fn End(&self) -> chunk::Row;
    /// 迭代过程中捕获的错误（若有）。
    fn Error(&self) -> Option<RowIteratorError>;
    /// 关闭底层资源。
    fn Close(&mut self);
}

/// 带行迭代器能力的结果集（游标协议侧入口）。
pub trait CursorResultSet: ResultSet {
    /// 取得可变行迭代器引用。
    fn GetRowIterator(&mut self) -> &mut dyn RowIterator;
}

/// 基于已物化 RowContainer 的游标结果集。
pub struct TidbCursorResultSet {
    /// 被包装的原始 ResultSet。
    result_set: SharedResultSet,
    /// 从 RowContainer 读取的行迭代器。
    reader: RowContainerReaderIter,
}

/// 用 RowContainerReader 包装 ResultSet，得到可逐行遍历的游标。
pub fn WrapWithRowContainerCursor(
    result_set: Box<dyn ResultSet>,
    row_container: Box<dyn chunk::row_container_reader::RowContainerReader>,
) -> Box<dyn CursorResultSet> {
    Box::new(TidbCursorResultSet {
        result_set: Rc::new(RefCell::new(result_set)),
        reader: RowContainerReaderIter {
            reader: row_container,
        },
    })
}

impl CursorResultSet for TidbCursorResultSet {
    fn GetRowIterator(&mut self) -> &mut dyn RowIterator {
        &mut self.reader
    }
}

/// RowContainerReader 到 RowIterator 的适配器。
pub struct RowContainerReaderIter {
    reader: Box<dyn chunk::row_container_reader::RowContainerReader>,
}

impl RowIterator for RowContainerReaderIter {
    fn Next(&mut self, _ctx: &sqlexec::context::Context) -> chunk::Row {
        self.reader.Next()
    }

    fn Current(&mut self, _ctx: &sqlexec::context::Context) -> chunk::Row {
        self.reader.Current()
    }

    fn End(&self) -> chunk::Row {
        self.reader.End()
    }

    fn Error(&self) -> Option<RowIteratorError> {
        self.reader
            .Error()
            .map(|error| Arc::new(error) as RowIteratorError)
    }

    fn Close(&mut self) {
        self.reader.Close();
    }
}

/// Fetch 返回后的通知回调（由协议层在 COM_STMT_FETCH 完成后触发）。
pub trait FetchNotifier {
    fn OnFetchReturned(&mut self);
}

/// 按需从底层 ResultSet 拉取 chunk 的 lazy 游标结果集。
pub struct TidbLazyCursorResultSet {
    result_set: SharedResultSet,
    iter: LazyRowIterator,
}

/// 包装 ResultSet 为 lazy 游标：按 capacity/max_chunk_size 分配缓冲 chunk，
/// 迭代时不足则调用底层 Next 填充。
pub fn WrapWithLazyCursor(
    result_set: Box<dyn ResultSet>,
    capacity: usize,
    max_chunk_size: usize,
) -> Box<dyn CursorResultSet> {
    let result_set = Rc::new(RefCell::new(result_set));
    // 按结果集字段类型预分配 chunk，供迭代器复用。
    let field_types = result_set.borrow().FieldTypes();
    let chunk = sqlexec::RecordChunk::from_boxed(chunk::New(field_types, capacity, max_chunk_size));
    Box::new(TidbLazyCursorResultSet {
        result_set: Rc::clone(&result_set),
        iter: LazyRowIterator {
            result_set,
            error: None,
            chunk,
            index_in_chunk: 0,
            started: false,
        },
    })
}

impl CursorResultSet for TidbLazyCursorResultSet {
    fn GetRowIterator(&mut self) -> &mut dyn RowIterator {
        &mut self.iter
    }
}

/// Lazy 行迭代器：在当前 chunk 内前进，耗尽后向 ResultSet 再取一批。
pub struct LazyRowIterator {
    result_set: SharedResultSet,
    error: Option<RowIteratorError>,
    chunk: sqlexec::RecordChunk,
    index_in_chunk: usize,
    /// 是否已至少调用过一次 Next（影响 Current 行为）。
    started: bool,
}

impl RowIterator for LazyRowIterator {
    fn Next(&mut self, ctx: &sqlexec::context::Context) -> chunk::Row {
        self.started = true;
        self.index_in_chunk = self.index_in_chunk.saturating_add(1);

        // 当前 chunk 用尽：拉取下一批；失败或空批则返回 End。
        if self.index_in_chunk >= self.chunk.NumRows() {
            if let Err(error) = self.result_set.borrow_mut().Next(ctx, &mut self.chunk) {
                self.error = Some(Arc::from(error));
                return self.End();
            }
            if self.chunk.NumRows() == 0 {
                return self.End();
            }
            self.index_in_chunk = 0;
        }

        self.chunk
            .with_chunk(|chunk| chunk.GetRow(self.index_in_chunk))
    }

    fn Current(&mut self, ctx: &sqlexec::context::Context) -> chunk::Row {
        // 首次 Current 自动触发一次 Next，对齐 Go 游标语义。
        if !self.started {
            return self.Next(ctx);
        }
        if self.chunk.NumRows() == 0 {
            return self.End();
        }
        self.chunk
            .with_chunk(|chunk| chunk.GetRow(self.index_in_chunk))
    }

    fn End(&self) -> chunk::Row {
        chunk::Row::default()
    }

    fn Error(&self) -> Option<RowIteratorError> {
        self.error.clone()
    }

    fn Close(&mut self) {
        self.result_set.borrow_mut().Close();
    }
}

/// 将 ResultSet 全部方法转发到底层 `result_set` 字段，避免游标包装重复实现。
macro_rules! impl_result_set_forwarder {
    ($type:ty) => {
        impl ResultSet for $type {
            fn Columns(&mut self) -> Vec<Arc<Info>> {
                self.result_set.borrow_mut().Columns()
            }

            fn NewChunk(
                &mut self,
                allocator: Option<&mut dyn chunk::Allocator>,
            ) -> sqlexec::RecordChunk {
                self.result_set.borrow_mut().NewChunk(allocator)
            }

            fn Next(
                &mut self,
                ctx: &sqlexec::context::Context,
                req: &mut sqlexec::RecordChunk,
            ) -> Result<(), sqlexec::GoError> {
                self.result_set.borrow_mut().Next(ctx, req)
            }

            fn Close(&mut self) {
                self.result_set.borrow_mut().Close();
            }

            fn IsClosed(&self) -> bool {
                self.result_set.borrow().IsClosed()
            }

            fn FieldTypes(&self) -> Vec<Box<chunk::types::FieldType>> {
                self.result_set.borrow().FieldTypes()
            }

            fn SetPreparedStmt(&mut self, stmt: Option<PreparedStmtRef>) {
                self.result_set.borrow_mut().SetPreparedStmt(stmt);
            }

            fn Finish(&mut self) -> Result<(), sqlexec::GoError> {
                self.result_set.borrow_mut().Finish()
            }

            fn TryDetach(
                &mut self,
            ) -> Result<(Option<Box<dyn ResultSet>>, bool), sqlexec::GoError> {
                self.result_set.borrow_mut().TryDetach()
            }

            fn OnFetchReturned(&mut self) {
                self.result_set.borrow_mut().OnFetchReturned();
            }

            fn SetCursorRUV2Tracker(&mut self, tracker: Option<Arc<CursorRUV2Tracker>>) {
                self.result_set.borrow_mut().SetCursorRUV2Tracker(tracker);
            }

            fn ReportCursorRUV2Delta(&mut self, result_chunk_cells_delta: i64) {
                self.result_set
                    .borrow_mut()
                    .ReportCursorRUV2Delta(result_chunk_cells_delta);
            }
        }
    };
}

impl_result_set_forwarder!(TidbCursorResultSet);
impl_result_set_forwarder!(TidbLazyCursorResultSet);
