// Copyright 2026 AsterSQL.

//! Thread-safe protocol handles for canonical record sets owned by the session
//! worker. The existing lazy cursor supplies Current/Next/Error semantics.

use crate::conn::{ConnError, ConnResult, ProtocolResultSet, QueryResult, SessionState, Value};
use crate::runtime::{SessionRequest, protocol_value, result_metadata};
use astersql_server_internal_resultset as resultset;
use astersql_util_chunk as chunk;
use astersql_util_sqlexec as sqlexec;
#[cfg(test)]
#[path = "protocol_result_test_support.rs"]
mod test_support;
#[cfg(test)]
use test_support::BoundaryProbe;

use std::collections::HashMap;
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Chunk,
    Current,
    Advance,
    Finish,
    FetchReturned,
    Close,
}
pub(crate) enum OperationResult {
    Rows(Vec<Vec<Value>>),
    Row(Option<Vec<Value>>),
    Done,
}

#[derive(Debug)]
struct ResultHandle {
    id: u64,
    sender: Arc<mpsc::Sender<SessionRequest>>,
    closed: AtomicBool,
}
impl ResultHandle {
    fn request(&self, operation: Operation) -> ConnResult<OperationResult> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .send(SessionRequest::ResultOperation {
                id: self.id,
                operation,
                response: tx,
            })
            .map_err(|error| ConnError::Io(error.to_string()))?;
        rx.recv()
            .map_err(|error| ConnError::Io(error.to_string()))?
    }
}
impl ProtocolResultSet for ResultHandle {
    fn next_chunk(&self) -> ConnResult<Vec<Vec<Value>>> {
        match self.request(Operation::Chunk)? {
            OperationResult::Rows(rows) => Ok(rows),
            _ => unreachable!(),
        }
    }
    fn current_row(&self) -> ConnResult<Option<Vec<Value>>> {
        match self.request(Operation::Current)? {
            OperationResult::Row(row) => Ok(row),
            _ => unreachable!(),
        }
    }
    fn advance(&self) -> ConnResult<()> {
        self.request(Operation::Advance).map(|_| ())
    }
    fn finish(&self) -> ConnResult<()> {
        self.request(Operation::Finish).map(|_| ())
    }
    fn on_fetch_returned(&self) -> ConnResult<()> {
        self.request(Operation::FetchReturned).map(|_| ())
    }
    fn close(&self) -> ConnResult<()> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.request(Operation::Close)?;
        }
        Ok(())
    }
}
impl Drop for ResultHandle {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // Never wait in Drop: the worker may be shutting down. FIFO preserves
            // Close before the response lifecycle's statement completion request.
            let (tx, _) = mpsc::sync_channel(1);
            let _ = self.sender.send(SessionRequest::ResultOperation {
                id: self.id,
                operation: Operation::Close,
                response: tx,
            });
        }
    }
}

/// ConcreteRecordSet exposes canonical textual cells, including its NULL/binary
/// sentinels. Transport chunks retain these cells without guessing native types;
/// original column metadata stays separate and drives the existing wire encoder.
struct CanonicalResultSet {
    source: astersql_session::runtime::ConcreteRecordSet,
    columns: Vec<Arc<astersql_server_internal_column::Info>>,
    closed: bool,
    init_chunk_size: usize,
    max_chunk_size: usize,
    #[cfg(test)]
    probe: Arc<std::sync::Mutex<BoundaryProbe>>,
}
impl resultset::ResultSet for CanonicalResultSet {
    fn Columns(&mut self) -> Vec<Arc<astersql_server_internal_column::Info>> {
        self.columns.clone()
    }
    fn FieldTypes(&self) -> Vec<Box<chunk::types::FieldType>> {
        self.columns
            .iter()
            .map(|_| chunk::types::NewFieldType(chunk::mysql::TypeVarString))
            .collect()
    }
    fn NewChunk(&mut self, _: Option<&mut dyn chunk::Allocator>) -> sqlexec::RecordChunk {
        sqlexec::RecordChunk::from_boxed(chunk::New(
            self.FieldTypes(),
            self.init_chunk_size,
            self.max_chunk_size,
        ))
    }
    fn Next(
        &mut self,
        _: &sqlexec::context::Context,
        req: &mut sqlexec::RecordChunk,
    ) -> Result<(), sqlexec::GoError> {
        #[cfg(test)]
        self.probe.lock().unwrap().before("Next")?;
        let required = req.with_chunk(|chunk| chunk.RequiredRows());
        req.with_chunk_mut(|chunk| chunk.Reset());
        for _ in 0..required {
            let Some(row) = self.source.next_row()? else {
                break;
            };
            req.with_chunk_mut(|chunk| {
                for (index, cell) in row.iter().enumerate() {
                    chunk.AppendString(index, cell);
                }
            });
        }
        Ok(())
    }
    fn Close(&mut self) {
        if !self.closed {
            #[cfg(test)]
            {
                let _ = self.probe.lock().unwrap().before("Close");
            }
            let _ = self.source.close();
            self.closed = true;
        }
    }
    fn IsClosed(&self) -> bool {
        self.closed
    }
    fn SetPreparedStmt(&mut self, _: Option<resultset::PreparedStmtRef>) {}
    // CanonicalRecordSet is already executed and has no executor owner. These
    // optional hooks match a Go record set without Finish/FetchNotifier; the
    // wrapper still invokes them at the exact protocol boundary.
    fn Finish(&mut self) -> Result<(), sqlexec::GoError> {
        #[cfg(test)]
        self.probe.lock().unwrap().before("Finish")?;
        Ok(())
    }
    fn TryDetach(
        &mut self,
    ) -> Result<(Option<Box<dyn resultset::ResultSet>>, bool), sqlexec::GoError> {
        Ok((None, false))
    }
    fn OnFetchReturned(&mut self) {
        #[cfg(test)]
        {
            let _ = self.probe.lock().unwrap().before("FetchReturned");
        }
    }
    fn SetCursorRUV2Tracker(&mut self, _: Option<Arc<resultset::CursorRUV2Tracker>>) {}
    fn ReportCursorRUV2Delta(&mut self) {}
}
impl Drop for CanonicalResultSet {
    fn drop(&mut self) {
        resultset::ResultSet::Close(self);
    }
}

struct WorkerResult {
    regular: Option<Box<dyn resultset::ResultSet>>,
    cursor: Option<Box<dyn resultset::CursorResultSet>>,
    init_chunk_size: usize,
    max_chunk_size: usize,
}
impl WorkerResult {
    fn cursor(&mut self) -> &mut dyn resultset::CursorResultSet {
        if self.cursor.is_none() {
            self.cursor = Some(resultset::WrapWithLazyCursor(
                self.regular.take().expect("result set"),
                self.init_chunk_size,
                self.max_chunk_size,
            ));
        }
        self.cursor.as_deref_mut().expect("cursor")
    }
    fn source(&mut self) -> &mut dyn resultset::ResultSet {
        match self.cursor.as_mut() {
            Some(cursor) => cursor.as_mut(),
            None => self.regular.as_deref_mut().expect("result set"),
        }
    }
}

pub(crate) struct WorkerResults {
    results: HashMap<u64, WorkerResult>,
    next_id: u64,
    sender: std::sync::Weak<mpsc::Sender<SessionRequest>>,
    #[cfg(test)]
    probe: Arc<std::sync::Mutex<BoundaryProbe>>,
}
impl WorkerResults {
    pub(crate) fn new(sender: std::sync::Weak<mpsc::Sender<SessionRequest>>) -> Self {
        Self {
            results: HashMap::new(),
            next_id: 0,
            sender,
            #[cfg(test)]
            probe: Arc::new(std::sync::Mutex::new(BoundaryProbe::default())),
        }
    }
    pub(crate) fn register(
        &mut self,
        source: astersql_session::runtime::ConcreteRecordSet,
        state: SessionState,
        collation: u16,
        init_chunk_size: usize,
        max_chunk_size: usize,
    ) -> ConnResult<QueryResult> {
        let sender = self
            .sender
            .upgrade()
            .ok_or_else(|| ConnError::Session("session is closed".into()))?;
        let mut result = result_metadata(&source, state, collation);
        let columns = result
            .columns
            .iter()
            .map(|column| {
                Arc::new(astersql_server_internal_column::Info {
                    Schema: column.schema.clone(),
                    Table: column.table.clone(),
                    OrgTable: column.org_table.clone(),
                    Name: column.name.clone(),
                    OrgName: column.org_name.clone(),
                    ColumnLength: column.column_length,
                    Charset: column.charset,
                    Flag: column.flags,
                    Decimal: column.decimals,
                    Type: column.column_type,
                    DefaultValue: None,
                })
            })
            .collect();
        self.next_id += 1;
        let id = self.next_id;
        self.results.insert(
            id,
            WorkerResult {
                regular: Some(Box::new(CanonicalResultSet {
                    source,
                    columns,
                    closed: false,
                    init_chunk_size,
                    max_chunk_size,
                    #[cfg(test)]
                    probe: self.probe.clone(),
                })),
                cursor: None,
                init_chunk_size,
                max_chunk_size,
            },
        );
        result.result_set = Some(Arc::new(ResultHandle {
            id,
            sender,
            closed: AtomicBool::new(false),
        }));
        Ok(result)
    }
    pub(crate) fn operate(&mut self, id: u64, operation: Operation) -> ConnResult<OperationResult> {
        if operation == Operation::Close {
            if let Some(mut result) = self.results.remove(&id) {
                result.source().Close();
            }
            return Ok(OperationResult::Done);
        }
        let result = self
            .results
            .get_mut(&id)
            .ok_or_else(|| ConnError::Session("result set is closed".into()))?;
        let ctx = sqlexec::context::Context::new();
        match operation {
            Operation::Chunk => {
                let source = result.source();
                let mut chunk = source.NewChunk(None);
                source.Next(&ctx, &mut chunk).map_err(session_error)?;
                Ok(OperationResult::Rows(chunk.with_chunk(|chunk| {
                    (0..chunk.NumRows())
                        .map(|index| owned_row(chunk.GetRow(index)))
                        .collect()
                })))
            }
            Operation::Current | Operation::Advance => {
                let iter = result.cursor().GetRowIterator();
                let row = if operation == Operation::Current {
                    iter.Current(&ctx)
                } else {
                    iter.Next(&ctx)
                };
                if let Some(error) = iter.Error() {
                    return Err(ConnError::Session(error.to_string()));
                }
                if operation == Operation::Advance {
                    return Ok(OperationResult::Done);
                }
                Ok(OperationResult::Row(if row == iter.End() {
                    None
                } else {
                    Some(owned_row(row))
                }))
            }
            Operation::Finish => {
                result.source().Finish().map_err(session_error)?;
                Ok(OperationResult::Done)
            }
            Operation::FetchReturned => {
                result.source().OnFetchReturned();
                Ok(OperationResult::Done)
            }
            Operation::Close => unreachable!(),
        }
    }
}
fn session_error(error: impl fmt::Display) -> ConnError {
    ConnError::Session(error.to_string())
}
fn owned_row(row: chunk::Row) -> Vec<Value> {
    (0..row.Len())
        .map(|index| protocol_value(row.GetString(index)))
        .collect()
}
