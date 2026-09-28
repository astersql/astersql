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

// 规划器单元测试套件（PlannerSuite）组装与 EXPLAIN 字段解析。
//
// 将 mock 表、session context、parser 配置打包成可复用的测试夹具；
// InfoSchema 在此以 `Vec<TableInfo>` 近似，待外部 crate 接通后替换。

// planner 测试套件的组装流程。
// parser、infoschema、sessionctx、domain 和 mock 是尚未接入 Rust crate 的外部类型。
//
// Go 语义：从 explain 文本中提取 prefix 后、空格前的字段值，并去掉逗号。
// pub fn get_field_value(prefix: &str, row: &str) -> String {
//     if let Some(index) = row.find(prefix) {
//         if index > 0 {
//             let start = index + prefix.len();
//             if let Some(end) = row[start..].find(' ') {
//                 if end > 0 {
//                     return row[start..start + end].trim_matches(',').to_owned();
//                 }
//             }
//         }
//     }
//     String::new()
// }
//
// Go 导出的 PlannerSuite：保存 parser、information schema、session context 和 plan context。
// pub struct PlannerSuite {
//     parser: parser::Parser,
//     info_schema: infoschema::InfoSchema,
//     session_ctx: sessionctx::Context,
//     plan_ctx: base::PlanContext,
// }
//
// 以下访问器保持 Go 的公开 API 语义；返回引用形态由后续 Rust 类型接线时确定。
// impl PlannerSuite {
//     pub fn get_parser(&self) -> &parser::Parser { &self.parser }
//     pub fn get_is(&self) -> &infoschema::InfoSchema { &self.info_schema }
//     pub fn get_sctx(&self) -> &sessionctx::Context { &self.session_ctx }
//     pub fn get_ctx(&self) -> &base::PlanContext { &self.plan_ctx }
//
// Go CreatePlannerSuite：用给定 session context 和 information schema 初始化套件。
//     pub fn create(sctx: sessionctx::Context, is: infoschema::InfoSchema) -> Self {
//         let parser = parser::Parser::new();
//         let plan_ctx = sctx.get_plan_ctx();
//         Self { parser, info_schema: is, session_ctx: sctx, plan_ctx }
//     }
//
// Go Close：关闭统计句柄；这里仅保留资源收尾的调用位置，不真正操作外部资源。
//     pub fn close(&mut self) {
// domain.GetDomain(ctx).StatsHandle().Close()。
//     }
// }
//
// Go CreatePlannerSuiteElems：供 core 包外部测试创建默认套件。
// pub fn create_planner_suite_elems() -> PlannerSuite {
//     create_planner_suite()
// }
//
// Go createPlannerSuite：按固定顺序创建 mock 表、分配分区 ID，并配置 parser/session 选项。
// pub fn create_planner_suite() -> PlannerSuite {
//     let mut table_infos = vec![
//         MockSignedTable(), MockUnsignedTable(), MockView(), MockNoPKTable(),
//         MockRangePartitionTable(), MockHashPartitionTable(), MockListPartitionTable(),
//         MockStateNoneColumnTable(), MockGlobalIndexHashPartitionTable(),
//     ];
//     let mut id: i64 = 1;
//     for table in &mut table_infos {
//         table.id = id;
//         id += 1;
// Go 中 nil partition 会跳过；用 Option 分支表达同一控制流。
//         if let Some(partition) = table.partition.as_mut() {
//             for definition in &mut partition.definitions {
//                 definition.id = id;
//                 id += 1;
//             }
//         }
//     }
//     let is = infoschema::mock_info_schema(table_infos);
//     let mut ctx = mock::new_context();
//     ctx.store = mock::Store { client: mock::Client {} };
//     ctx.session_vars_mut().current_db = "test".to_owned();
// Go 会创建统计句柄并绑定 domain；这里只记录顺序，避免网络/数据库副作用。
//     let domain = domain::new_mock_domain();
//     domain.create_stats_handle();
//     ctx.bind_domain_and_schema_validator(domain, None);
//     ctx.set_info_schema(is.clone());
//     ctx.session_vars_mut().enable_window_function = true;
//     let mut p = parser::Parser::new();
//     p.set_parser_config(parser::ParserConfig {
//         enable_window_function: true,
//         enable_strict_double_type_check: true,
//     });
//     let plan_ctx = ctx.get_plan_ctx();
//     PlannerSuite { parser: p, info_schema: is, session_ctx: ctx, plan_ctx }
// }
//
// 这些表构造器来自同目录 mock.rs；名称和声明顺序与 Go 保持一致。
// use super::mock::{MockGlobalIndexHashPartitionTable, MockHashPartitionTable, MockListPartitionTable,
//     MockNoPKTable, MockRangePartitionTable, MockSignedTable, MockStateNoneColumnTable,
//     MockUnsignedTable, MockView};
// */
use crate::mock::{
    InfoSchema, MockContext, PlanContext, TableInfo, mock_context,
    mock_global_index_hash_partition_table, mock_hash_partition_table, mock_list_partition_table,
    mock_no_pk_table, mock_range_partition_table, mock_signed_table, mock_state_none_column_table,
    mock_unsigned_table, mock_view,
};

/// 从 EXPLAIN 行文本中提取 `prefix` 后、空格前的字段值，并去掉尾部逗号。
pub fn get_field_value(prefix: &str, row: &str) -> String {
    let Some(index) = row.find(prefix).filter(|index| *index > 0) else {
        return String::new();
    };
    let tail = &row[index + prefix.len()..];
    let Some((value, _)) = tail.split_once(' ').filter(|(value, _)| !value.is_empty()) else {
        return String::new();
    };
    value.trim_matches(',').to_string()
}

/// SQL 解析器配置桩：窗口函数与严格 DOUBLE 检查开关。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParserConfig {
    pub window_functions: bool,
    pub strict_double_type_check: bool,
}
/// 解析器桩，仅保存配置供套件持有。
#[derive(Clone, Debug)]
pub struct Parser {
    pub config: ParserConfig,
}

/// 规划器测试套件：聚合 parser、InfoSchema（表列表）与会话上下文。
pub struct PlannerSuite {
    parser: Parser,
    info_schema: InfoSchema,
    session_context: MockContext,
    plan_context: PlanContext,
    closed: bool,
}

impl PlannerSuite {
    /// 返回套件内的解析器引用。
    pub fn parser(&self) -> &Parser {
        &self.parser
    }
    /// Go `GetParser` 的 Rust 命名别名。
    pub fn get_parser(&self) -> &Parser {
        self.parser()
    }
    /// 返回 mock InfoSchema（表元信息切片）。
    pub fn info_schema(&self) -> &[TableInfo] {
        &self.info_schema.tables
    }
    /// Go `GetIS` 的 Rust 命名别名。
    pub fn get_is(&self) -> &InfoSchema {
        &self.info_schema
    }
    /// 返回 mock 会话上下文。
    pub fn session_context(&self) -> &MockContext {
        &self.session_context
    }
    /// Go `GetSCtx` 的 Rust 命名别名。
    pub fn get_sctx(&self) -> &MockContext {
        self.session_context()
    }
    /// 返回与 Go `GetCtx` 对应的规划上下文快照。
    pub fn plan_context(&self) -> &PlanContext {
        &self.plan_context
    }
    /// Go `GetCtx` 的 Rust 命名别名。
    pub fn get_ctx(&self) -> &PlanContext {
        self.plan_context()
    }
    /// 标记套件已关闭（对应 Go 关闭统计句柄）。
    pub fn close(&mut self) {
        self.closed = true;
        self.session_context.stats_handle_created = false;
    }
    /// 是否已调用 `close`。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// 用给定会话上下文与表列表创建 PlannerSuite；对应 Go 的公开构造器，保留默认 parser 配置。
pub fn create_planner_suite(
    session_context: MockContext,
    info_schema: impl Into<InfoSchema>,
) -> PlannerSuite {
    let info_schema = info_schema.into();
    let session_context = session_context;
    let plan_context = session_context.get_plan_context();
    PlannerSuite {
        parser: Parser {
            config: ParserConfig {
                window_functions: false,
                strict_double_type_check: false,
            },
        },
        info_schema,
        session_context,
        plan_context,
        closed: false,
    }
}

/// 创建默认测试套件：按固定顺序装入各类 mock 表并统一分配表/分区 ID。
pub fn create_planner_suite_elements() -> PlannerSuite {
    let mut tables = vec![
        mock_signed_table(),
        mock_unsigned_table(),
        mock_view(),
        mock_no_pk_table(),
        mock_range_partition_table(),
        mock_hash_partition_table(),
        mock_list_partition_table(),
        mock_state_none_column_table(),
        mock_global_index_hash_partition_table(),
    ];
    // 顺序分配全局 ID：先表后各分区定义，与 Go createPlannerSuite 一致。
    let mut id = 1;
    for table in &mut tables {
        table.id = id;
        id += 1;
        if let Some(partition) = &mut table.partition {
            for definition in &mut partition.definitions {
                definition.id = id;
                id += 1;
            }
        }
    }
    let info_schema = InfoSchema { tables };
    let mut context = mock_context();
    context.info_schema = Some(info_schema.clone());
    context.window_functions_enabled = true;
    let mut suite = create_planner_suite(context, info_schema);
    suite.parser.config.window_functions = true;
    suite.parser.config.strict_double_type_check = true;
    suite
}

/// Go `CreatePlannerSuiteElems` 的 Rust 命名别名。
pub fn create_planner_suite_elems() -> PlannerSuite {
    create_planner_suite_elements()
}

impl From<Vec<TableInfo>> for InfoSchema {
    fn from(tables: Vec<TableInfo>) -> Self {
        Self { tables }
    }
}
