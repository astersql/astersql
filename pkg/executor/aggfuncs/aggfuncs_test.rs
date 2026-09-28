// Copyright 2026 AsterSQL.

use crate::{BaseAggFunc, FieldType};

#[test]
fn base_agg_func_uses_the_real_nullable_go_contract_types() {
    fn accepts_types_field_type(_: &astersql_types::field::FieldType) {}

    let base = BaseAggFunc::default();
    assert!(base.return_type.is_none());

    let field_type = FieldType::default();
    accepts_types_field_type(&field_type);
}
