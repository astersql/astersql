// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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
use expression::Expression as _;
use std::sync::Arc;

struct SupportedPushDownClient;

impl kv::Client for SupportedPushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("window pushdown test must not send a request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

struct BlacklistGuard;

impl Drop for BlacklistGuard {
    fn drop(&mut self) {
        expression::infer_pushdown::clear_pushdown_blacklist();
    }
}

#[test]
fn window_pushdown_honors_tiflash_specific_scalar_blacklist() {
    let _guard = BlacklistGuard;
    expression::infer_pushdown::replace_pushdown_blacklist([(
        ast::Plus.to_owned(),
        expression::infer_pushdown::StoreType::TiFlash.mask(),
    )]);

    let build_context = exprstatic::NewExprContext(Vec::new());
    let return_type = *types::NewFieldType(mysql::TypeLonglong);
    let scalar = expression::NewFunctionBase(
        &build_context,
        ast::Plus,
        return_type.clone(),
        vec![
            Box::new(expression::Column::new(return_type.clone(), 1, 1, 0)),
            Box::new(expression::Column::new(return_type.clone(), 2, 2, 1)),
        ],
    )
    .unwrap();
    let descriptor = WindowFuncDesc {
        baseFuncDesc: baseFuncDesc {
            Name: ast::WindowFuncFirstValue.to_owned(),
            Args: vec![scalar],
            RetTp: Some(return_type),
        },
    };
    let pushdown_context = expression::NewPushDownContext(
        Arc::new(build_context),
        Some(Arc::new(SupportedPushDownClient)),
        false,
        None,
        None,
        1024,
    );

    assert!(!descriptor.CanPushDownToTiFlash(&pushdown_context));

    expression::infer_pushdown::replace_pushdown_blacklist([(
        "plus.plusint".to_owned(),
        expression::infer_pushdown::StoreType::TiFlash.mask(),
    )]);
    assert!(!descriptor.CanPushDownToTiFlash(&pushdown_context));
}
