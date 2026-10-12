# Rust failpoint 切换 AsterSQL 路径计划

目标：将 Rust 生产代码与 Rust 测试中实际参与匹配的 TiDB failpoint key 迁移为 `github.com/astersql/astersql/...`，并保持故障注入行为不变。

范围：包含 Rust 源文件中传给 `astersql_testkit_testfailpoint`、`testfailpoint`或 `failpoint` API 的运行时 key，包括生产注入点、单元测试和 RealTiKV 测试消费者。当前 Git 远端是 `github.com/astersql/astersql`，因此它是新 key 的权威前缀。

范围外：不改 Go 源码，不改 PingCAP Issue/PR 链接、Go 包溯源注释、工具的 Go module 解析样例和仅作对照的原始 Go 文本。

假设：Rust failpoint 注册表以完整字符串为精确 key；生产注入点和 Rust 测试消费者必须同步改名。Go 代码仍使用 PingCAP module path，不与本次 Rust key 命名绑定。

## 设计决策

- 采用直接迁移 Rust key 的方案，不在 failpoint helper 中增加隐式双向别名；这样可以暴露未同步的生产者/消费者，避免旧品牌 key 无期保留。
- 仅迁移参与 Rust 运行时 failpoint API 的 key；文档、链接、Go 包名和原始对照文本继续保留 PingCAP 路径。
- 使用原有 failpoint 名的尾部路径，仅把前缀 `github.com/pingcap/tidb` 改为 `github.com/astersql/astersql`，不改故障注入语义。

## 架构说明

- `pkg/testkit/testfailpoint/failpoint.rs` 是 Rust failpoint 统一封装，`enable`/`eval_string`/`inject` 都对 key 做精确匹配。
- `pkg/session/runtime/control.rs` 的 `injectNow`、`mockStaleReadTSO`、`assertStaleTSO` 形成同一 stale-read 测试契约，必须与 `tests/realtikvtest/txntest/stale_read_test.rs` 一起验证。
- 迁移是大范围精确字符串修改，主要风险是遗漏或误改非运行时引用，而不是算法或性能变化。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 在实施步骤前规划失败验证命令和预期失败。
- 将实施步骤限制在失败测试所证明的行为范围内。
- 测试真实行为，不验证模拟对象或实现细节。

## Progress

- [ ] 迁移 stale-read 时间注入契约。
- [ ] 迁移 session 其余运行时 failpoint。
- [ ] 迁移 executor/planner/statistics 的 Rust failpoint。
- [ ] 迁移 DDL/DXF/BR/服务器的 Rust failpoint。
- [ ] 迁移 RealTiKV 的三类测试消费者。
- [ ] 完成全仓精确审计和 Ready 验证。

## Surprises & Discoveries

- 初始搜索在 231 个 Rust 文件中发现 PingCAP 路径，但仅按“文件同时含 failpoint API 和 PingCAP 路径”缩小后仍有 91 个候选文件；说明不能全局文本替换。
- 当前工作树没有 `.agents/skills/tidb-verify-profile/SKILL.md`，执行时需按 `AGENTS.md` 的 Ready 契约和现有 `docs/agents/testing-flow.md` 选择最小验证集，并在终交付中记录该缺失。

## Decision Log

- Decision: AsterSQL failpoint 前缀使用 `github.com/astersql/astersql`。
  Rationale: 本地 `origin` 的 fetch/push 远端均为 `git@github.com:astersql/astersql.git`。
  Date/Author: 2026-10-12 / Codex
- Decision: 不提供旧 PingCAP key 别名。
  Rationale: 用户要求处理 AsterSQL 路径，精确迁移更能在测试中捕获漏改。
  Date/Author: 2026-10-12 / Codex

## Outcomes & Retrospective

未实施。完成后应记录迁移的活跃 key 数量、有意保留的 PingCAP 引用类别、聚焦测试和未能本地执行的 RealTiKV 验证。

## Validation and Acceptance

验收时，所有已迁移生产注入点都必须能由同名 AsterSQL key 激活，关联测试不得继续使用 PingCAP key。审计命令仍允许上游链接、Go 包注释和对照文本存在，但不允许 Rust failpoint API 的生效 key 留有 `github.com/pingcap/tidb/` 前缀。Rust 改动先执行 `cargo fmt --all`，然后按编号任务的 Cargo 共享槽位规则执行聚焦测试；最终代码交付执行 `make lint`。

