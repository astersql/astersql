// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Load backup schemas into databases, ported from `br/pkg/metautil/load.go`.
//!
//! 将 `MetaReader::ReadSchemasFiles` 产出的表流聚合成 `HashMap<库名, Database>`。
//! 读取在独立线程中进行，主循环通过 channel 收表并按库名归桶；
//! 取消会像 Go `select` 一样立即返回，后台错误则在回收读取线程后向上传播。
//! `loadStats=false` 时注入 `SkipStats`，避免恢复路径无谓解析统计 JSON。
//! 库名键使用 `DB.Name.O`（原始大小写），与 Go map key 约定一致。
//! 本模块不负责文件归属算法本身，那部分在 MetaReader 批处理回调中完成。

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use astersql_errors::{SharedError, Trace};
use astersql_meta_model as model;
use astersql_objstore_storeapi::Context;

use crate::metafile::{MetaReader, ReadSchemaOption, SkipStats, Table};

/// 包装 Trace，保持与 Go `errors.Trace` 调用点一致的错误栈形态。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// Database wraps the schema and tables of a database.
///
/// 备份中单个逻辑库的聚合视图：持有 `DBInfo` 与下属 `Table` 列表。
/// `reusedByPITR` 标记该库是否被 PITR 复用路径认领，供上层跳过重复恢复。
pub struct Database {
    pub Info: model::DBInfo,
    pub Tables: Vec<Table>,
    reusedByPITR: bool,
}

impl Database {
    /// 标记库已被 PITR 复用，后续恢复逻辑可据此短路。
    pub fn SetReusedByPITR(&mut self) {
        self.reusedByPITR = true;
    }

    /// 查询 PITR 复用标记；默认 false，仅显式 Set 后为 true。
    pub fn IsReusedByPITR(&self) -> bool {
        self.reusedByPITR
    }

    /// GetTable returns a table of the database by name.
    ///
    /// 按表原始名 `Name.O` 线性查找；缺失 `Info` 与 Go nil 解引用一样失败。
    pub fn GetTable(&self, name: &str) -> Option<&Table> {
        for table in &self.Tables {
            let info = table.Info.as_ref().expect("table info must not be nil");
            if info.Name.O == name {
                return Some(table);
            }
        }
        None
    }
}

/// LoadBackupTables loads schemas from BackupMeta.
///
/// 启动后台线程调用 `ReadSchemasFiles`；主线程轮询取消、错误通道与表通道。
/// channel 关闭后若仍有后台错误会优先返回错误，否则交出完整库映射。
pub fn LoadBackupTables(
    ctx: &Context,
    reader: &MetaReader,
    loadStats: bool,
) -> Result<HashMap<String, Database>, SharedError> {
    let mut opts: Vec<ReadSchemaOption> = Vec::new();
    // 与 Go `metautil.SkipStats` 选项对齐：不加载统计以加速纯 schema 场景。
    if !loadStats {
        opts.push(SkipStats);
    }

    // 表通道与错误通道分离，避免错误与正常表交错难辨。
    let (tx, rx) = mpsc::channel();
    let (errTx, errRx) = mpsc::channel();
    let reader_ctx = ctx.clone();
    let reader = reader.clone();
    // 后台读 schema：发送失败视为输出端已关闭，转为 BrokenPipe 风格错误。
    let handle = thread::spawn(move || {
        let result = reader.ReadSchemasFiles(
            &reader_ctx,
            |table| {
                tx.send(table).map_err(|_| {
                    SharedError::new(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "schema reader output closed",
                    ))
                })
            },
            &opts,
        );
        if let Err(err) = result {
            let _ = errTx.send(err);
        }
    });

    let mut databases: HashMap<String, Database> = HashMap::new();
    loop {
        // 取消优先：像 Go `select` 一样立即返回，不等待可能阻塞的存储调用。
        if ctx.is_cancelled() {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "context canceled",
            )));
        }
        if let Ok(err) = errRx.try_recv() {
            let _ = handle.join();
            return Err(trace_err(err));
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(table) => {
                // 按库原始名归桶；首张表决定 Database.Info。
                let dbName = table.DB.Name.O.clone();
                let db = databases.entry(dbName.clone()).or_insert_with(|| Database {
                    Info: table.DB.clone(),
                    Tables: Vec::new(),
                    reusedByPITR: false,
                });
                db.Tables.push(table);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // 读端结束：再冲一次错误通道，防止后台晚到的错误被吞。
                let _ = handle.join();
                if let Ok(err) = errRx.try_recv() {
                    return Err(trace_err(err));
                }
                return Ok(databases);
            }
        }
    }
}
