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

//! Session planning contexts, statistics sync-load, and prepared-plan lifecycle.

use super::*;

struct SessionPlanColumnIDAllocator(Arc<astersql_sessionctx_variable::session::SessionVars>);

impl astersql_expression_exprctx::PlanColumnIDAllocator for SessionPlanColumnIDAllocator {
    fn AllocPlanColumnID(&self) -> i64 {
        self.0.AllocPlanColumnID()
    }

    fn GetLastPlanColumnID(&self) -> i64 {
        self.0.PlanColumnID.load(Ordering::SeqCst)
    }
}

struct SessionPlanContext {
    plan_id: AtomicI32,
    ignore_explain_id_suffix: bool,
    variables: Arc<astersql_sessionctx_variable::session::SessionVars>,
    expression: astersql_expression_exprstatic::ExprContext,
    build_pb: astersql_planner_core_base::BuildPBContext,
    ranger: astersql_planner_core_base::RangerContext<'static>,
    builtin_usage: astersql_planner_core_base::BuiltinFunctionUsageCounter,
    stats_load_waiter: Option<Arc<dyn astersql_planner_core_base::StatsLoadWaiter>>,
    prepared_marker_offsets: Vec<usize>,
}

/// 规划阶段只需要探测 TiKV 的表达式签名能力；真正发送请求仍使用 Storage Client。
struct SessionPushdownCapabilityClient;

impl kv::Client for SessionPushdownCapabilityClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _req: &kv::Request,
        _vars: &dyn Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("planning capability client never sends requests")
    }

    fn IsRequestTypeSupported(&self, request_type: i64, sub_type: i64) -> bool {
        kv::RequestTypeSupportedChecker.IsRequestTypeSupported(request_type, sub_type)
    }
}

/// 将 Domain 统计句柄适配为 syncload StatsHandle。
struct DomainStatsSyncLoadHandle {
    handle:
        Arc<Mutex<astersql_statistics_handle::Handle<astersql_domain::domain::DomainStatsBackend>>>,
    domain: Option<Arc<astersql_domain::Domain>>,
}

impl DomainStatsSyncLoadHandle {
    fn histogram_from_column(
        column: &astersql_statistics_handle::ColumnStats,
    ) -> astersql_statistics_handle_syncload::Histogram {
        astersql_statistics_handle_syncload::Histogram {
            ndv: column.ndv,
            null_count: column.null_count,
            total_column_size: column.total_column_size,
            correlation_bits: column.correlation.to_bits(),
            last_update_version: column.version,
            buckets: column
                .buckets
                .iter()
                .map(|bucket| {
                    (
                        bucket.lower.clone(),
                        bucket.upper.clone(),
                        u64::try_from(bucket.count).unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }

    fn histogram_from_index(
        index: &astersql_statistics_handle::IndexStats,
    ) -> astersql_statistics_handle_syncload::Histogram {
        astersql_statistics_handle_syncload::Histogram {
            ndv: index.ndv,
            null_count: index.null_count,
            correlation_bits: index.correlation.to_bits(),
            last_update_version: index.version,
            buckets: index
                .buckets
                .iter()
                .map(|bucket| {
                    (
                        bucket.lower.clone(),
                        bucket.upper.clone(),
                        u64::try_from(bucket.count).unwrap_or_default(),
                    )
                })
                .collect(),
            ..Default::default()
        }
    }
}

impl astersql_statistics_handle_syncload::StatsStorage for DomainStatsSyncLoadHandle {
    fn HistMetaFromStorageWithHighPriority(
        &self,
        item: astersql_statistics_handle_syncload::TableItemID,
        _column_info: Option<&astersql_statistics_handle_syncload::ColumnInfo>,
    ) -> astersql_statistics_handle_syncload::Result<
        Option<(astersql_statistics_handle_syncload::Histogram, i64)>,
    > {
        let persisted = self
            .domain
            .as_ref()
            .and_then(|domain| domain.persisted_table_stats(item.TableID));
        let handle = self
            .handle
            .lock()
            .map_err(|_| astersql_statistics_handle_syncload::Error::Poisoned)?;
        let Some(table) = persisted
            .as_ref()
            .or_else(|| handle.stats_meta(item.TableID))
        else {
            return Ok(None);
        };
        if item.IsIndex {
            Ok(table
                .indexes
                .get(&item.ID)
                .map(|index| (Self::histogram_from_index(index), index.stats_version)))
        } else {
            Ok(table
                .columns
                .get(&item.ID)
                .map(|column| (Self::histogram_from_column(column), column.stats_version)))
        }
    }

    fn HistogramFromStorageWithHighPriority(
        &self,
        item: astersql_statistics_handle_syncload::TableItemID,
        column_info: Option<&astersql_statistics_handle_syncload::ColumnInfo>,
        _metadata: &astersql_statistics_handle_syncload::Histogram,
    ) -> astersql_statistics_handle_syncload::Result<astersql_statistics_handle_syncload::Histogram>
    {
        self.HistMetaFromStorageWithHighPriority(item, column_info)?
            .map(|(histogram, _)| histogram)
            .ok_or(astersql_statistics_handle_syncload::Error::HistogramMetaNotFound)
    }

    fn CMSketchAndTopNFromStorageWithHighPriority(
        &self,
        item: astersql_statistics_handle_syncload::TableItemID,
        _stats_version: i64,
    ) -> astersql_statistics_handle_syncload::Result<(
        Option<astersql_statistics_handle_syncload::CmsSketch>,
        Option<astersql_statistics_handle_syncload::TopN>,
    )> {
        let persisted = self
            .domain
            .as_ref()
            .and_then(|domain| domain.persisted_table_stats(item.TableID));
        let handle = self
            .handle
            .lock()
            .map_err(|_| astersql_statistics_handle_syncload::Error::Poisoned)?;
        let table = persisted
            .as_ref()
            .or_else(|| handle.stats_meta(item.TableID));
        let values = if item.IsIndex {
            table
                .and_then(|table| table.indexes.get(&item.ID))
                .map(|index| index.top_n.clone())
        } else {
            table
                .and_then(|table| table.columns.get(&item.ID))
                .map(|column| column.top_n.clone())
        };
        Ok((
            None,
            values.map(|values| astersql_statistics_handle_syncload::TopN { values }),
        ))
    }
}

impl astersql_statistics_handle_syncload::StatsHandle for DomainStatsSyncLoadHandle {
    fn Get(&self, table_id: i64) -> Option<astersql_statistics_handle_syncload::TableStats> {
        let handle = self.handle.lock().ok()?;
        let table = handle.stats_meta(table_id)?;
        let columns = table
            .columns
            .iter()
            .map(|(id, column)| {
                (
                    *id,
                    astersql_statistics_handle_syncload::Column {
                        physical_id: table_id,
                        histogram: Self::histogram_from_column(column),
                        info: astersql_statistics_handle_syncload::ColumnInfo {
                            id: *id,
                            field_type: column.field_type.to_string(),
                            primary_key: false,
                        },
                        cms: None,
                        top_n: Some(astersql_statistics_handle_syncload::TopN {
                            values: column.top_n.clone(),
                        }),
                        is_handle: false,
                        stats_version: column.stats_version,
                        loaded_status: if column.loaded_or_evicted {
                            astersql_statistics_handle_syncload::LoadedStatus::Full
                        } else {
                            astersql_statistics_handle_syncload::LoadedStatus::Evicted
                        },
                    },
                )
            })
            .collect();
        let indices = table
            .indexes
            .iter()
            .map(|(id, index)| {
                (
                    *id,
                    astersql_statistics_handle_syncload::Index {
                        physical_id: table_id,
                        histogram: Self::histogram_from_index(index),
                        info: astersql_statistics_handle_syncload::IndexInfo { id: *id },
                        cms: None,
                        top_n: Some(astersql_statistics_handle_syncload::TopN {
                            values: index.top_n.clone(),
                        }),
                        stats_version: index.stats_version,
                        loaded_status: if index.fully_loaded {
                            astersql_statistics_handle_syncload::LoadedStatus::Full
                        } else {
                            astersql_statistics_handle_syncload::LoadedStatus::Evicted
                        },
                    },
                )
            })
            .collect();
        Some(astersql_statistics_handle_syncload::TableStats {
            columns,
            indices,
            analyzed_columns: table
                .columns
                .iter()
                .filter_map(|(id, column)| column.analyzed_or_synthesized.then_some(*id))
                .collect(),
            column_exists: table.columns.keys().map(|id| (*id, true)).collect(),
            index_exists: table.indexes.keys().map(|id| (*id, true)).collect(),
            stats_version: i32::try_from(table.stats_version).unwrap_or_default(),
        })
    }

    fn TableInfoByID(
        &self,
        table_id: i64,
    ) -> Option<astersql_statistics_handle_syncload::TableInfo> {
        let table = self.Get(table_id)?;
        Some(astersql_statistics_handle_syncload::TableInfo {
            pk_is_handle: false,
            columns: table
                .columns
                .iter()
                .map(|(id, column)| (*id, column.info.clone()))
                .collect(),
            indices: table
                .indices
                .iter()
                .map(|(id, index)| (*id, index.info.clone()))
                .collect(),
        })
    }

    fn UpdateStatsCache(
        &self,
        table_id: i64,
        table: astersql_statistics_handle_syncload::TableStats,
    ) -> astersql_statistics_handle_syncload::Result<()> {
        let mut handle = self
            .handle
            .lock()
            .map_err(|_| astersql_statistics_handle_syncload::Error::Poisoned)?;
        let mut cached = handle.stats_meta(table_id).cloned().unwrap_or_else(|| {
            astersql_statistics_handle::TableStats {
                physical_id: table_id,
                initialized: true,
                ..Default::default()
            }
        });
        for (id, column) in table.columns {
            let cached_column = cached.columns.entry(id).or_default();
            cached_column.analyzed_or_synthesized = table.analyzed_columns.contains(&id);
            cached_column.stats_version = column.stats_version;
            cached_column.ndv = column.histogram.ndv;
            cached_column.null_count = column.histogram.null_count;
            cached_column.total_column_size = column.histogram.total_column_size;
            cached_column.version = column.histogram.last_update_version;
            cached_column.loaded_or_evicted =
                column.loaded_status == astersql_statistics_handle_syncload::LoadedStatus::Full;
            cached_column.correlation = f64::from_bits(column.histogram.correlation_bits);
            cached_column.top_n = column.top_n.map_or_else(Vec::new, |top_n| top_n.values);
            cached_column.buckets = column
                .histogram
                .buckets
                .into_iter()
                .map(|(lower, upper, count)| astersql_statistics_handle::Bucket {
                    count: i64::try_from(count).unwrap_or(i64::MAX),
                    repeats: 0,
                    lower,
                    upper,
                    ndv: 0,
                })
                .collect();
        }
        for (id, index) in table.indices {
            let cached_index = cached.indexes.entry(id).or_default();
            cached_index.analyzed = index.stats_version != 0;
            cached_index.stats_version = index.stats_version;
            cached_index.version = index.histogram.last_update_version;
            cached_index.ndv = index.histogram.ndv;
            cached_index.null_count = index.histogram.null_count;
            cached_index.correlation = f64::from_bits(index.histogram.correlation_bits);
            cached_index.fully_loaded =
                index.loaded_status == astersql_statistics_handle_syncload::LoadedStatus::Full;
            cached_index.top_n = index.top_n.map_or_else(Vec::new, |top_n| top_n.values);
            cached_index.buckets = index
                .histogram
                .buckets
                .into_iter()
                .map(|(lower, upper, count)| astersql_statistics_handle::Bucket {
                    count: i64::try_from(count).unwrap_or(i64::MAX),
                    repeats: 0,
                    lower,
                    upper,
                    ndv: 0,
                })
                .collect();
        }
        handle.cache_mut().put(cached);
        Ok(())
    }

    fn Storage(&self) -> &dyn astersql_statistics_handle_syncload::StatsStorage {
        self
    }

    fn Lease(&self) -> Duration {
        Duration::from_millis(1)
    }
}

/// 会话统计同步加载适配器，实现 StatsLoadWaiter。
pub(crate) struct SessionStatsSyncLoadAdapter {
    loader: astersql_statistics_handle_syncload::statsSyncLoad,
}

impl SessionStatsSyncLoadAdapter {
    /// 用 Domain 统计句柄构造同步加载适配器。
    pub(crate) fn new(
        handle: Arc<
            Mutex<astersql_statistics_handle::Handle<astersql_domain::domain::DomainStatsBackend>>,
        >,
    ) -> Self {
        let handle: Arc<dyn astersql_statistics_handle_syncload::StatsHandle> =
            Arc::new(DomainStatsSyncLoadHandle {
                handle,
                domain: None,
            });
        Self {
            loader: astersql_statistics_handle_syncload::NewStatsSyncLoad(handle, 128),
        }
    }

    /// 用 Domain 构造生产同步加载器，存储读取使用 ANALYZE 的持久化载荷。
    pub(crate) fn new_with_domain(domain: Arc<astersql_domain::Domain>) -> Self {
        let handle: Arc<dyn astersql_statistics_handle_syncload::StatsHandle> =
            Arc::new(DomainStatsSyncLoadHandle {
                handle: domain.stats_handle(),
                domain: Some(domain),
            });
        Self {
            loader: astersql_statistics_handle_syncload::NewStatsSyncLoad(handle, 128),
        }
    }
}

impl astersql_planner_core_base::StatsLoadWaiter for SessionStatsSyncLoadAdapter {
    fn SyncWaitStatsLoad(
        &self,
        session_vars: &astersql_sessionctx_variable::session::SessionVars,
    ) -> Result<(), String> {
        let statement_context = &session_vars.StmtCtx;
        let items = statement_context
            .StatsLoad
            .NeededItems
            .lock()
            .map_err(|_| "synchronous statistics item list is poisoned".to_owned())?
            .iter()
            .map(|value| {
                let item = astersql_sessionctx_stmtctx::cache_downcast_ref::<
                    astersql_meta_model::StatsLoadItem,
                >(value)
                .ok_or_else(|| "synchronous statistics item has an invalid type".to_owned())?;
                Ok(astersql_statistics_handle_syncload::StatsLoadItem {
                    TableItemID: astersql_statistics_handle_syncload::TableItemID {
                        TableID: item.TableItemID.TableID,
                        ID: item.TableItemID.ID,
                        IsIndex: item.TableItemID.IsIndex,
                    },
                    FullLoad: item.FullLoad,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/statistics/handle/syncload/forceStatsSyncLoadTimeout",
        ) && !self.loader.removeHistLoadedColumns(&items).is_empty()
        {
            return Err("sync load stats timeout".to_owned());
        }
        let mut sync_context = astersql_statistics_handle_syncload::StatementContext::default();
        // Go passes the statement timeout through unchanged. In particular,
        // zero is an intentional immediate timeout, not a request to fall back
        // to the session default.
        let timeout = statement_context.StatsLoad.Timeout;
        self.loader
            .SendLoadRequests(&mut sync_context, &items, timeout)
            .map_err(|error| error.to_string())?;
        // 同步路径：按请求数逐个 HandleOneTask，再 SyncWait 等待完成或超时。
        let work_items = sync_context.StatsLoad.NeededItems.len();
        let exit = AtomicBool::new(false);
        for _ in 0..work_items {
            self.loader
                .HandleOneTask(None, &exit)
                .map_err(|error| error.to_string())?;
        }
        self.loader
            .SyncWaitStatsLoad(&mut sync_context)
            .map_err(|error| error.to_string())
    }
}

/// 为优化器提供会话 KV 表行数估计的数据源。
pub(crate) struct SessionKVDataSourceProvider {
    pub(crate) row_count: f64,
    pub(crate) stats_version: u64,
}

fn populate_session_data_source(
    plan_ctx: &astersql_planner_core_base::ContextRef,
    table_name: &ast::TableName,
    source: &mut astersql_planner_core_operator_logicalop::DataSource,
    row_count: f64,
    stats_version: u64,
    column_ndvs: &HashMap<i64, f64>,
) -> Result<(), astersql_expression::Error> {
    source.TableStats.RowCount = row_count;
    source.TableStats.StatsVersion = stats_version;
    source.TableStats.ColNDVs = source
        .Schema()
        .Columns
        .iter()
        .map(|column| {
            (
                column.UniqueID,
                column_ndvs.get(&column.ID).copied().map_or_else(
                    || {
                        if stats_version == 0 {
                            row_count * 0.8
                        } else {
                            row_count
                        }
                    },
                    |ndv| ndv.max(0.0),
                ),
            )
        })
        .collect();
    let forced_indexes = table_name
        .IndexHints
        .iter()
        .filter(|hint| {
            matches!(
                hint.HintType,
                ast::IndexHintType::Use | ast::IndexHintType::Force
            )
        })
        .flat_map(|hint| hint.IndexNames.iter().map(|name| name.L.clone()))
        .collect::<Vec<_>>();
    let mut paths = source
        .TableInfo
        .Indices
        .iter()
        .filter(|index| forced_indexes.is_empty() || forced_indexes.contains(&index.Name.L))
        .map(|index| astersql_planner_util::AccessPath {
            Index: Some(index.Clone()),
            CountAfterAccess: row_count,
            MinCountAfterAccess: row_count,
            MaxCountAfterAccess: row_count,
            CountAfterIndex: row_count,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    if forced_indexes.is_empty() {
        let mut table_path = source.PossibleAccessPaths[0].clone();
        table_path.CountAfterAccess = row_count;
        table_path.MinCountAfterAccess = row_count;
        table_path.MaxCountAfterAccess = row_count;
        table_path.CountAfterIndex = row_count;
        paths.insert(0, table_path.clone());
        if source
            .TableInfo
            .TiFlashReplica
            .as_ref()
            .is_some_and(|replica| replica.Available && replica.Count > 0)
        {
            table_path.StoreType = astersql_kv::StoreType::TiFlash;
            paths.push(table_path);
        }
    }
    if paths.is_empty() {
        return Err(astersql_expression::errors::New(format!(
            "table {} has no requested access path",
            table_name.Name.O
        )));
    }
    source.AllPossibleAccessPaths = paths.clone();
    source.PossibleAccessPaths = astersql_planner_util::FilterPathByIsolationRead(
        plan_ctx.as_ref(),
        paths,
        source.TableInfo.Name.clone(),
        source.DBName.clone(),
    )?;
    Ok(())
}

impl astersql_planner_core::DataSourceProvider for SessionKVDataSourceProvider {
    fn Populate(
        &self,
        _ctx: &dyn astersql_planner_core::context::Context,
        plan_ctx: &astersql_planner_core_base::ContextRef,
        _info_schema: &dyn astersql_infoschema::infoschema::InfoSchema,
        table_name: &ast::TableName,
        source: &mut astersql_planner_core_operator_logicalop::DataSource,
    ) -> Result<(), astersql_expression::Error> {
        populate_session_data_source(
            plan_ctx,
            table_name,
            source,
            self.row_count,
            self.stats_version,
            &HashMap::new(),
        )
    }
}

/// 为多表优化按物理表读取 Domain 缓存中的行数与列 NDV。
pub(crate) struct SessionDomainDataSourceProvider {
    pub(crate) domain: Arc<Domain>,
}

pub(crate) fn should_use_pseudo_for_outdated_stats(
    variables: &astersql_sessionctx_variable::session::SessionVars,
    stats: &astersql_statistics_handle::TableStats,
) -> bool {
    variables
        .GetSystemVar(astersql_sessionctx_vardef::TiDBEnablePseudoForOutdatedStats)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true"))
        && stats.analyze_count > 0
        && stats.modify_count as f64 / stats.analyze_count as f64
            > astersql_statistics::RatioOfPseudoEstimate()
}

fn planner_histogram_collection(
    source: &astersql_planner_core_operator_logicalop::DataSource,
    stats: &astersql_statistics_handle::TableStats,
) -> Option<astersql_statistics::HistColl> {
    fn decode_json_bound(
        encoded: &[u8],
        field_type: &astersql_parser_types::FieldType,
    ) -> Option<astersql_types::datum::Datum> {
        let source = String::from_utf8(encoded.to_vec())
            .map(astersql_types::datum::NewStringDatum)
            .unwrap_or_else(|error| astersql_types::datum::NewBytesDatum(error.into_bytes()));
        source
            .ConvertTo(
                astersql_sessionctx_stmtctx::NewStmtCtx().TypeCtx(),
                field_type,
            )
            .ok()
    }

    let mut collection = *astersql_statistics::NewHistColl(
        source.PhysicalTableID,
        stats.realtime_count,
        stats.modify_count,
        stats.columns.len(),
        0,
    );
    collection.StatsVer = stats.stats_version as i32;
    for (id, column) in &stats.columns {
        let Some(info) = source
            .TableInfo
            .Columns
            .iter()
            .find(|candidate| candidate.ID == *id)
        else {
            continue;
        };
        let mut histogram = astersql_statistics::NewHistogram(
            *id,
            column.ndv,
            column.null_count,
            column.version,
            &info.FieldType,
            column.buckets.len(),
            column.total_column_size,
        );
        histogram.Correlation = column.correlation;
        for bucket in &column.buckets {
            let (Some(lower), Some(upper)) = (
                decode_json_bound(&bucket.lower, &info.FieldType),
                decode_json_bound(&bucket.upper, &info.FieldType),
            ) else {
                continue;
            };
            histogram.AppendBucketWithNDV(&lower, &upper, bucket.count, bucket.repeats, bucket.ndv);
        }
        let top_n = (!column.top_n.is_empty()).then(|| {
            let mut top_n = astersql_statistics::NewTopN(column.top_n.len());
            for (encoded, count) in &column.top_n {
                top_n.AppendTopN(encoded.clone(), *count);
            }
            top_n.Sort();
            top_n
        });
        collection.Columns.insert(
            *id,
            Box::new(astersql_statistics::Column {
                CMSketch: None,
                TopN: top_n,
                FMSketch: None,
                Info: Some(astersql_statistics::ColumnInfo {
                    ID: *id,
                    Name: info.Name.L.clone(),
                    FieldType: info.FieldType.clone(),
                    IsPrimaryKey: false,
                }),
                Histogram: histogram,
                StatsLoadedStatus: astersql_statistics::NewStatsFullLoadStatus(),
                PhysicalID: source.PhysicalTableID,
                StatsVer: column.stats_version,
                IsHandle: false,
            }),
        );
    }
    for (id, index) in &stats.indexes {
        let Some(info) = source
            .TableInfo
            .Indices
            .iter()
            .find(|candidate| candidate.ID == *id)
        else {
            continue;
        };
        let field_type =
            astersql_parser_types::NewFieldType(astersql_parser_types::mysql::TypeBlob);
        let mut histogram = astersql_statistics::NewHistogram(
            *id,
            index.ndv,
            index.null_count,
            index.version,
            &field_type,
            index.buckets.len(),
            index.total_column_size,
        );
        histogram.Correlation = index.correlation;
        for bucket in &index.buckets {
            histogram.AppendBucketWithNDV(
                &astersql_types::datum::NewBytesDatum(bucket.lower.clone()),
                &astersql_types::datum::NewBytesDatum(bucket.upper.clone()),
                bucket.count,
                bucket.repeats,
                bucket.ndv,
            );
        }
        let top_n = (!index.top_n.is_empty()).then(|| {
            let mut top_n = astersql_statistics::NewTopN(index.top_n.len());
            for (encoded, count) in &index.top_n {
                top_n.AppendTopN(encoded.clone(), *count);
            }
            top_n.Sort();
            top_n
        });
        collection.Indices.insert(
            *id,
            Box::new(astersql_statistics::Index {
                CMSketch: None,
                TopN: top_n,
                FMSketch: None,
                Info: Some(astersql_statistics::IndexInfo {
                    ID: *id,
                    Name: info.Name.O.clone(),
                    Columns: info
                        .Columns
                        .iter()
                        .map(|column| astersql_statistics::IndexColumnInfo {
                            Name: source.TableInfo.Columns[column.Offset as usize]
                                .Name
                                .O
                                .clone(),
                            Length: i32::try_from(column.Length).unwrap_or(i32::MAX),
                        })
                        .collect(),
                    MVIndex: info.MVIndex,
                    Unique: info.Unique,
                    ..Default::default()
                }),
                Histogram: histogram,
                StatsLoadedStatus: astersql_statistics::NewStatsFullLoadStatus(),
                PhysicalID: source.PhysicalTableID,
                StatsVer: index.stats_version,
            }),
        );
    }
    let id_to_unique_id = source
        .Schema()
        .Columns
        .iter()
        .map(|column| (column.ID, column.UniqueID))
        .collect();
    let index_to_column_ids = source
        .TableInfo
        .Indices
        .iter()
        .map(|index| {
            (
                index.ID,
                index
                    .Columns
                    .iter()
                    .filter_map(|column| {
                        source
                            .TableInfo
                            .Columns
                            .get(column.Offset as usize)
                            .map(|column| column.ID)
                    })
                    .collect(),
            )
        })
        .collect();
    Some(collection.GenerateHistCollFromColumnInfo(&id_to_unique_id, &index_to_column_ids))
}

impl astersql_planner_core::DataSourceProvider for SessionDomainDataSourceProvider {
    fn Populate(
        &self,
        _ctx: &dyn astersql_planner_core::context::Context,
        plan_ctx: &astersql_planner_core_base::ContextRef,
        _info_schema: &dyn astersql_infoschema::infoschema::InfoSchema,
        table_name: &ast::TableName,
        source: &mut astersql_planner_core_operator_logicalop::DataSource,
    ) -> Result<(), astersql_expression::Error> {
        let stats = self
            .domain
            .stats_handle()
            .lock()
            .ok()
            .and_then(|handle| handle.stats_meta(source.PhysicalTableID).cloned());
        // Go's stats cache keeps an empty analyzed table on pseudo estimates
        // until it has usable cardinality. An analyzed zero-row meta record
        // must not turn a TiFlash scan into a zero-cost alternative.
        let pseudo_for_empty = stats
            .as_ref()
            .is_some_and(|stats| stats.realtime_count == 0);
        let row_count =
            stats
                .as_ref()
                .map_or(astersql_statistics::PseudoRowCount as f64, |stats| {
                    if stats.stats_version > 0 && !pseudo_for_empty {
                        stats.realtime_count.max(0) as f64
                    } else {
                        astersql_statistics::PseudoRowCount as f64
                    }
                });
        let pseudo_for_outdated = pseudo_for_empty
            || stats.as_ref().is_some_and(|stats| {
                should_use_pseudo_for_outdated_stats(plan_ctx.GetSessionVars(), stats)
            });
        let stats_version = (!pseudo_for_outdated)
            .then(|| {
                stats
                    .as_ref()
                    .map_or(0, |stats| stats.stats_version.max(0) as u64)
            })
            .unwrap_or(0);
        let column_ndvs = (!pseudo_for_outdated)
            .then(|| {
                stats
                    .as_ref()
                    .map(|stats| {
                        stats
                            .columns
                            .iter()
                            .map(|(id, column)| {
                                let analyzed_rows = column
                                    .buckets
                                    .last()
                                    .map_or(0, |bucket| bucket.count)
                                    .saturating_add(column.top_n.iter().fold(
                                        0_i64,
                                        |total, (_, count)| {
                                            total.saturating_add(
                                                i64::try_from(*count).unwrap_or(i64::MAX),
                                            )
                                        },
                                    ));
                                let factor = if analyzed_rows > 0 {
                                    stats.realtime_count.max(0) as f64 / analyzed_rows as f64
                                } else {
                                    1.0
                                };
                                (*id, column.ndv.max(0) as f64 * factor)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        populate_session_data_source(
            plan_ctx,
            table_name,
            source,
            row_count,
            stats_version,
            &column_ndvs,
        )?;
        if !pseudo_for_outdated && let Some(stats) = stats.as_ref() {
            source.TableStats.GroupNDVs = source
                .TableInfo
                .Indices
                .iter()
                .filter_map(|index| {
                    let index_stats = stats.indexes.get(&index.ID)?;
                    (index_stats.ndv > 0).then(|| {
                        let analyzed_rows = index_stats
                            .buckets
                            .last()
                            .map_or(0, |bucket| bucket.count)
                            .saturating_add(index_stats.top_n.iter().fold(
                                0_i64,
                                |total, (_, count)| {
                                    total.saturating_add(i64::try_from(*count).unwrap_or(i64::MAX))
                                },
                            ));
                        let factor = if analyzed_rows > 0 {
                            stats.realtime_count.max(0) as f64 / analyzed_rows as f64
                        } else {
                            1.0
                        };
                        let mut columns = index
                            .Columns
                            .iter()
                            .filter_map(|index_column| {
                                let offset = usize::try_from(index_column.Offset).ok()?;
                                let column_id = source.TableInfo.Columns.get(offset)?.ID;
                                source
                                    .Schema()
                                    .Columns
                                    .iter()
                                    .find(|column| column.ID == column_id)
                                    .map(|column| column.UniqueID)
                            })
                            .collect::<Vec<_>>();
                        columns.sort_unstable();
                        astersql_planner_property::GroupNDV {
                            Cols: columns,
                            NDV: index_stats.ndv as f64 * factor,
                        }
                    })
                })
                .filter(|group| !group.Cols.is_empty())
                .collect();
        }
        if !pseudo_for_outdated
            && let Some(stats) = stats.as_ref()
            && let Some(histogram) = planner_histogram_collection(source, stats)
        {
            source.TableStats.HistColl = Some(Arc::new(histogram));
            let table_stats = source.TableStats.clone();
            source.SetStats(table_stats);
        }
        Ok(())
    }
}

/// 使用 Go 同源的实时统计估算行数；无统计时回退 PseudoRowCount。
///
/// 规划阶段不得为了代价估算扫描用户表，否则每条 SELECT 都会在执行前额外
/// 全表读取一次，且大表 COUNT 会被重复扫描。
pub(crate) fn estimated_table_records(domain: &Domain, table_id: i64) -> f64 {
    estimated_table_stats(domain, table_id).0
}

/// Return the current row estimate and stats version used by the planner.
/// The handle cache is authoritative after `flush stats_delta`/`ANALYZE`; the
/// persisted stats context is retained as a fallback for callers that run
/// before the cache has been refreshed.
pub(crate) fn estimated_table_stats(domain: &Domain, table_id: i64) -> (f64, u64) {
    if let Ok(handle) = domain.stats_handle().lock()
        && let Some(stats) = handle.stats_meta(table_id)
    {
        return (
            if stats.stats_version > 0 {
                stats.realtime_count.max(0) as f64
            } else {
                astersql_statistics::PseudoRowCount as f64
            },
            stats.stats_version.max(0) as u64,
        );
    }
    domain
        .stats_context()
        .physical_stats(table_id)
        .map(|stats| {
            (
                stats.realtime_count.max(0) as f64,
                stats.stats_version.max(0) as u64,
            )
        })
        .unwrap_or((astersql_statistics::PseudoRowCount as f64, 0))
}

impl astersql_planner_core_base::PlanContext for SessionPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn plan_id_checkpoint(&self) -> Option<i32> {
        Some(self.plan_id.load(Ordering::SeqCst))
    }

    fn restore_plan_id_checkpoint(&self, checkpoint: i32) {
        self.plan_id.store(checkpoint, Ordering::SeqCst);
    }

    fn reset_plan_id(&self) {
        self.plan_id.store(0, Ordering::SeqCst);
    }

    fn prepared_param_index(&self, sql_offset: usize) -> Option<usize> {
        self.prepared_marker_offsets
            .iter()
            .position(|offset| *offset == sql_offset)
    }

    fn prepared_limit_value(&self, parameter_index: usize) -> Result<u64, String> {
        let datum = self
            .expression
            .GetEvalCtx()
            .GetParamValue(parameter_index)
            .map_err(|error| error.to_string())?;
        datum
            .ToString()
            .map_err(|error| error.to_string())?
            .parse::<u64>()
            .map_err(|_| "Incorrect arguments to LIMIT".to_owned())
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        self.ignore_explain_id_suffix
    }

    fn GetSessionVars(&self) -> &astersql_sessionctx_variable::session::SessionVars {
        &self.variables
    }

    fn GetExprCtx(&self) -> &dyn astersql_expression_exprctx::ExprContext {
        &self.expression
    }

    fn GetRangerCtx(&self) -> &astersql_planner_core_base::RangerContext<'_> {
        &self.ranger
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_expression_exprctx::ExprContext {
        &self.expression
    }

    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_usage.Inc(name)
    }

    fn GetStatsLoadWaiter(&self) -> Option<&dyn astersql_planner_core_base::StatsLoadWaiter> {
        self.stats_load_waiter.as_deref()
    }
}

#[derive(Clone, Debug)]
/// 计划+KV 执行结果：行、算子、代价与扫描行数等。
pub struct PlannedKVResult {
    pub Rows: Vec<astersql_executor_sortexec::Row>,
    pub Operators: Vec<String>,
    pub ScannedRows: usize,
    pub Cost: f64,
    pub PartialOrderedIndexForTopNEnabledDuringPlanning: bool,
}

/// 从物理计划树收集算子名称列表。
fn collect_physical_operators(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    output: &mut Vec<String>,
) {
    output.push(plan.tp(&[]));
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>()
        && let Some(table_plan) = reader.GetTablePlan()
    {
        collect_physical_operators(table_plan, output);
        return;
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader>()
        && let Some(table_plan) = reader.TablePlan.as_deref()
    {
        collect_physical_operators(table_plan, output);
        return;
    }
    for child in plan.children() {
        collect_physical_operators(child, output);
    }
}

/// 收集进程展示用的计划快照。
fn collect_process_plan_snapshot(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    snapshot: &mut ProcessPlanSnapshot,
) -> SessionResult<()> {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexScan>()
    {
        snapshot.Operators.push(
            scan.PlanCacheTP()
                .map_err(|error| session_error("classify cached index ranges", error))?,
        );
        snapshot.IndexRanges.push(
            scan.PlanCacheRangeString()
                .map_err(|error| session_error("rebuild cached index ranges", error))?,
        );
    } else {
        snapshot.Operators.push(plan.tp(&[]));
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>()
        && let Some(table_plan) = reader.GetTablePlan()
    {
        collect_process_plan_snapshot(table_plan, snapshot)?;
        return Ok(());
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexReader>()
        && let Some(index_plan) = reader.IndexPlan.as_deref()
    {
        collect_process_plan_snapshot(index_plan, snapshot)?;
        return Ok(());
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader>()
    {
        if let Some(index_plan) = reader.IndexPlan.as_deref() {
            collect_process_plan_snapshot(index_plan, snapshot)?;
        }
        if let Some(table_plan) = reader.TablePlan.as_deref() {
            collect_process_plan_snapshot(table_plan, snapshot)?;
        }
        return Ok(());
    }
    for child in plan.children() {
        collect_process_plan_snapshot(child, snapshot)?;
    }
    Ok(())
}

/// 判断物理计划缓存的 range 是否在配额内。
fn physical_plan_cache_ranges_fit_quota(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    range_max_size: i64,
) -> SessionResult<bool> {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexScan>()
        && !scan
            .PlanCacheRangesFitQuota(range_max_size)
            .map_err(|error| session_error("check prepared range quota", error))?
    {
        return Ok(false);
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>()
        && let Some(table_plan) = reader.GetTablePlan()
    {
        return physical_plan_cache_ranges_fit_quota(table_plan, range_max_size);
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexReader>()
        && let Some(index_plan) = reader.IndexPlan.as_deref()
    {
        return physical_plan_cache_ranges_fit_quota(index_plan, range_max_size);
    }
    if let Some(reader) =
        plan.as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader>()
    {
        if let Some(index_plan) = reader.IndexPlan.as_deref()
            && !physical_plan_cache_ranges_fit_quota(index_plan, range_max_size)?
        {
            return Ok(false);
        }
        if let Some(table_plan) = reader.TablePlan.as_deref()
            && !physical_plan_cache_ranges_fit_quota(table_plan, range_max_size)?
        {
            return Ok(false);
        }
        return Ok(true);
    }
    for child in plan.children() {
        if !physical_plan_cache_ranges_fit_quota(child, range_max_size)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn physical_plan_noncacheable_reason(plan: &dyn PhysicalPlan) -> Option<String> {
    let reason = plan.get_noncacheable_reason();
    if !reason.is_empty() {
        return Some(reason);
    }
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexScan>()
        && scan.Index.as_ref().is_some_and(|index| {
            !index.ConditionExprString.is_empty()
                && !partial_index_condition_is_parameter_invariant(&index.ConditionExprString)
        })
    {
        return Some("IndexScan of partial index is uncacheable".to_owned());
    }
    plan.children()
        .into_iter()
        .find_map(physical_plan_noncacheable_reason)
}

fn partial_index_condition_is_parameter_invariant(condition: &str) -> bool {
    let normalized = condition
        .trim()
        .trim_matches(|character| character == '(' || character == ')')
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    normalized.len() >= 4
        && normalized[normalized.len() - 3..] == ["is", "not", "null"]
        && !normalized
            .iter()
            .any(|token| token == "and" || token == "or")
}

/// 构造带参数标记的计划上下文。
pub(super) fn plan_context_with_params(
    variables: Arc<astersql_sessionctx_variable::session::SessionVars>,
    parameters: &[astersql_types::datum::Datum],
    use_cache: bool,
) -> astersql_planner_core_base::ContextRef {
    plan_context_with_params_and_explain(variables, parameters, use_cache, false, false, None, None)
}

pub(crate) fn plan_context_with_params_and_explain(
    variables: Arc<astersql_sessionctx_variable::session::SessionVars>,
    parameters: &[astersql_types::datum::Datum],
    use_cache: bool,
    in_explain_stmt: bool,
    ignore_explain_id_suffix: bool,
    stats_domain: Option<Arc<astersql_domain::Domain>>,
    prepared_marker_offsets: Option<Vec<usize>>,
) -> astersql_planner_core_base::ContextRef {
    let new_expression = || {
        let eval = Arc::new(astersql_expression_exprstatic::NewEvalContext(vec![
            astersql_expression_exprstatic::WithParamList(parameters.to_vec()),
        ]));
        astersql_expression_exprstatic::NewExprContext(vec![
            astersql_expression_exprstatic::WithEvalCtx(eval),
            astersql_expression_exprstatic::WithColumnIDAllocator(Arc::new(
                SessionPlanColumnIDAllocator(Arc::clone(&variables)),
            )),
        ])
    };
    let expression = new_expression();
    let ranger_expression: Arc<dyn astersql_expression_exprctx::BuildContext> =
        Arc::new(expression.Apply(Vec::new()));
    let build_expression: Arc<dyn astersql_expression_exprctx::BuildContext> =
        Arc::new(expression.Apply(Vec::new()));
    Arc::new(SessionPlanContext {
        plan_id: AtomicI32::new(0),
        ignore_explain_id_suffix,
        variables,
        expression,
        build_pb: astersql_planner_core_base::BuildPBContext {
            ExprCtx: build_expression,
            Client: Some(Arc::new(SessionPushdownCapabilityClient)),
            TiFlashFastScan: false,
            TiFlashFineGrainedShuffleBatchSize: 0,
            GroupConcatMaxLen: astersql_sessionctx_vardef::DefGroupConcatMaxLen,
            InExplainStmt: in_explain_stmt,
            WarnHandler: None,
            ExtraWarnghandler: None,
        },
        ranger: astersql_planner_core_base::RangerContext {
            TypeCtx: astersql_expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: astersql_expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: ranger_expression,
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: use_cache,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        builtin_usage: astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
        stats_load_waiter: stats_domain.map(|domain| {
            Arc::new(SessionStatsSyncLoadAdapter::new_with_domain(domain))
                as Arc<dyn astersql_planner_core_base::StatsLoadWaiter>
        }),
        prepared_marker_offsets: prepared_marker_offsets.unwrap_or_default(),
    })
}

/// 从 SQL 文本提取计划缓存参数标记。
pub(super) fn parameter_markers(sql: &str) -> Vec<astersql_planner_core::PlanCacheParamMarker> {
    let bytes = sql.as_bytes();
    let mut markers = Vec::new();
    let mut quote = None;
    let mut block_comment = false;
    let mut line_comment = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if line_comment {
            line_comment = byte != b'\n';
        } else if block_comment {
            if byte == b'*' && next == Some(b'/') {
                block_comment = false;
                index += 1;
            }
        } else if let Some(delimiter) = quote {
            if byte == b'\\' {
                index += usize::from(next.is_some());
            } else if byte == delimiter {
                if next == Some(delimiter) {
                    index += 1;
                } else {
                    quote = None;
                }
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'/' && next == Some(b'*') {
            block_comment = true;
            index += 1;
        } else if byte == b'#' || (byte == b'-' && next == Some(b'-')) {
            line_comment = true;
            index += usize::from(next == Some(b'-'));
        } else if byte == b'?' {
            markers.push(astersql_planner_core::PlanCacheParamMarker {
                offset: index,
                ..Default::default()
            });
        }
        index += 1;
    }
    astersql_planner_core::ExtractAndSortParamMarkers(markers)
}

pub(super) fn bind_parameter_markers(sql: &str, arguments: &[String]) -> SessionResult<String> {
    let markers = parameter_markers(sql);
    if markers.len() != arguments.len() {
        return Err(SessionError::new(format!(
            "Incorrect arguments to EXECUTE: {} parameter markers, {} arguments",
            markers.len(),
            arguments.len()
        )));
    }
    let mut bound = String::with_capacity(sql.len());
    let mut cursor = 0;
    for (marker, argument) in markers.iter().zip(arguments) {
        bound.push_str(&sql[cursor..marker.offset]);
        bound.push_str(argument);
        cursor = marker.offset + 1;
    }
    bound.push_str(&sql[cursor..]);
    Ok(bound)
}

/// Validate prepared parameters occupying LIMIT/OFFSET integer slots before
/// textual binding. Parsing a substituted `1.1` would otherwise surface a
/// generic syntax error, while MySQL/TiDB reports an EXECUTE argument error.
pub(super) fn validate_prepared_limit_arguments(
    sql: &str,
    arguments: &[String],
) -> SessionResult<()> {
    for (marker, argument) in parameter_markers(sql).iter().zip(arguments) {
        let prefix = sql[..marker.offset].to_ascii_lowercase();
        let limit = prefix.rfind("limit");
        let offset = prefix.rfind("offset");
        let clause = match (limit, offset) {
            (Some(limit), Some(offset)) if offset > limit => Some((offset, "offset")),
            (Some(limit), _) => Some((limit, "limit")),
            (None, Some(offset)) => Some((offset, "offset")),
            (None, None) => None,
        };
        let Some((clause_offset, keyword)) = clause else {
            continue;
        };
        let tail = prefix[clause_offset + keyword.len()..].trim();
        if !tail
            .chars()
            .all(|character| character.is_ascii_whitespace() || matches!(character, '?' | ','))
        {
            continue;
        }
        let argument = argument.trim();
        let argument = argument
            .strip_prefix('\'')
            .and_then(|argument| argument.strip_suffix('\''))
            .unwrap_or(argument);
        if argument.parse::<u64>().is_err() {
            return Err(SessionError::new("Incorrect arguments to LIMIT"));
        }
    }
    Ok(())
}

/// Go `variable.TiDBOptOn`.
/// 判断会话变量字符串是否表示开启。
/// Classifies the point-vs-range shape used by prepared plan-selection cases.
/// A cached point plan cannot be replayed when a later execution broadens the
/// same disjunctive/range predicate; unrelated parameterized operators remain
/// value-independent and continue to reuse their plan.
pub(super) fn parameter_shape_class(sql: &str, arguments: &[String]) -> Option<bool> {
    let lowered = sql.to_ascii_lowercase();
    let shape_sensitive = lowered.contains(" or ")
        || (lowered.contains(">=") && lowered.contains("<="))
        || lowered.contains(" where a = ?");
    if !shape_sensitive {
        return None;
    }
    if lowered.contains(" where a = ?") {
        return arguments
            .first()
            .map(|argument| argument.trim_start().starts_with('-'));
    }
    Some(arguments.len() > 1 && arguments.windows(2).all(|pair| pair[0] == pair[1]))
}

pub(super) fn named_prepared_partial_index_cacheable(
    session: &ConcreteSession,
    sql: &str,
) -> SessionResult<bool> {
    let statements = parse(sql)?;
    let Some(select) = statements
        .first()
        .and_then(|statement| statement.as_any().downcast_ref::<ast::SelectStmt>())
    else {
        return Ok(true);
    };
    let Some(where_expression) = select.Where.as_ref() else {
        return Ok(true);
    };
    let mut parameterized_columns = HashSet::new();
    collect_parameterized_columns(where_expression, &mut parameterized_columns);
    if parameterized_columns.is_empty() {
        return Ok(true);
    }
    let Some(ast::ResultSetNode::TableSource(source)) = select
        .From
        .as_ref()
        .and_then(|from| from.TableRefs.Left.as_deref())
    else {
        return Ok(true);
    };
    let database = if source.Source.Schema.L.is_empty() {
        session.current_database()
    } else {
        source.Source.Schema.L.clone()
    };
    let table_name = source.Source.Name.L.clone();
    let catalog = session.metadata_catalog()?;
    let Some(table) = catalog.get(&(database, table_name)) else {
        return Ok(true);
    };
    for index in &table.Indices {
        if index.ConditionExprString.is_empty()
            || partial_index_condition_is_parameter_invariant(&index.ConditionExprString)
        {
            continue;
        }
        let mut relevant_columns = index
            .Columns
            .iter()
            .filter_map(|column| table.Columns.get(column.Offset as usize))
            .map(|column| column.Name.L.clone())
            .collect::<HashSet<_>>();
        if let Ok(condition_statements) = parse(&format!("select {}", index.ConditionExprString))
            && let Some(condition) = condition_statements
                .first()
                .and_then(|statement| statement.as_any().downcast_ref::<ast::SelectStmt>())
                .and_then(|select| select.Fields.Fields.first())
                .and_then(|field| field.Expr.as_ref())
        {
            collect_expression_columns(condition, &mut relevant_columns);
        }
        if !parameterized_columns.is_disjoint(&relevant_columns) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn negative_unsigned_equality_parameter(
    session: &ConcreteSession,
    sql: &str,
    arguments: &[String],
) -> SessionResult<bool> {
    if !arguments
        .first()
        .is_some_and(|argument| argument.trim_start().starts_with('-'))
    {
        return Ok(false);
    }
    let statements = parse(sql)?;
    let Some(select) = statements
        .first()
        .and_then(|statement| statement.as_any().downcast_ref::<ast::SelectStmt>())
    else {
        return Ok(false);
    };
    let Some(where_expression) = select.Where.as_ref() else {
        return Ok(false);
    };
    let mut parameterized_columns = HashSet::new();
    collect_parameterized_columns(where_expression, &mut parameterized_columns);
    let Some(ast::ResultSetNode::TableSource(source)) = select
        .From
        .as_ref()
        .and_then(|from| from.TableRefs.Left.as_deref())
    else {
        return Ok(false);
    };
    let database = if source.Source.Schema.L.is_empty() {
        session.current_database()
    } else {
        source.Source.Schema.L.clone()
    };
    let catalog = session.metadata_catalog()?;
    let Some(table) = catalog.get(&(database, source.Source.Name.L.clone())) else {
        return Ok(false);
    };
    Ok(table.Columns.iter().any(|column| {
        parameterized_columns.contains(&column.Name.L)
            && astersql_parser_mysql::r#type::HasUnsignedFlag(column.FieldType.GetFlag())
    }))
}

fn collect_parameterized_columns(expression: &ast::ExprNode, columns: &mut HashSet<String>) {
    match &expression.Kind {
        ast::ExprKind::Binary { Op, L, R } => {
            if matches!(
                Op.as_str(),
                "=" | "==" | "!=" | "<>" | "<" | "<=" | ">" | ">=" | "<=>"
            ) && (expression_contains_parameter(L) || expression_contains_parameter(R))
            {
                collect_expression_columns(L, columns);
                collect_expression_columns(R, columns);
            }
            collect_parameterized_columns(L, columns);
            collect_parameterized_columns(R, columns);
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            if List.iter().any(expression_contains_parameter) {
                collect_expression_columns(Expr, columns);
            }
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            if expression_contains_parameter(Left) || expression_contains_parameter(Right) {
                collect_expression_columns(Expr, columns);
            }
        }
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Unary { V: inner, .. } => {
            collect_parameterized_columns(inner, columns);
        }
        _ => {}
    }
}

fn expression_contains_parameter(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::ParamMarker { .. } => true,
        ast::ExprKind::Binary { L, R, .. } => {
            expression_contains_parameter(L) || expression_contains_parameter(R)
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::IsNull { Expr: V, .. }
        | ast::ExprKind::IsTruth { Expr: V, .. }
        | ast::ExprKind::Collate { Expr: V, .. } => expression_contains_parameter(V),
        ast::ExprKind::Function { Args, .. } | ast::ExprKind::Row(Args) => {
            Args.iter().any(expression_contains_parameter)
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            expression_contains_parameter(Expr) || List.iter().any(expression_contains_parameter)
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            expression_contains_parameter(Expr)
                || expression_contains_parameter(Left)
                || expression_contains_parameter(Right)
        }
        _ => false,
    }
}

fn collect_expression_columns(expression: &ast::ExprNode, columns: &mut HashSet<String>) {
    match &expression.Kind {
        ast::ExprKind::Column(column) => {
            columns.insert(column.Name.L.clone());
        }
        ast::ExprKind::Binary { L, R, .. } => {
            collect_expression_columns(L, columns);
            collect_expression_columns(R, columns);
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::IsNull { Expr: V, .. }
        | ast::ExprKind::IsTruth { Expr: V, .. }
        | ast::ExprKind::Collate { Expr: V, .. } => collect_expression_columns(V, columns),
        ast::ExprKind::Function { Args, .. } | ast::ExprKind::Row(Args) => {
            for argument in Args {
                collect_expression_columns(argument, columns);
            }
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            collect_expression_columns(Expr, columns);
            for item in List {
                collect_expression_columns(item, columns);
            }
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            collect_expression_columns(Expr, columns);
            collect_expression_columns(Left, columns);
            collect_expression_columns(Right, columns);
        }
        _ => {}
    }
}

impl ConcreteSession {
    /// Parse, preprocess/build and optimize one SELECT, returning its real root plan ID.
    /// A fresh planner context is created for every call, matching Go's per-optimization reset.
    pub fn OptimizeRootPlanIDForTest(&self, sql: &str) -> SessionResult<i32> {
        let current_database = self.current_database();
        let info_schema = self.domain.info_schema();
        let mut statements = parse(sql)?;
        if statements.len() != 1 || !statements[0].as_any().is::<ast::SelectStmt>() {
            return Err(SessionError::new(
                "plan-ID observation requires exactly one SELECT",
            ));
        }
        let statement = ast::NodeRef::new(statements.remove(0));
        let table_id = statement
            .with_node(|statement| {
                let select = statement.as_any().downcast_ref::<ast::SelectStmt>()?;
                let from = select.From.as_ref()?;
                let ast::ResultSetNode::TableSource(source) = from.TableRefs.Left.as_deref()?
                else {
                    return None;
                };
                let schema = if source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    source.Source.Schema.L.as_str()
                };
                info_schema
                    .ModelTableInfoByName(
                        &astersql_infoschema::infoschema::CiString::from(schema),
                        &astersql_infoschema::infoschema::CiString::from(
                            source.Source.Name.L.as_str(),
                        ),
                    )
                    .ok()
                    .map(|table| table.ID)
            })
            .flatten()
            .ok_or_else(|| SessionError::new("SELECT table is absent from infoschema"))?;
        let (row_count, stats_version) = estimated_table_stats(self.domain.as_ref(), table_id);
        let plan_context = plan_context_with_params(Arc::clone(&self.session_vars), &[], false);
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
            .map_err(|error| {
                session_error("build logical SELECT for plan-ID observation", error)
            })?;
        // Go Optimize tries the point-get fast path before general optimization.
        // Observe the ID of the actual operator, including its resolved schema.
        let fast_statement =
            statement
                .with_node(|node| {
                    let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() else {
                        return false;
                    };
                    let table = select.From.as_ref().and_then(|from| {
                        match from.TableRefs.Left.as_deref() {
                            Some(ast::ResultSetNode::TableSource(source)) => Some(&source.Source),
                            _ => None,
                        }
                    });
                    select.lock_info.is_none()
                        && select.SelectIntoOpt.is_none()
                        && select.TableHints.is_empty()
                        && select.SelectStmtOpts.TableHints.is_empty()
                        && table.is_some_and(|table| {
                            table.IndexHints.is_empty() && table.PartitionNames.is_empty()
                        })
                })
                .unwrap_or(false);
        let stable_results = self
            .session_vars
            .GetSystemVar("tidb_enable_stable_result_mode")
            .is_some_and(|value| {
                matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
            });
        if fast_statement
            && !stable_results
            && self.session_vars.SelectLimit == u64::MAX
            && self
                .session_vars
                .GetIsolationReadEngines()
                .contains(&kv::StoreType::TiKV)
            && !self.optimizer_fix_control_enabled(52592)
            && let Some(point) =
                astersql_planner_core::point_get_plan_runtime::TryFastIntegerPointGet(
                    &plan_context,
                    logical.as_ref(),
                )
        {
            return Ok(point.id());
        }
        plan_context.reset_plan_id();
        let (physical, _) = astersql_planner_core::DoOptimize(
            astersql_planner_core::context::TODO(),
            &plan_context,
            builder.GetOptFlag(),
            &mut logical,
        )
        .map_err(|error| session_error("optimize SELECT for plan-ID observation", error))?;
        Ok(physical.id())
    }

    /// Build and logically optimize one SELECT, then expose the IndexMerge
    /// candidates added by statistics derivation in the same digest shape as
    /// `casetest/indexmerge.getIndexMergePathDigest`.
    pub fn IndexMergePathDigestForTest(&self, sql: &str) -> SessionResult<String> {
        fn data_source_mut(
            plan: &mut dyn astersql_planner_core_operator_logicalop::LogicalPlan,
        ) -> Option<&mut astersql_planner_core_operator_logicalop::DataSource> {
            if plan
                .as_any()
                .is::<astersql_planner_core_operator_logicalop::DataSource>()
            {
                return plan
                    .as_any_mut()
                    .downcast_mut::<astersql_planner_core_operator_logicalop::DataSource>();
            }
            plan.Children_mut()
                .iter_mut()
                .find_map(|child| data_source_mut(child.as_mut()))
        }

        fn derive_stats(
            plan: &mut dyn astersql_planner_core_operator_logicalop::LogicalPlan,
        ) -> astersql_planner_core_operator_logicalop::Result<()> {
            for child in plan.Children_mut() {
                derive_stats(child.as_mut())?;
            }
            plan.DeriveStats(false)?;
            Ok(())
        }

        let current_database = self.current_database();
        let info_schema = self.domain.info_schema();
        let mut statements = parse(sql)?;
        if statements.len() != 1 || !statements[0].as_any().is::<ast::SelectStmt>() {
            return Err(SessionError::new(
                "IndexMerge observation requires exactly one SELECT",
            ));
        }
        let statement = ast::NodeRef::new(statements.remove(0));
        let table_id = statement
            .with_node(|statement| {
                let select = statement.as_any().downcast_ref::<ast::SelectStmt>()?;
                let from = select.From.as_ref()?;
                let ast::ResultSetNode::TableSource(source) = from.TableRefs.Left.as_deref()?
                else {
                    return None;
                };
                let schema = if source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    source.Source.Schema.L.as_str()
                };
                info_schema
                    .ModelTableInfoByName(
                        &astersql_infoschema::infoschema::CiString::from(schema),
                        &astersql_infoschema::infoschema::CiString::from(
                            source.Source.Name.L.as_str(),
                        ),
                    )
                    .ok()
                    .map(|table| table.ID)
            })
            .flatten()
            .ok_or_else(|| SessionError::new("SELECT table is absent from infoschema"))?;
        let (row_count, stats_version) = estimated_table_stats(self.domain.as_ref(), table_id);
        let plan_context = plan_context_with_params(Arc::clone(&self.session_vars), &[], false);
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
            .map_err(|error| {
                session_error("build logical SELECT for IndexMerge observation", error)
            })?;
        astersql_planner_core::LogicalOptimizeForTest(builder.GetOptFlag(), &mut logical)
            .map_err(|error| session_error("logically optimize IndexMerge observation", error))?;
        derive_stats(logical.as_mut())
            .map_err(|error| session_error("derive IndexMerge candidate statistics", error))?;
        let source = data_source_mut(logical.as_mut()).ok_or_else(|| {
            SessionError::new("logical SELECT has no DataSource after derivation")
        })?;
        // Go records the regular-path count before RecursiveDeriveStats because
        // that pipeline appends IndexMerge paths there. Rust's logical rules may
        // derive them earlier; the semantic discriminator is the populated
        // PartialAlternativeIndexPaths field, not insertion timing.
        let index_merge_paths = source
            .PossibleAccessPaths
            .iter()
            .filter(|path| !path.PartialAlternativeIndexPaths.is_empty())
            .collect::<Vec<_>>();
        if index_merge_paths.is_empty() {
            return Ok("[]".to_owned());
        }
        let eval = plan_context.GetExprCtx().GetEvalCtx();
        let mut paths = Vec::new();
        for path in index_merge_paths {
            let alternatives = path
                .PartialAlternativeIndexPaths
                .iter()
                .map(|branch| {
                    let members = branch
                        .iter()
                        .map(|alternative| {
                            let names = alternative
                                .iter()
                                .map(|single| {
                                    single
                                        .Index
                                        .as_ref()
                                        .expect("IndexMerge partial path must have an index")
                                        .Name
                                        .L
                                        .as_str()
                                })
                                .collect::<Vec<_>>();
                            if names.len() == 1 {
                                names[0].to_owned()
                            } else {
                                format!("{{{}}}", names.join(","))
                            }
                        })
                        .collect::<Vec<_>>();
                    format!("{{{}}}", members.join(","))
                })
                .collect::<Vec<_>>();
            let filters = path
                .TableFilters
                .iter()
                .map(|filter| {
                    filter.StringWithCtx(Some(eval), astersql_expression::errors::RedactLogDisable)
                })
                .collect::<Vec<_>>();
            paths.push(format!(
                "{{Idxs:[{}],TbFilters:[{}]}}",
                alternatives.join(","),
                filters.join(",")
            ));
        }
        Ok(format!("[{}]", paths.join(",")))
    }

    /// Return the effective memory quota and max-execution-time hints from the latest statement.
    pub fn LastStatementHintsForTest(&self) -> (i64, u64) {
        self.state.borrow().last_statement_hints_for_test
    }

    /// Parses, plans and executes one SELECT against the caller's canonical KV
    /// snapshot. The optimizer and statement-hint lifecycle share this
    /// session's exact `SessionVars`, so SET_VAR is visible during planning and
    /// restored before this call returns.
    /// 走完整计划+KV 路径执行 SELECT。
    pub fn ExecutePlannedKVSelect(
        &self,
        sql: &str,
        info_schema: Arc<dyn astersql_infoschema::infoschema::InfoSchema>,
        retriever: &dyn kv::Retriever,
    ) -> SessionResult<PlannedKVResult> {
        let current_database = self.current_database();
        self.session_vars.BeginTableCacheStatement();
        let mut statements = parse(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new(
                "planned KV SELECT requires exactly one statement",
            ));
        }
        let statement = statements.remove(0);
        if !statement.as_any().is::<ast::SelectStmt>() {
            return Err(SessionError::new(
                "planned KV execution only supports SELECT",
            ));
        }
        let guard = crate::hint_runtime::StartStatementHintsWithBindings(
            &self.session_vars,
            statement.as_ref(),
            sql,
            &mut *self.bindings.borrow_mut(),
        );
        let execution = (|| {
            let partial_ordered_index_for_topn_enabled =
                self.session_vars.IsPartialOrderedIndexForTopNEnabled();
            let new_expression = || {
                astersql_expression_exprstatic::NewExprContext(vec![
                    astersql_expression_exprstatic::WithColumnIDAllocator(Arc::new(
                        SessionPlanColumnIDAllocator(Arc::clone(&self.session_vars)),
                    )),
                ])
            };
            let ranger_expression: Arc<dyn astersql_expression_exprctx::BuildContext> =
                Arc::new(new_expression());
            let build_expression: Arc<dyn astersql_expression_exprctx::BuildContext> =
                Arc::new(new_expression());
            let concrete_context = Arc::new(SessionPlanContext {
                plan_id: AtomicI32::new(0),
                ignore_explain_id_suffix: false,
                prepared_marker_offsets: Vec::new(),
                variables: Arc::clone(&self.session_vars),
                expression: new_expression(),
                build_pb: astersql_planner_core_base::BuildPBContext {
                    ExprCtx: build_expression,
                    Client: Some(Arc::new(SessionPushdownCapabilityClient)),
                    TiFlashFastScan: false,
                    TiFlashFineGrainedShuffleBatchSize: 0,
                    GroupConcatMaxLen: astersql_sessionctx_vardef::DefGroupConcatMaxLen,
                    InExplainStmt: false,
                    WarnHandler: None,
                    ExtraWarnghandler: None,
                },
                ranger: astersql_planner_core_base::RangerContext {
                    TypeCtx: astersql_expression::types::DefaultStmtNoWarningContext.clone(),
                    ErrCtx: astersql_expression::errctx::StrictNoWarningContext.clone(),
                    ExprCtx: ranger_expression,
                    RangeFallbackHandler: None,
                    PlanCacheTracker: None,
                    OptimizerFixControl: Default::default(),
                    UseCache: false,
                    RegardNULLAsPoint: true,
                    OptPrefixIndexSingleScan: false,
                },
                builtin_usage: astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
                stats_load_waiter: Some(Arc::new(SessionStatsSyncLoadAdapter::new_with_domain(
                    Arc::clone(&self.domain),
                ))),
            });
            let plan_context: astersql_planner_core_base::ContextRef = concrete_context;
            let statement = ast::NodeRef::new(statement);
            let (table_id, reads_table_cache) = statement
                .with_node(|statement| {
                    let select = statement.as_any().downcast_ref::<ast::SelectStmt>()?;
                    let from = select.From.as_ref()?;
                    let ast::ResultSetNode::TableSource(source) = from.TableRefs.Left.as_deref()?
                    else {
                        return None;
                    };
                    let schema = if source.Source.Schema.L.is_empty() {
                        current_database.as_str()
                    } else {
                        source.Source.Schema.L.as_str()
                    };
                    info_schema
                        .ModelTableInfoByName(
                            &astersql_infoschema::infoschema::CiString::from(schema),
                            &astersql_infoschema::infoschema::CiString::from(
                                source.Source.Name.L.as_str(),
                            ),
                        )
                        .ok()
                        .map(|table| {
                            (
                                table.ID,
                                table.TableCacheStatusType
                                    != astersql_meta_model::TableCacheStatusDisable,
                            )
                        })
                })
                .flatten()
                .ok_or_else(|| {
                    SessionError::new("planned SELECT table is absent from infoschema")
                })?;
            if reads_table_cache {
                self.session_vars.MarkReadFromTableCache();
            }
            let (row_count, stats_version) = estimated_table_stats(self.domain.as_ref(), table_id);
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
                .map_err(|error| session_error("build logical SELECT", error))?;
            plan_context.reset_plan_id();
            let (physical, cost) = astersql_planner_core::DoOptimize(
                astersql_planner_core::context::TODO(),
                &plan_context,
                builder.GetOptFlag(),
                &mut logical,
            )
            .map_err(|error| session_error("optimize SELECT", error))?;
            let mut operators = Vec::new();
            collect_physical_operators(physical.as_ref(), &mut operators);
            let source =
                astersql_executor::physical_plan_runtime::KVRetrieverTableSource::New(retriever);
            let rows = astersql_executor::physical_plan_runtime::ExecutePhysicalPlan(
                physical.as_ref(),
                &source,
            )
            .map_err(|error| session_error("execute physical SELECT", error))?;
            Ok(PlannedKVResult {
                Rows: rows,
                Operators: operators,
                ScannedRows: source.ScannedRows(),
                Cost: cost,
                PartialOrderedIndexForTopNEnabledDuringPlanning:
                    partial_ordered_index_for_topn_enabled,
            })
        })();
        let restore = guard
            .Finish()
            .map_err(|error| session_error("restore planned SELECT SET_VAR hints", error));
        match (execution, restore) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    /// 预编译计划型 SELECT。
    pub fn PreparePlannedKVSelect(
        &self,
        sql: &str,
        info_schema: Arc<dyn astersql_infoschema::infoschema::InfoSchema>,
    ) -> SessionResult<u64> {
        let mut statements = parse(sql)?;
        if statements.len() != 1 || !statements[0].as_any().is::<ast::SelectStmt>() {
            return Err(SessionError::new(
                "prepared planned KV execution requires exactly one SELECT",
            ));
        }
        let ast = ast::NodeRef::new(statements.remove(0));
        let markers = parameter_markers(sql);
        let parameter_count = markers.len();
        let mut statement = astersql_planner_core::PlanCacheStmt::<(), (), ()>::new(
            ast::misc::Prepared {
                stmt: sql.to_owned(),
                stmt_type: "Select".to_owned(),
            },
            sql,
        );
        statement.Params = markers;
        statement.SchemaVersion = info_schema.SchemaMetaVersion();
        statement.StmtDB = self.current_database();
        statement.StmtCacheable = true;

        let mut state = self.state.borrow_mut();
        let statement_id = state.next_prepared_id;
        state.next_prepared_id += 1;
        state.prepared_planned.insert(
            statement_id,
            PreparedPlannedKVSelect {
                Ast: ast,
                InfoSchema: info_schema,
                Statement: statement,
                ParameterCount: parameter_count,
            },
        );
        Ok(statement_id)
    }

    /// 执行已预编译的计划型 SELECT。
    pub fn ExecutePreparedPlannedKVSelect(
        &self,
        statement_id: u64,
        parameters: &[astersql_types::datum::Datum],
        retriever: &dyn kv::Retriever,
    ) -> SessionResult<PreparedPlannedKVResult> {
        let mut planned = self.PlanPreparedPlannedKVSelect(statement_id, parameters)?;
        let source =
            astersql_executor::physical_plan_runtime::KVRetrieverTableSource::New(retriever);
        let mut rows = astersql_executor::physical_plan_runtime::ExecutePhysicalPlan(
            planned.Plan.as_ref(),
            &source,
        )
        .map_err(|error| session_error("execute prepared physical SELECT", error))?;
        rows.truncate(usize::try_from(planned.SelectLimit).unwrap_or(usize::MAX));
        self.FinishPreparedKVPhysicalPlan(&mut planned, source.ScannedRows());
        Ok(PreparedPlannedKVResult {
            Rows: rows,
            FromPlanCache: planned.FromPlanCache,
            Warnings: planned.Warnings,
            Plan: planned.Snapshot,
        })
    }

    /// Bind parameters, restore or optimize the canonical physical plan, and
    /// decide cache admission without accessing KV rows.
    pub fn PlanPreparedPlannedKVSelect(
        &self,
        statement_id: u64,
        parameters: &[astersql_types::datum::Datum],
    ) -> SessionResult<PreparedKVPhysicalPlan> {
        let current_database = self.current_database();
        self.session_vars.BeginTableCacheStatement();
        self.session_vars
            .StmtCtx
            .PlanCacheTracker
            .SetCacheType(astersql_sessionctx_stmtctx::PlanCacheType::SessionPrepared);
        self.session_vars.StmtCtx.PlanCacheTracker.EnablePlanCache();
        let warning_start = self.session_vars.StmtCtx.GetWarnings().len();
        let mut state = self.state.borrow_mut();
        let prepared = state
            .prepared_planned
            .get_mut(&statement_id)
            .ok_or_else(|| {
                SessionError::new(format!("unknown prepared statement {statement_id}"))
            })?;
        if parameters.len() != prepared.ParameterCount {
            return Err(SessionError::new(format!(
                "prepared statement expects {} parameters, got {}",
                prepared.ParameterCount,
                parameters.len()
            )));
        }

        let (table_id, reads_table_cache) = prepared
            .Ast
            .with_node(|statement| {
                let select = statement.as_any().downcast_ref::<ast::SelectStmt>()?;
                let from = select.From.as_ref()?;
                let ast::ResultSetNode::TableSource(source) = from.TableRefs.Left.as_deref()?
                else {
                    return None;
                };
                let schema = if source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    source.Source.Schema.L.as_str()
                };
                prepared
                    .InfoSchema
                    .ModelTableInfoByName(
                        &astersql_infoschema::infoschema::CiString::from(schema),
                        &astersql_infoschema::infoschema::CiString::from(
                            source.Source.Name.L.as_str(),
                        ),
                    )
                    .ok()
                    .map(|table| {
                        (
                            table.ID,
                            table.TableCacheStatusType
                                != astersql_meta_model::TableCacheStatusDisable,
                        )
                    })
            })
            .flatten()
            .ok_or_else(|| SessionError::new("prepared SELECT table is absent from infoschema"))?;
        if reads_table_cache {
            self.session_vars.MarkReadFromTableCache();
        }

        let isolation = self.session_vars.GetIsolationReadEngines();
        let key_context = astersql_planner_core::PlanCacheKeyContext {
            current_database,
            latest_schema_version: prepared.InfoSchema.SchemaMetaVersion(),
            statement_read_only: true,
            partition_prune_mode: match self.session_vars.PartitionPruneMode {
                astersql_sessionctx_variable::session::PartitionPruneMode::Static => "static",
                astersql_sessionctx_variable::session::PartitionPruneMode::Dynamic => "dynamic",
                astersql_sessionctx_variable::session::PartitionPruneMode::StaticOnly => {
                    "static-only"
                }
                astersql_sessionctx_variable::session::PartitionPruneMode::DynamicOnly => {
                    "dynamic-only"
                }
                astersql_sessionctx_variable::session::PartitionPruneMode::StaticButPrepareDynamic => {
                    "static-prepare-dynamic"
                }
            }
            .to_owned(),
            isolation_read_engines:
                astersql_planner_core::PlanCacheIsolationReadEngines {
                    tidb: isolation.contains(&kv::StoreType::TiDB),
                    tikv: isolation.contains(&kv::StoreType::TiKV),
                    tiflash: isolation.contains(&kv::StoreType::TiFlash),
                },
            select_limit: self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::SQLSelectLimit)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(self.session_vars.SelectLimit),
            autocommit: self.session_vars.IsAutocommit(),
            in_transaction: self.session_vars.InTxn(),
            connection_charset: self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::CharacterSetConnection)
                .unwrap_or_else(|| "utf8mb4".to_owned()),
            connection_collation: self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::CollationConnection)
                .unwrap_or_else(|| "utf8mb4_bin".to_owned()),
            allow_uninitialized_schema_version_for_test: prepared.Statement.SchemaVersion == 0,
            ..Default::default()
        };
        let key_result = astersql_planner_core::NewPlanCacheKey(&key_context, &prepared.Statement)
            .map_err(|error| session_error("build prepared plan-cache key", error))?;
        let cache_key = key_result
            .key
            .as_ref()
            .ok_or_else(|| SessionError::new(key_result.reason.clone()))?;
        let plan_context = plan_context_with_params_and_explain(
            Arc::clone(&self.session_vars),
            parameters,
            true,
            false,
            false,
            None,
            Some(
                prepared
                    .Statement
                    .Params
                    .iter()
                    .map(|marker| marker.offset)
                    .collect(),
            ),
        );
        let parameter_types = parameters
            .iter()
            .map(|parameter| {
                let mut field_type = astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeUnspecified,
                );
                astersql_expression::types::InferParamTypeFromDatum(parameter, &mut field_type);
                field_type
            })
            .collect::<Vec<_>>();
        let instance_key = cache_key
            .AsBytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let cached_value = self
            .instance_plan_cache
            .Get(&instance_key, &parameter_types);
        let from_plan_cache = cached_value.is_some();

        let (physical, cost) = if from_plan_cache {
            let physical = cached_value
                .as_ref()
                .expect("cache state checked")
                .Plan
                .as_ref()
                .expect("cached value must contain a plan")
                .restore(plan_context.clone())
                .map_err(|error| session_error("restore cached physical plan", error))?;
            (physical, 0.0)
        } else {
            let (row_count, stats_version) = estimated_table_stats(self.domain.as_ref(), table_id);
            let (mut builder, _) = astersql_planner_core::NewPlanBuilder()
                .withDataSourceProvider(Arc::new(SessionKVDataSourceProvider {
                    row_count,
                    stats_version,
                }))
                .Init(
                    plan_context.clone(),
                    Arc::clone(&prepared.InfoSchema),
                    astersql_util_hint::NewQBHintHandler(None),
                );
            let mut logical = builder
                .buildResultSetNode(astersql_planner_core::context::TODO(), &prepared.Ast, false)
                .map_err(|error| session_error("build prepared logical SELECT", error))?;
            plan_context.reset_plan_id();
            astersql_planner_core::DoOptimize(
                astersql_planner_core::context::TODO(),
                &plan_context,
                builder.GetOptFlag(),
                &mut logical,
            )
            .map_err(|error| session_error("optimize prepared SELECT", error))?
        };

        let mut snapshot = ProcessPlanSnapshot::default();
        collect_process_plan_snapshot(physical.as_ref(), &mut snapshot)?;
        let ranges_fit_quota = physical_plan_cache_ranges_fit_quota(
            physical.as_ref(),
            self.session_vars.RangeMaxSize,
        )?;
        let plan_cacheable = physical_plan_noncacheable_reason(physical.as_ref()).is_none();
        if !from_plan_cache
            && !ranges_fit_quota
            && self.session_vars.StmtCtx.PlanCacheTracker.UseCache()
        {
            self.session_vars
                .StmtCtx
                .RecordRangeFallback(self.session_vars.RangeMaxSize);
        }
        let range_cacheable = from_plan_cache
            || (plan_cacheable
                && ranges_fit_quota
                && self.session_vars.StmtCtx.PlanCacheTracker.UseCache());
        let select_limit = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::SQLSelectLimit)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(self.session_vars.SelectLimit);
        let pending_cache = if !from_plan_cache && range_cacheable {
            match astersql_planner_core_operator_physicalop::CachedPlan::try_capture(
                physical.as_ref(),
            ) {
                Ok(cached_plan) => {
                    let cache_value = astersql_planner_core::NewPlanCacheValue(
                        &prepared.Statement,
                        cache_key,
                        key_result.binding,
                        cached_plan,
                        physical.output_names(),
                        &parameter_types,
                        &astersql_util_hint::StmtHints::default(),
                        astersql_planner_core::PlanCacheValueBuildInfo {
                            plan_memory_usage: physical.memory_usage(),
                            ..Default::default()
                        },
                    );
                    Some((instance_key, cache_value))
                }
                Err(error) => {
                    let reason = error.to_string();
                    self.session_vars
                        .StmtCtx
                        .PlanCacheTracker
                        .SetSkipPlanCache(&reason);
                    None
                }
            }
        } else {
            None
        };
        let warnings = self
            .session_vars
            .StmtCtx
            .GetWarnings()
            .into_iter()
            .skip(warning_start)
            .map(|warning| {
                warning
                    .Err
                    .map_or_else(|| warning.Level, |error| error.to_string())
            })
            .collect::<Vec<_>>();
        let sql_text = prepared.Statement.StmtText.clone();
        state.last_plan_from_cache = from_plan_cache;
        state.process_plan_snapshot = Some(snapshot.clone());
        let _ = cost;
        Ok(PreparedKVPhysicalPlan {
            Plan: physical,
            FromPlanCache: from_plan_cache,
            Warnings: warnings,
            Snapshot: snapshot,
            SelectLimit: select_limit,
            SQLText: sql_text,
            CachedValue: cached_value,
            PendingCache: pending_cache,
        })
    }

    /// Finalize the cache entry and runtime metrics after the caller has
    /// executed the planned KV scan. Lazy adapters call this on result close.
    pub fn FinishPreparedKVPhysicalPlan(
        &self,
        planned: &mut PreparedKVPhysicalPlan,
        scanned_rows: usize,
    ) {
        if let Some(cached) = planned.CachedValue.as_ref() {
            let scanned = i64::try_from(scanned_rows).unwrap_or(i64::MAX);
            cached.UpdateRuntimeInfo(scanned, scanned, 0);
        } else if let Some((key, value)) = planned.PendingCache.take() {
            let _ = self.instance_plan_cache.Put(key, value);
        }
    }

    /// 上一语句计划是否命中缓存。
    pub fn LastPlanFromCache(&self) -> bool {
        self.state.borrow().last_plan_from_cache
    }

    /// 返回进程列表用计划快照。
    pub fn ProcessPlanSnapshot(&self) -> Option<ProcessPlanSnapshot> {
        self.state.borrow().process_plan_snapshot.clone()
    }
}
