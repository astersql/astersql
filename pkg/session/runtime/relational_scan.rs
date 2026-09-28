// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Relational record/index access paths, coprocessor reads, and transaction overlays.

use super::*;

impl ConcreteSession {
    pub(super) fn scan_relational_ann_rows(
        &self,
        table: &astersql_meta_model::TableInfo,
        index: &astersql_meta_model::IndexInfo,
        reference: &str,
        top_k: u32,
    ) -> SessionResult<Option<Vec<RelationalRow>>> {
        self.domain.storage().with_storage(|store| {
            if store.Name() != "TiKV" {
                return Ok(None);
            }
            let version = store
                .CurrentVersion(kv::GlobalTxnScope)
                .map_err(|error| session_error("ANN read TSO", error))?;
            let vector_column = table
                .Columns
                .iter()
                .find(|column| {
                    index
                        .Columns
                        .first()
                        .is_some_and(|indexed| indexed.Name.L == column.Name.L)
                })
                .ok_or_else(|| SessionError::new("ANN index vector column is missing"))?;
            let vector = astersql_types::vector::ParseVectorFloat32(reference)
                .map_err(|error| session_error("parse ANN reference vector", error))?;
            let mut ann = tipb::AnnQueryInfo::new();
            ann.set_query_type(tipb::AnnQueryType::OrderBy);
            ann.set_distance_metric(tipb::VectorDistanceMetric::L2);
            ann.set_top_k(top_k);
            ann.set_column_name(vector_column.Name.L.clone());
            ann.set_index_id(index.ID);
            ann.set_ref_vec_f32(vector.SerializeTo(Vec::new()));
            ann.set_column(relational_scan_column(table, vector_column));
            let mut index_info = tipb::ColumnarIndexInfo::new();
            index_info.set_index_type(tipb::ColumnarIndexType::TypeVector);
            index_info.set_ann_query_info(ann);
            let mut scan = tipb::TableScan::new();
            scan.set_table_id(table.ID);
            scan.set_columns(
                table
                    .Columns
                    .iter()
                    .map(|column| relational_scan_column(table, column))
                    .collect::<Vec<_>>()
                    .into(),
            );
            scan.set_used_columnar_indexes(vec![index_info].into());
            let mut executor = tipb::Executor::new();
            executor.set_tp(tipb::ExecType::TypeTableScan);
            executor.set_tbl_scan(scan);
            let mut dag = tipb::DagRequest::new();
            dag.set_executors(vec![executor].into());
            dag.set_output_offsets((0..table.Columns.len() as u32).collect());
            let mut request = relational_coprocessor_request(table, version.Ver)?;
            request.Data = protobuf::Message::write_to_bytes(&dag)
                .map_err(|error| session_error("encode ANN DAG", error))?;
            request.StoreType = kv::StoreType::TiFlash;
            request.BatchCop = true;
            let context = kv::Context::todo();
            let option = kv::ClientSendOption {
                SessionMemTracker: None,
                EnabledRateLimitAction: false,
                EventCb: None,
                EnableCollectExecutionInfo: false,
                TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
                AppendWarning: None,
                TryCopLiteWorker: None,
            };
            let mut response = store
                .GetClient()
                .Send(&context, &request, &(), &option)
                .ok_or_else(|| SessionError::new("TiFlash returned no ANN response"))?;
            let result = (|| {
                let mut rows = Vec::new();
                while let Some(subset) = response
                    .Next(&context)
                    .map_err(|error| session_error("execute TiFlash ANN scan", error))?
                {
                    let result: tipb::SelectResponse = protobuf::parse_from_bytes(subset.GetData())
                        .map_err(|error| session_error("decode TiFlash ANN response", error))?;
                    if result.has_error() {
                        return Err(SessionError::new(format!(
                            "TiFlash ANN scan failed: {}",
                            result.get_error().get_msg()
                        )));
                    }
                    for chunk in result.get_chunks() {
                        let mut encoded = chunk.get_rows_data();
                        while !encoded.is_empty() {
                            let mut row = HashMap::new();
                            for column in &table.Columns {
                                let (remaining, datum) =
                                    astersql_tablecodec::codec::DecodeOne(encoded).map_err(
                                        |error| session_error("decode TiFlash ANN datum", error),
                                    )?;
                                row.insert(
                                    column.Name.L.clone(),
                                    datum_to_runtime_value(&datum, Some(column))?,
                                );
                                encoded = remaining;
                            }
                            rows.push((0, row));
                        }
                    }
                }
                Ok(rows)
            })();
            let close = response
                .Close()
                .map_err(|error| session_error("close TiFlash ANN response", error));
            match (result, close) {
                (Err(error), _) | (_, Err(error)) => Err(error),
                (Ok(rows), Ok(())) => Ok(Some(rows)),
            }
        })
    }
}

/// 扫描表前缀得到关系行列表。
pub(super) fn scan_relational_rows(
    retriever: &dyn kv::Retriever,
    table: &astersql_meta_model::TableInfo,
) -> SessionResult<Vec<RelationalRow>> {
    scan_relational_rows_with_limit(retriever, table, None)
}

/// 仅推进表记录键迭代器统计行数，不读取、复制或解码行值。
///
/// 与 Go TableReader/HashAgg 的分块消费一致，内存占用不随表行数增长。
pub(crate) fn count_relational_rows(
    retriever: &dyn kv::Retriever,
    table: &astersql_meta_model::TableInfo,
) -> SessionResult<usize> {
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    let mut iterator = retriever
        .Iter(prefix.clone(), Some(prefix.PrefixNext()))
        .map_err(|error| session_error("count relational rows", error))?;
    let result = (|| {
        let mut count = 0_usize;
        while iterator.Valid() {
            count = count
                .checked_add(1)
                .ok_or_else(|| SessionError::new("relational row count overflow"))?;
            iterator
                .Next()
                .map_err(|error| session_error("advance relational row count", error))?;
        }
        Ok(count)
    })();
    iterator.Close();
    result
}

pub(super) fn relational_scan_column(
    table: &astersql_meta_model::TableInfo,
    column: &astersql_meta_model::ColumnInfo,
) -> tipb::ColumnInfo {
    let field_type = astersql_expression::ToPBFieldType(&column.FieldType);
    let mut scan_column = tipb::ColumnInfo::new();
    scan_column.set_column_id(column.ID);
    scan_column.set_tp(field_type.get_tp());
    scan_column.set_collation(field_type.get_collate());
    scan_column.set_column_len(field_type.get_flen());
    scan_column.set_decimal(field_type.get_decimal());
    scan_column.set_flag(field_type.get_flag() as i32);
    scan_column.set_elems(protobuf::RepeatedField::from_vec(
        field_type.get_elems().to_vec(),
    ));
    scan_column.set_array(column.FieldType.IsArray());
    scan_column.set_pk_handle(
        table.PKIsHandle
            && table
                .GetPkColInfo()
                .is_some_and(|primary| primary.ID == column.ID),
    );
    scan_column
}

pub(crate) fn relational_count_dag(
    table: &astersql_meta_model::TableInfo,
) -> SessionResult<Vec<u8>> {
    let source_column = table
        .GetPkColInfo()
        .filter(|_| table.PKIsHandle)
        .or_else(|| table.Columns.first())
        .ok_or_else(|| SessionError::new("relational COUNT table has no scan column"))?;
    let scan_column = relational_scan_column(table, source_column);

    let mut scan = tipb::TableScan::new();
    scan.set_table_id(table.ID);
    scan.set_columns(protobuf::RepeatedField::from_vec(vec![scan_column]));

    let mut executors = Vec::with_capacity(2);
    let mut scan_executor = tipb::Executor::new();
    scan_executor.set_tp(tipb::ExecType::TypeTableScan);
    scan_executor.set_tbl_scan(scan);
    executors.push(scan_executor);

    let mut one_type = tipb::FieldType::new();
    one_type.set_tp(astersql_parser_mysql::r#type::TypeLonglong as i32);
    let mut one = tipb::Expr::new();
    one.set_tp(tipb::ExprType::Int64);
    one.set_val(astersql_tablecodec::codec::EncodeInt(Vec::new(), 1));
    one.set_field_type(one_type);

    let mut count_type = tipb::FieldType::new();
    count_type.set_tp(astersql_parser_mysql::r#type::TypeLonglong as i32);
    count_type.set_flag(astersql_parser_mysql::r#type::UnsignedFlag as u32);
    let mut count = tipb::Expr::new();
    count.set_tp(tipb::ExprType::Count);
    count.set_children(protobuf::RepeatedField::from_vec(vec![one]));
    count.set_field_type(count_type);
    count.set_agg_func_mode(tipb::AggFunctionMode::Partial1Mode);

    let mut aggregation = tipb::Aggregation::new();
    aggregation.set_agg_func(protobuf::RepeatedField::from_vec(vec![count]));
    let mut aggregate_executor = tipb::Executor::new();
    aggregate_executor.set_tp(tipb::ExecType::TypeAggregation);
    aggregate_executor.set_aggregation(aggregation);
    executors.push(aggregate_executor);

    let mut dag = tipb::DagRequest::new();
    dag.set_executors(protobuf::RepeatedField::from_vec(executors));
    dag.set_output_offsets(vec![0]);
    protobuf::Message::write_to_bytes(&dag)
        .map_err(|error| session_error("encode relational COUNT DAG", error))
}

pub(super) fn physical_table_reader(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
) -> Option<&astersql_planner_core_operator_physicalop::PhysicalTableReader> {
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>()
    {
        return Some(reader);
    }
    plan.children().into_iter().find_map(physical_table_reader)
}

/// 按 Go `FlattenListPushDownPlan + ConstructDAGReq` 把 TableReader 子树编码为 TiKV DAG。
pub(super) fn planned_table_reader_dag(
    plan_context: &astersql_planner_core_base::ContextRef,
    reader: &astersql_planner_core_operator_physicalop::PhysicalTableReader,
) -> SessionResult<Vec<u8>> {
    let table_plan = reader
        .TablePlan
        .as_deref()
        .ok_or_else(|| SessionError::new("planned TableReader has no table plan"))?;
    let mut build_context = plan_context.GetBuildPBCtx().clone();
    let mut executors = Vec::new();
    for operator in astersql_planner_core_operator_physicalop::FlattenListPushDownPlan(table_plan) {
        let executor = operator
            .to_pb(&mut build_context, kv::StoreType::TiKV)
            .map_err(|error| {
                let detail = operator
                    .as_any()
                    .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalSelection>()
                    .and_then(|selection| selection.Conditions.first())
                    .and_then(|condition| {
                        condition
                            .as_any()
                            .downcast_ref::<astersql_expression::ScalarFunction>()
                    })
                    .map(|function| {
                        format!(
                            " (function={}, pb_code={}, return_type={}, args={})",
                            function.FuncName.L,
                            function.Function.PbCode(),
                            function.RetType.is_some(),
                            function.GetArgs().len()
                        )
                    })
                    .unwrap_or_default();
                SessionError::new(format!("encode planned TiKV executor: {error}{detail}"))
            })?;
        executors.push(*executor);
    }
    if matches!(
        executors.last().map(tipb::Executor::get_tp),
        Some(tipb::ExecType::TypeTableScan | tipb::ExecType::TypeSelection)
    ) {
        let mut one_type = tipb::FieldType::new();
        one_type.set_tp(astersql_parser_mysql::r#type::TypeLonglong as i32);
        let mut one = tipb::Expr::new();
        one.set_tp(tipb::ExprType::Int64);
        one.set_val(astersql_tablecodec::codec::EncodeInt(Vec::new(), 1));
        one.set_field_type(one_type);
        let mut count_type = tipb::FieldType::new();
        count_type.set_tp(astersql_parser_mysql::r#type::TypeLonglong as i32);
        count_type.set_flag(astersql_parser_mysql::r#type::UnsignedFlag as u32);
        let mut count = tipb::Expr::new();
        count.set_tp(tipb::ExprType::Count);
        count.set_children(protobuf::RepeatedField::from_vec(vec![one]));
        count.set_field_type(count_type);
        count.set_agg_func_mode(tipb::AggFunctionMode::Partial1Mode);
        let mut aggregation = tipb::Aggregation::new();
        aggregation.set_agg_func(protobuf::RepeatedField::from_vec(vec![count]));
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeAggregation);
        executor.set_aggregation(aggregation);
        executors.push(executor);
    }
    if !matches!(
        executors.first().map(tipb::Executor::get_tp),
        Some(tipb::ExecType::TypeTableScan)
    ) || !matches!(
        executors.last().map(tipb::Executor::get_tp),
        Some(tipb::ExecType::TypeAggregation | tipb::ExecType::TypeStreamAgg)
    ) {
        return Err(SessionError::new(format!(
            "planned scalar COUNT did not produce TableScan-to-Aggregation TiKV executors: {:?}",
            executors
                .iter()
                .map(tipb::Executor::get_tp)
                .collect::<Vec<_>>()
        )));
    }
    let mut dag = tipb::DagRequest::new();
    dag.set_executors(executors.into());
    dag.set_output_offsets(vec![0]);
    protobuf::Message::write_to_bytes(&dag)
        .map_err(|error| session_error("encode planned relational COUNT DAG", error))
}

/// 构造只扫描表记录范围的 TiKV Checksum 请求载荷。
///
/// 聚簇表的每条可见记录对应 record prefix 下一个 KV；Checksum 直接在 MVCC
/// 快照上累计 `total_kvs`，避免 TableScan 为 COUNT(*) 解码整行。
pub(crate) fn relational_count_checksum() -> SessionResult<Vec<u8>> {
    let mut checksum = tipb::ChecksumRequest::new();
    checksum.set_scan_on(tipb::ChecksumScanOn::Table);
    checksum.set_algorithm(tipb::ChecksumAlgorithm::Crc64Xor);
    protobuf::Message::write_to_bytes(&checksum)
        .map_err(|error| session_error("encode relational COUNT checksum", error))
}

/// 解码一个或多个 Region 返回的 COUNT 部分结果。
pub(crate) fn decode_relational_count_response(data: &[u8]) -> SessionResult<usize> {
    let response: tipb::SelectResponse = protobuf::parse_from_bytes(data)
        .map_err(|error| session_error("decode relational COUNT response", error))?;
    if response.has_error() {
        return Err(SessionError::new(format!(
            "TiKV relational COUNT failed: {}",
            response.get_error().get_msg()
        )));
    }
    let mut total = 0_usize;
    for chunk in response.get_chunks() {
        let mut encoded = chunk.get_rows_data();
        while !encoded.is_empty() {
            let (remaining, datum) = astersql_tablecodec::codec::DecodeOne(encoded)
                .map_err(|error| session_error("decode relational COUNT datum", error))?;
            let partial = match datum.Kind() {
                astersql_tablecodec::types::KindInt64 if datum.GetInt64() >= 0 => {
                    datum.GetInt64() as u64
                }
                astersql_tablecodec::types::KindUint64 => datum.GetUint64(),
                kind => {
                    return Err(SessionError::new(format!(
                        "TiKV relational COUNT returned datum kind {kind}"
                    )));
                }
            };
            total = total
                .checked_add(usize::try_from(partial).map_err(|_| {
                    SessionError::new("TiKV relational COUNT exceeds addressable row count")
                })?)
                .ok_or_else(|| SessionError::new("relational row count overflow"))?;
            encoded = remaining;
        }
    }
    Ok(total)
}

/// 解码单个 Region 的 Checksum 结果，只取精确可见 KV 数。
pub(crate) fn decode_relational_checksum_count_response(data: &[u8]) -> SessionResult<usize> {
    let response: tipb::ChecksumResponse = protobuf::parse_from_bytes(data)
        .map_err(|error| session_error("decode relational COUNT checksum response", error))?;
    usize::try_from(response.get_total_kvs())
        .map_err(|_| SessionError::new("TiKV relational COUNT exceeds addressable row count"))
}

pub(super) fn relational_coprocessor_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
) -> SessionResult<kv::Request> {
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    relational_coprocessor_range_request(table, start_ts, prefix.clone(), prefix.PrefixNext())
}

pub(super) fn relational_select_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    replica_read: kv::ReplicaReadType,
    txn_scope: &str,
    stale_read: bool,
    connection_id: u64,
) -> SessionResult<kv::Request> {
    relational_table_scan_request(
        table,
        start_ts,
        None,
        replica_read,
        txn_scope,
        stale_read,
        connection_id,
    )
}

pub(super) fn relational_analyze_select_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    concurrency: i32,
    replica_read: kv::ReplicaReadType,
    txn_scope: &str,
    stale_read: bool,
    connection_id: u64,
) -> SessionResult<kv::Request> {
    relational_table_scan_request(
        table,
        start_ts,
        Some(concurrency),
        replica_read,
        txn_scope,
        stale_read,
        connection_id,
    )
}

fn relational_table_scan_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    concurrency: Option<i32>,
    replica_read: kv::ReplicaReadType,
    txn_scope: &str,
    stale_read: bool,
    connection_id: u64,
) -> SessionResult<kv::Request> {
    let mut request = relational_coprocessor_request(table, start_ts)?;
    let mut scan = tipb::TableScan::new();
    scan.set_table_id(table.ID);
    scan.set_columns(protobuf::RepeatedField::from_vec(
        table
            .Columns
            .iter()
            .map(|column| relational_scan_column(table, column))
            .collect(),
    ));
    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeTableScan);
    executor.set_tbl_scan(scan);
    let mut dag = tipb::DagRequest::new();
    dag.set_executors(protobuf::RepeatedField::from_vec(vec![executor]));
    dag.set_output_offsets((0..table.Columns.len() as u32).collect());
    request.Data = protobuf::Message::write_to_bytes(&dag)
        .map_err(|error| session_error("encode relational TableScan DAG", error))?;
    let physical_ids = table
        .GetPartitionInfo()
        .map(|partition| {
            partition
                .Definitions
                .iter()
                .map(|definition| definition.ID)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    request.KeyRanges = if physical_ids.is_empty() {
        let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
        Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: prefix.clone(),
            EndKey: prefix.PrefixNext(),
        }]))
    } else {
        Some(kv::NewPartitionedKeyRanges(
            physical_ids
                .iter()
                .map(|physical_id| {
                    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(*physical_id).0);
                    vec![kv::KeyRange {
                        StartKey: prefix.clone(),
                        EndKey: prefix.PrefixNext(),
                    }]
                })
                .collect(),
        ))
    };
    request.Concurrency = concurrency.unwrap_or_else(|| {
        let partition_num = request
            .KeyRanges
            .as_ref()
            .map_or(1, kv::KeyRanges::PartitionNum);
        i32::try_from(
            partition_num.min(astersql_sessionctx_vardef::DefDistSQLScanConcurrency as usize),
        )
        .unwrap_or(astersql_sessionctx_vardef::DefDistSQLScanConcurrency as i32)
    });
    request.ReplicaRead = replica_read;
    request.TxnScope = txn_scope.to_owned();
    request.ReadReplicaScope = txn_scope.to_owned();
    request.IsStaleness = stale_read;
    request.ConnID = connection_id;
    Ok(request)
}

fn scan_relational_rows_with_coprocessor(
    store: &dyn kv::Storage,
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    concurrency: i32,
    replica_read: kv::ReplicaReadType,
    txn_scope: &str,
    connection_id: u64,
) -> SessionResult<Option<Vec<RelationalRow>>> {
    let client = store.GetClient();
    if !client.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic) {
        return Ok(None);
    }
    let request = relational_analyze_select_request(
        table,
        start_ts,
        concurrency,
        replica_read,
        txn_scope,
        false,
        connection_id,
    )?;
    let context = kv::Context::todo();
    let option = kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    };
    let mut response = client
        .Send(&context, &request, &(), &option)
        .ok_or_else(|| SessionError::new("TiKV client returned no ANALYZE response"))?;
    let result = (|| {
        let mut rows = Vec::new();
        while let Some(subset) = response
            .Next(&context)
            .map_err(|error| session_error("execute ANALYZE TableScan in TiKV", error))?
        {
            let result: tipb::SelectResponse = protobuf::parse_from_bytes(subset.GetData())
                .map_err(|error| session_error("decode ANALYZE TableScan response", error))?;
            if result.has_error() {
                return Err(SessionError::new(format!(
                    "TiKV ANALYZE TableScan failed: {}",
                    result.get_error().get_msg()
                )));
            }
            for chunk in result.get_chunks() {
                let mut encoded = chunk.get_rows_data();
                while !encoded.is_empty() {
                    let mut row = HashMap::new();
                    for column in &table.Columns {
                        let (remaining, datum) = astersql_tablecodec::codec::DecodeOne(encoded)
                            .map_err(|error| {
                                session_error("decode ANALYZE TableScan datum", error)
                            })?;
                        row.insert(
                            column.Name.L.clone(),
                            datum_to_runtime_value(&datum, Some(column))?,
                        );
                        encoded = remaining;
                    }
                    rows.push((0, row));
                }
            }
        }
        Ok(rows)
    })();
    let close_result = response
        .Close()
        .map_err(|error| session_error("close ANALYZE TableScan response", error));
    match (result, close_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(rows), Ok(())) => Ok(Some(rows)),
    }
}

pub(super) fn relational_coprocessor_range_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    start_key: kv::Key,
    end_key: kv::Key,
) -> SessionResult<kv::Request> {
    Ok(kv::Request {
        Tp: kv::ReqTypeDAG,
        StartTs: start_ts,
        Data: relational_count_dag(table)?,
        KeyRanges: Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: start_key,
            EndKey: end_key,
        }])),
        PartitionIDAndRanges: Vec::new(),
        Concurrency: RELATIONAL_COP_SCAN_CONCURRENCY,
        CoprRequestRateLimit: None,
        IsolationLevel: kv::IsoLevel::SI,
        Priority: kv::PriorityNormal,
        MemTracker: None,
        KeepOrder: false,
        Desc: false,
        NotFillCache: false,
        ReplicaRead: kv::ReplicaReadType::ReplicaReadLeader,
        StoreType: kv::StoreType::TiKV,
        Cacheable: true,
        SchemaVar: 0,
        BatchCop: false,
        TaskID: 0,
        TiDBServerID: 0,
        TxnScope: kv::GlobalTxnScope.to_owned(),
        ReadReplicaScope: String::new(),
        IsStaleness: false,
        ClosestReplicaReadAdjuster: None,
        MatchStoreLabels: Vec::new(),
        ResourceGroupTagger: None,
        Paging: kv::Paging::default(),
        RequestSource: kv::util::RequestSource::default(),
        StoreBatchSize: 0,
        ResourceGroupName: "default".to_owned(),
        LimitSize: 0,
        StoreBusyThreshold: Duration::ZERO,
        TiKVClientReadTimeout: 0,
        MaxExecutionTime: 0,
        MaxKeysRead: 0,
        MaxKeysReadCounter: None,
        RunawayChecker: None,
        ResourceControlInterceptor: None,
        ConnID: 0,
        ConnAlias: String::new(),
    })
}

pub(super) fn relational_checksum_request(
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
) -> SessionResult<kv::Request> {
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    relational_checksum_range_request(start_ts, prefix.clone(), prefix.PrefixNext())
}

pub(super) fn relational_checksum_range_request(
    start_ts: u64,
    start_key: kv::Key,
    end_key: kv::Key,
) -> SessionResult<kv::Request> {
    Ok(kv::Request {
        Tp: kv::ReqTypeChecksum,
        StartTs: start_ts,
        Data: relational_count_checksum()?,
        KeyRanges: Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: start_key,
            EndKey: end_key,
        }])),
        PartitionIDAndRanges: Vec::new(),
        Concurrency: RELATIONAL_COP_SCAN_CONCURRENCY,
        CoprRequestRateLimit: None,
        IsolationLevel: kv::IsoLevel::SI,
        Priority: kv::PriorityNormal,
        MemTracker: None,
        KeepOrder: false,
        Desc: false,
        // 对齐 TiDB Checksum：大范围精确计数不污染 TiKV block cache。
        NotFillCache: true,
        ReplicaRead: kv::ReplicaReadType::ReplicaReadLeader,
        StoreType: kv::StoreType::TiKV,
        Cacheable: false,
        SchemaVar: 0,
        BatchCop: false,
        TaskID: 0,
        TiDBServerID: 0,
        TxnScope: kv::GlobalTxnScope.to_owned(),
        ReadReplicaScope: String::new(),
        IsStaleness: false,
        ClosestReplicaReadAdjuster: None,
        MatchStoreLabels: Vec::new(),
        ResourceGroupTagger: None,
        Paging: kv::Paging::default(),
        RequestSource: kv::util::RequestSource::default(),
        StoreBatchSize: 0,
        ResourceGroupName: "default".to_owned(),
        LimitSize: 0,
        StoreBusyThreshold: Duration::ZERO,
        TiKVClientReadTimeout: 0,
        MaxExecutionTime: 0,
        MaxKeysRead: 0,
        MaxKeysReadCounter: None,
        RunawayChecker: None,
        ResourceControlInterceptor: None,
        ConnID: 0,
        ConnAlias: String::new(),
    })
}

pub(super) fn execute_relational_count_request(
    client: &dyn kv::Client,
    context: &kv::Context,
    request: &kv::Request,
    option: &kv::ClientSendOption,
    checksum: bool,
) -> SessionResult<usize> {
    let mut response = client
        .Send(context, request, &(), option)
        .ok_or_else(|| SessionError::new("TiKV client returned no response"))?;
    let result = (|| {
        let mut count = 0_usize;
        while let Some(subset) = response
            .Next(context)
            .map_err(|error| session_error("execute relational COUNT in TiKV", error))?
        {
            count = count
                .checked_add(if checksum {
                    decode_relational_checksum_count_response(subset.GetData())?
                } else {
                    decode_relational_count_response(subset.GetData())?
                })
                .ok_or_else(|| SessionError::new("relational row count overflow"))?;
        }
        Ok(count)
    })();
    let close_result = response
        .Close()
        .map_err(|error| session_error("close relational COUNT response", error));
    match (result, close_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(count), Ok(())) => Ok(count),
    }
}

pub(super) fn count_relational_rows_with_coprocessor(
    store: &dyn kv::Storage,
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    checker: Option<kv::resourcegroup::SharedRunawayChecker>,
    resource_group_name: &str,
) -> SessionResult<Option<usize>> {
    let client = store.GetClient();
    if !client.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic) {
        return Ok(None);
    }
    let context = kv::Context::todo();
    let option = kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    };

    // RequestTypeSupportedChecker 对齐 Go，只描述表达式下推能力，因此不会报告
    // Checksum；真实 TiKV transport 仍支持该请求。失败时回退标准 HashAgg DAG。
    let mut checksum_request = relational_checksum_request(table, start_ts)?;
    checksum_request.RunawayChecker = checker.clone();
    checksum_request.ResourceGroupName = resource_group_name.to_owned();
    match execute_relational_count_request(client, &context, &checksum_request, &option, true) {
        Ok(count) => Ok(Some(count)),
        Err(_) => {
            let mut dag_request = relational_coprocessor_request(table, start_ts)?;
            dag_request.RunawayChecker = checker;
            dag_request.ResourceGroupName = resource_group_name.to_owned();
            execute_relational_count_request(client, &context, &dag_request, &option, false)
                .map(Some)
        }
    }
}

/// 在 TiKV 侧精确统计一个记录键范围，只把每个 Region 的部分计数返回 root。
///
/// 大 OFFSET 的整数 handle seek 用该计数补偿主键空洞；Checksum 不可用时
/// 回退与 COUNT(*) 相同的 TableScan + HashAgg DAG，均不会回传逐行 key/value。
pub(super) fn count_relational_key_range_with_coprocessor(
    store: &dyn kv::Storage,
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    start_key: kv::Key,
    end_key: kv::Key,
    checker: Option<kv::resourcegroup::SharedRunawayChecker>,
    resource_group_name: &str,
) -> SessionResult<Option<usize>> {
    if start_key.Cmp(&end_key) >= 0 {
        return Ok(Some(0));
    }
    let client = store.GetClient();
    if !client.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic) {
        return Ok(None);
    }
    let context = kv::Context::todo();
    let option = kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    };
    let mut checksum_request =
        relational_checksum_range_request(start_ts, start_key.clone(), end_key.clone())?;
    checksum_request.RunawayChecker = checker.clone();
    checksum_request.ResourceGroupName = resource_group_name.to_owned();
    match execute_relational_count_request(client, &context, &checksum_request, &option, true) {
        Ok(count) => Ok(Some(count)),
        Err(_) => {
            let mut dag_request =
                relational_coprocessor_range_request(table, start_ts, start_key, end_key)?;
            dag_request.RunawayChecker = checker;
            dag_request.ResourceGroupName = resource_group_name.to_owned();
            execute_relational_count_request(client, &context, &dag_request, &option, false)
                .map(Some)
        }
    }
}

pub(super) fn count_relational_rows_with_planned_filter_coprocessor(
    store: &dyn kv::Storage,
    table: &astersql_meta_model::TableInfo,
    start_ts: u64,
    data: Vec<u8>,
    checker: Option<kv::resourcegroup::SharedRunawayChecker>,
    resource_group_name: &str,
) -> SessionResult<Option<usize>> {
    let client = store.GetClient();
    if !client.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic) {
        return Ok(None);
    }
    let mut request = relational_coprocessor_request(table, start_ts)?;
    request.Data = data;
    request.RunawayChecker = checker;
    request.ResourceGroupName = resource_group_name.to_owned();
    let context = kv::Context::todo();
    let option = kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    };
    execute_relational_count_request(client, &context, &request, &option, false).map(Some)
}

/// 扫描表前缀，并在安全的无排序 SELECT 中限制解码行数。
pub(super) fn scan_relational_rows_with_limit(
    retriever: &dyn kv::Retriever,
    table: &astersql_meta_model::TableInfo,
    row_limit: Option<usize>,
) -> SessionResult<Vec<RelationalRow>> {
    scan_relational_rows_window(retriever, table, 0, row_limit.unwrap_or(usize::MAX), None)
}

/// 消费一个关系表记录键迭代器，先跳过 OFFSET，再解码最终需要返回的行。
pub(super) fn decode_relational_row_value(
    table: &astersql_meta_model::TableInfo,
    fields: &HashMap<i64, Box<astersql_tablecodec::types::FieldType>>,
    handle: &dyn astersql_tablecodec::kv::Handle,
    value: &[u8],
) -> SessionResult<RelationalRow> {
    let datums = astersql_tablecodec::DecodeRowToDatumMap(
        Some(value.to_vec()),
        fields.clone(),
        Some(astersql_tablecodec::time::UTC),
    )
    .map_err(|error| session_error("decode relational row", error))?;
    let mut row = HashMap::new();
    for column in &table.Columns {
        // A column added by `ALTER TABLE ADD COLUMN` is absent from the
        // rows written before the DDL; Go fills those in from the origin
        // default value instead of backfilling the record values.
        let value = match datums.get(&column.ID) {
            Some(datum) => datum_to_runtime_value(datum, Some(column))?,
            None => origin_default_runtime_value(column),
        };
        row.insert(column.Name.L.clone(), value);
        if column.GetType() == astersql_parser_mysql::r#type::TypeFloat {
            row.insert(
                relational_float32_column_marker(&column.Name.L),
                Some("1".to_owned()),
            );
        }
        if column.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32 {
            row.insert(
                relational_vector_column_marker(&column.Name.L),
                Some("1".to_owned()),
            );
        }
        if matches!(
            column.GetType(),
            astersql_parser_mysql::r#type::TypeString
                | astersql_parser_mysql::r#type::TypeVarchar
                | astersql_parser_mysql::r#type::TypeVarString
                | astersql_parser_mysql::r#type::TypeBlob
                | astersql_parser_mysql::r#type::TypeTinyBlob
                | astersql_parser_mysql::r#type::TypeMediumBlob
                | astersql_parser_mysql::r#type::TypeLongBlob
        ) {
            row.insert(
                relational_string_column_marker(&column.Name.L),
                Some("1".to_owned()),
            );
        }
    }
    for column in table
        .Columns
        .iter()
        .filter(|column| column.IsGenerated() && !column.GeneratedStored)
    {
        let expression = crate::dml_runtime::ParseGeneratedExpr(&column.GeneratedExprString)?;
        let value = crate::dml_runtime::EvalExpr(&expression, &row, None)?;
        row.insert(column.Name.L.clone(), value);
    }
    if !table.PKIsHandle && !table.IsCommonHandle && table.GetAutoIncrementColInfo().is_none() {
        row.insert(
            "_tidb_rowid".to_owned(),
            Some(handle.IntValue().to_string()),
        );
    }
    Ok((handle.IsInt().then(|| handle.IntValue()).unwrap_or(0), row))
}

pub(super) fn consume_relational_row_iterator(
    iterator: &mut dyn kv::Iterator,
    table: &astersql_meta_model::TableInfo,
    fields: &HashMap<i64, Box<astersql_tablecodec::types::FieldType>>,
    remaining_offset: &mut usize,
    count: usize,
    rows: &mut Vec<RelationalRow>,
) -> SessionResult<()> {
    while iterator.Valid() && *remaining_offset != 0 {
        iterator
            .Next()
            .map_err(|error| session_error("advance relational OFFSET", error))?;
        *remaining_offset -= 1;
    }
    while iterator.Valid() && rows.len() < count {
        let key = iterator.Key();
        let (_, handle) =
            astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(key.0.clone()))
                .map_err(|error| session_error("decode relational record key", error))?;
        rows.push(decode_relational_row_value(
            table,
            fields,
            handle.as_ref(),
            &iterator.Value(),
        )?);
        iterator
            .Next()
            .map_err(|error| session_error("advance relational rows", error))?;
    }
    Ok(())
}

pub(crate) type RelationalRowScanRange = (kv::Key, kv::Key, bool);

#[derive(Clone, Debug)]
pub(crate) struct RelationalSecondaryIndexAccess {
    pub(crate) index: astersql_meta_model::IndexInfo,
    pub(crate) ranges: Vec<RelationalRowScanRange>,
}

#[derive(Clone, Debug)]
pub(crate) struct RelationalIndexMergeAccess {
    pub(crate) branches: Vec<RelationalSecondaryIndexAccess>,
    pub(crate) intersection: bool,
}

fn local_secondary_index_table_ids(table: &astersql_meta_model::TableInfo) -> Vec<i64> {
    table
        .GetPartitionInfo()
        .filter(|partition| !partition.Definitions.is_empty())
        .map(|partition| {
            partition
                .Definitions
                .iter()
                .map(|definition| definition.ID)
                .collect()
        })
        .unwrap_or_else(|| vec![table.ID])
}

pub(super) fn secondary_index_cursor_parts<'a>(
    expression: &'a ast::ExprNode,
) -> Option<Vec<&'a ast::ExprNode>> {
    match &expression.Kind {
        ast::ExprKind::Parentheses(inner) => secondary_index_cursor_parts(inner),
        ast::ExprKind::Row(values) => Some(values.iter().collect()),
        _ => Some(vec![expression]),
    }
}

pub(super) fn secondary_index_cursor_key(
    table: &astersql_meta_model::TableInfo,
    index: &astersql_meta_model::IndexInfo,
    columns_expression: &ast::ExprNode,
    values_expression: &ast::ExprNode,
    flags: astersql_types::Flags,
) -> SessionResult<Option<kv::Key>> {
    let Some(columns) = secondary_index_cursor_parts(columns_expression) else {
        return Ok(None);
    };
    let Some(values) = secondary_index_cursor_parts(values_expression) else {
        return Ok(None);
    };
    if columns.len() != values.len() || columns.is_empty() || columns.len() > index.Columns.len() {
        return Ok(None);
    }
    let mut datums = Vec::with_capacity(columns.len());
    for (position, (column_expression, value_expression)) in
        columns.into_iter().zip(values).enumerate()
    {
        let ast::ExprKind::Column(column_name) = &column_expression.Kind else {
            return Ok(None);
        };
        let index_column = &index.Columns[position];
        if index_column.Name.L != column_name.Name.L {
            return Ok(None);
        }
        let Some(column) = table
            .Columns
            .iter()
            .find(|column| column.Name.L == index_column.Name.L)
        else {
            return Ok(None);
        };
        let value = match literal(value_expression) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let Ok(value) = runtime_value_to_datum(Some(&value), column, flags) else {
            // An incompatible constant cannot form an index cursor. Go's
            // ranger abandons this access path and leaves coercion to the
            // predicate evaluator instead of failing the SELECT.
            return Ok(None);
        };
        datums.push(value);
    }
    let codec_table = astersql_tablecodec::model::TableInfo {
        Columns: table.Columns.clone(),
        Indices: table.Indices.clone(),
        PKIsHandle: table.PKIsHandle,
        IsCommonHandle: table.IsCommonHandle,
        CommonHandleVersion: table.CommonHandleVersion,
        ..Default::default()
    };
    astersql_tablecodec::TruncateIndexValues(
        Box::new(codec_table),
        Box::new(index.clone()),
        &mut datums,
    );
    let encoded =
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled())
            .EncodeKey(astersql_tablecodec::time::UTC, Vec::new(), datums)
            .map_err(|error| session_error("encode secondary-index cursor", error))?;
    Ok(Some(kv::Key(
        astersql_tablecodec::EncodeIndexSeekKey(table.ID, index.ID, Some(encoded)).0,
    )))
}

pub(super) fn secondary_index_predicate_bounds(
    table: &astersql_meta_model::TableInfo,
    index: &astersql_meta_model::IndexInfo,
    predicate: &ast::ExprNode,
    flags: astersql_types::Flags,
) -> SessionResult<Option<(kv::Key, kv::Key)>> {
    let (index_start, index_end) = astersql_tablecodec::GetTableIndexKeyRange(table.ID, index.ID);
    let full_range = || (kv::Key(index_start.clone()), kv::Key(index_end.clone()));
    if let ast::ExprKind::Parentheses(inner) = &predicate.Kind {
        return secondary_index_predicate_bounds(table, index, inner, flags);
    }
    if let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind
        && Op.eq_ignore_ascii_case("and")
    {
        let Some(left) = secondary_index_predicate_bounds(table, index, L, flags)? else {
            return Ok(None);
        };
        let Some(right) = secondary_index_predicate_bounds(table, index, R, flags)? else {
            return Ok(None);
        };
        return Ok(Some((
            if left.0.Cmp(&right.0) >= 0 {
                left.0
            } else {
                right.0
            },
            if left.1.Cmp(&right.1) <= 0 {
                left.1
            } else {
                right.1
            },
        )));
    }
    if let ast::ExprKind::Between {
        Expr,
        Left,
        Right,
        Not,
    } = &predicate.Kind
    {
        if *Not {
            return Ok(None);
        }
        let Some(lower) = secondary_index_cursor_key(table, index, Expr, Left, flags)? else {
            return Ok(None);
        };
        let Some(upper) = secondary_index_cursor_key(table, index, Expr, Right, flags)? else {
            return Ok(None);
        };
        return Ok(Some((lower, upper.PrefixNext())));
    }
    let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind else {
        return Ok(None);
    };
    let left_is_columns = secondary_index_cursor_parts(L).is_some_and(|parts| {
        parts
            .iter()
            .all(|part| matches!(part.Kind, ast::ExprKind::Column(_)))
    });
    let right_is_columns = secondary_index_cursor_parts(R).is_some_and(|parts| {
        parts
            .iter()
            .all(|part| matches!(part.Kind, ast::ExprKind::Column(_)))
    });
    let (columns, values, normalized_op) = if left_is_columns && !right_is_columns {
        (L.as_ref(), R.as_ref(), Op.as_str())
    } else if right_is_columns && !left_is_columns {
        let normalized = match Op.as_str() {
            "=" | "==" => "=",
            ">" => "<",
            ">=" => "<=",
            "<" => ">",
            "<=" => ">=",
            _ => return Ok(None),
        };
        (R.as_ref(), L.as_ref(), normalized)
    } else {
        return Ok(None);
    };
    if !matches!(normalized_op, "=" | "==" | ">" | ">=" | "<" | "<=") {
        return Ok(None);
    }
    let Some(cursor) = secondary_index_cursor_key(table, index, columns, values, flags)? else {
        return Ok(None);
    };
    let cursor_next = cursor.PrefixNext();
    let (start, end) = full_range();
    Ok(Some(match normalized_op {
        "=" | "==" => (cursor, cursor_next),
        ">" => (cursor_next, end),
        ">=" => (cursor, end),
        "<" => (start, cursor),
        "<=" => (start, cursor_next),
        _ => unreachable!("secondary index comparison normalized above"),
    }))
}

pub(super) fn relational_row_scan_ranges(
    table: &astersql_meta_model::TableInfo,
    primary_key_desc: Option<bool>,
) -> Vec<RelationalRowScanRange> {
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    let table_end = prefix.PrefixNext();
    let unsigned_primary_key = primary_key_desc.is_some()
        && table.PKIsHandle
        && table
            .GetPkColInfo()
            .is_some_and(|column| astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()));
    let zero_handle_key = unsigned_primary_key.then(|| {
        kv::Key(
            astersql_tablecodec::EncodeRowKeyWithHandle(
                table.ID,
                Box::new(astersql_tablecodec::kv::IntHandle(0)),
            )
            .0,
        )
    });
    match (primary_key_desc, zero_handle_key) {
        (Some(false), Some(zero)) => vec![
            (zero.clone(), table_end.clone(), false),
            (prefix.clone(), zero, false),
        ],
        (Some(true), Some(zero)) => vec![
            (prefix.clone(), zero.clone(), true),
            (zero, table_end.clone(), true),
        ],
        (Some(desc), None) => vec![(prefix.clone(), table_end.clone(), desc)],
        (None, _) => vec![(prefix.clone(), table_end.clone(), false)],
    }
}

pub(super) fn integer_primary_key_predicate_interval(
    predicate: &ast::ExprNode,
    primary_key_name: &str,
    domain_start: i128,
    domain_end: i128,
) -> Option<(i128, i128)> {
    if let ast::ExprKind::Parentheses(inner) = &predicate.Kind {
        return integer_primary_key_predicate_interval(
            inner,
            primary_key_name,
            domain_start,
            domain_end,
        );
    }
    if let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind
        && Op.eq_ignore_ascii_case("and")
    {
        let left =
            integer_primary_key_predicate_interval(L, primary_key_name, domain_start, domain_end)?;
        let right =
            integer_primary_key_predicate_interval(R, primary_key_name, domain_start, domain_end)?;
        return Some((left.0.max(right.0), left.1.min(right.1)));
    }
    if let ast::ExprKind::Between {
        Expr,
        Left,
        Right,
        Not,
    } = &predicate.Kind
    {
        if *Not {
            return None;
        }
        let ast::ExprKind::Column(column) = &Expr.Kind else {
            return None;
        };
        if column.Name.L != primary_key_name {
            return None;
        }
        let lower = literal(Left).ok()?.parse::<i128>().ok()?;
        let upper = literal(Right).ok()?.parse::<i128>().ok()?;
        return Some((
            lower.max(domain_start),
            upper.saturating_add(1).min(domain_end),
        ));
    }
    let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind else {
        return None;
    };
    let (column, value, normalized_op) = match (&L.Kind, &R.Kind) {
        (ast::ExprKind::Column(column), _) => {
            let normalized = match Op.as_str() {
                "=" | "==" | ">" | ">=" | "<" | "<=" => Op.as_str(),
                _ => return None,
            };
            (column, literal(R).ok()?.parse::<i128>().ok()?, normalized)
        }
        (_, ast::ExprKind::Column(column)) => {
            let normalized = match Op.as_str() {
                "=" | "==" => "=",
                ">" => "<",
                ">=" => "<=",
                "<" => ">",
                "<=" => ">=",
                _ => return None,
            };
            (column, literal(L).ok()?.parse::<i128>().ok()?, normalized)
        }
        _ => return None,
    };
    if column.Name.L != primary_key_name {
        return None;
    }
    let interval = match normalized_op {
        "=" | "==" => (value, value.saturating_add(1)),
        ">" => (value.saturating_add(1), domain_end),
        ">=" => (value, domain_end),
        "<" => (domain_start, value),
        "<=" => (domain_start, value.saturating_add(1)),
        _ => unreachable!("comparison operator normalized above"),
    };
    Some((interval.0.max(domain_start), interval.1.min(domain_end)))
}

/// 将 Go ranger 的整数聚簇主键访问条件收敛为 table record key ranges。
///
/// 返回 `None` 表示 WHERE 仍含残余条件，不能在范围内提前应用 LIMIT；
/// 返回空 ranges 表示谓词与主键值域无交集。
pub(crate) fn relational_primary_key_scan_ranges(
    table: &astersql_meta_model::TableInfo,
    predicate: &ast::ExprNode,
    primary_key_desc: Option<bool>,
) -> Option<Vec<RelationalRowScanRange>> {
    if !table.PKIsHandle {
        return None;
    }
    let primary_key = table.GetPkColInfo()?;
    let unsigned = astersql_parser_mysql::r#type::HasUnsignedFlag(primary_key.GetFlag());
    let (domain_start, domain_end) = if unsigned {
        (0, (u64::MAX as i128) + 1)
    } else {
        (i64::MIN as i128, (i64::MAX as i128) + 1)
    };
    let (start, end) = integer_primary_key_predicate_interval(
        predicate,
        &primary_key.Name.L,
        domain_start,
        domain_end,
    )?;
    if start >= end {
        return Some(Vec::new());
    }

    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    let table_end = prefix.PrefixNext();
    let mut ranges = if unsigned {
        let unsigned_half = (i64::MAX as i128) + 1;
        let mut ranges = Vec::with_capacity(2);
        let positive_start = start.max(0);
        let positive_end = end.min(unsigned_half);
        if positive_start < positive_end {
            ranges.push((
                relational_handle_key(table.ID, positive_start),
                if positive_end == unsigned_half {
                    table_end.clone()
                } else {
                    relational_handle_key(table.ID, positive_end)
                },
                false,
            ));
        }
        let wrapped_start = start.max(unsigned_half);
        let wrapped_end = end.min(domain_end);
        if wrapped_start < wrapped_end {
            let zero = relational_handle_key(table.ID, 0);
            ranges.push((
                if wrapped_start == unsigned_half {
                    prefix.clone()
                } else {
                    relational_handle_key(table.ID, wrapped_start)
                },
                if wrapped_end == domain_end {
                    zero
                } else {
                    relational_handle_key(table.ID, wrapped_end)
                },
                false,
            ));
        }
        ranges
    } else {
        vec![(
            if start == domain_start {
                prefix
            } else {
                relational_handle_key(table.ID, start)
            },
            if end == domain_end {
                table_end
            } else {
                relational_handle_key(table.ID, end)
            },
            false,
        )]
    };

    match primary_key_desc {
        Some(true) => {
            ranges.reverse();
            for range in &mut ranges {
                range.2 = true;
            }
        }
        None if unsigned => {
            ranges.sort_by(|left, right| left.0.as_ref().cmp(right.0.as_ref()));
        }
        _ => {}
    }
    Some(ranges)
}

pub(super) fn scan_relational_row_ranges(
    retriever: &dyn kv::Retriever,
    table: &astersql_meta_model::TableInfo,
    ranges: &[RelationalRowScanRange],
    offset: usize,
    count: usize,
) -> SessionResult<Vec<RelationalRow>> {
    let fields: HashMap<i64, Box<astersql_tablecodec::types::FieldType>> = table
        .Columns
        .iter()
        .map(|column| (column.ID, Box::new(column.FieldType.clone())))
        .collect();
    let mut rows = Vec::with_capacity(count.min(1024));
    let mut remaining_offset = offset;
    for (lower, upper, reverse) in ranges {
        if rows.len() >= count {
            break;
        }
        let mut iterator = if *reverse {
            retriever.IterReverse(Some(upper.clone()), Some(lower.clone()))
        } else {
            retriever.Iter(lower.clone(), Some(upper.clone()))
        }
        .map_err(|error| session_error("scan relational rows", error))?;
        let result = consume_relational_row_iterator(
            iterator.as_mut(),
            table,
            &fields,
            &mut remaining_offset,
            count,
            &mut rows,
        );
        iterator.Close();
        result?;
    }
    Ok(rows)
}

pub(super) fn scan_relational_secondary_index_window(
    snapshot: &mut dyn kv::Snapshot,
    table: &astersql_meta_model::TableInfo,
    access: &RelationalSecondaryIndexAccess,
    offset: usize,
    count: usize,
) -> SessionResult<Vec<RelationalRow>> {
    if count == 0 || access.ranges.is_empty() {
        return Ok(Vec::new());
    }
    let mut remaining_offset = offset;
    let mut handles =
        Vec::<Box<dyn astersql_tablecodec::kv::Handle>>::with_capacity(count.min(1024));
    for (lower, upper, reverse) in &access.ranges {
        if handles.len() >= count {
            break;
        }
        let mut iterator = if *reverse {
            snapshot.IterReverse(Some(upper.clone()), Some(lower.clone()))
        } else {
            snapshot.Iter(lower.clone(), Some(upper.clone()))
        }
        .map_err(|error| session_error("scan secondary index", error))?;
        let result = (|| {
            while iterator.Valid() && remaining_offset != 0 {
                iterator
                    .Next()
                    .map_err(|error| session_error("advance secondary-index OFFSET", error))?;
                remaining_offset -= 1;
            }
            while iterator.Valid() && handles.len() < count {
                handles.push(
                    astersql_tablecodec::DecodeIndexHandle(
                        iterator.Key().0,
                        iterator.Value().to_vec(),
                        access.index.Columns.len(),
                    )
                    .map_err(|error| session_error("decode secondary-index handle", error))?
                    .ok_or_else(|| {
                        SessionError::new("secondary-index value does not contain a handle")
                    })?,
                );
                iterator
                    .Next()
                    .map_err(|error| session_error("advance secondary index", error))?;
            }
            Ok(())
        })();
        iterator.Close();
        result?;
    }
    if handles.is_empty() {
        return Ok(Vec::new());
    }
    let record_keys = handles
        .iter()
        .map(|handle| {
            kv::Key(astersql_tablecodec::EncodeRowKeyWithHandle(table.ID, handle.Copy()).0)
        })
        .collect::<Vec<_>>();
    let values = snapshot
        .BatchGet(&kv::Context::todo(), &record_keys, &[])
        .map_err(|error| session_error("batch get secondary-index rows", error))?;
    let fields: HashMap<i64, Box<astersql_tablecodec::types::FieldType>> = table
        .Columns
        .iter()
        .map(|column| (column.ID, Box::new(column.FieldType.clone())))
        .collect();
    handles
        .iter()
        .zip(record_keys)
        .map(|(handle, key)| {
            let key_name = kv::KeyMapName(key.as_ref());
            let value = values.get(&key_name).ok_or_else(|| {
                SessionError::new(format!(
                    "secondary index {} points to a missing record",
                    access.index.Name.O
                ))
            })?;
            decode_relational_row_value(table, &fields, handle.as_ref(), &value.Value)
        })
        .collect()
}

pub(super) fn scan_relational_row_ranges_window_key_only(
    snapshot: &mut dyn kv::Snapshot,
    table: &astersql_meta_model::TableInfo,
    mut ranges: Vec<RelationalRowScanRange>,
    offset: usize,
    count: usize,
) -> SessionResult<Vec<RelationalRow>> {
    if count == 0 || ranges.is_empty() {
        return Ok(Vec::new());
    }
    if offset == 0 {
        return scan_relational_row_ranges(snapshot, table, &ranges, 0, count);
    }
    snapshot.SetOption(kv::KeyOnly, Some(Box::new(true)));
    let boundary = (|| {
        let mut remaining_offset = offset;
        for (index, (lower, upper, reverse)) in ranges.iter().enumerate() {
            let mut iterator = if *reverse {
                snapshot.IterReverse(Some(upper.clone()), Some(lower.clone()))
            } else {
                snapshot.Iter(lower.clone(), Some(upper.clone()))
            }
            .map_err(|error| session_error("scan relational OFFSET keys", error))?;
            let locate_result = (|| {
                while iterator.Valid() && remaining_offset != 0 {
                    iterator
                        .Next()
                        .map_err(|error| session_error("advance relational OFFSET keys", error))?;
                    remaining_offset -= 1;
                }
                Ok((remaining_offset == 0 && iterator.Valid()).then(|| (index, iterator.Key())))
            })();
            iterator.Close();
            if let Some(boundary) = locate_result? {
                return Ok(Some(boundary));
            }
        }
        Ok(None)
    })();
    snapshot.SetOption(kv::KeyOnly, Some(Box::new(false)));

    let Some((range_index, boundary_key)) = boundary? else {
        return Ok(Vec::new());
    };
    ranges.drain(..range_index);
    if ranges[0].2 {
        let boundary_end = boundary_key.Next();
        if boundary_end.Cmp(&ranges[0].1) < 0 {
            ranges[0].1 = boundary_end;
        }
    } else {
        ranges[0].0 = boundary_key;
    }
    scan_relational_row_ranges(snapshot, table, &ranges, 0, count)
}

/// 扫描表前缀，在存储游标层跳过 OFFSET，只解码最终需要返回的行。
///
/// `primary_key_desc` 为 `Some` 时按整数聚簇主键顺序扫描。无符号主键在
/// `i64::MAX` 处拆成两个物理范围，保持与 TiDB unsigned handle range 一致。
pub(super) fn scan_relational_rows_window(
    retriever: &dyn kv::Retriever,
    table: &astersql_meta_model::TableInfo,
    offset: usize,
    count: usize,
    primary_key_desc: Option<bool>,
) -> SessionResult<Vec<RelationalRow>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let ranges = relational_row_scan_ranges(table, primary_key_desc);
    scan_relational_row_ranges(retriever, table, &ranges, offset, count)
}

/// 以物理顺序首个整数 handle 加 OFFSET 构造一个不会越过目标行的 seek key。
///
/// 唯一整数 handle 间至少相差 1，因此 `[first, first + offset)` 中可见行数
/// 必然不超过 offset；后续用 TiKV 精确范围计数补偿删除/空洞即可保持语义。
pub(crate) fn integer_handle_offset_candidate(
    snapshot: &mut dyn kv::Snapshot,
    table: &astersql_meta_model::TableInfo,
    offset: usize,
) -> SessionResult<Option<kv::Key>> {
    if !table.PKIsHandle || offset == 0 {
        return Ok(None);
    }
    let Ok(offset) = i64::try_from(offset) else {
        return Ok(None);
    };
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    let table_end = prefix.PrefixNext();
    snapshot.SetOption(kv::KeyOnly, Some(Box::new(true)));
    let first_key = (|| {
        let mut iterator = snapshot
            .Iter(prefix, Some(table_end))
            .map_err(|error| session_error("locate first relational handle", error))?;
        let first_key = iterator.Valid().then(|| iterator.Key());
        iterator.Close();
        Ok(first_key)
    })();
    snapshot.SetOption(kv::KeyOnly, Some(Box::new(false)));
    let Some(first_key) = first_key? else {
        return Ok(None);
    };
    let (_, first_handle) =
        astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(first_key.0))
            .map_err(|error| session_error("decode first relational handle", error))?;
    if !first_handle.IsInt() {
        return Ok(None);
    }
    let Some(candidate) = first_handle.IntValue().checked_add(offset) else {
        return Ok(None);
    };
    Ok(Some(kv::Key(
        astersql_tablecodec::EncodeRowKeyWithHandle(
            table.ID,
            Box::new(astersql_tablecodec::kv::IntHandle(candidate)),
        )
        .0,
    )))
}

/// 用 key-only Scan 定位 OFFSET 边界，再从该键开始读取最终窗口的完整 value。
///
/// 这与普通顺序迭代保持完全相同的稀疏 handle 语义，但跳过大 OFFSET 时不再
/// 把宽表的整行 value 从 TiKV 传回 TiDB。
pub(crate) fn scan_relational_rows_window_key_only_from(
    snapshot: &mut dyn kv::Snapshot,
    table: &astersql_meta_model::TableInfo,
    offset: usize,
    count: usize,
    primary_key_desc: Option<bool>,
    start_key: Option<kv::Key>,
) -> SessionResult<Vec<RelationalRow>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut ranges = relational_row_scan_ranges(table, primary_key_desc);
    if let Some(start_key) = start_key {
        if primary_key_desc.is_some() || ranges[0].2 {
            return Err(SessionError::new(
                "integer handle OFFSET seek requires physical forward order",
            ));
        }
        ranges[0].0 = start_key;
    }
    scan_relational_row_ranges_window_key_only(snapshot, table, ranges, offset, count)
}

pub(crate) fn scan_relational_rows_window_key_only(
    snapshot: &mut dyn kv::Snapshot,
    table: &astersql_meta_model::TableInfo,
    offset: usize,
    count: usize,
    primary_key_desc: Option<bool>,
) -> SessionResult<Vec<RelationalRow>> {
    scan_relational_rows_window_key_only_from(
        snapshot,
        table,
        offset,
        count,
        primary_key_desc,
        None,
    )
}

/// Tests whether a real transaction mem-buffer contains any record or index
/// key belonging to the table. This is shared by the session FTS runtime and
/// mirrors the table-prefix check used by Go's union-scan guard.
/// 判断事务缓冲中是否含指定表前缀写入。
pub fn transaction_has_table_prefix(
    transaction: &dyn kv::Transaction,
    table_id: i64,
) -> SessionResult<bool> {
    if transaction.IsPipelined() {
        return Ok(false);
    }
    let prefix = kv::Key(astersql_tablecodec::GenTablePrefix(table_id).0);
    let mut iterator = transaction
        .GetMemBuffer()
        .Iter(prefix.clone(), Some(prefix.PrefixNext()))
        .map_err(|error| session_error("scan transaction mem-buffer", error))?;
    let found = iterator.Valid();
    iterator.Close();
    Ok(found)
}

/// 校验 SQL 操作的是会话 KV 表。

impl ConcreteSession {
    pub(super) fn cop_resource_group_name(&self) -> String {
        self.state
            .borrow()
            .runaway_resource_group_name
            .clone()
            .unwrap_or_else(|| {
                let configured =
                    self.WithSessionVars(|vars| vars.StmtCtx.ResourceGroupName.clone());
                if configured.is_empty() {
                    astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME.into()
                } else {
                    configured
                }
            })
    }
    pub(super) fn scan_registered_table(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<Vec<RelationalRow>> {
        if astersql_testkit_testfailpoint::eval_bool("session/updateFullTableScan") {
            return Err(SessionError::new(
                "injected failure: UPDATE reached a full relational table scan",
            ));
        }
        self.scan_registered_table_with_limit(table, None)
    }

    /// Read only clustered-primary-key ranges while preserving the same
    /// stale-read, RC, and transaction visibility rules as a registered-table
    /// scan. Callers must fall back when the predicate cannot produce ranges.
    pub(super) fn scan_registered_table_ranges(
        &self,
        table: &astersql_meta_model::TableInfo,
        ranges: &[RelationalRowScanRange],
    ) -> SessionResult<Vec<RelationalRow>> {
        let state = self.state.borrow();
        if let Some(read_ts) = state
            .transaction_stale_read_ts
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            })
        {
            return self.domain.storage().with_storage(|store| {
                let snapshot = store.GetSnapshot(kv::NewVersion(read_ts));
                scan_relational_row_ranges(snapshot.as_ref(), table, ranges, 0, usize::MAX)
            });
        }
        if state.transaction.is_some()
            && state
                .transaction_isolation
                .eq_ignore_ascii_case("READ-COMMITTED")
        {
            drop(state);
            return self.scan_latest_with_transaction_overlay_ranges(table, ranges);
        }
        if let Some(transaction) = state.transaction.as_ref() {
            scan_relational_row_ranges(transaction.as_ref(), table, ranges, 0, usize::MAX)
        } else {
            self.domain.storage().with_storage(|store| {
                let version = store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?;
                let snapshot = store.GetSnapshot(version);
                scan_relational_row_ranges(snapshot.as_ref(), table, ranges, 0, usize::MAX)
            })
        }
    }

    pub(crate) fn relational_secondary_index_access(
        &self,
        table: &astersql_meta_model::TableInfo,
        statement: &ast::SelectStmt,
    ) -> Option<RelationalSecondaryIndexAccess> {
        let predicate = statement.Where.as_ref();
        if predicate.is_none() && statement.OrderBy.is_empty() {
            return None;
        }
        let descending = statement
            .OrderBy
            .first()
            .is_some_and(|first_order| first_order.Desc);
        let order_columns = statement
            .OrderBy
            .iter()
            .map(|item| {
                if item.Desc != descending {
                    return None;
                }
                match &item.Expr.Kind {
                    ast::ExprKind::Column(column) => Some(column.Name.L.as_str()),
                    _ => None,
                }
            })
            .collect::<Option<Vec<_>>>()?;
        let mut candidates =
            table
                .Indices
                .iter()
                .filter(|index| {
                    index.State == astersql_meta_model::StatePublic
                        && !index.Primary
                        && !index.Invisible
                        && !index.MVIndex
                        && !index.Global
                        && index.ConditionExprString.is_empty()
                        && index.Columns.len() >= order_columns.len()
                        && index.Columns.iter().all(|column| column.Length < 0)
                        && index.Columns.iter().zip(&order_columns).all(
                            |(index_column, order_column)| index_column.Name.L == **order_column,
                        )
                })
                .cloned()
                .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.Columns
                .len()
                .cmp(&right.Columns.len())
                .then_with(|| left.ID.cmp(&right.ID))
        });
        for index in candidates {
            let mut ranges = Vec::new();
            let mut supported = true;
            for physical_table_id in local_secondary_index_table_ids(table) {
                let mut physical_table = table.clone();
                physical_table.ID = physical_table_id;
                let bounds = if let Some(predicate) = predicate {
                    let Ok(Some(bounds)) = secondary_index_predicate_bounds(
                        &physical_table,
                        &index,
                        predicate,
                        self.dml_type_flags(),
                    ) else {
                        supported = false;
                        break;
                    };
                    bounds
                } else {
                    let (lower, upper) =
                        astersql_tablecodec::GetTableIndexKeyRange(physical_table_id, index.ID);
                    (kv::Key(lower), kv::Key(upper))
                };
                if bounds.0.Cmp(&bounds.1) < 0 {
                    ranges.push((bounds.0, bounds.1, descending));
                }
            }
            if !supported {
                continue;
            }
            return Some(RelationalSecondaryIndexAccess { index, ranges });
        }
        None
    }

    pub(super) fn relational_index_merge_access(
        &self,
        table: &astersql_meta_model::TableInfo,
        statement: &ast::SelectStmt,
    ) -> SessionResult<Option<RelationalIndexMergeAccess>> {
        let Some(predicate) = statement.Where.as_ref() else {
            return Ok(None);
        };
        let ast::ExprKind::Binary { Op, .. } = &predicate.Kind else {
            return Ok(None);
        };
        let intersection = if Op.eq_ignore_ascii_case("or") || Op == "||" {
            false
        } else if Op.eq_ignore_ascii_case("and") || Op == "&&" {
            true
        } else {
            return Ok(None);
        };
        let index_merge_hint = statement
            .TableHints
            .iter()
            .find(|hint| hint.HintName.L.eq_ignore_ascii_case("use_index_merge"));
        // TiDB only enables intersection paths when the optimizer chose them or
        // USE_INDEX_MERGE requested them. This compact runtime has no cost-based
        // intersection enumeration, so require the explicit hint for AND.
        if intersection && index_merge_hint.is_none() {
            return Ok(None);
        }
        let hinted_indexes = index_merge_hint
            .map(|hint| {
                hint.Indexes
                    .iter()
                    .map(|index| index.L.as_str())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut terms = Vec::new();
        if intersection {
            flatten_and(predicate, &mut terms);
        } else {
            flatten_or(predicate, &mut terms);
        }
        let branch_access = |branch: &ast::ExprNode| {
            let Some(index) = explain_indexed_branch_matching(branch, table, |index| {
                index.State == astersql_meta_model::StatePublic
                    && !index.Primary
                    && !index.Invisible
                    && !index.MVIndex
                    && !index.Global
                    && index.ConditionExprString.is_empty()
                    && (hinted_indexes.is_empty() || hinted_indexes.contains(index.Name.L.as_str()))
            }) else {
                return Ok(None);
            };
            let mut ranges = Vec::new();
            for physical_table_id in local_secondary_index_table_ids(table) {
                let mut physical_table = table.clone();
                physical_table.ID = physical_table_id;
                let Some((lower, upper)) = secondary_index_predicate_bounds(
                    &physical_table,
                    index,
                    branch,
                    self.dml_type_flags(),
                )?
                else {
                    return Ok(None);
                };
                if lower.Cmp(&upper) < 0 {
                    ranges.push((lower, upper, false));
                }
            }
            Ok::<_, SessionError>(Some(RelationalSecondaryIndexAccess {
                index: index.clone(),
                ranges,
            }))
        };
        let mut seen_indexes = HashSet::new();
        let mut branches = Vec::new();
        for term in terms {
            let Some(access) = branch_access(term)? else {
                continue;
            };
            if seen_indexes.insert(access.index.ID) {
                branches.push(access);
            }
        }
        if branches.len() < 2 {
            return Ok(None);
        }
        Ok(Some(RelationalIndexMergeAccess {
            branches,
            intersection,
        }))
    }

    /// 在当前事务或快照上按可选行数上限扫描已注册表。
    pub(super) fn scan_registered_table_with_limit(
        &self,
        table: &astersql_meta_model::TableInfo,
        row_limit: Option<usize>,
    ) -> SessionResult<Vec<RelationalRow>> {
        let state = self.state.borrow();
        if let Some(concurrency) = state.restricted_analyze_scan_concurrency {
            let replica_read = state.replica_read.as_str();
            let replica_read = if replica_read.eq_ignore_ascii_case("leader") {
                kv::ReplicaReadType::ReplicaReadLeader
            } else {
                kv::ReplicaReadType::ReplicaReadMixed
            };
            let connection_id = self.connection_id();
            drop(state);
            let mut rows = self.domain.storage().with_storage(|store| {
                let version = store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get ANALYZE snapshot version", error))?;
                if let Some(rows) = scan_relational_rows_with_coprocessor(
                    store,
                    table,
                    version.Ver,
                    concurrency,
                    replica_read,
                    kv::GlobalTxnScope,
                    connection_id,
                )? {
                    return Ok(rows);
                }
                let snapshot = store.GetSnapshot(version);
                scan_relational_rows_with_limit(snapshot.as_ref(), table, row_limit)
            })?;
            if let Some(row_limit) = row_limit {
                rows.truncate(row_limit);
            }
            return Ok(rows);
        }
        if let Some(read_ts) = state
            .transaction_stale_read_ts
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            })
        {
            return self.domain.storage().with_storage(|store| {
                let snapshot = store.GetSnapshot(kv::NewVersion(read_ts));
                scan_relational_rows_with_limit(snapshot.as_ref(), table, row_limit)
            });
        }
        if state.transaction.is_some()
            && state
                .transaction_isolation
                .eq_ignore_ascii_case("READ-COMMITTED")
        {
            drop(state);
            let mut rows = self.scan_latest_with_transaction_overlay(table)?;
            if let Some(row_limit) = row_limit {
                rows.truncate(row_limit);
            }
            return Ok(rows);
        }
        if let Some(transaction) = state.transaction.as_ref() {
            scan_relational_rows_with_limit(transaction.as_ref(), table, row_limit)
        } else {
            self.domain.storage().with_storage(|store| {
                let version = store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?;
                let snapshot = store.GetSnapshot(version);
                scan_relational_rows_with_limit(snapshot.as_ref(), table, row_limit)
            })
        }
    }

    pub(super) fn scan_registered_table_at(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
    ) -> SessionResult<Vec<RelationalRow>> {
        self.scan_registered_table_at_with_limit(table, read_ts, None)
    }

    pub(super) fn scan_registered_table_at_with_limit(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
        row_limit: Option<usize>,
    ) -> SessionResult<Vec<RelationalRow>> {
        if let Some(read_ts) = read_ts {
            return self.domain.storage().with_storage(|store| {
                let snapshot = store.GetSnapshot(kv::NewVersion(read_ts));
                scan_relational_rows_with_limit(snapshot.as_ref(), table, row_limit)
            });
        }
        self.scan_registered_table_with_limit(table, row_limit)
    }

    /// 对无排序、无过滤的自动提交分页查询，在 KV 游标层消费 OFFSET。
    ///
    /// 这保留 TiDB 根 LimitExec 的结果语义，但不会把 `offset + count` 行全部
    /// 解码并物化到 TiDB 内存；同时为该次大 OFFSET 扫描提高远端 Scan 批大小。
    pub(super) fn scan_registered_table_at_with_window(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
        window: RelationalLimitWindow,
        primary_key_desc: Option<bool>,
        access_ranges: Option<&[RelationalRowScanRange]>,
    ) -> SessionResult<Vec<RelationalRow>> {
        let state = self.state.borrow();
        if state.transaction.is_some() {
            return Err(SessionError::new(
                "relational OFFSET scan window requires an autocommit snapshot",
            ));
        }
        let effective_read_ts = read_ts
            .or(state.transaction_stale_read_ts)
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            });
        drop(state);
        let runaway_checker = self.state.borrow().runaway_checker.clone();
        let resource_group_name = self.cop_resource_group_name();
        self.domain.storage().with_storage(|store| {
            let version = match effective_read_ts {
                Some(read_ts) => kv::NewVersion(read_ts),
                None => store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?,
            };
            let mut snapshot = store.GetSnapshot(version);
            let mut remaining_offset = window.offset;
            let mut start_key = None;
            if access_ranges.is_none()
                && primary_key_desc.is_none()
                && window.offset >= RELATIONAL_OFFSET_HANDLE_SEEK_THRESHOLD
                && let Some(candidate) =
                    integer_handle_offset_candidate(snapshot.as_mut(), table, window.offset)?
            {
                let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
                if let Some(skipped) = count_relational_key_range_with_coprocessor(
                    store,
                    table,
                    version.Ver,
                    prefix,
                    candidate.clone(),
                    runaway_checker.clone(),
                    &resource_group_name,
                )? && skipped <= window.offset
                {
                    remaining_offset = window.offset - skipped;
                    start_key = Some(candidate);
                }
            }
            let requested = remaining_offset.saturating_add(window.count);
            let scan_batch_size = requested.min(RELATIONAL_OFFSET_SCAN_BATCH_SIZE).max(256);
            snapshot.SetOption(kv::ScanBatchSize, Some(Box::new(scan_batch_size)));
            if let Some(access_ranges) = access_ranges {
                scan_relational_row_ranges_window_key_only(
                    snapshot.as_mut(),
                    table,
                    access_ranges.to_vec(),
                    remaining_offset,
                    window.count,
                )
            } else {
                scan_relational_rows_window_key_only_from(
                    snapshot.as_mut(),
                    table,
                    remaining_offset,
                    window.count,
                    primary_key_desc,
                    start_key,
                )
            }
        })
    }

    pub(super) fn scan_registered_table_at_with_index_window(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
        window: RelationalLimitWindow,
        access: &RelationalSecondaryIndexAccess,
    ) -> SessionResult<Vec<RelationalRow>> {
        let state = self.state.borrow();
        if state.transaction.is_some() {
            return Err(SessionError::new(
                "secondary-index lookup requires an autocommit snapshot",
            ));
        }
        let effective_read_ts = read_ts
            .or(state.transaction_stale_read_ts)
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            });
        drop(state);
        self.domain.storage().with_storage(|store| {
            let version = match effective_read_ts {
                Some(read_ts) => kv::NewVersion(read_ts),
                None => store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?,
            };
            let mut snapshot = store.GetSnapshot(version);
            let requested = window.offset.saturating_add(window.count);
            snapshot.SetOption(
                kv::ScanBatchSize,
                Some(Box::new(
                    requested.min(RELATIONAL_OFFSET_SCAN_BATCH_SIZE).max(256),
                )),
            );
            scan_relational_secondary_index_window(
                snapshot.as_mut(),
                table,
                access,
                window.offset,
                window.count,
            )
        })
    }

    pub(super) fn scan_registered_table_at_with_index_merge(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
        access: &RelationalIndexMergeAccess,
        predicate: Option<&ast::ExprNode>,
        embedded_limit: Option<RelationalLimitWindow>,
    ) -> SessionResult<Vec<RelationalRow>> {
        let state = self.state.borrow();
        if state.transaction.is_some() {
            return Err(SessionError::new(
                "index merge requires an autocommit snapshot",
            ));
        }
        let effective_read_ts = read_ts
            .or(state.transaction_stale_read_ts)
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            });
        drop(state);
        let version = self.domain.storage().with_storage(|store| {
            effective_read_ts.map_or_else(
                || {
                    store
                        .CurrentVersion("global")
                        .map_err(|error| session_error("get index-merge snapshot version", error))
                },
                |read_ts| Ok(kv::NewVersion(read_ts)),
            )
        })?;
        let branch_requests = self
            .state
            .borrow()
            .last_select_request
            .as_ref()
            .map(|observed| {
                std::iter::once(observed.request.clone())
                    .chain(observed.auxiliary_requests.iter().cloned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let branches = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(access.branches.len());
            for (branch, access) in access.branches.iter().cloned().enumerate() {
                let domain = Arc::clone(&self.domain);
                let table = table.clone();
                let request = branch_requests.get(branch).cloned();
                workers.push(scope.spawn(move || {
                    if let Some(request) = request {
                        std::hint::black_box((
                            request.Concurrency,
                            request
                                .KeyRanges
                                .as_ref()
                                .map_or(0, kv::KeyRanges::PartitionNum),
                        ));
                    }
                    domain.storage().with_storage(|store| {
                        let mut snapshot = store.GetSnapshot(version);
                        scan_relational_secondary_index_window(
                            snapshot.as_mut(),
                            &table,
                            &access,
                            0,
                            usize::MAX,
                        )
                    })
                }));
            }
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .map_err(|_| SessionError::new("index-merge worker panicked"))?
                })
                .collect::<SessionResult<Vec<_>>>()
        })?;
        let flags = self.dml_type_flags();
        let branch_count = branches.len();
        let mut merged = BTreeMap::new();
        for (branch, rows) in branches.into_iter().enumerate() {
            for candidate in rows {
                let record_key = encode_relational_row(table, &candidate.1, flags)?.0.0;
                let physical_id = Self::row_physical_id(table, &candidate.1);
                let (_, matched_branches) = merged
                    .entry((physical_id, record_key))
                    .or_insert_with(|| (candidate, HashSet::new()));
                matched_branches.insert(branch);
            }
        }
        let rows = merged
            .into_values()
            .filter_map(|(candidate, matched_branches)| {
                (!access.intersection || matched_branches.len() == branch_count)
                    .then_some(candidate)
            })
            .collect::<Vec<_>>();
        if let Some(window) = embedded_limit {
            let rows = if let Some(predicate) = predicate {
                self.filter_relational_query_rows(rows, predicate, table)?
            } else {
                rows
            };
            return execute_relational_limit(rows, Some(window));
        }
        Ok(rows)
    }

    /// 使用与 Go 相同的 PlanBuilder→DoOptimize→TableReader 展平链生成 COUNT DAG。
    pub(crate) fn planned_scalar_count_dag(
        &self,
        sql: &str,
        expected_table_id: i64,
    ) -> SessionResult<Option<Vec<u8>>> {
        let mut statements = parse(sql)?;
        if statements.len() != 1 || !statements[0].as_any().is::<ast::SelectStmt>() {
            return Ok(None);
        }
        let statement = ast::NodeRef::new(statements.remove(0));
        let info_schema = self.domain.info_schema();
        let plan_context = plan_context_with_params(Arc::clone(&self.session_vars), &[], false);
        let (row_count, stats_version) =
            estimated_table_stats(self.domain.as_ref(), expected_table_id);
        let (mut builder, _) = astersql_planner_core::NewPlanBuilder()
            .withDataSourceProvider(Arc::new(SessionKVDataSourceProvider {
                row_count,
                stats_version,
            }))
            .Init(
                plan_context.clone(),
                info_schema,
                astersql_util_hint::NewQBHintHandler(None),
            );
        let mut logical = builder
            .buildResultSetNode(astersql_planner_core::context::TODO(), &statement, false)
            .map_err(|error| session_error("build scalar COUNT plan", error))?;
        let (physical, _) = astersql_planner_core::DoOptimize(
            astersql_planner_core::context::TODO(),
            &plan_context,
            builder.GetOptFlag(),
            &mut logical,
        )
        .map_err(|error| session_error("optimize scalar COUNT", error))?;
        let Some(reader) = physical_table_reader(physical.as_ref()) else {
            return Ok(None);
        };
        let scan = reader
            .GetTableScan()
            .map_err(|error| session_error("resolve planned COUNT table scan", error))?;
        let table_id = scan
            .Table
            .as_ref()
            .map_or(scan.PhysicalTableID, |table| table.ID);
        if table_id != expected_table_id || reader.StoreType != kv::StoreType::TiKV {
            return Ok(None);
        }
        planned_table_reader_dag(&plan_context, reader).map(Some)
    }

    /// 自动提交快照下，将规划器判定可下推的过滤与 COUNT 一起下推到 TiKV。
    ///
    /// 显式事务可能含本地未提交写入，必须保留 root 侧 union-scan 语义。
    pub(super) fn count_registered_table_with_filter_at(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
        statement_sql: Option<&str>,
    ) -> SessionResult<Option<usize>> {
        let state = self.state.borrow();
        if state.transaction.is_some() {
            return Ok(None);
        }
        let effective_read_ts = read_ts
            .or(state.transaction_stale_read_ts)
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            });
        drop(state);
        let runaway_checker = self.state.borrow().runaway_checker.clone();
        let resource_group_name = self.cop_resource_group_name();
        self.domain.storage().with_storage(|store| {
            let version = match effective_read_ts {
                Some(read_ts) => kv::NewVersion(read_ts),
                None => store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?,
            };
            let Some(sql) = statement_sql else {
                return Ok(None);
            };
            let Some(data) = self.planned_scalar_count_dag(sql, table.ID)? else {
                return Ok(None);
            };
            count_relational_rows_with_planned_filter_coprocessor(
                store,
                table,
                version.Ver,
                data,
                runaway_checker,
                &resource_group_name,
            )
        })
    }

    /// 在与普通 SELECT 相同的事务/快照可见性下，仅统计记录键。
    pub(super) fn count_registered_table_at(
        &self,
        table: &astersql_meta_model::TableInfo,
        read_ts: Option<u64>,
    ) -> SessionResult<usize> {
        let runaway_checker = self.state.borrow().runaway_checker.clone();
        let resource_group_name = self.cop_resource_group_name();
        if let Some(read_ts) = read_ts {
            return self.domain.storage().with_storage(|store| {
                if let Some(count) = count_relational_rows_with_coprocessor(
                    store,
                    table,
                    read_ts,
                    runaway_checker.clone(),
                    &resource_group_name,
                )? {
                    return Ok(count);
                }
                let snapshot = store.GetSnapshot(kv::NewVersion(read_ts));
                count_relational_rows(snapshot.as_ref(), table)
            });
        }
        let state = self.state.borrow();
        if let Some(read_ts) = state
            .transaction_stale_read_ts
            .or(state.snapshot_read_ts)
            .or(state.session_stale_read_ts)
            .or_else(|| {
                (state.enable_external_ts_read && state.external_read_ts != 0)
                    .then_some(state.external_read_ts)
            })
        {
            return self.domain.storage().with_storage(|store| {
                if let Some(count) = count_relational_rows_with_coprocessor(
                    store,
                    table,
                    read_ts,
                    runaway_checker.clone(),
                    &resource_group_name,
                )? {
                    return Ok(count);
                }
                let snapshot = store.GetSnapshot(kv::NewVersion(read_ts));
                count_relational_rows(snapshot.as_ref(), table)
            });
        }
        if state.transaction.is_some()
            && state
                .transaction_isolation
                .eq_ignore_ascii_case("READ-COMMITTED")
        {
            drop(state);
            return Ok(self.scan_latest_with_transaction_overlay(table)?.len());
        }
        if let Some(transaction) = state.transaction.as_ref() {
            count_relational_rows(transaction.as_ref(), table)
        } else {
            self.domain.storage().with_storage(|store| {
                let version = store
                    .CurrentVersion("global")
                    .map_err(|error| session_error("get relational snapshot version", error))?;
                if let Some(count) = count_relational_rows_with_coprocessor(
                    store,
                    table,
                    version.Ver,
                    runaway_checker,
                    &resource_group_name,
                )? {
                    return Ok(count);
                }
                let snapshot = store.GetSnapshot(version);
                count_relational_rows(snapshot.as_ref(), table)
            })
        }
    }

    /// Locking reads combine the newest committed rows with this transaction's
    /// local changes. This preserves read-your-writes without hiding commits
    /// made after the transaction snapshot (the RC/FOR UPDATE behavior).
    pub(super) fn scan_latest_with_transaction_overlay(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<Vec<RelationalRow>> {
        let flags = self.dml_type_flags();
        let row_map = |rows: Vec<RelationalRow>| {
            rows.into_iter()
                .map(|row| {
                    let key = encode_relational_row(table, &row.1, flags)?.0.0;
                    Ok((key, row))
                })
                .collect::<SessionResult<BTreeMap<_, _>>>()
        };
        let mut latest = self.domain.storage().with_storage(|store| {
            let version = store
                .CurrentVersion("global")
                .map_err(|error| session_error("get latest relational version", error))?;
            let snapshot = store.GetSnapshot(version);
            row_map(scan_relational_rows(snapshot.as_ref(), table)?)
        })?;
        let state = self.state.borrow();
        let Some(transaction) = state.transaction.as_ref() else {
            return Ok(latest.into_values().collect());
        };
        let base = row_map(scan_relational_rows(transaction.GetSnapshot(), table)?)?;
        let visible = row_map(scan_relational_rows(transaction.as_ref(), table)?)?;
        for key in base.keys() {
            if !visible.contains_key(key) {
                latest.remove(key);
            }
        }
        for (key, row) in visible {
            if base.get(&key) != Some(&row) {
                latest.insert(key, row);
            }
        }
        Ok(latest.into_values().collect())
    }

    /// Range-limited counterpart of [`Self::scan_latest_with_transaction_overlay`].
    pub(super) fn scan_latest_with_transaction_overlay_ranges(
        &self,
        table: &astersql_meta_model::TableInfo,
        ranges: &[RelationalRowScanRange],
    ) -> SessionResult<Vec<RelationalRow>> {
        let flags = self.dml_type_flags();
        let row_map = |rows: Vec<RelationalRow>| {
            rows.into_iter()
                .map(|row| {
                    let key = encode_relational_row(table, &row.1, flags)?.0.0;
                    Ok((key, row))
                })
                .collect::<SessionResult<BTreeMap<_, _>>>()
        };
        let mut latest = self.domain.storage().with_storage(|store| {
            let version = store
                .CurrentVersion("global")
                .map_err(|error| session_error("get latest relational version", error))?;
            let snapshot = store.GetSnapshot(version);
            row_map(scan_relational_row_ranges(
                snapshot.as_ref(),
                table,
                ranges,
                0,
                usize::MAX,
            )?)
        })?;
        let state = self.state.borrow();
        let Some(transaction) = state.transaction.as_ref() else {
            return Ok(latest.into_values().collect());
        };
        let base = row_map(scan_relational_row_ranges(
            transaction.GetSnapshot(),
            table,
            ranges,
            0,
            usize::MAX,
        )?)?;
        let visible = row_map(scan_relational_row_ranges(
            transaction.as_ref(),
            table,
            ranges,
            0,
            usize::MAX,
        )?)?;
        for key in base.keys() {
            if !visible.contains_key(key) {
                latest.remove(key);
            }
        }
        for (key, row) in visible {
            if base.get(&key) != Some(&row) {
                latest.insert(key, row);
            }
        }
        Ok(latest.into_values().collect())
    }

    /// 读取原始 KV 值。
    pub(super) fn read_raw_kv(&self, key: kv::Key) -> SessionResult<Option<Vec<u8>>> {
        let state = self.state.borrow();
        let result = if let Some(transaction) = state.transaction.as_ref() {
            transaction.Get(&kv::Context::default(), key, &[])
        } else {
            self.domain.storage().with_storage(|store| {
                let version = store.CurrentVersion("global")?;
                store
                    .GetSnapshot(version)
                    .Get(&kv::Context::default(), key, &[])
            })
        };
        match result {
            Ok(value) => Ok(Some(value.Value)),
            Err(error) if kv::IsErrNotFound(&error) => Ok(None),
            Err(error) => Err(session_error("read relational KV", error)),
        }
    }

    /// Read the newest committed value, deliberately bypassing an active
    /// transaction snapshot. Pessimistic uniqueness checks run this only
    /// after owning the corresponding exclusive row lock.
    pub(super) fn read_latest_raw_kv(&self, key: kv::Key) -> SessionResult<Option<Vec<u8>>> {
        self.domain.storage().with_storage(|store| {
            let version = store
                .CurrentVersion("global")
                .map_err(|error| session_error("get latest relational version", error))?;
            let snapshot = store.GetSnapshot(version);
            match snapshot.Get(&kv::Context::default(), key, &[]) {
                Ok(value) => Ok(Some(value.Value)),
                Err(error) if kv::IsErrNotFound(&error) => Ok(None),
                Err(error) => Err(session_error("read latest relational KV", error)),
            }
        })
    }
}
