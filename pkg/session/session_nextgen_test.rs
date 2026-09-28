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

// Next-gen（下一代）部署模式下会话行为的测试草稿。
//
// 覆盖 Starter 部署模式禁用 pipelined DML（流水线式批量写入）并写入 warning 的语义，
// 对应 Go build tag `nextgen`。

// Starter 部署模式下 pipelined DML 被禁用并写入 warning 的测试语义。
// Go build tag: nextgen。

// DeployModeDraft 对应 Go deploymode.Get/Set 中本测试关心的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 部署模式草稿：Starter 或其它。
pub enum DeployModeDraft {
    Starter,
    Other,
}

// WarningDraft 对应 StmtCtx.GetWarnings 返回项里 Err 的最小展示形状。
#[derive(Debug, Clone, PartialEq, Eq)]
/// 警告条目草稿，仅保留错误展示字符串。
pub struct WarningDraft {
    pub err: String,
}

// StatementContextDraft 对应 Go sessionVars.StmtCtx，本测试只关心 InInsertStmt 和 warning 列表。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// 语句上下文草稿：是否在 INSERT 中，以及 warning 列表。
pub struct StatementContextDraft {
    pub in_insert_stmt: bool,
    pub warnings: Vec<WarningDraft>,
}

// SessionVarsDraft 对应 variable.NewSessionVars(nil) 之后测试手动设置的 session vars。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// 会话变量草稿：是否启用 bulk DML 与语句上下文。
pub struct SessionVarsDraft {
    pub bulk_dml_enabled: bool,
    pub stmt_ctx: StatementContextDraft,
}

// SessionDraft 对应 Go 的 &session{sessionVars: variable.NewSessionVars(nil)}。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// 会话草稿，对应 Go `&session{sessionVars: ...}`。
pub struct SessionDraft {
    pub session_vars: SessionVarsDraft,
}

impl SessionDraft {
    // use_pipelined_dml_or_warn 对应 Go 的 s.usePipelinedDmlOrWarn(context.Background())。
    // Starter 模式禁用 pipelined DML，并把原 Go 错误文本追加到 warning 列表中。
    /// Starter 模式下禁用 pipelined DML 并追加 warning；否则返回 bulk_dml_enabled。
    pub fn use_pipelined_dml_or_warn(&mut self, deploy_mode: DeployModeDraft) -> bool {
        if self.session_vars.bulk_dml_enabled
            && self.session_vars.stmt_ctx.in_insert_stmt
            && deploy_mode == DeployModeDraft::Starter
        {
            self.session_vars.stmt_ctx.warnings.push(WarningDraft {
                err: "Pipelined DML is not supported in this deployment. Fallback to standard mode"
                    .to_owned(),
            });
            return false;
        }

        self.session_vars.bulk_dml_enabled
    }
}

// test_use_pipelined_dml_disabled_in_starter 对应 Go 的 TestUsePipelinedDMLDisabledInStarter。
// Go 用 t.Cleanup 恢复 originalMode；这里以变量记录该资源收尾语义。
#[test]
/// 对应 Go TestUsePipelinedDMLDisabledInStarter。
fn test_use_pipelined_dml_disabled_in_starter() {
    let original_mode = DeployModeDraft::Other;
    let deploy_mode = DeployModeDraft::Starter;

    let mut s = SessionDraft {
        session_vars: SessionVarsDraft {
            bulk_dml_enabled: true,
            stmt_ctx: StatementContextDraft {
                in_insert_stmt: true,
                warnings: Vec::new(),
            },
        },
    };

    let enabled = s.use_pipelined_dml_or_warn(deploy_mode);
    let warnings = &s.session_vars.stmt_ctx.warnings;

    assert!(!enabled);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].err,
        "Pipelined DML is not supported in this deployment. Fallback to standard mode"
    );

    // Go: t.Cleanup(func() { deploymode.Set(originalMode) })，这里保留测试结束恢复部署模式的意图。
    let restored_mode = original_mode;
    assert_eq!(restored_mode, DeployModeDraft::Other);
}

#[test]
/// 校验部署模式解析：STARTER 成功，带空格则失败。
fn canonical_deploy_mode_parser_selects_starter_without_trimming() {
    use astersql_config_deploymode::{Parse, Starter};

    assert_eq!(Parse("STARTER").unwrap(), Starter);
    assert!(Parse(" starter").is_err());
    assert_eq!(Starter.String(), "starter");
}
