// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

//! Importer process entry matching `cmd/importer/main.go`.
//! 本模块是 importer 二进制真正执行主流程的装配层。
//! 它不承载具体造数算法，而是把配置解析、DDL 解析、统计加载和并发导入串成固定顺序。
//! 整体调用次序尽量贴近 Go 入口，方便用相同心智模型排查迁移后的行为差异。
//! 这里最重要的约束是先构建表元数据，再决定是否接入统计信息，最后才执行建表和导入。
//! 这样列定义、索引信息和可选直方图会先完整挂到 `table` 上，后续 worker 才能按统一视图生成数据。
//! 本文件也负责把命令行失败映射成进程退出码，而不是把这些分支散落到子模块里。

use std::sync::Arc;

use crate::config::NewConfig;
use crate::db::{closeDBs, createDBs, execSQL};
use crate::job::doProcess;
use crate::parser::{newTable, parseIndexSQL, parseTableSQL};
use crate::stats::{histogram, loadStats};
use crate::stubs::{self, DB};

/// Entry matching Go `main`.
/// 外部进程只经过这一层读取环境参数并调用真正可复用的 `run_with_args`。
/// 这样二进制入口与 parity 测试共享同一条业务路径，只把参数来源差异隔离在最外层。
/// 如果解析失败，退出码也在这里统一落到 stub 的 `os_exit`，保持与 Go `main` 一样的边界契约。
pub fn main() {
    let args = stubs::args_from_env();
    match run_with_args(&args, None) {
        Ok(()) => {}
        Err(code) => stubs::os_exit(code),
    }
}

/// Runnable entry for binaries and parity tests.
///
/// When `dbs_override` is `Some`, skip `createDBs` and use the provided handles
/// (keeps Go call order for DDL exec + doProcess while allowing in-memory DB).
/// `run_with_args` 是本模块真正的主流程，生产运行和测试回归都依赖它。
/// `dbs_override` 只改变连接来源，不改变 DDL 执行、统计预热和任务调度的先后顺序。
/// 因此测试可以替换数据库句柄，但仍能验证 importer 对外暴露的执行编排是否保持 Go 语义。
pub fn run_with_args(args: &[String], dbs_override: Option<Vec<DB>>) -> Result<(), i32> {
    let mut cfg = NewConfig();
    match cfg.Parse(args) {
        Ok(()) => {}
        Err(err) => {
            // 帮助输出在 Go 中也是成功退出，这里保留退出码 0，避免把查看帮助误判为失败。
            if err.is_help {
                return Err(0);
            }
            // 其他解析错误统一映射为退出码 2，便于脚本侧沿用 Go 版本的错误处理习惯。
            stubs::log_error(format!("parse cmd flags: {err}"));
            return Err(2);
        }
    }

    let mut table = newTable();
    // 先解析建表 SQL，再解析索引 SQL，让索引列偏移建立在已经存在的列定义之上。
    if let Err(err) = parseTableSQL(&mut table, &cfg.DDLCfg.TableSQL) {
        stubs::fatal(err.Error());
    }
    if let Err(err) = parseIndexSQL(&mut table, &cfg.DDLCfg.IndexSQL) {
        stubs::fatal(err.Error());
    }

    let dbs = if let Some(dbs) = dbs_override {
        // 测试注入的连接直接复用，确保后续逻辑观察到的仍是一组已就绪句柄。
        dbs
    } else {
        match createDBs(&cfg.DBCfg, cfg.SysCfg.WorkerCount) {
            Ok(dbs) => dbs,
            Err(err) => stubs::fatal(err.Error()),
        }
    };

    if !cfg.StatsCfg.Path.is_empty() {
        // 统计信息是可选增强；缺失时 importer 仍可运行，只是退回普通随机造数。
        let statsInfo = match loadStats(&table.tblInfo, &cfg.StatsCfg.Path) {
            Ok(s) => s,
            Err(err1) => stubs::fatal(err1.Error()),
        };
        for idxInfo in &table.tblInfo.Indices {
            if idxInfo.Columns.is_empty() {
                continue;
            }
            let offset = idxInfo.Columns[0].Offset;
            if let Some(hist) = statsInfo.GetIdx(idxInfo.ID) {
                if !hist.Histogram.Buckets.is_empty() {
                    // 索引直方图优先挂到首列偏移，保持 Go 版本依靠第一列驱动索引分布的约定。
                    if offset < table.columns.len() {
                        table.columns[offset].hist = Some(Arc::new(histogram::from_core(
                            hist.Histogram.clone(),
                            hist.Info.clone(),
                        )));
                    }
                }
            }
        }
        for (i, colInfo) in table.tblInfo.Columns.iter().enumerate() {
            if let Some(hist) = statsInfo.GetCol(colInfo.ID) {
                if table.columns[i].hist.is_none() && !hist.Histogram.Buckets.is_empty() {
                    // 列直方图只在还没有索引直方图占位时补上，避免覆盖更具体的分布信息。
                    table.columns[i].hist =
                        Some(Arc::new(histogram::from_core(hist.Histogram.clone(), None)));
                }
            }
        }
    }

    if let Err(err) = execSQL(&dbs[0], &cfg.DDLCfg.TableSQL) {
        stubs::fatal(err.Error());
    }
    if let Err(err) = execSQL(&dbs[0], &cfg.DDLCfg.IndexSQL) {
        stubs::fatal(err.Error());
    }

    // 真正的数据导入总是在 DDL 成功后才启动，避免 worker 在目标表尚未存在时抢跑。
    doProcess(
        Arc::new(table),
        &dbs,
        cfg.SysCfg.JobCount,
        cfg.SysCfg.WorkerCount,
        cfg.SysCfg.Batch,
    );

    // 显式关闭连接与 Go 的 `defer closeDBs` 等价，只是 Rust 这里把时机写在主流程末尾。
    closeDBs(&dbs);
    Ok(())
}
