// Copyright 2026 AsterSQL.

//! Synchronous file import for the concrete SQL/KV runtime. Storage is an
//! injected boundary; region planning, parsing, row writes and index checks use
//! the same implementations as other production entry points.
use super::*;
use astersql_dxf_framework_storage as dxf;
use astersql_executor_importer as importer;
use astersql_lightning_mydump as dump;

#[derive(Clone, Debug)]
pub struct ImportFileSubtask {
    pub step: i64,
    pub engine_id: i32,
    pub rows: usize,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct ImportFileTask {
    pub id: i64,
    pub key: String,
    pub state: String,
    pub subtasks: Vec<ImportFileSubtask>,
}

pub(super) struct ImportFiles {
    pub(super) storage: Option<Arc<dyn dump::Storage>>,
    region_size: i64,
    sst_stats: kv::SSTImportStats,
}
impl Default for ImportFiles {
    fn default() -> Self {
        Self {
            storage: None,
            region_size: 96 * 1024 * 1024,
            sst_stats: kv::SSTImportStats::default(),
        }
    }
}

fn error(error: impl std::fmt::Display) -> SessionError {
    SessionError::new(error.to_string())
}

impl ConcreteSession {
    pub fn LastImportSSTStats(&self) -> kv::SSTImportStats {
        self.import_files.borrow().sst_stats.clone()
    }
    /// Install an object-storage boundary. Paths remain the SQL source URIs.
    pub fn SetImportFileStorage(&self, storage: Arc<dyn dump::Storage>) {
        self.import_files.borrow_mut().storage = Some(storage);
    }

    /// Set the region limit used by the production region planner, returning
    /// the previous value so callers can restore it at the end of a scope.
    pub fn SetImportRegionSize(&self, size: i64) -> SessionResult<i64> {
        if size <= 0 {
            return Err(error("region size must be positive"));
        }
        Ok(std::mem::replace(
            &mut self.import_files.borrow_mut().region_size,
            size,
        ))
    }

    pub fn ImportFileTaskByKeyWithHistory(&self, key: &str) -> SessionResult<ImportFileTask> {
        let manager = self.ImportTaskManager()?;
        let task = manager
            .GetTaskByKeyWithHistory((), key.to_owned())
            .map_err(error)?;
        let subtasks = manager
            .GetSubtasksWithHistory((), task.ID, astersql_dxf_framework_proto::ImportStepImport)
            .map_err(error)?
            .unwrap_or_default();
        Ok(ImportFileTask {
            id: task.ID,
            key: task.Key.clone(),
            state: task.State.to_owned(),
            subtasks: subtasks
                .into_iter()
                .map(|subtask| {
                    let summary: serde_json::Value =
                        serde_json::from_str(&subtask.Summary).unwrap_or_default();
                    ImportFileSubtask {
                        step: subtask.Step,
                        engine_id: subtask.Ordinal - 1,
                        rows: summary["row_count"].as_u64().unwrap_or_default() as usize,
                        state: subtask.State.to_owned(),
                    }
                })
                .collect(),
        })
    }

    pub(super) fn execute_import_file(
        &self,
        statement: &ast::ImportIntoStmt,
        sql: &str,
    ) -> SessionResult<ConcreteRecordSet> {
        if !statement
            .Options
            .iter()
            .any(|option| option.Name.eq_ignore_ascii_case("split_file"))
        {
            return self.execute_import_compression(statement);
        }
        let mut csv = dump::CsvConfig::default();
        let mut skip = 0_u64;
        let mut engine_size: f64 = 100.0 * 1024.0 * 1024.0 * 1024.0;
        if statement.Select.is_some()
            || !statement.ColumnAssignments.is_empty()
            || !statement.ColumnsAndUserVars.is_empty()
        {
            return Err(error("file import column mapping is not configured"));
        }
        if statement
            .Format
            .as_deref()
            .is_some_and(|format| !format.eq_ignore_ascii_case("csv"))
        {
            return Err(error("split_file requires CSV input"));
        }
        for option in &statement.Options {
            let value = option
                .Value
                .as_ref()
                .map(|value| crate::dml_runtime::EvalExpr(value, &HashMap::new(), None))
                .transpose()?
                .flatten();
            let required = || {
                value
                    .clone()
                    .ok_or_else(|| error(format!("{} requires a value", option.Name)))
            };
            match option.Name.to_ascii_lowercase().as_str() {
                "split_file" => {}
                "lines_terminated_by" => csv.lines_terminated_by = required()?,
                "fields_terminated_by" => csv.fields_terminated_by = required()?,
                "fields_enclosed_by" => csv.fields_enclosed_by = required()?,
                "fields_escaped_by" => csv.fields_escaped_by = required()?,
                "skip_rows" => skip = required()?.parse().map_err(error)?,
                "__max_engine_size" => engine_size = required()?.parse().map_err(error)?,
                other => return Err(error(format!("unsupported file import option {other}"))),
            }
        }
        if csv.lines_terminated_by.is_empty() || engine_size <= 0.0 || !engine_size.is_finite() {
            return Err(error("invalid split_file options"));
        }
        let (storage, region_size) = {
            let files = self.import_files.borrow();
            (
                files
                    .storage
                    .clone()
                    .ok_or_else(|| error("import object storage is not configured"))?,
                files.region_size,
            )
        };
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.O.clone()
        };
        let table = self
            .resolve_runtime_table(&database, &statement.Table.Name.L)
            .ok_or_else(|| error("import target table not found"))?;
        let size = storage
            .list()
            .map_err(error)?
            .into_iter()
            .find(|(path, _)| path == &statement.Path)
            .ok_or_else(|| error(format!("source file {} not found", statement.Path)))?
            .1;
        let mut config = dump::NewDataDivideConfig();
        // Match Go LoadDataController.PopulateChunks: the planner needs the target
        // column count for row-ID estimates and SplitFile for CSV boundaries.
        config.column_count = table.Columns.len();
        config.strict_format = true;
        config.region_size = region_size;
        config.engine_data_size = engine_size;
        config.csv = csv;
        let mut file = dump::FileInfo {
            file_meta: dump::FileMeta {
                path: statement.Path.clone(),
                source_type: dump::SourceType::Csv,
                ..Default::default()
            },
            ..Default::default()
        };
        file.file_meta.file_size = size;
        file.file_meta.real_size = size;
        let metadata = dump::MDTableMeta {
            db: database,
            name: statement.Table.Name.O.clone(),
            data_files: vec![file],
            total_size: size,
            is_row_ordered: true,
            ..Default::default()
        };
        let regions =
            dump::MakeTableRegions(&metadata, &config, storage.as_ref()).map_err(error)?;
        let mut engines = BTreeMap::<i32, Vec<dump::TableRegion>>::new();
        for region in regions {
            engines.entry(region.engine_id).or_default().push(region);
        }
        let manager = self.ImportTaskManager()?;
        let executor_id = format!("canonical-import-{}", std::process::id());
        dxf::SetNodeResource(dxf::proto::NewNodeResource(
            std::thread::available_parallelism().map_or(1, |n| n.get()) as i32,
            0,
            0,
        ));
        manager
            .InitMeta((), executor_id.clone(), String::new())
            .map_err(error)?;
        let created_by = format!(
            "{}@{}",
            self.login_user.as_deref().unwrap_or("root"),
            self.authenticated_host.as_deref().unwrap_or("%")
        );
        let mut job_id = 0_i64;
        manager.WithNewSession(|se| {
            let executor = se.GetSQLExecutor();
            dxf::sqlexec::ExecSQL((), executor.clone(),
                "insert into mysql.tidb_import_jobs(table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step) values(%?,%?,%?,%?,%?,%?,%?,%?)",
                vec![metadata.db.clone().into(), metadata.name.clone().into(), table.ID.into(), created_by.clone().into(), serde_json::json!({"file_location":statement.Path}).to_string().into(), size.into(), "running".into(), "importing".into()])?;
            let rows = dxf::sqlexec::ExecSQL((), executor, "select @@last_insert_id", Vec::new())?;
            job_id = rows.first().ok_or_else(|| dxf::Error::new("missing import job ID"))?.GetInt64(0);
            Ok(())
        }).map_err(error)?;
        let mut task_meta = astersql_dxf_importinto::TaskMeta {
            JobID: job_id,
            Stmt: sql.to_owned(),
            Plan: importer::Plan {
                DBName: metadata.db.clone(),
                TableInfo: Some(Arc::new(table.clone())),
                Path: statement.Path.clone(),
                Format: "csv".into(),
                SplitFile: true,
                IgnoreLines: skip,
                TotalFileSize: size,
                ThreadCnt: 1,
                User: created_by,
                LineFieldsInfo: importer::LineFieldsInfo {
                    FieldsTerminatedBy: config.csv.fields_terminated_by.clone(),
                    FieldsEnclosedBy: config.csv.fields_enclosed_by.clone(),
                    FieldsEscapedBy: config.csv.fields_escaped_by.clone(),
                    LinesTerminatedBy: config.csv.lines_terminated_by.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ChunkMap: engines
                .iter()
                .map(|(id, regions)| {
                    (
                        *id,
                        regions
                            .iter()
                            .map(|region| importer::Chunk {
                                Path: region.file_meta.path.clone(),
                                FileSize: region.file_meta.file_size,
                                Offset: region.chunk.offset,
                                EndOffset: region.chunk.end_offset,
                                PrevRowIDMax: region.chunk.prev_row_id_max,
                                RowIDMax: region.chunk.row_id_max,
                                Type: region.file_meta.source_type,
                                Compression: region.file_meta.compression,
                                ..Default::default()
                            })
                            .collect(),
                    )
                })
                .collect(),
            ..Default::default()
        };
        let task_id = manager
            .CreateTask(
                (),
                astersql_dxf_importinto::TaskKey(job_id),
                dxf::proto::ImportInto,
                String::new(),
                1,
                String::new(),
                1,
                dxf::proto::ExtraParams::default(),
                task_meta.Marshal().map_err(error)?,
            )
            .map_err(error)?;
        let task = manager.GetTaskByID((), task_id).map_err(error)?;
        let subtasks = engines
            .keys()
            .map(|engine_id| {
                let chunks = &task_meta.ChunkMap[engine_id];
                Ok(dxf::proto::Subtask {
                    SubtaskBase: dxf::proto::SubtaskBase {
                        Step: astersql_dxf_framework_proto::ImportStepImport,
                        Type: dxf::proto::ImportInto,
                        TaskID: task_id,
                        Concurrency: 1,
                        ExecID: executor_id.clone(),
                        Ordinal: engine_id + 1,
                        ..Default::default()
                    },
                    Meta: astersql_dxf_importinto::ImportStepMeta {
                        ID: *engine_id,
                        Chunks: chunks.clone(),
                        ..Default::default()
                    }
                    .Marshal()
                    .map_err(error)?,
                    ..Default::default()
                })
            })
            .collect::<SessionResult<Vec<_>>>()?;
        manager
            .SwitchTaskStep(
                (),
                task,
                dxf::proto::TaskStateRunning,
                astersql_dxf_framework_proto::ImportStepImport,
                subtasks,
            )
            .map_err(error)?;
        let subtasks = manager
            .GetSubtasksWithHistory((), task_id, astersql_dxf_framework_proto::ImportStepImport)
            .map_err(error)?
            .unwrap_or_default();
        self.import_files.borrow_mut().sst_stats = kv::SSTImportStats::default();
        let result = (|| -> SessionResult<usize> {
            use astersql_lightning_backend as backend;
            let physical = super::import_sst::Backend::new(self.domain.clone(), task_id)?;
            let engine_manager = backend::MakeEngineManager(physical.clone());
            let context = astersql_lightning_backend_encode::Context::default();
            let runtime = super::import_sst::Runtime {
                table: table.clone(),
                storage,
                config,
                skip_rows: skip,
                flags: self.dml_type_flags(),
            };
            let mut count = 0;
            for engine_id in engines.keys() {
                let subtask = subtasks
                    .iter()
                    .find(|subtask| subtask.Ordinal == engine_id + 1)
                    .ok_or_else(|| error("planned DXF subtask missing"))?;
                manager
                    .StartSubtask((), subtask.ID, executor_id.clone())
                    .map_err(error)?;
                let name = format!("{}.{}#{engine_id}", metadata.db, metadata.name);
                let data = engine_manager
                    .OpenEngine(
                        &context,
                        &backend::EngineConfig::default(),
                        &name,
                        *engine_id,
                    )
                    .map_err(error)?;
                let indices = engine_manager
                    .OpenEngine(
                        &context,
                        &backend::EngineConfig::default(),
                        &name,
                        importer::IndexEngineID,
                    )
                    .map_err(error)?;
                let progress = Arc::new(super::import_sst::Progress::default());
                let chunks = &task_meta.ChunkMap[engine_id];
                let mut maximum_row_id = 0;
                for chunk in chunks {
                    let before = progress.rows.load(std::sync::atomic::Ordering::Relaxed);
                    importer::ProcessChunk(
                        &context,
                        chunk,
                        &runtime,
                        &data,
                        &indices,
                        None,
                        Some(progress.clone()),
                    )
                    .map_err(error)?;
                    let rows = progress.rows.load(std::sync::atomic::Ordering::Relaxed) - before;
                    maximum_row_id = maximum_row_id.max(chunk.PrevRowIDMax + rows);
                }
                let rows =
                    usize::try_from(progress.rows.load(std::sync::atomic::Ordering::Relaxed))
                        .map_err(error)?;
                for engine in [data, indices] {
                    let closed = engine.Close(&context).map_err(error)?;
                    closed
                        .Import(&context, 96 * 1024 * 1024, 960_000)
                        .map_err(error)?;
                    closed.Cleanup(&context).map_err(error)?;
                }
                self.import_files.borrow_mut().sst_stats = physical.stats.lock().unwrap().clone();
                if !table.PKIsHandle && !table.IsCommonHandle {
                    self.allocate_runtime_auto_id(
                        table.ID,
                        Some(u64::try_from(maximum_row_id).map_err(error)?),
                        2,
                        1,
                        1,
                    )?;
                }
                manager
                    .UpdateSubtaskSummary(
                        (),
                        subtask.ID,
                        dxf::execute::SubtaskSummary {
                            RowCount: rows as i64,
                        },
                    )
                    .map_err(error)?;
                manager
                    .FinishSubtask((), executor_id.clone(), subtask.ID, subtask.Meta.clone())
                    .map_err(error)?;
                count += rows;
            }
            Ok(count)
        })();
        let (status, summary, failure) = match &result {
            Ok(rows) => (
                "finished",
                serde_json::json!({"imported_rows":rows}).to_string(),
                None,
            ),
            Err(error) => ("failed", String::new(), Some(error.to_string())),
        };
        manager.ExecuteSQLWithNewSession((), "update mysql.tidb_import_jobs set status=%?,summary=%?,error_message=%?,end_time=CURRENT_TIMESTAMP() where id=%?", vec![status.into(), summary.into(), failure.clone().map_or(dxf::Value::Null, dxf::Value::String), job_id.into()]).map_err(error)?;
        if let Some(failure) = failure {
            manager
                .FailSubtask(
                    (),
                    executor_id.clone(),
                    task_id,
                    Some(dxf::Error::new(failure.clone())),
                )
                .map_err(error)?;
            manager
                .FailTask(
                    (),
                    task_id,
                    dxf::proto::TaskStateRunning,
                    dxf::Error::new(failure),
                )
                .map_err(error)?;
        } else {
            manager.SucceedTask((), task_id).map_err(error)?;
        }
        let mut task = manager.GetTaskByID((), task_id).map_err(error)?;
        if let Ok(rows) = &result {
            task_meta.Summary.ImportedRows = *rows as i64;
        }
        task.Meta = task_meta.Marshal().map_err(error)?;
        manager
            .TransferTasks2History((), vec![task])
            .map_err(error)?;
        let count = result?;
        Ok(ConcreteRecordSet::new(
            vec!["Job_ID".into(), "Imported_Rows".into()],
            vec![vec![job_id.to_string(), count.to_string()]],
        ))
    }
}
