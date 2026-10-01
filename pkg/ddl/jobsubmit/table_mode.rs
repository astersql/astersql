// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 表模式（TableMode）变更：构造 `AlterTableMode` DDL Job。
//
// TableMode 用于标记表当前处于普通、导入（Import）或恢复（Restore）状态，
// 以限制并发 DDL/DML 行为。仅允许特定方向的模式迁移，相同模式视为 no-op。

use crate::{Error, Job, JobState, JobType};

/// 表的运行模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableMode {
    /// 普通在线模式。
    Normal,
    /// 导入模式（如 Lightning/IMPORT INTO 期间）。
    Import,
    /// 恢复模式（备份恢复期间）。
    Restore,
}

impl TableMode {
    /// 是否允许从当前模式切换到目标模式。
    ///
    /// 允许同模式及 Normal 与特殊模式之间的切换；仅 Import↔Restore 非法。
    pub fn can_transition_to(self, target: Self) -> bool {
        !matches!(
            (self, target),
            (Self::Import, Self::Restore) | (Self::Restore, Self::Import)
        )
    }
}

/// 发起表模式变更时的目标描述（含当前/目标模式与对象标识）。
#[derive(Clone, Debug)]
pub struct AlterTableModeTarget {
    /// 变更前的表模式。
    pub current_mode: TableMode,
    /// 期望切换到的表模式。
    pub target_mode: TableMode,
    /// Schema（库）ID。
    pub schema_id: i64,
    /// 表 ID。
    pub table_id: i64,
    /// Schema 名称。
    pub schema_name: String,
    /// 表名称。
    pub table_name: String,
}

/// 写入 Job 参数中的表模式变更载荷。
#[derive(Clone, Debug)]
pub struct AlterTableModeArgs {
    /// 目标表模式。
    pub table_mode: TableMode,
    pub schema_id: i64,
    pub table_id: i64,
}

/// 会话变量中与 CDC / SQL Mode 相关的子集，会写入 Job。
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionVariables {
    /// CDC（Change Data Capture，变更数据捕获）写来源标识。
    pub cdc_write_source: u64,
    /// SQL Mode 位图。
    pub sql_mode: u64,
}

/// 构造 AlterTableMode Job；非法迁移返回错误，同模式返回 no-op（第三元为 true）。
pub fn build_alter_table_mode_job(
    vars: SessionVariables,
    target: AlterTableModeTarget,
) -> Result<(Option<Job>, Option<AlterTableModeArgs>, bool), Error> {
    if !target.current_mode.can_transition_to(target.target_mode) {
        return Err(Error::invalid(format!(
            "invalid table mode transition {:?} -> {:?} for {}",
            target.current_mode, target.target_mode, target.table_name
        )));
    }
    // 当前模式与目标相同：无需提交 job。
    if target.current_mode == target.target_mode {
        return Ok((None, None, true));
    }
    let args = AlterTableModeArgs {
        table_mode: target.target_mode,
        schema_id: target.schema_id,
        table_id: target.table_id,
    };
    let job = Job {
        version: 2,
        schema_id: target.schema_id,
        table_id: target.table_id,
        schema_name: target.schema_name.to_lowercase(),
        table_name: target.table_name.to_lowercase(),
        job_type: JobType::AlterTableMode,
        query: "skip".into(),
        binlog_info_present: true,
        cdc_write_source: vars.cdc_write_source,
        sql_mode: vars.sql_mode,
        state: JobState::None,
        involving_schemas: vec![(
            target.schema_name.to_lowercase(),
            target.table_name.to_lowercase(),
        )],
        ..Job::default()
    };
    Ok((Some(job), Some(args), false))
}

/// Encode the same version-2 payload as Go model.AlterTableModeArgs.
pub fn table_mode_args(args: AlterTableModeArgs) -> crate::JobArgs {
    let mode = match args.table_mode {
        TableMode::Normal => 0,
        TableMode::Import => 1,
        TableMode::Restore => 2,
    };
    crate::JobArgs::Opaque(
        serde_json::to_vec(&serde_json::json!({
            "table_mode": mode, "schema_id": args.schema_id, "table_id": args.table_id,
        }))
        .expect("serialize table-mode arguments"),
    )
}
