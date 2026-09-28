// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.
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

//! S3 dumpling test import helper — matches `dumpling/tests/s3/import.go`.
//!
//! Opens a MySQL DSN (stubbed), creates a table, and concurrently inserts fixed
//! rows with an errgroup + worker-token channel.
// S3 集成测试数据导入逻辑，对齐 Go import.go：开 DSN、建表、errgroup 并发批量 INSERT。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::stubs::{
    self, CancelFunc, Context, Error, Flags, Group, OpenFn, RecordingDb, Result, Trace,
};

// cobra 短/长 flag 名，与 Go rootCmd 注册一致。
pub const FLAG_DATABASE: &str = "database";
pub const FLAG_TABLE: &str = "table";
pub const FLAG_PORT: &str = "port";
pub const FLAG_WORKER: &str = "worker";

/// Go runs 500 concurrent insert batches.
// Go 默认 500 个并发 insert 批次（errgroup 任务数）。
pub const DEFAULT_BATCHES: usize = 500;

/// Each INSERT statement contains 10000 value tuples (`i := 1; i < 10000` plus the seed).
// 每条 INSERT 含 10000 个 value 元组，与 Go 循环 `i < 10000` 加首值一致。
pub const INSERT_ROW_COUNT: usize = 10000;

/// Default flags matching Go cobra registration (TiDB port 4000).
// 默认 flag：database=s3, table=t, port=4000（TiDB harness）, worker=16。
pub fn default_flags() -> Flags {
    Flags::default()
}

/// Build DSN matching Go:
/// `root:@tcp(127.0.0.1:PORT)/DATABASE?charset=utf8mb4`
// 构造 MySQL DSN，host:port 经 JoinHostPort 与 Go net.JoinHostPort 对齐。
pub fn build_dsn(database: &str, port: isize) -> String {
    let host_port = stubs::JoinHostPort("127.0.0.1", &port.to_string());
    format!("root:@tcp({host_port})/{database}?charset=utf8mb4")
}

/// CREATE TABLE template matching Go `tableTemp`.
// CREATE TABLE 模板，%s 替换为表名。
pub fn build_create_table_sql(table: &str) -> String {
    let table_temp = "CREATE TABLE IF NOT EXISTS %s (\n\t   a VARCHAR(11)\n)";
    table_temp.replace("%s", table)
}

/// Build the multi-value INSERT matching Go (`10000` tuples of `'aaaaaaaaaa'`).
// 构造多值 INSERT，重复 ('aaaaaaaaaa') 共 INSERT_ROW_COUNT 次。
pub fn build_insert_query(table: &str) -> String {
    let mut query = format!("insert into {table} values('aaaaaaaaaa')");
    for _ in 1..INSERT_ROW_COUNT {
        query.push_str(",('aaaaaaaaaa')");
    }
    query
}

/// Count value tuples in an insert query (for parity assertions).
// parity 辅助：统计 INSERT 中 value 元组个数。
pub fn count_insert_values(query: &str) -> usize {
    // Each tuple is ('aaaaaaaaaa') — count occurrences.
    // 固定字面量元组，与 build_insert_query 生成规则绑定。
    let needle = "('aaaaaaaaaa')";
    query.match_indices(needle).count()
}

/// Go `rootCmd.RunE` body after flags are known.
///
/// `batches` is 500 in Go; tests may pass a smaller value for error-path coverage
/// without deadlocking on the worker token channel (Go also risks that when
/// failures do not return tokens).
// Go rootCmd.RunE 主体：先 Exec 建表，再 worker 令牌 channel 限流 errgroup 并发 ExecContext。
pub fn run_import_with_db(flags: &Flags, db: RecordingDb, batches: usize) -> Result<()> {
    let create_sql = build_create_table_sql(&flags.table);
    db.Exec(&create_sql).map_err(Trace)?;

    let query = build_insert_query(&flags.table);
    // Go's make(chan struct{}, worker) panics when worker is negative.
    assert!(flags.worker >= 0, "makechan: size out of range");
    let worker = flags.worker as usize;

    // ch := make(chan struct{}, worker); fill with worker tokens.
    // 有界 sync_channel 模拟 Go buffered channel，预填 worker 个令牌。
    let (tx, rx) = mpsc::sync_channel::<()>(worker.max(1));
    for _ in 0..worker {
        let _ = tx.send(());
    }
    // Keep sender alive across workers so receives do not see disconnect as EOF.
    // 保持 sender 存活，避免 worker 归还令牌时 channel 已关闭。
    let tx = Arc::new(Mutex::new(Some(tx)));
    let rx = Arc::new(Mutex::new(rx));

    let (parent_ctx, cancel) = stubs::WithCancel(stubs::Background());
    // defer cancel()
    // defer cancel：Drop 时取消 parent context。
    struct CancelOnDrop(CancelFunc);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.call();
        }
    }
    let _defer_cancel = CancelOnDrop(cancel.clone());

    let (eg, ctx) = Group::WithContext(parent_ctx);
    let db = db.clone();
    let query = Arc::new(query);

    for _ in 0..batches {
        // 父 context 已取消则不再派发新 goroutine，与 Go select ctx 类似。
        if ctx.Err().is_some() {
            break;
        }
        // <-ch
        // 取 worker 令牌；channel 关闭映射为错误（Go 失败不归还令牌可能死锁，测试用小 batches 规避）。
        {
            let rx_guard = rx.lock().unwrap();
            let _token = rx_guard
                .recv()
                .map_err(|_| Error::new("worker token closed"))?;
        }

        let db_task = db.clone();
        let query_task = Arc::clone(&query);
        let ctx_task = ctx.clone();
        let cancel_task = cancel.clone();
        let tx_task = Arc::clone(&tx);
        eg.Go(move || {
            // ExecContext 失败：cancel 父 ctx 并 Trace 向上传播。
            if let Err(err) = db_task.ExecContext(&ctx_task, &query_task) {
                cancel_task.call();
                return Err(Trace(err));
            }
            // ch <- struct{}{}
            // 成功则归还令牌。
            if let Some(tx) = tx_task.lock().unwrap().as_ref() {
                let _ = tx.send(());
            }
            Ok(())
        });
    }

    // 等待全部 worker；首错由 errgroup 返回。
    eg.Wait()
}

/// Open DB via `open`, then run import (Go RunE after flag reads).
// sql.Open 后 run_import_with_db，open 可注入用于测试失败路径。
pub fn run_import(flags: &Flags, open: OpenFn, batches: usize) -> Result<()> {
    let dsn = build_dsn(&flags.database, flags.port);
    let db = open(&dsn).map_err(Trace)?;
    run_import_with_db(flags, db, batches)
}

/// Parse args and run with default open + 500 batches (Go `rootCmd.Execute` path).
// parse_flags + default_open + DEFAULT_BATCHES，对应 Go rootCmd.Execute。
pub fn execute(args: &[String]) -> Result<()> {
    let flags = stubs::parse_flags(args).map_err(Trace)?;
    run_import(&flags, stubs::default_open(), DEFAULT_BATCHES)
}

/// Entry matching Go `main`.
// 二进制入口：读 argv，失败 print_fail 并 os.Exit(2)。
pub fn main() {
    let args = stubs::args_from_env();
    if let Err(err) = execute(&args) {
        stubs::print_fail(&err);
        stubs::os_exit(2);
    }
}

/// Expose context cancel helper type for tests that need the CancelFunc shape.
// 测试用类型别名，暴露 CancelFunc/Context 形状。
pub type ImportCancel = CancelFunc;
pub type ImportContext = Context;
