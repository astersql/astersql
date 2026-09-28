// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// Server 状态变量单元测试：TLS 日期与非负 Uptime。

use super::server::*;
use super::stat::*;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 测试用空 ServerDriver。
struct Driver;
impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "tidb"
    }
}
/// 可设定启动时间戳的测试 Domain。
struct TestDomain {
    started: i64,
}
impl Domain for TestDomain {
    fn server_id(&self) -> u64 {
        7
    }
    fn start_timestamp(&self) -> i64 {
        self.started
    }
}

#[test]
/// 验证 statistics 含 SSL 日期、Uptime≥5，且 GetScope 始终返回默认作用域。
fn statistics_include_tls_dates_and_non_negative_uptime() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = Server::new_test(ServerConfig::default(), Arc::new(Driver));
    server.set_domain(Arc::new(TestDomain { started: now - 5 }));
    server.update_tls_config(Some(TlsConfig {
        not_before_unix: Some(10),
        not_after_unix: Some(20),
        ..Default::default()
    }));
    let stats = server.statistics();
    assert_eq!(
        stats[SSL_SERVER_NOT_BEFORE],
        StatusValue::String("10".into())
    );
    assert_eq!(
        stats[SSL_SERVER_NOT_AFTER],
        StatusValue::String("20".into())
    );
    assert!(matches!(stats[UPTIME], StatusValue::Integer(value) if value >= 5));
    assert_eq!(
        server.status_scope(SSL_SERVER_NOT_AFTER),
        StatusScope::GlobalAndSession
    );
    assert_eq!(server.status_scope(UPTIME), StatusScope::GlobalAndSession);
    assert_eq!(
        server.status_scope("unknown_status"),
        StatusScope::GlobalAndSession
    );
}
