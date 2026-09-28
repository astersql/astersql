// Copyright 2015 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// Ported from `pkg/privilege/privileges/privileges_test.go`.
//
// Go's tests spin up a full mock TiKV store + session + SQL executor via
// `testkit`, then drive privilege checks by literally executing `CREATE
// USER`/`GRANT`/`SELECT ...` statements and inspecting session/executor
// errors. This crate only contains the privilege-cache/manager logic
// (`cache.rs`, `privileges.rs`); it has no session, planner, executor,
// domain, backup/restore, or `information_schema` implementation to drive
// through SQL. So each test below reconstructs the exact same `mysql.user` /
// `mysql.db` / `mysql.tables_priv` / ... row state the Go test's `CREATE
// USER`/`GRANT` statements would have produced (via the `RowDataSource`
// fixture helpers in `cache_test.rs`, which replay the real
// `decode*TableRow` production code), then calls the same
// `UserPrivileges`/`MySQLPrivilege` methods the SQL layer would have called
// (`RequestVerification`, `ConnectionVerification`, `ShowGrants`, `ensureActiveUser`,
// `RequestDynamicVerification`, ...) and asserts the same outcomes Go
// expects. No branch is deleted; setup is adapted from "run SQL" to "load
// the row state SQL would have produced".
//
// 本文件将 Go `privileges_test.go` 的行为测试迁移为对权限缓存/管理器
// 的直接调用。通过 `RowDataSource` 注入与 `CREATE USER`/`GRANT` 等价的
// `mysql.*` 行状态，再断言 `RequestVerification`、`ConnectionVerification`、
// `ShowGrants` 等与 Go 相同的结果。50 个 Go Test 场景均保留同名 Rust
// 用例；跨 parser/planner/executor 的语句只将本 crate 负责的真实权限门禁
// 下沉为直接断言，不以恒真或固定成功替代。

#![allow(non_snake_case, dead_code)]

use std::collections::HashMap;

use serde_json::json;
use serial_test::serial;
use sha1::{Digest, Sha1};

use crate::cache_test::{
    RowDataSource, db_row, default_role_row, dynamic_priv_row, global_priv_row, no_roles,
    role_edge_row, tables_priv_row, user_row,
};
use crate::*;

/// 作用域结束时执行清理（含 panic 路径），避免进程全局状态泄漏到下一用例。
///
/// Runs `cleanup` when dropped, including during a panicking test body, so a
/// failed assertion can never leak process-global state (SEM enabled,
/// `SkipWithGrant`, sandbox mode, ...) into the next `#[serial]` test.
struct DeferCleanup<F: FnMut()>(F);
impl<F: FnMut()> Drop for DeferCleanup<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// 用夹具行数据构造并刷新权限缓存句柄。
fn load(source: RowDataSource) -> Handle {
    let handle = Handle::New();
    handle.UpdateAll(&source).expect("fixture rows must decode");
    handle
}

/// 构造已 AuthSuccess 的 `UserPrivileges`（跳过真实密码校验）。
fn auth_as(handle: &Handle, user: &str, host: &str) -> UserPrivileges {
    let mut p = NewUserPrivileges(handle.clone());
    p.AuthSuccess(user, host);
    p
}

/// 生成 mysql_native_password 的 stage-2 存储哈希。
///
/// mysql_native_password stage-2 hash (`*` + upper-hex `SHA1(SHA1(password))`),
/// matching what `CREATE USER ... IDENTIFIED BY 'password'` stores in
/// `mysql.user.authentication_string`.
fn native_password_hash(password: &str) -> String {
    let stage1 = Sha1::digest(password.as_bytes());
    let stage2 = Sha1::digest(stage1);
    format!("*{}", hex::encode_upper(stage2))
}

#[test]
#[serial(privileges)]
/// 库级权限：GRANT/REVOKE 后 RequestVerification 结果。
fn TestCheckDBPrivilege() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("localhost", "testcheck", &[]),
        user_row("localhost", "testcheck_tmp", &[]),
    ];
    let handle = load(source.clone());
    let pc = auth_as(&handle, "testcheck", "localhost");
    let active_roles = no_roles();
    assert!(!pc.RequestVerification(&active_roles, "test", "", "", SelectPriv));

    source.db = vec![db_row("%", "test", "testcheck", &["select"])];
    // GRANT SELECT ON *.* TO 'testcheck'@'localhost' is a *global* grant.
    source.user[0] = user_row("localhost", "testcheck", &["select"]);
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "testcheck", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "", "", SelectPriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "", "", UpdatePriv));

    source.db = vec![db_row("%", "test", "testcheck", &["update"])];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "testcheck", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "", "", UpdatePriv));

    // GRANT 'testcheck'@'localhost' TO 'testcheck_tmp'@'localhost';
    source.role_edges = vec![role_edge_row(
        "localhost",
        "testcheck",
        "localhost",
        "testcheck_tmp",
    )];
    handle.merge(reload(&source));
    let active_roles = vec![RoleIdentity::new("testcheck", "localhost")];
    let pc2 = auth_as(&handle, "testcheck_tmp", "localhost");
    assert!(pc2.RequestVerification(&active_roles, "test", "", "", SelectPriv));
    assert!(pc2.RequestVerification(&active_roles, "test", "", "", UpdatePriv));
}

/// 从夹具重载并返回权限缓存快照。
fn reload(source: &RowDataSource) -> MySQLPrivilege {
    let handle = Handle::New();
    handle.UpdateAll(source).expect("fixture rows must decode");
    handle.Get()
}

#[test]
#[serial(privileges)]
/// 表级权限校验。
fn TestCheckTablePrivilege() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("localhost", "test1", &[]),
        user_row("localhost", "test1_tmp", &[]),
    ];
    let handle = load(source.clone());
    let active_roles = no_roles();
    let pc = auth_as(&handle, "test1", "localhost");
    assert!(!pc.RequestVerification(&active_roles, "test", "test", "", SelectPriv));

    source.user[0] = user_row("localhost", "test1", &["select"]);
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "test1", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "test", "", SelectPriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "test", "", UpdatePriv));

    source.db = vec![db_row("%", "test", "test1", &["update"])];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "test1", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "test", "", UpdatePriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "test", "", IndexPriv));

    source.role_edges = vec![role_edge_row(
        "localhost",
        "test1",
        "localhost",
        "test1_tmp",
    )];
    handle.merge(reload(&source));
    let active_roles = vec![RoleIdentity::new("test1", "localhost")];
    let pc2 = auth_as(&handle, "test1_tmp", "localhost");
    assert!(pc2.RequestVerification(&active_roles, "test", "test", "", SelectPriv));
    assert!(pc2.RequestVerification(&active_roles, "test", "test", "", UpdatePriv));
    assert!(!pc2.RequestVerification(&active_roles, "test", "test", "", IndexPriv));

    source.tables_priv = vec![tables_priv_row(
        "localhost",
        "test",
        "test1",
        "test",
        "Index",
        "",
    )];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "test1", "localhost");
    let pc2 = auth_as(&handle, "test1_tmp", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "test", "", IndexPriv));
    assert!(pc2.RequestVerification(&active_roles, "test", "test", "", IndexPriv));
}

#[test]
#[serial(privileges)]
/// 视图相关权限（CREATE VIEW / SHOW VIEW）。
fn TestCheckViewPrivilege() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "vuser", &[])];
    let handle = load(source.clone());
    let active_roles = no_roles();
    let pc = auth_as(&handle, "vuser", "localhost");
    assert!(!pc.RequestVerification(&active_roles, "test", "v", "", SelectPriv));

    source.tables_priv = vec![tables_priv_row(
        "localhost",
        "test",
        "vuser",
        "v",
        "Select",
        "",
    )];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "vuser", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "v", "", SelectPriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "v", "", ShowViewPriv));

    source.tables_priv = vec![tables_priv_row(
        "localhost",
        "test",
        "vuser",
        "v",
        "Select,Show View",
        "",
    )];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "vuser", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "v", "", SelectPriv));
    assert!(pc.RequestVerification(&active_roles, "test", "v", "", ShowViewPriv));
}

#[test]
#[serial(privileges)]
/// 通过角色继承获得的权限。
fn TestCheckPrivilegeWithRoles() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("localhost", "test_role", &[]),
        user_row("%", "r_1", &[]),
        user_row("%", "r_2", &[]),
        user_row("%", "r_3", &[]),
    ];
    source.role_edges = vec![
        role_edge_row("%", "r_1", "localhost", "test_role"),
        role_edge_row("%", "r_2", "localhost", "test_role"),
        role_edge_row("%", "r_3", "localhost", "test_role"),
    ];
    let handle = load(source.clone());
    let cache = handle.Get();
    // SET ROLE r_1, r_2: only requested roles which are actually granted
    // become active (FindAllRole further expands transitively below).
    let active_roles = vec![RoleIdentity::new("r_1", "%"), RoleIdentity::new("r_2", "%")];

    source.db = vec![db_row("%", "test", "r_1", &["select"])];
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "test_role", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "", "", SelectPriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "", "", UpdatePriv));

    source.db.push(db_row("%", "test", "r_2", &["update"]));
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "test_role", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "", "", UpdatePriv));

    // SET ROLE ALL activates every granted role.
    let all_roles = cache.getAllRoles("test_role", "localhost");
    assert_eq!(all_roles.len(), 3);
}

#[test]
#[serial(privileges)]
/// 鉴权后的错误身份必须使用 mysql.user 中匹配到的 host，而不是登录来源 host。
fn TestErrorMessage() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "wildcard", &["select"]),
        user_row("192.168.1.1", "specifichost", &["select"]),
    ];
    let handle = load(source);

    for (user, expected_host) in [("wildcard", "%"), ("specifichost", "192.168.1.1")] {
        let mut privileges = NewUserPrivileges(handle.clone());
        let info = privileges
            .ConnectionVerification(
                &UserIdentity {
                    Username: user.into(),
                    Hostname: "192.168.1.1".into(),
                },
                user,
                "192.168.1.1",
                &[],
                &[],
                &session_with_tls(None),
            )
            .expect("fixture user must authenticate");
        assert_eq!(info.authenticated_user, user);
        assert_eq!(info.authenticated_host, expected_host);
        privileges.AuthSuccess(&info.authenticated_user, &info.authenticated_host);
        assert_eq!(privileges.user, user);
        assert_eq!(privileges.host, expected_host);
    }
}

#[test]
#[serial(privileges)]
/// DROP TABLE 所需权限组合。
fn TestDropTablePrivileges() {
    // Adapted: exercises the same SelectPriv/DropPriv gate `DROP TABLE`
    // would go through, without a real executor/DDL path.
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "drop", &[])];
    source.tables_priv = vec![tables_priv_row(
        "localhost",
        "test",
        "drop",
        "todrop",
        "Select",
        "",
    )];
    let handle = load(source.clone());
    let active_roles = no_roles();
    let pc = auth_as(&handle, "drop", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "todrop", "", SelectPriv));
    assert!(!pc.RequestVerification(&active_roles, "test", "todrop", "", DropPriv));

    source.tables_priv[0] =
        tables_priv_row("localhost", "test", "drop", "todrop", "Select,Drop", "");
    handle.merge(reload(&source));
    let pc = auth_as(&handle, "drop", "localhost");
    assert!(pc.RequestVerification(&active_roles, "test", "todrop", "", DropPriv));
}

/// Builds a `TlsConnectionState` matching Go's `connectionState(issuer,
/// subject, cipher, opts...)` test helper.
/// 构造带证书信息的 TLS 连接状态，供证书鉴权测试使用。
fn connection_state(
    issuer: &str,
    subject: &str,
    cipher: &str,
    uris: &[&str],
) -> TlsConnectionState {
    TlsConnectionState {
        cipher_suite: cipher.into(),
        verified_chains: true,
        peer_certificate: Some(Certificate {
            issuer: issuer.into(),
            subject: subject.into(),
            dns_names: Vec::new(),
            ip_addresses: Vec::new(),
            uris: uris.iter().map(|s| s.to_string()).collect(),
        }),
    }
}

/// 构造仅含 TLS 状态的会话变量。
fn session_with_tls(tls: Option<TlsConnectionState>) -> SessionVars {
    SessionVars {
        tls_state: tls,
        default_password_lifetime: 0,
    }
}

#[test]
#[serial(privileges)]
/// 基于客户端证书（REQUIRE SSL/X509/SUBJECT/SAN）的连接鉴权。
fn TestCheckCertBasedAuth() {
    let issuer = "/C=US/ST=California/L=San Francisco/O=PingCAP/OU=TiDB/CN=TiDB admin";
    let subject = "/C=ZH/ST=Beijing/L=Haidian/O=PingCAP.Inc/OU=TiDB/CN=tester1";
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("localhost", "r1", &[]),
        user_row("localhost", "r2", &[]),
        user_row("localhost", "r3", &[]),
        user_row("localhost", "r4", &[]),
        user_row("localhost", "r5", &[]),
        user_row("localhost", "r6", &[]),
        user_row("localhost", "r7_issuer_only", &[]),
        user_row("localhost", "r8_subject_only", &[]),
        user_row("localhost", "r9_subject_disorder", &[]),
        user_row("localhost", "r10_issuer_disorder", &[]),
        user_row("localhost", "r11_cipher_only", &[]),
        user_row("localhost", "r12_old_tidb_user", &[]),
        user_row("localhost", "r13_broken_user", &[]),
        user_row("localhost", "r14_san_only_pass", &[]),
        user_row("localhost", "r15_san_only_fail", &[]),
    ];
    source.global_priv = vec![
        global_priv_row("localhost", "r2", json!({"ssl_type": -1})),
        global_priv_row("localhost", "r3", json!({"ssl_type": 1})),
        global_priv_row("localhost", "r4", json!({"ssl_type": 2})),
        global_priv_row(
            "localhost",
            "r5",
            json!({"ssl_type": 3, "x509_issuer": issuer, "x509_subject": subject, "ssl_cipher": "TLS_AES_128_GCM_SHA256"}),
        ),
        global_priv_row(
            "localhost",
            "r6",
            json!({"ssl_type": 3, "x509_issuer": issuer, "x509_subject": subject}),
        ),
        global_priv_row(
            "localhost",
            "r7_issuer_only",
            json!({"ssl_type": 3, "x509_issuer": issuer}),
        ),
        global_priv_row(
            "localhost",
            "r8_subject_only",
            json!({"ssl_type": 3, "x509_subject": subject}),
        ),
        global_priv_row(
            "localhost",
            "r9_subject_disorder",
            json!({"ssl_type": 3, "x509_subject": "/ST=Beijing/C=ZH/L=Haidian/O=PingCAP.Inc/OU=TiDB/CN=tester1"}),
        ),
        global_priv_row(
            "localhost",
            "r10_issuer_disorder",
            json!({"ssl_type": 3, "x509_issuer": "/ST=California/C=US/L=San Francisco/O=PingCAP/OU=TiDB/CN=TiDB admin"}),
        ),
        global_priv_row(
            "localhost",
            "r11_cipher_only",
            json!({"ssl_type": 3, "ssl_cipher": "TLS_AES_256_GCM_SHA384"}),
        ),
        // r12_old_tidb_user: no global_priv row at all (simulates the row
        // having been deleted), so checkSSL is skipped entirely.
        global_priv_row("localhost", "r13_broken_user", json!("abc")),
        global_priv_row(
            "localhost",
            "r14_san_only_pass",
            json!({"ssl_type": 3, "san": "URI:spiffe://mesh.pingcap.com/ns/timesh/sa/me1"}),
        ),
        global_priv_row(
            "localhost",
            "r15_san_only_fail",
            json!({"ssl_type": 3, "san": "URI:spiffe://mesh.pingcap.com/ns/timesh/sa/me2"}),
        ),
    ];
    let handle = load(source);
    let no_auth = vec![];
    let try_auth = |user: &str, session: &SessionVars| {
        let mut p = NewUserPrivileges(handle.clone());
        p.ConnectionVerification(
            &UserIdentity {
                Username: user.into(),
                Hostname: "localhost".into(),
            },
            user,
            "localhost",
            &no_auth,
            &no_auth,
            session,
        )
    };

    // test without ssl or ca
    let plain = session_with_tls(None);
    assert!(try_auth("r1", &plain).is_ok());
    assert!(try_auth("r2", &plain).is_ok());
    assert!(try_auth("r3", &plain).is_err());
    assert!(try_auth("r4", &plain).is_err());
    assert!(try_auth("r5", &plain).is_err());

    // test use ssl without ca: `verified_chains` empty
    let no_ca = session_with_tls(Some(TlsConnectionState {
        cipher_suite: String::new(),
        verified_chains: false,
        peer_certificate: None,
    }));
    assert!(try_auth("r1", &no_ca).is_ok());
    assert!(try_auth("r2", &no_ca).is_ok());
    assert!(try_auth("r3", &no_ca).is_ok()); // SSL only requires *some* TLS state
    assert!(try_auth("r4", &no_ca).is_err()); // X509 requires verified_chains
    assert!(try_auth("r5", &no_ca).is_err());

    // test use ssl with signed but empty cert (verified, but no cert data)
    let empty_verified = session_with_tls(Some(TlsConnectionState {
        cipher_suite: String::new(),
        verified_chains: true,
        peer_certificate: None,
    }));
    assert!(try_auth("r4", &empty_verified).is_ok());
    assert!(try_auth("r5", &empty_verified).is_err()); // SPECIFIED needs a real cert

    // full match
    let full = session_with_tls(Some(connection_state(
        issuer,
        subject,
        "TLS_AES_128_GCM_SHA256",
        &["spiffe://mesh.pingcap.com/ns/timesh/sa/me1"],
    )));
    assert!(try_auth("r1", &full).is_ok());
    assert!(try_auth("r2", &full).is_ok());
    assert!(try_auth("r3", &full).is_ok());
    assert!(try_auth("r4", &full).is_ok());
    assert!(try_auth("r5", &full).is_ok());
    assert!(try_auth("r14_san_only_pass", &full).is_ok());

    // require but give nothing
    assert!(try_auth("r5", &plain).is_err());

    // mismatched cipher
    let wrong_cipher = session_with_tls(Some(connection_state(
        issuer,
        subject,
        "TLS_AES_256_GCM_SHA384",
        &[],
    )));
    assert!(try_auth("r5", &wrong_cipher).is_err());
    assert!(try_auth("r6", &wrong_cipher).is_ok()); // r6 does not require cipher
    assert!(try_auth("r11_cipher_only", &wrong_cipher).is_ok());

    // only issuer / only subject specified: other fields aren't checked
    let other_subject = session_with_tls(Some(connection_state(
        issuer,
        "/C=AZ/ST=Beijing/L=Shijingshang/O=CAPPing.Inc/OU=TiDB/CN=tester2",
        "TLS_AES_128_GCM_SHA256",
        &[],
    )));
    assert!(try_auth("r7_issuer_only", &other_subject).is_ok());
    let other_issuer = session_with_tls(Some(connection_state(
        "/C=AU/ST=California/L=San Francisco/O=PingCAP/OU=TiDB/CN=TiDB admin2",
        subject,
        "TLS_AES_128_GCM_SHA256",
        &[],
    )));
    assert!(try_auth("r8_subject_only", &other_issuer).is_ok());

    // disordered issuer/subject strings don't match exactly.
    let disordered_subject = session_with_tls(Some(connection_state(
        "",
        subject,
        "TLS_AES_128_GCM_SHA256",
        &[],
    )));
    assert!(try_auth("r9_subject_disorder", &disordered_subject).is_err());
    let disordered_issuer = session_with_tls(Some(connection_state(
        issuer,
        "",
        "TLS_AES_128_GCM_SHA256",
        &[],
    )));
    assert!(try_auth("r10_issuer_disorder", &disordered_issuer).is_err());

    // mismatched SAN
    assert!(try_auth("r15_san_only_fail", &full).is_err());

    // old data (no global_priv row) passes trivially; broken JSON fails.
    assert!(try_auth("r12_old_tidb_user", &plain).is_ok());
    assert!(try_auth("r13_broken_user", &full).is_err());

    // Go treats values within one SAN type as alternatives: one matching
    // required URI is sufficient. Matching is exact, and unknown SAN types
    // are ignored for forward compatibility.
    let cert = Certificate {
        uris: vec!["spiffe://mesh.pingcap.com/ns/timesh/sa/me1".into()],
        ..Default::default()
    };
    let privilege = globalPrivRecord::default();
    assert!(checkCertSAN(
        &privilege,
        &cert,
        &HashMap::from([(
            "URI".into(),
            vec!["spiffe://missing".into(), cert.uris[0].clone()],
        )]),
    ));
    assert!(!checkCertSAN(
        &privilege,
        &cert,
        &HashMap::from([("URI".into(), vec![cert.uris[0].to_uppercase()])]),
    ));
    assert!(checkCertSAN(
        &privilege,
        &cert,
        &HashMap::from([("FUTURE".into(), vec!["value".into()])]),
    ));
}

#[test]
#[serial(privileges)]
/// 密码插件鉴权（native password 等）成功/失败路径。
fn TestCheckAuthenticate() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("localhost", "u1", &[]),
        {
            let mut r = user_row("localhost", "u2", &[]);
            r.insert(
                "authentication_string".into(),
                json!(native_password_hash("abc")),
            );
            r
        },
        user_row("localhost", "u3@example.com", &[]),
        user_row("localhost", "u4", &[]),
    ];
    let handle = load(source.clone());

    let salt: [u8; 20] = [
        85, 92, 45, 22, 58, 79, 107, 6, 122, 125, 58, 80, 12, 90, 103, 32, 90, 10, 74, 82,
    ];
    let authentication: [u8; 20] = [
        24, 180, 183, 225, 166, 6, 81, 102, 70, 248, 199, 143, 91, 204, 169, 9, 161, 171, 203, 33,
    ];
    let try_auth = |user: &str, auth: &[u8], salt: &[u8]| {
        let mut p = NewUserPrivileges(handle.clone());
        p.ConnectionVerification(
            &UserIdentity {
                Username: user.into(),
                Hostname: "localhost".into(),
            },
            user,
            "localhost",
            auth,
            salt,
            &session_with_tls(None),
        )
    };

    assert!(try_auth("u1", &[], &[]).is_ok());
    // Wrong (empty) credentials against u2's real password hash must fail.
    assert!(try_auth("u2", &[], &[]).is_err());
    assert!(try_auth("u2", &authentication, &salt).is_ok());
    assert!(try_auth("u3@example.com", &[], &[]).is_ok());
    assert!(try_auth("u4", &[], &[]).is_ok());
    assert!(
        !checkPasswordForPlugin("unknown_auth_plugin", "secret", &[], b"secret")
            .expect("unknown plugins are a non-match, not an error")
    );

    // Simulate DROP USER by reloading with an empty user table.
    let handle = load(RowDataSource::default());
    let try_auth = |user: &str| {
        let mut p = NewUserPrivileges(handle.clone());
        p.ConnectionVerification(
            &UserIdentity {
                Username: user.into(),
                Hostname: "localhost".into(),
            },
            user,
            "localhost",
            &[],
            &[],
            &session_with_tls(None),
        )
    };
    assert!(try_auth("u1").is_err());
    assert!(try_auth("u2").is_err());
    assert!(try_auth("u3@example.com").is_err());
    assert!(try_auth("u4").is_err());

    // Roles are locked by default (CREATE ROLE sets account_locked = 'Y'),
    // so authenticating as one directly must always fail.
    let mut source = RowDataSource::default();
    for (user, host) in [
        ("r1", "localhost"),
        ("r2", "localhost"),
        ("r3@example.com", "localhost"),
    ] {
        let mut r = user_row(host, user, &[]);
        r.insert("account_locked".into(), json!("Y"));
        source.user.push(r);
    }
    let handle = load(source);
    let try_auth = |user: &str| {
        let mut p = NewUserPrivileges(handle.clone());
        p.ConnectionVerification(
            &UserIdentity {
                Username: user.into(),
                Hostname: "localhost".into(),
            },
            user,
            "localhost",
            &[],
            &[],
            &session_with_tls(None),
        )
    };
    assert!(try_auth("r1").is_err());
    assert!(try_auth("r2").is_err());
    assert!(try_auth("r3@example.com").is_err());
}

#[test]
#[serial(privileges)]
/// USE db 可见性（DBIsVisible）。
fn TestUseDB() {
    // Adapted: `USE db` is gated by `DBIsVisible`.
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "usesuper", &["select"]),
        user_row("localhost", "dev", &[]),
        user_row("%", "app_developer", &[]),
    ];
    source.db = vec![db_row("%", "app_db", "app_developer", &["select"])];
    source.role_edges = vec![role_edge_row("%", "app_developer", "localhost", "dev")];
    source.default_roles = vec![default_role_row("localhost", "dev", "%", "app_developer")];
    let handle = load(source);
    let active_roles = vec![RoleIdentity::new("app_developer", "%")];
    let dev = auth_as(&handle, "dev", "localhost");
    assert!(dev.DBIsVisible(&active_roles, "app_db"));
    assert!(!dev.DBIsVisible(&active_roles, "mysql"));

    let no_roles = no_roles();
    let super_user = auth_as(&handle, "usesuper", "%");
    // A globally-granted SELECT makes every schema visible.
    assert!(super_user.DBIsVisible(&no_roles, "mysql"));
}

#[test]
#[serial(privileges)]
/// CONFIG 全局权限。
fn TestConfigPrivilege() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "tcd1", &["config"]),
        user_row("%", "tcd2", &[]),
    ];
    let handle = load(source);
    let active_roles = no_roles();
    let tcd1 = auth_as(&handle, "tcd1", "%");
    assert!(tcd1.RequestVerification(&active_roles, "", "", "", ConfigPriv));

    let tcd2 = auth_as(&handle, "tcd2", "%");
    assert!(!tcd2.RequestVerification(&active_roles, "", "", "", ConfigPriv));
}

#[test]
#[serial(privileges)]
/// 系统 schema（information_schema 等）写保护。
fn TestSystemSchema() {
    // Adapted: `RequestVerification`'s hard-coded system-schema rules (no
    // privilege check for reads on information_schema; SELECT+PROCESS
    // required for metrics_schema; writes always denied on both).
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "u1", &["select"])];
    let handle = load(source);
    let active_roles = no_roles();
    let u1 = auth_as(&handle, "u1", "localhost");

    assert!(u1.RequestVerification(
        &active_roles,
        "information_schema",
        "tables",
        "",
        SelectPriv
    ));
    assert!(!u1.RequestVerification(&active_roles, "information_schema", "t", "", CreatePriv));
    assert!(!u1.RequestVerification(&active_roles, "information_schema", "tables", "", DropPriv));

    assert!(u1.RequestVerification(
        &active_roles,
        "metrics_schema",
        "tidb_query_duration",
        "",
        SelectPriv
    ));
    assert!(!u1.RequestVerification(
        &active_roles,
        "metrics_schema",
        "tidb_query_duration",
        "",
        DropPriv
    ));
}

#[test]
#[serial(privileges)]
/// performance_schema 下 tidb_* 表写保护。
fn TestPerformanceSchema() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "u1", &[])];
    let handle = load(source.clone());
    let active_roles = no_roles();
    let u1 = auth_as(&handle, "u1", "localhost");
    assert!(!u1.RequestVerification(
        &active_roles,
        "performance_schema",
        "events_statements_summary_by_digest",
        "",
        SelectPriv
    ));

    source.user[0] = user_row("localhost", "u1", &["select"]);
    let handle = load(source);
    let u1 = auth_as(&handle, "u1", "localhost");
    assert!(u1.RequestVerification(
        &active_roles,
        "performance_schema",
        "events_statements_summary_by_digest",
        "",
        SelectPriv
    ));
    assert!(!u1.RequestVerification(
        &active_roles,
        "performance_schema",
        "tidb_x",
        "",
        UpdatePriv
    ));
}

#[test]
#[serial(privileges)]
/// metrics_schema 对 PROCESS 与 schema 级 SELECT 使用相同的可见/读取语义。
fn TestMetricsSchema() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "nobody", &[]),
        user_row("%", "msprocess", &["process"]),
        user_row("%", "msselect", &[]),
    ];
    source.db = vec![db_row("%", "metrics_schema", "msselect", &["select"])];
    let handle = load(source);

    let nobody = auth_as(&handle, "nobody", "%");
    assert!(nobody.DBIsVisible(&[], "information_schema"));
    assert!(!nobody.DBIsVisible(&[], "metrics_schema"));
    assert!(!nobody.RequestVerification(&[], "metrics_schema", "up", "", SelectPriv));

    let process = auth_as(&handle, "msprocess", "%");
    assert!(process.DBIsVisible(&[], "metrics_schema"));
    assert!(process.RequestVerification(&[], "metrics_schema", "up", "", SelectPriv));

    let select = auth_as(&handle, "msselect", "%");
    assert!(select.DBIsVisible(&[], "metrics_schema"));
    assert!(select.RequestVerification(&[], "metrics_schema", "up", "", SelectPriv));
}

#[test]
#[serial(privileges)]
/// 主机名匹配与身份解析。
fn TestAuthHost() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row(
            "%",
            "test_auth_host",
            &[
                "select", "insert", "update", "delete", "create", "drop", "grant",
            ],
        ),
        user_row("192.168.%", "test_auth_host", &["select"]),
    ];
    let handle = load(source);
    // A connection from 192.168.0.10 must resolve to the more specific
    // '192.168.%' host record, not '%', per Go's `compareBaseRecord`
    // host-specificity ordering.
    let mut p = NewUserPrivileges(handle);
    let info = p
        .ConnectionVerification(
            &UserIdentity {
                Username: "test_auth_host".into(),
                Hostname: "192.168.0.10".into(),
            },
            "test_auth_host",
            "192.168.0.10",
            &[],
            &[],
            &session_with_tls(None),
        )
        .expect("connection should be allowed");
    assert_eq!(info.authenticated_host, "192.168.%");
    p.AuthSuccess(&info.authenticated_user, &info.authenticated_host);
    let active_roles = no_roles();
    // The '192.168.%' record only has SELECT, so CREATE USER (via
    // CreateUserPriv) must be denied even though '%' has broader rights.
    assert!(!p.RequestVerification(&active_roles, "", "", "", CreateUserPriv));
}

#[test]
#[serial(privileges)]
/// 默认角色加载与激活。
fn TestDefaultRoles() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "testdefault", &[])];
    source.role_edges = vec![
        role_edge_row("localhost", "testdefault_r1", "localhost", "testdefault"),
        role_edge_row("localhost", "testdefault_r2", "localhost", "testdefault"),
    ];
    let mut source_no_default = source.clone();
    let handle = load(source_no_default.clone());
    let pc = auth_as(&handle, "root", "localhost");
    assert_eq!(pc.GetDefaultRoles("testdefault", "localhost").len(), 0);

    source_no_default.default_roles = vec![
        default_role_row("localhost", "testdefault", "localhost", "testdefault_r1"),
        default_role_row("localhost", "testdefault", "localhost", "testdefault_r2"),
    ];
    let handle = load(source_no_default);
    let pc = auth_as(&handle, "root", "localhost");
    assert_eq!(pc.GetDefaultRoles("testdefault", "localhost").len(), 2);

    let handle = load(source);
    let pc = auth_as(&handle, "root", "localhost");
    assert_eq!(pc.GetDefaultRoles("testdefault", "localhost").len(), 0);
}

#[test]
#[serial(privileges)]
/// USER_PRIVILEGES 表数据一致性。
fn TestUserTableConsistency() {
    // Adapted from Go's `len(mysql.Priv2UserCol) == len(mysql.AllGlobalPrivs)+1`
    // (GrantPriv is the "+1"): checks that granting every privilege in
    // `ALL_GLOBAL_PRIVS` via its `mysql.user` column name round-trips through
    // `decodeUserTableRow` back to the exact same bitmask, i.e. the column
    // name table (`privilege_column_name`) has no gaps or typos relative to
    // `ALL_GLOBAL_PRIVS`.
    // `user_row` appends `_priv` itself, so strip the suffix off the real
    // `mysql.user` column name (`privilege_column_name`, e.g. `Show_db_priv`)
    // rather than deriving it from `privilege_name`'s SHOW GRANTS display
    // text (e.g. "SHOW DATABASES"), which does not match MySQL's column
    // names for several privileges (ShowDBPriv, CreateTMPTablePriv,
    // ReplClientPriv, ReplSlavePriv, GrantPriv).
    let lower: Vec<String> = ALL_GLOBAL_PRIVS
        .iter()
        .map(|p| {
            privilege_column_name(*p)
                .to_ascii_lowercase()
                .trim_end_matches("_priv")
                .to_string()
        })
        .collect();
    let names: Vec<&str> = lower.iter().map(|s: &String| s.as_str()).collect();
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "superadmin", &names)];
    let handle = load(source);
    let cache = handle.Get();
    let record = cache.matchUser("superadmin", "%").unwrap();
    let expected = ALL_GLOBAL_PRIVS.iter().fold(0, |acc, p| acc | *p);
    assert_eq!(record.Privileges, expected);
}

#[test]
#[serial(privileges)]
/// 动态权限授予与 RequestDynamicVerification。
fn TestDynamicPrivs() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "notsuper", &[]),
        user_row("%", "anyrolename", &[]),
    ];
    source.role_edges = vec![role_edge_row("%", "anyrolename", "%", "notsuper")];
    let handle = load(source.clone());
    let cache = handle.Get();
    let active_roles = no_roles();

    // No SYSTEM_VARIABLES_ADMIN and no SUPER: denied.
    assert!(!cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));

    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "notsuper",
        "SYSTEM_VARIABLES_ADMIN",
        false,
    )];
    let cache = reload(&source);
    assert!(cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));
    // Explicitly granted, case-insensitive privilege name lookup.
    assert!(cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "system_variables_admin",
        false
    ));

    // Revoke it: falls back to checking SUPER, which notsuper lacks.
    source.dynamic_priv.clear();
    let cache = reload(&source);
    assert!(!cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));

    // Granting SUPER is accepted as a substitute for any dynamic privilege.
    source.user[0] = user_row("%", "notsuper", &["super"]);
    let cache = reload(&source);
    assert!(cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));

    // Revoke SUPER; grant SYSTEM_VARIABLES_ADMIN to a role instead, and
    // confirm the dynamic privilege is inherited from active roles.
    source.user[0] = user_row("%", "notsuper", &[]);
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "anyrolename",
        "SYSTEM_VARIABLES_ADMIN",
        false,
    )];
    let cache = reload(&source);
    assert!(!cache.RequestDynamicVerification(
        &[],
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));
    let active_roles = vec![RoleIdentity::new("anyrolename", "%")];
    assert!(cache.RequestDynamicVerification(
        &active_roles,
        "notsuper",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        false
    ));
}

#[test]
#[serial(privileges)]
/// 动态权限的 GRANT OPTION。
fn TestDynamicGrantOption() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "varuser1", &[]),
        user_row("%", "varuser2", &[]),
    ];
    source.dynamic_priv = vec![
        dynamic_priv_row("%", "varuser1", "SYSTEM_VARIABLES_ADMIN", false),
        dynamic_priv_row("%", "varuser2", "SYSTEM_VARIABLES_ADMIN", true),
    ];
    let handle = load(source);
    let cache = handle.Get();
    let active_roles = no_roles();

    // varuser1 has the privilege but not WITH GRANT OPTION.
    assert!(!cache.RequestDynamicVerification(
        &active_roles,
        "varuser1",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        true
    ));
    // varuser2 has WITH GRANT OPTION and can re-grant it.
    assert!(cache.RequestDynamicVerification(
        &active_roles,
        "varuser2",
        "%",
        "SYSTEM_VARIABLES_ADMIN",
        true
    ));
}

/// Go's `variable` package registers `hostname`/`tidb_enable_enhanced_security`
/// in an `init()` that runs for the full server binary. This crate's isolated
/// test binary never links that init, so `sem::Enable()` (called by
/// `SwitchToSEMForTest`) would panic on an unregistered sysvar. Mirrors
/// `pkg/util/sem/migration_aster_unit_test.rs`'s
/// `register_go_initialized_sem_sysvars` helper.
/// 注册 Go 初始化阶段会注入的 SEM 相关系统变量桩。
fn register_go_initialized_sem_sysvars() {
    let config: serde_json::Value =
        serde_json::from_str(sem::compatibleSEMV2Config).expect("SEM v2 fixture JSON");
    let restricted_names = config["restricted_variables"]
        .as_array()
        .expect("restricted_variables array")
        .iter()
        .filter_map(|entry| entry["name"].as_str());
    for name in std::iter::once(vardef::Hostname)
        .chain(std::iter::once(vardef::TiDBEnableEnhancedSecurity))
        .chain(restricted_names)
    {
        if variable::GetSysVar(name).is_some() {
            continue;
        }
        variable::RegisterSysVar(variable::SysVar {
            Scope: vardef::ScopeNone,
            Name: name.into(),
            Value: if name == vardef::Hostname {
                vardef::DefHostname.into()
            } else {
                vardef::Off.into()
            },
            Type: vardef::TypeStr,
            ..variable::SysVar::default()
        });
    }
}

#[test]
#[serial(privileges)]
/// SEM 下受限表/库只读规则。
fn TestSecurityEnhancedModeRestrictedTables() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "cloudadmin", &["select", "create"]),
        user_row(
            "%",
            "uroot",
            &[
                "select",
                "create",
                "insert",
                "update",
                "delete",
                "drop",
                "grant",
                "references",
            ],
        ),
    ];
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "cloudadmin",
        "RESTRICTED_TABLES_ADMIN",
        false,
    )];
    let handle = load(source);
    let active_roles = no_roles();

    register_go_initialized_sem_sysvars();
    let cleanup = sem::SwitchToSEMForTest(sem::V1);
    let _guard = DeferCleanup(|| cleanup());

    let uroot = auth_as(&handle, "uroot", "%");
    // metrics_schema is entirely invisible without RESTRICTED_TABLES_ADMIN.
    assert!(!uroot.DBIsVisible(&active_roles, "metrics_schema"));
    assert!(!uroot.RequestVerification(&active_roles, "metrics_schema", "uptime", "", SelectPriv));
    // mysql.* writes are denied even though uroot has global CREATE.
    assert!(!uroot.RequestVerification(&active_roles, "mysql", "abcd", "", CreatePriv));
    // Go's SEM hard rule only blocks the eight mutating DDL/DML privileges;
    // REFERENCES remains governed by the ordinary privilege cache.
    assert!(uroot.RequestVerification(&active_roles, "mysql", "abcd", "", ReferencesPriv));

    let cloudadmin = auth_as(&handle, "cloudadmin", "%");
    assert!(cloudadmin.DBIsVisible(&active_roles, "metrics_schema"));
    assert!(cloudadmin.RequestVerification(
        &active_roles,
        "metrics_schema",
        "uptime",
        "",
        SelectPriv
    ));
    assert!(cloudadmin.RequestVerification(&active_roles, "mysql", "abcd", "", CreatePriv));
}

#[test]
#[serial(privileges)]
/// 动态权限注册/去重/长度校验。
fn TestDynamicPrivsRegistration() {
    let count = GetDynamicPrivileges().len();
    let handle = load(RowDataSource::default());
    let pm = auth_as(&handle, "", "");

    assert!(!pm.IsDynamicPrivilege("ACDC_ADMIN"));
    assert!(
        !GetDynamicPrivileges()
            .iter()
            .any(|p| p.eq_ignore_ascii_case("ACDC_ADMIN"))
    );
    assert!(RegisterDynamicPrivilege("ACDC_ADMIN").is_ok());
    assert!(pm.IsDynamicPrivilege("ACDC_ADMIN"));
    assert_eq!(GetDynamicPrivileges().len(), count + 1);

    assert!(!pm.IsDynamicPrivilege("iAmdynamIC"));
    assert!(RegisterDynamicPrivilege("IAMdynamic").is_ok());
    assert!(pm.IsDynamicPrivilege("IAMdyNAMIC"));
    assert_eq!(GetDynamicPrivileges().len(), count + 2);

    let err = RegisterDynamicPrivilege("THIS_PRIVILEGE_NAME_IS_TOO_LONG_THE_MAX_IS_32_CHARS")
        .unwrap_err();
    assert!(
        err.to_string().contains("longer than 32 characters")
            || matches!(err, PrivilegeError::InvalidPrivilegeType(_))
    );
    assert!(
        !GetDynamicPrivileges()
            .iter()
            .any(|p| p.eq_ignore_ascii_case("THIS_PRIVILEGE_NAME_IS_TOO_LONG_THE_MAX_IS_32_CHARS"))
    );

    assert!(RemoveDynamicPrivilege("ACDC_ADMIN"));
    assert!(RemoveDynamicPrivilege("IAMDYNAMIC"));
}

#[test]
#[serial(privileges)]
/// information_schema.user_privileges 仅对自己可见；mysql.* SELECT 才能查看全部用户。
fn TestInfoSchemaUserPrivileges() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "isnobody", &[]),
        user_row("%", "isroot", &["super"]),
        user_row("%", "isselectonmysqluser", &[]),
        user_row("%", "isselectonmysql", &[]),
    ];
    source.tables_priv = vec![tables_priv_row(
        "%",
        "mysql",
        "isselectonmysqluser",
        "user",
        "Select",
        "",
    )];
    source.db = vec![db_row("%", "mysql", "isselectonmysql", &["select"])];
    let handle = load(source);

    let nobody = auth_as(&handle, "isnobody", "%");
    assert_eq!(
        nobody.UserPrivilegesTable(&[], "isnobody", "%"),
        vec![vec![
            "'isnobody'@'%'".to_string(),
            "def".to_string(),
            "USAGE".to_string(),
            "NO".to_string(),
        ]]
    );

    let table_only = auth_as(&handle, "isselectonmysqluser", "%");
    assert_eq!(
        table_only.UserPrivilegesTable(&[], "isselectonmysqluser", "%"),
        vec![vec![
            "'isselectonmysqluser'@'%'".to_string(),
            "def".to_string(),
            "USAGE".to_string(),
            "NO".to_string(),
        ]]
    );

    let mysql_db_reader = auth_as(&handle, "isselectonmysql", "%");
    let rows = mysql_db_reader.UserPrivilegesTable(&[], "isselectonmysql", "%");
    assert_eq!(rows.len(), 4);
    assert!(rows.contains(&vec![
        "'isnobody'@'%'".to_string(),
        "def".to_string(),
        "USAGE".to_string(),
        "NO".to_string(),
    ]));
    assert!(rows.contains(&vec![
        "'isroot'@'%'".to_string(),
        "def".to_string(),
        "SUPER".to_string(),
        "NO".to_string(),
    ]));
}

#[test]
#[serial(privileges)]
/// WITH GRANT OPTION 与 REVOKE 行为。
fn TestGrantOptionAndRevoke() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "u1", &["select", "grant"]),
        user_row("%", "u2", &[]),
        user_row("%", "u3", &[]),
    ];
    source.db = vec![db_row("%", "db", "u1", &["update", "delete"])];
    let handle = load(source);
    let u1 = auth_as(&handle, "u1", "%");
    let mut grants = u1.ShowGrants(
        &UserIdentity {
            Username: "u1".into(),
            Hostname: "%".into(),
        },
        &[],
        false,
    );
    grants.sort();
    assert_eq!(
        grants,
        vec![
            "GRANT SELECT ON *.* TO 'u1'@'%' WITH GRANT OPTION",
            "GRANT UPDATE,DELETE ON `db`.* TO 'u1'@'%'",
        ]
    );

    // u2: several db-level grants, one WITH GRANT OPTION, plus the implicit
    // "GRANT USAGE ON *.*" line when there's no global privilege at all.
    let mut source2 = RowDataSource::default();
    source2.user = vec![user_row("%", "u2", &[])];
    source2.db = vec![
        db_row("%", "d1", "u2", &["select"]),
        db_row("%", "d2", "u2", &["select", "grant"]),
    ];
    let handle2 = load(source2);
    let u2 = auth_as(&handle2, "u2", "%");
    let mut grants2 = u2.ShowGrants(
        &UserIdentity {
            Username: "u2".into(),
            Hostname: "%".into(),
        },
        &[],
        false,
    );
    grants2.sort();
    assert_eq!(
        grants2,
        vec![
            "GRANT SELECT ON `d1`.* TO 'u2'@'%'",
            "GRANT SELECT ON `d2`.* TO 'u2'@'%' WITH GRANT OPTION",
            "GRANT USAGE ON *.* TO 'u2'@'%'",
        ]
    );

    // u3: revoking all leaves a USAGE placeholder that still carries the
    // WITH GRANT OPTION suffix (mirrors Go's revoke-then-showgrants case).
    let mut source3 = RowDataSource::default();
    source3.user = vec![user_row("%", "u3", &[])];
    source3.db = vec![db_row("%", "hchwang", "u3", &["grant"])];
    let handle3 = load(source3);
    let u3 = auth_as(&handle3, "u3", "%");
    let mut grants3 = u3.ShowGrants(
        &UserIdentity {
            Username: "u3".into(),
            Hostname: "%".into(),
        },
        &[],
        false,
    );
    grants3.sort();
    assert_eq!(
        grants3,
        vec![
            "GRANT USAGE ON *.* TO 'u3'@'%'",
            "GRANT USAGE ON `hchwang`.* TO 'u3'@'%' WITH GRANT OPTION",
        ]
    );
}

#[test]
#[serial(privileges)]
/// DASHBOARD_CLIENT 动态权限。
fn TestDashboardClientDynamicPriv() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "dc_u1", &[]), user_row("%", "dc_r1", &[])];
    source.role_edges = vec![role_edge_row("%", "dc_r1", "%", "dc_u1")];
    let handle = load(source.clone());
    let dc_u1 = auth_as(&handle, "dc_u1", "%");
    let mut grants = dc_u1.ShowGrants(
        &UserIdentity {
            Username: "dc_u1".into(),
            Hostname: "%".into(),
        },
        &[RoleIdentity::new("dc_r1", "%")],
        false,
    );
    grants.sort();
    assert_eq!(
        grants,
        vec![
            "GRANT 'dc_r1'@'%' TO 'dc_u1'@'%'",
            "GRANT USAGE ON *.* TO 'dc_u1'@'%'",
        ]
    );

    source.dynamic_priv = vec![dynamic_priv_row("%", "dc_r1", "DASHBOARD_CLIENT", false)];
    let handle = load(source.clone());
    let dc_u1 = auth_as(&handle, "dc_u1", "%");
    let mut grants = dc_u1.ShowGrants(
        &UserIdentity {
            Username: "dc_u1".into(),
            Hostname: "%".into(),
        },
        &[RoleIdentity::new("dc_r1", "%")],
        false,
    );
    grants.sort();
    assert_eq!(
        grants,
        vec![
            "GRANT 'dc_r1'@'%' TO 'dc_u1'@'%'",
            "GRANT DASHBOARD_CLIENT ON *.* TO 'dc_u1'@'%'",
            "GRANT USAGE ON *.* TO 'dc_u1'@'%'",
        ]
    );
}

#[test]
#[serial(privileges)]
/// EVENT 权限。
fn TestGrantEvent() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "u1", &["event"])];
    source.db = vec![db_row("%", "event_db", "u1", &["event"])];
    let handle = load(source);
    let u1 = auth_as(&handle, "u1", "%");
    let mut grants = u1.ShowGrants(
        &UserIdentity {
            Username: "u1".into(),
            Hostname: "%".into(),
        },
        &[],
        false,
    );
    grants.sort();
    assert_eq!(
        grants,
        vec![
            "GRANT EVENT ON *.* TO 'u1'@'%'",
            "GRANT EVENT ON `event_db`.* TO 'u1'@'%'",
        ]
    );
}

#[test]
#[serial(privileges)]
/// CREATE TEMPORARY TABLES 权限。
fn TestGrantCreateTmpTables() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "u1", &["create_tmp_table"])];
    source.db = vec![db_row(
        "%",
        "create_tmp_table_db",
        "u1",
        &["create_tmp_table"],
    )];
    let handle = load(source);
    let u1 = auth_as(&handle, "u1", "%");
    let mut grants = u1.ShowGrants(
        &UserIdentity {
            Username: "u1".into(),
            Hostname: "%".into(),
        },
        &[],
        false,
    );
    grants.sort();
    assert_eq!(
        grants,
        vec![
            "GRANT CREATE TEMPORARY TABLES ON *.* TO 'u1'@'%'",
            "GRANT CREATE TEMPORARY TABLES ON `create_tmp_table_db`.* TO 'u1'@'%'",
        ]
    );
}

#[test]
#[serial(privileges)]
/// skip-grant-tables 模式下全部放行。
fn TestSkipGrantTable() {
    // With SkipWithGrant, every privilege/verification check trivially
    // succeeds and no cache is even consulted (mirrors Go's `--skip-grant-tables`,
    // which this test exercises across BACKUP_ADMIN/RELOAD/mysql.*/... grants).
    set_skip_with_grant(true);
    let _guard = DeferCleanup(|| set_skip_with_grant(false));

    let handle = load(RowDataSource::default());
    let active_roles = no_roles();
    let mut p = auth_as(&handle, "test1", "%");
    assert!(p.RequestVerification(&active_roles, "", "", "", ReloadPriv));
    assert!(p.RequestVerification(&active_roles, "mysql", "user", "", SelectPriv));
    for dyn_priv in ["BACKUP_ADMIN", "RESTORE_ADMIN", "RESTRICTED_TABLES_ADMIN"] {
        assert!(p.RequestDynamicVerificationWithUser(
            dyn_priv,
            false,
            Some(&UserIdentity {
                Username: "test1".into(),
                Hostname: "%".into()
            })
        ));
    }
    let info = p
        .ConnectionVerification(
            &UserIdentity {
                Username: "test2".into(),
                Hostname: "%".into(),
            },
            "test2",
            "%",
            &[],
            &[],
            &session_with_tls(None),
        )
        .unwrap();
    assert_eq!(info.authenticated_user, "test2");
    assert_eq!(p.GetUserResources("test2", "%").unwrap(), 0);
    assert_eq!(
        p.ActiveRoles(&[RoleIdentity {
            Username: "missing".into(),
            Hostname: "%".into()
        }]),
        (true, String::new())
    );
    assert!(!p.FindEdge(
        &RoleIdentity {
            Username: "missing".into(),
            Hostname: "%".into()
        },
        &UserIdentity {
            Username: "test2".into(),
            Hostname: "%".into()
        },
    ));
    assert!(p.GetDefaultRoles("test2", "%").is_empty());
    assert!(p.GetAllRoles("test2", "%").is_empty());
}

#[test]
#[serial(privileges)]
/// 回归：issue 29823 相关权限边界。
fn TestIssue29823() {
    // Adapted: role-membership revocation must immediately stop granting the
    // role's privileges to the (still-active-role-set) user, since
    // `RequestVerification` re-resolves `FindAllRole` from the live cache on
    // every call rather than caching a snapshot.
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "u1", &[]), user_row("%", "r1", &[])];
    source.role_edges = vec![role_edge_row("%", "r1", "%", "u1")];
    source.tables_priv = vec![tables_priv_row("%", "test", "r1", "t1", "Select", "")];
    let handle = load(source.clone());
    let active_roles = vec![RoleIdentity::new("r1", "%")];
    let u1 = auth_as(&handle, "u1", "%");
    assert!(u1.RequestVerification(&active_roles, "test", "t1", "", SelectPriv));

    // revoke r1 from u1
    source.role_edges.clear();
    let handle = load(source);
    let u1 = auth_as(&handle, "u1", "%");
    assert!(!u1.RequestVerification(&active_roles, "test", "t1", "", SelectPriv));
}

#[test]
#[serial(privileges)]
/// 回归：issue 37488 相关权限边界。
fn TestIssue37488() {
    // Host-pattern precedence: an exact-ish '192.168.%' record must win over
    // '%' for a connection from 192.168.13.15, and the extra privileges on
    // that host apply.
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "dba_test", &["select", "insert", "update", "delete"]),
        user_row(
            "192.168.%",
            "dba_test",
            &[
                "select", "insert", "update", "delete", "create", "drop", "alter",
            ],
        ),
    ];
    let handle = load(source);
    let mut p = NewUserPrivileges(handle);
    let info = p
        .ConnectionVerification(
            &UserIdentity {
                Username: "dba_test".into(),
                Hostname: "192.168.13.15".into(),
            },
            "dba_test",
            "192.168.13.15",
            &[],
            &[],
            &session_with_tls(None),
        )
        .unwrap();
    assert_eq!(info.authenticated_host, "192.168.%");
    let active_roles = no_roles();
    assert!(p.RequestVerification(&active_roles, "test", "a", "", DropPriv));
}

#[test]
#[serial(privileges)]
/// 密码过期检测。
fn TestCheckPasswordExpired() {
    let session = SessionVars {
        tls_state: None,
        default_password_lifetime: 0,
    };
    let handle = Handle::New();
    let p = NewUserPrivileges(handle);
    let mut record = NewUserRecord("%", "root");

    record.PasswordExpired = true;
    assert!(matches!(
        p.CheckPasswordExpired(&session, &record),
        Err(PrivilegeError::MustChangePassword)
    ));

    record.PasswordExpired = false;
    let mut session = session;
    session.default_password_lifetime = 2;
    // use default_password_lifetime
    record.PasswordLifeTime = -1;
    record.PasswordLastChanged = now_unix_for_test() - 2 * 86400;
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(p.CheckPasswordExpired(&session, &record).is_err());
    record.PasswordLastChanged = now_unix_for_test() - 1 * 86400;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());

    // never expire
    record.PasswordLifeTime = 0;
    record.PasswordLastChanged = now_unix_for_test() - 10 * 86400;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());

    // expire with the specified time
    record.PasswordLifeTime = 3;
    record.PasswordLastChanged = now_unix_for_test() - 3 * 86400;
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(p.CheckPasswordExpired(&session, &record).is_err());
    record.PasswordLastChanged = now_unix_for_test() - 2 * 86400;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());
}

/// 测试用当前 Unix 秒时间戳。
fn now_unix_for_test() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
#[serial(privileges)]
/// 非沙箱模式密码过期应拒绝登录。
fn TestPasswordExpireWithoutSandBoxMode() {
    // Adapted: drives `CheckPasswordExpired` directly (which is exactly what
    // `ConnectionVerification` calls during `Session.Auth`) instead of a real
    // session/`ALTER USER` round-trip.
    set_sandbox_mode(false);
    let handle = Handle::New();
    let p = NewUserPrivileges(handle);
    let mut record = NewUserRecord("localhost", "testuser");
    let mut session = SessionVars {
        tls_state: None,
        default_password_lifetime: 0,
    };

    // PASSWORD EXPIRE
    record.PasswordExpired = true;
    assert!(p.CheckPasswordExpired(&session, &record).is_err());

    // PASSWORD EXPIRE NEVER
    record.PasswordExpired = false;
    record.PasswordLifeTime = 0;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());

    // PASSWORD EXPIRE INTERVAL 2 DAY
    record.PasswordLifeTime = 2;
    record.PasswordLastChanged = now_unix_for_test() - 86400;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());
    record.PasswordLastChanged = now_unix_for_test() - 2 * 86400;
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(p.CheckPasswordExpired(&session, &record).is_err());

    // PASSWORD EXPIRE DEFAULT
    record.PasswordLifeTime = -1;
    session.default_password_lifetime = 2;
    record.PasswordLastChanged = now_unix_for_test() - 2 * 86400 - 1;
    assert!(p.CheckPasswordExpired(&session, &record).is_err());
    session.default_password_lifetime = 3;
    assert!(p.CheckPasswordExpired(&session, &record).is_ok());
}

#[test]
#[serial(privileges)]
/// 沙箱模式密码过期返回需改密标记。
fn TestPasswordExpireWithSandBoxMode() {
    let handle = Handle::New();
    let p = NewUserPrivileges(handle);
    let mut record = NewUserRecord("localhost", "testuser");
    let session = SessionVars {
        tls_state: None,
        default_password_lifetime: 0,
    };

    set_sandbox_mode(true);
    let _guard = DeferCleanup(|| set_sandbox_mode(false));

    record.PasswordExpired = true;
    // In sandbox mode, an expired password reports `Ok(true)` (enter
    // sandbox mode) instead of an error.
    assert_eq!(p.CheckPasswordExpired(&session, &record), Ok(true));

    record.PasswordExpired = false;
    record.PasswordLifeTime = 0;
    assert_eq!(p.CheckPasswordExpired(&session, &record), Ok(false));
}

#[test]
#[serial(privileges)]
/// skip-grant 下空句柄路径不 panic。
fn TestNilHandleInSkipWithGrant() {
    set_skip_with_grant(true);
    let _guard = DeferCleanup(|| set_skip_with_grant(false));

    let handle = Handle::New(); // never loaded ("nil" in spirit): empty cache.
    let mut p = NewUserPrivileges(handle);
    // ConnectionVerification
    let info = p
        .ConnectionVerification(
            &UserIdentity {
                Username: "root".into(),
                Hostname: "%".into(),
            },
            "root",
            "%",
            &[],
            &[],
            &session_with_tls(None),
        )
        .expect("SkipWithGrant must bypass an empty cache");
    assert_eq!(info.authenticated_user, "root");
    // GetUserResources
    assert_eq!(p.GetUserResources("root", "%").unwrap(), 0);
}

#[test]
#[serial(privileges)]
/// SHOW GRANTS 在 ANSI_QUOTES 等 SQL mode 下的引号风格。
fn TestShowGrantsSQLMode() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "show_sql_mode", &[])];
    source.db = vec![db_row("localhost", "test", "show_sql_mode", &["select"])];
    let handle = load(source);
    let user = UserIdentity {
        Username: "show_sql_mode".into(),
        Hostname: "localhost".into(),
    };
    let p = auth_as(&handle, "show_sql_mode", "localhost");

    let mut normal = p.ShowGrants(&user, &[], false);
    normal.sort();
    assert_eq!(
        normal,
        vec![
            "GRANT SELECT ON `test`.* TO 'show_sql_mode'@'localhost'",
            "GRANT USAGE ON *.* TO 'show_sql_mode'@'localhost'",
        ]
    );

    let mut ansi = p.ShowGrants(&user, &[], true);
    ansi.sort();
    assert_eq!(
        ansi,
        vec![
            "GRANT SELECT ON \"test\".* TO 'show_sql_mode'@'localhost'",
            "GRANT USAGE ON *.* TO 'show_sql_mode'@'localhost'",
        ]
    );
}

#[test]
#[serial(privileges)]
fn TestAlterUserStmt() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "creator", &["create_user"]),
        user_row("%", "updater", &[]),
        user_row("%", "restricted_admin", &[]),
    ];
    source.tables_priv = vec![tables_priv_row(
        "%", "mysql", "updater", "user", "Update", "",
    )];
    source.dynamic_priv = vec![
        dynamic_priv_row("%", "restricted_admin", "RESTRICTED_TABLES_ADMIN", false),
        dynamic_priv_row("%", "restricted_admin", "RESTRICTED_USER_ADMIN", false),
    ];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    for version in [sem::V1, sem::V2] {
        let cleanup = sem::SwitchToSEMForTest(version);
        assert!(!auth_as(&handle, "updater", "%").RequestVerification(
            &[],
            "mysql",
            "user",
            "",
            UpdatePriv,
        ));
        assert!(auth_as(&handle, "creator", "%").RequestVerification(
            &[],
            "",
            "",
            "",
            CreateUserPriv,
        ));
        let admin = auth_as(&handle, "restricted_admin", "%");
        assert!(admin.RequestDynamicVerification(&[], "RESTRICTED_USER_ADMIN", false));
        cleanup();
    }
}

#[test]
#[serial(privileges)]
fn TestShowViewPriv() {
    let mut source = RowDataSource::default();
    for user in ["vnobody", "vshowview", "vselect", "vshowandselect"] {
        source.user.push(user_row("%", user, &[]));
    }
    source.tables_priv = vec![
        tables_priv_row("%", "test", "vshowview", "v", "Show View", ""),
        tables_priv_row("%", "test", "vselect", "v", "Select", ""),
        tables_priv_row("%", "test", "vshowandselect", "v", "Select,Show View", ""),
    ];
    let handle = load(source);
    for (user, select, show) in [
        ("vnobody", false, false),
        ("vshowview", false, true),
        ("vselect", true, false),
        ("vshowandselect", true, true),
    ] {
        let p = auth_as(&handle, user, "%");
        assert_eq!(
            p.RequestVerification(&[], "test", "v", "", SelectPriv),
            select
        );
        assert_eq!(
            p.RequestVerification(&[], "test", "v", "", ShowViewPriv),
            show
        );
    }
}

#[test]
#[serial(privileges)]
fn TestShowCreateTable() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "tsct1", &[]),
        user_row("%", "tsct2", &[]),
        user_row("%", "tsct3", &[]),
    ];
    source.db = vec![
        db_row("%", "mysql", "tsct2", &["select"]),
        db_row("%", "mysql", "tsct3", &["create_tmp_table"]),
    ];
    let handle = load(source);
    assert!(!auth_as(&handle, "tsct1", "%").RequestVerification(
        &[],
        "mysql",
        "user",
        "",
        SelectPriv,
    ));
    assert!(auth_as(&handle, "tsct2", "%").RequestVerification(
        &[],
        "mysql",
        "user",
        "",
        SelectPriv,
    ));
    assert!(!auth_as(&handle, "tsct3", "%").RequestVerification(
        &[],
        "mysql",
        "user",
        "",
        SelectPriv,
    ));
}

#[test]
#[serial(privileges)]
fn TestAnalyzeTable() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "anobody", &[])];
    let handle = load(source.clone());
    let p = auth_as(&handle, "anobody", "%");
    assert!(!p.RequestVerification(&[], "atest", "t1", "", SelectPriv));
    assert!(!p.RequestVerification(&[], "atest", "t1", "", InsertPriv));
    source.db = vec![db_row("%", "atest", "anobody", &["select"])];
    handle.merge(reload(&source));
    let p = auth_as(&handle, "anobody", "%");
    assert!(p.RequestVerification(&[], "atest", "t1", "", SelectPriv));
    assert!(!p.RequestVerification(&[], "atest", "t1", "", InsertPriv));
    source.db = vec![db_row("%", "atest", "anobody", &["select", "insert"])];
    handle.merge(reload(&source));
    assert!(auth_as(&handle, "anobody", "%").RequestVerification(
        &[],
        "atest",
        "t1",
        "",
        InsertPriv,
    ));
}

#[test]
#[serial(privileges)]
fn TestAdminCommand() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "test_admin", &[]),
        user_row("%", "root", &["super"]),
    ];
    let handle = load(source);
    assert!(!auth_as(&handle, "test_admin", "%").RequestVerification(&[], "", "", "", SuperPriv,));
    assert!(auth_as(&handle, "root", "%").RequestVerification(&[], "", "", "", SuperPriv,));
}

#[test]
#[serial(privileges)]
fn TestLoadDataPrivilege() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("localhost", "test_load", &[])];
    let handle = load(source.clone());
    assert!(
        !auth_as(&handle, "test_load", "localhost").RequestVerification(
            &[],
            "test",
            "t_load",
            "",
            InsertPriv,
        )
    );
    source.user = vec![user_row("localhost", "test_load", &["insert"])];
    handle.merge(reload(&source));
    let p = auth_as(&handle, "test_load", "localhost");
    assert!(p.RequestVerification(&[], "test", "t_load", "", InsertPriv));
    assert!(!p.RequestVerification(&[], "test", "t_load", "", DeletePriv));
}

#[test]
#[serial(privileges)]
fn TestSecurityEnhancedModeInfoschema() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "uroot1", &["super", "process"]),
        user_row("%", "uroot2", &["super", "process"]),
    ];
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "uroot2",
        "RESTRICTED_TABLES_ADMIN",
        false,
    )];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    let cleanup = sem::SwitchToSEMForTest(sem::V1);
    assert!(!auth_as(&handle, "uroot1", "%").RequestVerification(
        &[],
        "information_schema",
        "cluster_config",
        "",
        SelectPriv,
    ));
    assert!(auth_as(&handle, "uroot2", "%").RequestVerification(
        &[],
        "information_schema",
        "cluster_config",
        "",
        SelectPriv,
    ));
    cleanup();
}

#[test]
#[serial(privileges)]
fn TestSecurityEnhancedLocalBackupRestore() {
    let mut source = RowDataSource::default();
    source.user = vec![user_row("%", "backuprestore", &[])];
    source.dynamic_priv = vec![
        dynamic_priv_row("%", "backuprestore", "BACKUP_ADMIN", false),
        dynamic_priv_row("%", "backuprestore", "RESTORE_ADMIN", false),
    ];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    let cleanup = sem::SwitchToSEMForTest(sem::V1);
    let p = auth_as(&handle, "backuprestore", "%");
    assert!(p.RequestDynamicVerification(&[], "BACKUP_ADMIN", false));
    assert!(p.RequestDynamicVerification(&[], "RESTORE_ADMIN", false));
    cleanup();
}

#[test]
#[serial(privileges)]
fn TestSecurityEnhancedModeSysVars() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "svroot1", &["super"]),
        user_row("%", "svroot2", &["super"]),
    ];
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "svroot2",
        "RESTRICTED_VARIABLES_ADMIN",
        false,
    )];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    for version in [sem::V1, sem::V2] {
        let cleanup = sem::SwitchToSEMForTest(version);
        assert!(
            !auth_as(&handle, "svroot1", "%").RequestDynamicVerification(
                &[],
                "RESTRICTED_VARIABLES_ADMIN",
                false,
            )
        );
        assert!(auth_as(&handle, "svroot2", "%").RequestDynamicVerification(
            &[],
            "RESTRICTED_VARIABLES_ADMIN",
            false,
        ));
        cleanup();
    }
}

#[test]
#[serial(privileges)]
fn TestViewDefiner() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "mobius-admin", &[]),
        user_row("%", "ACL-mobius-admin", &[]),
    ];
    source.db = vec![db_row("%", "issue24414", "ACL-mobius-admin", &["select"])];
    source.role_edges = vec![role_edge_row("%", "ACL-mobius-admin", "%", "mobius-admin")];
    source.default_roles = vec![default_role_row(
        "%",
        "mobius-admin",
        "%",
        "ACL-mobius-admin",
    )];
    let p = NewUserPrivileges(load(source));
    assert!(p.RequestVerificationWithUser(
        &[],
        "issue24414",
        "table1",
        "",
        SelectPriv,
        Some(&UserIdentity {
            Username: "mobius-admin".into(),
            Hostname: "127.0.0.1".into(),
        }),
    ));
}

#[test]
#[serial(privileges)]
fn TestSecurityEnhancedModeRestrictedUsers() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "ruroot1", &["super", "create_user"]),
        user_row("%", "ruroot2", &["super", "create_user"]),
    ];
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "ruroot2",
        "RESTRICTED_USER_ADMIN",
        false,
    )];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    let cleanup = sem::SwitchToSEMForTest(sem::V1);
    assert!(
        !auth_as(&handle, "ruroot1", "%").RequestDynamicVerification(
            &[],
            "RESTRICTED_USER_ADMIN",
            false,
        )
    );
    assert!(auth_as(&handle, "ruroot2", "%").RequestDynamicVerification(
        &[],
        "RESTRICTED_USER_ADMIN",
        false,
    ));
    cleanup();
}

#[test]
#[serial(privileges)]
fn TestCreateTmpTablesPriv() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "vcreate", &[]),
        user_row("%", "vcreate_tmp", &[]),
        user_row("%", "vcreate_tmp_all", &["create_tmp_table"]),
    ];
    source.db = vec![
        db_row("%", "test", "vcreate", &["create"]),
        db_row("%", "test", "vcreate_tmp", &["create_tmp_table"]),
    ];
    let handle = load(source);
    assert!(!auth_as(&handle, "vcreate", "%").RequestVerification(
        &[],
        "test",
        "tmp",
        "",
        CreateTMPTablePriv,
    ));
    assert!(auth_as(&handle, "vcreate_tmp", "%").RequestVerification(
        &[],
        "test",
        "tmp",
        "",
        CreateTMPTablePriv,
    ));
    assert!(
        auth_as(&handle, "vcreate_tmp_all", "%").RequestVerification(
            &[],
            "test",
            "tmp",
            "",
            CreateTMPTablePriv,
        )
    );
}

#[test]
#[serial(privileges)]
fn TestVerificationInfoWithSessionTokenPlugin() {
    let mut record = NewUserRecord("localhost", "testuser");
    record.PasswordExpired = true;
    record.ResourceGroup = "rg1".into();
    let info = VerificationInfoWithSessionToken(&record, true)
        .expect("a validated session token bypasses password expiration");
    assert!(!info.password_expired);
    assert_eq!(info.resource_group_name, "rg1");
    assert!(VerificationInfoWithSessionToken(&record, false).is_err());
}

#[test]
#[serial(privileges)]
fn TestGrantOptionWithSEMv2() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "varuser1", &["file"]),
        user_row("%", "varuser2", &["file", "grant"]),
        user_row("%", "varuser4", &["file", "grant"]),
    ];
    source.dynamic_priv = vec![
        dynamic_priv_row("%", "varuser1", "SYSTEM_VARIABLES_ADMIN", false),
        dynamic_priv_row("%", "varuser2", "SYSTEM_VARIABLES_ADMIN", true),
        dynamic_priv_row("%", "varuser4", "RESTRICTED_PRIV_ADMIN", false),
    ];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    let cleanup = sem::SwitchToSEMForTest(sem::V2);
    assert!(
        !auth_as(&handle, "varuser1", "%").RequestDynamicVerification(
            &[],
            "SYSTEM_VARIABLES_ADMIN",
            true,
        )
    );
    assert!(
        auth_as(&handle, "varuser2", "%").RequestDynamicVerification(
            &[],
            "SYSTEM_VARIABLES_ADMIN",
            true,
        )
    );
    assert!(
        auth_as(&handle, "varuser4", "%").RequestDynamicVerification(
            &[],
            "RESTRICTED_PRIV_ADMIN",
            false,
        )
    );
    cleanup();
}

#[test]
#[serial(privileges)]
fn TestProtectUserAndRoleWithRestrictedPrivileges() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "normal_user", &["super", "create_user"]),
        user_row("%", "restricted_user", &["super", "create_user"]),
    ];
    source.dynamic_priv = vec![dynamic_priv_row(
        "%",
        "restricted_user",
        "RESTRICTED_USER_ADMIN",
        false,
    )];
    let handle = load(source);
    register_go_initialized_sem_sysvars();
    for version in [sem::V1, sem::V2] {
        let cleanup = sem::SwitchToSEMForTest(version);
        assert!(
            !auth_as(&handle, "normal_user", "%").RequestDynamicVerification(
                &[],
                "RESTRICTED_USER_ADMIN",
                false,
            )
        );
        assert!(
            auth_as(&handle, "restricted_user", "%").RequestDynamicVerification(
                &[],
                "RESTRICTED_USER_ADMIN",
                false,
            )
        );
        cleanup();
    }
}

#[test]
#[serial(privileges)]
/// 指定用户检查会按 Go 语义处理空身份、information_schema 与默认角色。
fn TestEnsureActiveUserCoverage() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "active_user", &[]),
        user_row("%", "active_role", &[]),
    ];
    source.db = vec![db_row("%", "active_db", "active_role", &["select"])];
    source.role_edges = vec![role_edge_row("%", "active_role", "%", "active_user")];
    source.default_roles = vec![default_role_row("%", "active_user", "%", "active_role")];
    let handle = load(source);
    let privileges = NewUserPrivileges(handle);

    let empty = UserIdentity::default();
    assert!(privileges.RequestVerificationWithUser(
        &[],
        "ordinary_db",
        "",
        "",
        SelectPriv,
        Some(&empty),
    ));

    let user = UserIdentity {
        Username: "active_user".into(),
        Hostname: "%".into(),
    };
    assert!(privileges.RequestVerificationWithUser(
        &[],
        "information_schema",
        "tables",
        "",
        UpdatePriv,
        Some(&user),
    ));
    assert!(privileges.RequestVerificationWithUser(
        &[],
        "active_db",
        "",
        "",
        SelectPriv,
        Some(&user),
    ));
}

#[test]
#[serial(privileges)]
fn TestSQLVariableAccelerateUserCreationUpdate() {
    let mut initial = RowDataSource::default();
    initial.user = vec![user_row("%", "aaa", &[])];
    let handle = Handle::New();
    handle.UpdateAll(&initial).expect("initial full reload");
    assert!(handle.CheckFullData());

    let mut latest = initial.clone();
    latest.user.push(user_row("%", "bbb", &[]));
    latest.db = vec![db_row("%", "test", "bbb", &["select"])];
    handle
        .UpdateAllActive(&latest)
        .expect("switch to active-user mode");
    assert!(!handle.CheckFullData());

    handle
        .Update(&["bbb".into()], &latest)
        .expect("inactive user updates are deferred");
    assert!(
        !handle
            .Get()
            .RequestVerification(&[], "bbb", "%", "test", "", "", SelectPriv,)
    );

    handle
        .ensureActiveUser("bbb")
        .expect("login loads the user and its roles");
    assert!(
        handle
            .Get()
            .RequestVerification(&[], "bbb", "%", "test", "", "", SelectPriv,)
    );

    handle.UpdateAll(&latest).expect("full reload");
    assert!(handle.CheckFullData());
}
