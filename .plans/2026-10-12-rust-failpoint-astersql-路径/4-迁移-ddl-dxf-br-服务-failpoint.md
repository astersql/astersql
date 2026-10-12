# 任务 4: 迁移 DDL、DXF、BR 与服务 failpoint

批次：【批次 1】 无

状态：未开始

目的：迁移 DDL、分布式执行框架、BR 和服务器启动相关的活跃 Rust failpoint key。

来源任务：全仓类似位置分析。

预计会话范围：覆盖共享基础设施但修改类型单一；以 crate 为边界逐组验证，不包含 RealTiKV 消费者文件。

## 文件

- 测试：`pkg/ddl/tests/serial/serial_test.rs`
- 测试：`pkg/dxf/framework/integrationtests/framework_test.rs`
- 修改：`cmd/tidb-server/main.rs`
- 测试：`br/pkg/checkpoint/checkpoint_test.rs`
- 测试：`br/pkg/utiltest/syncpoint/parity_test.rs`
- 测试：`br/pkg/utiltest/syncpoint/syncpoint_test.rs`
- 修改：执行前通过 RustCodeGraph 确认上述消费者对应的 `pkg/ddl/**`、`pkg/dxf/**`、`br/**` 生产注入点精确文件，并只把已确认文件加入本任务。

## 上下文

- DDL 相关修改前必须阅读 `docs/agents/ddl/README.md`。RealTiKV 测试文件由任务 6、7 处理，避免并行文件冲突。

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

- 行为：AsterSQL key 可以触发 DDL/DXF/BR/服务启动的既有同步点和错误路径。
- 失败验证测试：选择一个无需外部集群的现有 checkpoint、syncpoint 或 DXF 测试，先迁移测试 key。
- 失败验证命令：`cargo test -p astersql-br-pkg-checkpoint checksum_flush_reports_go_post_write_failpoint -- --exact`
- 预期失败原因：测试与生产注入点 key 暂时不一致。
- 通过验证命令：`cargo test -p astersql-br-pkg-checkpoint checksum_flush_reports_go_post_write_failpoint -- --exact`
- 模拟策略：复用现有内存 fixture；需要真实 TiKV 的路径留给对应 RealTiKV 任务。

## 步骤

1. 阅读 DDL 入口文档并用调用/引用分析确认生产注入点。
2. 获取一个本地可运行用例的失败证据。
3. 成对迁移各 crate 的活跃 key，不修改 Go 包注释。
4. 格式化并分 crate 运行聚焦测试。
5. 审计剩余命中并记录保留理由。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-br-pkg-checkpoint checksum_flush_reports_go_post_write_failpoint -- --exact`
- 运行：`cargo test -p astersql-dxf-framework framework_executor_failure_is_returned_without_hiding_cleanup -- --exact`
- 运行：`cargo test -p astersql-ddl go_trace_helpers_preserve_test_identity_and_source -- --exact`
- 预期：每个实际修改 crate 的有效测试通过，生产者/消费者 key 成对。
- 所需证据：文档已读记录、fail-before/pass-after、退出码、有效测试数、槽位信息和差异审查。

## 完成

DDL/DXF/BR/服务范围内的活跃 Rust failpoint 使用 AsterSQL 路径，且没有误改 Go 来源信息。

