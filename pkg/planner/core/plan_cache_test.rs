// Copyright 2026 AsterSQL.

use crate::{Datum, PlanCacheParamMarker, PlanCacheStmt, SetParameterValuesIntoSCtx, ast};

#[test]
fn set_parameter_values_updates_each_marker_datum_in_order() {
    let mut statement = PlanCacheStmt::<()>::new(
        ast::misc::Prepared::default(),
        "select * from t where a = ? and b = ?",
    );
    statement.Params = vec![
        PlanCacheParamMarker {
            offset: 26,
            ..Default::default()
        },
        PlanCacheParamMarker {
            offset: 36,
            ..Default::default()
        },
    ];

    SetParameterValuesIntoSCtx(
        &mut statement,
        vec![Datum::Int(7), Datum::String("aster".to_owned())],
    )
    .unwrap();

    assert_eq!(statement.Params[0].datum, Some(Datum::Int(7)));
    assert_eq!(
        statement.Params[1].datum,
        Some(Datum::String("aster".to_owned()))
    );
    assert!(statement.Params.iter().all(|marker| marker.in_execute));
}

#[test]
fn wrong_parameter_count_does_not_partially_mutate_markers() {
    let mut statement = PlanCacheStmt::<()>::new(
        ast::misc::Prepared::default(),
        "select * from t where a = ? and b = ?",
    );
    statement.Params = vec![
        PlanCacheParamMarker::default(),
        PlanCacheParamMarker::default(),
    ];

    assert!(SetParameterValuesIntoSCtx(&mut statement, vec![Datum::Int(7)]).is_err());
    assert!(
        statement
            .Params
            .iter()
            .all(|marker| { !marker.in_execute && marker.datum.is_none() })
    );
}
