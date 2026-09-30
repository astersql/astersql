# Real libpq 3.2 client regression

This living ExecPlan follows PLANS.md. The overall plan.md is read-only.

## Purpose / Big Picture

Prove that an actual external PostgreSQL client can authenticate and execute CRUD, typed text parameters and transactions against the canonical Rust session over TCP.

## Progress

- [x] Read task, plan, PLANS.md, execution and navigation skills, testing guide; task 6 is completed and removed.
- [x] Select locally installed PostgreSQL libpq 18.0, with min_protocol_version=max_protocol_version=3.2. No Cargo dependency is added.
- [x] Run external-client failure regression: libpq 180000 refuses empty password challenge; nonempty probe is rejected by PG.
- [x] Authenticate empty native credentials through the real canonical driver before AuthenticationOk, per explicit dispatch authorization. Secure mode and other users still fail.
- [x] cargo fmt --all; 20 PG tests pass; exact external workflow, MySQL command regression and disabled/dual listener lifecycle pass; settled make lint exits 0 without diagnostics; git diff --check passes.

## Surprises & Discoveries

The current PG connection sends AuthenticationCleartextPassword, but canonical authentication accepts only root with empty credentials. libpq normally refuses to send an empty password for this challenge. This must be confirmed with the external library, not a synthetic PasswordMessage.

## Decision Log

- Decision: Call installed libpq through Python ctypes from a separate Rust test module.
  Rationale: Uses the genuine PostgreSQL client library for startup, query framing, parameter binding and cancellation without adding or copying an external Rust dependency. PG_LIBPQ_LIBRARY can select a system-installed version 18 library; missing prerequisites fail explicitly.
  Date/Author: 2026-09-30 Codex.

## Outcomes & Retrospective

Completed real libpq 18 regression with protocol 3.2, invalid user/version rejection, CRUD, explicit integer text binding, BEGIN/COMMIT/ROLLBACK and libpq variable-key idle cancellation. The same Server keeps an authenticated MySQL connection alive throughout and responds to COM_PING afterward. Adjacent cancellation, secure-mode rejection and listener shutdown tests pass. No production-password, TLS, binary-format or real-TiKV compatibility claim. Test requires installed Python 3 and libpq >=18; missing prerequisites are explicit failures.

## Context and Orientation

pkg/server/pg_conn.rs negotiates authentication; runtime.rs ConcreteTiDBContext::authenticate accepts only empty native credentials in InsecureRootOnly and rejects SecureUnsupported. pg_extended.rs translates indexed parameters through the existing canonical prepared API. Source and tests remain separate.

## Plan of Work

Register pg_client_integration_test.rs in lib.rs. Start a genuine PgService with ConcreteSessionDriver and a temporary TCP port. Load PostgreSQL libpq 18, require PQfullProtocolVersion 30002 (wire startup version 196610) and test text SQL, typed parameters, transactions and cancel connection. Stop at authentication gaps that cannot be repaired within the no-trust-fallback boundary.

## Concrete Steps

From repository root run cargo test -p astersql-server postgres_client_workflow --lib. After a permissible fix run cargo fmt --all, repeat the test, run adjacent PG/MySQL tests and make lint.

## Validation and Acceptance

The actual libpq library must connect with protocol 3.2, return expected rows after CRUD and commit/rollback, execute PQexecParams and send a real 3.2 cancellation request. MySQL coexistence and disabled PG regressions must also pass. A failed authentication regression is failure evidence only, never completion evidence.

## Idempotence and Recovery

Use ephemeral ports and canonical test domain. Close service before asserting subprocess outcome. No installed software, credentials or remote resources are changed.

## Artifacts and Notes

Client: PostgreSQL libpq 18.0 installed at /opt/homebrew/opt/libpq/lib/libpq.dylib. Official connection option reference: https://www.postgresql.org/docs/18/libpq-connect.html.

## Interfaces and Dependencies

Only Python standard-library ctypes and an installed libpq 18 library are required. No Cargo manifest or lockfile change. Test-only PG logic stays in pg_*.rs.

- Decision: Expand direct test whitelist to pg_conn_test.rs, pg_query_test.rs, pg_types_test.rs and pg_error_test.rs alongside pg_extended_test.rs to update their obsolete empty PasswordMessage handshake.
  Rationale: Existing tests consumed the old R3 challenge. Dispatch explicitly permits real identity validation followed by libpq-compatible authentication in InsecureRootOnly. The PG adapter calls existing authenticate with empty native credentials, emits R0 only after success, and neither changes runtime.rs nor admits other users. Nonempty/production passwords remain unsupported. No MySQL packet or kernel changes.
  Date/Author: 2026-09-30 Codex, explicit supplementary authorization.

Initial evidence: cargo test -p astersql-server postgres_client_workflow --lib exited 101 with fe_sendauth: no password supplied. PG_CLIENT_TEST_PASSWORD=1 same command exited 101 with password authentication is unsupported for nonempty credentials. After PG authentication adaptation, the external workflow passed (1 test, 6.82s). PQfullProtocolVersion uses 30002 rather than startup wire version 196610; corrected that test API assertion.

## Final validation evidence

Ready profile selected for code delivery; repository .agents/skills/tidb-verify-profile/SKILL.md is absent, so root AGENTS.md and testing-flow govern checks. No Go/Bazel changes or new dependencies were introduced; no bazel_prepare trigger. Rust PG tests do not require Go failpoint instrumentation.

Commands from repository root:

    cargo fmt --all
    cargo test -p astersql-server postgres_client_workflow --lib
    PG_CLIENT_TEST_PASSWORD=1 cargo test -p astersql-server postgres_client_workflow --lib
    cargo test -p astersql-server startup_auth_roundtrip --lib
    cargo test -p astersql-server pg_ --lib
    cargo test -p astersql-server postgres_listener_lifecycle --lib
    cargo test -p astersql-server mysql_protocol_connection_commands_match_mysql_80 --lib
    make lint
    git diff --check

First workflow command failed as expected before the fix; the temporary nonempty password probe also failed. Final exact workflow passes 1 test in 6.84s. PG suite passes 20 tests in 11.88s. Listener regression passes 1 test; MySQL regression passes 1 test in 6.69s. Formatting and diff checks exit 0. Lint during concurrent Cargo work emitted transient target traversal and macOS find diagnostics despite exit 0; after Cargo settled, make lint exited 0 without either diagnostic. Logs: /tmp/pg-task7-red.log, /tmp/pg-task7-password-red.log, /tmp/pg-task7-exact-final.log, /tmp/pg-task7-suite.log, /tmp/pg-task7-listener-final.log, /tmp/pg-task7-mysql-final.log, /tmp/pg-task7-lint-settled.log.

Boundary review: task edits are lib.rs test registration, pg_client_integration_test.rs, pg_conn.rs authentication adaptation and adjacent pg_conn_test.rs/pg_query_test.rs/pg_extended_test.rs/pg_types_test.rs/pg_error_test.rs handshake updates. No shared runtime/conn/MySQL source or Cargo manifest changes by this task. rg -n 'pg_' pkg/server/conn.rs pkg/server/runtime.rs pkg/session/runtime/session.rs has no matches. plan.md remains untouched. Root working tree contains unrelated changes from other tasks, which are preserved.

Correctness risk: the real client test covers text parameters and idle cancellation; active cancellation remains covered by the adjacent PG suite. Compatibility: authentication intentionally admits only identities validated by the current canonical empty-credential developer mode; production password authentication remains unsupported and secure mode rejects. Performance: no benchmarks; existing portal result materialization remains unchanged. Real TiKV, TLS and broad external client combinations were not verified.

Update (2026-09-30): completed final Ready evidence and retained the implementation ExecPlan; numbered task is removed on delivery per execution skill.
