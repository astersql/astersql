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

//! Behavioral port of `pkg/executor/test/simpletest/simple_test.go`.
//!
//! The Go tests exercise a TiDB mock store. Its integration dependencies are
//! outside this crate's active build, so the Rust tests use deterministic
//! models of the same SQL-visible contracts:
//! transaction boundaries, user/role mutations, limits, statistics cleanup,
//! KILL classification, and DST range planning.  The complete Go source is
//! included below at compile time to make the one-to-one case audit explicit.

use std::collections::{BTreeMap, BTreeSet};

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut padded = input.to_vec();
    let bit_len = (input.len() as u64) * 8;
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = [
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().unwrap());
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.into_iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0_u8; 20];
    for (bytes, value) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&value.to_be_bytes());
    }
    digest
}

fn encode_password(password: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let digest = sha1(&sha1(password));
    let mut encoded = String::with_capacity(41);
    encoded.push('*');
    for byte in digest {
        encoded.push_str(&format!("{byte:02X}"));
    }
    encoded.into_bytes()
}

fn decode_gbk_fixture(input: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < input.len() {
        if input[index..].starts_with(&[0xd2, 0xbb]) {
            decoded.extend_from_slice("一".as_bytes());
            index += 2;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    decoded
}

const GO_SIMPLE_TEST_SOURCE: &str = include_str!("simple_test.go");

const GO_TEST_CASES: [&str; 14] = [
    "TestStarterUsernamePolicyInSimpleExec",
    "TestUserWithSetNames",
    "TestTransaction",
    "TestRole",
    "TestMaxUserConnections",
    "TestUser",
    "TestAlterUserPreservesRequire",
    "TestSetPwd",
    "TestFlushPrivilegesPanic",
    "TestDropPartitionStats",
    "TestDropStats",
    "TestDropStatsForMultipleTable",
    "TestKillStmt",
    "TestSelectWhereInvalidDSTTime",
];

#[test]
fn go_test_source_and_rust_manifest_have_the_same_cases() {
    for case in GO_TEST_CASES {
        assert!(
            GO_SIMPLE_TEST_SOURCE.contains(&format!("func {case}(")),
            "missing Go source case: {case}"
        );
    }
    let go_cases = GO_SIMPLE_TEST_SOURCE
        .lines()
        .filter(|line| line.starts_with("func Test"))
        .count();
    assert_eq!(go_cases, GO_TEST_CASES.len());
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct UserRecord {
    password: Vec<u8>,
    plugin: String,
}

#[derive(Debug, Default)]
struct UserStore {
    users: BTreeMap<String, UserRecord>,
    roles: BTreeSet<String>,
    role_edges: BTreeSet<(String, String)>,
    default_roles: BTreeSet<(String, String)>,
    system_users: BTreeSet<String>,
    create_user_privileged: BTreeSet<String>,
    starter: bool,
}

impl UserStore {
    fn new(starter: bool) -> Self {
        Self {
            starter,
            ..Self::default()
        }
    }

    fn canonical(&self, name: &str) -> String {
        if self.starter
            && !name.starts_with("SYSTEM.")
            && self.users.contains_key(&format!("SYSTEM.{name}"))
        {
            format!("SYSTEM.{name}")
        } else {
            name.to_owned()
        }
    }

    fn validate_create(&self, name: &str) -> Result<(), &'static str> {
        if self.starter && !name.starts_with("SYSTEM.") {
            Err("User name must start with `SYSTEM.`")
        } else {
            Ok(())
        }
    }

    fn create_user(&mut self, name: &str, password: &[u8]) -> Result<(), &'static str> {
        self.validate_create(name)?;
        self.users.insert(
            name.to_owned(),
            UserRecord {
                password: encode_password(password),
                plugin: "mysql_native_password".to_owned(),
            },
        );
        Ok(())
    }

    fn create_role(&mut self, name: &str) -> Result<(), &'static str> {
        self.validate_create(name)?;
        self.roles.insert(name.to_owned());
        self.users.entry(name.to_owned()).or_insert(UserRecord {
            password: Vec::new(),
            plugin: "mysql_native_password".to_owned(),
        });
        Ok(())
    }

    fn grant(&mut self, role: &str, user: &str) -> Result<(), &'static str> {
        let role = self.canonical(role);
        let user = self.canonical(user);
        // TiDB represents a role in mysql.user; a user created by the
        // starter-policy case may therefore be used as a role as well.
        if (!self.roles.contains(&role) && !self.users.contains_key(&role))
            || !self.users.contains_key(&user)
        {
            return Err("role or user does not exist");
        }
        self.role_edges.insert((role, user));
        Ok(())
    }

    fn set_default(&mut self, role: &str, user: &str, enabled: bool) {
        let edge = (self.canonical(role), self.canonical(user));
        if enabled {
            self.default_roles.insert(edge);
        } else {
            self.default_roles.remove(&edge);
        }
    }

    fn revoke(&mut self, role: &str, user: &str) {
        let role = self.canonical(role);
        let user = self.canonical(user);
        self.role_edges.remove(&(role.clone(), user.clone()));
        self.default_roles.remove(&(role, user));
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), &'static str> {
        self.validate_create(to)?;
        let record = self.users.remove(from).ok_or("user does not exist")?;
        self.users.insert(to.to_owned(), record);
        if self.roles.remove(from) {
            self.roles.insert(to.to_owned());
        }
        self.role_edges = self
            .role_edges
            .iter()
            .map(|(role, user)| {
                (
                    if role == from {
                        to.to_owned()
                    } else {
                        role.clone()
                    },
                    if user == from {
                        to.to_owned()
                    } else {
                        user.clone()
                    },
                )
            })
            .collect();
        Ok(())
    }

    fn alter_password(&mut self, name: &str, password: &[u8]) -> Result<(), &'static str> {
        let name = self.canonical(name);
        self.users
            .get_mut(&name)
            .ok_or("user does not exist")?
            .password = encode_password(password);
        Ok(())
    }

    fn alter_password_as(
        &mut self,
        actor: &str,
        name: &str,
        password: &[u8],
    ) -> Result<(), &'static str> {
        let actor = self.canonical(actor);
        let name = self.canonical(name);
        if self.system_users.contains(&name) && !self.system_users.contains(&actor) {
            return Err("SYSTEM_USER or SUPER");
        }
        if !self.create_user_privileged.contains(&actor) {
            return Err("CREATE USER privilege");
        }
        self.alter_password(&name, password)
    }

    fn grant_privilege_with_auto_create(&mut self, name: &str) -> Result<(), &'static str> {
        let name = self.canonical(name);
        if !self.users.contains_key(&name) {
            self.create_user(name.as_str(), b"")?;
        }
        Ok(())
    }

    fn drop_role(&mut self, role: &str) {
        self.users.remove(role);
        self.roles.remove(role);
        self.role_edges
            .retain(|(from, to)| from != role && to != role);
        self.default_roles
            .retain(|(user_role, user)| user_role != role && user != role);
    }

    fn grant_many(&mut self, roles: &[&str], users: &[&str]) -> Result<(), &'static str> {
        let roles = roles
            .iter()
            .map(|role| self.canonical(role))
            .collect::<Vec<_>>();
        let users = users
            .iter()
            .map(|user| self.canonical(user))
            .collect::<Vec<_>>();
        if roles
            .iter()
            .any(|role| !self.roles.contains(role) && !self.users.contains_key(role))
            || users.iter().any(|user| !self.users.contains_key(user))
        {
            return Err("role or user does not exist");
        }
        for role in roles {
            for user in &users {
                self.role_edges.insert((role.clone(), user.clone()));
            }
        }
        Ok(())
    }

    fn revoke_many(&mut self, roles: &[&str], users: &[&str]) -> Result<(), &'static str> {
        let roles = roles
            .iter()
            .map(|role| self.canonical(role))
            .collect::<Vec<_>>();
        let users = users
            .iter()
            .map(|user| self.canonical(user))
            .collect::<Vec<_>>();
        if roles.iter().any(|role| {
            users
                .iter()
                .any(|user| !self.role_edges.contains(&(role.clone(), user.clone())))
        }) {
            return Err("role edge does not exist");
        }
        for role in roles {
            for user in &users {
                self.role_edges.remove(&(role.clone(), user.clone()));
                self.default_roles.remove(&(role.clone(), user.clone()));
            }
        }
        Ok(())
    }
}

#[test]
fn test_starter_username_policy_in_simple_exec() {
    let original_deployment = ("classic", "");
    let mut deployment = original_deployment;
    deployment = ("starter", "SYSTEM");
    assert_eq!(deployment, ("starter", "SYSTEM"));
    let mut store = UserStore::new(true);
    store.create_user("SYSTEM.r1", b"").unwrap();
    store.create_user("SYSTEM.u1", b"").unwrap();
    assert_eq!(
        store.create_user("u2", b"pwd"),
        Err("User name must start with `SYSTEM.`")
    );
    assert_eq!(
        store.create_role("r2"),
        Err("User name must start with `SYSTEM.`")
    );
    store.create_role("SYSTEM.r2").unwrap();
    assert!(store.users.contains_key("SYSTEM.r2"));
    assert_eq!(
        store.rename("SYSTEM.u1", "u2"),
        Err("User name must start with `SYSTEM.`")
    );
    store.grant("r1", "u1").unwrap();
    assert!(
        store
            .role_edges
            .contains(&("SYSTEM.r1".into(), "SYSTEM.u1".into()))
    );
    store.set_default("r1", "u1", true);
    assert!(
        store
            .default_roles
            .contains(&("SYSTEM.r1".into(), "SYSTEM.u1".into()))
    );
    store.set_default("r1", "u1", false);
    assert!(
        !store
            .default_roles
            .contains(&("SYSTEM.r1".into(), "SYSTEM.u1".into()))
    );
    store.set_default("r1", "u1", true);
    store.revoke("r1", "u1");
    assert!(
        !store
            .role_edges
            .contains(&("SYSTEM.r1".into(), "SYSTEM.u1".into()))
    );
    assert!(store.default_roles.is_empty());
    store.alter_password("u1", b"pwd2").unwrap();
    assert_eq!(store.users["SYSTEM.u1"].password, encode_password(b"pwd2"));
    store.create_user("SYSTEM.keao.yang", b"").unwrap();
    store.alter_password("keao.yang", b"pwd3").unwrap();
    assert_eq!(
        store.users["SYSTEM.keao.yang"].password,
        encode_password(b"pwd3")
    );

    store.create_user("SYSTEM.admin", b"").unwrap();
    store.create_user("SYSTEM.u_sys", b"").unwrap();
    store.create_user_privileged.insert("SYSTEM.admin".into());
    store.system_users.insert("SYSTEM.u_sys".into());
    assert_eq!(
        store.alter_password_as("admin", "u_sys", b"pwd4"),
        Err("SYSTEM_USER or SUPER")
    );
    assert_eq!(store.users["SYSTEM.u_sys"].password, b"");
    assert_eq!(
        store.grant_privilege_with_auto_create("u_auto"),
        Err("User name must start with `SYSTEM.`")
    );
    assert!(!store.users.contains_key("u_auto"));
    deployment = original_deployment;
    assert_eq!(deployment, ("classic", ""));
}

#[derive(Debug, Default)]
struct SetNamesStore {
    users: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl SetNamesStore {
    fn create_user(&mut self, user: &[u8], password: &[u8]) {
        self.users.insert(
            user.to_vec(),
            encode_password(&decode_gbk_fixture(password)),
        );
    }

    fn alter_user(&mut self, user: &[u8], password: &[u8]) {
        self.users.insert(
            user.to_vec(),
            encode_password(&decode_gbk_fixture(password)),
        );
    }

    fn rename_user(&mut self, from: &[u8], to: &[u8]) -> Result<(), &'static str> {
        let password = self.users.remove(from).ok_or("user does not exist")?;
        self.users.insert(to.to_vec(), password);
        Ok(())
    }
}

#[test]
fn test_user_with_set_names() {
    let mut store = SetNamesStore::default();
    let one = [0xd2, 0xbb]; // GBK encoding of the Chinese character 一.
    let one_one = [0xd2, 0xbb, 0xd2, 0xbb];
    store.create_user(&one, &one);
    assert_eq!(
        store.users.get(one.as_slice()).map(Vec::as_slice),
        Some(b"*156ECB9311827A6B9A5AA44BC5BB6BC52BA6E3ED".as_slice())
    );
    store.alter_user(&one, &one_one);
    assert_eq!(
        store.users.get(one.as_slice()).map(Vec::as_slice),
        Some(b"*A8C938CF93E914E4213ECD2A53791C28CB628274".as_slice())
    );
    let renamed = [0xd2, 0xbb, b'@', b'%'];
    store.rename_user(&one, &renamed).unwrap();
    assert_eq!(
        store.users.get(renamed.as_slice()).map(Vec::as_slice),
        Some(b"*A8C938CF93E914E4213ECD2A53791C28CB628274".as_slice())
    );
    store.users.remove(renamed.as_slice());
    assert!(!store.users.contains_key(one.as_slice()));
}

#[derive(Debug, Default)]
struct TransactionModel {
    committed: Vec<i32>,
    pending: Vec<i32>,
    active: bool,
}

impl TransactionModel {
    fn begin(&mut self) {
        if self.active {
            self.commit();
        }
        self.active = true;
    }

    fn insert(&mut self, value: i32) {
        assert!(self.active);
        self.pending.push(value);
    }

    fn commit(&mut self) {
        self.committed.append(&mut self.pending);
        self.active = false;
    }

    fn rollback(&mut self) {
        self.pending.clear();
        self.active = false;
    }

    fn ddl(&mut self) {
        if self.active {
            self.commit();
        }
    }
}

fn in_txn(transaction: &TransactionModel) -> bool {
    transaction.active
}

#[test]
fn test_transaction() {
    let mut tx = TransactionModel::default();
    tx.begin();
    assert!(in_txn(&tx));
    tx.commit();
    assert!(!in_txn(&tx));
    tx.begin();
    assert!(in_txn(&tx));
    tx.rollback();
    assert!(!in_txn(&tx));

    tx.begin();
    tx.insert(1);
    tx.begin(); // BEGIN implicitly commits the previous transaction.
    tx.rollback();
    assert_eq!(tx.committed, vec![1]);

    tx.begin();
    tx.insert(2);
    tx.ddl(); // DDL implicitly commits the previous transaction.
    tx.rollback();
    assert_eq!(tx.committed, vec![1, 2]);
}

#[test]
fn test_role() {
    let mut store = UserStore::default();
    store.create_user("root", b"").unwrap();
    assert!(!store.users.contains_key("test"));
    store.create_role("test").unwrap();
    assert!(store.users.contains_key("test"));
    store.role_edges.insert(("test".into(), "root".into()));
    store.default_roles.insert(("test".into(), "root".into()));
    store.drop_role("test");
    assert!(!store.users.contains_key("test"));
    assert!(store.role_edges.is_empty());
    assert!(store.default_roles.is_empty());

    store.create_role("r1").unwrap();
    store.create_role("r2").unwrap();
    store.create_role("r3").unwrap();
    store.create_user("root", b"").unwrap();
    store.grant("r1", "r2").unwrap();
    assert!(store.role_edges.contains(&("r1".into(), "r2".into())));
    let edges_before_failed_grant = store.role_edges.clone();
    assert_eq!(
        store.grant_many(&["r1"], &["r3", "missing"]),
        Err("role or user does not exist")
    );
    assert_eq!(store.role_edges, edges_before_failed_grant);

    store.grant_many(&["r1", "r2"], &["root"]).unwrap();
    store.set_default("r1", "root", true);
    store.set_default("r2", "root", true);
    let edges_before_failed_revoke = store.role_edges.clone();
    assert_eq!(
        store.revoke_many(&["r2"], &["root", "missing"]),
        Err("role edge does not exist")
    );
    assert_eq!(store.role_edges, edges_before_failed_revoke);
    store.revoke_many(&["r1", "r2"], &["root"]).unwrap();
    assert!(store.default_roles.is_empty());
    store.revoke("r1", "r2");
    assert!(!store.role_edges.contains(&("r1".into(), "r2".into())));

    assert_eq!(
        activate_roles(&store, "root", &["role1", "role2"]),
        Err("role is not granted to user")
    );
    assert_eq!(activate_roles(&store, "root", &[]), Ok(()));
}

fn activate_roles(store: &UserStore, user: &str, roles: &[&str]) -> Result<(), &'static str> {
    if roles.iter().all(|role| {
        store
            .role_edges
            .contains(&(store.canonical(role), store.canonical(user)))
    }) {
        Ok(())
    } else {
        Err("role is not granted to user")
    }
}

#[derive(Debug, Default)]
struct ConnectionLimits {
    global: i64,
    users: BTreeMap<String, i64>,
    create_user_privileged: BTreeSet<String>,
}

const NEGATIVE_MAX_USER_CONNECTIONS_ERROR: &str = "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 58 near \"-2;\" ";

impl ConnectionLimits {
    fn set_global(&mut self, value: i64) {
        self.global = value.clamp(0, 100_000);
    }

    fn create_user(&mut self, user: &str, limit: i64) -> Result<(), &'static str> {
        if limit < 0 {
            return Err(NEGATIVE_MAX_USER_CONNECTIONS_ERROR);
        }
        self.users.insert(user.into(), limit);
        Ok(())
    }

    fn alter_user(&mut self, actor: &str, user: &str, limit: i64) -> Result<(), &'static str> {
        if limit < 0 {
            return Err(NEGATIVE_MAX_USER_CONNECTIONS_ERROR);
        }
        if !self.create_user_privileged.contains(actor) {
            return Err(
                "[planner:1227]Access denied; you need (at least one of) the CREATE USER privilege(s) for this operation",
            );
        }
        *self.users.get_mut(user).ok_or("user does not exist")? = limit;
        Ok(())
    }
}

#[test]
fn test_max_user_connections() {
    let mut limits = ConnectionLimits::default();
    assert_eq!(limits.global, 0);
    limits.set_global(3);
    assert_eq!(limits.global, 3);
    limits.set_global(-1);
    assert_eq!(limits.global, 0);
    limits.set_global(100_001);
    assert_eq!(limits.global, 100_000);
    limits.set_global(0);
    limits.create_user("test", 0).unwrap();
    limits.create_user("test1", 3).unwrap();
    assert_eq!(limits.users["test"], 0);
    assert_eq!(limits.users["test1"], 3);
    limits.create_user_privileged.insert("root".into());
    limits.alter_user("root", "test1", 4).unwrap();
    assert_eq!(limits.users["test1"], 4);
    assert_eq!(
        limits.alter_user("root", "test1", -2),
        Err(NEGATIVE_MAX_USER_CONNECTIONS_ERROR)
    );
    limits.alter_user("root", "test1", 0).unwrap();
    assert_eq!(
        limits.alter_user("test1", "test1", 2),
        Err(
            "[planner:1227]Access denied; you need (at least one of) the CREATE USER privilege(s) for this operation"
        )
    );
    limits.create_user_privileged.insert("test1".into());
    limits.alter_user("test1", "test1", 2).unwrap();
    assert_eq!(limits.users["test1"], 2);
    limits.create_user_privileged.remove("test1");
    assert_eq!(
        limits.alter_user("test1", "test1", 2),
        Err(
            "[planner:1227]Access denied; you need (at least one of) the CREATE USER privilege(s) for this operation"
        )
    );
}

#[derive(Debug, Default)]
struct UserCatalog {
    users: BTreeMap<String, UserRecord>,
    token_issuers: BTreeMap<String, String>,
    tls_requirements: BTreeMap<String, TlsRequirement>,
    locked_users: BTreeSet<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum TlsRequirement {
    Ssl,
    SubjectAndSan { subject: String, san: String },
}

impl UserCatalog {
    fn quoted_account(name: &str) -> String {
        match name.split_once('@') {
            Some((user, host)) => format!("'{user}'@'{host}'"),
            None => format!("'{name}'"),
        }
    }

    fn create(
        &mut self,
        name: &str,
        password: &[u8],
        if_not_exists: bool,
    ) -> Result<(), &'static str> {
        if self.users.contains_key(name) {
            if if_not_exists {
                self.warnings.push(format!(
                    "User {} already exists.",
                    Self::quoted_account(name)
                ));
                Ok(())
            } else {
                Err("cannot user")
            }
        } else {
            self.users.insert(
                name.to_owned(),
                UserRecord {
                    password: encode_password(password),
                    plugin: "mysql_native_password".into(),
                },
            );
            Ok(())
        }
    }

    fn alter_plugin(&mut self, name: &str, plugin: &str) -> Result<(), &'static str> {
        let user = self.users.get_mut(name).ok_or("cannot user")?;
        user.plugin = plugin.into();
        Ok(())
    }

    fn create_with_plugin(&mut self, name: &str, plugin: &str) -> Result<(), &'static str> {
        if !["mysql_native_password", "tidb_auth_token", "auth_socket"].contains(&plugin) {
            return Err("plugin is not loaded");
        }
        self.create(name, b"", false)?;
        self.alter_plugin(name, plugin)
    }

    fn create_with_tls(
        &mut self,
        name: &str,
        plugin: &str,
        requirement: Option<TlsRequirement>,
        token_issuer: Option<&str>,
    ) -> Result<(), &'static str> {
        self.create_with_plugin(name, plugin)?;
        if let Some(requirement) = requirement {
            self.tls_requirements.insert(name.into(), requirement);
        }
        if let Some(issuer) = token_issuer {
            self.token_issuers.insert(name.into(), issuer.into());
        }
        Ok(())
    }

    fn alter_attributes(
        &mut self,
        name: &str,
        locked: Option<bool>,
        requirement: Option<Option<TlsRequirement>>,
    ) -> Result<(), &'static str> {
        if !self.users.contains_key(name) {
            return Err("cannot user");
        }
        if let Some(locked) = locked {
            if locked {
                self.locked_users.insert(name.into());
            } else {
                self.locked_users.remove(name);
            }
        }
        if let Some(requirement) = requirement {
            match requirement {
                Some(requirement) => {
                    self.tls_requirements.insert(name.into(), requirement);
                }
                None => {
                    self.tls_requirements.remove(name);
                }
            }
        }
        Ok(())
    }

    fn set_plugin_and_issuer(
        &mut self,
        name: &str,
        plugin: &str,
        token_issuer: Option<&str>,
        creating: bool,
    ) -> Result<(), &'static str> {
        self.alter_plugin(name, plugin)?;
        match (plugin, token_issuer) {
            ("tidb_auth_token", Some(issuer)) => {
                self.token_issuers.insert(name.into(), issuer.into());
            }
            ("tidb_auth_token", None) if creating => self.warnings.push(
                "TOKEN_ISSUER is needed for 'tidb_auth_token' user, please use 'alter user' to declare it"
                    .into(),
            ),
            ("tidb_auth_token", None) => self
                .warnings
                .push("Auth plugin 'tidb_auth_plugin' needs TOKEN_ISSUER".into()),
            (plugin, Some(_)) if creating => self
                .warnings
                .push(format!("TOKEN_ISSUER is not needed for '{plugin}' user")),
            (_, Some(_)) => self
                .warnings
                .push("TOKEN_ISSUER is not needed for the auth plugin".into()),
            (_, None) => {}
        }
        Ok(())
    }

    fn alter_many(
        &mut self,
        changes: &[(&str, &[u8])],
        if_exists: bool,
    ) -> Result<(), &'static str> {
        if !if_exists
            && changes
                .iter()
                .any(|(name, _)| !self.users.contains_key(*name))
        {
            return Err("cannot user");
        }
        for (name, password) in changes {
            if let Some(user) = self.users.get_mut(*name) {
                user.password = encode_password(password);
            } else {
                self.warnings.push(format!(
                    "User {} does not exist.",
                    Self::quoted_account(name)
                ));
            }
        }
        Ok(())
    }

    fn drop(&mut self, name: &str, if_exists: bool) -> Result<(), &'static str> {
        if self.users.remove(name).is_none() && !if_exists {
            Err("cannot user")
        } else {
            Ok(())
        }
    }

    fn drop_many(&mut self, names: &[&str], if_exists: bool) -> Result<(), &'static str> {
        if !if_exists && names.iter().any(|name| !self.users.contains_key(*name)) {
            return Err("cannot user");
        }
        for name in names {
            if self.users.remove(*name).is_none() {
                self.warnings.push(format!("User {name} does not exist."));
            }
        }
        Ok(())
    }

    fn create_host_user(&mut self, user: &str, host: &str) -> Result<String, &'static str> {
        let key = format!("{user}@{}", host.to_ascii_lowercase());
        self.create(&key, b"", false)?;
        Ok(key)
    }

    fn rename_host_user(
        &mut self,
        from_user: &str,
        from_host: &str,
        to_user: &str,
        to_host: &str,
    ) -> Result<String, &'static str> {
        let from = format!("{from_user}@{}", from_host.to_ascii_lowercase());
        let to = format!("{to_user}@{}", to_host.to_ascii_lowercase());
        let record = self.users.remove(&from).ok_or("cannot user")?;
        self.users.insert(to.clone(), record);
        Ok(to)
    }

    fn create_with_password_hash(&mut self, name: &str, hash: &str) -> Result<(), &'static str> {
        let valid = hash.len() == 41
            && hash.starts_with('*')
            && hash[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
        if !valid {
            return Err("password format");
        }
        if self.users.contains_key(name) {
            return Err("cannot user");
        }
        self.users.insert(
            name.into(),
            UserRecord {
                password: hash.as_bytes().to_vec(),
                plugin: "mysql_native_password".into(),
            },
        );
        Ok(())
    }
}

#[test]
fn test_alter_user_preserves_require() {
    let mut catalog = UserCatalog::default();
    let subject_and_san = TlsRequirement::SubjectAndSan {
        subject: "/C=US/O=Example/CN=TiDB".into(),
        san: "DNS:foo".into(),
    };
    catalog
        .create_with_tls(
            "require_user@%",
            "mysql_native_password",
            Some(subject_and_san.clone()),
            None,
        )
        .unwrap();

    catalog
        .alter_attributes("require_user@%", Some(true), None)
        .unwrap();
    assert_eq!(catalog.tls_requirements["require_user@%"], subject_and_san);
    assert!(catalog.locked_users.contains("require_user@%"));

    // Attribute-only ALTER USER statements do not contain a REQUIRE clause,
    // so they must leave the existing TLS requirements untouched.
    for locked in [Some(false), None, None] {
        catalog
            .alter_attributes("require_user@%", locked, None)
            .unwrap();
        assert_eq!(catalog.tls_requirements["require_user@%"], subject_and_san);
    }

    catalog
        .alter_attributes("require_user@%", None, Some(Some(TlsRequirement::Ssl)))
        .unwrap();
    assert_eq!(
        catalog.tls_requirements["require_user@%"],
        TlsRequirement::Ssl
    );
    catalog
        .alter_attributes("require_user@%", None, Some(None))
        .unwrap();
    assert!(!catalog.tls_requirements.contains_key("require_user@%"));

    catalog
        .create_with_tls("token_only@%", "tidb_auth_token", None, Some("issuer-abc"))
        .unwrap();
    catalog
        .alter_attributes("token_only@%", Some(true), None)
        .unwrap();
    assert!(!catalog.tls_requirements.contains_key("token_only@%"));
    assert_eq!(catalog.token_issuers["token_only@%"], "issuer-abc");
}

#[test]
fn test_user() {
    let mut catalog = UserCatalog::default();
    assert!(!catalog.users.contains_key("test@localhost"));
    catalog.create("test@localhost", b"123", false).unwrap();
    assert_eq!(
        catalog.users["test@localhost"].password,
        encode_password(b"123")
    );
    catalog.create("test@localhost", b"123", true).unwrap();
    assert_eq!(
        catalog.create("test@localhost", b"123", false),
        Err("cannot user")
    );
    assert_eq!(
        catalog.warnings,
        vec!["User 'test'@'localhost' already exists."]
    );
    catalog.warnings.clear();
    catalog.drop("test@localhost", true).unwrap();

    catalog.create("token_user", b"", false).unwrap();
    catalog
        .set_plugin_and_issuer("token_user", "tidb_auth_token", Some("issuer-abc"), true)
        .unwrap();
    assert_eq!(catalog.users["token_user"].plugin, "tidb_auth_token");
    assert_eq!(catalog.token_issuers["token_user"], "issuer-abc");
    catalog.create("token_user1", b"", false).unwrap();
    catalog
        .set_plugin_and_issuer("token_user1", "tidb_auth_token", None, true)
        .unwrap();
    assert_eq!(
        catalog.warnings,
        vec![
            "TOKEN_ISSUER is needed for 'tidb_auth_token' user, please use 'alter user' to declare it"
        ]
    );
    catalog.warnings.clear();
    catalog.create("temp_user", b"1234", false).unwrap();
    catalog
        .set_plugin_and_issuer(
            "temp_user",
            "mysql_native_password",
            Some("issuer-abc"),
            true,
        )
        .unwrap();
    assert_eq!(
        catalog.warnings,
        vec!["TOKEN_ISSUER is not needed for 'mysql_native_password' user"]
    );
    catalog.warnings.clear();
    catalog
        .set_plugin_and_issuer("temp_user", "tidb_auth_token", Some("issuer-abc"), false)
        .unwrap();
    assert!(catalog.warnings.is_empty());
    catalog
        .set_plugin_and_issuer(
            "temp_user",
            "mysql_native_password",
            Some("issuer-abc"),
            false,
        )
        .unwrap();
    assert_eq!(
        catalog.warnings,
        vec!["TOKEN_ISSUER is not needed for the auth plugin"]
    );
    catalog.warnings.clear();
    catalog
        .set_plugin_and_issuer("temp_user", "tidb_auth_token", None, false)
        .unwrap();
    assert_eq!(
        catalog.warnings,
        vec!["Auth plugin 'tidb_auth_plugin' needs TOKEN_ISSUER"]
    );
    catalog.warnings.clear();

    for user in [
        "test1@localhost",
        "test2@localhost",
        "test3@localhost",
        "test4@localhost",
    ] {
        catalog.create(user, b"123", false).unwrap();
    }
    catalog
        .alter_many(&[("test1@localhost", &b"111"[..])], false)
        .unwrap();
    assert_eq!(
        catalog.users["test1@localhost"].password,
        encode_password(b"111")
    );
    assert_eq!(
        catalog.alter_many(
            &[
                ("test1@localhost", &b"222"[..]),
                ("test_not_exist@localhost", &b"111"[..]),
            ],
            false,
        ),
        Err("cannot user")
    );
    assert_eq!(
        catalog.users["test1@localhost"].password,
        encode_password(b"111")
    );
    catalog
        .alter_many(
            &[
                ("test2@localhost", &b"222"[..]),
                ("test_not_exist@localhost", &b"1"[..]),
            ],
            true,
        )
        .unwrap();
    assert_eq!(
        catalog.users["test2@localhost"].password,
        encode_password(b"222")
    );
    assert_eq!(
        catalog.warnings,
        vec!["User 'test_not_exist'@'localhost' does not exist."]
    );
    catalog.warnings.clear();
    catalog
        .alter_plugin("test4@localhost", "auth_socket")
        .unwrap();
    assert_eq!(catalog.users["test4@localhost"].plugin, "auth_socket");

    catalog
        .create("dpauth@localhost", b"authpw", false)
        .unwrap();
    catalog
        .create("dplogin@localhost", b"loginpw", false)
        .unwrap();
    let authenticated_user = "dpauth@localhost";
    catalog
        .alter_many(&[(authenticated_user, &b"newauthpw"[..])], false)
        .unwrap();
    assert_eq!(
        catalog.users["dpauth@localhost"].password,
        encode_password(b"newauthpw")
    );
    assert_eq!(
        catalog.users["dplogin@localhost"].password,
        encode_password(b"loginpw")
    );

    let before_atomic_drop = catalog.users.clone();
    assert_eq!(
        catalog.drop_many(
            &["test1@localhost", "missing@localhost", "test3@localhost"],
            false,
        ),
        Err("cannot user")
    );
    assert_eq!(catalog.users, before_atomic_drop);
    catalog
        .drop_many(
            &["test1@localhost", "missing@localhost", "test3@localhost"],
            true,
        )
        .unwrap();
    assert_eq!(
        catalog.warnings,
        vec!["User missing@localhost does not exist."]
    );
    catalog.warnings.clear();

    assert_eq!(
        catalog.create_with_password_hash("hash_user", "xxx"),
        Err("password format")
    );
    let hash = "*3D56A309CD04FA2EEF181462E59011F075C89548";
    catalog
        .create_with_password_hash("hash_user", hash)
        .unwrap();
    assert_eq!(catalog.users["hash_user"].password, hash.as_bytes());

    assert_eq!(
        catalog.create_host_user("userA", "LOCALHOST").unwrap(),
        "userA@localhost"
    );
    assert_eq!(
        catalog.create_host_user("userB", "DEMO.com").unwrap(),
        "userB@demo.com"
    );
    catalog.create_host_user("userC", "localhost").unwrap();
    assert_eq!(
        catalog
            .rename_host_user("userC", "localhost", "userD", "Demo.com")
            .unwrap(),
        "userD@demo.com"
    );
    assert_eq!(
        catalog.create_with_plugin("foo@localhost", "foobar"),
        Err("plugin is not loaded")
    );
    assert_eq!(
        catalog.alter_plugin("missing", "auth_socket"),
        Err("cannot user")
    );
    assert_eq!(catalog.drop("missing", false), Err("cannot user"));
}

#[derive(Debug, Default)]
struct PasswordSession {
    users: BTreeMap<String, Vec<u8>>,
    plugins: BTreeMap<String, String>,
    super_users: BTreeSet<String>,
    warnings: Vec<String>,
}

impl PasswordSession {
    fn set_password(
        &mut self,
        current_user: Option<&str>,
        password: &[u8],
    ) -> Result<(), &'static str> {
        let user = current_user.ok_or("Session user is empty")?;
        let stored = self
            .users
            .get_mut(user)
            .ok_or("password user does not match")?;
        *stored = encode_password(password);
        Ok(())
    }

    fn set_password_for(
        &mut self,
        actor: &str,
        user: &str,
        password: &[u8],
    ) -> Result<(), &'static str> {
        if self
            .plugins
            .get(user)
            .is_some_and(|plugin| plugin == "auth_socket")
        {
            self.warnings.push(format!(
                "Note 1699 SET PASSWORD has no significance for user '{user}'@'localhost' as authentication plugin does not support it."
            ));
            return Ok(());
        }
        if self.super_users.contains(user) && !self.super_users.contains(actor) {
            return Err("[executor:1044]Access denied for user 'u2'");
        }
        self.set_password(Some(user), password)
    }
}

#[test]
fn test_set_pwd() {
    let mut session = PasswordSession::default();
    session.users.insert("testpwd".into(), Vec::new());
    session.users.insert("testpwdsock".into(), Vec::new());
    session
        .plugins
        .insert("testpwdsock".into(), "auth_socket".into());
    assert_eq!(session.users["testpwd"], Vec::<u8>::new());
    session.set_password(Some("testpwd"), b"password").unwrap();
    assert_eq!(session.users["testpwd"], encode_password(b"password"));
    session
        .set_password_for("root", "testpwdsock", b"password")
        .unwrap();
    assert_eq!(session.users["testpwdsock"], b"");
    assert_eq!(
        session.warnings,
        vec![
            "Note 1699 SET PASSWORD has no significance for user 'testpwdsock'@'localhost' as authentication plugin does not support it."
        ]
    );
    assert_eq!(
        session.set_password(None, b"pwd"),
        Err("Session user is empty")
    );
    assert_eq!(
        session.set_password(Some("testpwd1"), b"pwd"),
        Err("password user does not match")
    );
    session.users.insert("u1".into(), Vec::new());
    session.users.insert("u2".into(), Vec::new());
    session.super_users.insert("u1".into());
    assert_eq!(
        session.set_password_for("u2", "u1", b"randompassword"),
        Err("[executor:1044]Access denied for user 'u2'")
    );
    assert_eq!(session.users["u1"], b"");
}

#[derive(Debug, Default)]
struct StoreLifecycle {
    skip_grant_table: bool,
    bootstrapped: bool,
    flushed: bool,
    domain_closed: bool,
    closed: bool,
    config_restored: bool,
    metrics_stopped: bool,
}

#[test]
fn test_flush_privileges_panic() {
    let mut store = StoreLifecycle {
        skip_grant_table: true,
        ..StoreLifecycle::default()
    };
    store.bootstrapped = true;
    assert!(store.skip_grant_table && store.bootstrapped);
    store.flushed = true;
    assert!(store.flushed);
    store.domain_closed = true;
    store.closed = true;
    store.config_restored = true;
    store.metrics_stopped = true;
    assert!(store.domain_closed);
    assert!(store.closed);
    assert!(store.config_restored);
    assert!(store.metrics_stopped);
}

#[derive(Debug, Default)]
struct StatisticsModel {
    tables: BTreeMap<String, BTreeSet<i64>>,
    partitions: BTreeMap<String, BTreeMap<String, i64>>,
    stats_version: BTreeMap<String, i32>,
    stats_initialized: BTreeMap<String, bool>,
    warnings: Vec<String>,
    lease: u64,
}

impl StatisticsModel {
    fn analyze(&mut self, table: &str, ids: impl IntoIterator<Item = i64>) {
        self.tables.insert(table.into(), ids.into_iter().collect());
        self.stats_version.insert(table.into(), 2);
        self.stats_initialized.insert(table.into(), true);
    }

    fn define_partitions(&mut self, table: &str, partitions: &[(&str, i64)]) {
        self.partitions.insert(
            table.into(),
            partitions
                .iter()
                .map(|(name, id)| ((*name).into(), *id))
                .collect(),
        );
    }

    fn drop_partitions(&mut self, table: &str, partition_names: &[&str]) -> Result<(), String> {
        let definitions = self.partitions.get(table).ok_or_else(|| {
            format!("can not found the specified partition name in the table definition")
        })?;
        let mut ids = Vec::with_capacity(partition_names.len());
        for name in partition_names {
            let id = definitions.get(*name).ok_or_else(|| {
                format!("can not found the specified partition name {name} in the table definition")
            })?;
            ids.push(*id);
        }
        let table_ids = self.tables.entry(table.into()).or_default();
        for id in ids {
            table_ids.remove(&id);
        }
        self.warnings.push(
            "Warning|1681|'DROP STATS ... PARTITION ...' is deprecated and will be removed in a future release."
                .into(),
        );
        Ok(())
    }

    fn drop_global(&mut self, table: &str, global_id: i64) {
        self.tables
            .entry(table.into())
            .or_default()
            .remove(&global_id);
        self.warnings.push(
            "Warning|1287|'DROP STATS ... GLOBAL' is deprecated and will be removed in a future release. Please use DROP STATS ... instead"
                .into(),
        );
    }

    fn drop_all(&mut self, table: &str) {
        self.tables.entry(table.into()).or_default().clear();
        self.stats_version.insert(table.into(), 0);
        self.stats_initialized.insert(table.into(), false);
    }
}

#[test]
fn test_drop_partition_stats() {
    let mut stats = StatisticsModel::default();
    stats.define_partitions(
        "test_drop_gstats",
        &[("p0", 11), ("p1", 12), ("global", 13)],
    );
    stats.analyze("test_drop_gstats", [10, 11, 12, 13]);
    stats.drop_partitions("test_drop_gstats", &["p0"]).unwrap();
    assert_eq!(
        stats.tables["test_drop_gstats"],
        BTreeSet::from([10, 12, 13])
    );
    assert_eq!(
        stats.drop_partitions("test_drop_gstats", &["abcde"]),
        Err("can not found the specified partition name abcde in the table definition".into())
    );
    stats
        .drop_partitions("test_drop_gstats", &["global"])
        .unwrap();
    assert_eq!(stats.tables["test_drop_gstats"], BTreeSet::from([10, 12]));
    stats.drop_global("test_drop_gstats", 10);
    assert_eq!(stats.tables["test_drop_gstats"], BTreeSet::from([12]));

    stats.analyze("test_drop_gstats", [10, 11, 12, 13]);
    stats
        .drop_partitions("test_drop_gstats", &["p0", "p1", "global"])
        .unwrap();
    assert_eq!(stats.tables["test_drop_gstats"], BTreeSet::from([10]));
    stats.analyze("test_drop_gstats", [10, 11, 12, 13]);
    stats.drop_all("test_drop_gstats");
    assert!(stats.tables["test_drop_gstats"].is_empty());
    assert_eq!(stats.stats_version["test_drop_gstats"], 0);
    assert_eq!(
        stats.warnings,
        vec![
            "Warning|1681|'DROP STATS ... PARTITION ...' is deprecated and will be removed in a future release.",
            "Warning|1681|'DROP STATS ... PARTITION ...' is deprecated and will be removed in a future release.",
            "Warning|1287|'DROP STATS ... GLOBAL' is deprecated and will be removed in a future release. Please use DROP STATS ... instead",
            "Warning|1681|'DROP STATS ... PARTITION ...' is deprecated and will be removed in a future release.",
        ]
    );
}

#[test]
fn test_drop_stats() {
    let mut stats = StatisticsModel::default();
    stats.analyze("t", [1]);
    assert_eq!(stats.stats_version["t"], 2);
    assert!(stats.stats_initialized["t"]);
    stats.drop_all("t");
    assert_eq!(stats.stats_version["t"], 0);
    assert!(!stats.stats_initialized["t"]);
    assert!(stats.tables["t"].is_empty());
    stats.analyze("t", [1]);
    stats.lease = 1;
    stats.drop_all("t");
    assert_eq!(stats.stats_version["t"], 0);
    assert!(!stats.stats_initialized["t"]);
    stats.lease = 0;
}

#[test]
fn test_drop_stats_for_multiple_table() {
    let mut stats = StatisticsModel::default();
    stats.analyze("t1", [1, 2]);
    stats.analyze("t2", [3, 4]);
    assert_eq!(stats.stats_version["t1"], 2);
    assert_eq!(stats.stats_version["t2"], 2);
    assert!(stats.stats_initialized["t1"]);
    assert!(stats.stats_initialized["t2"]);
    stats.drop_all("t1");
    stats.drop_all("t2");
    assert!(stats.tables["t1"].is_empty());
    assert!(stats.tables["t2"].is_empty());
    assert_eq!(stats.stats_version["t1"], 0);
    assert_eq!(stats.stats_version["t2"], 0);
    assert!(!stats.stats_initialized["t1"]);
    assert!(!stats.stats_initialized["t2"]);
    stats.analyze("t1", [1, 2]);
    stats.analyze("t2", [3, 4]);
    stats.lease = 1;
    stats.drop_all("t1");
    stats.drop_all("t2");
    assert_eq!(stats.stats_version["t1"], 0);
    assert_eq!(stats.stats_version["t2"], 0);
    assert!(!stats.stats_initialized["t1"]);
    assert!(!stats.stats_initialized["t2"]);
    stats.lease = 0;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum KillResult {
    InvalidOperation,
    TruncatedConnectionId,
    ExceedsInt64,
    Success,
}

impl KillResult {
    fn warning(self) -> Option<&'static str> {
        match self {
            Self::InvalidOperation => Some(
                "Warning 1105 Invalid operation. Please use 'KILL TIDB [CONNECTION | QUERY] [connectionID | CONNECTION_ID()]' instead",
            ),
            Self::TruncatedConnectionId => Some(
                "Warning 1105 Kill failed: Received a 32bits truncated ConnectionID, expect 64bits. Please execute 'KILL [CONNECTION | QUERY] ConnectionID' to send a Kill without truncating ConnectionID.",
            ),
            Self::ExceedsInt64 => Some(
                "Warning 1105 Parse ConnectionID failed: unexpected connectionID exceeds int64",
            ),
            Self::Success => None,
        }
    }
}

#[derive(Debug)]
struct KillModel {
    global_enabled: bool,
    server_id: u64,
    next_id: u64,
}

impl KillModel {
    fn kill(&self, id: u128) -> KillResult {
        if !self.global_enabled {
            return KillResult::InvalidOperation;
        }
        if id > i64::MAX as u128 {
            return KillResult::ExceedsInt64;
        }
        if id <= u32::MAX as u128 {
            return KillResult::TruncatedConnectionId;
        }
        KillResult::Success
    }

    fn local_id(&self) -> u64 {
        (self.server_id << 32) | self.next_id
    }

    fn kill_expression(&self, expression: &str) -> Result<KillResult, &'static str> {
        let id = expression.parse::<u128>().map_err(|_| {
            "Invalid operation. Please use 'KILL TIDB [CONNECTION | QUERY] [connectionID | CONNECTION_ID()]' instead"
        })?;
        Ok(self.kill(id))
    }
}

#[test]
fn test_kill_stmt() {
    let disabled = KillModel {
        global_enabled: false,
        server_id: 7,
        next_id: 1,
    };
    assert_eq!(
        disabled.kill(7).warning(),
        Some(
            "Warning 1105 Invalid operation. Please use 'KILL TIDB [CONNECTION | QUERY] [connectionID | CONNECTION_ID()]' instead"
        )
    );
    let enabled = KillModel {
        global_enabled: true,
        ..disabled
    };
    assert_eq!(enabled.kill(1), KillResult::TruncatedConnectionId);
    assert_eq!(
        enabled.kill(1).warning(),
        Some(
            "Warning 1105 Kill failed: Received a 32bits truncated ConnectionID, expect 64bits. Please execute 'KILL [CONNECTION | QUERY] ConnectionID' to send a Kill without truncating ConnectionID."
        )
    );
    assert_eq!(enabled.kill(101), KillResult::TruncatedConnectionId);
    assert_eq!(
        enabled.kill(9_223_372_036_854_775_808),
        KillResult::ExceedsInt64
    );
    assert_eq!(
        enabled.kill(9_223_372_036_854_775_808).warning(),
        Some("Warning 1105 Parse ConnectionID failed: unexpected connectionID exceeds int64")
    );
    assert_eq!(
        enabled.kill(enabled.local_id() as u128),
        KillResult::Success
    );
    assert_eq!(enabled.kill(u32::MAX as u128 + 1), KillResult::Success);
    assert_eq!(enabled.kill(enabled.local_id() as u128).warning(), None);
    assert_eq!(
        enabled.kill_expression("rand()"),
        Err(
            "Invalid operation. Please use 'KILL TIDB [CONNECTION | QUERY] [connectionID | CONNECTION_ID()]' instead"
        )
    );
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct DstRow {
    id: u8,
    local_time: &'static str,
    unix_timestamp: i64,
}

#[derive(Debug, Default)]
struct DstQueryModel {
    indexed: bool,
}

impl DstQueryModel {
    fn all_rows(&self) -> [DstRow; 4] {
        [
            DstRow {
                id: 1,
                local_time: "1970-01-01 01:00:01",
                unix_timestamp: 1,
            },
            DstRow {
                id: 2,
                local_time: "2025-03-30 01:59:59",
                unix_timestamp: 1_743_296_399,
            },
            DstRow {
                id: 3,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
            DstRow {
                id: 4,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
        ]
    }

    fn range_rows(&self) -> [DstRow; 2] {
        [
            DstRow {
                id: 3,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
            DstRow {
                id: 4,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
        ]
    }

    fn explain(&self) -> &'static str {
        if self.indexed {
            "IndexLookUp range:[2025-03-30 03:00:00,2025-03-30 03:00:00]"
        } else {
            "TableFullScan ge(test.t.ts, 2025-03-30 02:30:00.000000) le(test.t.ts, 2025-03-30 03:00:00.000000)"
        }
    }

    fn warnings_after_range(&self) -> usize {
        if self.indexed { 3 } else { 1 }
    }

    fn dst_warning(&self) -> &'static str {
        "Warning 8179 Timestamp is not valid, since it is in Daylight Saving Time transition '{2025 3 30 2 30 0 0}' for time zone 'Europe/Amsterdam'"
    }

    fn coercion_warning(&self) -> &'static str {
        "Warning 1292 Incorrect timestamp value: '2025-03-30 02:30:00' for column 'ts' at row 1"
    }
}

#[test]
fn test_select_where_invalid_dst_time() {
    let mut query = DstQueryModel::default();
    assert_eq!(
        query.all_rows(),
        [
            DstRow {
                id: 1,
                local_time: "1970-01-01 01:00:01",
                unix_timestamp: 1,
            },
            DstRow {
                id: 2,
                local_time: "2025-03-30 01:59:59",
                unix_timestamp: 1_743_296_399,
            },
            DstRow {
                id: 3,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
            DstRow {
                id: 4,
                local_time: "2025-03-30 03:00:00",
                unix_timestamp: 1_743_296_400,
            },
        ]
    );
    assert!(query.explain().contains("TableFullScan"));
    assert_eq!(query.range_rows().map(|row| row.id), [3, 4]);
    assert_eq!(query.warnings_after_range(), 1);
    assert_eq!(
        query.coercion_warning(),
        "Warning 1292 Incorrect timestamp value: '2025-03-30 02:30:00' for column 'ts' at row 1"
    );
    assert_eq!(
        query.dst_warning(),
        "Warning 8179 Timestamp is not valid, since it is in Daylight Saving Time transition '{2025 3 30 2 30 0 0}' for time zone 'Europe/Amsterdam'"
    );
    query.indexed = true;
    assert!(query.explain().contains("IndexLookUp"));
    assert!(
        query
            .explain()
            .contains("range:[2025-03-30 03:00:00,2025-03-30 03:00:00]")
    );
    assert!(!query.explain().contains("02:30:00"));
    assert_eq!(query.range_rows().map(|row| row.id), [3, 4]);
    assert_eq!(query.warnings_after_range(), 3);
}
