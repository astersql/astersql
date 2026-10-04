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

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_executor::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, SchemaColumn,
};
use astersql_kv as kv;
use astersql_meta_model::{ColumnInfo, TableInfo};
use astersql_planner_core_base::Plan;
use astersql_util_chunk as chunk;
use protobuf::Message;

use super::{ConcreteSession, SessionError};

/// A production TableReader result stream. Unlike the snapshot iterator path,
/// rows and execution evidence come from the same TiKV DAG response.
pub(super) struct CanonicalTableReaderExecutor {
    domain: Arc<astersql_domain::Domain>,
    request: Option<kv::Request>,
    response: Option<Box<dyn kv::Response>>,
    columns: Vec<ColumnInfo>,
    schema: Vec<SchemaColumn>,
    pending: VecDeque<Vec<astersql_types::datum::Datum>>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
    scanned_rows: usize,
    plan_id: i32,
    evidence: kv::CopRuntimeEvidence,
    shared_evidence: Arc<std::sync::Mutex<Option<(i32, kv::CopRuntimeEvidence)>>>,
    opened: bool,
    closed: bool,
}

impl CanonicalTableReaderExecutor {
    pub(super) fn new(
        session: &ConcreteSession,
        reader: &astersql_planner_core_operator_physicalop::PhysicalTableReader,
        table: &TableInfo,
        columns: &[ColumnInfo],
        ranges: &[astersql_executor::typed_kv_scan::KeyRange],
        start_ts: u64,
        plan_id: i32,
        shared_evidence: Arc<std::sync::Mutex<Option<(i32, kv::CopRuntimeEvidence)>>>,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<Self, SessionError> {
        let mut request = super::relational_scan::relational_select_request(
            table,
            start_ts,
            kv::ReplicaReadType::ReplicaReadLeader,
            kv::GlobalTxnScope,
            false,
            session.connection_id(),
        )?;
        request.Data = super::relational_scan::planned_table_reader_select_dag(
            &reader.s_ctx().clone(),
            reader,
        )?;
        request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(
            ranges
                .iter()
                .map(|range| kv::KeyRange {
                    StartKey: range.start.clone(),
                    EndKey: range.end.clone(),
                })
                .collect(),
        ));
        request.ResourceGroupName = session.cop_resource_group_name();
        request.Paging.PagingSizeBytes = session.cop_paging_size_bytes(&request.ResourceGroupName);
        let columns = columns.to_vec();
        let schema = columns
            .iter()
            .map(|column| SchemaColumn {
                field_type: column.FieldType.clone(),
            })
            .collect();
        Ok(Self {
            domain: Arc::clone(&session.domain),
            request: Some(request),
            response: None,
            columns,
            schema,
            pending: VecDeque::new(),
            initial_capacity,
            maximum_chunk_size,
            scanned_rows: 0,
            plan_id,
            evidence: kv::CopRuntimeEvidence::default(),
            shared_evidence,
            opened: false,
            closed: false,
        })
    }

    fn fetch_response(&mut self) -> AdapterResult<bool> {
        let response = self
            .response
            .as_mut()
            .ok_or_else(|| errors::New("TableReader response is not open"))?;
        let context = kv::Context::todo();
        let Some(subset) = response.Next(&context)? else {
            self.publish_evidence();
            return Ok(false);
        };
        if let Some(evidence) = subset.CopRuntimeEvidence() {
            self.evidence.total_keys = self.evidence.total_keys.saturating_add(evidence.total_keys);
            self.evidence.processed_keys = self
                .evidence
                .processed_keys
                .saturating_add(evidence.processed_keys);
            self.evidence.processed_bytes = self
                .evidence
                .processed_bytes
                .saturating_add(evidence.processed_bytes);
            if let Some(bytes) = evidence.tikv_response_bytes {
                self.evidence.tikv_response_bytes = Some(
                    self.evidence
                        .tikv_response_bytes
                        .unwrap_or_default()
                        .saturating_add(bytes),
                );
            }
            self.publish_evidence();
        }
        let select: tipb::SelectResponse = protobuf::parse_from_bytes(subset.GetData())
            .map_err(|error| errors::New(format!("decode TableReader response: {error}")))?;
        if select.has_error() {
            return Err(errors::New(format!(
                "TiKV TableReader failed: {}",
                select.get_error().get_msg()
            )));
        }
        for response_chunk in select.get_chunks() {
            let mut encoded = response_chunk.get_rows_data();
            while !encoded.is_empty() {
                let mut row = Vec::with_capacity(self.columns.len());
                for _ in &self.columns {
                    let (remaining, datum) = astersql_tablecodec::codec::DecodeOne(encoded)?;
                    row.push(datum);
                    encoded = remaining;
                }
                self.pending.push_back(row);
            }
        }
        Ok(true)
    }

    fn publish_evidence(&self) {
        *self.shared_evidence.lock().unwrap() = Some((self.plan_id, self.evidence));
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(errors::New("TableReader executor is not open"));
        }
        output.Reset();
        while !output.IsFull() {
            if let Some(row) = self.pending.pop_front() {
                for (offset, datum) in row.iter().enumerate() {
                    output.AppendDatum(offset, datum);
                }
                self.scanned_rows += 1;
                continue;
            }
            if let Some(killer) = context.and_then(|context| context.sql_killer.as_ref()) {
                killer.HandleSignal()?;
            }
            if !self.fetch_response()? {
                break;
            }
        }
        Ok(())
    }
}

impl ExecExecutor for CanonicalTableReaderExecutor {
    fn Open(&mut self) -> AdapterResult {
        if self.opened && !self.closed {
            return Ok(());
        }
        let request = self
            .request
            .take()
            .ok_or_else(|| errors::New("TableReader request was already consumed"))?;
        let response = self.domain.storage().with_storage(|store| {
            store.GetClient().Send(
                &kv::Context::todo(),
                &request,
                &() as &dyn std::any::Any,
                &kv::ClientSendOption {
                    SessionMemTracker: None,
                    EnabledRateLimitAction: false,
                    EventCb: None,
                    EnableCollectExecutionInfo: true,
                    TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
                    AppendWarning: None,
                    TryCopLiteWorker: None,
                },
            )
        });
        self.response =
            Some(response.ok_or_else(|| errors::New("TiKV returned no TableReader response"))?);
        self.pending.clear();
        self.scanned_rows = 0;
        self.evidence = kv::CopRuntimeEvidence::default();
        self.opened = true;
        self.closed = false;
        Ok(())
    }

    fn Close(&mut self) -> AdapterResult {
        self.closed = true;
        self.pending.clear();
        let result = self
            .response
            .take()
            .map_or(Ok(()), |mut response| response.Close());
        self.publish_evidence();
        result
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.next_inner(None, output)
    }

    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.next_inner(Some(context), output)
    }

    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig {
            fields: self
                .schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect(),
            initial_capacity: self.initial_capacity,
            maximum_chunk_size: self.maximum_chunk_size,
        }
    }

    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(
            self.columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect::<Vec<_>>(),
            self.initial_capacity,
            self.maximum_chunk_size,
        )
    }

    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        Vec::new()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _duration: Duration) {}
    fn ScannedRows(&self) -> usize {
        self.scanned_rows
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
