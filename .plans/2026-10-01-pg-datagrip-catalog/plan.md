# DataGrip PostgreSQL catalog probes

This living ExecPlan follows root PLANS.md.

## Purpose / Big Picture

The two SQL queries in the user's DataGrip log must execute in simple and extended PG protocol without altering MySQL SQL behavior. The database probe must report live schema identities and current-database ordering. The lock probe must report the oldest live native transaction identifier, not a fabricated empty set.

## Progress

- [x] Locate PG command and prepared-statement boundaries and canonical metadata sources.
- [x] Capture failing regression with the exact supplied SQL (42601 at ::varchar).
- [x] Add PG catalog query classification, typed results and extended-statement lifecycle support.
- [x] Run scoped PG regression, JDBC probes and Ready checks; update compatibility documentation.

## Surprises & Discoveries

PostgreSQL casts fail before catalog resolution. Native InfoSchema exposes real schema IDs. information_schema.tidb_trx exposes live native transaction start timestamps. Native databases do not have PostgreSQL template, owner or shared-description metadata.

## Decision Log

Use a PG-only catalog executor for these two supported introspection queries, with token-based full-query matching. Expose a generic read-only schema snapshot on the session context; shared modules must contain no PostgreSQL catalog names or SQL rewrites. Report unknown owner/description as NULL, native databases as non-templates. Native transaction IDs are TSO identifiers, not PostgreSQL 32-bit XIDs; oldest ordering uses native timestamp ordering. Date/author: 2026-10-01, Codex.

## Context and Orientation

pkg/server/pg_conn.rs dispatches simple queries; pg_extended.rs manages Parse/Bind/Describe/Execute/Close/Sync. pg_result.rs encodes native result metadata into PG OIDs. New pg_catalog.rs recognizes and evaluates catalog probes using TiDBContext's generic schema snapshot and live transaction view. Tests remain in separate files.

## Plan of Work

Add regression SQL fixtures and TCP coverage, then add the independent catalog executor. Keep engine-prepared statements unchanged, while PG catalog statements use their own statement identifiers and reevaluate data on Execute. Portal materialization preserves suspension and existing transaction cleanup. Document the supported query shapes and native-ID semantics.

## Milestones

First reproduce syntax failures. Next return live database rows and transaction rows with stable PG metadata in both protocols. Finally verify JDBC and all scoped PG tests, and self-review the diff.

## Concrete Steps

From repository root run cargo test -p astersql-server --lib datagrip_catalog --locked before and after implementation. Run cargo fmt --all, cargo test -p astersql-server --lib pg_ --locked, make lint and git diff --check for delivery. Use installed JDBC drivers against the real test TCP listener without adding dependencies.

## Validation and Acceptance

The exact database-list query reports created schemas, stable distinct IDs and current database first; dropped schemas disappear. The exact transaction query is empty when idle, reports a live transaction after BEGIN and DML, and becomes empty after rollback. Describe metadata matches Execute metadata and prepared catalog execution observes catalog changes after Parse. Token matching must preserve literals and reject modified or batched queries.

## Idempotence and Recovery

Tests use isolated in-memory domains and ephemeral TCP ports. Preserve preexisting working-tree edits and make no dependency changes. Reuse successful checks unless relevant changes invalidate them.

## Interfaces and Dependencies

TiDBContext::schema_snapshot returns Option<astersql_infoschema::SchemaRef>. ConcreteTiDBContext holds the canonical Domain to implement it. PG catalog plans supply their own typed metadata and execute read-only queries for transaction observations. No new external dependencies are needed.

## Outcomes & Retrospective

The exact regression failed with 42601 before wiring the catalog executor. The live metadata regression, all 26 PG tests, MySQL TCP lifecycle regression, installed JDBC 42.7.13/42.7.3 probes and make lint passed. Added cross-connection oldest-transaction coverage also passed: the first native TSO remains oldest while two transactions are active, then the second becomes visible after the first rolls back. Full DataGrip metadata-tree and RealTiKV distributed locks remain outside the verified scope.
