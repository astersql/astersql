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

// metabuild `Context` 迁移对齐单元测试。
//
// 校验默认系统变量、选项覆盖顺序、信息模式挂载，以及告警/备注与非严格 SQL 模式
// 行为与 Go 侧一致。信息模式（infoschema）此处用空实现桩对象代替真实 catalog。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::option::Option as StdOption;
use std::sync::Arc;

use astersql_meta_metabuild::contextutil::{WarnLevelNote, WarnLevelWarning};
use astersql_meta_metabuild::infoschemactx::{self, MetaOnlyInfoSchema, Misc, SchemaAndTable};
use astersql_meta_metabuild::*;

/// 测试用构建上下文：会话上下文与错误类型均为空/不可失败。
type TestContext = Context<(), Infallible>;
/// 测试用只读信息模式 trait 对象别名。
type TestInfoSchema = dyn MetaOnlyInfoSchema<Context = (), Error = Infallible>;

/// 空信息模式桩：所有查询返回空集合或 None，用于隔离上下文选项测试。
struct EmptyInfoSchema;

impl SchemaAndTable for EmptyInfoSchema {
    type Context = ();
    type Error = Infallible;

    fn AllSchemas(&self) -> Vec<Arc<infoschemactx::model::DBInfo>> {
        Vec::new()
    }

    fn SchemaTableInfos(
        &self,
        _ctx: &Self::Context,
        _schema: &infoschemactx::ast::CIStr,
    ) -> Result<Vec<Arc<infoschemactx::model::TableInfo>>, Self::Error> {
        Ok(Vec::new())
    }
}

impl Misc for EmptyInfoSchema {
    fn PolicyByName(
        &self,
        _name: &infoschemactx::ast::CIStr,
    ) -> StdOption<Arc<infoschemactx::model::PolicyInfo>> {
        None
    }

    fn ResourceGroupByName(
        &self,
        _name: &infoschemactx::ast::CIStr,
    ) -> StdOption<Arc<infoschemactx::model::ResourceGroupInfo>> {
        None
    }

    fn MaskingPolicyByName(
        &self,
        _name: &infoschemactx::ast::CIStr,
    ) -> StdOption<Arc<infoschemactx::model::MaskingPolicyInfo>> {
        None
    }

    fn MaskingPolicyByTableColumn(
        &self,
        _table_id: i64,
        _column_id: i64,
    ) -> StdOption<Arc<infoschemactx::model::MaskingPolicyInfo>> {
        None
    }

    fn PlacementBundleByPhysicalTableID(
        &self,
        _id: i64,
    ) -> StdOption<Arc<infoschemactx::placement::Bundle>> {
        None
    }

    fn AllPlacementBundles(&self) -> Vec<Arc<infoschemactx::placement::Bundle>> {
        Vec::new()
    }

    fn AllPlacementPolicies(&self) -> Vec<Arc<infoschemactx::model::PolicyInfo>> {
        Vec::new()
    }

    fn ClonePlacementPolicies(&self) -> HashMap<String, Arc<infoschemactx::model::PolicyInfo>> {
        HashMap::new()
    }

    fn AllMaskingPolicies(&self) -> Vec<Arc<infoschemactx::model::MaskingPolicyInfo>> {
        Vec::new()
    }

    fn AllResourceGroups(&self) -> Vec<Arc<infoschemactx::model::ResourceGroupInfo>> {
        Vec::new()
    }

    fn CloneResourceGroups(&self) -> HashMap<String, Arc<infoschemactx::model::ResourceGroupInfo>> {
        HashMap::new()
    }

    fn HasTemporaryTable(&self) -> bool {
        false
    }
}

impl MetaOnlyInfoSchema for EmptyInfoSchema {
    fn SchemaMetaVersion(&self) -> i64 {
        0
    }

    fn SchemaByName(
        &self,
        _schema: &infoschemactx::ast::CIStr,
    ) -> StdOption<Arc<infoschemactx::model::DBInfo>> {
        None
    }

    fn SchemaExists(&self, _schema: &infoschemactx::ast::CIStr) -> bool {
        false
    }

    fn TableInfoByName(
        &self,
        _schema: &infoschemactx::ast::CIStr,
        _table: &infoschemactx::ast::CIStr,
    ) -> Result<Arc<infoschemactx::model::TableInfo>, Self::Error> {
        unreachable!("empty info schema has no tables")
    }

    fn TableInfoByID(&self, _id: i64) -> StdOption<Arc<infoschemactx::model::TableInfo>> {
        None
    }

    fn FindTableInfoByPartitionID(
        &self,
        _partition_id: i64,
    ) -> StdOption<(
        Arc<infoschemactx::model::TableInfo>,
        Arc<infoschemactx::model::DBInfo>,
        Arc<infoschemactx::model::PartitionDefinition>,
    )> {
        None
    }

    fn TableExists(
        &self,
        _schema: &infoschemactx::ast::CIStr,
        _table: &infoschemactx::ast::CIStr,
    ) -> bool {
        false
    }

    fn SchemaByID(&self, _id: i64) -> StdOption<Arc<infoschemactx::model::DBInfo>> {
        None
    }

    fn AllSchemaNames(&self) -> Vec<infoschemactx::ast::CIStr> {
        Vec::new()
    }

    fn SchemaSimpleTableInfos(
        &self,
        _ctx: &Self::Context,
        _schema: &infoschemactx::ast::CIStr,
    ) -> Result<Vec<Arc<infoschemactx::model::TableNameInfo>>, Self::Error> {
        Ok(Vec::new())
    }

    fn ListTablesWithSpecialAttribute(
        &self,
        _filter: infoschemactx::SpecialAttributeFilter,
    ) -> Vec<infoschemactx::TableInfoResult> {
        Vec::new()
    }

    fn GetTableReferredForeignKeys(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Vec<Arc<infoschemactx::model::ReferredFKInfo>> {
        Vec::new()
    }
}

/// 无选项时，各开关与默认校对规则应等于 `vardef` / `mysql` 包级默认值。
#[test]
fn defaults_and_forwarded_expression_behavior_match_go() {
    let ctx = NewContext::<(), Infallible>(Vec::new());

    let (charset, collation) = ctx.GetExprCtx().GetCharsetInfo();
    assert_eq!(charset, mysql::DefaultCharset);
    assert_eq!(collation, mysql::DefaultCollationName);
    assert_eq!(
        ctx.GetSQLMode(),
        mysql::GetSQLMode(mysql::DefaultSQLMode).unwrap()
    );

    assert_eq!(
        ctx.EnableAutoIncrementInGenerated(),
        vardef::DefTiDBEnableAutoIncrementInGenerated
    );
    assert!(!ctx.PrimaryKeyRequired());
    assert_eq!(
        ctx.GetClusteredIndexDefMode(),
        vardef::DefTiDBEnableClusteredIndex
    );
    assert_eq!(ctx.GetShardRowIDBits(), vardef::DefShardRowIDBits as u64);
    assert_eq!(ctx.GetPreSplitRegions(), vardef::DefPreSplitRegions as u64);
    assert!(!ctx.SuppressTooLongIndexErr());
    assert_eq!(
        ctx.GetDefaultCollationForUTF8MB4(),
        mysql::DefaultCollationName
    );
    assert_eq!(ctx.GetSQLMode(), ctx.GetExprCtx().GetEvalCtx().SQLMode());
    assert!(ctx.GetInfoSchema().0.is_none());
    assert!(!ctx.GetInfoSchema().1);
}

/// 选项按传入顺序覆盖；后写的同名选项生效，且 InfoSchema 指针保持共享。
#[test]
fn options_override_in_order_and_preserve_shared_interfaces() {
    let eval_ctx = Arc::new(exprstatic::NewEvalContext(vec![exprstatic::WithSQLMode(
        mysql::ModeNone,
    )]));
    let expr_ctx: Arc<dyn exprctx::ExprContext> = Arc::new(exprstatic::NewExprContext(vec![
        exprstatic::WithEvalCtx(eval_ctx),
        exprstatic::WithDefaultCollationForUTF8MB4("custom_collation".to_owned()),
    ]));
    let info_schema: Arc<TestInfoSchema> = Arc::new(EmptyInfoSchema);

    let ctx: TestContext = NewContext(vec![
        WithExprCtx(Arc::clone(&expr_ctx)),
        WithEnableAutoIncrementInGenerated(false),
        WithEnableAutoIncrementInGenerated(true),
        WithPrimaryKeyRequired(true),
        WithClusteredIndexDefMode(vardef::ClusteredIndexDefModeOff),
        WithShardRowIDBits(8),
        WithPreSplitRegions(456),
        WithSuppressTooLongIndexErr(true),
        WithInfoSchema(Some(Arc::clone(&info_schema))),
    ]);

    assert_eq!(ctx.GetSQLMode(), mysql::ModeNone);
    assert_eq!(ctx.GetDefaultCollationForUTF8MB4(), "custom_collation");
    assert!(ctx.EnableAutoIncrementInGenerated());
    assert!(ctx.PrimaryKeyRequired());
    assert_eq!(
        ctx.GetClusteredIndexDefMode(),
        vardef::ClusteredIndexDefModeOff
    );
    assert_eq!(ctx.GetShardRowIDBits(), 8);
    assert_eq!(ctx.GetPreSplitRegions(), 456);
    assert!(ctx.SuppressTooLongIndexErr());

    let (stored, ok) = ctx.GetInfoSchema();
    assert!(ok);
    assert!(Arc::ptr_eq(
        &stored.expect("schema must be present"),
        &info_schema
    ));

    // 显式清空 InfoSchema 后，第二返回值应为 false。
    let cleared: TestContext = NewContext(vec![WithInfoSchema(None)]);
    assert_eq!(cleared.GetInfoSchema().1, false);

    for value in [true, false] {
        let ctx: TestContext = NewContext(vec![WithEnableAutoIncrementInGenerated(value)]);
        assert_eq!(ctx.EnableAutoIncrementInGenerated(), value);

        let ctx: TestContext = NewContext(vec![WithPrimaryKeyRequired(value)]);
        assert_eq!(ctx.PrimaryKeyRequired(), value);

        let ctx: TestContext = NewContext(vec![WithSuppressTooLongIndexErr(value)]);
        assert_eq!(ctx.SuppressTooLongIndexErr(), value);
    }

    for value in [
        vardef::ClusteredIndexDefModeOn,
        vardef::ClusteredIndexDefModeOff,
    ] {
        let ctx: TestContext = NewContext(vec![WithClusteredIndexDefMode(value)]);
        assert_eq!(ctx.GetClusteredIndexDefMode(), value);
    }

    for value in [6, 8] {
        let ctx: TestContext = NewContext(vec![WithShardRowIDBits(value)]);
        assert_eq!(ctx.GetShardRowIDBits(), value);
    }

    for value in [123, 456] {
        let ctx: TestContext = NewContext(vec![WithPreSplitRegions(value)]);
        assert_eq!(ctx.GetPreSplitRegions(), value);
    }
}

/// 告警与备注写入表达式求值上下文；非严格上下文的 SQL Mode 应为 ModeNone。
#[test]
fn warnings_notes_and_non_strict_context_match_go() {
    let ctx = NewContext::<(), Infallible>(Vec::new());
    ctx.AppendWarning(contextutil::errors::New("warning"));
    ctx.AppendNote(contextutil::errors::New("note"));

    let warnings = ctx.GetExprCtx().GetEvalCtx().CopyWarnings(Vec::new());
    assert_eq!(warnings.len(), 2);
    assert_eq!(warnings[0].Level, WarnLevelWarning);
    assert_eq!(warnings[0].Err.as_ref().unwrap().to_string(), "warning");
    assert_eq!(warnings[1].Level, WarnLevelNote);
    assert_eq!(warnings[1].Err.as_ref().unwrap().to_string(), "note");

    let non_strict = NewNonStrictContext();
    assert_eq!(non_strict.GetSQLMode(), mysql::ModeNone);
}
