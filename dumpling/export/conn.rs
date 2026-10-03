// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件把“执行 SQL + 失败重试 + 必要时重建连接”封装成统一抽象，
// 避免上层导出逻辑在每个查询点都重复处理同样的恢复流程。
// 三个公开方法分别面向逐行查询、按列取值查询和执行型 SQL，
// 但都共享同一套 backoff 与连接重建语义。
// 这里不决定“哪些错误值得重试”，而是把判断委托给 backoff 与上层回调。

// BaseConn 是导出路径上可重试 SQL 的最小连接单元，对应 Go 的同名结构。
pub struct BaseConn {
    // 当前正在使用的数据库连接；重试成功后可能会被新的连接替换。
    pub DBConn: Option<Conn>,
    // backOffer 记录本轮 SQL 调用的重试状态，每次调用结束后都会 reset。
    pub backOffer: Box<dyn backOfferResettable>,
    // 可选的重建连接函数；关闭重试时直接设为 None，避免误触发。
    pub rebuildConnFn: Option<Box<dyn Fn(&Conn, bool) -> Result<Conn> + Send>>,
}

pub fn newBaseConn(
    conn: Conn,
    should_retry: bool,
    rebuild: Option<Box<dyn Fn(&Conn, bool) -> Result<Conn> + Send>>,
) -> BaseConn {
    BaseConn {
        DBConn: Some(conn),
        backOffer: newRebuildConnBackOffer(should_retry),
        // 只有允许重试时才保留重建函数，明确区分“失败即返回”和“失败可恢复”。
        // 这样调用方只看构造参数，就能知道这个连接是否具备自愈能力。
        rebuildConnFn: if should_retry { rebuild } else { None },
    }
}

impl BaseConn {
    pub fn QuerySQL<F, R>(
        &mut self,
        tctx: &tcontext::Context,
        mut handle_one_row: F,
        reset: R,
        query: &str,
    ) -> Result<()>
    where
        F: FnMut(&mut Rows) -> Result<()>,
        R: FnMut(),
    {
        self.queryRows(
            tctx,
            |rows| {
                while rows.Next() {
                    handle_one_row(rows)?;
                }
                Ok(())
            },
            reset,
            query,
        )
    }

    pub fn queryRows<F, R>(
        &mut self,
        tctx: &tcontext::Context,
        mut handle_rows: F,
        mut reset: R,
        query: &str,
    ) -> Result<()>
    where
        F: FnMut(&mut Rows) -> Result<()>,
        R: FnMut(),
    {
        let mut retry_time = 0;
        let result = WithRetry(
            tctx.Done(),
            || {
                retry_time += 1;
                if retry_time > 1 {
                    if let Some(rebuild) = &self.rebuildConnFn {
                        let old = self.DBConn.as_ref().unwrap().clone();
                        self.DBConn = Some(rebuild(&old, false)?);
                    }
                }
                let attempt =
                    self.DBConn
                        .as_ref()
                        .unwrap()
                        .QueryContext(query)
                        .and_then(|mut rows| {
                            let result =
                                handle_rows(&mut rows).and_then(|_| rows.Err().map_or(Ok(()), Err));
                            let _ = rows.Close();
                            result
                        });
                if let Err(err) = attempt {
                    tctx.L().Info(
                        "cannot execute query",
                        [
                            Field::string("retryTime", retry_time.to_string()),
                            Field::string("sql", query),
                            Field::string("error", err.msg.clone()),
                        ],
                    );
                    reset();
                    return Err(errors_annotatef(err, format!("sql: {query}, args: []")));
                }
                Ok(())
            },
            self.backOffer.as_mut(),
        );
        self.backOffer.Reset();
        result
    }

    pub fn QuerySQLWithColumns(
        &mut self,
        tctx: &tcontext::Context,
        columns: &[&str],
        query: &str,
    ) -> Result<Vec<Vec<String>>> {
        let results = std::cell::RefCell::new(Vec::new());
        self.queryRows(
            tctx,
            |rows| {
                *results.borrow_mut() = GetSpecifiedColumnValuesAndClose(rows, columns)?;
                Ok(())
            },
            || results.borrow_mut().clear(),
            query,
        )?;
        Ok(results.into_inner())
    }

    pub fn ExecSQL<F>(
        &mut self,
        tctx: &tcontext::Context,
        mut can_retry: F,
        query: &str,
    ) -> Result<()>
    where
        F: FnMut(&SqlResult, Option<&Error>) -> Result<()>,
    {
        let mut retry_time = 0;
        let done = tctx.Done();
        let result = WithRetry(
            done,
            || {
                retry_time += 1;
                if retry_time > 1 {
                    // 执行型 SQL 的重试同样遵守“第二轮开始先重建连接”的规则。
                    // 查询和执行走同一恢复策略，便于维护者推理连接生命周期。
                    if let Some(rebuild) = &self.rebuildConnFn {
                        let old = self.DBConn.as_ref().unwrap().clone();
                        self.DBConn = Some(rebuild(&old, false)?);
                    }
                }
                let conn = self.DBConn.as_ref().unwrap();
                let callback_result = match conn.ExecContext(query) {
                    // 与 Go 一致：底层执行成功或失败都只调用一次 can_retry，
                    // 并完全以回调返回值决定本轮成功或交给 WithRetry 重试。
                    Ok(res) => can_retry(&res, None),
                    Err(exec_err) => can_retry(&SqlResult::default(), Some(&exec_err)),
                };
                match callback_result {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        tctx.L().Info(
                            "cannot execute query",
                            [
                                Field::string("retryTime", (retry_time as i64).to_string()),
                                Field::string("sql", query.to_string()),
                                Field::string("error", err.msg.clone()),
                            ],
                        );
                        Err(err)
                    }
                }
            },
            self.backOffer.as_mut(),
        );
        self.backOffer.Reset();
        result
    }
}
