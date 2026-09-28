// Copyright 2026 AsterSQL.

// `DomainContext` / `get_domain` 的单元测试。
//
// 覆盖跨 keyspace（键空间）会话无所属 Domain 时返回 `None` 的语义，
// 与 Go 侧 GetDomain 在跨 keyspace session 上可能得到 nil 的行为对齐。

/// 验证未绑定 Domain 的上下文经 `get_domain` 仍得到 `None`。
#[test]
fn canonical_domain_context_preserves_absent_cross_keyspace_domain() {
    use crate::domain::Domain;
    use crate::domainctx::{DomainContext, get_domain};
    use std::sync::Arc;
    // 模拟跨 keyspace / 未挂载 Domain 的会话。
    struct Detached;
    impl DomainContext for Detached {
        fn domain(&self) -> Option<Arc<Domain>> {
            None
        }
    }
    assert!(get_domain(&Detached).is_none());
}
