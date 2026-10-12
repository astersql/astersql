# txntest 单测性能优化计划

目标：将日志 `target/rust-test.NgV3pq` 中 txntest 最终耗时大于等于 1 秒的 18 个 Rust 测试逐项优化到严格小于 1 秒。

范围：仅处理 `astersql-tests-realtikvtest-txntest` 的 18 个热点；每个测试独立任务，保持 Go 测试语义、断言和真实事务行为，允许做最小必要的测试夹具及生产路径优化。

范围外：日志中其他 crate 的慢测试、Go 既有回归，以及与目标测试无关的子系统补建。

假设：日志中的最终 `PASS`、`FAIL`、`TIMEOUT` 行是来源基线；重复 `SLOW` 行不重复计数。性能验收采用单线程预热后三次独立测量，三次均须小于 1 秒。

## 设计决策

- 按来源日志最终耗时降序编号；用户确认 `1.000s` 也属于优化范围。
- 采用现有 `tools/check/rust-test-performance.sh` 与 `perf-audit` profile，使用 `--max-seconds 0.999` 表达“严格小于 1 秒”。
- RealTiKV 测试共享全局夹具、配置和验证资源，18 个任务均串行执行；每个任务只保留其 Go 对齐场景，不以删断言、缩减组合或 mock 真实事务路径换取速度。
- `TestSelectAsOf` 基线为失败；该任务先恢复与 Go 对齐的正确行为，再做性能验收。

## 架构说明

- Rust 测试 crate 入口为 `tests/realtikvtest/txntest/Cargo.toml`，各测试二进制由其中的 `[[test]]` 声明。
- Rust 测试与同名 Go 文件并列；优化前对照 Go 用例，优先消除 Rust 端固定 sleep、重复初始化和不必要的串行准备，但不得改变可观察行为。
- 测量脚本先编译预热，再使用 nextest 的单线程 `perf-audit` profile运行三次并输出 TSV 证据。
- 所有实现必须遵守仓库的 Cargo 共享槽位、RealTiKV 生命周期、`cargo fmt --all`、Ready 验证和差异自审要求。

## 开发策略

- 性能任务以“基线复现—剖析—最小优化—三次性能回归”为闭环；现有测试本身就是行为回归测试，因此不额外把测试逻辑塞入生产源文件。
- 先记录单测独立预热后的失败基线（耗时大于等于 1 秒，或功能失败），再用剖析证据定位固定等待、重复初始化、算法热点或真实 I/O。
- 生产修改须有对应 Go 行为或不可缺少的局部接线依据；不得删减 Go 覆盖的组合、断言、错误路径或事务语义。
- 每个任务完成前运行 `cargo fmt --all`、精确行为测试、三次性能审计，并按 Ready profile 执行适用交付检查。

## Progress

- [x] 2026-10-12：解析来源日志，按最终结果去重并识别 txntest 的 18 个耗时大于等于 1 秒测试。
- [ ] 依次完成 18 个编号任务，并为每项保存失败基线、通过结果和三次耗时。
- [ ] 完成全部任务后的 txntest 聚焦回归及 Ready 交付检查。

## Surprises & Discoveries

- Observation：`TestSelectAsOf` 在 1.200 秒处为功能失败，不只是性能超限。
  Evidence：`FAIL [   1.200s] ... stale_read_test TestSelectAsOf`。
- Observation：`TestStaleReadAllCombinations` 中存在两次显式 3 秒 sleep，是首要剖析候选，但必须保留 Go 的时间边界语义。
  Evidence：`tests/realtikvtest/txntest/stale_read_test.rs` 的该测试包含两处 `Duration::from_secs(3)`。

## Decision Log

- Decision：本计划范围限定为错误行所属的 txntest crate，而不是整份 workspace 日志的所有 crate。
  Rationale：用户给出的失败目标是 `astersql-tests-realtikvtest-txntest`，且要求一个单测一个任务；该 crate 中可形成可执行、可独立验收的 18 项清单。
  Date/Author：2026-10-12 / Codex
- Decision：所有批次串行。
  Rationale：测试共享真实 TiKV、全局配置和串行夹具；并行会污染性能测量和状态。
  Date/Author：2026-10-12 / Codex

## Outcomes & Retrospective

计划阶段尚未实施。完成时应记录 18 个测试的优化后最大耗时、行为回归结果、仍存在的环境噪声及任何范围外热点。

## Context and Orientation

“独立预热”指先完成编译，再单线程运行唯一目标测试，排除编译耗时与其他测试竞争。“严格小于 1 秒”通过脚本参数 `--max-seconds 0.999` 执行。每个任务的 Rust 文件与 Go 对照文件已写入编号任务；实现者必须先理解同名 Go 场景，再决定优化测试夹具还是底层生产路径。

## Plan of Work

依次执行编号任务。每项先复现并保存独立性能基线；如果基线已稳定低于 1 秒，仍需用三次审计证明且不得做无依据修改。若超限，使用计时或剖析定位占比最高的等待/初始化/算法路径，实施保持 Go 语义的最小变更。每项完成后提交，下一项基于前项提交继续，最终对整个 txntest crate 做聚焦回归。

## Concrete Steps

从仓库根目录按 `prompt.md` 的批次执行。精确行为命令统一使用：

    make rust-test-diagnose-one PACKAGE=astersql-tests-realtikvtest-txntest BINARY_ID='astersql-tests-realtikvtest-txntest::stale_read_test' TEST_NAME='TestStaleReadAllCombinations'

精确性能命令统一使用：

    tools/check/rust-test-performance.sh --max-seconds 0.999 --runs 3 -- --locked --package astersql-tests-realtikvtest-txntest -E 'binary_id(=astersql-tests-realtikvtest-txntest::stale_read_test) & test(=TestStaleReadAllCombinations)'

## Validation and Acceptance

每个测试必须功能通过且性能审计三次均执行恰好一个有效测试、退出码为 0、三次耗时均小于 1 秒。不得以编译成功、零测试、删场景或只跑一次作为证据。全部任务完成后运行 txntest crate 的完整 Rust 测试和适用 Ready 检查；RealTiKV 资源必须按仓库流程启动与清理。

## Idempotence and Recovery

测试、预热、性能审计和格式化可重复运行。性能失败时保留 TSV 报告用于比较；中断时释放本人 Cargo 槽位锁并清理本人启动的 playground，不能删除他人锁、进程或缓存。源码修改由每任务独立 Git 提交提供恢复边界。

## Artifacts and Notes

来源日志：`target/rust-test.NgV3pq`。热点排序、基线状态和精确过滤器写在各编号任务文件中；`prompt.md` 提供逐任务可复制执行提示。

## Interfaces and Dependencies

测试包为 `astersql-tests-realtikvtest-txntest`；性能工具为 `tools/check/rust-test-performance.sh`；nextest 配置为 `.config/nextest.toml` 的 `profile.perf-audit`。不得新增本地 `[patch]`、vendor 或 third_party 外部 Rust 依赖。

