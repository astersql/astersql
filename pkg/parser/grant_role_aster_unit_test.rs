// Copyright 2026 AsterSQL.

use crate::Parser;

#[test]
fn bare_role_identifiers_keep_go_default_host_for_grant_and_revoke() {
    let grant = Parser::default()
        .ParseOneStmt("grant r_1 to root", "", "")
        .expect("parse GRANT ROLE with bare identifiers");
    let grant = grant
        .as_any()
        .downcast_ref::<parser_ast::GrantRoleStmt>()
        .expect("GRANT ROLE AST");
    assert_eq!(grant.Roles.len(), 1);
    assert_eq!(grant.Roles[0].username, "r_1");
    assert_eq!(grant.Roles[0].hostname, "%");
    assert_eq!(grant.Users.len(), 1);
    assert_eq!(grant.Users[0].username, "root");
    assert_eq!(grant.Users[0].hostname, "%");

    let revoke = Parser::default()
        .ParseOneStmt("revoke r_1 from root", "", "")
        .expect("parse REVOKE ROLE with bare identifiers");
    let revoke = revoke
        .as_any()
        .downcast_ref::<parser_ast::RevokeRoleStmt>()
        .expect("REVOKE ROLE AST");
    assert_eq!(revoke.Roles.len(), 1);
    assert_eq!(revoke.Roles[0].username, "r_1");
    assert_eq!(revoke.Roles[0].hostname, "%");
    assert_eq!(revoke.Users.len(), 1);
    assert_eq!(revoke.Users[0].username, "root");
    assert_eq!(revoke.Users[0].hostname, "%");
}
