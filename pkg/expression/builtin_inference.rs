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

// SQL EMBED_TEXT evaluation with Domain-owned runtime and optional session context.

use crate::*;
#[derive(Clone)]
pub struct builtinEmbedTextSig {
    pub(crate) baseBuiltinFunc: formal_registry::RegistryBuiltinBase,
}
pub struct embedTextFunctionClass {
    pub baseFunctionClass: baseFunctionClass,
}
impl functionClass for embedTextFunctionClass {
    fn getFunction(
        &self,
        ctx: &dyn BuildContext,
        args: Vec<ExprBox>,
    ) -> Result<Box<dyn builtinFunc>, Error> {
        self.baseFunctionClass.verifyArgs(&args)?;
        let string_type = *types::NewFieldType(mysql::TypeVarString);
        let args = args
            .into_iter()
            .map(|arg| {
                if arg.GetType(ctx.GetEvalCtx()).EvalType() == types::ETString {
                    arg
                } else {
                    BuildCastFunction(ctx, &arg, &string_type)
                }
            })
            .collect();
        let mut ret = *types::NewFieldType(mysql::TypeTiDBVectorFloat32);
        ret.SetCharset(charset::CharsetBin.into());
        ret.SetCollate(charset::CollationBin.into());
        Ok(Box::new(builtinEmbedTextSig {
            baseBuiltinFunc: formal_registry::RegistryBuiltinBase::new_recursive(args, ret),
        }))
    }
    fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
        self.baseFunctionClass.verifyArgsByCount(count)
    }
    fn getDisplayName(&self) -> &str {
        &self.baseFunctionClass.funcName
    }
}
impl builtinEmbedTextSig {
    fn evaluate(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        if !deploymode::IsStarter() {
            return Err(errors::New(
                "EMBED_TEXT is only supported in starter deployment mode",
            ));
        }
        let reader = expropt::SessionContextPropReader;
        let session = reader
            .get_session_context(ctx)
            .map_err(|_| errors::New("EMBED_TEXT requires session context"))?;
        let args = &self.baseBuiltinFunc.args;
        let (model, null) = args[0].EvalString(ctx, row.clone())?;
        if null {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        let (text, null) = args[1].EvalString(ctx, row.clone())?;
        if null {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        let mut opts = inference::Options::new();
        if args.len() == 3 {
            let (value, null) = args[2].EvalString(ctx, row)?;
            if !null && !value.is_empty() {
                let json: serde_json::Value = serde_json::from_str(&value)
                    .map_err(|_| errors::New("EMBED_TEXT expects options in JSON format"))?;
                let object = json
                    .as_object()
                    .ok_or_else(|| errors::New("EMBED_TEXT expects options in JSON format"))?;
                opts.extend(
                    object
                        .iter()
                        .filter(|(key, _)| !key.ends_with("@search"))
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
        }
        let runtime = session.embedding_runtime().ok_or_else(|| {
            errors::New("EMBED_TEXT requires an initialized Domain embedding runtime")
        })?;
        let value = runtime
            .embed_with_context_values(
                &model,
                &text,
                &opts,
                &|| session.embedding_cancellation(),
                &session.embedding_context_values(),
            )
            .map_err(errors::New)?;
        types_dependency::vector::CheckVectorDimValid(value.len() as i32)
            .map_err(|error| errors::New(error.to_string()))?;
        let vector = types_dependency::vector::CreateVectorFloat32(&value)
            .map_err(|error| errors::New(error.to_string()))?;
        Ok((vector, false))
    }
}
impl CollationInfo for builtinEmbedTextSig {
    fn HasCoercibility(&self) -> bool {
        self.baseBuiltinFunc.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.baseBuiltinFunc.Coercibility()
    }
    fn SetCoercibility(&self, value: Coercibility) {
        self.baseBuiltinFunc.SetCoercibility(value)
    }
    fn Repertoire(&self) -> Repertoire {
        self.baseBuiltinFunc.Repertoire()
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.baseBuiltinFunc.SetRepertoire(value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.baseBuiltinFunc.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.baseBuiltinFunc
            .SetCharsetAndCollation(charset, collation)
    }
    fn IsExplicitCharset(&self) -> bool {
        self.baseBuiltinFunc.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.baseBuiltinFunc.SetExplicitCharset(explicit)
    }
}

impl builtinFunc for builtinEmbedTextSig {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        self.baseBuiltinFunc.SafeToShareAcrossSession()
    }
    fn getArgs(&self) -> &[ExprBox] {
        &self.baseBuiltinFunc.args
    }
    fn getArgsMut(&mut self) -> &mut [ExprBox] {
        &mut self.baseBuiltinFunc.args
    }
    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|right| self.baseBuiltinFunc.equal(ctx, &right.baseBuiltinFunc))
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.baseBuiltinFunc.return_type
    }
    fn setPbCode(&mut self, code: i32) {
        self.baseBuiltinFunc.pb_code = code
    }
    fn PbCode(&self) -> i32 {
        self.baseBuiltinFunc.pb_code
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.baseBuiltinFunc.collator = collator
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.baseBuiltinFunc.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(self.clone())
    }
    fn MemoryUsage(&self) -> i64 {
        self.baseBuiltinFunc.memory_usage()
    }
    fn vectorized(&self) -> bool {
        false
    }
    fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        exprctx::OptPropSessionContext.AsPropKeySet()
    }
    fn evalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        self.evaluate(ctx, row)
    }
}
