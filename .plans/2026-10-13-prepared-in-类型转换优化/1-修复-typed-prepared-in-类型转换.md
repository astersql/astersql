# 任务 1: 修复 typed prepared IN 类型转换

批次：【批次 1】 无

状态：已完成，待回归

待回归记录（2026-10-14）：

- 功能与交付检查已完成：新增 session 回归精确 1/1 通过，四个 Go 对齐端到端用例精确 4/4 通过，`cargo fmt --all` 与 `make lint` 均退出 0。
- TDD 红灯为 `int heap IN query must use typed prepared path`；修复后 typed path、参数重绑、plan-cache hit、schema generation 失效、NULL 与不可转换值均由独立回归覆盖。
- 为保留 10 worker × 100 组 `PREPARE/SET/EXECUTE` 的规模，同时处理剖析发现的重复解析边界，`PREPARE` 现在持久化已验证 AST；typed `EXECUTE` 复用该 AST，只有回退通用路径才重新解析绑定 SQL。没有缩减测试规模或绕过真实 session/planner/executor。
- 性能锁下五轮均为功能通过，但样本为 `0.612039, 0.623958, 0.618164, 0.622457, 0.590856` 秒，超过 `0.499` 秒门槛；报告位于 `target/rust-performance/prepared-in-decimal-after.tsv`。
- 计时期间宿主机 `load averages` 为 `16.06 15.17 16.69`，仓库外虚拟机进程约占 `634.6% CPU`，RustCodeGraph 进程另占约 `100.0% + 46.5% CPU`。这些进程不属于本任务且未被终止，因此按计划的“无关本次修改的原因导致无法运行验证”条款保留本文件，待空闲、独占性能环境复跑五轮门槛。
- 实际 Cargo 槽位为 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；性能锁已释放，Cargo 槽位在提交完成后释放并保留缓存。

目的：使 prepared `IN (?, ?)` 在 INT、VARCHAR、REAL、DECIMAL 列上通过通用 typed 路径保持 Go 结果，并消除 decimal 慢测瓶颈。

来源任务：`.plans/2026-10-12-rust-慢测极限优化/38-test_builtin_in_decimal_sig-46b91e26.md`

预计会话范围：一个聚焦会话内完成 typed prepared SELECT 的参数转换、`IN` 路由、四类型语义回归和 decimal 性能门槛；若必须转向 AST/plan 持久化，先在本任务文件记录剖析证据与新边界。

## 文件

- 修改：`pkg/session/runtime/dispatch.rs`
- 可能修改：`pkg/session/runtime/session.rs`
- 可能修改：`pkg/session/runtime/planning.rs`
- 可能修改：`pkg/executor/physical_plan.rs`
- 测试：`pkg/session/runtime_test/typed_adapter_bridge.rs`
- 测试：`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`

## 上下文

- `execute_simple_prepared_select_through_adapter` 目前只把参数解析为 i64，且 typed predicate 只覆盖等值索引。已有原型证明直接传字符串 Datum 会使 DECIMAL/REAL/VARCHAR `IN` 错返空集；转换必须根据目标列类型完成。
- Go 对齐规模为 4 表×100 行、10 worker×100 组 `PREPARE/SET/EXECUTE` 及全部结果断言；禁止减少规模或绕过真实 session/planner/executor。
- 执行前阅读 `pkg/planner/core/base/doc.go`、目标 Rust 源码及相邻测试；用 RustCodeGraph 复核修改符号的 callers/callees/impact。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 性能测量互斥规则

- 五次计时前用原子 `mkdir target/rust-performance/perf-audit.lock` 领取仓库级锁，记录任务、会话和 PID；持锁时不运行其他测试、lint、fmt 或高负载命令。
- 成功、失败或中断后均只清理自己的占用记录和锁目录；遗留锁必须确认持有进程已退出。

## 测试计划

- 行为：prepared `IN (?, ?)` 在无索引和有索引表上均正确处理 INT、VARCHAR、REAL、DECIMAL，参数变化、cache hit 和 schema generation 失效不改变结果。
- 失败验证测试：在 `pkg/session/runtime_test/typed_adapter_bridge.rs` 新增独立测试，覆盖四种列类型、NULL/不可转换值和缓存失效；保留 `builtin_func_test.rs` 四个现有端到端用例。
- 失败验证命令：`cargo nextest run --locked --package astersql-session --lib -E 'test(typed_prepared_in_coerces_parameters_like_go)'`
- 预期失败原因：现有 typed adapter 只接受 i64 且不路由 `IN`；直接传字符串 Datum 时数值与字符串列会错返空集。
- 通过验证命令：`cargo nextest run --locked --package astersql-session --lib -E 'test(typed_prepared_in_coerces_parameters_like_go)'` 和 `cargo nextest run --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'test(test_builtin_in_int_sig) | test(test_builtin_in_string_sig) | test(test_builtin_in_real_sig) | test(test_builtin_in_decimal_sig)'`
- 模拟策略：使用现有 mockstore 和真实 session/planner/executor 链路；不 mock 类型转换、计划缓存或表/索引扫描。

## 步骤

1. 阅读 Go 对应 prepared plan cache、Datum 转换及 `IN` range 构建代码，将参数类型、warning/error 和 NULL 语义映射到新回归测试。
2. 新增失败测试并运行精确命令，记录非零退出码与具体错误结果。
3. 使 predicate 识别通用支持 `IN` 的列与参数形状；根据表列 FieldType 构造相应 Datum，复用现有 Go 对齐转换函数而非自建简化 parser。
4. 打通无索引表扫描与索引范围的 typed `IN`；不支持的形状显式回退到原通用路径，不吞掉真实执行错误。
5. 验证 parameter shape、catalog/schema generation、事务上下文和 `last_plan_from_cache` 行为与现有 Go 契约一致。
6. 运行 `cargo fmt --all`，再运行两组精确功能测试。目标包未使用 failpoint，不需要启用 failpoint。
7. 预热后领取性能锁，运行 decimal 精确测试五次；任一次超过 0.499 秒则用剖析证据继续优化，不减少 Go 规模。
8. 按 Ready 交付要求运行 `make lint`，审查 diff 中的 Go 对齐依据、license、Rust 源文件顶部 AsterSQL 标记和无关共享工作区变更。

## 验证

- 格式化：`cargo fmt --all`
- session 回归：`cargo nextest run --locked --package astersql-session --lib -E 'test(typed_prepared_in_coerces_parameters_like_go)'`
- 端到端回归：`cargo nextest run --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'test(test_builtin_in_int_sig) | test(test_builtin_in_string_sig) | test(test_builtin_in_real_sig) | test(test_builtin_in_decimal_sig)'`
- 性能门槛：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/prepared-in-decimal-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_decimal_sig)'`
- Ready：`make lint`
- 预期：session 新回归精确 1 项通过；四个端到端用例全通过；decimal 性能测试 5/5 通过、零失败/超时且每次 `<=0.499s`；lint 退出 0。
- 所需证据：失败验证的具体错误，每条命令的退出码与有效测试数，性能 TSV 与五个 sample，实际槽位与绝对 `CARGO_TARGET_DIR`，以及无语义弱化的 diff 自审结果。

## 完成

- 生产修复以 `pkg/session/runtime/dispatch.rs::execute_simple_prepared_select_through_adapter` 为入口，可按实际调用链修改 planning/executor，但必须为通用 prepared SELECT 能力。
- 完成时必须同时满足四类型结果正确、cache/invalidation 回归通过、decimal 五次性能门槛和 Ready lint；不得用零测试、单次最佳值或 fallback 路径假冒 typed 修复。
- 获得全部证据后将状态改为“已完成”，使用 `$git-commit` 只提交本任务变更并删除本任务文件；无法达到时保留文件并记录阻塞证据。
