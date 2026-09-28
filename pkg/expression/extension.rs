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
        checkPrivileges(checker.as_ref(), &self.definition)?;
        self.base.verifyArgs(&args)?;

        let argument_refs: Vec<&dyn Expression> =
            args.iter().map(|argument| argument.as_ref()).collect();
        let collation = crate::CheckAndDeriveCollationFromExprs(
            ctx,
            &self.definition.Name,
            self.definition.EvalTp,
            &argument_refs,
        )?;
        let mut return_type = fieldTypeForEval(self.definition.EvalTp);
        return_type.SetCharset(collation.Charset.clone());
        return_type.SetCollate(collation.Collation.clone());
        return_type.SetFlen(self.flen);
        // 按声明的参数求值类型做 CAST，保证回调收到期望类型。
        let cast_args = args
            .into_iter()
            .zip(&self.definition.ArgTps)
            .map(|(argument, eval_type)| {
                let mut target = fieldTypeForEval(*eval_type);
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
        checkPrivileges(checker.as_ref(), &self.definition)?;
        let vars = self
            .session_vars_reader
            .get_session_vars(ctx)
            .map_err(|error| errors::New(error.to_string()))?;
        Ok(extensionFnContext {
            ctx,
            vars,
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
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        OptionalEvalPropKeySet(
            self.session_vars_reader.required_optional_eval_props().0
                | self.privilege_reader.required_optional_eval_props().0,
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
            Arc::ptr_eq(&self.definition, &other.definition) && self.base.equal(ctx, &other.base)
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
    ctx: &'a dyn EvalContext,
    vars: &'a expropt::variable::SessionVars,
    signature: &'a extensionFuncSig,
}

impl extension::ExtensionContext for extensionFnContext<'_> {}

impl extension::FunctionContext for extensionFnContext<'_> {
    fn User(&self) -> Option<&extension::auth_identity::UserIdentity> {
        self.vars.User.as_ref()
    }

    fn ActiveRoles(&self) -> Vec<&extension::auth_identity::RoleIdentity> {
        self.vars.ActiveRoles.iter().collect()
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
