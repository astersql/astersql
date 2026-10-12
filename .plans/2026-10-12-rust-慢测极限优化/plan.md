# Rust 慢测极限优化计划

目标：逐项复核并优化 `target/rust-test.4uY80L` 中所有耗时大于等于 0.5 秒的唯一 Rust 测试，使每项在预热、单线程、五次独立测量中均小于 0.500 秒，同时保持 Go 对齐逻辑、测试规模和真实行为。

范围：按日志最大观测耗时降序覆盖 2,284 个唯一测试身份；包括 1,843 个 lib target、405 个 test target 和 36 个 bench target，涉及 198 个 Cargo package、444 个 Rust 源/测试文件。每个测试独立一个编号任务，包含原始耗时、状态、源码位置、精确 nextest 过滤器、性能门槛和 Ready 交付检查。

范围外：日志中小于 0.5 秒的测试、7 个小于阈值但失败的 Rust 测试、Go 既有失败，以及通过删案例、缩小 Go 对齐数据规模、改用不等价 mock、放宽超时或降低 benchmark 工作量获得的表面提速。

假设：源日志 SHA-256 为 `d56cb051199f5fcfff8acb47177f2c4cae57254acb4c698a07f092874dea405c`；它来自 `make rust-test` 的 `--workspace --all-targets` 并发运行，因此不是纯 `--lib` 单元测试，也不是可靠的隔离性能基线。编号任务会用仓库现有 `tools/check/rust-test-performance.sh` 的 `perf-audit` profile 先预热编译，再单线程重复五次。

## 设计决策

- 方案一是直接按全量并发耗时改代码，速度快但会把 CPU、I/O 和共享服务争用误判成测试本身开销；不采用。
- 方案二是按 package 合并任务，文件少但违背“一项测试一个任务”，也难以给出逐项完成证据；不采用。
- 采用“一项唯一 binary/test 身份一个任务”：原始日志只负责排序与入选，真正修改以五次预热隔离基线和剖析证据为准。
- 严格目标是每次 `<0.500s`，命令使用 `--max-seconds 0.499` 留出计时精度余量；五次中任一次越线、失败、超时或零测试均不得完成。
- 2,284 个任务设为 2,284 个线性批次。虽然 Cargo 有 10 个共享槽位，但 `cargo fmt --all`、`make lint`、真实 TiKV/etcd 端口、共享源码和主机性能测量会相互干扰；顺序执行是可重复测量所需的资源边界。

## 架构说明

- `Makefile` 的 `rust-test` 使用 cargo-nextest 执行 workspace 的 `--all-targets`；`.config/nextest.toml` 的默认 profile 并行运行，`perf-audit` profile 则单线程运行。
- `tools/check/rust-test-performance.sh` 负责预热、重复测量、零测试/失败/超时检查和 TSV 证据输出；每个任务用 binary id 与完整 test name 双重精确过滤。
- 入选测试中 2,271 项通过、5 项失败、8 项超时；耗时层级为：53 项 `>=5s`、319 项 `>=2s`、1,466 项 `>=1s`、全部 2,284 项 `>=0.5s`。
- 初步热点分型为：1,682 项初始化/完整 Domain 或 TestKit 夹具，381 项外部服务，71 项算法或 I/O，63 项组合规模，51 项固定等待，36 项 benchmark harness。分型只是剖析起点，不是允许预设修改的结论。
- 重点证据：实例计划缓存的 Go/Rust 对齐并发用例包含 2,000 条语句和 10 个 worker；统计用例显式构造 100,010 行、7 个分区；planner golden 用例常同时运行 standard/cascades。上述规模与双路径覆盖不得删减。
- RealTiKV 任务必须按 `docs/agents/testing-flow.md` 启停真实 playground 并清理；DDL 任务必须先读 `docs/agents/ddl/README.md`，验证 job、schema state、reorg、回滚和 schema sync 语义没有被性能改动破坏。

## 开发策略

- 每项先运行原测试取得隔离基线；若五次已全部小于 0.500 秒，则把它认定为全量并发噪声并停止改代码；若基线仍越线，对于日志中的 FAIL/TIMEOUT 先恢复正确通过，再做性能优化。
- 用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact；在测试的 setup、fixture/load、主体、teardown 四段做临时时间分解，提交前删除诊断插桩。
- 优先消除重复且语义无关的初始化、重复解析、忙等/固定 sleep、重复磁盘往返和不必要分配；只缓存不可变数据，任何共享 Domain/Session/全局状态复用都必须证明隔离与清理。
- 不减少 SQL、case、并发 worker、行数、分区数、压缩格式、planner 模式、真实服务边界或断言；Rust 测试逻辑和生产逻辑继续尽可能对齐 Go。
- 若真实租约、网络、DDL propagation 或 benchmark 采样存在不可消除的 `>=0.5s` 下界，任务必须以 `已阻塞` 记录剖析和五次数据，不能通过弱化语义达标。
- Rust 源与测试逻辑保持分文件；真正可用的 Rust 生产代码顶部补 `// Copyright 2026 AsterSQL.`，保留原 PingCAP Apache License；外部 Rust 依赖不做本地 vendor/patch。
- Rust 代码修改后先 `cargo fmt --all`，再做精确性能验证与 `make lint`；默认不触发 `make bazel_prepare`，除非执行中实际满足 AGENTS.md 的 Bazel trigger。

## 验证与完成标准

- 每项 `before` 报告必须精确选中 1 个测试。非零退出应明确归因为耗时超过 0.499 秒、失败或超时；若五次已全部通过且最大值 `<=0.499s`，则以该证据判定原日志为并发噪声，不得为制造优化 diff 而改代码。
- 每项 `after` 报告必须有 5 个成功 sample、零失败、零超时，最大值 `<=0.499s`，且原有行为断言完整通过。
- 每项交付必须记录实际 Cargo 槽位、绝对 `CARGO_TARGET_DIR`、命令、退出码、有效测试数量、五次耗时和 diff 自审结论。
- 代码任务使用 Ready profile：`cargo fmt --all`、任务内精确性能命令和 `make lint`。只做基线复核且无需源代码修改时，仍记录五次证据并按任务执行协议处理状态。
- 所有任务完成后再运行一次 `make rust-test` 做 workspace 回归并解析新日志；由于范围外仍有 7 个原始短时失败，必须逐一确认最终非零退出是否仅来自这些已记录的范围外项，不能把新的失败归为既有问题。

## 风险与边界

- 正确性风险最高：共享夹具和缓存可能泄漏 schema、session variable、全局 registry、时钟或随机状态；必须有显式 reset/teardown 证据。
- 兼容性风险中等：Go golden、DDL 状态机、TiKV/PD 真实交互和协议测试不能被 mock 替代。
- 性能风险中等：全量并发日志含争用噪声，隔离变快不保证全量墙钟时间同步下降；最终 workspace 回归用于发现该差异。
- 交付规模风险高：2,284 个顺序提交会长期占用执行时间，但这是“一测试一任务”、可归因性能证据和共享资源互斥共同决定的结果。
