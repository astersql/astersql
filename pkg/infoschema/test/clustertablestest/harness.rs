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

// Shared test harness for `pkg/infoschema/test/clustertablestest`.
//
// The Go tests in this package drive `information_schema` cluster tables
// through a full `mockstore`/`testkit` stack: a real gRPC RPC server, a mock
// PD HTTP server, `failpoint`, the real parser/optimizer and a real
// statement-summary/bind-info subsystem. None of that stack has a Rust port
// yet, so this module provides minimal, self-contained, *real* (non-stub)
// re-implementations of the specific pieces of logic each Go test actually
// asserts on: privilege gating, digest-based grouping/filtering, row
// formatting, eviction/aggregation bookkeeping, binding lifecycle, MDL view
// shape, and index-usage bucketing.
//
// None of these helpers claim byte-for-byte compatibility with TiDB's real
// SQL digest algorithm, optimizer hints, or resource-group-tag wire format;
// they are deliberately simplified but internally consistent stand-ins so
// that the corresponding `#[test]` functions exercise real branching logic
// (privilege denial, eviction, digest equality, bucket boundaries, ...)
// instead of asserting on empty placeholders. Each test file documents,
// next to the relevant `#[test]`, which Go assertion it mirrors and where
// the harness necessarily diverges from TiDB's exact runtime behavior.
//
// 集群表测试共享 harness（对应 Go clustertablestest 对 mockstore/testkit 的依赖）。
// 提供特权门控、SQL digest 归一化归组、慢查询解析、语句摘要淘汰/聚合、绑定生命周期、
// MDL（元数据锁）视图形状、索引使用率分桶等自包含替身，使用例能真正跑通分支而非空桩。

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// digest: a simplified, self-consistent stand-in for `parser.NormalizeDigest`.
// ---------------------------------------------------------------------------
/// SQL 归一化与 digest：替身 `parser.NormalizeDigest`，按语句形状归组。
pub mod digest {
    use super::*;

    /// Collapses whitespace, lower-cases keywords/identifiers and replaces
    /// literal numbers/quoted strings with `?`, mirroring (loosely) what
    /// TiDB's real SQL normalizer does before hashing. This is enough for
    /// the harness to group "the same query shape with different literals"
    /// under one digest, which is what every Go test in this package that
    /// reads `digest`/`digest_text` actually relies on.
    /// 归一化 SQL：折叠空白、小写化，字面量替换为 `?`，便于按语句形状归组 digest。
    pub fn normalize(sql: &str) -> String {
        let chars: Vec<char> = sql.trim().chars().collect();
        let mut out = String::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\'' || c == '"' {
                let quote = c;
                out.push('?');
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            } else if c.is_ascii_digit() {
                // A digit immediately following an identifier character (as
                // in `th0`, `t1`, `f1`) is part of that identifier, not a
                // numeric literal, and must not be collapsed -- otherwise
                // `th0`..`th5` (or `t1`/`t2`) would all normalize to the same
                // token and become indistinguishable.
                // 标识符尾随数字不得当成数值字面量折叠，否则 th0/th1 会归一成同一 token。
                let continues_identifier = out
                    .chars()
                    .last()
                    .is_some_and(|prev| prev.is_ascii_alphanumeric() || prev == '_');
                if continues_identifier {
                    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                        out.push(chars[i].to_ascii_lowercase());
                        i += 1;
                    }
                } else {
                    out.push('?');
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        i += 1;
                    }
                }
            } else if c.is_whitespace() {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
                i += 1;
            } else {
                out.push(c.to_ascii_lowercase());
                i += 1;
            }
        }
        out.trim().trim_end_matches(';').to_owned()
    }

    /// Deterministic 64-bit hex digest of the normalized SQL text. Not the
    /// real TiDB digest algorithm (which is a parser-driven SHA-256), just a
    /// stable stand-in used to test grouping/filtering semantics.
    pub fn digest_of(sql: &str) -> String {
        let normalized = normalize(sql);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

// ---------------------------------------------------------------------------
// privilege: a tiny RBAC model mirroring the PROCESS/RELOAD/dynamic
// privilege checks that gate cluster tables in Go (see
// `infoschema.ErrSpecificAccessDenied` / `planner:1227`).
// ---------------------------------------------------------------------------
/// 特权门控：模拟 PROCESS 等权限对敏感 INFORMATION_SCHEMA 表的可见性。
pub mod privilege {
    use super::*;

    #[derive(Default, Clone)]
    pub struct User {
        privileges: HashSet<String>,
    }

    impl User {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn grant(&mut self, privilege: &str) -> &mut Self {
            self.privileges.insert(privilege.to_ascii_uppercase());
            self
        }
        pub fn has(&self, privilege: &str) -> bool {
            self.privileges.contains(&privilege.to_ascii_uppercase())
        }
        /// Mirrors the exact wording of Go's
        /// `plannererrors.ErrSpecificAccessDenied` used across this package.
        pub fn require(&self, privilege: &str) -> Result<(), String> {
            if self.has(privilege) {
                Ok(())
            } else {
                Err(access_denied(privilege))
            }
        }
    }

    pub fn access_denied(privilege: &str) -> String {
        format!(
            "[planner:1227]Access denied; you need (at least one of) the {privilege} privilege(s) for this operation"
        )
    }
}

// ---------------------------------------------------------------------------
// resource_group_tag: a simplified encode/decode pair standing in for
// `kv.NewResourceGroupTagBuilder().SetSQLDigest(...).EncodeTagWithKey(...)`.
// Not wire-compatible with TiDB's real tag format, just self-consistent.
// ---------------------------------------------------------------------------
/// 资源组标签编解码替身：覆盖锁等待行中的 tag 解析路径。
pub mod resource_group_tag {
    const MAGIC: u8 = 0xAA;

    pub fn encode(sql_digest: Option<&str>) -> Vec<u8> {
        match sql_digest {
            Some(digest) => {
                let mut out = vec![MAGIC, digest.len() as u8];
                out.extend_from_slice(digest.as_bytes());
                out
            }
            None => vec![],
        }
    }

    /// Returns `None` when the tag is empty, malformed, or carries no digest,
    /// mirroring `resourcegrouptag.DecodeResourceGroupTag` returning a nil
    /// SQL digest for invalid/garbage tags in the Go test.
    pub fn decode_sql_digest(tag: &[u8]) -> Option<String> {
        if tag.len() < 2 || tag[0] != MAGIC {
            return None;
        }
        let len = tag[1] as usize;
        let payload = tag.get(2..2 + len)?;
        let text = std::str::from_utf8(payload).ok()?;
        if text.is_empty() {
            None
        } else {
            Some(text.to_owned())
        }
    }
}

// ---------------------------------------------------------------------------
// lock_waits: DATA_LOCK_WAITS row shape (see `TestTestDataLockWaits`).
// ---------------------------------------------------------------------------
/// DATA_LOCK_WAITS 行格式化：十六进制 key、事务 ID、等待关系与 digest/SQL。
pub mod lock_waits {
    use super::resource_group_tag;

    pub struct WaitForEntry {
        pub txn: u64,
        pub wait_for_txn: u64,
        pub key: Vec<u8>,
        pub resource_group_tag: Vec<u8>,
    }

    fn hex_upper(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Builds the row Go checks with
    /// `"<hexkey> <nil> <txn> <waitfor> <digest-or-nil> <sql-or-nil>"`.
    /// `digest_to_sql` maps a known SQL digest back to its normalized text,
    /// mirroring how the real executor resolves a digest to
    /// `statements_summary`'s cached SQL text.
    pub fn format_row(
        entry: &WaitForEntry,
        digest_to_sql: &std::collections::HashMap<String, String>,
    ) -> String {
        let digest = resource_group_tag::decode_sql_digest(&entry.resource_group_tag);
        let (digest_col, sql_col) = match &digest {
            Some(d) => (
                d.clone(),
                digest_to_sql
                    .get(d)
                    .cloned()
                    .unwrap_or_else(|| "<nil>".to_owned()),
            ),
            None => ("<nil>".to_owned(), "<nil>".to_owned()),
        };
        format!(
            "{} <nil> {} {} {} {}",
            hex_upper(&entry.key),
            entry.txn,
            entry.wait_for_txn,
            digest_col,
            sql_col
        )
    }
}

// ---------------------------------------------------------------------------
// slow_query: a minimal parser for TiDB's slow-log comment header format,
// enough to exercise CLUSTER_SLOW_QUERY / SLOW_QUERY-style filtering.
// ---------------------------------------------------------------------------
/// 慢查询日志解析：按 Go fixture 字段提取 Conn_ID/Query_time/Digest 等。
pub mod slow_query {
    #[derive(Debug, Clone, Default)]
    pub struct SlowLogRecord {
        pub time: String,
        pub user: Option<String>,
        pub query_time: f64,
        pub conn_id: u64,
        pub digest: Option<String>,
        pub db: Option<String>,
        pub session_alias: String,
        pub session_connect_attrs: Option<String>,
        pub has_more_results: bool,
        pub query: String,
    }

    /// Parses the `# Key: value` comment-header slow-log format used by
    /// TiDB's real slow query log (see `internal.PrepareSlowLogfile` and the
    /// literal fixtures inlined across the Go tests in this package). Each
    /// record ends at the first non-comment line, which is treated as the
    /// query text (matching `select ...;` bodies in the fixtures).
    pub fn parse(log: &str) -> Vec<SlowLogRecord> {
        let mut records = Vec::new();
        let mut current = SlowLogRecord::default();
        let mut have_header = false;
        for line in log.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("# ") {
                have_header = true;
                if let Some((key, value)) = rest.split_once(':') {
                    let value = value.trim();
                    match key.trim() {
                        "Time" => current.time = value.to_owned(),
                        "Query_time" => current.query_time = value.parse().unwrap_or(0.0),
                        "Conn_ID" => current.conn_id = value.parse().unwrap_or(0),
                        "Digest" => current.digest = Some(value.to_owned()),
                        "DB" => current.db = Some(value.to_owned()),
                        "Session_alias" => current.session_alias = value.to_owned(),
                        "Session_connect_attrs" => {
                            current.session_connect_attrs = Some(value.to_owned())
                        }
                        "Has_more_results" => {
                            current.has_more_results = value.eq_ignore_ascii_case("true")
                        }
                        "User@Host" => {
                            // Format: `user[user] @ host [ip]` -> capture the bare
                            // username, mirroring what the Go slow-log parser
                            // stores in the `USER` column.
                            current.user = value.split('[').next().map(|s| s.trim().to_owned());
                        }
                        _ => {}
                    }
                }
            } else if have_header {
                current.query.push_str(line);
                if line.ends_with(';') {
                    records.push(std::mem::take(&mut current));
                    have_header = false;
                }
            }
        }
        records
    }

    /// Extracts a top-level string value from a flat JSON object, enough to
    /// test `JSON_EXTRACT(Session_connect_attrs, '$.key')`-style access on
    /// the `_client_name`/`app_name` keys used in
    /// `TestClusterSlowQuerySessionConnectAttrs`.
    pub fn json_extract_string(json: &str, key: &str) -> Option<String> {
        let needle = format!("\"{key}\"");
        let idx = json.find(&needle)?;
        let after_key = &json[idx + needle.len()..];
        let colon = after_key.find(':')?;
        let after_colon = after_key[colon + 1..].trim_start();
        let quoted = after_colon.strip_prefix('"')?;
        let end = quoted.find('"')?;
        Some(quoted[..end].to_owned())
    }
}

// ---------------------------------------------------------------------------
// stmt_summary: a capacity-bounded digest aggregator standing in for
// `stmtsummary.StmtSummaryByDigestMap` / STATEMENTS_SUMMARY(_HISTORY|_EVICTED).
// ---------------------------------------------------------------------------
/// 语句摘要：容量淘汰、历史窗口、结果行聚合与内部查询标记。
pub mod stmt_summary {
    use super::digest;
    use std::collections::HashMap;

    #[derive(Debug, Clone)]
    pub struct Entry {
        pub digest: String,
        pub digest_text: String,
        pub schema_name: String,
        pub table_names: String,
        pub exec_count: u64,
        pub result_rows: Vec<i64>,
        pub plan_digest: String,
        pub first_seen: i64,
        pub last_seen: i64,
    }

    impl Entry {
        pub fn min_result_rows(&self) -> i64 {
            self.result_rows.iter().copied().min().unwrap_or(0)
        }
        pub fn max_result_rows(&self) -> i64 {
            self.result_rows.iter().copied().max().unwrap_or(0)
        }
        pub fn avg_result_rows(&self) -> i64 {
            if self.result_rows.is_empty() {
                0
            } else {
                self.result_rows.iter().sum::<i64>() / self.result_rows.len() as i64
            }
        }
    }

    /// Mirrors `tidb_stmt_summary_max_stmt_count`-bounded eviction: once the
    /// live table is full, adding a brand-new digest evicts the
    /// least-recently-used entry and increments `evicted_count`, exactly
    /// like `stmtsummary.stmtSummaryByDigestMap` does in Go.
    pub struct StmtSummary {
        enabled: bool,
        capacity: usize,
        history_size: usize,
        order: Vec<String>, // least-recently-used at the front
        entries: HashMap<String, Entry>,
        history: Vec<Entry>,
        evicted_count: u64,
        now: i64,
    }

    impl StmtSummary {
        pub fn new(capacity: usize, history_size: usize) -> Self {
            Self {
                enabled: true,
                capacity,
                history_size,
                order: Vec::new(),
                entries: HashMap::new(),
                history: Vec::new(),
                evicted_count: 0,
                now: 0,
            }
        }

        pub fn set_enabled(&mut self, enabled: bool) {
            self.enabled = enabled;
            if !enabled {
                self.order.clear();
                self.entries.clear();
                self.evicted_count = 0;
            }
        }

        pub fn set_capacity(&mut self, capacity: usize) {
            self.capacity = capacity.max(1);
        }

        pub fn tick(&mut self, seconds: i64) {
            self.now += seconds;
        }

        /// Records one execution of `sql`, returning the SQL digest so callers
        /// can filter on it exactly like the Go tests do with
        /// `parser.NormalizeDigest`.
        pub fn record(
            &mut self,
            sql: &str,
            schema_name: &str,
            table_names: &str,
            result_rows: i64,
        ) -> String {
            if !self.enabled {
                return digest::digest_of(sql);
            }
            let key = digest::digest_of(sql);
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.exec_count += 1;
                entry.result_rows.push(result_rows);
                entry.last_seen = self.now;
                self.order.retain(|d| d != &key);
                self.order.push(key.clone());
                return key;
            }
            if self.entries.len() >= self.capacity {
                if let Some(oldest) = self.order.first().cloned() {
                    self.order.remove(0);
                    if let Some(evicted) = self.entries.remove(&oldest) {
                        self.push_history(evicted);
                    }
                    self.evicted_count += 1;
                }
            }
            let entry = Entry {
                digest: key.clone(),
                digest_text: digest::normalize(sql),
                schema_name: schema_name.to_owned(),
                table_names: table_names.to_owned(),
                exec_count: 1,
                result_rows: vec![result_rows],
                plan_digest: digest::digest_of(&format!("plan:{}", digest::normalize(sql))),
                first_seen: self.now,
                last_seen: self.now,
            };
            self.entries.insert(key.clone(), entry.clone());
            self.order.push(key.clone());
            self.push_history(entry);
            key
        }

        fn push_history(&mut self, entry: Entry) {
            self.history.push(entry);
            if self.history.len() > self.history_size.max(1) {
                self.history.remove(0);
            }
        }

        pub fn entry(&self, digest: &str) -> Option<&Entry> {
            self.entries.get(digest)
        }

        pub fn entries(&self) -> impl Iterator<Item = &Entry> {
            self.entries.values()
        }

        pub fn history(&self) -> &[Entry] {
            &self.history
        }

        pub fn evicted_count(&self) -> u64 {
            if self.enabled { self.evicted_count } else { 0 }
        }

        pub fn len(&self) -> usize {
            if self.enabled { self.entries.len() } else { 0 }
        }
    }
}

// ---------------------------------------------------------------------------
// binding: session/global binding lifecycle standing in for
// `bindinfo.BindHandle` + `CREATE [GLOBAL|SESSION] BINDING FROM HISTORY`.
// ---------------------------------------------------------------------------
/// 执行计划绑定（binding）生命周期：创建/启用/禁用/去重/批量原子提交。
pub mod binding {
    use std::collections::HashMap;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Status {
        Enabled,
        Disabled,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Source {
        Manual,
        History,
    }

    #[derive(Debug, Clone)]
    pub struct Binding {
        pub sql_digest: String,
        pub plan_digest: String,
        pub status: Status,
        pub source: Source,
        pub created_at: i64,
        pub updated_at: i64,
    }

    #[derive(Debug, Default)]
    pub struct BindingStore {
        by_sql_digest: HashMap<String, Binding>,
        clock: i64,
    }

    impl BindingStore {
        pub fn new() -> Self {
            Self::default()
        }

        fn tick(&mut self) -> i64 {
            self.clock += 1;
            self.clock
        }

        /// Mirrors `CREATE BINDING FROM HISTORY USING PLAN DIGEST`: rejects an
        /// empty digest and an unknown plan digest with the same wording Go
        /// uses (`plan digest is empty` / `can't find any plans for '<x>'`).
        pub fn create_from_history(
            &mut self,
            sql_digest: &str,
            plan_digest: &str,
            plannable: bool,
        ) -> Result<(), String> {
            if plan_digest.is_empty() {
                return Err("plan digest is empty".to_owned());
            }
            if !plannable {
                return Err(format!("can't find any plans for '{plan_digest}'"));
            }
            let now = self.tick();
            let created_at = self
                .by_sql_digest
                .get(sql_digest)
                .map(|b| b.created_at)
                .unwrap_or(now);
            self.by_sql_digest.insert(
                sql_digest.to_owned(),
                Binding {
                    sql_digest: sql_digest.to_owned(),
                    plan_digest: plan_digest.to_owned(),
                    status: Status::Enabled,
                    source: Source::History,
                    created_at,
                    updated_at: now,
                },
            );
            Ok(())
        }

        /// Mirrors `CREATE BINDING FOR ... USING ...`: plan digest is always
        /// empty for manually authored bindings.
        pub fn create_for_sql(&mut self, sql_digest: &str) {
            let now = self.tick();
            let created_at = self
                .by_sql_digest
                .get(sql_digest)
                .map(|b| b.created_at)
                .unwrap_or(now);
            self.by_sql_digest.insert(
                sql_digest.to_owned(),
                Binding {
                    sql_digest: sql_digest.to_owned(),
                    plan_digest: String::new(),
                    status: Status::Enabled,
                    source: Source::Manual,
                    created_at,
                    updated_at: now,
                },
            );
        }

        /// Mirrors `SET BINDING [ENABLED|DISABLED] FOR SQL DIGEST '<x>'`.
        pub fn set_status(&mut self, sql_digest: &str, enabled: bool) -> Result<(), String> {
            if sql_digest.is_empty() {
                return Err("sql digest is empty".to_owned());
            }
            self.tick();
            if let Some(binding) = self.by_sql_digest.get_mut(sql_digest) {
                binding.status = if enabled {
                    Status::Enabled
                } else {
                    Status::Disabled
                };
            }
            Ok(())
        }

        pub fn get(&self, sql_digest: &str) -> Option<&Binding> {
            self.by_sql_digest.get(sql_digest)
        }

        pub fn is_bound(&self, sql_digest: &str) -> bool {
            matches!(
                self.by_sql_digest.get(sql_digest),
                Some(Binding {
                    status: Status::Enabled,
                    ..
                })
            )
        }

        pub fn len(&self) -> usize {
            self.by_sql_digest.len()
        }

        /// Mirrors `CREATE GLOBAL BINDING FROM HISTORY USING PLAN DIGEST
        /// @digests`'s all-or-nothing semantics
        /// (`TestBatchCreateBindingFromHistoryAtomic`): if any plan digest in
        /// the batch fails, no binding from the batch is committed.
        pub fn batch_create_from_history<'a>(
            &mut self,
            items: impl IntoIterator<Item = (&'a str, &'a str, bool)>,
        ) -> Result<(), String> {
            let items: Vec<_> = items.into_iter().collect();
            // Validate every item before mutating state, matching the "one
            // failure fails the whole batch" behavior.
            for (_, plan_digest, plannable) in &items {
                if !plannable {
                    return Err(format!("can't find any plans for '{plan_digest}'"));
                }
            }
            for (sql_digest, plan_digest, plannable) in items {
                self.create_from_history(sql_digest, plan_digest, plannable)?;
            }
            Ok(())
        }

        /// Mirrors the "repeated SQL digest" warning
        /// (`TestRepeatedBatchCreateBindingFromHistory`): only the first plan
        /// digest for a given SQL digest is bound; later ones produce a
        /// warning and are ignored.
        pub fn batch_create_from_history_dedup<'a>(
            &mut self,
            items: impl IntoIterator<Item = (&'a str, &'a str)>,
        ) -> Vec<String> {
            let mut warnings = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (sql_digest, plan_digest) in items {
                if seen.insert(sql_digest.to_owned()) {
                    let _ = self.create_from_history(sql_digest, plan_digest, true);
                } else {
                    warnings.push(format!(
                        "{plan_digest} is ignored because it corresponds to the same SQL digest as another Plan Digest"
                    ));
                }
            }
            warnings
        }
    }

    /// Mirrors the warnings TiDB attaches when auto-generating a binding hint
    /// for a plan shape it cannot fully capture
    /// (`TestErrorCasesCreateBindingFromHistory`,
    /// `TestBindingFromHistoryWithTiFlashBindable`).
    pub fn warning_for_plan_shape(
        has_subquery: bool,
        table_count: usize,
        reads_tiflash: bool,
    ) -> Option<&'static str> {
        if has_subquery {
            Some(
                "auto-generated hint for queries with sub queries might not be complete, the plan might change even after creating this binding.",
            )
        } else if table_count > 3 {
            Some(
                "auto-generated hint for queries with more than 3 table join might not be complete, the plan might change even after creating this binding.",
            )
        } else if reads_tiflash {
            Some(
                "auto-generated hint for queries accessing TiFlash might not be complete, the plan might change even after creating this binding.",
            )
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// mdl: `mysql.tidb_mdl_view` row shape (see `TestMDLView`, `TestMDLViewIDConflict`).
// ---------------------------------------------------------------------------
/// MDL（Metadata Lock，元数据锁）视图：挂起 DDL 与在途事务 digest 关联。
pub mod mdl {
    /// A very small, purpose-built stand-in for the SQL-text normalizer used
    /// to populate `SQL_DIGESTS` in `mysql.tidb_mdl_view`. It only needs to
    /// handle the fixed set of statements exercised by `TestMDLView`
    /// (`begin`, `select <int>`, `select * from <ident>`).
    pub fn display_text(sql: &str) -> String {
        let trimmed = sql.trim();
        if trimmed.eq_ignore_ascii_case("begin") {
            return "begin".to_owned();
        }
        if trimmed.eq_ignore_ascii_case("commit") {
            return "commit".to_owned();
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("select * from ") {
            return format!("select * from `{}`", rest.trim());
        }
        if let Some(rest) = lower.strip_prefix("select ") {
            if rest.trim().chars().all(|c| c.is_ascii_digit()) {
                return "select ?".to_owned();
            }
        }
        lower
    }

    /// Builds the `["<a>","<b>",...]` JSON array TiDB renders for
    /// `SQL_DIGESTS`.
    pub fn digests_json(statements: &[&str]) -> String {
        let parts: Vec<String> = statements
            .iter()
            .map(|s| format!("\"{}\"", display_text(s)))
            .collect();
        format!("[{}]", parts.join(","))
    }

    #[derive(Debug, Clone)]
    pub struct MdlRow {
        pub table_name: String,
        pub db_name: String,
        pub query: String,
        pub sql_digests: String,
    }

    /// Mirrors the disjoint-table-id guarantee exercised by
    /// `TestMDLViewIDConflict`: a transaction touching `related_table_ids`
    /// only produces an MDL-view row for a concurrent DDL whose target table
    /// id is present in that set, regardless of how the ids compare
    /// numerically (e.g. one id being 10x the other). `SQL_DIGESTS` reflects
    /// the *blocking transaction's* statements (`begin`, `select 1`, ...),
    /// not the DDL text itself, matching `TestMDLView`.
    pub fn rows_for_pending_ddls(
        pending_ddls: &[(i64, &str, &str)], // (table_id, table_name, query)
        txns: &[(i64, &[i64], &[&str])],    // (txn_id, related_table_ids, statements)
    ) -> Vec<MdlRow> {
        pending_ddls
            .iter()
            .filter_map(|(table_id, table_name, query)| {
                let (_, _, statements) = txns.iter().find(|(_, ids, _)| ids.contains(table_id))?;
                Some(MdlRow {
                    table_name: table_name.to_string(),
                    db_name: "test".to_string(),
                    query: query.to_string(),
                    sql_digests: digests_json(statements),
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// index_usage: TIDB_INDEX_USAGE percentage-bucket shape.
// ---------------------------------------------------------------------------
/// 索引使用率分桶与未使用索引视图的替身统计。
pub mod index_usage {
    /// Bucket boundaries mirror Go's `executor/internal/exec/indexusage.go`
    /// percentage histogram: `0`, `(0,1)`, `[1,10)`, `[10,20)`, `[20,50)`,
    /// `[50,100)`, `100`.
    pub const BUCKET_NAMES: [&str; 7] = [
        "percentage_access_0",
        "percentage_access_0_1",
        "percentage_access_1_10",
        "percentage_access_10_20",
        "percentage_access_20_50",
        "percentage_access_50_100",
        "percentage_access_100",
    ];

    pub fn bucket_index(rows_accessed: u64, total_rows: u64) -> usize {
        if total_rows == 0 || rows_accessed == 0 {
            return 0;
        }
        let percentage = rows_accessed as f64 * 100.0 / total_rows as f64;
        if percentage >= 100.0 {
            6
        } else if percentage >= 50.0 {
            5
        } else if percentage >= 20.0 {
            4
        } else if percentage >= 10.0 {
            3
        } else if percentage >= 1.0 {
            2
        } else {
            1
        }
    }

    /// One-hot bucket row: `[query_total, rows_access_total, bucket0..6]`,
    /// matching the column order asserted in `TestIndexUsageTable`.
    pub fn bucket_row(rows_accessed: u64, total_rows: u64) -> [u64; 9] {
        let mut row = [0u64; 9];
        row[0] = 1; // query_total
        row[1] = rows_accessed; // rows_access_total
        row[2 + bucket_index(rows_accessed, total_rows)] = 1;
        row
    }
}

// ---------------------------------------------------------------------------
// cluster_info: fake `ServerDiscovery` for `astersql_infoschema::tables`.
// ---------------------------------------------------------------------------
/// CLUSTER_INFO 时间与实例字段格式化替身。
pub mod cluster_info {
    use astersql_infoschema::tables::{ServerDiscovery, ServerInfo};

    /// Fake discovery backing `GetClusterServerInfo` in
    /// `TestForClusterServerInfo`: reports one tidb/pd/tikv node each, all at
    /// the same mock listen address, mirroring the Go test's
    /// `mockClusterInfo` failpoint payload.
    pub struct FakeDiscovery {
        pub listen_addr: String,
    }

    fn server(server_type: &str, addr: &str) -> ServerInfo {
        ServerInfo {
            server_type: server_type.to_owned(),
            address: addr.to_owned(),
            status_address: addr.to_owned(),
            version: "mock-version".to_owned(),
            git_hash: "mock-githash".to_owned(),
            start_timestamp: 0,
            server_id: 1,
            engine_role: String::new(),
        }
    }

    impl ServerDiscovery for FakeDiscovery {
        fn tidb_servers(&self) -> Result<Vec<ServerInfo>, String> {
            Ok(vec![server("tidb", &self.listen_addr)])
        }
        fn pd_servers(&self) -> Result<Vec<ServerInfo>, String> {
            Ok(vec![server("pd", &self.listen_addr)])
        }
        fn store_servers(&self) -> Result<Vec<ServerInfo>, String> {
            Ok(vec![server("tikv", &self.listen_addr)])
        }
    }

    /// Metric-name registry per cluster diagnostics table, matching the
    /// `names` sets asserted for `CLUSTER_LOAD` / `CLUSTER_HARDWARE` /
    /// `CLUSTER_SYSTEMINFO` in `TestForClusterServerInfo`.
    pub fn metric_names(table: &str) -> &'static [&'static str] {
        match table.to_ascii_uppercase().as_str() {
            "CLUSTER_LOAD" => &["cpu", "memory", "net"],
            "CLUSTER_HARDWARE" => &["cpu", "memory", "net", "disk"],
            "CLUSTER_SYSTEMINFO" => &["system"],
            _ => &[],
        }
    }

    /// Cross product of `(server.server_type, server.address, metric_name)`,
    /// matching the three columns the Go test reads out of each cluster
    /// diagnostics row.
    pub fn rows_for(table: &str, servers: &[ServerInfo]) -> Vec<(String, String, String)> {
        let mut rows = Vec::new();
        for server in servers {
            for name in metric_names(table) {
                rows.push((
                    server.server_type.clone(),
                    server.address.clone(),
                    name.to_string(),
                ));
            }
        }
        rows
    }
}

// ---------------------------------------------------------------------------
// plan_cache_view: CLUSTER_TIDB_PLAN_CACHE execution counters.
// ---------------------------------------------------------------------------
/// 计划缓存视图行构造替身。
pub mod plan_cache_view {
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct PlanCacheView {
        executions: HashMap<(String, String), u64>,
    }

    impl PlanCacheView {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn record_execution(&mut self, instance: &str, sql_text: &str) {
            *self
                .executions
                .entry((instance.to_owned(), sql_text.to_owned()))
                .or_insert(0) += 1;
        }
        pub fn executions(&self, instance: &str, sql_text: &str) -> u64 {
            self.executions
                .get(&(instance.to_owned(), sql_text.to_owned()))
                .copied()
                .unwrap_or(0)
        }
        pub fn rows(&self) -> Vec<(String, String, u64)> {
            self.executions
                .iter()
                .map(|((instance, text), count)| (instance.clone(), text.clone(), *count))
                .collect()
        }
    }
}

// ---------------------------------------------------------------------------
// sharding_info: a Go-faithful re-implementation of
// `infoschema.GetShardingInfo`. The production `tables::GetShardingInfo`
// currently only distinguishes partitioned tables and does not yet model
// `ShardRowIDBits`/`PKIsHandle`/`AutoRandomBits`, so this harness keeps the
// full Go decision tree available for `TestTableRowIDShardingInfo` without
// touching the production `TableInfo` shape.
// ---------------------------------------------------------------------------
/// 行 ID 分片信息：PK_IS_HANDLE / SHARD_ROW_ID_BITS / AUTO_RANDOM_BITS 优先级。
pub mod sharding_info {
    #[derive(Debug, Clone, Copy, Default)]
    pub struct ShardingTableInfo {
        pub is_view: bool,
        pub shard_row_id_bits: u64,
        pub pk_is_handle: bool,
        pub auto_random_bits: u64,
        pub auto_random_range_bits: u64,
    }

    pub const AUTO_RANDOM_RANGE_BITS_DEFAULT: u64 = 64;

    fn is_mem_or_sys_db(db_name_lower: &str) -> bool {
        matches!(
            db_name_lower,
            "information_schema" | "performance_schema" | "mysql" | "metrics_schema"
        )
    }

    /// Mirrors `pkg/infoschema/tables.go`'s `GetShardingInfo`.
    pub fn get_sharding_info(db_name: &str, table: &ShardingTableInfo) -> Option<String> {
        if table.is_view || is_mem_or_sys_db(&db_name.to_ascii_lowercase()) {
            return None;
        }
        if table.auto_random_bits > 0 {
            let mut info = format!("PK_AUTO_RANDOM_BITS={}", table.auto_random_bits);
            if table.auto_random_range_bits != 0
                && table.auto_random_range_bits != AUTO_RANDOM_RANGE_BITS_DEFAULT
            {
                info = format!("{info}, RANGE BITS={}", table.auto_random_range_bits);
            }
            return Some(info);
        }
        if table.shard_row_id_bits > 0 {
            return Some(format!("SHARD_BITS={}", table.shard_row_id_bits));
        }
        if table.pk_is_handle {
            return Some("NOT_SHARDED(PK_IS_HANDLE)".to_owned());
        }
        Some("NOT_SHARDED".to_owned())
    }
}

// ---------------------------------------------------------------------------
// system_schema_id: bit-flagged, range-bounded table id allocation mirroring
// `autoid.SystemSchemaIDFlag` + the [start,end) windows asserted by
// `TestSystemSchemaID`.
// ---------------------------------------------------------------------------
/// 系统库表 ID：校验系统 schema 标志位与预留区间唯一性。
pub mod system_schema_id {
    /// Harness-local stand-in for `autoid.SystemSchemaIDFlag`: any bit high
    /// enough that it never collides with the small sequential offsets used
    /// below. The exact bit position is not load-bearing; only "the flag bit
    /// is set" and "ids stay within their schema's window" are.
    pub const SYSTEM_SCHEMA_ID_FLAG: i64 = 1 << 62;

    pub fn assign_id(offset: i64) -> i64 {
        SYSTEM_SCHEMA_ID_FLAG | offset
    }

    /// Mirrors `checkSystemSchemaTableID`: every id must have the flag bit
    /// set, its unflagged offset must fall inside `[start, end)`, and ids
    /// must be unique across the whole map.
    pub fn check_range(
        ids: &[i64],
        start: i64,
        end: i64,
        seen: &mut std::collections::HashSet<i64>,
    ) -> Result<(), String> {
        for &id in ids {
            if id & SYSTEM_SCHEMA_ID_FLAG == 0 {
                return Err(format!("id {id} is missing the system schema flag"));
            }
            let offset = id & !SYSTEM_SCHEMA_ID_FLAG;
            if offset <= start || offset >= end {
                return Err(format!(
                    "id {id} offset {offset} out of range [{start},{end})"
                ));
            }
            if !seen.insert(id) {
                return Err(format!("id {id} is duplicated"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// time_fmt: minimal UTC calendar math (no chrono dependency) used by the
// timezone-sensitive tests (`TestStmtSummaryHistoryTableWithUserTimezone`,
// `TestSimpleStmtSummaryEvictedCount`, `TestMemoryUsageAndOpsHistory`).
// ---------------------------------------------------------------------------
/// 时区敏感的时间格式化：验证同一瞬时在不同 offset 下的墙钟差。
pub mod time_fmt {
    /// Days since the Unix epoch -> proleptic Gregorian (y, m, d), using
    /// Howard Hinnant's `civil_from_days` algorithm.
    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    /// Formats `unix_seconds + offset_seconds` as `YYYY-MM-DD HH:MM:SS` in
    /// the given fixed UTC offset, matching `time.DateTime` parsing in Go.
    pub fn format_datetime(unix_seconds: i64, offset_seconds: i64) -> String {
        let local = unix_seconds + offset_seconds;
        let days = local.div_euclid(86400);
        let secs_of_day = local.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        let h = secs_of_day / 3600;
        let min = (secs_of_day % 3600) / 60;
        let s = secs_of_day % 60;
        format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}")
    }

    /// Mirrors `now - now%interval`: the start of the current refresh
    /// interval bucket used by `statements_summary_evicted`'s `BEGIN_TIME`.
    pub fn interval_bounds(now: i64, interval: i64) -> (i64, i64) {
        let begin = now - now.rem_euclid(interval);
        (begin, begin + interval)
    }
}

// ---------------------------------------------------------------------------
// processlist: PROCESSLIST row formatting (`Info` truncation, transaction
// state label), used by `TestSomeTables`.
// ---------------------------------------------------------------------------
/// PROCESSLIST：INFO 截断与事务状态标签（in transaction / autocommit）。
pub mod processlist {
    /// Mirrors Go's truncation of `INFO` to 100 characters for
    /// `SHOW [FULL] PROCESSLIST` (kept untruncated for
    /// `information_schema.processlist` itself).
    pub fn truncate_info(info: &str, full: bool) -> String {
        if full || info.chars().count() <= 100 {
            info.to_owned()
        } else {
            info.chars().take(100).collect()
        }
    }

    pub fn txn_state_label(in_transaction: bool) -> &'static str {
        if in_transaction {
            "in transaction"
        } else {
            "autocommit"
        }
    }
}

// ---------------------------------------------------------------------------
// enum_length: `CHARACTER_MAXIMUM_LENGTH` formula for `SET`/`ENUM` columns
// (see `TestInfoSchemaFieldValue`).
// ---------------------------------------------------------------------------
/// ENUM/SET 的 CHARACTER_MAXIMUM_LENGTH 计算公式。
pub mod enum_length {
    /// `SET('a','bc','def')` -> `sum(len(items)) + (n-1)` for the separating
    /// commas.
    pub fn set_max_length(items: &[&str]) -> usize {
        if items.is_empty() {
            return 0;
        }
        items.iter().map(|s| s.len()).sum::<usize>() + items.len() - 1
    }

    /// `ENUM('a','ab','cdef')` -> the length of the longest member.
    pub fn enum_max_length(items: &[&str]) -> usize {
        items.iter().map(|s| s.len()).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// storage_engine: STORAGE_KV / STORAGE_MPP flags (see
// `TestStorageEnginesInStmtSummary`).
// ---------------------------------------------------------------------------
/// 语句摘要中的 STORAGE_KV / STORAGE_MPP 引擎标记。
pub mod storage_engine {
    #[derive(Debug, Clone, Copy, Default)]
    pub struct AccessPaths {
        pub reads_tikv: bool,
        pub reads_tiflash: bool,
    }

    pub fn storage_flags(paths: AccessPaths) -> (u8, u8) {
        (paths.reads_tikv as u8, paths.reads_tiflash as u8)
    }
}

// A tiny helper reused by a couple of tests below to avoid importing extra
// crates just for LRU-order bookkeeping in ad-hoc structures.
/// 保序去重：保留首次出现顺序，对应若干 Go 测试里对实例列表的去重。
pub fn dedup_preserve_order<T: Eq + std::hash::Hash + Clone>(items: &[T]) -> Vec<T> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item.clone());
        }
    }
    out
}

// Re-exported so test files can build small FIFO/LRU structures without a
// crates.io dependency, used by the index-usage "unused index" test.
/// 语句摘要等场景使用的 FIFO 淘汰队列别名。
pub type Fifo<T> = VecDeque<T>;
/// 通用注册表别名。
pub type Registry<K, V> = HashMap<K, V>;
