// Copyright 2026 AsterSQL.

use crate::*;
use std::any::Any;

struct EmptyUserVars;

impl exprctx::UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }
    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }
    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

struct TestProvider;

impl OptionalEvalPropProvider for TestProvider {
    fn Desc(&self) -> &'static OptionalEvalPropDesc {
        &exprctx::OPTIONAL_PROPERTY_DESC_LIST[0]
    }
}

struct TestEvalContext {
    user_vars: EmptyUserVars,
    provider: TestProvider,
}

impl contextutil::WarnAppender for TestEvalContext {
    fn AppendWarning(&self, _error: contextutil::errors::SharedError) {}
    fn AppendNote(&self, _error: contextutil::errors::SharedError) {}
}

impl contextutil::WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        0
    }
    fn TruncateWarnings(&self, _start: isize) -> Vec<contextutil::SQLWarn> {
        Vec::new()
    }
    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        destination
    }
}

impl ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, exprctx::ParamError> {
        Err(exprctx::ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        1
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }
    fn TypeCtx(&self) -> types::Context {
        panic!("not used by this test")
    }
    fn ErrCtx(&self) -> errctx::Context {
        panic!("not used by this test")
    }
    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }
    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        Ok(chrono::DateTime::UNIX_EPOCH.with_timezone(&chrono_tz::UTC))
    }
    fn CurrentDB(&self) -> String {
        String::new()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }
    fn GetTiDBRedactLog(&self) -> String {
        "OFF".into()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".into()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }
    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        &self.user_vars
    }
    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet {
        exprctx::OptPropCurrentUser.AsPropKeySet()
    }
    fn GetOptionalPropProvider(
        &self,
        key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        (key == exprctx::OptPropCurrentUser).then_some(&self.provider)
    }
}

struct TestBuiltin {
    required: OptionalEvalPropKeySet,
    allowed: OptionalEvalPropKeySet,
    collation: collationInfo,
    args: Vec<Box<dyn Expression>>,
    ret_type: types::FieldType,
    collator: Box<dyn collate::Collator>,
}

impl TestBuiltin {
    fn new(required: OptionalEvalPropKeySet) -> Self {
        Self {
            required,
            allowed: OptionalEvalPropKeySet::default(),
            collation: collationInfo::default(),
            args: Vec::new(),
            ret_type: types::FieldType::default(),
            collator: collate::GetBinaryCollator(),
        }
    }
}

impl CollationInfo for TestBuiltin {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.collation.Coercibility()
    }
    fn Repertoire(&self) -> Repertoire {
        self.collation.Repertoire()
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }
    fn SetCoercibility(&self, value: Coercibility) {
        self.collation.SetCoercibility(value);
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.collation.SetRepertoire(value);
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.collation.SetCharsetAndCollation(charset, collation);
    }
    fn IsExplicitCharset(&self) -> bool {
        self.collation.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.collation.SetExplicitCharset(explicit);
    }
}

impl builtinFunc for TestBuiltin {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        self.required
    }
    fn AllowedOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        self.allowed
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        true
    }
    fn getArgs(&self) -> &[Box<dyn Expression>] {
        &self.args
    }
    fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
        &mut self.args
    }
    fn equal(&self, _ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().is::<Self>()
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.ret_type
    }
    fn setPbCode(&mut self, _code: i32) {}
    fn PbCode(&self) -> i32 {
        0
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.collator = collator;
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(Self {
            required: self.required,
            allowed: self.allowed,
            collation: self.collation.clone(),
            args: self.args.clone(),
            ret_type: self.ret_type.clone(),
            collator: self.collator.Clone(),
        })
    }
    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
    }
    fn vectorized(&self) -> bool {
        true
    }
}

#[test]
fn nested_assertion_context_checks_only_the_current_builtin_like_go() {
    let base = TestEvalContext {
        user_vars: EmptyUserVars,
        provider: TestProvider,
    };
    let parent = TestBuiltin::new(OptionalEvalPropKeySet::default());
    let child = TestBuiltin::new(exprctx::OptPropCurrentUser.AsPropKeySet());
    let parent_context = assertionEvalContext::new_for_test(&base, &parent);
    let child_context = assertionEvalContext::new_for_test(&parent_context, &child);

    assert!(
        child_context
            .GetOptionalPropProvider(exprctx::OptPropCurrentUser)
            .is_some()
    );
}

#[test]
fn assertion_context_accepts_allowed_optional_property_without_requiring_it() {
    let base = TestEvalContext {
        user_vars: EmptyUserVars,
        provider: TestProvider,
    };
    let mut function = TestBuiltin::new(OptionalEvalPropKeySet::default());
    function.allowed = exprctx::OptPropCurrentUser.AsPropKeySet();
    let context = assertionEvalContext::new_for_test(&base, &function);

    assert!(
        context
            .GetOptionalPropProvider(exprctx::OptPropCurrentUser)
            .is_some()
    );
    assert_eq!(
        function.RequiredOptionalEvalProps(),
        OptionalEvalPropKeySet::default()
    );
}

#[test]
#[should_panic(expected = "RequiredOptionalEvalProps 或 AllowedOptionalEvalProps")]
fn assertion_context_rejects_undeclared_optional_property() {
    let base = TestEvalContext {
        user_vars: EmptyUserVars,
        provider: TestProvider,
    };
    let function = TestBuiltin::new(OptionalEvalPropKeySet::default());
    assertionEvalContext::new_for_test(&base, &function)
        .GetOptionalPropProvider(exprctx::OptPropCurrentUser);
}

#[test]
fn uncompress_allows_session_vars_without_requiring_them() {
    assert_eq!(
        formal_registry::allowedOptionalEvalPropsForSignature(
            "builtinUncompressSig",
            OptionalEvalPropKeySet::default(),
        ),
        exprctx::OptPropSessionVars.AsPropKeySet()
    );
    let fallback = exprctx::OptPropCurrentUser.AsPropKeySet();
    assert_eq!(
        formal_registry::allowedOptionalEvalPropsForSignature("builtinCompressSig", fallback),
        fallback
    );
}
