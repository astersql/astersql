// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;

struct SupportedClient;

impl kv::Client for SupportedClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("PB conversion must not send a request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

fn column(index: isize) -> expression::ExprBox {
    column_type(index, mysql::TypeLonglong)
}

fn column_type(index: isize, field_type: u8) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(field_type),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

fn row(values: &[Option<i64>]) -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(
        values
            .iter()
            .map(|value| value.map_or_else(types::Datum::default, types::NewIntDatum))
            .collect(),
    )
    .ToRow()
    .CopyConstruct()
}

#[test]
fn go_merge_44_descriptor_and_pushdown() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    for name in [ast::AggFuncMaxCount, ast::AggFuncMinCount] {
        for input_type in [mysql::TypeDouble, mysql::TypeBit] {
            let inferred =
                NewAggFuncDesc(&ctx, name, vec![column_type(0, input_type)], false).unwrap();
            assert_eq!(
                inferred.RetTp.as_ref().unwrap().GetType(),
                mysql::TypeLonglong
            );
            assert_eq!(inferred.RetTp.as_ref().unwrap().GetFlen(), 21);
            assert_eq!(inferred.RetTp.as_ref().unwrap().GetDecimal(), 0);
            assert!(mysql::HasNotNullFlag(
                inferred.RetTp.as_ref().unwrap().GetFlag()
            ));
            let (_, final_desc) = inferred.Split(&[4]);
            assert_eq!(
                final_desc.Args[0].GetType(ctx.GetEvalCtx()).GetType(),
                input_type
            );
        }
        let mut desc = NewAggFuncDesc(&ctx, name, vec![column(0)], false).unwrap();
        assert_eq!(desc.RetTp.as_ref().unwrap().GetType(), mysql::TypeLonglong);
        assert_eq!(desc.RetTp.as_ref().unwrap().GetFlen(), 21);
        assert_eq!(desc.RetTp.as_ref().unwrap().GetDecimal(), 0);
        assert!(mysql::HasNotNullFlag(
            desc.RetTp.as_ref().unwrap().GetFlag()
        ));
        assert_eq!(desc.GetDefaultValue().GetInt64(), 0);
        assert!(noNeedCastAggFuncs().contains(name));
        assert!(NeedCount(name));
        assert!(NeedValue(name));
        assert!(CheckAggPushDown(
            ctx.GetEvalCtx(),
            &desc,
            kv::StoreType::TiFlash
        ));
        assert!(!CheckAggPushDown(
            ctx.GetEvalCtx(),
            &desc,
            kv::StoreType::TiKV
        ));
        assert_eq!(
            desc.GetTiPBExpr(false),
            if name == ast::AggFuncMaxCount {
                tipb::ExprType::MaxCount
            } else {
                tipb::ExprType::MinCount
            }
        );
        let push = expression::NewPushDownContext(
            Arc::new(exprstatic::NewExprContext(Vec::new())),
            Some(Arc::new(SupportedClient)),
            false,
            None,
            None,
            1024,
        );
        let protobuf = AggFuncToPBExpr(&push, &desc, kv::StoreType::TiFlash).unwrap();
        assert_eq!(protobuf.get_tp(), desc.GetTiPBExpr(false));
        let decoded = PBExprToAggFuncDesc(
            &ctx,
            &protobuf,
            &[*types::NewFieldType(mysql::TypeLonglong)],
        )
        .unwrap();
        assert_eq!(decoded.Name, name);
        let (mut distributed, distributed_desc) = NewDistAggFunc(
            &protobuf,
            &[*types::NewFieldType(mysql::TypeLonglong)],
            &ctx,
        )
        .unwrap();
        assert_eq!(distributed_desc.Name, name);
        let mut distributed_state =
            distributed.CreateContext(Arc::new(exprstatic::NewEvalContext(Vec::new())));
        assert_eq!(distributed.GetResult(&distributed_state).GetInt64(), 0);
        distributed
            .Update(
                &mut distributed_state,
                &stmtctx::NewStmtCtx(),
                row(&[Some(7)]),
            )
            .unwrap();
        assert_eq!(distributed.GetResult(&distributed_state).GetInt64(), 1);
        for mode in [Partial1Mode, Partial2Mode, FinalMode] {
            desc.Mode = mode;
            assert!(CheckAggPushDown(
                ctx.GetEvalCtx(),
                &desc,
                kv::StoreType::TiFlash
            ));
        }
        let (partial, final_desc) = desc.Split(&[4]);
        assert_eq!(partial.Mode, Partial2Mode);
        assert_eq!(
            final_desc.Args[0].GetType(ctx.GetEvalCtx()).GetType(),
            mysql::TypeLonglong
        );
        assert_eq!(
            final_desc.Args[0]
                .as_any()
                .downcast_ref::<expression::Column>()
                .unwrap()
                .Index,
            4
        );
        desc.Mode = DedupMode;
        assert!(!CheckAggPushDown(
            ctx.GetEvalCtx(),
            &desc,
            kv::StoreType::TiFlash
        ));
        desc.Mode = FinalMode;
        desc.Args.push(column(1));
        assert!(!CheckAggPushDown(
            ctx.GetEvalCtx(),
            &desc,
            kv::StoreType::TiFlash
        ));
    }
}

#[test]
fn go_merge_44_complete_and_final_count_extrema() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let statement = stmtctx::NewStmtCtx();
    for (name, expected_extreme, expected_count) in
        [(ast::AggFuncMaxCount, 3, 2), (ast::AggFuncMinCount, 1, 2)]
    {
        let desc = NewAggFuncDesc(&ctx, name, vec![column(0)], false).unwrap();
        let mut function = desc.GetAggFunc(&ctx);
        let eval: Arc<dyn expression::EvalContext> =
            Arc::new(exprstatic::NewEvalContext(Vec::new()));
        let mut state = function.CreateContext(Arc::clone(&eval));
        assert_eq!(function.GetResult(&state).GetInt64(), 0);
        for value in [Some(2), Some(3), Some(3), Some(1), Some(1), None] {
            function
                .Update(&mut state, &statement, row(&[value]))
                .unwrap();
        }
        assert_eq!(function.GetResult(&state).GetInt64(), expected_count);
        let partial = function.GetPartialResult(&state);
        assert_eq!(partial[0].GetInt64(), expected_count);
        assert_eq!(partial[1].GetInt64(), expected_extreme);

        let mut final_desc = NewAggFuncDesc(&ctx, name, vec![column(0), column(1)], false).unwrap();
        final_desc.Mode = FinalMode;
        let mut final_function = final_desc.GetAggFunc(&ctx);
        let mut final_state = final_function.CreateContext(eval);
        for ignored in [
            row(&[Some(0), Some(9)]),
            row(&[None, Some(9)]),
            row(&[Some(9), None]),
        ] {
            final_function
                .Update(&mut final_state, &statement, ignored)
                .unwrap();
        }
        assert_eq!(final_function.GetResult(&final_state).GetInt64(), 0);
        for (count, value) in [(2, 3), (1, 3), (4, 2), (3, 1), (2, 1)] {
            final_function
                .Update(
                    &mut final_state,
                    &statement,
                    row(&[Some(count), Some(value)]),
                )
                .unwrap();
        }
        assert_eq!(
            final_function.GetResult(&final_state).GetInt64(),
            if name == ast::AggFuncMaxCount { 3 } else { 5 }
        );
        final_function.ResetContext(
            Arc::new(exprstatic::NewEvalContext(Vec::new())),
            &mut final_state,
        );
        assert_eq!(final_function.GetResult(&final_state).GetInt64(), 0);
        assert!(final_function.GetPartialResult(&final_state)[1].IsNull());
    }
}

#[test]
fn go_merge_44_outer_join_count_defaults_and_not_null_flag() {
    let mut ctx = exprstatic::NewExprContext(Vec::new());
    let schema = expression::NewSchema(Vec::new());
    for name in [ast::AggFuncMaxCount, ast::AggFuncMinCount] {
        let value = Box::new(expression::Constant::with_type(
            types::NewIntDatum(7),
            *types::NewFieldType(mysql::TypeLonglong),
        ));
        let mut desc = NewAggFuncDesc(&ctx, name, vec![value], false).unwrap();
        let (result, valid) = desc.EvalNullValueInOuterJoin(&mut ctx, &schema).unwrap();
        assert!(valid);
        assert_eq!(result.GetInt64(), 1);
        desc.UpdateNotNullFlag4RetType(false, false).unwrap();
        assert!(mysql::HasNotNullFlag(
            desc.RetTp.as_ref().unwrap().GetFlag()
        ));
    }
}
