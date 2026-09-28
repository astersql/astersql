// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// ADD / DROP QUERY WATCH 执行逻辑。
//
// 将 AST 选项转为隔离记录（QuarantineRecord），校验资源组与 runaway
//（失控查询）动作后，交给 RunawayManager 注册或删除监视规则。
// 下方注释块保留 Go 结构与控制流映射，实际可执行实现位于其后。

// ADD/DROP QUERY WATCH 如何解析 AST 选项、构造 runaway quarantine record，并调用 domain 中的管理器。
//
// setWatchOption 对应 Go 函数：把单个 QueryWatchOption AST 节点写入 QuarantineRecord。
// pub fn setWatchOption(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     newSctx: sessionctx::Context,
//     record: &mut runaway::QuarantineRecord,
//     op: &ast::QueryWatchOption,
// ) -> Result<(), errors::Error> {
//     match op.Tp {
//         ast::QueryWatchResourceGroup => {
//             let resourceGroupOption = &op.ResourceGroupOption;
//             if resourceGroupOption.GroupNameExpr.is_some() {
//                 let expr = plannerutil::RewriteAstExprWithPlanCtx(
//                     sctx.GetPlanCtx(),
//                     resourceGroupOption.GroupNameExpr.clone(),
//                     None,
//                     None,
//                     false,
//                 )?;
//                 let (name, isNull, err) =
//                     expr.EvalString(sctx.GetExprCtx().GetEvalCtx(), chunk::Row {});
//                 if let Some(err) = err {
//                     return Err(err);
//                 }
//                 if isNull {
//                     return Err(errors::Errorf("invalid resource group name expression"));
//                 }
//                 record.ResourceGroupName = name;
//             } else {
//                 record.ResourceGroupName = resourceGroupOption.GroupNameStr.L.clone();
//             }
//         }
//         ast::QueryWatchAction => {
//             record.Action = rmpb::RunawayAction(op.ActionOption.Type);
//             record.SwitchGroupName = op.ActionOption.SwitchGroupName.String();
//         }
//         ast::QueryWatchType => {
//             let textOption = &op.TextOption;
//             let expr = plannerutil::RewriteAstExprWithPlanCtx(
//                 sctx.GetPlanCtx(),
//                 textOption.PatternExpr.clone(),
//                 None,
//                 None,
//                 false,
//             )?;
//             let (strval, isNull, err) =
//                 expr.EvalString(sctx.GetExprCtx().GetEvalCtx(), chunk::Row {});
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if isNull {
//                 return Err(errors::Errorf("invalid watch text expression"));
//             }
//
//             let watchType = textOption.Type;
//             record.Watch = rmpb::RunawayWatchType(watchType);
//             if textOption.TypeSpecified {
//                 let p = parser::New();
//                 let (stmts, _, err) = p.ParseSQL(strval);
//                 if let Some(err) = err {
//                     return Err(err);
//                 }
//                 if stmts.len() != 1 {
//                     return Err(errors::Errorf("only support one SQL"));
//                 }
//
//                 let sql = stmts[0].Text();
//                 match watchType {
//                     ast::WatchNone => return Err(errors::Errorf("watch type must be specified")),
//                     ast::WatchExact => {
//                         record.WatchText = sql;
//                     }
//                     ast::WatchSimilar => {
// Go 对 SQL 做 normalize digest；这里只保留 digest 字符串作为 watch text 的流向。
//                         let (_, digest) = parser::NormalizeDigest(&sql);
//                         record.WatchText = digest.String();
//                     }
//                     ast::WatchPlan => {
//                         let sqlExecutor = newSctx.GetSQLExecutor();
// Go 会执行 explain 来填充 StmtCtx 的 plan digest；这是外部 SQL 执行点，不实际执行。
//                         if let (_, Some(err)) =
//                             sqlExecutor.ExecuteInternal(ctx, format!("explain {}", stmts[0].Text()))
//                         {
//                             return Err(err);
//                         }
//                         let (_, digest) = newSctx.GetSessionVars().StmtCtx.GetPlanDigest();
//                         if digest.is_none() {
//                             return Err(errors::Errorf("no plan digest"));
//                         }
//                         record.WatchText = digest.unwrap().String();
//                     }
//                     _ => {}
//                 }
//             } else {
// Go 未指定 watch type 时要求传入 64 位 digest 字符串。
//                 if strval.len() != 64 {
//                     return Err(errors::Errorf("digest format error"));
//                 }
//                 record.WatchText = strval;
//             }
//         }
//         _ => {}
//     }
//     Ok(())
// }
//
// fromQueryWatchOptionList 对应 Go 函数：用一组选项构造默认 QuarantineRecord。
// pub fn fromQueryWatchOptionList(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     newSctx: sessionctx::Context,
//     optionList: Vec<ast::QueryWatchOption>,
// ) -> Result<runaway::QuarantineRecord, errors::Error> {
//     let mut record = runaway::QuarantineRecord {
//         Source: runaway::ManualSource,
//         StartTime: time::Now().UTC(),
//         EndTime: runaway::NullTime,
//         ExceedCause: "None".to_string(),
//         ..Default::default()
//     };
//     for op in &optionList {
//         setWatchOption(ctx.clone(), sctx.clone(), newSctx.clone(), &mut record, op)?;
//     }
//     Ok(record)
// }
//
// validateWatchRecord 对应 Go 函数：补默认 resource group、校验 resource group/action/watch type。
// pub fn validateWatchRecord(
//     record: &mut runaway::QuarantineRecord,
//     client: &rmclient::ResourceGroupsController,
// ) -> Result<(), errors::Error> {
//     if record.ResourceGroupName.is_empty() {
//         record.ResourceGroupName = resourcegroup::DefaultResourceGroupName.to_string();
//     }
//
//     let rg = client.GetResourceGroup(&record.ResourceGroupName)?;
//     if rg.is_none() {
//         return Err(infoschema::ErrResourceGroupNotExists
//             .GenWithStackByArgs(record.ResourceGroupName.clone()));
//     }
//     let rg = rg.unwrap();
//     if record.Action == rmpb::RunawayAction_NoneAction {
//         if rg.RunawaySettings.is_none() {
//             return Err(errors::Errorf(format!(
//                 "must set runaway config for resource group `{}`",
//                 record.ResourceGroupName
//             )));
//         }
// Go 从 resource group 的 runaway settings 补齐默认 action 和 switch group。
//         let settings = rg.RunawaySettings.unwrap();
//         record.Action = settings.Action;
//         record.SwitchGroupName = settings.SwitchGroupName;
//     }
//
// TODO: validate the switch group.
//     if record.Watch == rmpb::RunawayWatchType_NoneWatch {
//         return Err(errors::Errorf("must specify watch type"));
//     }
//     Ok(())
// }
//
// AddExecutor 对应 Go 结构体：ADD QUERY WATCH 的 executor，内嵌 BaseExecutor 并用 done 防止重复执行。
// pub struct AddExecutor {
//     pub QueryWatchOptionList: Vec<ast::QueryWatchOption>,
//     pub BaseExecutor: exec::BaseExecutor,
//     pub done: bool,
// }
//
// impl AddExecutor {
// Next 对应 Go Executor 接口实现：构造、校验并注册 runaway watch，结果 id 写入 chunk。
//     pub fn Next(&mut self, ctx: context::Context, req: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         req.Reset();
//         if self.done {
//             return Ok(());
//         }
//         self.done = true;
//
//         let newSctx = self.GetSysSession()?;
//         let mut record = fromQueryWatchOptionList(
//             ctx,
//             self.Ctx(),
//             newSctx,
//             self.QueryWatchOptionList.clone(),
//         )?;
//         let dom = domain::GetDomain(self.Ctx());
//         validateWatchRecord(&mut record, dom.ResourceGroupsController())?;
//         let id = dom.RunawayManager().AddRunawayWatch(record)?;
//         req.AppendUint64(0, id);
//         Ok(())
//     }
// }
//
// ExecDropQueryWatch 对应 Go 函数：按 DROP 语句中的 group name、变量或 id 删除 watch。
// pub fn ExecDropQueryWatch(
//     sctx: sessionctx::Context,
//     s: &ast::DropQueryWatchStmt,
// ) -> Result<(), errors::Error> {
//     let dom = domain::GetDomain(sctx.clone());
//     if s.GroupNameStr.String() != "" {
//         return dom
//             .RunawayManager()
//             .RemoveRunawayResourceGroupWatch(s.GroupNameStr.String());
//     }
//     if s.GroupNameExpr.is_some() {
//         let userVars = sctx.GetSessionVars().UserVars;
// Go 这里把 GroupNameExpr 断言为 *ast.VariableExpr；保留变量读取的强假设。
//         if let Some(v) = userVars.GetUserVarVal(s.GroupNameExpr.as_ref().unwrap().Name.clone()) {
//             if let Ok(groupName) = v.ToString() {
//                 return dom.RunawayManager().RemoveRunawayResourceGroupWatch(groupName);
//             }
//         }
//         return Err(errors::Errorf("invalid group name variable"));
//     }
//     dom.RunawayManager().RemoveRunawayWatch(s.IntValue)
// }
// */
use std::collections::BTreeMap;
use std::time::SystemTime;

// 复用 parser crate 已审计的 Go 等价 digester 源码。当前聚焦 crate 的 manifest
// 不拥有依赖修改范围，因此在模块边界复用源码，并以本文件的无依赖 SHA-256
// 适配器满足 digester 所需的增量哈希接口。
#[path = "../../../parser/keywords.rs"]
mod keywords;

mod local_sha2 {
    /// `sha2::Digest` 中 digester 实际使用的最小接口。
    pub trait Digest {
        fn new() -> Self;
        fn update(&mut self, value: impl AsRef<[u8]>);
        fn finalize_reset(&mut self) -> Vec<u8>;
    }

    /// 收集增量输入，并在 finalize 时调用本模块的 SHA-256 实现。
    pub struct Sha256(Vec<u8>);

    impl Digest for Sha256 {
        fn new() -> Self {
            Self(Vec::new())
        }

        fn update(&mut self, value: impl AsRef<[u8]>) {
            self.0.extend_from_slice(value.as_ref());
        }

        fn finalize_reset(&mut self) -> Vec<u8> {
            let value = std::mem::take(&mut self.0);
            super::sha256_bytes(&value).to_vec()
        }
    }
}

#[allow(dead_code, non_snake_case)]
mod parser_digester {
    use super::local_sha2 as sha2;

    include!("../../../parser/digester.rs");
}

/// 未指定资源组时使用的默认名。
pub const DEFAULT_RESOURCE_GROUP: &str = "default";
#[derive(Clone, Debug, PartialEq, Eq)]
/// 对匹配到的 runaway 查询采取的动作。
pub enum RunawayAction {
    /// 未指定；校验阶段可从资源组默认配置补齐。
    None,
    /// 仅记录，不实际干预。
    DryRun,
    /// 降速/冷却。
    CoolDown,
    /// 终止查询。
    Kill,
    /// 切换到另一资源组。
    SwitchGroup,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 监视匹配方式：精确 SQL、相似摘要或执行计划 digest。
pub enum WatchType {
    /// 未指定，校验时必须被替换。
    None,
    /// 精确匹配 SQL 文本。
    Exact,
    /// 按 normalize 后的 digest 匹配相似 SQL。
    Similar,
    /// 按执行计划 digest 匹配。
    Plan,
}
#[derive(Clone, Debug)]
/// 一条 runaway 隔离/监视规则记录。
pub struct QuarantineRecord {
    /// 所属资源组名。
    pub resource_group: String,
    /// 触发后的处置动作。
    pub action: RunawayAction,
    /// SwitchGroup 动作的目标资源组。
    pub switch_group: String,
    /// 监视类型。
    pub watch: WatchType,
    /// 监视文本：SQL、digest 或 plan digest。
    pub watch_text: String,
    /// 规则来源（默认 manual）。
    pub source: String,
    /// 规则生效起始时间。
    pub start_time: SystemTime,
    /// 规则失效时间；手工新增规则对应 Go `runaway.NullTime`，默认为空。
    pub end_time: Option<SystemTime>,
    /// 超额/触发原因说明。
    pub exceed_cause: String,
}
/// 默认记录：manual 来源、空资源组、None 动作与监视类型。
impl Default for QuarantineRecord {
    fn default() -> Self {
        Self {
            resource_group: String::new(),
            action: RunawayAction::None,
            switch_group: String::new(),
            watch: WatchType::None,
            watch_text: String::new(),
            source: "manual".to_string(),
            start_time: SystemTime::now(),
            end_time: None,
            exceed_cause: "None".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
/// ADD QUERY WATCH 的单个选项，对应 AST 中的一类设置。
pub enum QueryWatchOption {
    /// 指定资源组名。
    ResourceGroup(String),
    /// 指定动作及可选的切换目标组。
    Action(RunawayAction, Option<String>),
    /// 指定监视文本与类型；`type_specified` 为假时要求 64 位 hex digest。
    Text {
        watch: WatchType,
        value: String,
        type_specified: bool,
    },
}
#[derive(Clone, Debug)]
/// 资源组摘要：名称与可选的默认 runaway 动作。
pub struct ResourceGroup {
    /// 资源组名。
    pub name: String,
    /// 默认 (动作, 切换组)；动作为 None 时用于补齐。
    pub default_action: Option<(RunawayAction, String)>,
}
/// 按名查询资源组，对应 domain 中的 ResourceGroupsController。
pub trait ResourceGroupController {
    /// 返回指定资源组；不存在时为 `Ok(None)`。
    fn resource_group(&self, name: &str) -> Result<Option<ResourceGroup>, String>;
}
/// runaway 监视规则的增删接口。
pub trait RunawayManager {
    /// 注册监视并返回规则 ID。
    fn add_watch(&self, record: QuarantineRecord) -> Result<u64, String>;
    /// 按 ID 删除监视。
    fn remove_watch(&self, id: u64) -> Result<(), String>;
    /// 删除某资源组下全部监视。
    fn remove_group_watches(&self, group: &str) -> Result<(), String>;
}
/// 由 SQL 计算执行计划 digest 的抽象（对应 explain + StmtCtx plan digest）。
pub trait PlanDigester {
    /// 返回 plan digest 字符串。
    fn plan_digest(&self, sql: &str) -> Result<String, String>;
}

/// 由选项列表构造 `QuarantineRecord`，并按监视类型填充 `watch_text`。
pub fn from_option_list(
    options: &[QueryWatchOption],
    digester: &dyn PlanDigester,
) -> Result<QuarantineRecord, String> {
    // 逐项应用选项；Text 在指定类型时解析单条 SQL 再生成文本/digest。
    let mut record = QuarantineRecord::default();
    for option in options {
        match option {
            QueryWatchOption::ResourceGroup(name) => {
                record.resource_group = name.clone();
            }
            QueryWatchOption::Action(action, switch) => {
                record.action = action.clone();
                record.switch_group = switch.clone().unwrap_or_default();
            }
            QueryWatchOption::Text {
                watch,
                value,
                type_specified,
            } => {
                record.watch = watch.clone();
                // 已指定 watch type：解析唯一 SQL，再按 Exact/Similar/Plan 填 watch_text。
                if *type_specified {
                    let sql = single_sql(value)?;
                    record.watch_text = match watch {
                        WatchType::None => return Err("watch type must be specified".to_string()),
                        WatchType::Exact => sql,
                        WatchType::Similar => normalize_digest(&sql),
                        WatchType::Plan => digester.plan_digest(&sql)?,
                    };
                    // 未指定 type：Go 只检查长度并原样保存调用方提供的 digest。
                } else {
                    if value.len() != 64 {
                        return Err("digest format error".to_string());
                    }
                    record.watch_text = value.clone();
                }
            }
        }
    }
    Ok(record)
}

/// 补默认资源组、校验资源组存在性，并补齐/检查 action 与 watch type。
pub fn validate_watch_record(
    record: &mut QuarantineRecord,
    controller: &dyn ResourceGroupController,
) -> Result<(), String> {
    // 空资源组名回落到 default。
    if record.resource_group.is_empty() {
        record.resource_group = DEFAULT_RESOURCE_GROUP.to_string();
    }
    let group = controller
        .resource_group(&record.resource_group)?
        .ok_or_else(|| format!("the group {} does not exist", record.resource_group))?;
    // 未显式指定动作时，从资源组 runaway 默认配置补齐。
    if record.action == RunawayAction::None {
        let (action, switch) = group.default_action.ok_or_else(|| {
            format!(
                "must set runaway config for resource group `{}`",
                record.resource_group
            )
        })?;
        record.action = action;
        record.switch_group = switch;
    }
    if record.watch == WatchType::None {
        return Err("must specify watch type".to_string());
    }
    Ok(())
}

/// ADD QUERY WATCH 执行器：一次性构造、校验并注册规则，返回规则 ID。
pub struct AddExecutor<'a> {
    /// 语句中的选项列表。
    pub options: Vec<QueryWatchOption>,
    /// 资源组控制器。
    pub controller: &'a dyn ResourceGroupController,
    /// runaway 管理器。
    pub manager: &'a dyn RunawayManager,
    /// 计划 digest 计算器。
    pub digester: &'a dyn PlanDigester,
    /// 是否已执行过，防止重复 Next。
    done: bool,
}
/// ADD QUERY WATCH 执行器方法。
impl<'a> AddExecutor<'a> {
    /// 构造尚未执行的 AddExecutor。
    pub fn new(
        options: Vec<QueryWatchOption>,
        controller: &'a dyn ResourceGroupController,
        manager: &'a dyn RunawayManager,
        digester: &'a dyn PlanDigester,
    ) -> Self {
        Self {
            options,
            controller,
            manager,
            digester,
            done: false,
        }
    }
    /// 对应 Executor::Next：首次调用完成注册并返回 ID，之后返回 None。
    pub fn next(&mut self) -> Result<Option<u64>, String> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        let mut record = from_option_list(&self.options, self.digester)?;
        validate_watch_record(&mut record, self.controller)?;
        self.manager.add_watch(record).map(Some)
    }
}

/// DROP QUERY WATCH 的目标：按组名、用户变量或规则 ID。
pub enum DropQueryWatch {
    /// 按资源组名删除。
    Group(String),
    /// 从用户变量读取组名再删除。
    GroupVariable(String),
    /// 按监视规则 ID 删除。
    Id(u64),
}
/// 执行 DROP QUERY WATCH：按语句形态分派到 manager 的删除接口。
pub fn exec_drop_query_watch(
    manager: &dyn RunawayManager,
    statement: DropQueryWatch,
    user_variables: &BTreeMap<String, String>,
) -> Result<(), String> {
    match statement {
        DropQueryWatch::Group(group) => manager.remove_group_watches(&group),
        DropQueryWatch::GroupVariable(variable) => user_variables
            .get(&variable)
            .ok_or_else(|| "invalid group name variable".to_string())
            .and_then(|group| manager.remove_group_watches(group)),
        DropQueryWatch::Id(id) => manager.remove_watch(id),
    }
}

/// 要求输入恰好一条非空 SQL，并返回 Go parser 写入 AST 的 statement text。
///
/// statement text 保留前置注释、空语句和终止分号，但排除终止分号后的
/// 空语句/注释；字符串、标识符及注释中的 `;` 不作为语句分隔符。
fn single_sql(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut offset = 0;
    let mut quote = None;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut statement_has_token = false;
    let mut statement_count = 0;
    let mut first_statement_end = 0;
    while offset < bytes.len() {
        if line_comment {
            if bytes[offset] == b'\n' {
                line_comment = false;
            }
            offset += 1;
            continue;
        }
        if block_comment {
            if bytes[offset..].starts_with(b"*/") {
                block_comment = false;
                offset += 2;
            } else {
                offset += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if bytes[offset] == b'\\' && delimiter != b'`' {
                offset = (offset + 2).min(bytes.len());
                continue;
            }
            if bytes[offset] == delimiter {
                if bytes.get(offset + 1) == Some(&delimiter) {
                    offset += 2;
                    continue;
                }
                quote = None;
            }
            offset += 1;
            continue;
        }
        if bytes[offset..].starts_with(b"--")
            && bytes
                .get(offset + 2)
                .is_none_or(|byte| byte.is_ascii_whitespace())
        {
            line_comment = true;
            offset += 2;
        } else if bytes[offset] == b'#' {
            line_comment = true;
            offset += 1;
        } else if bytes[offset..].starts_with(b"/*") {
            block_comment = true;
            offset += 2;
        } else if matches!(bytes[offset], b'\'' | b'"' | b'`') {
            statement_has_token = true;
            quote = Some(bytes[offset]);
            offset += 1;
        } else if bytes[offset] == b';' {
            if statement_has_token {
                statement_count += 1;
                if statement_count == 1 {
                    first_statement_end = offset + 1;
                } else {
                    return Err("only support one SQL".to_string());
                }
            }
            statement_has_token = false;
            offset += 1;
        } else {
            if !bytes[offset].is_ascii_whitespace() {
                statement_has_token = true;
            }
            offset += 1;
        }
    }
    if quote.is_some() || block_comment {
        return Err("invalid SQL syntax".to_string());
    }
    if statement_has_token {
        statement_count += 1;
        if statement_count == 1 {
            first_statement_end = value.len();
        }
    }
    if statement_count != 1 {
        return Err("only support one SQL".to_string());
    }

    // Scanner.stmtText trims at most one boundary newline, but preserves spaces.
    let mut start = 0;
    if bytes.first() == Some(&b'\n') {
        start = 1;
    }
    if bytes.get(first_statement_end.wrapping_sub(1)) == Some(&b'\n') {
        first_statement_end -= 1;
    }
    Ok(value[start..first_statement_end].to_string())
}

/// 使用 parser crate 的完整 Go 等价 token 归并规则计算 SQL digest。
fn normalize_digest(sql: &str) -> String {
    parser_digester::NormalizeDigest(sql).1.String().to_owned()
}

fn sha256_bytes(value: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (value.len() as u64) * 8;
    let mut message = value.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut hash = INITIAL;
    for chunk in message.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(
                chunk[index * 4..index * 4 + 4]
                    .try_into()
                    .expect("four bytes"),
            );
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(ROUND[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
        hash[5] = hash[5].wrapping_add(f);
        hash[6] = hash[6].wrapping_add(g);
        hash[7] = hash[7].wrapping_add(h);
    }
    let mut output = [0_u8; 32];
    for (index, word) in hash.into_iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}
