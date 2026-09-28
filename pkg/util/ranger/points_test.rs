// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use crate::points_impl::builder;
use crate::test_support_aster_unit_test::{column, field_type, ranger_context, scalar};
use crate::{ast, mysql, types};

#[test]
fn binary_comparison_eval_error_does_not_set_builder_error() {
    let context = ranger_context();
    let indexed = column(1, 0);
    let mut lazy_constant = expression::Constant::with_type(
        types::NewIntDatum(1),
        indexed.RetType.clone().expect("test column has a type"),
    );
    lazy_constant.ParamMarker = Some(expression::ParamMarker::new(0));
    for args in [
        vec![
            Box::new(indexed.clone()) as expression::ExprBox,
            Box::new(lazy_constant.Clone()),
        ],
        vec![Box::new(lazy_constant.Clone()), Box::new(indexed.clone())],
    ] {
        let comparison = scalar(&context, ast::EQ, args);
        let mut range_builder = builder {
            err: None,
            sctx: &context,
        };

        let points = range_builder.build(
            comparison.as_ref(),
            &field_type(mysql::TypeLonglong),
            types::UnspecifiedLength,
            false,
        );

        assert!(points.is_empty());
        assert!(range_builder.err.is_none());
    }
}
