// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 用户连接计数与限额逻辑的单元测试。
//
// 覆盖用户级限额覆盖全局、递减释放槽位、预检超限日志，以及零限额表示不限制。

use super::user_connections::*;
use std::sync::{Arc, Mutex};

/// 测试用运行时：固定全局/用户限额，并记录超限日志调用。
struct Runtime {
    global: u32,
    per_user: u32,
    exceeded: Mutex<Vec<(String, u32)>>,
}
impl UserConnectionRuntime for Runtime {
    fn global_limit(&self) -> u32 {
        self.global
    }
    fn user_limit(&self, _: &str, _: &str) -> Result<u32, ConnectionError> {
        Ok(self.per_user)
    }
    fn match_identity(
        &self,
        presented: &UserIdentity,
        _: &str,
    ) -> Result<UserIdentity, ConnectionError> {
        Ok(presented.clone())
    }
    fn log_limit_exceeded(&self, user: &str, limit: u32) {
        self.exceeded.lock().unwrap().push((user.into(), limit));
    }
}

/// 构造 alice 用户的测试连接及共享 Runtime。
fn connection(global: u32, per_user: u32) -> (ClientConn, Arc<Runtime>) {
    let runtime = Arc::new(Runtime {
        global,
        per_user,
        exceeded: Mutex::new(Vec::new()),
    });
    (
        ClientConn {
            user: UserIdentity {
                username: "alice".into(),
                hostname: "client".into(),
                auth_username: "alice".into(),
                auth_hostname: "%".into(),
            },
            runtime: runtime.clone(),
            registry: UserConnectionRegistry::default(),
        },
        runtime,
    )
}

/// 用户限额 2 应覆盖全局 10；第 3 次递增失败，递减后可再递增。
#[test]
fn per_user_limit_overrides_global_and_decrease_releases_slot() {
    let (connection, _) = connection(10, 2);
    increaseUserConnectionsCount(&connection).unwrap();
    increaseUserConnectionsCount(&connection).unwrap();
    assert!(matches!(
        increaseUserConnectionsCount(&connection),
        Err(ConnectionError::TooManyUserConnections(_))
    ));
    assert_eq!(
        getUserConnectionCount(&connection, &connection.user).unwrap(),
        2
    );
    decreaseUserConnectionCount(&connection).unwrap();
    increaseUserConnectionsCount(&connection).unwrap();
}

/// 预检超限时应按匹配后的登录串记录日志。
#[test]
fn precheck_logs_the_matched_login_when_limit_is_reached() {
    let (connection, runtime) = connection(1, 0);
    increaseUserConnectionsCount(&connection).unwrap();
    assert!(checkUserConnectionCount(&connection, "127.0.0.1").is_err());
    assert_eq!(
        runtime.exceeded.lock().unwrap().as_slice(),
        &[("alice@client".into(), 1)]
    );
}

/// 全局与用户限额均为 0 时表示不限制连接数。
#[test]
fn zero_limits_are_unlimited() {
    let (connection, _) = connection(0, 0);
    for _ in 0..100 {
        increaseUserConnectionsCount(&connection).unwrap();
    }
    assert_eq!(
        getUserConnectionCount(&connection, &connection.user).unwrap(),
        100
    );
}

/// 与 Go `auth.UserIdentity.String` 一致：尚未填入认证身份时回退到登录身份。
#[test]
fn identity_string_falls_back_to_login_identity() {
    let user = UserIdentity {
        username: "alice".into(),
        hostname: "client.example".into(),
        auth_username: String::new(),
        auth_hostname: String::new(),
    };

    assert_eq!(user.String(), "alice@client.example");
}

/// 覆盖 Go 测试中的全局限额路径，以及用户限额高于全局限额时仍优先采用用户限额。
#[test]
fn global_limit_and_higher_per_user_override_match_go() {
    let (global_only, _) = connection(3, 0);
    for _ in 0..3 {
        increaseUserConnectionsCount(&global_only).unwrap();
    }
    assert!(matches!(
        increaseUserConnectionsCount(&global_only),
        Err(ConnectionError::TooManyUserConnections(user)) if user == "alice@%"
    ));

    let (per_user_override, _) = connection(2, 3);
    for _ in 0..3 {
        increaseUserConnectionsCount(&per_user_override).unwrap();
    }
    assert!(increaseUserConnectionsCount(&per_user_override).is_err());
}

/// 不同客户端主机匹配到同一认证账号时，应像 Go 一样共享连接计数。
#[test]
fn authenticated_identity_shares_count_across_client_hosts() {
    let (first, runtime) = connection(0, 3);
    let second = ClientConn {
        user: UserIdentity {
            username: "alice".into(),
            hostname: "other-client".into(),
            auth_username: "alice".into(),
            auth_hostname: "%".into(),
        },
        runtime,
        registry: first.registry.clone(),
    };

    increaseUserConnectionsCount(&first).unwrap();
    increaseUserConnectionsCount(&first).unwrap();
    increaseUserConnectionsCount(&second).unwrap();
    assert!(increaseUserConnectionsCount(&second).is_err());
    assert_eq!(getUserConnectionCount(&first, &first.user).unwrap(), 3);

    decreaseUserConnectionCount(&second).unwrap();
    decreaseUserConnectionCount(&first).unwrap();
    decreaseUserConnectionCount(&first).unwrap();
    assert_eq!(getUserConnectionCount(&first, &first.user).unwrap(), 0);
}
