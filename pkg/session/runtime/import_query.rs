// Copyright 2026 AsterSQL.

//! IMPORT FROM SELECT uses the canonical query chunk encoder and local SST
//! backend, just like TableImporter.ImportSelectedRows. SQL supplies rows from
//! one real MVCC snapshot; no SQL INSERT or REPLACE writes the target.
use super::*;
use astersql_executor_importer as importer;
use astersql_lightning_backend as backend;
use astersql_lightning_backend_encode as encode;
use astersql_lightning_verification as verification;
use std::sync::mpsc;

struct QueryRuntime {
    table: astersql_meta_model::TableInfo,
    flags: astersql_types::Flags,
    mode: u64,
    allocators: astersql_lightning_backend_kv::Allocators,
    chunks: importer::SharedQueryChunkReceiver,
}
impl importer::TableImporterRuntime for QueryRuntime {
    fn DataSourceType(&self) -> importer::DataSourceType {
        importer::DataSourceTypeQuery
    }
    fn TableInfo(&self) -> &astersql_meta_model::TableInfo {
        &self.table
    }
    fn GetKeySpace(&self) -> Vec<u8> {
        Vec::new()
    }
    fn GetKVEncoder(
        &self,
        chunk: &dyn importer::ImportChunk,
    ) -> Result<importer::TableKVEncoder, String> {
        let mut definition = importer::NewTableDefinitionFromMeta(&self.table)?;
        definition.allocators = self.allocators.clone();
        importer::NewTableKVEncoderFromMeta(
            &encode::EncodingConfig {
                Table: Some(Arc::new(definition)),
                SessionOptions: encode::SessionOptions {
                    SQLMode: self.mode,
                    Timestamp: chunk.Timestamp(),
                    ..Default::default()
                },
                ..Default::default()
            },
            &self.table,
            Arc::new(importer::CanonicalImportDatumConverter(self.flags)),
        )
    }
    fn GetParser(
        &self,
        _: &encode::Context,
        _: &dyn importer::ImportChunk,
    ) -> Result<Box<dyn astersql_lightning_mydump::Parser + Send>, String> {
        Err("query import has no file parser".into())
    }
    fn TakeQueryChunks(&self) -> Result<importer::SharedQueryChunkReceiver, String> {
        Ok(self.chunks.clone())
    }
}
fn error(e: impl std::fmt::Display) -> SessionError {
    SessionError::new(e.to_string())
}
impl ConcreteSession {
    pub(super) fn execute_import_query(
        &self,
        statement: &ast::ImportIntoStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let source = statement
            .Select
            .as_ref()
            .ok_or_else(|| error("query import requires SELECT"))?;
        if !statement.ColumnsAndUserVars.is_empty() || !statement.ColumnAssignments.is_empty() {
            return Err(error("query import column mapping is not configured"));
        }
        let db = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.O.clone()
        };
        let table = self
            .resolve_runtime_table(&db, &statement.Table.Name.L)
            .ok_or_else(|| error("import target table not found"))?;
        let mut disable_precheck = false;
        let mut quota = importer::DefaultDiskQuota.0;
        let mut checksum_level = importer::PostOpLevel::Required;
        for option in &statement.Options {
            let value = option
                .Value
                .as_ref()
                .map(|expr| crate::dml_runtime::EvalExpr(expr, &HashMap::new(), None))
                .transpose()?
                .flatten();
            match option.Name.to_ascii_lowercase().as_str() {
                "disable_precheck" | "disable_tikv_import_mode" => {
                    if option.Value.is_some() {
                        return Err(error("flag import option does not accept a value"));
                    }
                    if option.Name.eq_ignore_ascii_case("disable_precheck") {
                        disable_precheck = true;
                    }
                }
                "thread" => {
                    let n = value
                        .ok_or_else(|| error("thread requires a value"))?
                        .parse::<usize>()
                        .map_err(error)?;
                    if n == 0 {
                        return Err(error("thread must be positive"));
                    }
                }
                "disk_quota" => {
                    let value = value.ok_or_else(|| error("disk_quota requires a value"))?;
                    quota = importer::parseByteSize(&value).map_err(error)?;
                    if quota <= 0 {
                        return Err(error("disk quota must be positive"));
                    }
                }
                "checksum_table" => checksum_level
                    .FromStringValue(
                        &value.ok_or_else(|| error("checksum_table requires a value"))?,
                    )
                    .map_err(error)?,
                other => return Err(error(format!("unsupported query import option {other}"))),
            }
        }
        let prefix = astersql_tablecodec::GenTablePrefix(table.ID);
        if !disable_precheck {
            let nonempty = self
                .domain
                .storage()
                .with_storage(|store| {
                    let mut it = store
                        .GetSnapshot(kv::MaxVersion)
                        .Iter(prefix.clone(), Some(prefix.PrefixNext()))?;
                    let nonempty = it.Valid();
                    it.Close();
                    Ok::<_, kv::errors::SharedError>(nonempty)
                })
                .map_err(error)?;
            if nonempty {
                return Err(error("IMPORT INTO target table is not empty"));
            }
        }
        // The SELECT runs through the relational executor against the current
        // transaction's snapshot. Its result is converted to canonical Datums.
        let selected = self.execute_insert_select_node(source.as_ref())?;
        let visible = table.Columns.iter().filter(|c| !c.Hidden).count();
        if selected.columns.len() != visible {
            return Err(error("query import target/source column counts differ"));
        }
        let count = selected.rows.len();
        let (sender, receiver) = mpsc::channel();
        for (batch_index, batch) in selected.rows.chunks(1024).enumerate() {
            let rows = batch
                .iter()
                .map(|row| {
                    selected
                        .columns
                        .iter()
                        .map(|col| match row.get(col).cloned().flatten() {
                            None => encode::Datum::Null,
                            Some(value) => match super::row_codec::binary_runtime_bytes(&value) {
                                Some(b) => encode::Datum::Bytes(b),
                                None => encode::Datum::String(value),
                            },
                        })
                        .collect()
                })
                .collect();
            sender
                .send(importer::QueryChunk {
                    rows,
                    row_id_offset: (batch_index * 1024) as i64,
                })
                .map_err(error)?;
        }
        drop(sender);
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(error)?
            .0 as u64;
        let allocators = astersql_lightning_backend_kv::NewPanickingAllocators(false);
        let runtime = QueryRuntime {
            allocators: allocators.clone(),
            table: table.clone(),
            flags: self.dml_type_flags(),
            mode,
            chunks: Arc::new(Mutex::new(receiver)),
        };
        let physical = super::import_sst::Backend::new(self.domain.clone(), 0)?;
        let engines = backend::MakeEngineManager(physical.clone());
        let context = encode::Context::default();
        let name = format!("{db}.{}", table.Name.O);
        let data = engines
            .OpenEngine(&context, &backend::EngineConfig::default(), &name, 1)
            .map_err(error)?;
        let index = engines
            .OpenEngine(
                &context,
                &backend::EngineConfig::default(),
                &name,
                importer::IndexEngineID,
            )
            .map_err(error)?;
        let checksum = Arc::new(Mutex::new(
            verification::NewKVGroupChecksumWithKeyspace(&[]),
        ));
        importer::ProcessChunk(
            &context,
            &importer::Chunk::default(),
            &runtime,
            &data,
            &index,
            Some(checksum.clone()),
            None,
        )
        .map_err(error)?;
        // Once writers finish, flushing the complete engines also relieves disk
        // quota pressure. Physical import owns splitting and Write/MultiIngest.
        let _pressure = physical.disk_quota_pressure(quota);
        for engine in [data, index] {
            let closed = engine.Close(&context).map_err(error)?;
            closed
                .Import(&context, 96 * 1024 * 1024, 960_000)
                .map_err(error)?;
            closed.Cleanup(&context).map_err(error)?;
        }
        self.import_files.borrow_mut().sst_stats = physical.stats.lock().unwrap().clone();
        use astersql_lightning_backend_kv::AllocatorType;
        for (kind, allocator_type, needed) in [
            (
                0,
                AllocatorType::AutoIncrementType,
                table.GetAutoIncrementColInfo().is_some(),
            ),
            (
                1,
                AllocatorType::AutoRandomType,
                table.ContainsAutoRandomBits(),
            ),
            (
                2,
                AllocatorType::RowIDAllocType,
                !table.PKIsHandle && !table.IsCommonHandle,
            ),
        ] {
            if needed {
                let maximum = allocators.Get(allocator_type).Base();
                if maximum > 0 {
                    self.allocate_runtime_auto_id(table.ID, Some(maximum as u64), kind, 1, 1)?;
                }
            }
        }
        let local = checksum.lock().unwrap().MergedChecksum();
        importer::VerifyChecksum(
            &importer::Plan {
                Checksum: checksum_level,
                ..Default::default()
            },
            &local,
            || {
                self.domain
                    .storage()
                    .with_storage(|store| {
                        let mut remote = verification::NewKVChecksum();
                        let mut it = store
                            .GetSnapshot(kv::MaxVersion)
                            .Iter(prefix.clone(), Some(prefix.PrefixNext()))?;
                        while it.Valid() {
                            remote.UpdateOne(&verification::KvPair {
                                key: it.Key().0,
                                val: it.Value(),
                                ..Default::default()
                            });
                            it.Next()?;
                        }
                        it.Close();
                        Ok::<_, kv::errors::SharedError>(importer::RemoteChecksum {
                            Schema: db.clone(),
                            Table: table.Name.O.clone(),
                            Checksum: remote.Sum(),
                            TotalKVs: remote.SumKVS(),
                            TotalBytes: remote.SumSize(),
                        })
                    })
                    .map_err(|e| e.to_string())
            },
        )
        .map_err(error)?;
        Ok(ConcreteRecordSet::new(
            vec!["Imported_Rows".into()],
            vec![vec![count.to_string()]],
        ))
    }
}
