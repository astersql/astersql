// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 导出 Writer：消费 Task 并写入外部存储，对应 Go `export/writer.go`。
// 每个 Writer 绑定一条 DB 连接与一个 Storage；元数据走 SQL 文件，数据走 FileFormat 分支。

// Writer 是并发导出池中的工作协程等价物，id 用于日志区分实例。
pub struct Writer {
    // id 在 Writer 池中的序号，对应 Go writer id。
    pub id: i64,
    // tctx 携带 logger 与 cancel，贯穿 Write* 与 IR 生命周期。
    pub tctx: tcontext::Context,
    // conf 含 OutputFileTemplate、FileType、ServerInfo 等全局导出配置。
    pub conf: Arc<Config>,
    // conn 写表数据时传给 TableDataIR::Start，关闭后为 None。
    pub conn: Option<Conn>,
    // ext_storage 实际落盘目标（本地/S3 等），经 LazyStringWriter 延迟 Create。
    pub ext_storage: Arc<dyn Storage>,
    // file_fmt 由 NewWriter 从 conf.FileType 解析得到。
    pub file_fmt: FileFormat,
    metrics: Option<metrics>,
    // received_task_count 每 handleTask 递增，供 countTotalTask 汇总。
    pub received_task_count: i32,
    // finish_task_callback 单任务完成钩子，默认空实现。
    pub finish_task_callback: Box<dyn Fn(&TaskEnum) + Send>,
    // finish_table_callback 整表完成钩子，默认空实现。
    pub finish_table_callback: Box<dyn Fn(&TaskEnum) + Send>,
}

// Preserve shared collector ownership across the existing write paths.
pub fn NewWriter(
    tctx: tcontext::Context,
    id: i64,
    config: Arc<Config>,
    conn: Conn,
    external_store: Arc<dyn Storage>,
    metrics: Option<&metrics>,
) -> Writer {
    let file_fmt = match config.FileType.to_ascii_lowercase().as_str() {
        FileFormatSQLTextString => FileFormat::FileFormatSQLText,
        FileFormatCSVString => FileFormat::FileFormatCSV,
        FileFormatParquetString => FileFormat::FileFormatParquet,
        _ => FileFormat::FileFormatUnknown,
    };
    // finish_* 回调默认为 no-op，上层可在创建后替换以挂 metrics/进度。
    Writer {
        id,
        tctx,
        conf: config,
        conn: Some(conn),
        ext_storage: external_store,
        file_fmt,
        metrics: metrics.cloned(),
        received_task_count: 0,
        finish_task_callback: Box::new(|_| {}),
        finish_table_callback: Box::new(|_| {}),
    }
}

impl Writer {
    pub fn setFinishTaskCallBack(&mut self, callback: Box<dyn Fn(&TaskEnum) + Send>) {
        self.finish_task_callback = callback;
    }

    pub fn setFinishTableCallBack(&mut self, callback: Box<dyn Fn(&TaskEnum) + Send>) {
        self.finish_table_callback = callback;
    }

    // Task 分发入口：递增计数后按枚举变体调用对应 Write*，与 Go handleTask 结构一致。
    pub fn handleTask(&mut self, task: &mut TaskEnum) -> Result<()> {
        self.received_task_count += 1;
        match task {
            TaskEnum::DatabaseMeta(t) => {
                self.WriteDatabaseMeta(&t.DatabaseName, &t.CreateDatabaseSQL)
            }
            TaskEnum::TableMeta(t) => {
                self.WriteTableMeta(&t.DatabaseName, &t.TableName, &t.CreateTableSQL)
            }
            TaskEnum::ViewMeta(t) => self.WriteViewMeta(
                &t.DatabaseName,
                &t.ViewName,
                &t.CreateTableSQL,
                &t.CreateViewSQL,
            ),
            TaskEnum::SequenceMeta(t) => {
                self.WriteSequenceMeta(&t.DatabaseName, &t.SequenceName, &t.CreateSequenceSQL)
            }
            TaskEnum::PolicyMeta(t) => self.WritePolicyMeta(&t.PolicyName, &t.CreatePolicySQL),
            TaskEnum::TableData(t) => {
                let idx = t.ChunkIndex;
                let is_last_chunk = idx + 1 == t.TotalChunks;
                self.WriteTableData(t.Meta.as_ref(), t.Data.as_mut(), idx)?;
                if is_last_chunk {
                    (self.finish_table_callback)(task);
                }
                Ok(())
            }
        }
    }

    // placement policy 元数据：路径模板 placement-policy，扩展名 .sql。
    // 路径模板 Execute 失败会直接向上返回，不写部分文件。
    pub fn WritePolicyMeta(&self, policy: &str, create_sql: &str) -> Result<()> {
        let path =
            self.conf
                .OutputFileTemplate
                .Execute(outputFileTemplatePolicy, "", "", "", policy)?;
        self.writeMetaToFile("placement-policy", create_sql, &format!("{path}.sql"))
    }

    pub fn WriteDatabaseMeta(&self, db: &str, create_sql: &str) -> Result<()> {
        let path =
            self.conf
                .OutputFileTemplate
                .Execute(outputFileTemplateSchema, db, "", "", "")?;
        self.writeMetaToFile(db, create_sql, &format!("{path}.sql"))
    }

    // 表元数据文件名由 table 模板段决定，target 参数用 table 名。
    pub fn WriteTableMeta(&self, db: &str, table: &str, create_sql: &str) -> Result<()> {
        let path =
            self.conf
                .OutputFileTemplate
                .Execute(outputFileTemplateTable, db, table, "", "")?;
        self.writeMetaToFile(table, create_sql, &format!("{path}.sql"))
    }

    // 视图文件先写底层表 DDL（补分号换行），再拼接 CREATE VIEW，与 Go WriteViewMeta 相同。
    pub fn WriteViewMeta(
        &self,
        db: &str,
        view: &str,
        create_table_sql: &str,
        create_view_sql: &str,
    ) -> Result<()> {
        let table_path =
            self.conf
                .OutputFileTemplate
                .Execute(outputFileTemplateTable, db, view, "", "")?;
        let view_path =
            self.conf
                .OutputFileTemplate
                .Execute(outputFileTemplateView, db, view, "", "")?;
        self.writeMetaToFile(db, create_table_sql, &format!("{table_path}.sql"))?;
        self.writeMetaToFile(db, create_view_sql, &format!("{view_path}.sql"))
    }

    pub fn WriteSequenceMeta(&self, db: &str, sequence: &str, create_sql: &str) -> Result<()> {
        let path = self.conf.OutputFileTemplate.Execute(
            outputFileTemplateSequence,
            db,
            sequence,
            "",
            "",
        )?;
        self.writeMetaToFile(sequence, create_sql, &format!("{path}.sql"))
    }

    // handleTask 对 TableData 分支的唯一入口，需 mut self 以持有 conn。
    pub fn WriteTableData(
        &mut self,
        meta: &dyn TableMeta,
        ir: &mut dyn TableDataIR,
        current_chunk: i32,
    ) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| errors_new("writer conn closed"))?;
        ir.Start(&self.tctx, conn)?;
        let dynamic_meta = if self.conf.SQL.is_empty() {
            None
        } else {
            let rows = ir
                .RawRows()
                .ok_or_else(|| errors_new("raw rows unavailable for SQL query metadata"))?;
            let inferred = setTableMetaFromRows(self.conf.ServerInfo.ServerType, rows)?;
            if let Some(err) = rows.Err() {
                return Err(err);
            }
            Some(inferred)
        };
        let meta = dynamic_meta.as_deref().unwrap_or(meta);
        let mut namer = newOutputFileNamer(
            meta,
            current_chunk,
            self.conf.Rows != 0,
            self.conf.FileSize != 0,
        );
        if self.file_fmt == FileFormat::FileFormatCSV {
            let mut iter = ir.Rows();
            let result = (|| -> Result<()> {
                while iter.HasNext() {
                    let (name, _) =
                        namer.NextName(&self.conf.OutputFileTemplate, FileFormatCSVString)?;
                    let mut lazy = LazyStringWriter::new(self.ext_storage.clone(), name);
                    lazy.option = Some(astersql_objstore_storeapi::WriterOption {
                        Concurrency: uploadConcurrency,
                        PartSize: uploadPartSize,
                    });
                    let result = writeCSVFile(
                        &self.conf,
                        meta,
                        iter.as_mut(),
                        &mut lazy,
                        self.metrics.as_ref(),
                    );
                    let closed = lazy.Close();
                    result?;
                    closed?;
                    if self.conf.FileSize == UnspecifiedSize {
                        break;
                    }
                }
                iter.Error().map_or(Ok(()), Err)
            })();
            let closed_iter = iter.Close();
            let closed_ir = ir.Close();
            result?;
            closed_iter?;
            return closed_ir;
        }
        if self.file_fmt == FileFormat::FileFormatSQLText
            && (self.conf.FileSize != UnspecifiedSize || self.conf.StatementSize != UnspecifiedSize)
        {
            let result = self.writeSplitSql(meta, ir, &mut namer);
            let close_ir_result = ir.Close();
            result?;
            return close_ir_result;
        }
        let extension = if self.file_fmt == FileFormat::FileFormatParquet {
            match self.conf.ParquetCompressType {
                CompressType::Gzip => "gz.parquet",
                CompressType::Snappy => "snappy.parquet",
                CompressType::Zstd => "zstd.parquet",
                CompressType::Lzo => "lzo.parquet",
                CompressType::NoCompression => FileFormatParquetString,
            }
        } else {
            self.file_fmt.Extension()
        };
        let (file_name, _) = namer.NextName(&self.conf.OutputFileTemplate, extension)?;
        let mut lazy = LazyStringWriter::new(self.ext_storage.clone(), file_name);
        if self.file_fmt == FileFormat::FileFormatSQLText {
            lazy.option = Some(astersql_objstore_storeapi::WriterOption {
                Concurrency: uploadConcurrency,
                PartSize: uploadPartSize,
            });
        }
        let write_result = self.file_fmt.WriteInsert(
            &self.tctx,
            &self.conf,
            meta,
            ir,
            &mut lazy,
            self.metrics.as_ref(),
        );
        let close_writer_result = lazy.Close();
        let close_ir_result = ir.Close();
        write_result?;
        close_writer_result?;
        close_ir_result
    }

    fn writeSplitSql(
        &self,
        meta: &dyn TableMeta,
        ir: &mut dyn TableDataIR,
        namer: &mut outputFileNamer,
    ) -> Result<()> {
        let mut iter = ir.Rows();
        let result = (|| -> Result<()> {
            while iter.HasNext() {
                let (name, _) =
                    namer.NextName(&self.conf.OutputFileTemplate, FileFormatSQLTextString)?;
                let mut lazy = LazyStringWriter::new(self.ext_storage.clone(), name);
                lazy.option = Some(astersql_objstore_storeapi::WriterOption {
                    Concurrency: uploadConcurrency,
                    PartSize: uploadPartSize,
                });
                let result = writeSQLFile(
                    &self.conf,
                    meta,
                    iter.as_mut(),
                    &mut lazy,
                    self.metrics.as_ref(),
                );
                let closed = lazy.Close();
                result?;
                closed?;
                if self.conf.FileSize == UnspecifiedSize {
                    break;
                }
            }
            iter.Error().map_or(Ok(()), Err)
        })();
        let closed = iter.Close();
        result?;
        closed
    }

    // 元数据写入：special comments 逐行 + MetaSQL 正文，经 LazyStringWriter 落盘。
    pub fn writeMetaToFile(&self, target: &str, meta_sql: &str, path: &str) -> Result<()> {
        let mut md = metaData {
            target: target.to_string(),
            meta_sql: meta_sql.to_string(),
            // spec_cmts 按 ServerType 注入 conditional comments（如 SET NAMES）。
            spec_cmts: getSpecialComments(self.conf.ServerInfo.ServerType),
        };
        let mut lazy = LazyStringWriter::new(self.ext_storage.clone(), path.to_string());
        let mut it = md.SpecialComments();
        while it.HasNext() {
            let mut cmt = it.Next();
            cmt.push('\n');
            lazy.Write(cmt.as_bytes())?;
        }
        lazy.Write(md.MetaSQL().as_bytes())?;
        // LazyStringWriter Close 提交 storage 文件。
        lazy.Close()
    }
}

// 汇总所有 Writer 已接收任务数，用于进度统计。
pub fn countTotalTask(writers: &[Writer]) -> i32 {
    // 对各 Writer.received_task_count 求和。
    writers.iter().map(|w| w.received_task_count).sum()
}

// 数据文件命名状态：DB/Table/Index 供 OutputTemplate data 段展开。
pub struct outputFileNamer {
    // DB/Table 来自 TableMeta，Index 为 chunk 序号。
    pub DB: String,
    pub Table: String,
    pub ChunkIndex: i32,
    pub FileIndex: i32,
    rows: bool,
    file_size: bool,
}

pub fn newOutputFileNamer(
    meta: &dyn TableMeta,
    chunk_idx: i32,
    rows: bool,
    file_size: bool,
) -> outputFileNamer {
    // rows/file_size 决定 Go 的 chunk/file 双索引格式。
    outputFileNamer {
        DB: meta.DatabaseName().to_string(),
        Table: meta.TableName().to_string(),
        ChunkIndex: chunk_idx,
        FileIndex: 0,
        rows,
        file_size,
    }
}

impl outputFileNamer {
    // 九位零填充索引，与 Go IndexStr 一致。
    pub fn IndexStr(&self) -> String {
        match (self.rows, self.file_size) {
            (true, true) => format!("{:09}{:04}", self.ChunkIndex, self.FileIndex),
            (false, true) => format!("{:09}", self.FileIndex),
            _ => format!("{:09}", self.ChunkIndex),
        }
    }
    pub fn NextName(&mut self, tmpl: &OutputTemplate, file_type: &str) -> Result<(String, String)> {
        // 返回 (完整文件名含扩展名, 不含扩展名的 base)。
        let base = tmpl.Execute(
            outputFileTemplateData,
            &self.DB,
            &self.Table,
            &self.IndexStr(),
            "",
        )?;
        self.FileIndex += 1;
        Ok((format!("{base}.{file_type}"), base))
    }
}
