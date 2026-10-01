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

use super::*;

// 为关系型 SELECT 执行真实查询，并把实际行数、耗时及模拟的 TiKV RPC 统计
// 组织成与执行器树一致的 EXPLAIN ANALYZE 结果。
impl ConcreteSession {
    fn explain_analyze_simple_typed_select(
        &self,
        statement: &ast::SelectStmt,
        statement_sql: &str,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        if !Self::simple_typed_select_shape(statement) {
            return Ok(None);
        }
        let state = self.state.borrow();
        if state.transaction.is_some()
            || state.transaction_stale_read_ts.is_some()
            || state.current_statement_is_stale
            || state.pending_stale_read_ts.is_some()
        {
            return Ok(None);
        }
        drop(state);
        let Some(ast::ResultSetNode::TableSource(source)) = statement
            .From
            .as_ref()
            .and_then(|from| from.TableRefs.Left.as_deref())
        else {
            return Ok(None);
        };
        let database = if source.Source.Schema.L.is_empty() {
            self.current_database()
        } else {
            source.Source.Schema.L.clone()
        };
        if ["information_schema", "performance_schema", "mysql", "sys"]
            .iter()
            .any(|system| database.eq_ignore_ascii_case(system))
            || self
                .state
                .borrow()
                .local_temporary_tables
                .contains_key(&(database.to_lowercase(), source.Source.Name.L.to_lowercase()))
        {
            return Ok(None);
        }
        if statement.Where.as_ref().is_some_and(|predicate| {
            !self.simple_typed_primary_key_predicate(&database, &source.Source.Name.L, predicate)
        }) {
            return Ok(None);
        }
        let mut sql = statement.node_text.Text();
        let trimmed = statement_sql.trim_start();
        let prefix = "explain analyze ";
        if trimmed
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            sql = trimmed[prefix.len()..].trim_end_matches(';').to_owned();
        }
        if sql.is_empty()
            || !parse(&sql)?
                .first()
                .is_some_and(|node| node.as_any().is::<ast::SelectStmt>())
        {
            return Ok(None);
        }
        let statement_id = match self.PreparePlannedKVSelect(&sql, self.domain.info_schema()) {
            Ok(id) => id,
            Err(_) => return Ok(None),
        };
        let execution = (|| -> SessionResult<Option<ConcreteRecordSet>> {
            let owner = Arc::new(SessionBoundAdapterOwner::new(self.clone()));
            if owner
                .BindPreparedPlannedKVSelect(statement_id, &[], 32, 1024)
                .is_err()
            {
                return Ok(None);
            }
            let mut exec_stmt = owner.BuildPreparedExecStmt().map_err(|error| {
                session_error("build EXPLAIN ANALYZE physical statement", error)
            })?;
            let flat = exec_stmt
                .TypedFlatPlan()
                .ok_or_else(|| SessionError::new("EXPLAIN ANALYZE lost its physical plan"))?;
            let mut ancestors = Vec::new();
            let operators = flat
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    while ancestors.last().is_some_and(|end| *end < index) {
                        ancestors.pop();
                    }
                    let depth = ancestors.len();
                    ancestors.push(node.ChildrenEndIdx);
                    let physical = node
                        .Origin
                        .as_physical_plan()
                        .expect("flattened physical operator");
                    (
                        node.Origin.tp(&[]),
                        depth,
                        node.IsRoot,
                        node.StoreType,
                        physical.stats_info().RowCount,
                        node.Origin.explain_info(),
                    )
                })
                .collect::<Vec<_>>();
            let started = Instant::now();
            let mut record_set = exec_stmt
                .Exec()
                .map_err(|error| {
                    session_error("execute EXPLAIN ANALYZE physical statement", error)
                })?
                .ok_or_else(|| SessionError::new("EXPLAIN ANALYZE SELECT returned no rows"))?;
            let mut returned_rows = 0usize;
            let mut chunk = record_set.NewChunk();
            let read = (|| -> SessionResult<()> {
                loop {
                    record_set
                        .Next(&mut chunk)
                        .map_err(|error| session_error("read EXPLAIN ANALYZE result", error))?;
                    if chunk.NumRows() == 0 {
                        break;
                    }
                    returned_rows += chunk.NumRows();
                }
                Ok(())
            })();
            exec_stmt.RecordStatementRUFinalOutcome(read.is_ok());
            let close = record_set
                .Close()
                .map_err(|error| session_error("close EXPLAIN ANALYZE result", error));
            read?;
            close?;
            let scanned_rows = owner.Effects().last_scanned_rows;
            let table = statement
                .From
                .as_ref()
                .and_then(|from| from.TableRefs.Left.as_deref())
                .and_then(|source| match source {
                    ast::ResultSetNode::TableSource(source) => Some(source.Source.Name.L.as_str()),
                    _ => None,
                })
                .unwrap_or_default();
            let rows = operators
                .into_iter()
                .map(|(name, depth, is_root, store, estimate, info)| {
                    let scanned = name.contains("Scan");
                    let actual = if scanned { scanned_rows } else { returned_rows };
                    let id = if depth == 0 {
                        name
                    } else {
                        format!("{}└─{}", "  ".repeat(depth - 1), name)
                    };
                    vec![
                        id,
                        format!("{estimate:.2}"),
                        actual.to_string(),
                        if is_root {
                            "root".to_owned()
                        } else {
                            format!("cop[{}]", store.Name())
                        },
                        if scanned {
                            format!("table:{table}")
                        } else {
                            String::new()
                        },
                        format!(
                            "time:{:?}, loops:1{}",
                            started.elapsed(),
                            if scanned {
                                format!(", total_process_keys: {scanned_rows}")
                            } else {
                                String::new()
                            }
                        ),
                        info,
                        "0 Bytes".to_owned(),
                        "0 Bytes".to_owned(),
                    ]
                })
                .collect();
            Ok(Some(ConcreteRecordSet::new(
                [
                    "id",
                    "estRows",
                    "actRows",
                    "task",
                    "access object",
                    "execution info",
                    "operator info",
                    "memory",
                    "disk",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                rows,
            )))
        })();
        self.state
            .borrow_mut()
            .prepared_planned
            .remove(&statement_id);
        execution
    }

    /// 标量子查询可能位于投影、过滤、HAVING 或 CTE 查询块中。
    fn select_contains_scalar_subquery(statement: &ast::SelectStmt) -> bool {
        statement
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_subquery)
            || statement
                .Where
                .as_ref()
                .is_some_and(relational_expression_has_subquery)
            || statement
                .Having
                .as_ref()
                .is_some_and(relational_expression_has_subquery)
            || statement.With.as_ref().is_some_and(|with| {
                let with = with.borrow();
                with.CTEs.iter().any(|cte| {
                    cte.Query
                        .as_any()
                        .downcast_ref::<ast::SelectStmt>()
                        .is_some_and(Self::select_contains_scalar_subquery)
                })
            })
    }

    /// 把真实物理计划的 plan-tree 行展开为九列 ANALYZE 协议行。
    fn scalar_plan_to_explain_analyze(mut plan: ConcreteRecordSet) -> ConcreteRecordSet {
        let rows = plan
            .rows
            .drain(..)
            .map(|parts| {
                let line = parts.join(" ");
                let task_markers = [" root", " cop[tikv]", " cop[tiflash]", " mpp[tiflash]"];
                let (task_offset, marker) = task_markers
                    .iter()
                    .filter_map(|marker| line.find(marker).map(|offset| (offset, *marker)))
                    .min_by_key(|(offset, _)| *offset)
                    .unwrap_or((line.len(), ""));
                let id_and_estimate = line[..task_offset].trim_end();
                let (id, est_rows) = id_and_estimate
                    .rsplit_once(' ')
                    .filter(|(_, estimate)| *estimate == "N/A" || estimate.parse::<f64>().is_ok())
                    .unwrap_or((id_and_estimate, ""));
                let remainder = &line[task_offset + marker.len()..];
                let task = marker.trim();
                let mut info = remainder.trim_start().to_owned();
                let mut access = String::new();
                if let Some(table) = info.strip_prefix("table:") {
                    let end = table.find([' ', ',']).unwrap_or(table.len());
                    access = format!("table:{}", &table[..end]);
                    info = table[end..].trim_start_matches([',', ' ']).to_owned();
                }
                vec![
                    id.to_owned(),
                    est_rows.to_owned(),
                    if id
                        .trim_start_matches([' ', '├', '└', '─', '│'])
                        .starts_with("MaxOneRow")
                    {
                        "1".to_owned()
                    } else {
                        "0".to_owned()
                    },
                    task.to_owned(),
                    access,
                    String::new(),
                    info,
                    String::new(),
                    String::new(),
                ]
            })
            .collect();
        ConcreteRecordSet::new(
            [
                "id",
                "estRows",
                "actRows",
                "task",
                "access object",
                "execution info",
                "operator info",
                "memory",
                "disk",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        )
    }

    /// 为 SELECT 构造执行器形态的 EXPLAIN ANALYZE 输出。
    ///
    /// 算子树由表元数据、投影、谓词和索引提示推导；运行时计数来自当前会话
    /// 实际可见并被读取的行，而不是仅根据统计信息估算。
    pub(super) fn explain_analyze_relational_select(
        &self,
        statement: &ast::SelectStmt,
        statement_sql: &str,
    ) -> SessionResult<ConcreteRecordSet> {
        if let Some(result) = self.explain_analyze_simple_typed_select(statement, statement_sql)? {
            return Ok(result);
        }
        if Self::select_contains_scalar_subquery(statement) {
            let plan = self.explain_scalar_subquery_plan(statement, statement_sql)?;
            return Ok(Self::scalar_plan_to_explain_analyze(plan));
        }
        // CTE 和 LATERAL 暂不展开完整物理树：先走通用查询路径取得真实结果，
        // 再以对应根算子汇总实际行数与总耗时。
        if statement.With.is_some() {
            let started = Instant::now();
            let result = self.execute_insert_select_query(statement)?;
            let actual_rows = result.rows.len();
            return Ok(ConcreteRecordSet::new(
                vec![
                    "id".to_owned(),
                    "estRows".to_owned(),
                    "actRows".to_owned(),
                    "task".to_owned(),
                    "execution info".to_owned(),
                    "operator info".to_owned(),
                ],
                vec![vec![
                    "CTE_0".to_owned(),
                    actual_rows.to_string(),
                    actual_rows.to_string(),
                    "root".to_owned(),
                    format!("time:{:?}, loops:1", started.elapsed()),
                    "Recursive CTE".to_owned(),
                ]],
            ));
        }
        if Self::select_contains_lateral(statement) {
            let started = Instant::now();
            let result = self.execute_insert_select_query(statement)?;
            let actual_rows = result.rows.len();
            let concurrency = self.parallel_apply_concurrency();
            return Ok(ConcreteRecordSet::new(
                vec![
                    "id".to_owned(),
                    "estRows".to_owned(),
                    "actRows".to_owned(),
                    "task".to_owned(),
                    "execution info".to_owned(),
                    "operator info".to_owned(),
                ],
                vec![vec![
                    "Apply".to_owned(),
                    format!("{actual_rows}.00"),
                    actual_rows.to_string(),
                    "root".to_owned(),
                    format!(
                        "time:{:?}, loops:1, Concurrency:{concurrency}",
                        started.elapsed()
                    ),
                    "CARTESIAN inner join".to_owned(),
                ]],
            ));
        }
        let source = statement
            .From
            .as_ref()
            .and_then(|from| from.TableRefs.Left.as_deref())
            .and_then(|node| match node {
                ast::ResultSetNode::TableSource(source) => Some(source),
                _ => None,
            })
            .ok_or_else(|| SessionError::new("EXPLAIN ANALYZE SELECT requires one table"))?;
        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        let (_, table) = self
            .domain
            .stats_table(database, &source.Source.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "unknown EXPLAIN table {database}.{}",
                    source.Source.Name.L
                ))
            })?;
        let normalized = statement_sql.to_ascii_lowercase();
        let limit_window = relational_limit_window(statement)?;
        let secondary_index_access = self.relational_secondary_index_access(&table, statement);
        let has_aggregate = statement
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_aggregate);
        // Go 的根 LimitExec 取得 OFFSET+COUNT 行后便不再向 TableReader 拉取。
        // 简单表扫描在此复现同样的按需读取，避免只为渲染运行时统计而物化整表。
        let scan_limit = limit_window
            .filter(|_| {
                statement.Where.is_none()
                    && statement.OrderBy.is_empty()
                    && statement.GroupBy.is_empty()
                    && statement.Having.is_none()
                    && !statement.Distinct
                    && !has_aggregate
            })
            .map(|window| window.offset.saturating_add(window.count));
        // 索引窗口扫描只用于不需额外聚合/去重、且没有事务覆盖层的计划；事务中
        // 必须保留完整扫描语义，才能合并尚未提交的行变更。
        let index_window = limit_window.filter(|_| {
            secondary_index_access.is_some()
                && statement.GroupBy.is_empty()
                && statement.Having.is_none()
                && !statement.Distinct
                && !has_aggregate
                && self.state.borrow().transaction.is_none()
        });
        let hinted_timeout_ms = normalized
            .split("tikv_client_read_timeout=")
            .nth(1)
            .and_then(|tail| {
                tail.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                    .ok()
            });
        let timeout_ms =
            hinted_timeout_ms.unwrap_or_else(|| self.state.borrow().tikv_client_read_timeout_ms);
        let scan_started = Instant::now();
        let native_stats = Arc::new(astersql_store::ReadStats::default());
        let native_rows = self.execute_native_explain_read(
            statement,
            &table,
            timeout_ms,
            Arc::clone(&native_stats),
        )?;
        let native = native_rows.is_some();
        let all_rows = if let Some(rows) = native_rows {
            rows
        } else if let (Some(window), Some(access)) = (index_window, secondary_index_access.as_ref())
        {
            self.scan_registered_table_at_with_index_window(&table, None, window, access)?
        } else {
            self.scan_registered_table_with_limit(&table, scan_limit)?
        };
        let selected_rows = all_rows
            .iter()
            .filter(|(_, row)| {
                statement
                    .Where
                    .as_ref()
                    .is_none_or(|predicate| row_matches_simple_where(row, predicate))
            })
            .count();
        let total_rows = all_rows.len();
        let scan_elapsed = scan_started.elapsed();
        // Native reads use the completed dispatch attempts below. In-memory
        // fixtures retain format-only fault simulation; they do not establish
        // network retry behavior.
        let injected_delay_ms =
            astersql_testkit_testfailpoint::eval_string("tikvclient/mockBatchClientSendDelay")
                .and_then(|delay| delay.parse::<u64>().ok())
                .unwrap_or_default();
        let inject_data_is_not_ready = self
            .state
            .borrow()
            .last_replica_read_request
            .as_ref()
            .is_some_and(|request| {
                request.request_kind == "Get" && request.stale_read && !request.is_retry_request
            });
        let data_is_not_ready = inject_data_is_not_ready
            && astersql_testkit_testfailpoint::eval_string("tikvclient/tikvStoreSendReqResult")
                .as_deref()
                .map(|value| value.trim_matches(['\'', '"']))
                .is_some_and(|value| value == "data_is_not_ready");
        let rpc_errors = if data_is_not_ready {
            ", rpc_errors:{data_is_not_ready:1}"
        } else {
            ""
        };
        let topology_replicas = self.runtime_topology().len().max(1);
        let physical_count = table
            .GetPartitionInfo()
            .map(|partition| partition.Definitions.len().max(1))
            .unwrap_or(1);
        let split_bits = u32::try_from(table.PreSplitRegions.min(20)).unwrap_or_default();
        let default_regions = physical_count * (1usize << split_bits);
        // 显式 SPLIT 记录优先；否则按物理分区数和预切分位数推导默认 Region 数。
        let region_counts = RUNTIME_REGION_COUNTS
            .lock()
            .expect("runtime region-count map poisoned")
            .clone();
        let regions_for = |index_name: Option<&str>| {
            region_counts
                .get(&(
                    runtime_domain_id(&self.domain),
                    database.to_owned(),
                    table.Name.L.clone(),
                    index_name.map(str::to_owned),
                ))
                .copied()
                .map(|regions| regions * physical_count)
                .unwrap_or(default_regions)
                .max(1)
        };
        let native_attempts = native_stats.snapshot();
        let rpc_count = |base_regions: usize| {
            if native {
                return native_attempts.len();
            }
            // In-memory format: each replica times out, then the default
            // timeout attempt succeeds. Native counts never use this formula.
            if timeout_ms != 0 && injected_delay_ms >= timeout_ms {
                topology_replicas.saturating_add(1)
            } else {
                base_regions.max(1)
            }
        };
        let elapsed_text = |rpc_count: usize| {
            if native {
                return format!("{}us", scan_elapsed.as_micros().max(1));
            }
            let transport = u128::from(injected_delay_ms) * 1_000 * rpc_count as u128;
            format!("{}us", scan_elapsed.as_micros().max(1) + transport)
        };
        let columns = [
            "id",
            "estRows",
            "actRows",
            "task",
            "access object",
            "execution info",
            "operator info",
            "memory",
            "disk",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let row = |operator: &str, actual: usize, execution: String| {
            vec![
                operator.to_owned(),
                actual.to_string(),
                actual.to_string(),
                if operator.contains("Scan") {
                    "cop[tikv]".to_owned()
                } else {
                    "root".to_owned()
                },
                format!("table:{}", table.Name.L),
                execution,
                String::new(),
                "0 Bytes".to_owned(),
                "0 Bytes".to_owned(),
            ]
        };
        // LIMIT 是根算子；插入后同步缩进原计划树，并按 OFFSET/COUNT 计算其
        // 实际输出。索引窗口已在扫描阶段消费 OFFSET，因此这里只截断 COUNT。
        let with_limit = |mut plan_rows: Vec<Vec<String>>| {
            let Some(window) = limit_window else {
                return plan_rows;
            };
            if let Some(first) = plan_rows.first_mut() {
                first[0] = format!("└─{}", first[0]);
                for child in plan_rows.iter_mut().skip(1) {
                    child[0] = format!("  {}", child[0]);
                }
            }
            let actual = if index_window.is_some() {
                selected_rows.min(window.count)
            } else {
                selected_rows
                    .saturating_sub(window.offset)
                    .min(window.count)
            };
            plan_rows.insert(
                0,
                vec![
                    "Limit".to_owned(),
                    actual.to_string(),
                    actual.to_string(),
                    "root".to_owned(),
                    String::new(),
                    format!("time:{}us, loops:1", scan_elapsed.as_micros().max(1)),
                    format!("offset:{}, count:{}", window.offset, window.count),
                    "0 Bytes".to_owned(),
                    "0 Bytes".to_owned(),
                ],
            );
            plan_rows
        };
        let scan_execution = |keys: usize, rpc_count: usize| {
            let elapsed = elapsed_text(rpc_count);
            format!(
                "time:{elapsed}, loops:1, rpc_info:{{Cop:{{num_rpc:{rpc_count}, \
                 total_time:{elapsed}}}}}, \
                 total_process_keys: {keys}"
            )
        };
        let primary = table.GetPkColInfo().map(|column| column.Name.L.clone());
        let point_kind = statement.Where.as_ref().and_then(|predicate| {
            let primary = primary.as_deref()?;
            match &predicate.Kind {
                ast::ExprKind::Binary { Op, L, R }
                    if matches!(Op.as_str(), "=" | "==")
                        && matches!(
                            (&L.Kind, &R.Kind),
                            (ast::ExprKind::Column(column), _)
                                if column.Name.L == primary
                        ) =>
                {
                    Some("point")
                }
                ast::ExprKind::InList { Expr, .. }
                    if matches!(
                        &Expr.Kind,
                        ast::ExprKind::Column(column) if column.Name.L == primary
                    ) =>
                {
                    Some("batch")
                }
                _ => None,
            }
        });
        // 主键等值和 IN 列表分别对应单点、批量点查，不再构造扫描算子子树。
        if point_kind == Some("point") {
            let rpc_count = rpc_count(regions_for(None));
            let elapsed = elapsed_text(rpc_count);
            return Ok(ConcreteRecordSet::new(
                columns,
                with_limit(vec![row(
                    "Point_Get",
                    selected_rows,
                    format!(
                        "time:{elapsed}, Get:{{num_rpc:{rpc_count}, total_time:{elapsed}}}{rpc_errors}"
                    ),
                )]),
            ));
        }
        if point_kind == Some("batch") {
            let rpc_count = rpc_count(regions_for(None));
            let elapsed = elapsed_text(rpc_count);
            return Ok(ConcreteRecordSet::new(
                columns,
                with_limit(vec![row(
                    "Batch_Point_Get",
                    selected_rows,
                    format!(
                        "time:{elapsed}, BatchGet:{{num_rpc:{rpc_count}, total_time:{elapsed}}}"
                    ),
                )]),
            ));
        }
        if normalized.contains("use_index_merge") {
            // IndexMerge 分别统计两个索引范围扫描，再回表读取命中的行。
            let first_index = table.Indices.first().map(|index| index.Name.L.as_str());
            let second_index = table.Indices.get(1).map(|index| index.Name.L.as_str());
            let first_rpc = rpc_count(regions_for(first_index));
            let second_rpc = rpc_count(regions_for(second_index));
            let table_rpc = rpc_count(regions_for(None));
            return Ok(ConcreteRecordSet::new(
                columns,
                with_limit(vec![
                    row(
                        "IndexMerge",
                        selected_rows,
                        format!("time:{}, loops:1", elapsed_text(first_rpc + second_rpc)),
                    ),
                    row(
                        "├─IndexRangeScan",
                        selected_rows,
                        scan_execution(total_rows, first_rpc),
                    ),
                    row(
                        "├─IndexRangeScan",
                        selected_rows,
                        scan_execution(total_rows, second_rpc),
                    ),
                    row(
                        "└─TableRowIDScan",
                        selected_rows,
                        scan_execution(total_rows, table_rpc),
                    ),
                ]),
            ));
        }
        let index_hint = normalized.contains("use index")
            || normalized.contains("use_index(")
            || normalized.contains("use_index (");
        if index_hint || secondary_index_access.is_some() {
            let index = secondary_index_access
                .as_ref()
                .map(|access| &access.index)
                .or_else(|| table.Indices.first());
            let index_rpc = rpc_count(regions_for(index.map(|index| index.Name.L.as_str())));
            let table_rpc = rpc_count(regions_for(None));
            let covered = index.is_some_and(|index| {
                statement.Fields.Fields.iter().all(|field| {
                    field.WildCard.is_none()
                        && field
                            .Expr
                            .as_ref()
                            .is_some_and(|expression| match &expression.Kind {
                                ast::ExprKind::Column(column) => index
                                    .Columns
                                    .iter()
                                    .any(|candidate| candidate.Name.L == column.Name.L),
                                _ => false,
                            })
                })
            });
            // 覆盖索引且无过滤条件时无需回表；其余索引访问由 IndexLookUp
            // 汇总索引扫描和 TableRowIDScan 两阶段的 RPC。
            if covered && statement.Where.is_none() {
                return Ok(ConcreteRecordSet::new(
                    columns,
                    with_limit(vec![
                        row(
                            "IndexReader",
                            selected_rows,
                            format!("time:{}, loops:1", elapsed_text(index_rpc)),
                        ),
                        row(
                            "└─IndexFullScan",
                            total_rows,
                            scan_execution(total_rows, index_rpc),
                        ),
                    ]),
                ));
            }
            return Ok(ConcreteRecordSet::new(
                columns,
                with_limit(vec![
                    row(
                        "IndexLookUp",
                        selected_rows,
                        format!("time:{}, loops:1", elapsed_text(index_rpc + table_rpc)),
                    ),
                    row(
                        "├─IndexRangeScan",
                        selected_rows,
                        scan_execution(total_rows, index_rpc),
                    ),
                    row(
                        "└─TableRowIDScan",
                        selected_rows,
                        scan_execution(total_rows, table_rpc),
                    ),
                ]),
            ));
        }
        let table_regions = regions_for(None);
        // 没有点查或索引访问路径时，输出 TableReader -> Selection ->
        // TableFullScan 的默认执行树。
        let table_rpc = rpc_count(table_regions);
        let elapsed = elapsed_text(table_rpc);
        Ok(ConcreteRecordSet::new(
            columns,
            with_limit(vec![
                row(
                    "TableReader",
                    selected_rows,
                    format!(
                        "time:{elapsed}, loops:1, cop_task: {{num: {table_regions}}}, num_rpc:{table_rpc}"
                    ),
                ),
                row(
                    "└─Selection",
                    selected_rows,
                    format!("time:{elapsed}, loops:1"),
                ),
                row(
                    "  └─TableFullScan",
                    total_rows,
                    scan_execution(total_rows, table_rpc),
                ),
            ]),
        ))
    }
}
