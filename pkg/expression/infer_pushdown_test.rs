// Copyright 2026 AsterSQL.

use crate::fts_to_like_kernel::{Expression, FieldType, Signature};
use crate::infer_pushdown_kernel::{PushDownContext, StoreType, can_expr_push_down};

fn scalar(name: &str, signature: &str) -> Expression {
    Expression::scalar(
        name,
        Signature::Generic(signature.into()),
        Vec::new(),
        FieldType::integer(),
    )
}

#[test]
fn tiflash_rejects_signatures_not_explicitly_supported_by_go() {
    let context = PushDownContext::new(false, None, None, 0);

    for (name, signature) in [
        ("round", "RoundFuture"),
        ("truncate", "TruncateFuture"),
        ("least", "LeastDecimal"),
        ("greatest", "GreatestDecimal"),
    ] {
        assert!(
            !can_expr_push_down(
                &context,
                &scalar(name, signature),
                StoreType::TiFlash,
                false,
            ),
            "{name}.{signature} must stay local until Go explicitly enables it",
        );
    }
}

#[test]
fn tiflash_keeps_go_supported_signature_variants() {
    let context = PushDownContext::new(false, None, None, 0);

    for (name, signature) in [
        ("round", "RoundWithFracDec"),
        ("truncate", "TruncateUint"),
        ("least", "LeastString"),
        ("greatest", "GreatestReal"),
    ] {
        assert!(
            can_expr_push_down(
                &context,
                &scalar(name, signature),
                StoreType::TiFlash,
                false,
            ),
            "{name}.{signature} is explicitly enabled by Go",
        );
    }
}
