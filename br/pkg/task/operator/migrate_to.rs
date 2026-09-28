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

//! Migrate-to operator — mirrors `br/pkg/task/operator/migrate_to.go`.
//!
//! 将日志备份 migration 栈合并/迁移到目标版本（Recent / Base / 显式 SeqNum）。
//! 支持 DryRun（只估效果并落盘 JSON）与交互确认；真实执行走
//! `MergeAndMigrateTo`。与 Go 同文件数据流与分支语义对齐。
//! 入口由 `br operator migrate-to` 调用，不改动备份数据本体以外的集群状态。

use std::sync::Arc;

use crate::config::MigrateToConfig;
use crate::stubs::{
    ConsoleOperations, CreateStorage, Error, InteractiveCheck, MergeAndMigratedTo, Migration,
    MigrationExt, MigrationExtension, Migrations, NewOperationContext, NewProgressBarHooks,
    ParseBackend, Result, SaveJSONEffectsToTmp, color_bold, color_hi_red,
};

impl MigrateToConfig {
    /// 解析目标 SeqNum：`Recent`→最新 Layer；`Base`→0；否则用 `MigrateTo`。
    /// 第二返回值 false 表示 Recent 但无 Layer，调用方应跳过。
    pub fn getTargetVersion(&self, migs: &Migrations) -> (i32, bool) {
        if self.Recent {
            // 无 Layer 时无法“迁到最近”，与 Go 一样返回 (0, false)。
            if migs.Layers.is_empty() {
                return (0, false);
            }
            return (migs.Layers[0].SeqNum, true);
        }
        if self.Base {
            // Base=0：折叠到 BASE migration。
            return (0, true);
        }
        (self.MigrateTo, true)
    }
}

/// 运行时上下文：配置、控制台与带 OperationContext 的 MigrationExt。
struct migrateToCtx {
    cfg: MigrateToConfig,
    console: ConsoleOperations,
    est: MigrationExt,
}

impl migrateToCtx {
    /// 将估算阶段 Warnings 以红色列表打印；空切片则静默。
    fn printErr(&self, errs: &[Error], msg: &str) {
        if !errs.is_empty() {
            self.console.Println(msg);
            for w in errs {
                self.console.Printf(format!("- {}\n", color_hi_red(&w.msg)));
            }
        }
    }

    /// 交互确认：表格展示目标 migration 后 PromptBool；`--yes` 路径不调用此方法。
    fn askForContinue(&self, targetMig: &Migration) -> bool {
        let mut tbl = self.console.CreateTable();
        self.est.AddMigrationToTable(targetMig, &mut tbl);
        self.console
            .Println("The migration going to be executed will be like: ");
        tbl.Print();
        self.console.PromptBool("Continue? ")
    }

    /// DryRun：在临时 Ext 上跑合并回调，打印新 BASE、effects 临时文件与警告。
    /// 真实存储不落盘；effects 经 `SaveJSONEffectsToTmp` 供人工审查。
    fn dryRun(&self, f: impl FnOnce(MigrationExt) -> Result<MergeAndMigratedTo>) -> Result<()> {
        let est = self.est.clone();
        let console = &self.console;
        let mut runErr: Option<Error> = None;
        let mut estBase = MergeAndMigratedTo::default();
        // DryRun 闭包内错误先记下，结束后再 Trace，避免部分 effects 被吞。
        let effects = est.DryRun(|me| match f(me) {
            Ok(v) => estBase = v,
            Err(e) => runErr = Some(e),
        });
        if let Some(err) = runErr {
            return Err(Error::Trace(err));
        }

        let mut tbl = console.CreateTable();
        self.est.AddMigrationToTable(&estBase.NewBase, &mut tbl);
        console.Println("The new BASE migration will be like: ");
        tbl.Print();
        // 效果条数高亮：提醒运维去临时 JSON 核对删除/改写对象。
        let file = SaveJSONEffectsToTmp(&effects)?;
        console.Printf(format!(
            "{} effects will happen in the external storage, you may check them in {}\n",
            color_hi_red(&format!("{}", effects.len())),
            color_bold(&file)
        ));
        self.printErr(
            &estBase.Warnings,
            "The following errors happened during estimating: ",
        );
        Ok(())
    }
}

/// CLI 入口：校验配置 → 打开存储 → 解析目标版本 → DryRun 或真实 MergeAndMigrateTo。
pub fn RunMigrateTo(cfg: MigrateToConfig) -> Result<()> {
    cfg.Verify()?;

    // OperationContext 用于进度/取消传播，名称与 Go `operation.NewContext` 一致。
    let operationContext = NewOperationContext("operator migrate-to")?;

    let backend = ParseBackend(&cfg.StorageURI, &cfg.BackendOptions)?;
    let st = CreateStorage(&backend, false)?;

    let console = ConsoleOperations::StdIO();

    let est = MigrationExtension(st).WithOperationContext(operationContext);
    // 挂进度条 Hooks；Load(false) 表示缺失 migration 不当作错误选项组合（对齐 Go Load）。
    NewProgressBarHooks(&console);
    let migs = est.Load(false)?;

    let cx = migrateToCtx {
        cfg: cfg.clone(),
        console: console.clone(),
        est: est.clone(),
    };

    let (targetVersion, ok) = cfg.getTargetVersion(&migs);
    if !ok {
        // Recent 且无 Layer：跳过而非失败，与 Go 一致。
        console.Printf("No recent migration found. Skipping.");
        return Ok(());
    }

    let yes = cfg.Yes;
    let console_ops = cx.console.clone();
    let est_for_ask = cx.est.clone();
    // InteractiveCheck：`--yes` 直接通过；否则复现 askForContinue 表格确认。
    let check: InteractiveCheck = Arc::new(move |m: &Migration| {
        if yes {
            return true;
        }
        let mut tbl = console_ops.CreateTable();
        est_for_ask.AddMigrationToTable(m, &mut tbl);
        console_ops.Println("The migration going to be executed will be like: ");
        tbl.Print();
        console_ops.PromptBool("Continue? ")
    });

    if cfg.DryRun {
        // 估测路径：不改外部存储，仅打印新 BASE 与 effects 临时文件。
        let check = check.clone();
        cx.dryRun(|est| est.MergeAndMigrateTo(targetVersion, Some(check)))
    } else {
        // 真实合并：写入外部存储；部分失败以 Warnings 暴露。
        let result = est.MergeAndMigrateTo(targetVersion, Some(check))?;
        // Warnings 非空仍算成功返回，提示可重试；与 Go 打印路径一致。
        if !result.Warnings.is_empty() {
            console.Printf("The following errors happened, you may re-execute to retry: ");
            for w in &result.Warnings {
                // 红色列出每条警告，便于运维定位可重试对象。
                console.Printf(format!("- {}\n", color_hi_red(&w.msg)));
            }
        }
        Ok(())
    }
}
