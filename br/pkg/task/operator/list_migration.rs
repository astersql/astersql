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

//! List migrations — mirrors `br/pkg/task/operator/list_migration.go`.
//!
//! 列出外部存储上的日志备份 migration 栈：BASE + 各 Layer。
//! 数据流：ParseBackend → CreateStorage → MigrationExtension.Load →
//! JSON 整包输出，或控制台表格逐层打印。与 Go `RunListMigrations` 对齐。

use crate::config::ListMigrationConfig;
use crate::stubs::{
    ConsoleOperations, CreateStorage, MLNotFoundIsErr, MigrationExtension, ParseBackend, Result,
    color_bold, color_green,
};

/// statusOK make a string like <green>●</green> <bold>{message}</bold>
/// 控制台成功态前缀；颜色与 Go `color.GreenString`/`Bold` 一致。
pub fn statusOK(message: &str) -> String {
    format!("{}{}", color_green("●"), color_bold(&format!(" {message}")))
}

/// CLI 入口：加载 migration 并按 `JSONOutput` 选择机器可读或表格展示。
pub fn RunListMigrations(cfg: ListMigrationConfig) -> Result<()> {
    // 与 Go `objstore.ParseBackend`/`Create` 一致：只打开存储，不改写对象。
    let backend = ParseBackend(&cfg.StorageURI, &cfg.BackendOptions)?;
    let st = CreateStorage(&backend, false)?;
    let ext = MigrationExtension(st);
    // `MLNotFoundIsErr`：缺失 migration 视为错误，避免空栈被当成成功。
    let migs = ext.Load(MLNotFoundIsErr())?;
    if cfg.JSONOutput {
        // 自动化路径：整包序列化，字段布局对齐 Go `json.Encoder`。
        let encoded =
            serde_json::to_string(&migs).map_err(|e| crate::stubs::Error::new(e.to_string()))?;
        println!("{encoded}");
    } else {
        let console = ConsoleOperations::StdIO();
        // Layers+1：BASE 单独算一层，与 Go `len(migs.Layers)+1` 一致。
        console.Println(statusOK(&format!(
            "Total {} Migrations.",
            migs.Layers.len() + 1
        )));
        console.Printf(">   BASE   <\n");
        let mut tbl = console.CreateTable();
        ext.AddMigrationToTable(&migs.Base, &mut tbl);
        tbl.Print();
        for t in &migs.Layers {
            // SeqNum 八位零填充，便于与文件名/日志对照。
            console.Printf(format!("> {:08} <\n", t.SeqNum));
            let mut tbl = console.CreateTable();
            ext.AddMigrationToTable(&t.Content, &mut tbl);
            tbl.Print();
        }
    }
    Ok(())
}
