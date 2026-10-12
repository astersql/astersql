# 任务 1: 迁移 stale-read 时间注入

批次：【批次 1】 无

状态：未开始

目的：将 `injectNow`、`mockStaleReadTSO` 和 `assertStaleTSO` 的 Rust 生产者/消费者统一迁移到 AsterSQL key。

来源任务：用户指出 `pkg/session/runtime/control.rs` 中的 `github.com/pingcap/tidb/pkg/expression/injectNow`。

预计会话范围：仅处理 stale-read 时间基准和断言链路，可在一个聚焦会话中完成失败测试、实现与定向验证。

## 文件

- 修改：`pkg/session/runtime/control.rs`
- 测试：`pkg/session/runtime/session_test.rs`
- 测试：`tests/realtikvtest/txntest/stale_read_test.rs`

## 上下文

- `refresh_session_stale_read_ts` 和 `evaluate_stale_read_ts` 读取时间 failpoint；RealTiKV stale-read 测试使用单次 `1*return(...)` 同时验证 key 被消费。三个 key 必须保持尾部路径和数值单位不变。

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
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能代替行为验收证据。

## 测试计划

- 行为：AsterSQL `injectNow` 能确定性驱动 session stale read，并与 AsterSQL `assertStaleTSO` 匹配。
- 失败验证测试：先将 `pkg/session/runtime/session_test.rs` 的 `session_read_staleness_honors_mock_stale_read_tso` 或新增的窄测试切换到 AsterSQL key，生产代码未改时应因 failpoint 未消费或 TSO 不匹配而失败。
- 失败验证命令：`cargo test -p astersql-session session_read_staleness_honors_mock_stale_read_tso -- --exact`
- 预期失败原因：测试注册 AsterSQL key，旧生产代码仍读取 PingCAP key。
- 通过验证命令：`cargo test -p astersql-session session_read_staleness_honors_mock_stale_read_tso -- --exact`
- 模拟策略：使用项目现有真实 failpoint registry，不 mock session 时间计算。RealTiKV 案例留作交叉完整性验证。

## 步骤

1. 先把聚焦回归测试中的三个 key 切换为 AsterSQL 前缀，运行并保留失败证据。
2. 同步修改 `control.rs` 的生产读取点和 stale-read Rust 测试消费者。
3. 确认时间单位、TSO 位移和单次消费语义未改变。
4. 执行格式化和聚焦测试，审查差异只包含 key 迁移与必要回归断言。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-session session_read_staleness_honors_mock_stale_read_tso -- --exact`
- 可用 RealTiKV 时运行：`cargo test -p astersql-tests-realtikvtest-txntest --test stale_read_test TestSelectAsOf -- --exact`
- 预期：AsterSQL key 被消费，TSO 断言通过，旧 key 不再出现于这三个文件的活跃 failpoint 调用。
- 所需证据：预期失败输出、通过测试的退出码与有效测试数、槽位和 `CARGO_TARGET_DIR`、差异审查结果。

## 完成

完成时，`pkg/session/runtime/control.rs`、`pkg/session/runtime/session_test.rs` 和 `tests/realtikvtest/txntest/stale_read_test.rs` 的 stale-read failpoint 契约统一使用 `github.com/astersql/astersql/...`，且有 fail-before/pass-after 证据。

