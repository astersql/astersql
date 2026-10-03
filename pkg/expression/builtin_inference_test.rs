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

use crate::*;
#[test]
fn embedding_builtin_registers_arity_and_optimizer_effects() {
    let class = formal_registry::funcs
        .get("embed_text")
        .expect("EMBED_TEXT registered");
    assert!(class.verifyArgsByCount(1).is_err());
    assert!(class.verifyArgsByCount(2).is_ok());
    assert!(class.verifyArgsByCount(3).is_ok());
    assert!(class.verifyArgsByCount(4).is_err());
    assert!(formal_registry::GetBuiltinList().contains(&"embed_text".to_owned()));
    assert!(is_unfoldable("embed_text"));
    assert!(has_mutable_effect("embed_text"));
    assert!(is_illegal_generated_column_function("embed_text"));
}

use std::sync::Arc;
struct EmbeddingSession {
    runtime: Arc<inference::EmbedFn>,
}
impl expropt::SessionContext for EmbeddingSession {
    fn embedding_runtime(&self) -> Option<Arc<inference::EmbedFn>> {
        Some(self.runtime.clone())
    }
    fn embedding_cancellation(&self) -> Option<String> {
        None
    }
}
fn context() -> exprstatic::ExprContext {
    let runtime = Arc::new(inference::EmbedFn::new());
    runtime
        .register("mock", Arc::new(inference::MockEmbedder))
        .unwrap();
    let provider = expropt::SessionContextPropProvider::new(Arc::new(EmbeddingSession { runtime }));
    let eval = Arc::new(exprstatic::NewEvalContext(vec![
        exprstatic::WithOptionalProperty(vec![Box::new(provider)]),
    ]));
    exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(eval)])
}
fn string(value: Option<&str>) -> ExprBox {
    let mut datum = types::Datum::default();
    if let Some(value) = value {
        datum.SetString(value.into(), charset::CollationUTF8MB4.into());
    }
    Box::new(Constant::with_type(
        datum,
        *types::NewFieldType(mysql::TypeVarString),
    ))
}
#[test]
fn embedding_factory_observes_deployment_before_nulls() {
    let _guard = MODE.lock().unwrap();
    let previous = deploymode::Get();
    let _ = deploymode::Set(deploymode::Premium);
    let ctx = context();
    let expression = NewFunctionBase(
        &ctx,
        "embed_text",
        *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
        vec![string(None), string(Some("[1]"))],
    )
    .unwrap();
    assert!(
        expression
            .EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap_err()
            .to_string()
            .contains("starter deployment mode")
    );
    let _ = deploymode::Set(previous);
}
#[test]
fn embedding_factory_starter_options_nulls_and_errors() {
    let _guard = MODE.lock().unwrap();
    let previous = deploymode::Get();
    if deploymode::Set(deploymode::Starter).is_err() {
        return;
    }
    let previous_assertions = intest::EnableAssert.swap(true, std::sync::atomic::Ordering::Relaxed);
    let ctx = context();
    for (options, expected) in [
        (None, "[1,2,3]"),
        (Some(""), "[1,2,3]"),
        (Some("{\"plus\":1,\"plus@search\":10}"), "[2,3,4]"),
    ] {
        let mut args = vec![string(Some("mock/json")), string(Some("[1,2,3]"))];
        if let Some(options) = options {
            args.push(string(Some(options)));
        }
        let expr = NewFunctionBase(
            &ctx,
            "embed_text",
            *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
            args,
        )
        .unwrap();
        let (vector, null) = expr
            .EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap();
        assert!(!null);
        assert_eq!(vector.String(), expected);
    }
    for options in ["null", "[]", "{invalid}"] {
        let expr = NewFunctionBase(
            &ctx,
            "embed_text",
            *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
            vec![
                string(Some("mock/json")),
                string(Some("[1]")),
                string(Some(options)),
            ],
        )
        .unwrap();
        assert!(
            expr.EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
                .unwrap_err()
                .to_string()
                .contains("options in JSON")
        );
    }
    for args in [
        vec![string(None), string(Some("[1]"))],
        vec![string(Some("mock/json")), string(None)],
    ] {
        let expr = NewFunctionBase(
            &ctx,
            "embed_text",
            *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
            args,
        )
        .unwrap();
        assert!(
            expr.EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
                .unwrap()
                .1
        );
    }
    let expr = NewFunctionBase(
        &ctx,
        "embed_text",
        *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
        vec![string(Some("mock/json")), string(Some("[1]")), string(None)],
    )
    .unwrap();
    assert_eq!(
        expr.EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .0
            .String(),
        "[1]"
    );
    for (model, text) in [("unknown/model", "[1]"), ("mock/json", "invalid")] {
        let expr = NewFunctionBase(
            &ctx,
            "embed_text",
            *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
            vec![string(Some(model)), string(Some(text))],
        )
        .unwrap();
        assert!(
            expr.EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
                .is_err()
        );
    }
    let oversized = format!("[{}0]", "0,".repeat(16_383));
    let expr = NewFunctionBase(
        &ctx,
        "embed_text",
        *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
        vec![string(Some("mock/json")), string(Some(&oversized))],
    )
    .unwrap();
    assert!(
        expr.EvalVectorFloat32(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap_err()
            .to_string()
            .contains("vector cannot have more than 16383 dimensions")
    );
    intest::EnableAssert.store(previous_assertions, std::sync::atomic::Ordering::Relaxed);
    deploymode::Set(previous).unwrap();
}

static MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());
