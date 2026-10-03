// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 本文件是 dumpling export 包的主编排层，负责把“配置”真正变成一次可执行的导出任务。
// 它大致分成四个阶段：
// 1. 构造 Dumper，并按顺序完成 logger / storage / DB / server info 等初始化；
// 2. 根据配置与数据库元信息准备待导出的库表列表；
// 3. 生成元数据任务和表数据任务，并交给 writer 消费；
// 4. 在需要时维护 GC safepoint / barrier 等外部保护状态。
// 与 Go 版本相比，当前 Rust 实现保留了初始化顺序和主要数据流，
// 但导出执行路径更偏向单进程、单 writer 的简化骨架。

// Dumper 聚合一次导出会话运行期需要共享的核心状态。
// 这里既有静态配置，也有 DB / storage / metrics / HTTP / PD 这种运行时句柄。
pub struct Dumper {
    // tctx 统一承载取消信号和 logger，是几乎所有子流程的公共入口。
    // conf 以 Arc 共享；若某个步骤需要变更配置，会 clone_for_mutate 后再整体替换。
    pub tctx: tcontext::Context,
    pub conf: Arc<Config>,
    pub db: Option<DB>,
    // ext_storage 由初始化阶段创建，writer 侧直接复用它输出文件。
    // metrics / speedRecorder / totalTables 共同支撑进度和吞吐统计。
    pub ext_storage: Option<Arc<dyn Storage>>,
    pub metrics: Arc<metrics>,
    pub speedRecorder: Arc<Mutex<SpeedRecorder>>,
    pub status: Arc<Mutex<DumpStatus>>,
    // totalTables 会在表清单准备完成后一次性写入，用于进度估算。
    // cancel / http / pd_client 都属于需要在 Close 中显式回收的外部资源。
    pub totalTables: Arc<AtomicI64>,
    pub cancel: Option<astersql_dumpling_context::CancelFunc>,
    pub http: Option<HttpServiceHandle>,
    pub pd_client: Option<PdClient>,
}

pub fn NewDumper(conf: Config) -> Result<Dumper> {
    // Dumper 自己创建一份可取消上下文，确保整个导出会话有统一的停止入口。
    let (tctx, cancel) = tcontext::Background().WithCancel();
    let factory = conf.PromFactory.clone();
    // labels 在构造 metrics 时只读取一次，后续配置副本可安全替换。
    let labels = conf.Labels.clone();
    let mut d = Dumper {
        tctx,
        conf: Arc::new(conf),
        db: None,
        ext_storage: None,
        metrics: std::sync::Arc::new(newMetrics(factory.as_ref(), &labels)),
        speedRecorder: std::sync::Arc::new(Mutex::new(NewSpeedRecorder())),
        status: std::sync::Arc::new(std::sync::Mutex::new(DumpStatus::default())),
        totalTables: std::sync::Arc::new(AtomicI64::new(0)),
        cancel: Some(cancel),
        http: None,
        pd_client: None,
    };
    // 初始化顺序基本对齐 Go：先日志，再外部存储和 HTTP，再 DB，再探测 server info。
    // 这样每一步失败都能尽量用已经就绪的 logger 记录上下文。
    // `runSteps` 把 NewDumper 维持成一个“声明式步骤列表”，方便后续扩展初始化阶段。
    runSteps(
        &mut d,
        &[
            initLogger,
            createExternalStore,
            startHTTPService,
            openSQLDB,
            detectServerInfo,
            resolveAutoConsistency,
            validateResolveAutoConsistency,
            setSessionParam,
        ],
    )?;
    Ok(d)
}

impl Dumper {
    pub fn L(&self) -> Logger {
        // 对外统一暴露 tctx 里的 logger，避免调用方直接碰内部上下文实现。
        self.tctx.L()
    }

    pub fn Close(&mut self) -> Result<()> {
        // Close 既是显式资源回收点，也是取消后台协程/定时更新器的入口。
        // 它应该是幂等的：资源被 take 走后再次调用只会看到空句柄。
        if let Some(c) = self.cancel.take() {
            c.call();
        }
        // HTTP 服务和 PD 客户端都属于“有副作用的外部句柄”，要优先停止。
        if let Some(h) = self.http.take() {
            h.stop();
        }
        if let Some(pd) = self.pd_client.take() {
            pd.Close();
        }
        if let Some(db) = self.db.take() {
            db.Close()?;
        }
        // 指标注册在 registry 上，需要显式卸载，避免多次运行时重复注册。
        self.metrics.unregisterFrom(self.conf.PromRegistry.as_ref());
        Ok(())
    }

    pub fn Dump(&mut self) -> Result<()> {
        if !self.conf.columnFilter.Filters.is_empty() {
            validateColumnFilterOptions(&self.conf, "column-filter-file")?;
        }
        self.metrics.registerTo(self.conf.PromRegistry.as_ref());
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| errors_new("db not open"))?
            .clone();
        let repeatable_read =
            needRepeatableRead(self.conf.ServerInfo.ServerType, &self.conf.Consistency);

        if self.conf.Consistency == ConsistencyTypeLock {
            let conn = createConnWithConsistency(&db, repeatable_read)?;
            let mut conf = (*self.conf).clone_for_mutate();
            let prepare_result = prepareTableListToDumpInner(&self.tctx, &mut conf, &conn);
            let _ = conn.Close();
            prepare_result?;
            self.conf = Arc::new(conf);
        }

        let mut consistency = NewConsistencyController(&self.conf, &db)?;
        consistency.Setup(&self.tctx)?;
        let result = (|| {
            let conn = createConnWithConsistency(&db, repeatable_read)?;
            let mut metadata = newGlobalMetadata(
                self.tctx.clone(),
                self.ext_storage.clone(),
                self.conf.Snapshot.clone(),
            );
            metadata.recordStartTime(SystemTime::now());
            if let Err(err) = metadata.recordGlobalMetaData(&conn, &self.conf.ServerInfo, false) {
                self.tctx.L().Warn(
                    "get global metadata failed",
                    [Field::string("error", err.msg)],
                );
            }
            if self.conf.Consistency != ConsistencyTypeLock {
                let mut conf = (*self.conf).clone_for_mutate();
                prepareTableListToDumpInner(&self.tctx, &mut conf, &conn)?;
                self.conf = Arc::new(conf);
            }
            self.totalTables.store(
                calculateTableCount(&self.conf.Tables) as i64,
                Ordering::SeqCst,
            );

            let mut meta_conn = newBaseConn(
                conn,
                canRebuildConn(&self.conf.Consistency, self.conf.TransactionalConsistency),
                None,
            );
            if self.conf.SQL.is_empty() && !self.conf.columnFilter.Filters.is_empty() {
                let mut conf = self.conf.clone_for_mutate();
                if let Err(err) = prepareColumnProjection(&self.tctx, &mut conf, &mut meta_conn) {
                    if let Some(conn) = meta_conn.DBConn.take() {
                        let _ = conn.Close();
                    }
                    return Err(err);
                }
                self.conf = Arc::new(conf);
            }
            let mut progress = self.startLogProgress(&self.tctx);
            let (tx, rx) = std::sync::mpsc::channel::<TaskEnum>();
            let produce_result = self.dumpDatabases(&mut meta_conn, tx);
            if let Some(conn) = meta_conn.DBConn.take() {
                let _ = conn.Close();
            }
            produce_result?;
            self.metrics.progressReady.store(true, Ordering::SeqCst);

            let store = self
                .ext_storage
                .clone()
                .ok_or_else(|| errors_new("no storage"))?;
            let wconn = db.Conn()?;
            let mut writer = NewWriter(
                self.tctx.clone(),
                0,
                self.conf.clone(),
                wconn,
                store,
                Some(&self.metrics),
            );
            let table_metrics = self.metrics.clone();
            writer.setFinishTableCallBack(Box::new(move |task| {
                if matches!(task, TaskEnum::TableData(_)) {
                    IncCounter(Some(&table_metrics.finishedTablesCounter));
                }
            }));
            let chunk_metrics = self.metrics.clone();
            writer.setFinishTaskCallBack(Box::new(move |task| {
                if matches!(task, TaskEnum::TableData(_)) {
                    chunk_metrics.completedChunks.fetch_add(1, Ordering::SeqCst);
                }
            }));
            while let Ok(mut task) = rx.recv() {
                writer.handleTask(&mut task)?;
                (writer.finish_task_callback)(&task);
            }
            progress.stop();
            if let Some(conn) = writer.conn.take() {
                let _ = conn.Close();
            }
            metadata.recordFinishTime(SystemTime::now());
            metadata.writeGlobalMetaData()
        })();
        if let Err(err) = consistency.TearDown() {
            self.tctx.L().Warn(
                "fail to tear down consistency controller",
                [Field::string("error", err.msg)],
            );
        }
        result
    }

    pub fn dumpDatabases(
        &mut self,
        meta_conn: &mut BaseConn,
        task_chan: std::sync::mpsc::Sender<TaskEnum>,
    ) -> Result<()> {
        // 自定义 SQL 模式不再遍历数据库/表清单，直接生成一个匿名结果任务。
        if !self.conf.SQL.is_empty() {
            self.dumpSQL(meta_conn, &task_chan);
            return Ok(());
        }
        let conf = self.conf.clone();
        if conf.ServerInfo.ServerType == ServerType::ServerTypeTiDB {
            match ListAllPlacementPolicyNames(&self.tctx, meta_conn) {
                Ok(policies) => {
                    for policy in policies {
                        let create = ShowCreatePlacementPolicy(&self.tctx, meta_conn, &policy)?;
                        task_chan
                            .send(TaskEnum::PolicyMeta(NewTaskPolicyMeta(
                                policy,
                                format!("/*T![placement] {} */", create),
                            )))
                            .map_err(|_| errors_new("task channel closed"))?;
                    }
                }
                Err(err) => self.tctx.L().Warn(
                    "fail to dump placement policy",
                    [Field::string("error", err.msg)],
                ),
            }
        }
        for (db_name, tables) in &conf.Tables {
            // 库级 schema 先于表任务写出，保持输出顺序更贴近 Go。
            if !conf.NoSchemas {
                let create = ShowCreateDatabase(&self.tctx, meta_conn, db_name)?;
                task_chan
                    .send(TaskEnum::DatabaseMeta(NewTaskDatabaseMeta(
                        db_name.clone(),
                        create,
                    )))
                    .map_err(|_| errors_new("task channel closed"))?;
            }
            for table in tables {
                let meta = dumpTableMeta(&self.tctx, &conf, meta_conn, db_name, table)?;
                match table.Type {
                    TableType::TableTypeBase => {
                        if !conf.NoSchemas {
                            task_chan
                                .send(TaskEnum::TableMeta(NewTaskTableMeta(
                                    db_name.clone(),
                                    table.Name.clone(),
                                    meta.ShowCreateTable().to_string(),
                                )))
                                .map_err(|_| errors_new("task channel closed"))?;
                        }
                        if !conf.NoData {
                            self.dumpWholeTableDirectly(meta, &task_chan, "", "", 0, 1)?;
                        }
                    }
                    TableType::TableTypeView => {
                        // view 只导出元信息，不生成数据任务。
                        if conf.NoViews {
                            // NoViews 开关只影响 view，不会连带影响普通表。
                            continue;
                        }
                        task_chan
                            .send(TaskEnum::ViewMeta(NewTaskViewMeta(
                                db_name.clone(),
                                table.Name.clone(),
                                meta.ShowCreateTable().to_string(),
                                meta.ShowCreateView().to_string(),
                            )))
                            .map_err(|_| errors_new("task channel closed"))?;
                    }
                    TableType::TableTypeSequence => {
                        // sequence 同样只需要对应的 create 语句元数据。
                        if conf.NoSequences {
                            // 与 view 一样，sequence 也通过专门开关独立控制。
                            continue;
                        }
                        task_chan
                            .send(TaskEnum::SequenceMeta(NewTaskSequenceMeta(
                                db_name.clone(),
                                table.Name.clone(),
                                meta.ShowCreateTable().to_string(),
                            )))
                            .map_err(|_| errors_new("task channel closed"))?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn dumpWholeTableDirectly(
        &self,
        meta: Box<dyn TableMeta>,
        task_chan: &std::sync::mpsc::Sender<TaskEnum>,
        partition: &str,
        order_by_clause: &str,
        current_chunk: i32,
        total_chunks: i32,
    ) -> Result<()> {
        // `SelectAllFromTable` 代表最直接的整表扫描路径，不做额外 chunk 规划。
        // 当前 chunk 编号始终由调用方给定，本函数只负责把它封装成任务。
        let data = SelectAllFromTable(&self.conf, meta.as_ref(), partition, order_by_clause);
        let task = self.newTaskTableData(meta, data, current_chunk, total_chunks);
        task_chan
            .send(TaskEnum::TableData(task))
            .map_err(|_| errors_new("task channel closed"))
    }

    pub fn dumpSQL(&self, meta_conn: &mut BaseConn, task_chan: &std::sync::mpsc::Sender<TaskEnum>) {
        // `--sql` 路径下没有真实库表名，因此构造一个匿名 `result` 表元信息。
        // `_meta_conn` 暂未使用，但保留签名是为了和普通导出入口尽量统一。
        let td = newTableData(self.conf.SQL.clone(), 0, true);
        // anonymous meta
        let meta = Box::new(tableMeta {
            database: String::new(),
            table: "result".into(),
            col_types: vec![],
            selected_field: String::new(),
            source_col_types: vec![],
            spec_cmts: getSpecialComments(self.conf.ServerInfo.ServerType),
            show_create_table: String::new(),
            show_create_view: String::new(),
            avg_row_length: 0,
            has_implicit_row_id: false,
        });
        // 单 SQL 查询天然只有一个 chunk。
        let task = self.newTaskTableData(meta, Box::new(td), 0, 1);
        let estimate = detectEstimateRows(
            &self.tctx,
            meta_conn,
            &format!("EXPLAIN {}", self.conf.SQL),
            &["rows", "estRows", "count"],
        );
        AddCounter(
            Some(&self.metrics.estimateTotalRowsCounter),
            estimate as f64,
        );
        self.totalTables.store(1, Ordering::SeqCst);
        let _ = task_chan.send(TaskEnum::TableData(task));
    }

    pub fn newTaskTableData(
        &self,
        meta: Box<dyn TableMeta>,
        data: Box<dyn TableDataIR>,
        current_chunk: i32,
        total_chunks: i32,
    ) -> TaskTableData {
        self.metrics.totalChunks.fetch_add(1, Ordering::SeqCst);
        NewTaskTableData(meta, data, current_chunk, total_chunks)
    }
}

pub fn canRebuildConn(consistency: &str, trx_consistency_only: bool) -> bool {
    match consistency {
        ConsistencyTypeLock | ConsistencyTypeFlush => !trx_consistency_only,
        ConsistencyTypeSnapshot | ConsistencyTypeNone => true,
        _ => false,
    }
}

pub fn runSteps(d: &mut Dumper, steps: &[fn(&mut Dumper) -> Result<()>]) -> Result<()> {
    // 顺序执行初始化步骤，任何一步失败都会终止构造并把错误向上返回。
    // 每个步骤都以 `&mut Dumper` 为唯一输入，避免额外的初始化上下文对象。
    for step in steps {
        step(d)?;
    }
    Ok(())
}

pub fn initLogger(d: &mut Dumper) -> Result<()> {
    // 如果配置里已经带了 logger，就直接复用而不是重复初始化全局日志系统。
    if d.conf.Logger.is_some() {
        if let Some(l) = &d.conf.Logger {
            // 复用外部 logger 时，只需要把 tctx 切换到该 logger 即可。
            d.tctx = d.tctx.WithLogger(l.clone());
        }
        return Ok(());
    }
    // 否则根据配置动态构造 app logger，并在初始化完成后打印版本信息。
    let conf = log::Config {
        Level: if d.conf.LogLevel.is_empty() {
            "info".into()
        } else {
            d.conf.LogLevel.clone()
        },
        File: d.conf.LogFile.clone(),
        Format: d.conf.LogFormat.clone(),
        ..Default::default()
    };
    let (logger, _guard) = log::InitAppLogger(&conf).map_err(|e| errors_new(e))?;
    // guard 在当前简化路径下不外传，只要求初始化期间不报错即可。
    d.tctx = d.tctx.WithLogger(logger);
    cli::LogLongVersion(&d.tctx.L());
    Ok(())
}

pub fn createExternalStore(d: &mut Dumper) -> Result<()> {
    // 外部存储创建可能会回写配置中的 ExtStorage，因此这里要走可变副本。
    let mut conf = (*d.conf).clone_for_mutate();
    // createExternalStorage 自己负责缓存/复用已有存储对象。
    let s = conf.createExternalStorage()?;
    d.ext_storage = Some(s);
    d.conf = Arc::new(conf);
    Ok(())
}

pub fn startHTTPService(d: &mut Dumper) -> Result<()> {
    // status 地址为空时说明用户显式关闭了 HTTP / pprof 服务。
    if d.conf.StatusAddr.is_empty() {
        return Ok(());
    }
    match startDumplingServiceWithDumper(&d.tctx, &d.conf.StatusAddr, Some(d)) {
        Ok(h) => {
            d.http = Some(h);
            Ok(())
        }
        Err(e) => {
            // 非法地址属于配置错误；端口占用等运行时问题则只记 warn，不阻断导出。
            d.tctx.L().Warn(
                "start status server failed",
                [Field::string("error", e.msg.clone())],
            );
            // non-fatal for unit path when addr busy — still surface invalid
            if e.msg.contains("invalid") {
                Err(e)
            } else {
                Ok(())
            }
        }
    }
}

pub fn openSQLDB(d: &mut Dumper) -> Result<()> {
    // 使用空 DBName 的 driver config 打开主连接，后续具体查询再按需切库。
    let cfg = d.conf.GetDriverConfig("");
    // 这里不做 ping，真正的能力探测留给后续 detectServerInfo 等步骤。
    d.db = Some(openDB(&cfg)?);
    Ok(())
}

pub fn detectServerInfo(d: &mut Dumper) -> Result<()> {
    // server info 是后续自动一致性选择、表列举策略等逻辑的前置条件。
    let db = d.db.as_ref().unwrap();
    // 这里只读取版本串，不提前做更多 server capability 探针。
    let ver = SelectVersion(db)?;
    let mut conf = (*d.conf).clone_for_mutate();
    conf.ServerInfo = ParseServerInfo(&ver);
    d.conf = Arc::new(conf);
    Ok(())
}

pub fn resolveAutoConsistency(d: &mut Dumper) -> Result<()> {
    // auto 不是最终执行模式，必须在这里收敛成具体的一致性策略。
    if d.conf.Consistency != ConsistencyTypeAuto {
        return Ok(());
    }
    let mut conf = (*d.conf).clone_for_mutate();
    conf.Consistency = match conf.ServerInfo.ServerType {
        ServerType::ServerTypeTiDB => ConsistencyTypeSnapshot.to_string(),
        ServerType::ServerTypeMySQL | ServerType::ServerTypeMariaDB => {
            ConsistencyTypeFlush.to_string()
        }
        _ => ConsistencyTypeNone.to_string(),
    };
    d.conf = Arc::new(conf);
    Ok(())
}

pub fn validateResolveAutoConsistency(d: &mut Dumper) -> Result<()> {
    if d.conf.Consistency != ConsistencyTypeSnapshot && !d.conf.Snapshot.is_empty() {
        return Err(errors_new(format!(
            "can't specify --snapshot when --consistency isn't snapshot, resolved consistency: {}",
            d.conf.Consistency
        )));
    }
    if d.conf.Consistency == ConsistencyTypeSnapshot
        && d.conf.ServerInfo.ServerType != ServerType::ServerTypeTiDB
    {
        return Err(errors_new(
            "snapshot consistency is not supported for this server",
        ));
    }
    Ok(())
}

pub fn setSessionParam(d: &mut Dumper) -> Result<()> {
    // Match the manual-GC branch of Go tidbStartGCSavepointUpdateService.
    // Run before session parameters, as in Go's initialization sequence.
    if d.pd_client.is_none() && d.conf.ServerInfo.ServerType == ServerType::ServerTypeTiDB {
        // Since TiDB v5.0.0, GC lifetime is a system variable instead of a mysql.tidb row.
        d.tctx.L().Warn(
            concat!(
                "If the amount of data to dump is large (more than 60 GB or expected to take more than 10 minutes),\n",
                "consider increasing tidb_gc_life_time to prevent historical data from being collected during the dump.\n",
                "Before dumping, record the current value with `SELECT @@GLOBAL.tidb_gc_life_time;`,\n",
                "then run `SET GLOBAL tidb_gc_life_time = '720h';`.\n",
                "After dumping, restore tidb_gc_life_time to the recorded value.\n",
            ),
            [],
        );
    }
    // 默认 session 参数同样通过克隆配置再整体替换，避免直接修改 Arc 内部。
    let mut conf = (*d.conf).clone_for_mutate();
    // 这样后续真实连接在执行 SQL 前，可以统一读取这份最终参数集。
    setDefaultSessionParams(&conf.ServerInfo, &mut conf.SessionParams);
    d.conf = Arc::new(conf);
    Ok(())
}

pub fn setDefaultSessionParams(
    si: &ServerInfo,
    session_params: &mut std::collections::HashMap<String, String>,
) {
    let enable_paging_version = SemVer::new("6.2.0");
    if si.ServerType == ServerType::ServerTypeTiDB
        && si.HasTiKV
        && si
            .ServerVersion
            .as_ref()
            .is_some_and(|version| !version.LessThan(&enable_paging_version))
    {
        session_params
            .entry("tidb_enable_paging".into())
            .or_insert_with(|| "ON".into());
    }
}

pub fn getListTableTypeByConf(conf: &Config) -> listTableType {
    if conf.Consistency == ConsistencyTypeLock {
        listTableType::listTableByInfoSchema
    } else if conf.Consistency == ConsistencyTypeFlush && matchMysqlBugversion(&conf.ServerInfo) {
        listTableType::listTableByShowFullTables
    } else {
        listTableType::listTableByShowTableStatus
    }
}

pub fn prepareTableListToDump(
    tctx: &tcontext::Context,
    conf: &mut Config,
    db: &Conn,
) -> Result<()> {
    // 对外保留一个公开包装层，真正逻辑放在内部函数中便于后续扩展。
    prepareTableListToDumpInner(tctx, conf, db)
}

fn prepareTableListToDumpInner(
    tctx: &tcontext::Context,
    conf: &mut Config,
    db: &Conn,
) -> Result<()> {
    if !conf.SQL.is_empty() {
        return Ok(());
    }
    // 指定了显式表清单时，不再走数据库枚举，只对现有 Tables 做二次过滤。
    if conf.SpecifiedTables {
        // 这里假设 conf.Tables 已由 CLI 解析阶段按 `db.table` 构造完成。
        filterTables(tctx, conf);
        return Ok(());
    }
    // 常规路径：先得到库列表，再按数据库类型选择最稳妥的列举策略。
    let databases = prepareDumpingDatabases(tctx, conf, db)?;
    let list_type = getListTableTypeByConf(conf);
    // 允许列举哪些对象类型，取决于 no-views / no-sequences 两个开关。
    let mut types = vec![TableType::TableTypeBase];
    if !conf.NoViews {
        types.push(TableType::TableTypeView);
    }
    if !conf.NoSequences {
        types.push(TableType::TableTypeSequence);
    }
    conf.Tables = ListAllDatabasesTables(tctx, db, &databases, list_type, &types)?;
    // 枚举出全量对象后仍需再过一遍 filter，确保规则最终生效。
    // 这样无论是默认 filter 还是用户自定义规则，最后都落在同一个裁剪点。
    filterTables(tctx, conf);
    Ok(())
}

pub fn dumpTableMeta(
    tctx: &tcontext::Context,
    conf: &Config,
    conn: &mut BaseConn,
    db: &str,
    table: &TableInfo,
) -> Result<Box<dyn TableMeta>> {
    let projection = match conf
        .columnProjection
        .get(&(db.to_owned(), table.Name.clone()))
    {
        Some(projection) => projection.clone(),
        None if !conf.columnFilter.Filters.is_empty() => {
            return Err(errors_new(format!(
                "missing column projection for table `{}`.`{}`",
                escapeString(db),
                escapeString(&table.Name)
            )));
        }
        None => buildColumnProjection(tctx, conf, conn, db, table)?,
    };
    let mut has_implicit_row_id = false;
    if conf.ServerInfo.ServerType == ServerType::ServerTypeTiDB {
        if let Ok(has_row_id) = SelectTiDBRowID(tctx, conn, db, &table.Name) {
            has_implicit_row_id = has_row_id;
        }
    }
    let (show_create_table, show_create_view) = if conf.NoSchemas {
        (String::new(), String::new())
    } else {
        match table.Type {
            TableType::TableTypeView => ShowCreateView(tctx, conn, db, &table.Name)?,
            TableType::TableTypeSequence => (
                ShowCreateSequence(tctx, conn, db, &table.Name, conf)?,
                String::new(),
            ),
            TableType::TableTypeBase => (
                if projection.schemaSQL.is_empty() {
                    ShowCreateTable(tctx, conn, db, &table.Name)?
                } else {
                    projection.schemaSQL.clone()
                },
                String::new(),
            ),
        }
    };
    Ok(Box::new(tableMeta {
        database: db.to_string(),
        table: table.Name.clone(),
        col_types: projection.selectedTypes,
        source_col_types: projection.sourceTypes,
        selected_field: projection.selectField,
        spec_cmts: getSpecialComments(conf.ServerInfo.ServerType),
        show_create_table,
        show_create_view,
        avg_row_length: table.AvgRowLength,
        has_implicit_row_id,
    }))
}

fn getColumnTypes(
    tctx: &tcontext::Context,
    conn: &mut BaseConn,
    fields: &str,
    database: &str,
    table: &str,
) -> Result<Vec<ColumnType>> {
    let query = format!(
        "SELECT {} FROM `{}`.`{}` LIMIT 1",
        fields,
        escapeString(database),
        escapeString(table)
    );
    let col_types = std::cell::RefCell::new(Vec::new());
    conn.queryRows(
        tctx,
        |rows| {
            *col_types.borrow_mut() = rows.ColumnTypes()?;
            rows.Close()?;
            rows.Err().map_or(Ok(()), Err)
        },
        || col_types.borrow_mut().clear(),
        &query,
    )?;
    Ok(col_types.into_inner())
}

pub fn firstNonEmpty(vals: &[&str]) -> String {
    // cluster 级参数优先于通用参数时，可复用这个“取首个非空值”的小工具。
    // 它保持左到右优先级，因此调用方要先传“更专用”的配置项。
    for v in vals {
        if !v.is_empty() {
            return (*v).to_string();
        }
    }
    String::new()
}

pub fn adjustDatabaseCollation(
    _tctx: &tcontext::Context,
    collation_compatible: &str,
    origin_sql: &str,
    _charset_map: &std::collections::HashMap<String, String>,
) -> Result<String> {
    // 目前严格/宽松排序规则兼容模式在最小实现中都退化成原样返回。
    // 保留这个钩子是为了以后在 schema SQL 层真正做替换时不改调用面。
    if collation_compatible == LooseCollationCompatible {
        return Ok(origin_sql.to_string());
    }
    Ok(origin_sql.to_string())
}

pub fn adjustTableCollation(
    _tctx: &tcontext::Context,
    collation_compatible: &str,
    origin_sql: &str,
    _charset_map: &std::collections::HashMap<String, String>,
) -> Result<String> {
    // 表级 collation 调整同样预留接口，但当前不做实际 SQL 改写。
    // 与数据库级函数分开保留，是因为未来两者可能有不同的替换规则。
    if collation_compatible == LooseCollationCompatible {
        return Ok(origin_sql.to_string());
    }
    Ok(origin_sql.to_string())
}

pub struct PDSecurityOption {
    // 这三项只服务 PD / GC 控制连接，不直接影响普通 SQL DB 连接。
    // 这三项只服务 PD / GC 控制连接，不直接影响普通 SQL DB 连接。
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

pub fn pdSecurityOptionForGC(conf: &Config) -> PDSecurityOption {
    // GC 控制优先使用 cluster-ssl-*，为空时再回落到通用 Security 配置。
    // 这样用户可以只为 PD 单独指定证书，而不影响主数据库连接参数。
    PDSecurityOption {
        CAPath: firstNonEmpty(&[&conf.ClusterSSLCA, &conf.Security.CAPath]),
        CertPath: firstNonEmpty(&[&conf.ClusterSSLCert, &conf.Security.CertPath]),
        KeyPath: firstNonEmpty(&[&conf.ClusterSSLKey, &conf.Security.KeyPath]),
    }
}

pub fn parseClusterSSLFlags(ca: &str, cert: &str, key: &str) -> Result<(String, String, String)> {
    // 目前这里只做透传，未来若要扩展校验可直接挂在这里。
    Ok((ca.to_string(), cert.to_string(), key.to_string()))
}

pub fn tidbResolveKeyspaceMetaForGC(d: &mut Dumper) -> Result<()> {
    // arm64-safe 路径下把 keyspace 解析简化为轻量探测，不阻断主流程。
    // Slim arm64 path: mark keyspace resolution as no-op success unless DB scripts an error.
    // 即便查询不到 keyspace，这里也尽量给出一个 mock client 继续主流程。
    if let Some(db) = &d.db {
        if let Ok(mut rows) = db.Query("SELECT @@tidb_keyspace_name") {
            let _ = rows.Close();
        }
    }
    if d.pd_client.is_none() {
        d.pd_client = Some(PdClient::new_mock());
    }
    Ok(())
}

pub fn updateServiceSafePoint(
    tctx: &tcontext::Context,
    pd_client: &PdClient,
    ttl: i64,
    snapshot_ts: u64,
) {
    // 传统 safepoint 保护路径：为本次 dumpling 运行生成唯一 service id。
    // 唯一 id 能避免多个 dumpling 实例互相覆盖对方的 GC 保护记录。
    let id = format!(
        "{}_{}",
        dumplingServiceSafePointPrefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    tctx.L().Info(
        "generate dumpling gc safePoint id",
        [Field::string("id", id.clone())],
    );
    runGCProtectionUpdater(
        tctx,
        ttl,
        snapshot_ts,
        // update 回调负责续租 safepoint，cleanup 回调负责在退出时删除它。
        |protect_ts, _retry| pd_client.UpdateServiceGCSafePoint(&id, ttl, protect_ts),
        || {
            let _ = pd_client.UpdateServiceGCSafePoint(&id, 0, 0);
        },
    );
}

pub fn updateKeyspaceGCBarrier(
    tctx: &tcontext::Context,
    pd_client: &PdClient,
    keyspace_id: u32,
    ttl: i64,
    snapshot_ts: u64,
) {
    // keyspace GC barrier 与普通 service safepoint 类似，但走的是 keyspace 专用 API。
    // 相比通用 safepoint，这里还要额外携带 keyspace_id 做路由。
    let barrier_id = format!(
        "{}_{}",
        dumplingServiceSafePointPrefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    tctx.L().Info(
        "generate dumpling gc barrier id",
        [
            Field::string("id", barrier_id.clone()),
            Field::string("keyspaceID", keyspace_id.to_string()),
        ],
    );
    let gc = pd_client.GetGCStatesClient(keyspace_id);
    let ttl_dur = Duration::from_secs(ttl as u64);
    runGCProtectionUpdater(
        tctx,
        ttl,
        snapshot_ts,
        |protect_ts, _retry| {
            gc.SetGCBarrier(&barrier_id, protect_ts, ttl_dur)
                .map(|_| 0u64)
        },
        || {
            let _ = gc.DeleteGCBarrier(&barrier_id);
        },
    );
}

fn runGCProtectionUpdater<U, C>(
    tctx: &tcontext::Context,
    ttl: i64,
    snapshot_ts: u64,
    mut update: U,
    cleanup: C,
) where
    U: FnMut(u64, i32) -> Result<u64>,
    C: FnOnce(),
{
    // updater 每隔 ttl/2 续租一次保护点，这是与 Go 相同的基本策略。
    // ttl 很小时也至少保证 1 秒间隔，避免出现 0 秒 busy loop。
    let update_interval = Duration::from_secs((ttl / 2).max(1) as u64);
    let mut protect_ts = snapshot_ts;
    // protect_ts 通常比 snapshot_ts 小 1，避免把 GC 保护点推进到快照本身。
    if protect_ts > 0 {
        protect_ts -= 1;
    }
    loop {
        // 一旦外层上下文取消，就先清理再退出，避免留下脏的 GC 保护状态。
        if tctx.Done() {
            cleanup();
            return;
        }
        for retry_cnt in 0..11 {
            // 每个续租周期内部允许有限次快速重试，吸收瞬时 PD 错误。
            if tctx.Done() {
                cleanup();
                return;
            }
            match update(protect_ts, retry_cnt) {
                Ok(_) => break,
                Err(_) => {
                    // 当前测试/简化实现只做短暂 sleep，不引入更复杂 backoff。
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        let wait_start = Instant::now();
        while wait_start.elapsed() < update_interval {
            if tctx.Done() {
                cleanup();
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if tctx.Done() {
            cleanup();
            return;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct columnProjection {
    pub sourceTypes: Vec<ColumnType>,
    pub selectedTypes: Vec<ColumnType>,
    pub selectField: String,
    pub schemaSQL: String,
}

pub fn prepareColumnProjection(
    tctx: &tcontext::Context,
    conf: &mut Config,
    conn: &mut BaseConn,
) -> Result<()> {
    conf.columnProjection = HashMap::with_capacity(calculateTableCount(&conf.Tables) as usize);
    let mut any_filtered_columns = false;
    for (db, tables) in &conf.Tables {
        for table in tables {
            let projection = buildColumnProjection(tctx, conf, conn, db, table)?;
            any_filtered_columns |= projection.sourceTypes.len() != projection.selectedTypes.len();
            conf.columnProjection
                .insert((db.clone(), table.Name.clone()), projection);
        }
    }
    if conf.NoSchemas || !any_filtered_columns {
        return Ok(());
    }
    if conf
        .Tables
        .values()
        .flatten()
        .any(|table| table.Type == TableType::TableTypeView)
    {
        return Err(errors_new(
            "schema output with an active column filter is not supported when the dump includes views",
        ));
    }
    let mut parser = schema_projection::new_schema_parser(&conf.SessionParams)?;
    let mut schemas = schema_projection::ProjectedTableSchemas::new();
    for (db, tables) in &conf.Tables {
        for table in tables {
            if table.Type != TableType::TableTypeBase {
                continue;
            }
            let key = (db.clone(), table.Name.clone());
            let mut projection = conf.columnProjection.get(&key).unwrap().clone();
            let original = ShowCreateTable(tctx, conn, db, &table.Name)?;
            let filtered = projection.sourceTypes.len() != projection.selectedTypes.len();
            let schema = if filtered {
                schema_projection::build_projected_table_schema(
                    &mut parser,
                    &original,
                    &projection
                        .selectedTypes
                        .iter()
                        .map(|column| column.Name().to_owned())
                        .collect::<Vec<_>>(),
                )
            } else {
                schema_projection::parse_table_schema(&mut parser, &original)
            }
            .map_err(|err| {
                errors_new(format!(
                    "failed to analyze schema projection for table `{}`.`{}`: {}",
                    escapeString(db),
                    escapeString(&table.Name),
                    err.msg
                ))
            })?;
            projection.schemaSQL = if filtered {
                schema_projection::restore_projected_schema(&schema.create_table).map_err(
                    |err| {
                        errors_new(format!(
                            "failed to restore schema projection for table `{}`.`{}`: {}",
                            escapeString(db),
                            escapeString(&table.Name),
                            err.msg
                        ))
                    },
                )?
            } else {
                original
            };
            conf.columnProjection.insert(key.clone(), projection);
            schemas.insert(key, schema);
        }
    }
    // Every parent must exist before FK validation; HashMap iteration order must
    // not turn an unbuilt parent into an apparently external table.
    for (db, tables) in &conf.Tables {
        for table in tables {
            if table.Type != TableType::TableTypeBase {
                continue;
            }
            let schema = schemas.get(&(db.clone(), table.Name.clone())).unwrap();
            schema_projection::validate_foreign_key_parents(db, schema, &schemas).map_err(
                |err| {
                    errors_new(format!(
                        "failed to validate schema projection for table `{}`.`{}`: {}",
                        escapeString(db),
                        escapeString(&table.Name),
                        err.msg
                    ))
                },
            )?;
        }
    }
    Ok(())
}

pub fn buildColumnProjection(
    tctx: &tcontext::Context,
    conf: &Config,
    conn: &mut BaseConn,
    database: &str,
    table: &TableInfo,
) -> Result<columnProjection> {
    if table.Type != TableType::TableTypeBase {
        return Ok(columnProjection::default());
    }
    let (source, generated) = getWritableColumnNames(tctx, conn, database, &table.Name)?;
    let (selected, indexes) = conf
        .columnFilter
        .applyToColumns(database, &table.Name, &source)?;
    if selected.is_empty() {
        return Ok(columnProjection::default());
    }
    let selectField = if !generated && source.len() == selected.len() && !conf.CompleteInsert {
        "*".to_owned()
    } else {
        columnNamesToSelectFields(&selected).join(",")
    };
    let sourceTypes = getColumnTypes(
        tctx,
        conn,
        &columnNamesToSelectFields(&source).join(","),
        database,
        &table.Name,
    )?;
    let selectedTypes = indexes
        .into_iter()
        .map(|i| sourceTypes[i].clone())
        .collect();
    Ok(columnProjection {
        sourceTypes,
        selectedTypes,
        selectField,
        schemaSQL: String::new(),
    })
}

pub fn columnNamesToSelectFields(columns: &[String]) -> Vec<String> {
    columns.iter().map(|c| wrapBackTicks(c)).collect()
}

pub fn tableSourceColumnNames(meta: &dyn TableMeta) -> Vec<String> {
    meta.sourceColumnNames()
}
pub fn tableSourceColumnTypes(meta: &dyn TableMeta) -> Vec<String> {
    meta.sourceColumnTypes()
}

pub fn GetPrimaryKeyAndColumnTypes(
    tctx: &tcontext::Context,
    conn: &mut BaseConn,
    meta: &dyn TableMeta,
) -> Result<(Vec<String>, Vec<String>)> {
    let names = GetPrimaryKeyColumns(tctx, conn, meta.DatabaseName(), meta.TableName())?;
    let types = string2Map(&tableSourceColumnNames(meta), &tableSourceColumnTypes(meta));
    let column_types = names
        .iter()
        .map(|n| types.get(n).cloned().unwrap_or_default())
        .collect();
    Ok((names, column_types))
}
