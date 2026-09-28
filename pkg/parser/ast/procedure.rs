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
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// 存储过程 AST 节点与 SQL restore（还原为语句文本）。
//
// 对照 procedure.go：参数模式、DECLARE、BEGIN/END 块、IF/CASE、循环、
// 游标、错误处理器、标签与 LEAVE/ITERATE 跳转。

/// IN 参数：调用方传入。
pub const MODE_IN: i32 = 0;
/// OUT 参数：过程写出。
pub const MODE_OUT: i32 = 1;
/// INOUT 参数：双向。
pub const MODE_INOUT: i32 = 2;
/// 错误处理：CONTINUE（继续执行）。
pub const PROCEDUR_CONTINUE: i32 = 0;
/// 错误处理：EXIT（退出当前块）。
pub const PROCEDUR_EXIT: i32 = 1;
/// 条件类别：SQLWARNING。
pub const PROCEDUR_SQLWARNING: i32 = 0;
/// 条件类别：NOT FOUND（如游标耗尽）。
pub const PROCEDUR_NOT_FOUND: i32 = 1;
/// 条件类别：SQLEXCEPTION。
pub const PROCEDUR_SQLEXCEPTION: i32 = 2;
/// 条件列表结束标记。
pub const PROCEDUR_END: i32 = 3;

/// restore 结果：成功返回 SQL 片段，失败返回错误信息。
pub type RestoreResult = Result<String, String>;

/// 可还原为过程相关 SQL 文本的节点。
pub trait ProcedureNode {
    fn restore(&self) -> RestoreResult;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 原始 SQL 片段占位节点，restore 原样输出。
pub struct RawProcedureNode(pub String);
impl ProcedureNode for RawProcedureNode {
    fn restore(&self) -> RestoreResult {
        Ok(self.0.clone())
    }
}

/// 反引号引用标识符，内部反引号加倍转义。
fn name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
/// 单引号字符串字面量，内部单引号加倍转义。
fn string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
/// 依次 restore 多个节点并追加 suffix（通常为分号）。
fn restore_many(nodes: &[Box<dyn ProcedureNode>], suffix: &str) -> RestoreResult {
    let mut out = String::new();
    for node in nodes {
        out.push_str(&node.restore()?);
        out.push_str(suffix);
    }
    Ok(out)
}

/// DECLARE 类声明节点标记。
pub trait DeclNode: ProcedureNode {}
/// 错误条件节点标记。
pub trait ErrNode: ProcedureNode {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 声明信息占位（与 Go 结构对齐）。
pub struct ProcedureDeclInfo;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 错误条件占位（与 Go 结构对齐）。
pub struct ProcedureErrorCondition;

/// 标签信息：起止名、是否块、以及被标签包裹的主体。
pub trait LabelInfo {
    fn get_error_status(&self) -> (&str, bool);
    fn get_label_name(&self) -> &str;
    fn is_block(&self) -> bool;
    fn get_block(&self) -> &dyn ProcedureNode;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 存储过程参数：模式、类型与名称。
pub struct StoreParameter {
    pub param_status: i32,
    pub param_type: String,
    pub param_name: String,
}
impl StoreParameter {
    /// 构造参数；status 为 MODE_IN/OUT/INOUT。
    pub fn new(status: i32, param_name: impl Into<String>, param_type: impl Into<String>) -> Self {
        Self {
            param_status: status,
            param_type: param_type.into(),
            param_name: param_name.into(),
        }
    }
    /// 还原为含模式前缀的参数片段。
    pub fn restore(&self) -> String {
        // 未知模式输出空前缀，与 Go 一致。
        let mode = match self.param_status {
            MODE_IN => " IN ",
            MODE_OUT => " OUT ",
            MODE_INOUT => " INOUT ",
            _ => "",
        };
        format!("{mode}{} {}", name(&self.param_name), self.param_type)
    }
}

/// DECLARE 变量：名列表、类型与可选 DEFAULT。
pub struct ProcedureDecl {
    pub decl_names: Vec<String>,
    pub decl_type: String,
    pub decl_default: Option<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for ProcedureDecl {
    fn restore(&self) -> RestoreResult {
        let names = self
            .decl_names
            .iter()
            .map(|v| name(v))
            .collect::<Vec<_>>()
            .join(",");
        let default = match &self.decl_default {
            Some(v) => format!(
                " DEFAULT {}",
                v.restore()
                    .map_err(|error| format!("An error occur while restore expr: {error}"))?
            ),
            None => String::new(),
        };
        Ok(format!("DECLARE {names} {}{default}", self.decl_type))
    }
}
impl DeclNode for ProcedureDecl {}

/// BEGIN … END 块：局部声明后接过程语句。
pub struct ProcedureBlock {
    pub procedure_vars: Vec<Box<dyn DeclNode>>,
    pub procedure_proc_stmts: Vec<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for ProcedureBlock {
    fn restore(&self) -> RestoreResult {
        let declarations = self
            .procedure_vars
            .iter()
            .map(|v| v.restore())
            .collect::<Result<Vec<_>, _>>()?
            .join(";");
        let statements = self
            .procedure_proc_stmts
            .iter()
            .map(|v| v.restore())
            .collect::<Result<Vec<_>, _>>()?
            .join(";");
        // 声明与语句段各自以分号收尾，再包进 BEGIN/END。
        let mut body = String::new();
        if !declarations.is_empty() {
            body.push_str(&declarations);
            body.push(';');
        }
        if !statements.is_empty() {
            body.push_str(&statements);
            body.push(';');
        }
        Ok(format!("BEGIN {body} END"))
    }
}

/// CREATE PROCEDURE 定义：可选 IF NOT EXISTS、参数列表与过程体。
pub struct ProcedureInfo {
    pub if_not_exists: bool,
    pub procedure_name: String,
    pub procedure_param: Vec<StoreParameter>,
    pub procedure_body: Box<dyn ProcedureNode>,
    pub procedure_param_str: String,
}
impl ProcedureNode for ProcedureInfo {
    fn restore(&self) -> RestoreResult {
        let params = self
            .procedure_param
            .iter()
            .map(StoreParameter::restore)
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!(
            "CREATE PROCEDURE {}{}({params}) {}",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            name(&self.procedure_name),
            self.procedure_body.restore()?
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DROP PROCEDURE，可选 IF EXISTS。
pub struct DropProcedureStmt {
    pub if_exists: bool,
    pub procedure_name: String,
}
impl ProcedureNode for DropProcedureStmt {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "DROP PROCEDURE {}{}",
            if self.if_exists { "IF EXISTS " } else { "" },
            name(&self.procedure_name)
        ))
    }
}

/// IF … END IF 顶层语句。
pub struct ProcedureIfInfo {
    pub if_body: Box<ProcedureIfBlock>,
}
impl ProcedureNode for ProcedureIfInfo {
    fn restore(&self) -> RestoreResult {
        Ok(format!("IF {}END IF", self.if_body.restore()?))
    }
}
/// ELSEIF 子句，内嵌另一 IF 块。
pub struct ProcedureElseIfBlock {
    pub procedure_if_stmt: Box<ProcedureIfBlock>,
}
impl ProcedureNode for ProcedureElseIfBlock {
    fn restore(&self) -> RestoreResult {
        Ok(format!("ELSEIF {}", self.procedure_if_stmt.restore()?))
    }
}
/// ELSE 子句，含语句列表。
pub struct ProcedureElseBlock {
    pub procedure_if_stmts: Vec<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for ProcedureElseBlock {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "ELSE {}",
            restore_many(&self.procedure_if_stmts, ";")?
        ))
    }
}
/// IF 条件块：THEN 语句 + 可选 ELSE/ELSEIF。
pub struct ProcedureIfBlock {
    pub if_expr: Box<dyn ProcedureNode>,
    pub procedure_if_stmts: Vec<Box<dyn ProcedureNode>>,
    pub procedure_else_stmt: Option<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for ProcedureIfBlock {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "{} THEN {}{}",
            self.if_expr.restore()?,
            restore_many(&self.procedure_if_stmts, ";")?,
            match &self.procedure_else_stmt {
                Some(v) => v.restore()?,
                None => String::new(),
            }
        ))
    }
}

/// 简单 CASE 的 WHEN … THEN 分支。
pub struct SimpleWhenThenStmt {
    pub expr: Box<dyn ProcedureNode>,
    pub procedure_stmts: Vec<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for SimpleWhenThenStmt {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "WHEN {} THEN {}",
            self.expr.restore()?,
            restore_many(&self.procedure_stmts, ";")?
        ))
    }
}
/// 简单 CASE：CASE expr WHEN … END CASE。
pub struct SimpleCaseStmt {
    pub condition: Box<dyn ProcedureNode>,
    pub when_cases: Vec<SimpleWhenThenStmt>,
    pub else_cases: Option<Vec<Box<dyn ProcedureNode>>>,
}
impl ProcedureNode for SimpleCaseStmt {
    fn restore(&self) -> RestoreResult {
        let when = self
            .when_cases
            .iter()
            .map(ProcedureNode::restore)
            .collect::<Result<Vec<_>, _>>()?
            .join("");
        let otherwise = match &self.else_cases {
            Some(v) => format!(" ELSE {}", restore_many(v, ";")?),
            None => String::new(),
        };
        Ok(format!(
            "CASE {} {when}{otherwise} END CASE",
            self.condition.restore()?
        ))
    }
}
/// 搜索 CASE 的 WHEN 条件分支。
pub struct SearchWhenThenStmt {
    pub expr: Box<dyn ProcedureNode>,
    pub procedure_stmts: Vec<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for SearchWhenThenStmt {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "WHEN {} THEN {}",
            self.expr.restore()?,
            restore_many(&self.procedure_stmts, ";")?
        ))
    }
}
/// 搜索 CASE：CASE WHEN cond THEN … END CASE。
pub struct SearchCaseStmt {
    pub when_cases: Vec<SearchWhenThenStmt>,
    pub else_cases: Option<Vec<Box<dyn ProcedureNode>>>,
}
impl ProcedureNode for SearchCaseStmt {
    fn restore(&self) -> RestoreResult {
        let when = self
            .when_cases
            .iter()
            .map(ProcedureNode::restore)
            .collect::<Result<Vec<_>, _>>()?
            .join("");
        let otherwise = match &self.else_cases {
            Some(v) => format!(" ELSE {}", restore_many(v, ";")?),
            None => String::new(),
        };
        Ok(format!("CASE {when}{otherwise} END CASE"))
    }
}

/// REPEAT … UNTIL … END REPEAT 循环。
pub struct ProcedureRepeatStmt {
    pub body: Vec<Box<dyn ProcedureNode>>,
    pub condition: Box<dyn ProcedureNode>,
}
impl ProcedureNode for ProcedureRepeatStmt {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "REPEAT {}UNTIL {} END REPEAT",
            restore_many(&self.body, ";")?,
            self.condition.restore()?
        ))
    }
}
/// WHILE … DO … END WHILE 循环。
pub struct ProcedureWhileStmt {
    pub condition: Box<dyn ProcedureNode>,
    pub body: Vec<Box<dyn ProcedureNode>>,
}
impl ProcedureNode for ProcedureWhileStmt {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "WHILE {} DO {}END WHILE",
            self.condition.restore()?,
            restore_many(&self.body, ";")?
        ))
    }
}

/// DECLARE cursor CURSOR FOR select。
pub struct ProcedureCursor {
    pub cur_name: String,
    pub select_string: Box<dyn ProcedureNode>,
}
impl ProcedureNode for ProcedureCursor {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "DECLARE {} CURSOR FOR {}",
            self.cur_name,
            self.select_string.restore()?
        ))
    }
}
impl DeclNode for ProcedureCursor {}
/// DECLARE HANDLER FOR 条件列表 + 处理动作。
pub struct ProcedureErrorControl {
    pub control_handle: i32,
    pub error_con: Vec<Box<dyn ErrNode>>,
    pub operate: Box<dyn ProcedureNode>,
}
impl ProcedureNode for ProcedureErrorControl {
    fn restore(&self) -> RestoreResult {
        // CONTINUE/EXIT；未知值输出空控制字。
        let control = match self.control_handle {
            PROCEDUR_CONTINUE => "CONTINUE ",
            PROCEDUR_EXIT => "EXIT ",
            _ => "",
        };
        let conditions = self
            .error_con
            .iter()
            .map(|v| v.restore())
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        Ok(format!(
            "DECLARE {control}HANDLER FOR {conditions} {}",
            self.operate.restore()?
        ))
    }
}
impl DeclNode for ProcedureErrorControl {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// OPEN 游标。
pub struct ProcedureOpenCur {
    pub cur_name: String,
}
impl ProcedureNode for ProcedureOpenCur {
    fn restore(&self) -> RestoreResult {
        Ok(format!("OPEN {}", self.cur_name))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// CLOSE 游标。
pub struct ProcedureCloseCur {
    pub cur_name: String,
}
impl ProcedureNode for ProcedureCloseCur {
    fn restore(&self) -> RestoreResult {
        Ok(format!("CLOSE {}", self.cur_name))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// FETCH cursor INTO 变量列表。
pub struct ProcedureFetchInto {
    pub cur_name: String,
    pub variables: Vec<String>,
}
impl ProcedureNode for ProcedureFetchInto {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "FETCH {} INTO {}",
            self.cur_name,
            self.variables.join(", ")
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 按错误号匹配的条件。
pub struct ProcedureErrorVal {
    pub error_num: u64,
}
impl ProcedureNode for ProcedureErrorVal {
    fn restore(&self) -> RestoreResult {
        Ok(self.error_num.to_string())
    }
}
impl ErrNode for ProcedureErrorVal {}
#[derive(Clone, Debug, Eq, PartialEq)]
/// SQLSTATE 条件。
pub struct ProcedureErrorState {
    pub code_status: String,
}
impl ProcedureNode for ProcedureErrorState {
    fn restore(&self) -> RestoreResult {
        Ok(format!("SQLSTATE {}", string(&self.code_status)))
    }
}
impl ErrNode for ProcedureErrorState {}
#[derive(Clone, Debug, Eq, PartialEq)]
/// SQLWARNING / NOT FOUND / SQLEXCEPTION 条件类别。
pub struct ProcedureErrorCon {
    pub error_con: i32,
}
impl ProcedureNode for ProcedureErrorCon {
    fn restore(&self) -> RestoreResult {
        Ok(match self.error_con {
            PROCEDUR_SQLWARNING => "SQLWARNING",
            PROCEDUR_NOT_FOUND => "NOT FOUND",
            PROCEDUR_SQLEXCEPTION => "SQLEXCEPTION",
            _ => "",
        }
        .into())
    }
}
impl ErrNode for ProcedureErrorCon {}

/// 标签：起止名须一致，否则 restore 报错。
pub struct ProcedureLabel {
    pub label_name: String,
    pub label_end: String,
    pub block: Box<dyn ProcedureNode>,
    pub is_block: bool,
}
impl ProcedureLabel {
    /// 用原始字符串体构造块标签（is_block=true）。
    pub fn new(begin: impl Into<String>, end: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            label_name: begin.into(),
            label_end: end.into(),
            block: Box::new(RawProcedureNode(body.into())),
            is_block: true,
        }
    }
    pub fn restore(&self) -> RestoreResult {
        let output = format!("{}: {}", name(&self.label_name), self.block.restore()?);
        // 起止标签名不一致视为错误，对齐 Go 校验。
        if self.label_name != self.label_end {
            return Err(format!(
                "the same label has different names,begin: {},end: {}",
                self.label_name, self.label_end
            ));
        }
        Ok(format!("{output} {}", name(&self.label_name)))
    }
}
impl LabelInfo for ProcedureLabel {
    fn get_error_status(&self) -> (&str, bool) {
        (&self.label_end, self.label_name != self.label_end)
    }
    fn get_label_name(&self) -> &str {
        &self.label_name
    }
    fn is_block(&self) -> bool {
        self.is_block
    }
    fn get_block(&self) -> &dyn ProcedureNode {
        self.block.as_ref()
    }
}

/// 块标签包装，实现 ProcedureNode。
pub struct ProcedureLabelBlock(pub ProcedureLabel);
impl ProcedureNode for ProcedureLabelBlock {
    fn restore(&self) -> RestoreResult {
        self.0.restore()
    }
}
/// 循环标签包装，实现 ProcedureNode。
pub struct ProcedureLabelLoop(pub ProcedureLabel);
impl ProcedureNode for ProcedureLabelLoop {
    fn restore(&self) -> RestoreResult {
        self.0.restore()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// LEAVE / ITERATE 跳转。
pub struct ProcedureJump {
    pub name: String,
    pub is_leave: bool,
}
impl ProcedureJump {
    /// is_leave=true 为 LEAVE，否则为 ITERATE。
    pub fn new(name: impl Into<String>, is_leave: bool) -> Self {
        Self {
            name: name.into(),
            is_leave,
        }
    }
}
impl ProcedureNode for ProcedureJump {
    fn restore(&self) -> RestoreResult {
        Ok(format!(
            "{} {}",
            if self.is_leave { "LEAVE" } else { "ITERATE" },
            string(&self.name)
        ))
    }
}
impl ProcedureJump {
    /// 不失败的便捷 restore（跳转文本恒成功）。
    pub fn restore(&self) -> String {
        ProcedureNode::restore(self).expect("infallible jump restore")
    }
}
