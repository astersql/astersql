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

// 表达式到 TiPB（Tipb）协议的编码与下推辅助。
//
// 将 Constant / Column / CorrelatedColumn / ScalarFunction 转为 `tipb::Expr`，
// 供 DistSQL/DAG 下推到 TiKV/TiFlash。包含字段类型往返、GROUP BY / ORDER BY 项编码，
// 以及与 Go 一致的下推黑名单检查。

// Constructs TiPB expressions in memory only; capability checks are delegated
// to the KV client, but this module does not issue RPCs.

use crate::*;
use protobuf::ProtobufEnum;

/// 对应 Go ExpressionsToPBList：任一表达式不可下推时返回带表达式文本的内部错误。
pub fn ExpressionsToPBList(
    ctx: &dyn EvalContext,
    exprs: &[ExprBox],
    client: &dyn kv::Client,
) -> Result<Vec<tipb::Expr>, Error> {
    let converter = NewPBConverter(client, ctx);
    let mut result = Vec::with_capacity(exprs.len());
    for expr in exprs {
        let encoded = converter.ExprToPB(expr.as_ref()).ok_or_else(|| {
            errors::New(format!(
                "expression {} cannot be pushed down",
                expr.StringWithCtx(Some(ctx), errors::RedactLogDisable)
            ))
        })?;
        result.push(encoded);
    }
    Ok(result)
}

/// 对应 Go ProjectionExpressionsToPBList：顶层 Column 不做类型检查，因为它本身不表示计算。
pub fn ProjectionExpressionsToPBList(
    ctx: &dyn EvalContext,
    exprs: &[ExprBox],
    client: &dyn kv::Client,
) -> Result<Vec<tipb::Expr>, Error> {
    let converter = NewPBConverter(client, ctx);
    let mut result = Vec::with_capacity(exprs.len());
    for expr in exprs {
        let encoded = if let Some(column) = expr.as_any().downcast_ref::<Column>() {
            converter.columnToPBExpr(column, false)
        } else {
            converter.ExprToPB(expr.as_ref())
        }
        .ok_or_else(|| {
            errors::New(format!(
                "expression {} cannot be pushed down",
                expr.StringWithCtx(Some(ctx), errors::RedactLogDisable)
            ))
        })?;
        result.push(encoded);
    }
    Ok(result)
}

/// PbConverter 对应 Go 同名结构：client 提供能力探测，ctx 提供求值和类型上下文。
pub struct PbConverter<'a> {
    client: &'a dyn kv::Client,
    ctx: &'a dyn EvalContext,
}

/// 对应 Go NewPBConverter。
pub fn NewPBConverter<'a>(client: &'a dyn kv::Client, ctx: &'a dyn EvalContext) -> PbConverter<'a> {
    PbConverter { client, ctx }
}

impl<'a> PbConverter<'a> {
    /// 对应 Go ExprToPB：只接受 Constant、CorrelatedColumn、Column 与 ScalarFunction。
    pub fn ExprToPB(&self, expr: &dyn Expression) -> Option<tipb::Expr> {
        if expr.as_any().is::<Constant>() || expr.as_any().is::<CorrelatedColumn>() {
            self.conOrCorColToPBExpr(expr)
        } else if let Some(column) = expr.as_any().downcast_ref::<Column>() {
            self.columnToPBExpr(column, true)
        } else if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
            self.scalarFuncToPBExpr(function)
        } else {
            None
        }
    }

    /// 常量与关联列先在 root 求值，再编码为协议字面量；不支持的请求类型拒绝下推。
    fn conOrCorColToPBExpr(&self, expr: &dyn Expression) -> Option<tipb::Expr> {
        let field_type = expr.GetType(self.ctx);
        let datum = match expr.Eval(self.ctx, chunk::Row::default()) {
            Ok(datum) => datum,
            Err(err) => {
                logutil::BgLogger().error(format!(
                    "eval constant or correlated column {}: {err}",
                    expr.ExplainInfo(self.ctx)
                ));
                return None;
            }
        };
        let (expr_type, value) = self.encodeDatum(field_type, datum)?;
        if !self
            .client
            .IsRequestTypeSupported(kv::ReqTypeSelect, expr_type as i64)
        {
            return None;
        }
        let mut encoded = tipb::Expr::new();
        encoded.set_tp(expr_type);
        encoded.set_val(value);
        encoded.set_field_type(ToPBFieldType(field_type));
        Some(encoded)
    }

    /// 对应 Go encodeDatum：按 Datum kind 选择 TiPB 类型及 codec 编码。
    fn encodeDatum(
        &self,
        field_type: &types::FieldType,
        datum: types::Datum,
    ) -> Option<(tipb::ExprType, Vec<u8>)> {
        let pair = match datum.Kind() {
            types::KindNull => (tipb::ExprType::Null, Vec::new()),
            types::KindInt64 => (
                tipb::ExprType::Int64,
                codec::EncodeInt(Vec::new(), datum.GetInt64()),
            ),
            types::KindUint64 => (
                tipb::ExprType::Uint64,
                codec::EncodeUint(Vec::new(), datum.GetUint64()),
            ),
            types::KindString | types::KindBinaryLiteral => {
                (tipb::ExprType::String, datum.GetBytes())
            }
            types::KindMysqlBit => (tipb::ExprType::MysqlBit, datum.GetBytes()),
            types::KindBytes => (tipb::ExprType::Bytes, datum.GetBytes()),
            types::KindFloat32 => {
                let value = datum.GetFloat64();
                // mem-comparable 浮点编码会把 -0 归一为 +0；为避免语义漂移，负零留在 root。
                if value == 0.0 && value.is_sign_negative() {
                    return None;
                }
                (
                    tipb::ExprType::Float32,
                    codec::EncodeFloat(Vec::new(), value),
                )
            }
            types::KindFloat64 => {
                let value = datum.GetFloat64();
                if value == 0.0 && value.is_sign_negative() {
                    return None;
                }
                (
                    tipb::ExprType::Float64,
                    codec::EncodeFloat(Vec::new(), value),
                )
            }
            types::KindMysqlDuration => (
                tipb::ExprType::MysqlDuration,
                codec::EncodeInt(Vec::new(), datum.GetMysqlDuration().Duration),
            ),
            types::KindMysqlDecimal => {
                // 精度和小数位必须来自 MyDecimal，而非输出 schema，避免协议计算丢失精度。
                let encoded = match codec::EncodeDecimal(Vec::new(), &datum.GetMysqlDecimal(), 0, 0)
                {
                    Ok(value) => value,
                    Err(err) => {
                        logutil::BgLogger().error(format!("encode decimal: {err}"));
                        return None;
                    }
                };
                (tipb::ExprType::MysqlDecimal, encoded)
            }
            types::KindMysqlTime => {
                if !self
                    .client
                    .IsRequestTypeSupported(kv::ReqTypeDAG, tipb::ExprType::MysqlTime as i64)
                {
                    return None;
                }
                let type_context = typeCtx(self.ctx);
                let encoded = codec::EncodeMySQLTime(
                    type_context.Location(),
                    datum.GetMysqlTime(),
                    field_type.GetType(),
                    Vec::new(),
                );
                match encoded {
                    Ok(value) => return Some((tipb::ExprType::MysqlTime, value)),
                    Err(err) => {
                        if let Some(unhandled) = errCtx(self.ctx).HandleError(Some(err)) {
                            logutil::BgLogger().error(format!("encode mysql time: {unhandled}"));
                        }
                        return None;
                    }
                }
            }
            types::KindMysqlEnum => (
                tipb::ExprType::MysqlEnum,
                codec::EncodeUint(Vec::new(), datum.GetUint64()),
            ),
            types::KindVectorFloat32 => (
                tipb::ExprType::TiDbVectorFloat32,
                datum.GetVectorFloat32().ZeroCopySerialize().to_vec(),
            ),
            _ => return None,
        };
        Some(pair)
    }

    /// 对应 Go columnToPBExpr：先检查 client 能力，再按类型开关和协议版本选择 index 或 ID。
    fn columnToPBExpr(&self, column: &Column, check_type: bool) -> Option<tipb::Expr> {
        if !self
            .client
            .IsRequestTypeSupported(kv::ReqTypeSelect, tipb::ExprType::ColumnRef as i64)
        {
            return None;
        }
        if check_type {
            match column.GetType(self.ctx).GetType() {
                mysql::TypeBit if !IsPushDownEnabled("bit", kv::StoreType::TiKV) => {
                    return None;
                }
                mysql::TypeSet | mysql::TypeGeometry | mysql::TypeUnspecified => return None,
                mysql::TypeEnum if !IsPushDownEnabled("enum", kv::StoreType::UnSpecified) => {
                    return None;
                }
                _ => {}
            }
        }

        if self
            .client
            .IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic)
        {
            let mut encoded = tipb::Expr::new();
            encoded.set_tp(tipb::ExprType::ColumnRef);
            encoded.set_val(codec::EncodeInt(Vec::new(), column.Index as i64));
            encoded.set_field_type(ToPBFieldType(column.GetType(self.ctx)));
            return Some(encoded);
        }

        // 旧协议使用列 ID；0 与 -1 都不是可推送的真实表列。
        if column.ID == 0 || column.ID == -1 {
            return None;
        }
        let mut encoded = tipb::Expr::new();
        encoded.set_tp(tipb::ExprType::ColumnRef);
        encoded.set_val(codec::EncodeInt(Vec::new(), column.ID));
        Some(encoded)
    }

    /// 对应 Go scalarFuncToPBExpr：校验签名与可推送性，递归编码参数，再附加 metadata 和返回类型。
    fn scalarFuncToPBExpr(&self, expr: &ScalarFunction) -> Option<tipb::Expr> {
        let pb_code = expr.Function.PbCode();
        if pb_code <= tipb::ScalarFuncSig::Unspecified as i32 {
            return None;
        }
        if !canFuncBePushed(self.ctx, expr, kv::StoreType::UnSpecified) {
            return None;
        }

        let mut children = Vec::with_capacity(expr.GetArgs().len());
        for arg in expr.GetArgs() {
            // 任何一个参数不能编码，整个标量函数都留在 root。
            children.push(self.ExprToPB(arg.as_ref())?);
        }

        let encoded_metadata = expr.Function.metadata().unwrap_or_default();

        let mut return_type = expr.RetType.clone()?;
        if collate::NewCollationEnabled() {
            let (_, collation) = expr.CharsetAndCollation();
            // Go 强制把派生排序规则写回 RetType，以便 TiKV/MockTiKV 收到一致信息。
            return_type.SetCollate(collation);
        }
        let mut encoded = tipb::Expr::new();
        encoded.set_tp(tipb::ExprType::ScalarFunc);
        encoded.set_val(encoded_metadata);
        encoded.set_sig(tipb::ScalarFuncSig::from_i32(pb_code)?);
        encoded.set_children(children.into());
        encoded.set_field_type(ToPBFieldType(&return_type));
        Some(encoded)
    }
}

/// 将 kv::StoreType 映射为 infer_pushdown_kernel 使用的存储类型枚举。
fn pushdownStoreType(store_type: kv::StoreType) -> infer_pushdown_kernel::StoreType {
    match store_type {
        kv::StoreType::TiKV => infer_pushdown_kernel::StoreType::TiKV,
        kv::StoreType::TiFlash => infer_pushdown_kernel::StoreType::TiFlash,
        kv::StoreType::TiDB => infer_pushdown_kernel::StoreType::TiDB,
        kv::StoreType::UnSpecified => infer_pushdown_kernel::StoreType::Unspecified,
    }
}

/// Checks the shared expression-pushdown blacklist with Go-compatible store masks.
/// 查询共享下推黑名单；掩码语义与 Go 各存储引擎一致。
pub fn IsPushDownEnabled(name: &str, store_type: kv::StoreType) -> bool {
    infer_pushdown_kernel::is_push_down_enabled(name, pushdownStoreType(store_type))
}

/// Scalar pushdown requires a TiPB signature and passes both blacklist keys used by Go:
/// the SQL function name and the function/signature pair.
/// 标量下推需有合法 TiPB 签名，并同时通过「函数名」与「函数.签名」两道黑名单键检查。
fn canFuncBePushed(
    _ctx: &dyn EvalContext,
    function: &ScalarFunction,
    store_type: kv::StoreType,
) -> bool {
    if !IsPushDownEnabled(&function.FuncName.L, store_type) {
        return false;
    }
    let Some(signature) = tipb::ScalarFuncSig::from_i32(function.Function.PbCode()) else {
        return false;
    };
    // rust-protobuf 2.x generated enums in TiPB do not implement descriptor
    // lookup; Debug is the generated enum symbol and matches Go `String()`.
    let full_name = format!("{}.{signature:?}", function.FuncName.L).to_ascii_lowercase();
    IsPushDownEnabled(&full_name, store_type)
}

/// 对应 Go ToPBFieldType：逐字段复制类型、flag、flen、decimal、charset、collation 与 elems。
pub fn ToPBFieldType(field_type: &types::FieldType) -> tipb::FieldType {
    let mut encoded = tipb::FieldType::new();
    encoded.set_tp(field_type.GetType() as i32);
    encoded.set_flag(field_type.GetFlag() as u32);
    encoded.set_flen(field_type.GetFlen() as i32);
    encoded.set_decimal(field_type.GetDecimal() as i32);
    encoded.set_charset(field_type.GetCharset().to_owned());
    encoded.set_collate(collate::CollationToProto(field_type.GetCollate()));
    encoded.set_elems(field_type.GetElems().to_vec().into());
    encoded
}

/// 对应 Go ToPBFieldTypeWithCheck：TiFlash 拒绝 flen/decimal 非法的 Decimal。
pub fn ToPBFieldTypeWithCheck(
    field_type: &types::FieldType,
    store_type: kv::StoreType,
) -> Result<tipb::FieldType, Error> {
    if store_type == kv::StoreType::TiFlash && !field_type.IsDecimalValid() {
        return Err(errors::New(format!(
            "{} can not be pushed to TiFlash because it contains invalid decimal('{}','{}').",
            field_type,
            field_type.GetFlen(),
            field_type.GetDecimal()
        )));
    }
    Ok(ToPBFieldType(field_type))
}

/// 对应 Go FieldTypeFromPB：协议排序规则转换回来后再恢复 elems。
pub fn FieldTypeFromPB(field_type: &tipb::FieldType) -> types::FieldType {
    let mut local = types::FieldType::default();
    local.SetType(field_type.get_tp() as u8);
    local.SetFlag(field_type.get_flag() as usize);
    local.SetFlen(field_type.get_flen() as isize);
    local.SetDecimal(field_type.get_decimal() as isize);
    local.SetCharset(field_type.get_charset().to_owned());
    local.SetCollate(collate::ProtoToCollation(field_type.get_collate()));
    local.SetElems(field_type.get_elems().to_vec());
    local
}

/// 对应 Go GroupByItemToPB：不可下推时返回 None。
pub fn GroupByItemToPB(
    ctx: &dyn EvalContext,
    client: &dyn kv::Client,
    expr: &dyn Expression,
) -> Option<tipb::ByItem> {
    let mut item = tipb::ByItem::new();
    item.set_expr(NewPBConverter(client, ctx).ExprToPB(expr)?);
    Some(item)
}

/// 对应 Go SortByItemToPB：除表达式外原样携带 desc 排序方向。
pub fn SortByItemToPB(
    ctx: &dyn EvalContext,
    client: &dyn kv::Client,
    expr: &dyn Expression,
    desc: bool,
) -> Option<tipb::ByItem> {
    let mut item = tipb::ByItem::new();
    item.set_expr(NewPBConverter(client, ctx).ExprToPB(expr)?);
    item.set_desc(desc);
    Some(item)
}
