// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::*;

fn assert_send_sync<T: Send + Sync>() {}

fn int_type() -> types::FieldType {
    *types::NewFieldType(mysql::TypeLonglong)
}

fn round_trip(ctx: &dyn BuildContext, expr: &dyn Expression) -> Box<dyn Expression> {
    CachedExpression::try_from_expression(expr)
        .expect("expression should be cacheable")
        .restore(ctx)
        .expect("snapshot should restore")
}

#[test]
fn cached_expression_is_send_and_sync() {
    assert_send_sync::<CachedExpression>();
    assert_send_sync::<CachedColumn>();
    assert_send_sync::<CachedConstant>();
    assert_send_sync::<CachedScalarFunction>();
}

#[test]
fn four_expression_kinds_round_trip_with_hash_semantics() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let eval = ctx.GetEvalCtx();

    let column = Column::new(int_type(), 7, 11, 2);
    let restored = round_trip(&ctx, &column);
    assert!(restored.as_any().is::<Column>());
    assert_eq!(column.HashCode(), restored.HashCode());

    let correlated = CorrelatedColumn {
        column: column.clone(),
        data: Some(NewCorrelatedDatum(types::NewDatum(&42_i64))),
    };
    let restored = round_trip(&ctx, &correlated);
    let restored = restored
        .as_any()
        .downcast_ref::<CorrelatedColumn>()
        .expect("correlated column");
    assert_eq!(correlated.HashCode(), restored.HashCode());
    assert_eq!(
        restored
            .Eval(eval, chunk::Row::default())
            .unwrap()
            .GetInt64(),
        42
    );
    assert!(!Arc::ptr_eq(
        correlated.data.as_ref().unwrap(),
        restored.data.as_ref().unwrap()
    ));

    let mut parameter = Constant::with_type(types::NewDatum(&9_i64), int_type());
    parameter.ParamMarker = Some(ParamMarker::new(3));
    parameter.SubqueryRefID = 17;
    let restored = round_trip(&ctx, &parameter);
    let restored = restored.as_any().downcast_ref::<Constant>().unwrap();
    assert_eq!(restored.Value.GetInt64(), 9);
    assert_eq!(restored.ParamMarker.as_ref().unwrap().order(), 3);
    assert_eq!(restored.SubqueryRefID, 17);
    assert_eq!(parameter.HashCode(), restored.HashCode());

    let scalar = NewFunctionBase(
        &ctx,
        ast::Plus,
        int_type(),
        vec![Box::new(column), Box::new(parameter)],
    )
    .unwrap();
    let restored = round_trip(&ctx, scalar.as_ref());
    assert!(restored.as_any().is::<ScalarFunction>());
    assert_eq!(scalar.GetType(eval), restored.GetType(eval));
    assert_eq!(scalar.HashCode(), restored.HashCode());
}

#[test]
fn nested_builtin_and_deferred_constant_round_trip() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let ret = int_type();
    let inner = NewFunctionBase(
        &ctx,
        ast::Plus,
        ret.clone(),
        vec![
            Box::new(Constant::with_type(types::NewDatum(&1_i64), ret.clone())),
            Box::new(Constant::with_type(types::NewDatum(&2_i64), ret.clone())),
        ],
    )
    .unwrap();
    let inner = inner
        .as_any()
        .downcast_ref::<ScalarFunction>()
        .unwrap()
        .clone_scalar();
    let deferred = Constant::with_deferred(types::NewDatum(&3_i64), ret.clone(), inner);
    let outer = NewFunctionBase(&ctx, ast::UnaryMinus, ret, vec![Box::new(deferred)]).unwrap();

    let restored = round_trip(&ctx, outer.as_ref());
    assert_eq!(outer.HashCode(), restored.HashCode());
    let outer = restored.as_any().downcast_ref::<ScalarFunction>().unwrap();
    let deferred = outer.GetArgs()[0]
        .as_any()
        .downcast_ref::<Constant>()
        .unwrap();
    assert!(
        deferred
            .DeferredExpr
            .as_ref()
            .unwrap()
            .as_any()
            .is::<ScalarFunction>()
    );
}

#[test]
fn builtin_snapshot_uses_stable_id_and_registered_factory() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let ret = int_type();
    let scalar = NewFunctionBase(
        &ctx,
        ast::Plus,
        ret.clone(),
        vec![
            Box::new(Constant::with_type(types::NewDatum(&4_i64), ret.clone())),
            Box::new(Constant::with_type(types::NewDatum(&5_i64), ret)),
        ],
    )
    .unwrap();

    let snapshot = CachedExpression::try_from_expression(scalar.as_ref()).unwrap();
    let CachedExpression::ScalarFunction(function) = &snapshot else {
        panic!("expected scalar-function snapshot");
    };
    assert_eq!(function.builtin_id.as_str(), "builtin:v1:plus");

    let restored = snapshot.restore(&ctx).unwrap();
    assert_eq!(scalar.HashCode(), restored.HashCode());
}

#[test]
fn builtin_snapshot_rejects_unknown_stable_id() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let scalar = NewFunctionBase(
        &ctx,
        ast::Plus,
        int_type(),
        vec![
            Box::new(Constant::with_type(types::NewDatum(&4_i64), int_type())),
            Box::new(Constant::with_type(types::NewDatum(&5_i64), int_type())),
        ],
    )
    .unwrap();
    let mut snapshot = CachedExpression::try_from_expression(scalar.as_ref()).unwrap();
    let CachedExpression::ScalarFunction(function) = &mut snapshot else {
        panic!("expected scalar-function snapshot");
    };
    function.builtin_id = CachedBuiltinId::from_raw_for_test("builtin:v1:not_registered");

    let error = match snapshot.restore(&ctx) {
        Ok(_) => panic!("unknown builtin id must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unknown builtin snapshot id"));
}

#[test]
fn every_core_snapshot_builtin_has_a_stable_id() {
    let mut names = funcs.names();
    names.extend([
        ast::Cast.to_owned(),
        ast::GetVar.to_owned(),
        InternalFuncFromBinary.to_owned(),
        InternalFuncToBinary.to_owned(),
    ]);
    names.sort_unstable();
    names.dedup();

    for name in names {
        let id = CachedBuiltinId::from_name(&name).unwrap();
        assert_eq!(id.as_str(), format!("builtin:v1:{name}"));
        assert_eq!(id.registered_name().unwrap(), name);
    }
}

#[test]
fn cached_schema_round_trip_preserves_columns_and_keys() {
    assert_send_sync::<CachedSchema>();
    assert_send_sync::<CachedNameSlice>();

    let ctx = exprstatic::NewExprContext(Vec::new());
    let virtual_expr = NewFunctionBase(
        &ctx,
        ast::Plus,
        int_type(),
        vec![
            Box::new(Column::new(int_type(), 1, 101, 0)),
            Box::new(Constant::with_type(types::NewDatum(&1_i64), int_type())),
        ],
    )
    .unwrap();

    let mut first = Column::new(int_type(), 1, 101, 0);
    first.OrigName = "t.a".to_owned();
    first.VirtualExpr = Some(virtual_expr);
    let mut prefix = first.clone();
    prefix.Index = 1;
    prefix.IsPrefix = true;
    let mut second = Column::new(int_type(), 2, 202, 2);
    second.OrigName = "t.b".to_owned();
    second.IsHidden = true;

    let mut schema = NewSchema(vec![first.clone(), prefix.clone(), second.clone()]);
    schema.SetKeys(vec![
        vec![first.clone(), second.clone()],
        vec![first.clone()],
    ]);
    schema.SetUniqueKeys(vec![
        vec![prefix.clone(), second.clone()],
        vec![prefix.clone(), prefix.clone()],
    ]);

    let snapshot = CachedSchema::try_from_schema(&schema).unwrap();
    let restored = snapshot.restore(&ctx).unwrap();

    assert_eq!(schema.String(), restored.String());
    assert!(schema.Equal(&restored));
    assert!(restored.IsUnique(true, &[second.clone(), first.clone()]));
    assert!(restored.IsUnique(false, &[prefix.clone(), second.clone()]));
    assert_eq!(restored.PKOrUK[0][0].UniqueID, 101);
    assert_eq!(restored.PKOrUK[0][1].UniqueID, 202);
    assert_eq!(restored.NullableUK[1].len(), 2);
    assert_eq!(restored.NullableUK[1][0].UniqueID, 101);
    assert_eq!(restored.NullableUK[1][1].UniqueID, 101);
    assert!(restored.Columns[0].VirtualExpr.is_some());
    assert!(restored.Columns[1].IsPrefix);
    assert_eq!(restored.Columns[0].RetType, first.RetType);

    let names = types::NameSlice(vec![
        Some(Arc::new(types::FieldName {
            OrigTblName: ast::NewCIStr("OrigT"),
            OrigColName: ast::NewCIStr("OrigA"),
            DBName: ast::NewCIStr("DB"),
            TblName: ast::NewCIStr("AliasT"),
            ColName: ast::NewCIStr("AliasA"),
            Hidden: false,
            NotExplicitUsable: true,
            Redundant: true,
        })),
        None,
        Some(types::EmptyName.clone()),
    ]);
    let restored_names = CachedNameSlice::from_name_slice(&names).restore();
    assert_eq!(restored_names.0.len(), 3);
    let restored_name = restored_names.0[0].as_ref().unwrap();
    assert_eq!(restored_name.String(), "db.aliast.aliasa");
    assert_eq!(restored_name.OrigTblName.O, "OrigT");
    assert_eq!(restored_name.OrigColName.O, "OrigA");
    assert!(restored_name.NotExplicitUsable);
    assert!(restored_name.Redundant);
    assert!(restored_names.0[1].is_none());
    assert!(restored_names.0[2].as_ref().unwrap().Hidden);
}
