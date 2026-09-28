// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

fn int_column(unique_id: i64) -> Column {
    Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        unique_id,
        unique_id,
        unique_id as isize,
    )
}

fn binary(
    ctx: &dyn BuildContext,
    name: &str,
    left: Box<dyn Expression>,
    right: Box<dyn Expression>,
) -> Box<dyn Expression> {
    NewFunctionInternal(
        ctx,
        name,
        *types::NewFieldType(mysql::TypeLonglong),
        vec![left, right],
    )
    .expect("the test expression must be constructible")
}

#[test]
fn borrowed_join_propagation_matches_owned_context_and_preserves_join_key() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let left = int_column(1);
    let right = int_column(2);
    let conditions = vec![
        binary(
            &ctx,
            ast::EQ,
            Box::new(left.clone()),
            Box::new(right.clone()),
        ),
        binary(
            &ctx,
            ast::EQ,
            Box::new(left.clone()),
            Box::new(Constant::with_type(
                types::NewIntDatum(1),
                *types::NewFieldType(mysql::TypeLonglong),
            )),
        ),
    ];
    let render = |items: Vec<Box<dyn Expression>>| {
        items
            .iter()
            .map(|item| item.StringWithCtx(None, errors::RedactLogDisable))
            .collect::<Vec<_>>()
    };
    for keep in [false, true] {
        let borrowed = PropagateConstantForJoinRef(
            &ctx,
            keep,
            NewSchema(vec![left.clone()]),
            NewSchema(vec![right.clone()]),
            None,
            conditions.clone(),
        );
        let owned = crate::constant_propagation_kernel::PropagateConstantForJoin(
            Box::new(exprstatic::NewExprContext(Vec::new())),
            keep,
            NewSchema(vec![left.clone()]),
            NewSchema(vec![right.clone()]),
            None,
            conditions.clone(),
        );
        let borrowed = render(borrowed);
        assert_eq!(borrowed, render(owned));
        if keep {
            assert!(borrowed.iter().any(|item| item == "eq(Column#1, Column#2)"));
        }
    }
}

#[test]
fn non_equality_column_comparison_does_not_create_an_equivalence_class() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let left = int_column(1);
    let right = int_column(2);
    let conditions = vec![
        binary(&ctx, ast::NE, Box::new(left.clone()), Box::new(right)),
        binary(
            &ctx,
            ast::GT,
            Box::new(left),
            Box::new(Constant::with_type(
                types::NewIntDatum(1),
                *types::NewFieldType(mysql::TypeLonglong),
            )),
        ),
    ];

    let propagated = PropagateConstantRef(&ctx, None, conditions);
    let rendered = propagated
        .iter()
        .map(|expr| expr.StringWithCtx(None, errors::RedactLogDisable))
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["ne(Column#1, Column#2)", "gt(Column#1, 1)"]);
}

#[test]
fn outer_join_constant_evaluation_error_preserves_the_original_conditions() {
    let mut parameter = Constant::with_type(
        types::NewIntDatum(0),
        *types::NewFieldType(mysql::TypeLonglong),
    );
    parameter.ParamMarker = Some(ParamMarker::new(usize::MAX));

    let (join, filter) = crate::constant_propagation_kernel::PropConstForOuterJoin(
        Box::new(exprstatic::NewExprContext(Vec::new())),
        Vec::new(),
        vec![Box::new(parameter)],
        NewSchema(Vec::new()),
        NewSchema(Vec::new()),
        false,
        false,
        None,
    );

    assert!(join.is_empty());
    assert_eq!(filter.len(), 1);
    assert!(
        filter[0]
            .as_constant()
            .and_then(|constant| constant.ParamMarker.as_ref())
            .is_some()
    );
}
