// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `GRANT` 相关错误类型与全局 TLS 权限默认值的冒烟测试。
//
// `GRANT`：向用户授予权限；`GlobalPrivValue` 保存 REQUIRE SSL/X509 等全局选项。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::grant::{
    AuthTokenOrTlsOption, AuthTokenOrTlsOptionType, CellValue, FieldType, GlobalPrivValue,
    GrantDependencies, GrantError, GrantErrorKind, GrantExec, GrantLevel, GrantLevelKind,
    GrantResult, ObjectType, PrivilegeCatalog, PrivilegeElement, PrivilegeType, RecordSet,
    ResultField, Row, SslType, StaticPrivilege, SystemMutation, SystemQuery, SystemSession,
    TableMetadata, UserIdentity, UserSpec,
};

#[derive(Default)]
struct SessionState {
    mutations: Vec<SystemMutation>,
    began: usize,
    committed: usize,
    rolled_back: usize,
}

struct MockRecordSet {
    rows: Option<Vec<Row>>,
    fields: Vec<ResultField>,
}

impl RecordSet for MockRecordSet {
    fn next(&mut self, _maximum_rows: usize) -> GrantResult<Vec<Row>> {
        Ok(self.rows.take().unwrap_or_default())
    }

    fn fields(&self) -> Vec<ResultField> {
        self.fields.clone()
    }

    fn close(&mut self) -> GrantResult {
        Ok(())
    }
}

struct MockSession {
    state: Arc<Mutex<SessionState>>,
}

impl SystemSession for MockSession {
    fn set_user(&mut self, _user: &UserIdentity) -> GrantResult {
        Ok(())
    }

    fn begin(&mut self) -> GrantResult {
        self.state.lock().unwrap().began += 1;
        Ok(())
    }

    fn commit(&mut self) -> GrantResult {
        self.state.lock().unwrap().committed += 1;
        Ok(())
    }

    fn rollback(&mut self) -> GrantResult {
        self.state.lock().unwrap().rolled_back += 1;
        Ok(())
    }

    fn execute(&mut self, mutation: SystemMutation) -> GrantResult {
        self.state.lock().unwrap().mutations.push(mutation);
        Ok(())
    }

    fn query(&mut self, query: SystemQuery) -> GrantResult<Box<dyn RecordSet>> {
        let (rows, fields) = match query {
            SystemQuery::TablePrivileges { .. } => (
                vec![Row(vec![
                    CellValue::Set(String::new()),
                    CellValue::Set(String::new()),
                ])],
                vec![
                    ResultField {
                        field_type: FieldType::Set,
                    },
                    ResultField {
                        field_type: FieldType::Set,
                    },
                ],
            ),
            SystemQuery::ColumnPrivileges { .. } => (
                vec![Row(vec![CellValue::Set(String::new())])],
                vec![ResultField {
                    field_type: FieldType::Set,
                }],
            ),
            _ => (Vec::new(), Vec::new()),
        };
        Ok(Box::new(MockRecordSet {
            rows: Some(rows),
            fields,
        }))
    }

    fn grantor(&self) -> GrantResult<String> {
        Ok("root@localhost".to_owned())
    }

    fn maximum_chunk_size(&self) -> GrantResult<usize> {
        Ok(32)
    }
}

struct MockDependencies {
    state: Arc<Mutex<SessionState>>,
    schemas: BTreeMap<String, String>,
    tables: BTreeMap<(String, String), TableMetadata>,
    dynamic_privileges: Vec<String>,
    notified: Mutex<Vec<UserIdentity>>,
}

impl MockDependencies {
    fn new(state: Arc<Mutex<SessionState>>) -> Self {
        Self {
            state,
            schemas: BTreeMap::from([("TEST".to_owned(), "test".to_owned())]),
            tables: BTreeMap::from([(
                ("test".to_owned(), "T".to_owned()),
                TableMetadata {
                    schema_name: "test".to_owned(),
                    table_name: "t".to_owned(),
                    columns: vec!["ID".to_owned(), "name".to_owned()],
                },
            )]),
            dynamic_privileges: vec!["BACKUP_ADMIN".to_owned()],
            notified: Mutex::new(Vec::new()),
        }
    }
}

impl GrantDependencies for MockDependencies {
    fn current_database(&self) -> GrantResult<String> {
        Ok("test".to_owned())
    }

    fn current_user(&self) -> GrantResult<UserIdentity> {
        Ok(user("root"))
    }

    fn no_auto_create_user(&self) -> GrantResult<bool> {
        Ok(false)
    }

    fn new_transaction_in_statement(&self) -> GrantResult {
        Ok(())
    }

    fn set_in_transaction(&self, _in_transaction: bool) -> GrantResult {
        Ok(())
    }

    fn acquire_system_session(&self) -> GrantResult<Box<dyn SystemSession>> {
        Ok(Box::new(MockSession {
            state: Arc::clone(&self.state),
        }))
    }

    fn release_system_session(&self, _session: Box<dyn SystemSession>) -> GrantResult {
        Ok(())
    }

    fn default_auth_plugin(&self) -> GrantResult<String> {
        Ok("mysql_native_password".to_owned())
    }

    fn user_exists_with_retry_variants(
        &self,
        _username: &mut String,
        _hostname: &str,
    ) -> GrantResult<bool> {
        Ok(true)
    }

    fn validate_username(&self, _username: &str) -> GrantResult {
        Ok(())
    }

    fn encode_password(
        &self,
        _user: &UserSpec,
        _auth_plugin: &str,
        _default_auth_plugin: &str,
    ) -> GrantResult<String> {
        Ok("encoded".to_owned())
    }

    fn canonical_schema_name(&self, database: &str) -> GrantResult<Option<String>> {
        Ok(self
            .schemas
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(database))
            .map(|(_, canonical)| canonical.clone()))
    }

    fn table_by_name(&self, database: &str, table: &str) -> GrantResult<Option<TableMetadata>> {
        Ok(self
            .tables
            .iter()
            .find(|((schema, name), _)| {
                schema.eq_ignore_ascii_case(database) && name.eq_ignore_ascii_case(table)
            })
            .map(|(_, metadata)| metadata.clone()))
    }

    fn is_dynamic_privilege(&self, privilege: &str) -> GrantResult<bool> {
        Ok(self.dynamic_privileges.iter().any(|item| item == privilege))
    }

    fn is_supported_cipher(&self, cipher: &str) -> GrantResult<bool> {
        Ok(cipher == "TLS_AES_128_GCM_SHA256")
    }

    fn validate_x509_name(&self, _name: &str) -> GrantResult {
        Ok(())
    }

    fn validate_san(&self, _san: &str) -> GrantResult {
        Ok(())
    }

    fn notify_update_privilege(&self, users: &[UserIdentity]) -> GrantResult {
        self.notified.lock().unwrap().extend_from_slice(users);
        Ok(())
    }
}

fn user(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.to_owned(),
        hostname: "localhost".to_owned(),
        current_user: false,
    }
}

fn static_privilege(
    name: &str,
    global: bool,
    database: bool,
    table: bool,
    column: bool,
) -> StaticPrivilege {
    StaticPrivilege {
        name: name.to_owned(),
        column_name: format!("{name}_priv"),
        set_name: name.to_owned(),
        global,
        database,
        table,
        column,
        global_only: global && !database,
    }
}

fn executor(
    dependencies: Arc<MockDependencies>,
    level: GrantLevel,
    privileges: Vec<PrivilegeElement>,
) -> GrantExec {
    let select = static_privilege("Select", true, true, true, true);
    let insert = static_privilege("Insert", true, true, true, true);
    GrantExec {
        privileges,
        object_type: ObjectType::None,
        level,
        users: vec![UserSpec {
            user: user("alice"),
            auth_option: None,
        }],
        auth_token_or_tls_options: None,
        with_grant: false,
        done: false,
        catalog: PrivilegeCatalog {
            all_global: vec![select.clone(), insert.clone()],
            all_database: vec![select.clone(), insert.clone()],
            all_table: vec![select.clone(), insert.clone()],
            all_column: vec![select, insert],
        },
        dependencies,
    }
}

#[test]
/// 验证非法权限级别错误可分类展示，且全局 TLS 默认值为未指定且字段为空。
fn grant_error_class_and_global_tls_defaults_are_canonical() {
    let error = GrantError::with_kind(
        GrantErrorKind::IllegalPrivilegeLevel,
        "illegal privilege level",
    );
    assert_eq!(error.kind, GrantErrorKind::IllegalPrivilegeLevel);
    assert_eq!(error.to_string(), "illegal privilege level");
    let value = GlobalPrivValue::default();
    assert_eq!(value.ssl_type, SslType::NotSpecified);
    assert!(value.ssl_cipher.is_empty());
    assert!(value.x509_issuer.is_empty());
    assert!(value.x509_subject.is_empty());
    assert!(value.san.is_empty());
}

#[test]
fn grant_global_dynamic_and_tls_matches_go_mutations() {
    let state = Arc::new(Mutex::new(SessionState::default()));
    let dependencies = Arc::new(MockDependencies::new(Arc::clone(&state)));
    let mut grant = executor(
        Arc::clone(&dependencies),
        GrantLevel {
            level: GrantLevelKind::Global,
            db_name: String::new(),
            table_name: String::new(),
        },
        vec![PrivilegeElement {
            privilege: PrivilegeType::Extended("backup_admin".to_owned()),
            columns: Vec::new(),
        }],
    );
    grant.with_grant = true;
    grant.auth_token_or_tls_options = Some(vec![AuthTokenOrTlsOption {
        option_type: AuthTokenOrTlsOptionType::Cipher,
        value: "TLS_AES_128_GCM_SHA256".to_owned(),
    }]);

    grant.Next().unwrap();
    grant.Next().unwrap();

    let state = state.lock().unwrap();
    assert_eq!((state.began, state.committed, state.rolled_back), (1, 1, 0));
    assert!(state.mutations.iter().any(|mutation| matches!(
        mutation,
        SystemMutation::ReplaceDynamicGrant { privilege, with_grant_option: true, .. }
            if privilege == "BACKUP_ADMIN"
    )));
    assert!(state.mutations.iter().any(|mutation| matches!(
        mutation,
        SystemMutation::UpdateGlobalPriv { value: Some(value), .. }
            if value.contains("\"ssl_type\":3") && value.contains("TLS_AES_128_GCM_SHA256")
    )));
    assert_eq!(
        dependencies.notified.lock().unwrap().as_slice(),
        &[user("alice")]
    );
}

#[test]
fn grant_table_all_canonicalizes_names_and_expands_table_and_column_sets() {
    let state = Arc::new(Mutex::new(SessionState::default()));
    let dependencies = Arc::new(MockDependencies::new(Arc::clone(&state)));
    let mut grant = executor(
        dependencies,
        GrantLevel {
            level: GrantLevelKind::Table,
            db_name: "TEST".to_owned(),
            table_name: "T".to_owned(),
        },
        vec![PrivilegeElement {
            privilege: PrivilegeType::All,
            columns: Vec::new(),
        }],
    );

    grant.Next().unwrap();

    assert_eq!(grant.level.table_name, "t");
    let state = state.lock().unwrap();
    assert!(state.mutations.iter().any(|mutation| matches!(
        mutation,
        SystemMutation::InsertTablePriv { database, table, .. }
            if database == "test" && table == "t"
    )));
    assert!(state.mutations.iter().any(|mutation| matches!(
        mutation,
        SystemMutation::UpdateTableLevel {
            database,
            table,
            table_privileges,
            column_privileges,
            ..
        } if database == "test"
            && table == "t"
            && table_privileges == "Select,Insert"
            && column_privileges == "Select,Insert"
    )));
}

#[test]
fn grant_column_uses_canonical_column_and_rejects_non_column_privilege() {
    let state = Arc::new(Mutex::new(SessionState::default()));
    let dependencies = Arc::new(MockDependencies::new(Arc::clone(&state)));
    let column_privilege = static_privilege("Select", true, true, true, true);
    let mut grant = executor(
        Arc::clone(&dependencies),
        GrantLevel {
            level: GrantLevelKind::Table,
            db_name: "test".to_owned(),
            table_name: "t".to_owned(),
        },
        vec![PrivilegeElement {
            privilege: PrivilegeType::Static(column_privilege),
            columns: vec!["id".to_owned()],
        }],
    );
    grant.Next().unwrap();
    assert!(
        state
            .lock()
            .unwrap()
            .mutations
            .iter()
            .any(|mutation| matches!(
                mutation,
                SystemMutation::UpdateColumnLevel { column, column_privileges, .. }
                    if column == "ID" && column_privileges == "Select"
            ))
    );

    let non_column = static_privilege("Super", true, false, false, false);
    let mut invalid = executor(
        dependencies,
        GrantLevel {
            level: GrantLevelKind::Table,
            db_name: "test".to_owned(),
            table_name: "t".to_owned(),
        },
        vec![PrivilegeElement {
            privilege: PrivilegeType::Static(non_column),
            columns: vec!["id".to_owned()],
        }],
    );
    assert!(invalid.Next().unwrap_err().message.contains("COLUMN GRANT"));
}

#[test]
fn account_tls_require_serialization_matches_go_json() {
    use astersql_parser_ast::{AuthTokenOrTLSOption, AuthTokenOrTLSOptionType as Kind};
    let option = |kind, value: &str| AuthTokenOrTLSOption {
        Type: kind,
        Value: value.to_owned(),
    };
    let convert = crate::grant::account_tls_options_to_global_priv;
    assert_eq!(convert(&[]).unwrap(), Some("{}".to_owned()));
    assert_eq!(
        convert(&[option(Kind::TlsNone, "")]).unwrap(),
        Some("{}".to_owned())
    );
    assert_eq!(
        convert(&[option(Kind::Ssl, "")]).unwrap(),
        Some(r#"{"ssl_type":1}"#.to_owned())
    );
    assert_eq!(
        convert(&[option(Kind::X509, "")]).unwrap(),
        Some(r#"{"ssl_type":2}"#.to_owned())
    );
    assert_eq!(
        convert(&[option(Kind::TokenIssuer, "issuer-abc")]).unwrap(),
        None
    );
    assert_eq!(
        convert(&[
            option(Kind::Subject, "/C=US/O=Example/CN=TiDB"),
            option(Kind::SAN, "DNS:foo")
        ])
        .unwrap(),
        Some(
            r#"{"ssl_type":3,"x509_subject":"/C=US/O=Example/CN=TiDB","san":"DNS:foo"}"#.to_owned()
        ),
    );
}

#[test]
fn account_tls_require_validation_preserves_go_errors() {
    use astersql_parser_ast::{AuthTokenOrTLSOption, AuthTokenOrTLSOptionType as Kind};
    let option = |kind, value: &str| AuthTokenOrTLSOption {
        Type: kind,
        Value: value.to_owned(),
    };
    let convert = crate::grant::account_tls_options_to_global_priv;
    assert_eq!(
        convert(&[
            option(Kind::Subject, "/C=US"),
            option(Kind::Subject, "/C=SE")
        ])
        .unwrap_err()
        .to_string(),
        "Duplicate require SUBJECT clause",
    );
    assert!(convert(&[option(Kind::Subject, "/C=US=bad")]).is_err());
    assert!(convert(&[option(Kind::SAN, "EMAIL:client@example.com")]).is_err());
    assert_eq!(
        convert(&[option(Kind::Cipher, "not-a-cipher")])
            .unwrap_err()
            .to_string(),
        "Unsupported cipher suite: not-a-cipher",
    );
}
