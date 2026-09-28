// Copyright 2026 AsterSQL.

use super::*;

impl ConcreteSession {
    /// Execute native EXPLAIN reads at the KV/Coprocessor boundary so the
    /// displayed RPC statistics describe dispatched requests, including retries.
    pub(super) fn execute_native_explain_read(
        &self,
        statement: &ast::SelectStmt,
        table: &astersql_meta_model::TableInfo,
        timeout_ms: u64,
        stats: Arc<astersql_store::ReadStats>,
    ) -> SessionResult<Option<Vec<RelationalRow>>> {
        if self.state.borrow().transaction.is_some() || table.GetPartitionInfo().is_some() {
            return Ok(None);
        }
        let source = statement
            .From
            .as_ref()
            .and_then(|from| from.TableRefs.Left.as_deref())
            .and_then(|node| match node {
                ast::ResultSetNode::TableSource(source) => Some(source),
                _ => None,
            });
        let explicit_ts = source
            .and_then(|source| source.AsOf.as_ref())
            .map(|as_of| self.evaluate_stale_read_ts(&as_of.TsExpr))
            .transpose()?;
        let read_ts = explicit_ts.or_else(|| {
            let state = self.state.borrow();
            state.snapshot_read_ts.or(state.session_stale_read_ts)
        });
        self.domain.storage().with_storage(|store| {
            if store.Name() != "TiKV" { return Ok(None); }
            let version = match read_ts {
                Some(ts) => kv::NewVersion(ts),
                None => store.CurrentVersion(kv::GlobalTxnScope).map_err(|error| session_error("EXPLAIN read TSO", error))?,
            };
            let handles = if table.PKIsHandle {
                primary_point_get_value(table, statement.Where.as_ref()).and_then(|value| value.parse::<i128>().ok()).map(|handle| vec![handle])
                    .or_else(|| {
                        let primary = table.GetPkColInfo()?;
                        let ast::ExprKind::InList { Expr, List, Not, .. } = &statement.Where.as_ref()?.Kind else { return None; };
                        if *Not || !matches!(&Expr.Kind, ast::ExprKind::Column(column) if column.Name.L == primary.Name.L) { return None; }
                        List.iter().map(|value| literal(value).ok()?.parse::<i128>().ok()).collect::<Option<Vec<_>>>()
                    })
            } else { None };
            if let Some(handles) = handles {
                let mut snapshot = store.GetSnapshot(version);
                snapshot.SetOption(kv::TiKVClientReadTimeout, Some(Box::new(timeout_ms)));
                snapshot.SetOption(kv::CollectRuntimeStats, Some(Box::new(Arc::clone(&stats))));
                let keys = handles.iter().map(|handle| relational_handle_key(table.ID, *handle)).collect::<Vec<_>>();
                let values = if keys.len() == 1 {
                    match snapshot.Get(&kv::Context::todo(), keys[0].clone(), &[]) {
                        Ok(value) => vec![(keys[0].clone(), value.Value)],
                        Err(error) if kv::IsErrNotFound(&error) => Vec::new(),
                        Err(error) => return Err(session_error("EXPLAIN PointGet", error)),
                    }
                } else {
                    let values = snapshot.BatchGet(&kv::Context::todo(), &keys, &[]).map_err(|error| session_error("EXPLAIN BatchPointGet", error))?;
                    keys.into_iter().filter_map(|key| values.get(&kv::KeyMapName(key.as_ref())).map(|value| (key.clone(), value.Value.clone()))).collect()
                };
                let fields = table.Columns.iter().map(|column| (column.ID, Box::new(column.FieldType.clone()))).collect();
                let mut rows = Vec::new();
                for (key, value) in values {
                    let (_, handle) = astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(key.0))
                        .map_err(|error| session_error("EXPLAIN decode handle", error))?;
                    rows.push(decode_relational_row_value(table, &fields, handle.as_ref(), &value)?);
                }
                return Ok(Some(rows));
            }
            let mut scan = tipb::TableScan::new();
            scan.set_table_id(table.ID);
            scan.set_columns(protobuf::RepeatedField::from_vec(table.Columns.iter().map(|column| relational_scan_column(table, column)).collect()));
            let mut executor = tipb::Executor::new();
            executor.set_tp(tipb::ExecType::TypeTableScan);
            executor.set_tbl_scan(scan);
            let mut dag = tipb::DagRequest::new();
            dag.set_executors(protobuf::RepeatedField::from_vec(vec![executor]));
            dag.set_output_offsets((0..table.Columns.len() as u32).collect());
            let mut request = relational_coprocessor_request(table, version.Ver)?;
            request.Data = protobuf::Message::write_to_bytes(&dag).map_err(|error| session_error("encode EXPLAIN DAG", error))?;
            request.TiKVClientReadTimeout = timeout_ms;
            request.IsStaleness = read_ts.is_some();
            request.ReplicaRead = if self.state.borrow().replica_read == "leader" { kv::ReplicaReadType::ReplicaReadLeader } else { kv::ReplicaReadType::ReplicaReadMixed };
            let option = kv::ClientSendOption { SessionMemTracker: None, EnabledRateLimitAction: false, EventCb: None,
                EnableCollectExecutionInfo: true, TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(), AppendWarning: None, TryCopLiteWorker: None };
            let context = kv::Context::todo();
            let mut response = store.GetClient().Send(&context, &request, &stats, &option)
                .ok_or_else(|| SessionError::new("EXPLAIN Coprocessor returned no response"))?;
            let result = (|| {
                let mut rows = Vec::new();
                while let Some(subset) = response.Next(&context).map_err(|error| session_error("EXPLAIN Coprocessor", error))? {
                    let result: tipb::SelectResponse = protobuf::parse_from_bytes(subset.GetData()).map_err(|error| session_error("decode EXPLAIN Coprocessor", error))?;
                    if result.has_error() { return Err(SessionError::new(result.get_error().get_msg())); }
                    for chunk in result.get_chunks() {
                        let mut encoded = chunk.get_rows_data();
                        while !encoded.is_empty() {
                            let mut row = HashMap::new();
                            for column in &table.Columns {
                                let (remaining, datum) = astersql_tablecodec::codec::DecodeOne(encoded).map_err(|error| session_error("decode EXPLAIN datum", error))?;
                                row.insert(column.Name.L.clone(), datum_to_runtime_value(&datum, Some(column))?);
                                encoded = remaining;
                            }
                            rows.push((0, row));
                        }
                    }
                }
                Ok(rows)
            })();
            let closed = response.Close().map_err(|error| session_error("close EXPLAIN Coprocessor", error));
            match (result, closed) { (Err(error), _) | (_, Err(error)) => Err(error), (Ok(rows), Ok(())) => Ok(Some(rows)) }
        })
    }
}
