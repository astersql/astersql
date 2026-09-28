// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 会话权限（privileges）行为测试。
//
// 对照 Go `TestSkipWithGrant` / `TestSessionAuth`：验证 `SkipWithGrant` 开关对
// `UserPrivileges.ConnectionVerification`（连接认证）的影响，以及未知用户空密码拒绝路径。

const _GO_DRAFT_ARCHIVE: &str = r################"
// auth、privileges、testkit、require 等外部依赖均为 Go 语义占位。

// test_skip_with_grant 对应 Go 的 TestSkipWithGrant。
// 它临时修改全局 SkipWithGrant，验证关闭时未知用户认证失败、开启后可跳过 grant 检查。
#[test]
fn test_skip_with_grant() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);

    let save2 = privileges::SkipWithGrant;

    privileges::SkipWithGrant = false;
    require::Error(tk.Session().Auth(
        &auth::UserIdentity { Username: "user_not_exist".to_string(), Hostname: String::new() },
        b"yyy",
        b"zzz",
        None,
    ));

    privileges::SkipWithGrant = true;
    require::NoError(tk.Session().Auth(
        &auth::UserIdentity { Username: "xxx".to_string(), Hostname: "%".to_string() },
        b"yyy",
        b"zzz",
        None,
    ));
    require::NoError(tk.Session().Auth(
        &auth::UserIdentity { Username: "root".to_string(), Hostname: "%".to_string() },
        b"",
        b"",
        None,
    ));
    tk.MustExec("use test");
    tk.MustExec("create table t (id int)");
    tk.MustExec("create role r_1");
    tk.MustExec("grant r_1 to root");
    tk.MustExec("set role all");
    tk.MustExec("show grants for root");

    // Go 测试末尾手动恢复全局开关；显式保留这个资源收尾点。
    privileges::SkipWithGrant = save2;
}

// test_session_auth 对应 Go 的 TestSessionAuth。
// 它验证不存在用户名即使空密码也不能通过 Session.Auth。
#[test]
fn test_session_auth() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    require::Error(tk.Session().Auth(
        &auth::UserIdentity {
            Username: "Any not exist username with zero password!".to_string(),
            Hostname: "anyhost".to_string(),
        },
        b"",
        b"",
        None,
    ));
}
"################;

use serial_test::serial;

use astersql_privilege_privileges::{
    Handle, NewUserPrivileges, SessionVars, SkipWithGrant, UserIdentity, set_skip_with_grant,
};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

/// RAII 清理守卫：Drop 时执行闭包，模拟 Go `defer` 恢复全局开关。
struct DeferCleanup<F: FnMut()>(F);
impl<F: FnMut()> Drop for DeferCleanup<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// 将 Session.Auth 语义落到 `UserPrivileges.ConnectionVerification`。
///
/// 空 Hostname 时按 Go 惯例补成 `%`（任意主机）。
// 对应 Go `tk.Session().Auth`：Session 认证最终落到 UserPrivileges.ConnectionVerification。
fn session_auth(
    privileges: &mut astersql_privilege_privileges::UserPrivileges,
    user: &UserIdentity,
    auth_data: &[u8],
    salt: &[u8],
) -> Result<(), astersql_privilege_privileges::PrivilegeError> {
    privileges
        .ConnectionVerification(
            user,
            &user.Username,
            if user.Hostname.is_empty() {
                "%"
            } else {
                &user.Hostname
            },
            auth_data,
            salt,
            &SessionVars::default(),
        )
        .map(|_| ())
}

/// 关闭 SkipWithGrant 时未知用户认证失败；开启后可过 Auth，并继续角色授权相关调用。
// 对应 TestSkipWithGrant：关闭 SkipWithGrant 时未知用户认证失败；开启后任意用户可过，
// 并能继续完成 create role / grant / show grants 等会话动作。
#[test]
#[serial(privileges)]
fn skip_with_grant_toggles_session_auth_and_allows_role_grants() {
    let save = SkipWithGrant();
    let _guard = DeferCleanup(move || set_skip_with_grant(save));

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    let handle = Handle::New();
    let mut privileges = NewUserPrivileges(handle);

    set_skip_with_grant(false);
    assert!(
        session_auth(
            &mut privileges,
            &UserIdentity {
                Username: "user_not_exist".into(),
                Hostname: String::new()
            },
            b"yyy",
            b"zzz",
        )
        .is_err()
    );

    set_skip_with_grant(true);
    assert!(
        session_auth(
            &mut privileges,
            &UserIdentity {
                Username: "xxx".into(),
                Hostname: "%".into()
            },
            b"yyy",
            b"zzz",
        )
        .is_ok()
    );
    assert!(
        session_auth(
            &mut privileges,
            &UserIdentity {
                Username: "root".into(),
                Hostname: "%".into()
            },
            b"",
            b"",
        )
        .is_ok()
    );

    // 与 Go 一样通过真实会话 SQL 链路覆盖 DDL、角色授予、激活和授权展示；
    // 不直接注入权限缓存，否则会绕过 parser/dispatch/session 的行为契约。
    tk.Session()
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "root".into(),
            hostname: "%".into(),
            ..Default::default()
        })
        .expect("authenticate root session");
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (id int)", Vec::new());
    tk.MustExec("create role r_1", Vec::new());
    tk.MustExec("grant r_1 to root", Vec::new());
    tk.MustExec("set role all", Vec::new());
    let grants = tk.MustQuery("show grants for root", Vec::new()).Rows();
    assert!(
        grants.iter().flatten().any(|value| value.contains("r_1")),
        "SHOW GRANTS must include the granted role: {grants:?}"
    );
}

/// SkipWithGrant 关闭时，不存在用户即使空密码也必须认证失败。
// 对应 TestSessionAuth：SkipWithGrant 关闭时，不存在用户即使空密码也必须认证失败。
#[test]
#[serial(privileges)]
fn session_auth_rejects_unknown_user_with_empty_password() {
    let save = SkipWithGrant();
    let _guard = DeferCleanup(move || set_skip_with_grant(save));
    set_skip_with_grant(false);

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());

    let handle = Handle::New();
    let mut privileges = NewUserPrivileges(handle);
    let err = session_auth(
        &mut privileges,
        &UserIdentity {
            Username: "Any not exist username with zero password!".into(),
            Hostname: "anyhost".into(),
        },
        b"",
        b"",
    );
    assert!(
        err.is_err(),
        "unknown user must fail auth when SkipWithGrant is off"
    );
}
