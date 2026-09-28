// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.

// `distinctChecker` 单元测试：验证首次/重复组合与含 NULL 的去重行为。

use crate::*;
use std::sync::Arc;

#[test]
fn TestDistinct() {
    let context: Arc<dyn expression::EvalContext> =
        Arc::new(exprstatic::NewEvalContext(Vec::new()));
    let mut checker = createDistinctChecker(context);
    // 期望：同一组合第二次返回 false；含默认 Datum(NULL) 的组合同样去重。
    for (values, expected) in [
        (vec![types::NewIntDatum(1), types::NewIntDatum(1)], true),
        (vec![types::NewIntDatum(1), types::NewIntDatum(1)], false),
        (vec![types::NewIntDatum(1), types::NewIntDatum(2)], true),
        (vec![types::NewIntDatum(1), types::NewIntDatum(2)], false),
        (vec![types::NewIntDatum(1), types::Datum::default()], true),
        (vec![types::NewIntDatum(1), types::Datum::default()], false),
    ] {
        assert_eq!(checker.Check(values).unwrap(), expected);
    }
}
