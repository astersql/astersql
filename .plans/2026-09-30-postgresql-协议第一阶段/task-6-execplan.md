# Extended query and text binding

This living ExecPlan follows repository PLANS.md; plan.md remains read-only.

## Purpose / Big Picture

Explicit PostgreSQL 3.2 clients can prepare named/unnamed SQL, bind text/NULL values, describe results, execute portals, close objects and recover with Sync. PostgreSQL state and conversions live in pg_extended.rs. No parameter value is inserted into SQL by the adapter.

## Progress

- [x] Read skill, task, total plan, PLANS.md, root AGENTS.md and testing-flow; preceding task 5 implemented pending regression is accepted per dispatch authorization.
- [x] Inspect real shared prepare/execute boundary and obtain TCP red evidence via temporary independent harness.
- [x] Implement initial PG state machine, indexed marker mapping and original prepared metadata bridge.
- [x] Complete behavioral tests: table parameters, text injection isolation, NULL updates, repeat/out-of-order bindings, named/unnamed lifecycle, suspension and Sync recovery, plus fixed boolean constants.
- [x] Format, scoped PG/MySQL checks, Ready lint, boundary review and cleanup temporary harness; final exact test and library check pass.

## Surprises & Discoveries

The canonical prepare interface treats SELECT $1 as an unknown column, whereas SELECT ? prepares with one parameter. Initial real TCP Parse returns 0A000 rather than ParseComplete. Exact library command currently cannot compile because tests/mysqlcompat/compatibility-cases.json disappeared during concurrent work. Independent harness compiles library normally and preserves every existing test registration.

## Decision Log

- Decision: Extend task whitelist to conn.rs and runtime.rs for original engine prepared metadata, plus temporary Cargo test registration solely to run around unrelated missing fixture.
  Rationale: PreparedMetadata.columns loses boolean flags just like the task 5 query boundary; descriptions must use credible native engine types. No PG data enters shared types. Temporary target will be removed.
  Date/Author: 2026-09-30 Codex; explicit scope expansion authorized by dispatch.
- Decision: Adapt indexed markers solely in pg_extended.rs, preserving strings/comments and occurrence mapping; never interpolate argument values.
  Rationale: Dispatch explicitly authorized this adaptation after the syntax mismatch was reported. Shared parser and SQL semantics remain unchanged.
  Date/Author: 2026-09-30 Codex.
- Decision: Reject unknown OIDs/binary formats/type inference rather than invent parameter metadata.
  Rationale: Current shared interface has no inferred parameter type contract. Text integer, float, boolean and string parameters have lossless BinaryParam representations.
  Date/Author: 2026-09-30 Codex.

- Decision: Preserve the original prepared types and column names for parameter-free statements whose executed record set lacks native fields.
  Rationale: New real TCP SELECT TRUE regression failed with metadata count mismatch, then name mismatch. Prepare metadata is engine-derived and fixed for these statements; using it at the PG boundary avoids inferring parameter-dependent types or modifying shared SQL semantics. /tmp/pg-task6-literal-red.log records red evidence. The final exact test covers the fix.
  Date/Author: 2026-09-30 Codex.

## Outcomes & Retrospective

Implemented and validated text extended queries. Final exact test and library check pass in the original working tree. PG suite has 19 passing tests; MySQL listener, type packets and concrete-session regression each pass. Ready lint passes. No new dependency, MySQL packet edits, or execution-kernel SQL changes. The numbered task is deleted after recording completion evidence.

Compatibility limits remain explicit: parameter OID inference, binary parameter/result formats, timezone timestamps, empty prepared SQL and parameter-dependent projection metadata without native engine types are unsupported. Explicit text parameter OIDs support boolean, integers, floats, text, decimal, hex bytea, date/time and wall-clock timestamp. No default tokio-postgres binary-format compatibility claim is made. Portals materialize complete results before paging; performance benchmarks and real TiKV were not run.

## Context and Orientation

pkg/server/pg_conn.rs owns authentication and independent TCP message loop. TiDBContext in conn.rs is the shared execution interface. runtime.rs serializes requests to the real session worker. PreparedMetadata needs original engine type data for PG Describe. A portal is one named binding and cached result stream; a statement is prepared SQL with indexed-to-positional mapping.

## Plan of Work

Add pg_extended.rs with bounded message parsing, statement/portal registries, text OID conversion and Sync recovery. Register source and separate pg_extended_test.rs in lib.rs. pg_conn calls the adapter before its simple Query handler; Execute uses the existing cancellation registry. conn.rs adds protocol-independent native prepared type metadata; runtime.rs projects it from existing result fields without altering MySQL ColumnInfo.

## Concrete Steps

From repository root run cargo test -p astersql-server parse_bind_execute_sync --lib. If unrelated missing fixture blocks compilation, temporary pg_task6_probe test target uses /tmp/pg-task6-harness.rs and public initialized driver constructor; run cargo test -p astersql-server --test pg_task6_probe -- --nocapture. Remove exact added target at finish. Run cargo fmt --all before tests, cargo check -p astersql-server --lib, scoped PG/MySQL tests and make lint.

## Validation and Acceptance

TCP red: Parse yields ErrorResponse 0A000 (log /tmp/pg-task6-harness.log before implementation). Green must prove Parse/Bind/Describe/Execute/Close/Sync, named/unnamed statements, NULL, repeat bindings, suspended portals and error skipping until Sync. Marker regression tests cover quoted markers, comments and repeated/out-of-order references. Keep existing MySQL regressions in scope for shared metadata changes. Verify no pg_ imports in shared modified modules.

## Idempotence and Recovery

Tests use ephemeral TCP ports and real in-memory session domain. Close sockets/services on normal completion. Never create an empty missing fixture or disable existing modules. Remove only this task's temporary registration; preserve concurrent modifications.

## Artifacts and Notes

/tmp/pg-task6-red.log: exact lib test blocked by missing fixture. /tmp/pg-task6-harness.log: genuine TCP red plus shared syntax probe passing. pkg/server/doc.go and .agents/skills/tidb-verify-profile are absent. Rust failpoints use dynamic runtime injection, not Go rewriting; no Go/Bazel changes trigger bazel_prepare.

## Interfaces and Dependencies

No new dependency. Extended calls TiDBContext prepare_statement, execute_prepared_statement and close_prepared_statement. BinaryParam remains unchanged. PreparedMetadata.native_types is Vec<NativeType>, consistent with QueryResult native metadata and containing only engine code/flags/length/decimal.

## Final validation evidence

Profile: Ready, because repository code delivery requires formatting, scoped regressions and make lint. Missing verify-profile skill was handled by following root policy/testing-flow directly. No Go/Bazel/dependency change was made by this task; no bazel_prepare trigger. Rust failpoints are dynamic and the touched code needs no Go enable/disable rewriting.

Exact commands and outcomes:

    cargo test -p astersql-server parse_bind_execute_sync --lib
    cargo test -p astersql-server canonical_prepare_parameter_syntax_probe --lib -- --nocapture

Initial commands exited 101 only because the unrelated MySQL fixture vanished. Temporary safe alternative:

    cargo test -p astersql-server --test pg_task6_probe -- --nocapture

Initial alternative exited 101 with actual Parse ErrorResponse 0A000 (1 failed, 1 passed), then passed after initial implementation (2 passed). One later temporary harness extraction attempt failed to compile due to missing root alias; it was abandoned when the fixture returned. No fixture was fabricated, and no existing tests were disabled. Temporary Cargo target removed; /tmp harness source removed at completion.

    cargo fmt --all
    cargo test -p astersql-server pg_ --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-server mysql_type_packets_expose_correct_type_flags_charset_and_decimal --lib
    cargo test -p astersql-server concrete_session_driver_authenticates_and_returns_real_sql_results --lib
    make lint
    cargo test -p astersql-server parse_bind_execute_sync --lib
    cargo check -p astersql-server --lib
    git diff --check

Final outcomes: all exit 0. PG 19 pass (/tmp/pg-task6-final-pg2.log); each MySQL/shared targeted test 1 pass (/tmp/pg-task6-mysql-listener.log, /tmp/pg-task6-mysql-types.log, /tmp/pg-task6-runtime.log); exact final TCP test 1 pass (/tmp/pg-task6-final-exact.log); library check (/tmp/pg-task6-final-check.log), lint (/tmp/pg-task6-lint.log). A first compile check exposed an incorrect decimal module path, repaired to existing astersql_types::decimal::mydecimal. The strengthened boolean test failed before its PG metadata repair (two actual failures, metadata count and name consistency), then passed in final tests.

Boundary checks:

    rg -n 'pg_' pkg/server/conn.rs pkg/server/runtime.rs
    rg -n 'pg_task6_probe|/tmp/pg-task6' pkg/server/Cargo.toml pkg/server/lib.rs
    git diff -- '.plans/2026-09-30-postgresql-协议第一阶段/plan.md'

No matches/diff. Task changes: pg_extended.rs, pg_extended_test.rs, pg_conn.rs, lib.rs, PreparedMetadata.native_types in conn.rs and original prepared metadata projection in runtime.rs, this ExecPlan and task deletion. Cargo.toml retains only pre-existing concurrent modifications. Shared types contain no PG OID or wire message fields; PG-specific code stays in pg_ files. Existing copyright comments are preserved. Self-review accounted for concurrent dirty files and did not revert them.

Not verified: complete workspace suites, real TiKV, default PG drivers using binary formats, OID inference, full SQL compatibility, performance/memory benchmarks. Correctness risks are constrained by explicit errors for unavailable types/formats and prepared/executed metadata disagreements. Compatibility is first-stage text-protocol support. Performance uses cached materialized portal responses and typed parameter vectors; no claim about large results.

Updated at completion on 2026-09-30: recorded final red/green evidence, fixed constant metadata handling and preserved the authorized boundaries.
