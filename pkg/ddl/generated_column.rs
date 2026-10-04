// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 生成列（Generated Column）相关的 DDL 校验逻辑。
//
// 生成列是指其值由表达式根据同一行的其他列计算得出的列，分为
// 虚拟（VIRTUAL，读取时计算）与存储（STORED，写入时计算并持久化）两种。
// 本模块负责在 DDL（数据定义语言，如 CREATE/ALTER TABLE）阶段校验：
// - 生成列的依赖关系是否合法（只能依赖定义在其之前的生成列）；
// - 生成表达式中是否使用了非法结构（子查询、聚合函数、窗口函数等）；
// - 生成列是否引用了自增列；
// - 修改生成列定义时的各种限制（存储属性、被索引引用等）。
//
// 表达式索引（expression index）也复用这里的校验逻辑，因为其底层
// 实现同样是隐藏的虚拟生成列。

use std::collections::{HashMap, HashSet};

use crate::column::{ColumnInfo, ColumnPosition, TableInfo};

/// 描述一个列在生成列校验中所需的属性快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationAttribute {
    /// 列在表中的位置（偏移量），用于判断依赖列是否定义在前。
    pub position: usize,
    /// 该列是否为生成列。
    pub generated: bool,
    /// 生成表达式所依赖的列名集合。
    pub dependencies: HashSet<String>,
}

/// 生成表达式的使用场景：作为生成列，还是作为表达式索引。
///
/// 两种场景对函数的可用性要求不同：表达式索引要求函数结果在
/// 版本升级间保证稳定（guaranteed available），否则需要显式开启开关。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationType {
    /// 生成列场景。
    Column,
    /// 表达式索引场景。
    Index,
}

/// 生成表达式的抽象语法树节点（简化版 AST）。
///
/// 只保留生成列校验所需的结构信息，用于遍历表达式并检查
/// 其中出现的列引用与非法结构。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionNode {
    /// 列引用，携带列名。
    Column(String),
    /// 函数调用。
    Function {
        /// 函数名。
        name: String,
        /// 该函数是否允许出现在生成表达式中。
        supported: bool,
        /// 函数结果是否保证跨版本稳定（表达式索引需要此保证）。
        guaranteed_available: bool,
        /// 函数参数列表。
        arguments: Vec<ExpressionNode>,
    },
    /// 聚合函数（如 SUM/COUNT），生成表达式中禁止使用。
    Aggregate(Vec<ExpressionNode>),
    /// 行值构造器（如 (a, b)），生成表达式中禁止使用。
    Row(Vec<ExpressionNode>),
    /// 窗口函数（如 ROW_NUMBER() OVER ...），生成表达式中禁止使用。
    Window(Vec<ExpressionNode>),
    /// CAST(... AS ... ARRAY) 数组转换，仅表达式索引（多值索引）允许。
    CastArray(Box<ExpressionNode>),
    /// 子查询，生成表达式中禁止使用。
    Subquery,
    /// 系统/用户变量引用，生成表达式中禁止使用。
    Variable,
    /// 字面量常量，始终合法。
    Literal,
}

/// 生成列校验过程中可能出现的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GeneratedColumnError {
    /// 引用了不存在的列。
    UnknownColumn(String),
    /// 生成列依赖了定义在其之后的生成列（必须依赖在前的列）。
    NonPriorColumn(String),
    /// 表达式中使用了不允许的函数。
    IllegalFunction(String),
    /// 表达式中使用了聚合函数。
    AggregateFunction,
    /// 表达式中使用了行值构造器，携带生成列名。
    RowValue(String),
    /// 表达式中使用了窗口函数，携带生成列名。
    WindowFunction(String),
    /// 生成列表达式中使用了 CAST ... ARRAY（仅表达式索引允许）。
    CastArray,
    /// 表达式索引使用了不保证稳定的函数且未开启相应开关。
    UnsupportedExpressionIndex,
    /// 修改生成列时改变了 VIRTUAL/STORED 存储属性。
    StoredStatusChanged,
    /// 不允许修改存储（STORED）生成列的表达式。
    StoredColumnModification,
    /// 不允许修改被索引引用的生成列。
    IndexedColumnModification,
    /// 生成列引用了自增（AUTO_INCREMENT）列，携带生成列名。
    AutoIncrementReference(String),
}

/// 校验单个列的生成依赖是否合法。
///
/// 规则：生成列只能依赖已存在的列；若依赖的列同样是生成列，
/// 则该依赖列必须定义在当前列之前（位置更靠前），以保证按列序
/// 依次计算时依赖值已就绪。
pub fn verify_column_generation(
    columns: &HashMap<String, GenerationAttribute>,
    column_name: &str,
) -> Result<(), GeneratedColumnError> {
    let Some(attribute) = columns.get(column_name) else {
        return Err(GeneratedColumnError::UnknownColumn(column_name.into()));
    };
    // 非生成列无需校验依赖。
    if !attribute.generated {
        return Ok(());
    }
    for dependency in &attribute.dependencies {
        let depended = columns
            .get(dependency)
            .ok_or_else(|| GeneratedColumnError::UnknownColumn(dependency.clone()))?;
        // 依赖的生成列必须位于当前列之前，否则报错。
        if depended.generated && attribute.position <= depended.position {
            return Err(GeneratedColumnError::NonPriorColumn(dependency.clone()));
        }
    }
    Ok(())
}

/// 根据 ALTER TABLE 中的位置说明（FIRST/AFTER col/默认末尾），
/// 计算新列插入后的目标偏移量。
pub fn find_position_relative_column(
    columns: &[ColumnInfo],
    position: &ColumnPosition,
) -> Result<usize, GeneratedColumnError> {
    match position {
        // 未指定位置：追加到表尾。
        ColumnPosition::None => Ok(columns.len()),
        // FIRST：放在最前面。
        ColumnPosition::First => Ok(0),
        // AFTER name：放在指定列之后，找不到该列则报错。
        ColumnPosition::After(name) => columns
            .iter()
            .find(|column| column.name == *name)
            .map(|column| column.offset + 1)
            .ok_or_else(|| GeneratedColumnError::UnknownColumn(name.clone())),
    }
}

/// 检查生成表达式依赖的列是否都存在于表中。
///
/// 通过从依赖集合中逐个移除表里已有的可见列（隐藏列不参与），
/// 若最终仍有剩余，说明引用了不存在的列。注意本函数会就地清空
/// 已匹配的依赖项。
pub fn check_depended_columns_exist(
    dependencies: &mut HashSet<String>,
    columns: &[ColumnInfo],
) -> Result<(), GeneratedColumnError> {
    for column in columns.iter().filter(|column| !column.hidden) {
        dependencies.remove(&column.name);
    }
    // 集合非空说明还有未匹配的依赖，即引用了未知列。
    dependencies.iter().next().cloned().map_or(Ok(()), |name| {
        Err(GeneratedColumnError::UnknownColumn(name))
    })
}

/// 校验新增单个生成列时的依赖顺序。
///
/// 先根据位置说明计算新列的目标偏移量，再确保其依赖的生成列
/// 都位于该偏移量之前，否则违反“只能依赖在前的生成列”规则。
pub fn verify_column_generation_single(
    dependencies: &HashSet<String>,
    columns: &[ColumnInfo],
    position: &ColumnPosition,
) -> Result<(), GeneratedColumnError> {
    let offset = find_position_relative_column(columns, position)?;
    for column in columns {
        // 依赖的生成列若位于新列位置之后（含同位），则不合法。
        if dependencies.contains(&column.name) && column.generated && column.offset >= offset {
            return Err(GeneratedColumnError::NonPriorColumn(column.name.clone()));
        }
    }
    Ok(())
}

/// 收集表达式中引用的全部列名（统一转为小写，SQL 列名不区分大小写）。
pub fn find_column_names_in_expr(expression: &ExpressionNode) -> HashSet<String> {
    // 递归遍历 AST，将遇到的列引用记入集合。
    fn visit(expression: &ExpressionNode, names: &mut HashSet<String>) {
        match expression {
            ExpressionNode::Column(name) => {
                names.insert(name.to_ascii_lowercase());
            }
            ExpressionNode::Function { arguments, .. }
            | ExpressionNode::Aggregate(arguments)
            | ExpressionNode::Row(arguments)
            | ExpressionNode::Window(arguments) => {
                for argument in arguments {
                    visit(argument, names);
                }
            }
            ExpressionNode::CastArray(inner) => visit(inner, names),
            ExpressionNode::Subquery | ExpressionNode::Variable | ExpressionNode::Literal => {}
        }
    }
    let mut names = HashSet::new();
    visit(expression, &mut names);
    names
}

/// 检查生成表达式中是否使用了非法结构。
///
/// 非法结构包括：不受支持的函数、GROUPING 函数、子查询、变量、
/// 聚合函数、行值构造器、窗口函数等；此外按场景区分：
/// - 表达式索引要求函数结果跨版本稳定，否则需开启 `expression_index_enabled`；
/// - CAST ... ARRAY 仅表达式索引（多值索引）允许，生成列禁止。
pub fn check_illegal_function_for_generated(
    column_name: &str,
    generation_type: GenerationType,
    expression: &ExpressionNode,
    expression_index_enabled: bool,
) -> Result<(), GeneratedColumnError> {
    // 递归检查每个 AST 节点是否触犯上述限制。
    fn inspect(
        node: &ExpressionNode,
        generation_type: GenerationType,
        expression_index_enabled: bool,
        allow_array_cast: bool,
    ) -> Result<(), GeneratedColumnError> {
        match node {
            ExpressionNode::Function {
                name,
                supported,
                guaranteed_available,
                arguments,
            } => {
                // Go 将 GROUPING 归入聚合函数错误，而不是普通非法函数。
                if name.eq_ignore_ascii_case("grouping") {
                    return Err(GeneratedColumnError::AggregateFunction);
                }
                if !supported {
                    return Err(GeneratedColumnError::IllegalFunction(name.clone()));
                }
                // 表达式索引场景要求函数结果稳定，否则需要显式开启开关。
                if generation_type == GenerationType::Index
                    && !guaranteed_available
                    && !expression_index_enabled
                {
                    return Err(GeneratedColumnError::UnsupportedExpressionIndex);
                }
                for argument in arguments {
                    inspect(argument, generation_type, expression_index_enabled, false)?;
                }
            }
            ExpressionNode::Subquery | ExpressionNode::Variable => {
                return Err(GeneratedColumnError::IllegalFunction("expression".into()));
            }
            ExpressionNode::Aggregate(_) => return Err(GeneratedColumnError::AggregateFunction),
            ExpressionNode::Row(_) => {
                return Err(GeneratedColumnError::RowValue(String::new()));
            }
            ExpressionNode::Window(_) => {
                return Err(GeneratedColumnError::WindowFunction(String::new()));
            }
            ExpressionNode::CastArray(inner) => {
                // 与 Go visitor 一致：数组 CAST 仅能作为表达式索引的根表达式；
                // 位于函数参数或另一个数组 CAST 下时也必须拒绝。
                if generation_type == GenerationType::Column || !allow_array_cast {
                    return Err(GeneratedColumnError::CastArray);
                }
                inspect(inner, generation_type, expression_index_enabled, false)?;
            }
            ExpressionNode::Column(_) | ExpressionNode::Literal => {}
        }
        Ok(())
    }

    // 递归检查完成后，为需要携带列名的错误补充实际的生成列名。
    inspect(expression, generation_type, expression_index_enabled, true).map_err(
        |error| match error {
            GeneratedColumnError::RowValue(_) => GeneratedColumnError::RowValue(column_name.into()),
            GeneratedColumnError::WindowFunction(_) => {
                GeneratedColumnError::WindowFunction(column_name.into())
            }
            other => other,
        },
    )
}

/// 查找表中是否存在依赖指定列的生成列。
///
/// 用于删除/修改列前的检查：若某生成列（含表达式索引对应的
/// 隐藏列）依赖该列，则返回其名称与是否为隐藏列。
pub fn has_dependent_generated_column<'a>(
    table: &'a TableInfo,
    column_name: &str,
) -> Option<(&'a str, bool)> {
    table.columns.iter().find_map(|column| {
        column
            .dependencies
            .contains(column_name)
            .then_some((column.name.as_str(), column.hidden))
    })
}

/// 检查生成列是否引用了自增（AUTO_INCREMENT）列。
///
/// 自增列的值在插入时由系统分配，生成表达式引用它会导致
/// 计算时机不确定，因此被禁止。
pub fn check_auto_increment_reference(
    generated_name: &str,
    dependencies: &HashSet<String>,
    table: &TableInfo,
) -> Result<(), GeneratedColumnError> {
    if table
        .columns
        .iter()
        .any(|column| column.auto_increment && dependencies.contains(&column.name))
    {
        Err(GeneratedColumnError::AutoIncrementReference(
            generated_name.into(),
        ))
    } else {
        Ok(())
    }
}

/// 校验 ALTER TABLE MODIFY/CHANGE 修改生成列时的限制。
///
/// 规则依次为：
/// 1. 不允许改变 VIRTUAL/STORED 存储属性；
/// 2. 表达式未变时，仅当类型改变且列被索引引用才拒绝；
/// 3. 不允许修改存储（STORED）生成列的表达式；
/// 4. 不允许修改被索引引用的生成列的表达式；
/// 5. 用新列定义替换旧列后，重新校验全表的生成依赖顺序。
pub fn check_modify_generated_column(
    table: &TableInfo,
    old_column: &ColumnInfo,
    new_column: &ColumnInfo,
    indexed: bool,
) -> Result<(), GeneratedColumnError> {
    // 规则 1：VIRTUAL 与 STORED 不允许互相转换。
    if old_column.generated_stored != new_column.generated_stored {
        return Err(GeneratedColumnError::StoredStatusChanged);
    }
    // 规则 2：表达式未变时，只需关注类型变化对索引的影响。
    if old_column.generated_expression == new_column.generated_expression {
        if old_column.field_type == new_column.field_type || !indexed {
            return Ok(());
        }
        return Err(GeneratedColumnError::IndexedColumnModification);
    }
    // 规则 3：存储生成列的表达式不可修改。
    if new_column.generated_stored {
        return Err(GeneratedColumnError::StoredColumnModification);
    }
    // 规则 4：被索引引用的生成列表达式不可修改。
    if indexed {
        return Err(GeneratedColumnError::IndexedColumnModification);
    }
    // 规则 5：以新列定义替换旧列，构造属性快照并重新校验全表依赖顺序。
    let mut attributes = HashMap::with_capacity(table.columns.len());
    for column in &table.columns {
        let candidate = if column.id == old_column.id {
            new_column
        } else {
            column
        };
        attributes.insert(
            candidate.name.clone(),
            GenerationAttribute {
                position: candidate.offset,
                generated: candidate.generated,
                dependencies: candidate.dependencies.clone(),
            },
        );
    }
    for name in attributes.keys() {
        verify_column_generation(&attributes, name)?;
    }
    Ok(())
}

/// EMBED_TEXT has a dedicated STORED form; nested calls and virtual columns
/// remain unsupported even though the ordinary generated-column checker admits it.
pub fn check_embed_text_generated_column(
    name: &str,
    expr: &astersql_parser_ast::ExprNode,
    stored: bool,
) -> Result<(), astersql_parser::errors::Error> {
    use astersql_expression::{
        CheckEmbedTextAllowed, ContainsEmbedTextFunc, ExtractEmbedTextInfo, IsEmbedTextFuncCall,
    };
    let error = |message: String| {
        astersql_parser::errors::New(format!(
            "[ddl:3106]'{message}' is not supported for generated columns."
        ))
    };
    if !ContainsEmbedTextFunc(Some(expr)) {
        return Ok(());
    }
    CheckEmbedTextAllowed().map_err(|err| error(err.to_string()))?;
    if !IsEmbedTextFuncCall(expr) {
        return Err(error(
            "using EMBED_TEXT() as a nested expression inside other functions or expressions"
                .into(),
        ));
    }
    if !stored {
        return Err(error(
            "using EMBED_TEXT() in a virtual generated column".into(),
        ));
    }
    ExtractEmbedTextInfo(expr).map_err(|err| {
        error(format!(
            "EMBED_TEXT() usage in generated column '{name}': {err}"
        ))
    })?;
    Ok(())
}
pub fn embed_text_dependency_error(name: &str, dependency: &str) -> astersql_parser::errors::Error {
    astersql_parser::errors::New(format!(
        "[ddl:3106]'generated column '{name}' depends on generated column '{dependency}' that uses EMBED_TEXT()' is not supported for generated columns."
    ))
}

/// The inference functions stay blocked in functional indexes. Only the
/// explicitly validated generated-column EMBED_TEXT form is admitted.
pub fn check_embedding_function_usage(
    name: &str,
    expr: &astersql_parser_ast::ExprNode,
    functional_index: bool,
) -> Result<(), astersql_parser::errors::Error> {
    use astersql_parser_ast as ast;
    #[derive(Default)]
    struct Checker {
        functional_index: bool,
        embedding_expression: bool,
        blocked: bool,
        aggregate: bool,
        row: bool,
        window: bool,
        cast_array: bool,
        other_error: Option<String>,
    }
    impl ast::ExprNodeVisitor for Checker {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            let skip = match &input.Kind {
                ast::ExprKind::Function { FnName, Args, .. } => {
                    if self.embedding_expression && FnName.L == "grouping" {
                        self.aggregate = true;
                        true
                    } else if FnName.L.starts_with("vec_embed_")
                        || (self.functional_index && FnName.L == "embed_text")
                        || (self.embedding_expression
                            && ((astersql_expression::is_illegal_generated_column_function(
                                &FnName.L,
                            ) && FnName.L != "embed_text")
                                || !astersql_expression::formal_registry::IsFunctionSupported(
                                    &FnName.L,
                                )
                                || FnName.L == "values"))
                    {
                        self.blocked = true;
                        true
                    } else if self.embedding_expression {
                        match astersql_expression::formal_registry::VerifyArgsWrapper(
                            &FnName.L,
                            Args.len(),
                        ) {
                            Ok(()) => false,
                            Err(error) => {
                                self.other_error = Some(error.to_string());
                                true
                            }
                        }
                    } else {
                        false
                    }
                }
                ast::ExprKind::Variable { .. } | ast::ExprKind::Subquery { .. }
                    if self.embedding_expression =>
                {
                    self.blocked = true;
                    true
                }
                ast::ExprKind::AggregateFunction { .. } if self.embedding_expression => {
                    self.aggregate = true;
                    true
                }
                ast::ExprKind::Row(_) if self.embedding_expression => {
                    self.row = true;
                    true
                }
                ast::ExprKind::WindowFunction { .. } if self.embedding_expression => {
                    self.window = true;
                    true
                }
                ast::ExprKind::Cast { Tp, .. } if self.embedding_expression => {
                    self.cast_array |= Tp.IsArray();
                    false
                }
                _ => false,
            };
            (input.clone(), skip)
        }
        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }
    // Applying the existing checker to newly admitted EMBED_TEXT expressions
    // must not make unsafe arguments legal. Ordinary non-inference validation
    // remains owned by the existing generated-column paths.
    let mut checker = Checker {
        functional_index,
        embedding_expression: astersql_expression::ContainsEmbedTextFunc(Some(expr)),
        ..Default::default()
    };
    expr.Accept(&mut checker);
    let message = if checker.blocked {
        Some(if functional_index {
            format!(
                "[ddl:3758]Expression of expression index '{name}' contains a disallowed function"
            )
        } else {
            format!(
                "[ddl:3102]Expression of generated column '{name}' contains a disallowed function."
            )
        })
    } else if checker.aggregate {
        Some("[ddl:1111]Invalid use of group function".into())
    } else if checker.row {
        Some(if functional_index {
            format!("[ddl:3800]Expression of expression index '{name}' cannot refer to a row value")
        } else {
            format!("[ddl:3764]Expression of generated column '{name}' cannot refer to a row value")
        })
    } else if checker.window {
        Some(format!(
            "[ddl:3593]You cannot use the window function '{name}' in this context.'"
        ))
    } else if checker.other_error.is_some() {
        checker.other_error
    } else if !functional_index && checker.cast_array {
        Some("Use of CAST( .. AS .. ARRAY) outside of functional index in CREATE(non-SELECT)/ALTER TABLE or in general expressions".into())
    } else {
        None
    };
    message.map_or(Ok(()), |message| Err(astersql_parser::errors::New(message)))
}
