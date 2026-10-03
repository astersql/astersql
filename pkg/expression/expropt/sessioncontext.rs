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

use crate::{OptionalEvalPropContext, RequireOptionalEvalProps, exprctx, get_prop_provider};
use std::sync::Arc;

/// Session capabilities required by EMBED_TEXT; the runtime remains Domain-owned.
pub trait SessionContext {
    fn embedding_runtime(&self) -> Option<Arc<inference::EmbedFn>>;
    fn embedding_cancellation(&self) -> Option<String>;
    fn embedding_context_values(&self) -> inference::embed_fn::ContextValues {
        Default::default()
    }
}
pub struct SessionContextPropProvider {
    session: Arc<dyn SessionContext>,
}
impl SessionContextPropProvider {
    pub fn new<T: SessionContext + 'static>(session: Arc<T>) -> Self {
        Self { session }
    }
}
impl exprctx::OptionalEvalPropProvider for SessionContextPropProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropSessionContext.Desc()
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
pub struct SessionContextPropReader;
impl RequireOptionalEvalProps for SessionContextPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropSessionContext.AsPropKeySet()
    }
}
impl SessionContextPropReader {
    pub fn get_session_context<'a, C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &'a C,
    ) -> anyhow::Result<&'a dyn SessionContext> {
        Ok(
            get_prop_provider::<SessionContextPropProvider, _>(
                ctx,
                exprctx::OptPropSessionContext,
            )?
            .session
            .as_ref(),
        )
    }
}
