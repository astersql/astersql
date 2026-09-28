// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.

// 聚合函数描述符转 tipb 表达式的单元测试。
//
// 覆盖常见聚合名与 DISTINCT 标志的 PB 编码，以及 SumInt 在 TiFlash/TiKV 上的类型映射。
// tipb 是 DistSQL 下推到存储引擎时使用的表达式协议。

use crate::*;
use std::sync::Arc;

/// 测试用 Client：声明支持所有请求类型，且禁止真正发送 RPC。
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

/// 构造指定 MySQL 类型的列参数，供聚合描述符使用。
fn column(field_type: u8) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(field_type),
        1,
        1,
        0,
    ))
}

/// 构造带支持型 Client 的下推上下文。
fn pushdown_context() -> expression::PushDownContext {
    expression::NewPushDownContext(
        Arc::new(exprstatic::NewExprContext(Vec::new())),
        Some(Arc::new(SupportedClient)),
        false,
        None,
        None,
        1024,
    )
}

/// 校验 Go 用例覆盖的聚合函数在 DISTINCT 开关下的 PB 类型、返回类型与附加字段。
#[test]
fn TestAggFunc2Pb() {
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let pushdown_context = pushdown_context();
    for (name, expected, return_type) in [
        (ast::AggFuncSum, tipb::ExprType::Sum, mysql::TypeDouble),
        (
            ast::AggFuncCount,
            tipb::ExprType::Count,
            mysql::TypeLonglong,
        ),
        (ast::AggFuncAvg, tipb::ExprType::Avg, mysql::TypeDouble),
        (
            ast::AggFuncGroupConcat,
            tipb::ExprType::GroupConcat,
            mysql::TypeVarchar,
        ),
        (ast::AggFuncMax, tipb::ExprType::Max, mysql::TypeDouble),
        (ast::AggFuncMin, tipb::ExprType::Min, mysql::TypeDouble),
        (
            ast::AggFuncFirstRow,
            tipb::ExprType::First,
            mysql::TypeDouble,
        ),
    ] {
        // DISTINCT 与否都应写入 has_distinct，且保留一个参数子节点。
        for distinct in [true, false] {
            let mut descriptor = NewAggFuncDesc(
                &expression_context,
                name,
                vec![column(mysql::TypeDouble)],
                distinct,
            )
            .unwrap();
            descriptor.RetTp = Some(*types::NewFieldType(return_type));
            let protobuf =
                AggFuncToPBExpr(&pushdown_context, &descriptor, kv::StoreType::UnSpecified)
                    .unwrap();
            assert_eq!(protobuf.get_tp(), expected);
            assert_eq!(protobuf.get_has_distinct(), distinct);
            assert_eq!(protobuf.get_children().len(), 1);
            assert_eq!(protobuf.get_field_type().get_tp(), return_type as i32);
            if expected == tipb::ExprType::GroupConcat {
                assert_eq!(
                    protobuf.get_val(),
                    codec::EncodeUint(Vec::new(), pushdown_context.GetGroupConcatMaxLen())
                );
                assert!(protobuf.get_order_by().is_empty());
            } else {
                assert!(protobuf.get_val().is_empty());
                assert!(protobuf.get_order_by().is_empty());
            }
        }
    }
}

/// 校验 SumInt 在 TiFlash 与 TiKV 上均可编码为 tipb::SumInt。
#[test]
fn TestAggFuncSumIntToPb() {
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let pushdown_context = pushdown_context();
    for store_type in [kv::StoreType::TiFlash, kv::StoreType::TiKV] {
        for distinct in [true, false] {
            let descriptor = NewAggFuncDesc(
                &expression_context,
                ast::AggFuncSumInt,
                vec![column(mysql::TypeLonglong)],
                distinct,
            )
            .unwrap();
            let protobuf = AggFuncToPBExpr(&pushdown_context, &descriptor, store_type).unwrap();
            assert_eq!(protobuf.get_tp(), tipb::ExprType::SumInt);
            assert_eq!(protobuf.get_has_distinct(), distinct);
        }
    }
}

/// Go 的 PBExprToAggFuncDesc 不从 PB 的 has_distinct 字段恢复 DISTINCT。
#[test]
fn pb_expr_to_agg_func_desc_matches_go_distinct_contract() {
    let context = exprstatic::NewExprContext(Vec::new());
    let mut protobuf = tipb::Expr::new();
    protobuf.set_tp(tipb::ExprType::Count);
    protobuf.set_field_type(expression::ToPBFieldType(&*types::NewFieldType(
        mysql::TypeLonglong,
    )));
    protobuf.set_has_distinct(true);

    let descriptor = PBExprToAggFuncDesc(&context, &protobuf, &[]).unwrap();
    assert!(!descriptor.HasDistinct);
}
