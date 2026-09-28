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
        mut reset: R,
        query: &str,
    ) -> Result<()>
    where
        F: FnMut(&mut Rows) -> Result<()>,
        R: FnMut(),
    {
        // retry_time 从 1 开始计数，便于日志直接反映当前是第几次尝试。
        let mut retry_time = 0;
        let done = tctx.Done();
        let result = WithRetry(
            done,
            || {
                retry_time += 1;
                if retry_time > 1 {
                    // 首次直接使用现有连接，后续重试才尝试重建连接。
                    // 避免每次执行前都无意义重连，只有确认前一轮失败后才付出代价。
                    if let Some(rebuild) = &self.rebuildConnFn {
                        let old = self.DBConn.as_ref().unwrap().clone();
                        self.DBConn = Some(rebuild(&old, false)?);
                    }
                }
                let conn = self.DBConn.as_ref().unwrap();
                match simpleQueryWithArgs(tctx, conn, &mut handle_one_row, query) {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        // 失败时记录 SQL 与重试次数，便于排查是否是瞬时连接问题。
                        tctx.L().Info(
                            "cannot execute query",
                            [
                                Field::string("retryTime", (retry_time as i64).to_string()),
                                Field::string("sql", query.to_string()),
                                Field::string("error", err.msg.clone()),
                            ],
                        );
                        // reset 由调用方提供，用来清理已经部分累计的查询状态。
                        // 没有这一步的话，下一轮重试成功后可能把旧结果重复追加进去。
                        reset();
                        Err(err)
                    }
                }
            },
            self.backOffer.as_mut(),
        );
        // 无论成功还是失败，都要把 backoff 状态清空，避免污染下一条 SQL。
        // 同一个 BaseConn 往往会连续执行多条语句，因此 reset 是必须的。
        self.backOffer.Reset();
        result
    }

    pub fn QuerySQLWithColumns(
        &mut self,
        tctx: &tcontext::Context,
        columns: &[&str],
        query: &str,
    ) -> Result<Vec<Vec<String>>> {
        let mut retry_time = 0;
        // results 放在闭包外，便于重试成功后把最终结果带出。
        // 同时这也要求失败时显式 clear，不能让半截结果泄露给调用方。
        let mut results = Vec::new();
        let done = tctx.Done();
        let result = WithRetry(
            done,
            || {
                retry_time += 1;
                if retry_time > 1 {
                    // 重试时的重建失败也要写日志，因为它会直接阻断后续查询。
                    if let Some(rebuild) = &self.rebuildConnFn {
                        let old = self.DBConn.as_ref().unwrap().clone();
                        match rebuild(&old, false) {
                            Ok(c) => self.DBConn = Some(c),
                            Err(err) => {
                                // 重建阶段失败说明连“重新拿到可用连接”都做不到，应尽快终止。
                                tctx.L().Warn(
                                    "rebuild connection failed",
                                    [Field::string("error", err.msg.clone())],
                                );
                                return Err(err);
                            }
                        }
                    }
                }
                let conn = self.DBConn.as_ref().unwrap();
                let mut rows = match conn.QueryContext(query) {
                    Ok(r) => r,
                    Err(err) => {
                        // QueryContext 失败时直接包装 sql 文本，方便日志和错误链一起看。
                        tctx.L().Info(
                            "cannot execute query",
                            [
                                Field::string("retryTime", (retry_time as i64).to_string()),
                                Field::string("sql", query.to_string()),
                                Field::string("error", err.msg.clone()),
                            ],
                        );
                        return Err(errors_annotatef(err, format!("sql: {query}")));
                    }
                };
                match GetSpecifiedColumnValuesAndClose(&mut rows, columns) {
                    Ok(r) => {
                        // 只有完整读取并关闭 rows 成功后，才更新结果集。
                        // 这能保证返回值始终来自同一轮成功尝试。
                        results = r;
                        Ok(())
                    }
                    Err(err) => {
                        tctx.L().Info(
                            "cannot execute query",
                            [
                                Field::string("retryTime", (retry_time as i64).to_string()),
                                Field::string("sql", query.to_string()),
                                Field::string("error", err.msg.clone()),
                            ],
                        );
                        // 失败后清空部分结果，防止调用方误拿到半截数据。
                        results.clear();
                        Err(errors_annotatef(err, format!("sql: {query}")))
                    }
                }
            },
            self.backOffer.as_mut(),
        );
        self.backOffer.Reset();
        result?;
        Ok(results)
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
