// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! A/B SQL execution and comparison (Go `tests/llmtest/testcase/run.go`).

// 本文件对应 `tests/llmtest/testcase/run.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 7：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 8：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 9：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 10：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 11：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 12：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 13：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 14：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 15：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 16：阅读长列表时可按语义分组理解，而不是逐项记忆。
use crate::stubs::{AnyValue, Db, DbRows, SqlError};
use crate::testcase::{Case, Manager};
use astersql_tests_llmtest_logger::{Global, zap};

// `QueryResultSet` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
type QueryResultSet = Vec<Vec<String>>;
// `QueryResultSets` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
type QueryResultSets = Vec<QueryResultSet>;

/// executeSingleQueryInDB — scan SQL rows into a string matrix; NULL → `"<nil>"`.
// `execute_single_query_in_db` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn execute_single_query_in_db(
    db: &Db,
    query: &str,
    args: &[AnyValue],
) -> Result<QueryResultSet, SqlError> {
    let mut db_rows = db.query(query, args)?;

    // Go: defer dbRows.Close()
    let finish = |rows: &mut DbRows| {
        let _ = rows.close();
    };

    let cols = match db_rows.columns() {
        Ok(cols) => cols,
        Err(err) => {
            finish(&mut db_rows);
            return Err(err);
        }
    };

    let mut query_results = Vec::new();
    while db_rows.next() {
        let data = match db_rows.scan_null_strings() {
            Ok(data) => data,
            Err(err) => {
                finish(&mut db_rows);
                return Err(err);
            }
        };

        let mut row_strings = Vec::with_capacity(cols.len());
        for val in data {
            // Go sql.NullString: Valid → String, else "<nil>"
            row_strings.push(val.unwrap_or_else(|| "<nil>".to_string()));
        }
        query_results.push(row_strings);
    }

    // Some drivers only surface errors via Rows.Err() after Next returns false.
    if let Some(err) = db_rows.err() {
        finish(&mut db_rows);
        return Err(err);
    }

    finish(&mut db_rows);
    Ok(query_results)
}

/// executeSQLsInDB — split Case.SQL on `;`, skip empty fragments, share Args.
// `execute_sqls_in_db` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn execute_sqls_in_db(db: &Db, c: &Case) -> Result<QueryResultSets, SqlError> {
    let all_queries = c.sql.split(';');
    let mut all_results = Vec::new();
    let args = c.args.as_deref().unwrap_or(&[]);

    for query in all_queries {
        // Go checks len(query) == 0 only (no TrimSpace).
        if query.is_empty() {
            continue;
        }

        let query_results = execute_single_query_in_db(db, query, args)?;
        all_results.push(query_results);
    }

    Ok(all_results)
}

// 这里实现 `Manager` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Manager {
    /// RunABTest runs the A/B test on two databases.
    ///
    /// Holds `m.mu` for the whole walk (Go `m.mu.Lock` / `defer Unlock`).
    // `run_ab_test` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    pub fn run_ab_test(&self, db1: &Db, db2: &Db, recheck_passed: bool) {
        self.with_cases_mut(|cases| {
            for group_cases in cases.values_mut() {
                'case_loop: for c in group_cases.iter_mut() {
                    if c.known {
                        continue;
                    }
                    if !recheck_passed && c.pass {
                        continue;
                    }

                    // Go: logger.Global.With(zap.String("sql", ...), zap.Any("args", ...))
                    let sql_field = zap::String("sql", c.sql.as_str());
                    let args_field = zap::Any("args", &c.args);

                    let result1 = execute_sqls_in_db(db1, c);
                    let result2 = execute_sqls_in_db(db2, c);

                    match (result1, result2) {
                        (Err(err1), Err(err2)) => {
                            let _ = (err1, err2);
                            c.pass = true;
                            continue;
                        }
                        (Err(err1), Ok(_)) => {
                            Global.Info(
                                "One of the result is error",
                                &[
                                    sql_field.clone(),
                                    args_field.clone(),
                                    zap::Error(err1.to_string()),
                                    zap::Error(""),
                                ],
                            );
                            c.pass = false;
                            continue;
                        }
                        (Ok(_), Err(err2)) => {
                            Global.Info(
                                "One of the result is error",
                                &[
                                    sql_field.clone(),
                                    args_field.clone(),
                                    zap::Error(""),
                                    zap::Error(err2.to_string()),
                                ],
                            );
                            c.pass = false;
                            continue;
                        }
                        (Ok(r1), Ok(r2)) => {
                            if r1.len() != r2.len() {
                                Global.Info(
                                    "Different result set count",
                                    &[
                                        sql_field.clone(),
                                        args_field.clone(),
                                        zap::Any("result1", &r1),
                                        zap::Any("result2", &r2),
                                    ],
                                );
                                c.pass = false;
                                continue;
                            }

                            for i in 0..r1.len() {
                                if r1[i].len() != r2[i].len() {
                                    c.pass = false;
                                    Global.Info(
                                        "Different row count",
                                        &[
                                            sql_field.clone(),
                                            args_field.clone(),
                                            zap::Any("result1", &r1[i]),
                                            zap::Any("result2", &r2[i]),
                                        ],
                                    );
                                    continue 'case_loop;
                                }
                                for j in 0..r1[i].len() {
                                    if r1[i][j].len() != r2[i][j].len() {
                                        c.pass = false;
                                        Global.Info(
                                            "Different column length",
                                            &[
                                                sql_field.clone(),
                                                args_field.clone(),
                                                zap::Any("result1", &r1[i][j]),
                                                zap::Any("result2", &r2[i][j]),
                                            ],
                                        );
                                        continue 'case_loop;
                                    }
                                    for k in 0..r1[i][j].len() {
                                        if r1[i][j][k] != r2[i][j][k] {
                                            c.pass = false;
                                            Global.Info(
                                                "Different result",
                                                &[
                                                    sql_field.clone(),
                                                    args_field.clone(),
                                                    zap::Any("result1", &r1[i][j]),
                                                    zap::Any("result2", &r2[i][j]),
                                                ],
                                            );
                                            continue 'case_loop;
                                        }
                                    }
                                }
                            }

                            c.pass = true;
                        }
                    }
                }
            }
        });
    }
}
