// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 本文件负责把 dumpling 的“一致性级别”字符串映射成真正的控制器实现，
// 并封装 flush lock、锁表导出和 snapshot/none 这些模式的建链、清理与健康检查逻辑。
// Rust 版本继续保留 Go 侧的核心契约：
// 不同数据库类型支持的一致性模式不同，锁表路径可能需要重试并回写被屏蔽的表集合。

// 一致性模式字符串与 Go dumpling/export 的 CLI/配置常量完全同值。
pub const ConsistencyTypeAuto: &str = "auto";
pub const ConsistencyTypeFlush: &str = "flush";
pub const ConsistencyTypeLock: &str = "lock";
pub const ConsistencyTypeSnapshot: &str = "snapshot";
pub const ConsistencyTypeNone: &str = "none";

pub fn errTiDBDisableTableLock() -> Error {
    // TiDB 未启用 table lock 时，lock consistency 必须明确失败而不是静默降级。
    errors_new(
        "try to apply lock consistency on TiDB but it doesn't enable table lock. please set enable-table-lock=true in tidb server config",
    )
}

// ConsistencyController 抽象 setup / teardown / ping 三个生命周期动作。
// 这样 dumper 上层不必关心底层到底是 flush、lock 还是 no-op。
pub trait ConsistencyController: Send {
    fn Setup(&mut self, tctx: &tcontext::Context) -> Result<()>;
    fn TearDown(&mut self) -> Result<()>;
    fn PingContext(&self) -> Result<()>;
}

pub fn NewConsistencyController(
    conf: &Config,
    session: &DB,
) -> Result<Box<dyn ConsistencyController>> {
    // 控制器统一持有独立连接，避免和普通导出查询共用一个会话。
    // 这样 teardown 时也能独立解锁/关闭，不会误伤导出主连接。
    let conn = session.Conn()?;
    match conf.Consistency.as_str() {
        // flush 模式依赖全局读锁，只适合非 TiDB 数据库。
        ConsistencyTypeFlush => Ok(Box::new(ConsistencyFlushTableWithReadLock {
            server_type: conf.ServerInfo.ServerType,
            conn: Some(conn),
        })),
        ConsistencyTypeLock => Ok(Box::new(ConsistencyLockDumpingTables {
            conn: Some(conn),
            empty_lock_sql: false,
            specified_tables: conf.SpecifiedTables,
            server_type: conf.ServerInfo.ServerType,
            conf: conf.clone_for_mutate(),
        })),
        ConsistencyTypeSnapshot => {
            // snapshot 目前只对 TiDB 开放，其他数据库保持显式报错。
            if conf.ServerInfo.ServerType != ServerType::ServerTypeTiDB {
                return Err(errors_new(
                    "snapshot consistency is not supported for this server",
                ));
            }
            Ok(Box::new(ConsistencyNone {}))
        }
        // none 直接退化成 no-op controller，保留统一接口。
        ConsistencyTypeNone => Ok(Box::new(ConsistencyNone {})),
        other => Err(errors_errorf(format!("invalid consistency option {other}"))),
    }
}

pub struct ConsistencyNone;
impl ConsistencyController for ConsistencyNone {
    // none 模式三个阶段都应无副作用地成功返回。
    fn Setup(&mut self, _: &tcontext::Context) -> Result<()> {
        Ok(())
    }
    fn TearDown(&mut self) -> Result<()> {
        Ok(())
    }
    fn PingContext(&self) -> Result<()> {
        Ok(())
    }
}

// 这个控制器对应传统 MySQL 的 `FLUSH TABLES WITH READ LOCK` 路径。
pub struct ConsistencyFlushTableWithReadLock {
    pub server_type: ServerType,
    pub conn: Option<Conn>,
}
impl ConsistencyController for ConsistencyFlushTableWithReadLock {
    fn Setup(&mut self, tctx: &tcontext::Context) -> Result<()> {
        // TiDB 不允许用 FTWRL 保证一致性，因此这里要直接拒绝。
        if self.server_type == ServerType::ServerTypeTiDB {
            return Err(errors_new(
                "'flush table with read lock' cannot be used to ensure the consistency in TiDB",
            ));
        }
        FlushTableWithReadLock(tctx, self.conn.as_ref().unwrap())
    }
    fn TearDown(&mut self) -> Result<()> {
        // TearDown 既负责 UNLOCK TABLES，也负责把专用连接关闭掉。
        let Some(conn) = self.conn.take() else {
            return Ok(());
        };
        let res = UnlockTables(&conn);
        let _ = conn.Close();
        res
    }
    fn PingContext(&self) -> Result<()> {
        // 连接已被关闭时，显式返回错误比假装成功更利于上层感知状态。
        match &self.conn {
            None => Err(errors_new("consistency connection has already been closed")),
            Some(c) => c.PingContext(),
        }
    }
}

// lock controller 会按待导出的表集合生成 LOCK TABLES 语句并在必要时重试。
pub struct ConsistencyLockDumpingTables {
    pub conn: Option<Conn>,
    // empty_lock_sql 表示当前根本没有可锁对象，此时 ping/teardown 走轻量路径。
    pub empty_lock_sql: bool,
    pub specified_tables: bool,
    pub server_type: ServerType,
    pub conf: Config,
}

pub fn consistency_lock_setup(
    c: &mut ConsistencyLockDumpingTables,
    tctx: &tcontext::Context,
    conf: &mut Config,
) -> Result<()> {
    // TiDB 下的 lock consistency 先检查全局开关，否则后续锁表一定失败。
    if conf.ServerInfo.ServerType == ServerType::ServerTypeTiDB {
        let enable = CheckTiDBEnableTableLock(c.conn.as_ref().unwrap())?;
        if !enable {
            return Err(errTiDBDisableTableLock());
        }
    }
    // Go 版本让重试闭包和 backoffer 共享同一个 blockList。Rust 的 backoffer
    // 拥有该 map，因此每一轮都必须直接从它读取，才能在 1146 后排除缺失表。
    let mut backoffer = newLockTablesBackoffer(tctx.clone(), HashMap::new(), conf);
    loop {
        if tctx.Done() {
            return Err(errors_new("context canceled"));
        }

        let lock_sql = buildLockTablesSQL(&conf.Tables, &backoffer.block_list);
        let result = if lock_sql.is_empty() {
            c.empty_lock_sql = true;
            if let Some(conn) = c.conn.take() {
                let _ = conn.Close();
            }
            Ok(())
        } else {
            c.conn.as_ref().unwrap().ExecContext(&lock_sql).map(|_| ())
        };

        match result {
            Ok(()) => {
                if !backoffer.block_list.is_empty() {
                    filterTablesFunc(tctx, conf, |db, tbl| {
                        !backoffer
                            .block_list
                            .get(db)
                            .map(|tables| tables.contains_key(tbl))
                            .unwrap_or(false)
                    });
                }
                return Ok(());
            }
            Err(mut err) => {
                // 保留 mysql 根因供 lockTablesBackoffer 识别 1146；只扩充展示文本。
                err.msg = format!("sql: {lock_sql}: {}", err.msg);
                if backoffer.RemainingAttempts() <= 0 {
                    return Err(err);
                }
                let _ = backoffer.NextBackoff(&err);
                if backoffer.RemainingAttempts() <= 0 {
                    return Err(err);
                }
            }
        }
    }
}

impl ConsistencyController for ConsistencyLockDumpingTables {
    fn Setup(&mut self, tctx: &tcontext::Context) -> Result<()> {
        // setup 过程会修改 conf，因此先复制一份可变配置副本。
        let mut conf = self.conf.clone_for_mutate();
        // Move conn into temporary controller path via consistency_lock_setup.
        let mut tmp = ConsistencyLockDumpingTables {
            conn: self.conn.take(),
            empty_lock_sql: false,
            specified_tables: self.specified_tables,
            server_type: self.server_type,
            conf: conf.clone_for_mutate(),
        };
        let res = consistency_lock_setup(&mut tmp, tctx, &mut conf);
        self.conn = tmp.conn;
        self.empty_lock_sql = tmp.empty_lock_sql;
        self.conf = conf;
        res
    }
    fn TearDown(&mut self) -> Result<()> {
        // lock 模式 teardown 与 flush 模式一样：解锁并关闭专用连接。
        let Some(conn) = self.conn.take() else {
            return Ok(());
        };
        let res = UnlockTables(&conn);
        let _ = conn.Close();
        res
    }
    fn PingContext(&self) -> Result<()> {
        // 空锁表 SQL 说明无需真正持锁，此时 ping 直接成功即可。
        if self.empty_lock_sql {
            return Ok(());
        }
        match &self.conn {
            None => Err(errors_new("consistency connection has already been closed")),
            Some(c) => c.PingContext(),
        }
    }
}

// snapshot 查询结果里快照值位于第二列，这个索引常量供上层 SQL 解析复用。
pub const snapshotFieldIndex: usize = 1;
