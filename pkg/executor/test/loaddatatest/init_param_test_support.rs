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

//! Unused execution services fail immediately if initialization starts reading data.
use astersql_executor_importer as importer;
use astersql_parser_ast as ast;
use std::sync::Arc;

struct ControllerServices;
impl importer::ColumnAssignmentFactory for ControllerServices {
    fn BuildAssignment(
        &self,
        _: &astersql_parser_ast::Assignment,
    ) -> Result<Arc<dyn importer::ColAssignExpressionBuilder>, String> {
        unreachable!()
    }
}
impl importer::ImportDatumConverter for ControllerServices {
    fn CastColumnValue(
        &self,
        _: astersql_lightning_backend_encode::Datum,
        _: &astersql_table::Column,
    ) -> Result<astersql_lightning_backend_encode::Datum, String> {
        unreachable!()
    }
    fn CurrentTime(
        &self,
        _: &astersql_table::Column,
    ) -> Result<astersql_lightning_backend_encode::Datum, String> {
        unreachable!()
    }
}
impl importer::ImportParserFactory for ControllerServices {
    fn NewParser(
        &self,
        _: &str,
        _: Box<dyn astersql_lightning_mydump::ReadSeekCloser>,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &importer::Plan,
    ) -> Result<Box<dyn astersql_lightning_mydump::Parser>, String> {
        unreachable!()
    }
}
impl importer::ImportSizeEstimator for ControllerServices {
    fn EstimateRealSize(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &dyn astersql_objstore_storeapi::Storage,
    ) -> Result<i64, String> {
        unreachable!()
    }
    fn ParquetExpansionRatio(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &str,
        _: i64,
        _: &dyn astersql_objstore_storeapi::Storage,
    ) -> Result<f64, String> {
        unreachable!()
    }
}
impl importer::ImportStorageFactory for ControllerServices {
    fn Open(
        &self,
        _: &astersql_objstore_storeapi::Context,
        _: &str,
        _: &str,
    ) -> Result<importer::SharedStorage, String> {
        unreachable!()
    }
}
impl importer::TiKVConfigProbe for ControllerServices {
    fn IsRaftKV2(&self) -> Result<bool, String> {
        unreachable!()
    }
}
impl importer::ImportResourceCalculator for ControllerServices {
    fn TargetNodeCPUCnt(&self) -> Result<usize, String> {
        unreachable!()
    }
    fn ScheduleTuneFactors(&self, _: &str) -> Result<importer::ScheduleTuneFactors, String> {
        unreachable!()
    }
    fn SampleIndexSizeRatio(
        &self,
        _: &importer::LoadDataController,
        _: &[u8],
    ) -> Result<f64, String> {
        unreachable!()
    }
    fn Calculate(
        &self,
        _: i64,
        _: usize,
        _: f64,
        _: importer::ScheduleTuneFactors,
    ) -> importer::ResourceParams {
        unreachable!()
    }
}
pub(super) fn services() -> importer::LoadDataControllerServices {
    let mock = Arc::new(ControllerServices);
    importer::LoadDataControllerServices {
        DatumConverter: mock.clone(),
        AssignmentFactory: mock.clone(),
        ParserFactory: mock.clone(),
        SizeEstimator: mock.clone(),
        StorageFactory: mock.clone(),
        TiKVConfigProbe: mock.clone(),
        ResourceCalculator: mock,
    }
}

// The planner currently stores the common fields only. Preserve the parser's
// remaining clauses in this test adapter instead of recreating their semantics.
struct ResolvedLoadData {
    plan: astersql_planner_core::LoadData,
    stmt: ast::LoadDataStmt,
}
impl importer::ResolvedLoadDataPlan for ResolvedLoadData {
    fn DBName(&self) -> &str {
        "test"
    }
    fn DBID(&self) -> i64 {
        1
    }
    fn Path(&self) -> &str {
        &self.plan.Path
    }
    fn Charset(&self) -> Option<&str> {
        self.stmt.Charset.as_deref()
    }
    fn FileLocRef(&self) -> ast::FileLocRef {
        self.stmt.FileLocRef
    }
    fn OnDuplicate(&self) -> ast::OnDuplicateKeyHandlingType {
        self.stmt.OnDuplicate
    }
    fn FieldsInfo(&self) -> Option<&ast::FieldsClause> {
        self.stmt.FieldsInfo.as_ref()
    }
    fn LinesInfo(&self) -> Option<&ast::LinesClause> {
        self.stmt.LinesInfo.as_ref()
    }
    fn IgnoreLines(&self) -> Option<u64> {
        Some(self.plan.IgnoreLines)
    }
    fn ColumnsAndUserVars(&self) -> &[ast::ColumnNameOrUserVar] {
        &self.stmt.ColumnsAndUserVars
    }
    fn ColumnAssignments(&self) -> &[ast::Assignment] {
        &self.stmt.ColumnAssignments
    }
}
struct InitSession;
impl importer::ImportSessionContext for InitSession {
    fn SQLMode(&self) -> astersql_parser_mysql::r#const::SQLMode {
        astersql_parser_mysql::r#const::SQLMode(0)
    }
    fn SystemVariable(&self, _name: &str) -> Option<String> {
        None
    }
    fn DistSQLScanConcurrency(&self) -> usize {
        15
    }
    fn User(&self) -> String {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn Keyspace(&self) -> String {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn LocationID(&self) -> String {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn EvalLoadDataOption(
        &self,
        option: &ast::LoadDataOpt,
    ) -> Result<importer::ImportOptionValue, String> {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn FormatColumnOrUserVariable(
        &self,
        value: &ast::ColumnNameOrUserVar,
    ) -> Result<String, String> {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn FormatAssignment(&self, value: &ast::Assignment) -> Result<String, String> {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn RedactURL(&self, value: &str) -> String {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn IsNextGen(&self) -> bool {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn SEMEnabled(&self) -> bool {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn MaxDistTaskNodes(&self) -> i32 {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn AutoMaxDistTaskNodes(&self) -> Result<i32, String> {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
    fn DefaultCloudStorageURI(&self) -> String {
        unreachable!("execution service must not run during LOAD DATA initialization")
    }
}

pub(super) fn build_controller(sql: &str) -> Result<importer::LoadDataController, String> {
    let node = astersql_parser::New()
        .ParseOneStmt(sql, "", "")
        .expect("parse LOAD DATA");
    let stmt = *node
        .into_any()
        .downcast::<ast::LoadDataStmt>()
        .expect("LOAD DATA statement");
    let plan = astersql_planner_core::LoadData {
        Path: stmt.Path.clone(),
        Table: "load_data_test".into(),
        IgnoreLines: stmt.IgnoreLines.unwrap_or(0),
        ..Default::default()
    };
    let resolved = ResolvedLoadData { plan, stmt };
    let meta = astersql_meta_model::TableInfo {
        ID: 1,
        Name: ast::NewCIStr("load_data_test"),
        State: astersql_meta_model::StatePublic,
        Columns: ["a", "b"]
            .iter()
            .enumerate()
            .map(|(offset, name)| astersql_meta_model::ColumnInfo {
                ID: offset as i64 + 1,
                Name: ast::NewCIStr(name),
                Offset: offset as isize,
                State: astersql_meta_model::StatePublic,
                FieldType: *astersql_types::field::NewFieldType(
                    astersql_parser_mysql::r#type::TypeVarchar,
                ),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let plan = importer::NewPlanFromLoadDataPlan(&InitSession, &resolved)?;
    importer::NewLoadDataController(
        plan,
        Arc::new(astersql_planner_core_operator_physicalop::MetadataTableAdapter::New(&meta)),
        importer::ASTArgsFromPlan(&resolved),
        services(),
        vec![],
    )
}
