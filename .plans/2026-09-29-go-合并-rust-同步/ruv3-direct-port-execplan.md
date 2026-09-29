# Direct Go RU v3 port into Rust executor

This ExecPlan is a living document. Keep its progress, discoveries, decisions, and outcomes current. Reference: repository root `PLANS.md`. It coordinates tasks 187, 195, 197, and the dependent task 20; `plan.md` remains read-only.

## Purpose / Big Picture

The Rust executor must calculate and publish statement RU using the same evidence, formulas, eligibility rules, and terminal ordering as the current Go implementation. A successful statement should produce the same total and per-engine RU as Go for supported plans. Unsupported or incomplete terminal cases must fail closed rather than report a simplified estimate as complete.

## Progress

- [x] (2026-09-29) Trace Go `statement_ru_plan_walk.go`, `statement_ru_reporting.go`, and `statement_ru_result.go` to the Rust model and executor entry points.
- [x] (2026-09-29) Port and test write snapshots, scan evidence classification, configured weights, EXPLAIN prefix handling, engine formulas, full report value storage, calculator finalization, terminal response bytes, owner outcome/EOF, flat-tree shape, sort work, and unit-delta validation.
- [ ] Port every Go physical-operator branch and its runtime evidence checks to Rust's full physical-plan representation, including MPP, CTE, joins, aggregation, readers, point lookups, and writes.
- [ ] Preserve the full executed plan and required runtime counters through Rust `ExecStmt`; classify eligibility and unwrap Execute/EXPLAIN ANALYZE as Go does.
- [ ] Connect owner installation, session outcome, root EOF, exactly-once finalization, TopSQL total, resource-group report, calibration and Prometheus publication.
- [ ] Migrate all old RUv2 caller paths; delete old APIs only after equivalent Go behavior is live and validated.
- [ ] Run scoped Rust tests, SQL integration validation where applicable, `cargo fmt --all -- --check`, `make lint`, and diff review on the final state.

## Surprises & Discoveries

- The current Rust `pkg/executor/adapter.rs` `PlanInfo` is a summary. Go's RU walk reads typed physical operators and flattened child indices. Rust `pkg/planner/core/flat_plan.rs` has a typed `FlatPhysicalPlan`, but the current `ExecStmt` does not retain it. Production RU cannot use only `PlanInfo` without losing Go evidence.
- Rust `pkg/executor/compiler.rs` carries `Box<dyn base::Plan>` to `CompilerDependencies::BuildExecStmt`, but no concrete `BuildExecStmt` implementation is currently found in repository Rust sources (`rg -n 'BuildExecStmt|ExecStmtBuildInput' pkg -g '*.rs'`). The plan retention bridge must be traced through the session's actual statement builder before connecting publication; adding a default to `PlanInfo` would invent missing evidence.
- Rust has two planner representations relevant here: `pkg/planner/core/common_plans.rs::PlanNode` (used by its `FlatPhysicalPlan`) and `pkg/planner/core/base/plan_base.rs::Plan` with typed `physicalop` structures (used by `CompilerOptimizeResult`). Go's flat operators hold the typed physical plan itself. Porting the entire Go walk against only the current `PlanNode` would lose fields such as `PhysicalProjection.Exprs` and `PhysicalTableReader.ReadReqType`; the representation and build bridge must be unified before production wiring.
- The main checkout's executor tests currently stop at two unrelated planner `FullJoin` non-exhaustive matches. Isolated test worktree `ruv3-result-validation` has temporary explicit errors for those matches and two unrelated session `Option<ExprNode>` type fixes. These changes are validation-only and must not be delivered as Go-equivalent implementations.
- Rust `tikvutil::CommitDetails.WriteKeys` and `WriteSize` are `u64`; Go converts these to `int64`, so Rust `as i64` matches the overflow behavior.

## Decision Log

- Decision: Port the Go functions and their supporting structures directly. Do not use a reduced formula or publish RU from the old `PlanInfo` summary. Rationale: the user explicitly rejected a substitute path, and Go's branch evidence cannot be reconstructed from a summary. Date/author: 2026-09-29, Codex.
- Decision: Keep new RU v3 calculation code unconnected to statement publication until its typed plan and terminal evidence are complete. Rationale: premature use would silently undercharge and misreport support. Date/author: 2026-09-29, Codex.

## Outcomes & Retrospective

The direct Go port is in progress. Fourteen focused tests pass in an isolated worktree; the production plan walk and publication chain remain to be implemented. Update this section at each milestone with exact test counts and remaining behavior.

## Context and Orientation

Go source lives in `pkg/executor/statement_ru_plan_walk.go` (operator traversal and owner), `pkg/executor/statement_ru_reporting.go` (engine attribution and metrics), and `pkg/executor/statement_ru_result.go` (setup, scalar calculator, final result). Rust target files with matching names are beside `pkg/executor/adapter.rs`. `pkg/resourcegroup/ruv2/model.rs` already implements the raw `StmtUnits` and `calculate` weight formula. `pkg/util/execdetails/ruv2_metrics.rs` still contains compatibility APIs for current Rust callers; task 20 removes those only after the Go RU path is complete.

## Plan of Work

First finish the typed physical operator walk in `statement_ru_plan_walk.rs` against actual Rust planner and runtime stats types. Add each Go operator family and its failing then passing Rust regression in the separate test file. Extend the Rust flat-plan or runtime evidence types where Go reads fields currently absent; do not infer missing values from estimates. Then add the Go statement classification and owner lifecycle to `adapter.rs`, keeping exact success, failure, restricted SQL, cursor, and EOF conditions. Complete full-report metric publication and resource-group reporting. Finally remove obsolete RUv2 APIs and update every caller and adjacent test.

## Concrete Steps

Run commands from the repository root. For each group, read the named Go function and nearest Go tests, add `go_merge_187`, `go_merge_195`, or `go_merge_197` Rust tests in separate files, observe a failing result, implement, and rerun. Current focused command is `cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_1`; expected successful output is all new tests passing. Before delivery run `cargo fmt --all -- --check`, `make lint`, and `git diff --check`. For any changed Go imports, added Go test functions, Bazel metadata, or fresh checkout, apply the `make bazel_prepare` gate in `AGENTS.md` first.

## Validation and Acceptance

Acceptance requires a real statement result to receive the same supported Go RU units, total RU, engine split, TopSQL total, metrics, and resource-group report for read, write, point-get, MPP, and analyzed plans. Error, incomplete, and unsupported paths must publish no successful RU result. Rust unit tests must cover the per-operator branches and terminal lifecycle; integration tests must cover observable SQL behavior. Isolated worktree compile fixes prove only the target code; repeat in the main checkout after upstream planner/session fixes land.

## Idempotence and Recovery

The new Rust modules remain additive until production integration is complete. Rerun targeted tests after each edit. Preserve existing RUv2 APIs while callers remain. The isolated worktree's temporary patches must be removed by archiving that managed worktree after validation; never copy them to the main checkout.

## Artifacts and Notes

Isolated command `CARGO_TARGET_DIR=target cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_1` reached 14/14 passing after the latest additions. Source and test changes are in `pkg/executor/statement_ru_{plan_walk,reporting,result}.rs` and their separate `_test.rs` files.

## Interfaces and Dependencies

`StatementRUCalculator` owns `StmtUnits`, per-engine compute units, and optional bounded full report. `StatementRUOwner` owns first-outcome and root-EOF state and consumes setup once. `statement_ru_plan_walk.rs` must read a typed `FlatPhysicalPlan` and runtime snapshots, not the summary `adapter::PlanInfo`. Existing `pkg/resourcegroup/ruv2/model.rs::calculate` remains the shared RU weight formula. No external Rust dependency changes are planned.
