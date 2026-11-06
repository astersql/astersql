// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
// 表达式扩展点：自定义函数与扩展表达式接入。
//
// 允许插件以 `FunctionDef` 注册自定义 SQL 函数：校验与内建冲突、
// 动态权限、参数/返回类型，并包装为 `functionClass`/`builtinFunc`。
// `init` 将注册/删除钩子安装到 extension 依赖包。

use std::any::Any;
use std::sync::{Arc, Once};

use extension_dependency as extension;
use sem_dependency as sem;

use crate::expression_builtin::formal_registry::RegistryBuiltinBase;
use crate::{
    BuildCastFunction, BuildContext, Coercibility, CollationInfo, Error, EvalContext, ExprBox,
    Expression, OptionalEvalPropKeySet, Repertoire, baseFunctionClass, builtinFunc, chunk, collate,
    errors, extensionFuncs, functionClass, mysql, types,
};
use expropt::{
    PrivilegeChecker, PrivilegeCheckerPropReader, RequireOptionalEvalProps, SessionVarsPropReader,
};

/// 注册扩展函数：校验定义、拒绝与内建/已注册名冲突，写入 `extensionFuncs`。
pub fn registerExtensionFunc(
    definition: Option<&Arc<extension::FunctionDef>>,
) -> Result<(), extension::ExtensionError> {
    let definition = definition
        .cloned()
        .ok_or_else(|| extension::ExtensionError::new("extension function def is nil"))?;
    definition.Validate()?;

    let lower_name = definition.Name.to_lowercase();
    if crate::funcs.contains_key(&lower_name) {
        return Err(extension::ExtensionError::new(format!(
            "extension function name '{}' conflict with builtin",
            definition.Name
        )));
    }

    let class: Arc<dyn functionClass> = Arc::new(newExtensionFuncClass(definition.clone())?);
    // LoadOrStore：已存在则视为重复注册。
    let (_, loaded) = extensionFuncs.LoadOrStore(lower_name, class);
    if loaded {
        return Err(extension::ExtensionError::new(format!(
            "duplicated extension function name '{}'",
            definition.Name
        )));
    }
    Ok(())
}

/// 按传入键精确删除已注册的扩展函数，与 Go `sync.Map.Delete` 一致。
pub fn removeExtensionFunc(name: &str) {
    extensionFuncs.Delete(name);
}

/// 扩展函数的 `functionClass`：持有定义、权限 Reader 与返回列宽。
struct extensionFuncClass {
    base: baseFunctionClass,
    privilege_reader: PrivilegeCheckerPropReader,
    definition: Arc<extension::FunctionDef>,
    flen: isize,
    pg: Option<PgExtensionDeclaration>,
}

/// 按求值类型构造对应 MySQL FieldType。
fn fieldTypeForEval(eval_type: types::EvalType) -> types::FieldType {
    let mysql_type = if eval_type == types::ETInt {
        mysql::TypeLonglong
    } else if eval_type == types::ETReal {
        mysql::TypeDouble
    } else if eval_type == types::ETDecimal {
        mysql::TypeNewDecimal
    } else if eval_type == types::ETString {
        mysql::TypeVarString
    } else if eval_type == types::ETDatetime {
        mysql::TypeDatetime
    } else if eval_type == types::ETTimestamp {
        mysql::TypeTimestamp
    } else if eval_type == types::ETDuration {
        mysql::TypeDuration
    } else if eval_type == types::ETJson {
        mysql::TypeJSON
    } else if eval_type == types::ETVectorFloat32 {
        mysql::TypeTiDBVectorFloat32
    } else {
        mysql::TypeUnspecified
    };
    *types::NewFieldType(mysql_type)
}

/// 校验 Eval 回调存在与返回类型，构造 `extensionFuncClass`。
fn newExtensionFuncClass(
    definition: Arc<extension::FunctionDef>,
) -> Result<extensionFuncClass, extension::ExtensionError> {
    let flen = if definition.EvalTp == types::ETString {
        if definition.EvalStringFunc.is_none() {
            return Err(extension::ExtensionError::new("eval function is nil"));
        }
        mysql::MaxFieldVarCharLength as isize
    } else if definition.EvalTp == types::ETInt {
        if definition.EvalIntFunc.is_none() {
            return Err(extension::ExtensionError::new("eval function is nil"));
        }
        mysql::MaxIntWidth as isize
    } else {
        return Err(extension::ExtensionError::new(format!(
            "unsupported extension function ret type: '{:?}'",
            definition.EvalTp
        )));
    };

    let max_args = definition.ArgTps.len();
    let min_args = max_args - definition.OptionalArgsLen as usize;
    Ok(extensionFuncClass {
        base: baseFunctionClass::new(definition.Name.clone(), min_args, max_args as isize),
        privilege_reader: PrivilegeCheckerPropReader,
        definition,
        flen,
        pg: None,
    })
}

impl functionClass for extensionFuncClass {
    fn getFunction(
        &self,
        ctx: &dyn BuildContext,
        args: Vec<ExprBox>,
    ) -> Result<Box<dyn builtinFunc>, Error> {
        // 构建期先做动态权限检查。
        let checker = self
            .privilege_reader
            .get_privilege_checker(ctx.GetEvalCtx())
            .map_err(|error| errors::New(error.to_string()))?;
        checkPrivileges(checker.as_ref(), &self.definition).map_err(|e| {
            if self.pg.is_some() {
                PgExtensionError::error("42501", e.to_string())
            } else {
                e
            }
        })?;
        self.base.verifyArgs(&args)?;

        let argument_refs: Vec<&dyn Expression> =
            args.iter().map(|argument| argument.as_ref()).collect();
        let collation = crate::CheckAndDeriveCollationFromExprs(
            ctx,
            &self.definition.Name,
            self.definition.EvalTp,
            &argument_refs,
        )?;
        let mut return_type = self
            .pg
            .as_ref()
            .map(|p| pg_extension_type(p.result))
            .transpose()?
            .unwrap_or_else(|| fieldTypeForEval(self.definition.EvalTp));
        return_type.SetCharset(collation.Charset.clone());
        return_type.SetCollate(collation.Collation.clone());
        return_type.SetFlen(self.flen);
        // 按声明的参数求值类型做 CAST，保证回调收到期望类型。
        let cast_args = args
            .into_iter()
            .zip(&self.definition.ArgTps)
            .enumerate()
            .map(|(index, (argument, eval_type))| {
                let mut target = self
                    .pg
                    .as_ref()
                    .map(|p| {
                        pg_extension_type(p.arguments[index]).expect("validated PG extension type")
                    })
                    .unwrap_or_else(|| fieldTypeForEval(*eval_type));
                target.SetCharset(collation.Charset.clone());
                target.SetCollate(collation.Collation.clone());
                BuildCastFunction(ctx, &argument, &target)
            })
            .collect();

        // 扩展函数结果依赖外部回调，禁止进入计划缓存。
        ctx.SetSkipPlanCache("extension function should not be cached");
        Ok(Box::new(extensionFuncSig {
            base: RegistryBuiltinBase::new_never(cast_args, return_type),
            session_vars_reader: SessionVarsPropReader,
            privilege_reader: PrivilegeCheckerPropReader,
            definition: self.definition.clone(),
            require_identity: self.pg.is_some(),
            pg_identity: self.pg.as_ref().map(|p| (p.database, p.oid)),
        }))
    }

    fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
        self.base.verifyArgsByCount(count)
    }

    fn getDisplayName(&self) -> &str {
        &self.base.funcName
    }
}

/// 按 `RequireDynamicPrivileges` 请求动态权限；SEM 开启时消息不同。
fn checkPrivileges(
    checker: &dyn PrivilegeChecker,
    definition: &extension::FunctionDef,
) -> Result<(), Error> {
    let Some(required) = &definition.RequireDynamicPrivileges else {
        return Ok(());
    };
    let sem_enabled = sem::IsEnabled();
    for privilege in required(sem_enabled) {
        if !checker.request_dynamic_verification(&privilege, false) {
            let message = if sem_enabled {
                privilege
            } else {
                format!("SUPER or {privilege}")
            };
            let generated = crate::expression_errors_kernel::errSpecificAccessDenied
                .GenWithStackByArgs(&[message.into()]);
            return Err(generated.into());
        }
    }
    Ok(())
}

/// 扩展函数运行时签名：求值前再检权限并组装 `extensionFnContext`。
struct extensionFuncSig {
    require_identity: bool,
    pg_identity: Option<(i64, i64)>,
    base: RegistryBuiltinBase,
    session_vars_reader: SessionVarsPropReader,
    privilege_reader: PrivilegeCheckerPropReader,
    definition: Arc<extension::FunctionDef>,
}

impl extensionFuncSig {
    /// 求值前读取权限检查器与会话变量，构造扩展回调上下文。
    fn evaluate_context<'a>(
        &'a self,
        ctx: &'a dyn EvalContext,
    ) -> Result<extensionFnContext<'a>, Error> {
        let checker = self
            .privilege_reader
            .get_privilege_checker(ctx)
            .map_err(|error| errors::New(error.to_string()))?;
        checkPrivileges(checker.as_ref(), &self.definition).map_err(|e| {
            if self.require_identity {
                PgExtensionError::error("42501", e.to_string())
            } else {
                e
            }
        })?;
        let vars = self
            .session_vars_reader
            .get_session_vars(ctx)
            .map_err(|error| errors::New(error.to_string()))?;
        let (user, roles) = if self.require_identity {
            (
                Some(
                    expropt::CurrentUserPropReader
                        .current_user(ctx)
                        .map_err(|e| errors::New(e.to_string()))?,
                ),
                expropt::CurrentUserPropReader
                    .active_roles(ctx)
                    .map_err(|e| errors::New(e.to_string()))?,
            )
        } else {
            (
                vars.User.clone().map(Arc::new),
                vars.ActiveRoles.iter().cloned().map(Arc::new).collect(),
            )
        };
        Ok(extensionFnContext {
            ctx,
            vars,
            user,
            roles,
            signature: self,
        })
    }
}

impl CollationInfo for extensionFuncSig {
    fn HasCoercibility(&self) -> bool {
        self.base.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.base.Coercibility()
    }
    fn SetCoercibility(&self, value: Coercibility) {
        self.base.SetCoercibility(value)
    }
    fn Repertoire(&self) -> Repertoire {
        self.base.Repertoire()
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.base.SetRepertoire(value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.base.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.base.SetCharsetAndCollation(charset, collation)
    }
    fn IsExplicitCharset(&self) -> bool {
        self.base.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.base.SetExplicitCharset(explicit)
    }
}

impl builtinFunc for extensionFuncSig {
    fn PostgreSQLFunctionIdentity(&self) -> Option<(i64, i64)> {
        self.pg_identity
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        OptionalEvalPropKeySet(
            self.session_vars_reader.required_optional_eval_props().0
                | self.privilege_reader.required_optional_eval_props().0
                | if self.require_identity {
                    expropt::CurrentUserPropReader
                        .required_optional_eval_props()
                        .0
                } else {
                    0
                },
        )
    }

    fn isExtensionFunction(&self) -> bool {
        true
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        self.base.SafeToShareAcrossSession()
    }

    fn evalString(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(String, bool), Error> {
        if self.definition.EvalTp != types::ETString {
            return Err(errors::New("extension builtin does not return string"));
        }
        let function_context = self.evaluate_context(ctx)?;
        self.definition
            .EvalStringFunc
            .as_ref()
            .expect("validated string callback")(&function_context, row)
        .map_err(|error| errors::New(error.to_string()))
    }

    fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        if self.definition.EvalTp != types::ETInt {
            return Err(errors::New("extension builtin does not return int"));
        }
        let function_context = self.evaluate_context(ctx)?;
        self.definition
            .EvalIntFunc
            .as_ref()
            .expect("validated int callback")(&function_context, row)
        .map_err(|error| errors::New(error.to_string()))
    }

    fn getArgs(&self) -> &[ExprBox] {
        &self.base.args
    }

    fn getArgsMut(&mut self) -> &mut [ExprBox] {
        &mut self.base.args
    }

    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|other| {
            self.pg_identity == other.pg_identity
                && Arc::ptr_eq(&self.definition, &other.definition)
                && self.base.equal(ctx, &other.base)
        })
    }

    fn getRetTp(&self) -> &types::FieldType {
        &self.base.return_type
    }

    fn setPbCode(&mut self, code: i32) {
        self.base.pb_code = code;
    }

    fn PbCode(&self) -> i32 {
        self.base.pb_code
    }

    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.base.collator = collator;
    }

    fn collator(&self) -> &dyn collate::Collator {
        self.base.collator.as_ref()
    }

    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(Self {
            base: self.base.clone(),
            session_vars_reader: SessionVarsPropReader,
            privilege_reader: PrivilegeCheckerPropReader,
            definition: self.definition.clone(),
            require_identity: self.require_identity,
            pg_identity: self.pg_identity,
        })
    }

    fn MemoryUsage(&self) -> i64 {
        self.base.memory_usage()
    }

    fn vectorized(&self) -> bool {
        false
    }
}

/// 传给扩展 Eval 回调的上下文：会话用户/角色、当前库、连接信息与参数求值。
struct extensionFnContext<'a> {
    user: Option<Arc<extension::auth_identity::UserIdentity>>,
    roles: Vec<Arc<extension::auth_identity::RoleIdentity>>,
    ctx: &'a dyn EvalContext,
    vars: &'a expropt::variable::SessionVars,
    signature: &'a extensionFuncSig,
}

impl extension::ExtensionContext for extensionFnContext<'_> {}

impl extension::FunctionContext for extensionFnContext<'_> {
    fn PostgreSQLTableIsVisible(
        &self,
        oid: i64,
    ) -> Result<Option<bool>, extension::ExtensionError> {
        self.ctx
            .PgTableVisibilityEvaluator()
            .map(|evaluate| evaluate(oid))
            .ok_or_else(|| {
                extension::ExtensionError::new("PostgreSQL visibility context is unavailable")
            })
    }

    fn User(&self) -> Option<&extension::auth_identity::UserIdentity> {
        self.user.as_deref()
    }

    fn ActiveRoles(&self) -> Vec<&extension::auth_identity::RoleIdentity> {
        self.roles.iter().map(Arc::as_ref).collect()
    }

    fn CurrentDB(&self) -> String {
        self.ctx.CurrentDB()
    }

    fn ConnectionInfo(&self) -> Option<&extension::variable::ConnectionInfo> {
        self.vars.ConnectionInfo.as_ref()
    }

    /// 按当前行求值全部参数表达式为 Datum 列表。
    fn EvalArgs(&self, row: chunk::Row) -> Result<Vec<types::Datum>, extension::ExtensionError> {
        self.signature
            .base
            .args
            .iter()
            .map(|argument| {
                argument
                    .Eval(self.ctx, row.clone())
                    .map_err(|error| extension::ExtensionError::new(error.to_string()))
            })
            .collect()
    }
}

/// 幂等安装扩展函数注册/删除钩子（进程内只执行一次）。
pub fn init() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        extension::InstallExtensionFunctionHooks(
            Arc::new(|definition| registerExtensionFunc(Some(definition))),
            Arc::new(removeExtensionFunc),
        );
    });
}

/// A declaration belongs to a real registered callback object. Durable IDs are allocated by the metadata owner.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PgExtensionDeclaration {
    pub oid: i64,
    pub database: i64,
    pub namespace: i64,
    pub namespace_name: String,
    pub name: String,
    pub arguments: Vec<u32>,
    pub result: u32,
    pub owner: i64,
    pub implementation: String,
    pub temporary: bool,
}
pub fn pg_extension_type(oid: u32) -> Result<types::FieldType, Error> {
    let mut field = *types::NewFieldType(match oid {
        16 => mysql::TypeTiny,
        26 => mysql::TypeLonglong,
        23 => mysql::TypeLong,
        20 => mysql::TypeLonglong,
        25 => mysql::TypeVarString,
        _ => return Err(errors::New("unsupported PG extension type")),
    });
    if oid == 16 {
        field.AddFlag(mysql::IsBooleanFlag);
    }
    Ok(field)
}
pub fn validate_pg_extension(
    declaration: &PgExtensionDeclaration,
    definition: Arc<extension::FunctionDef>,
) -> Result<Arc<dyn functionClass>, Error> {
    definition
        .Validate()
        .map_err(|e| errors::New(e.to_string()))?;
    if definition.Name != declaration.name
        || definition.OptionalArgsLen != 0
        || definition.ArgTps.len() != declaration.arguments.len()
    {
        return Err(errors::New(
            "extension declaration does not match its implementation",
        ));
    }
    for (oid, eval) in declaration.arguments.iter().zip(&definition.ArgTps) {
        if pg_extension_type(*oid)?.EvalType() != *eval {
            return Err(errors::New("extension argument type mismatch"));
        }
    }
    if pg_extension_type(declaration.result)?.EvalType() != definition.EvalTp {
        return Err(errors::New("extension result type mismatch"));
    }
    let mut class = newExtensionFuncClass(definition).map_err(|e| errors::New(e.to_string()))?;
    class.pg = Some(declaration.clone());
    Ok(Arc::new(class))
}
pub fn register_pg_extension(
    scope: usize,
    declaration: PgExtensionDeclaration,
    definition: Arc<extension::FunctionDef>,
) -> Result<(), Error> {
    init();
    let class = validate_pg_extension(&declaration, definition)?;
    let mut entries = extensionFuncs
        .pg_entries
        .write()
        .map_err(|_| errors::New("extension registry poisoned"))?;
    if let Some((old, _)) = entries.get(&(scope, declaration.oid)) {
        if old != &declaration {
            return Err(errors::New("extension object identity conflict"));
        }
    }
    entries.insert((scope, declaration.oid), (declaration, class));
    Ok(())
}
pub fn remove_pg_extension(scope: usize, oid: i64) {
    extensionFuncs
        .pg_entries
        .write()
        .expect("extension registry poisoned")
        .remove(&(scope, oid));
}
pub fn build_pg_extension(
    scope: usize,
    declaration: &PgExtensionDeclaration,
    ctx: &dyn BuildContext,
    args: Vec<ExprBox>,
) -> Result<ExprBox, Error> {
    if ctx.GetEvalCtx().SQLDialect() != crate::ast::SqlDialect::PostgreSQL {
        return Err(PgExtensionError::error(
            "0A000",
            "PG extension construction requires PostgreSQL dialect",
        ));
    }
    let entry = extensionFuncs
        .pg_entries
        .read()
        .map_err(|_| errors::New("extension registry poisoned"))?
        .get(&(scope, declaration.oid))
        .cloned()
        .ok_or_else(|| {
            PgExtensionError::error("0A000", "PG extension implementation is not linked")
        })?;
    let mut registered = entry.0.clone();
    registered.namespace_name = declaration.namespace_name.clone();
    if registered != *declaration {
        return Err(PgExtensionError::error(
            "0A000",
            "PG extension object generation changed",
        ));
    }
    crate::NewFunctionFromClass(ctx, &declaration.name, entry.1.as_ref(), args)
}

/// A real factory backed by statement-owned relation identities, not a catalog-only declaration.
pub fn pg_table_visibility_definition() -> Arc<extension::FunctionDef> {
    Arc::new(extension::FunctionDef {
        Name: "pg_table_is_visible".into(),
        EvalTp: types::ETInt,
        ArgTps: vec![types::ETInt],
        EvalIntFunc: Some(Arc::new(|context, row| {
            let arguments = context.EvalArgs(row)?;
            if arguments[0].IsNull() {
                return Ok((0, true));
            }
            let visible = context.PostgreSQLTableIsVisible(arguments[0].GetInt64())?;
            Ok((i64::from(visible.unwrap_or(false)), visible.is_none()))
        })),
        ..Default::default()
    })
}

pub fn pg_extension_is_linked(scope: usize, oid: i64) -> bool {
    extensionFuncs
        .pg_entries
        .read()
        .expect("extension registry poisoned")
        .contains_key(&(scope, oid))
}
pub fn remove_pg_extension_scope(scope: usize) {
    extensionFuncs
        .pg_entries
        .write()
        .expect("extension registry poisoned")
        .retain(|(owner, _), _| *owner != scope);
}

/// Typed errors retain authorization/capability identity across the existing factory boundary.
#[derive(Clone, Debug)]
pub struct PgExtensionError {
    pub state: &'static str,
    pub message: String,
}
impl std::fmt::Display for PgExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PgExtensionError {}
impl PgExtensionError {
    fn error(state: &'static str, message: impl Into<String>) -> Error {
        crate::contextutil::errors::SharedError::new(Self {
            state,
            message: message.into(),
        })
        .into()
    }
}
