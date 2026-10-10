# 任务 8: 收敛 RealTiKV DDL 与 paging 超时

批次：【批次 3】 依赖批次 1、2；与任务 9 串行

状态：未开始

目的：优化外键自动建索引和 paging process-keys 用例，保留真实 TiKV、DDL 传播和分页统计语义。

来源任务：`test_add_foreign_key_with_auto_create_index`、`test_paging_act_rows_and_process_keys`。

预计会话范围：共享一次受控 playground，但分别修改 addindextest/sessiontest；不得与其他 RealTiKV 任务并行。

## 文件

- 测试：`tests/realtikvtest/addindextest/add_index_test.rs`
- 测试：`tests/realtikvtest/sessiontest/paging_test.rs`
- 修改：仅任务 1/采样证明的生产拥有文件

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

## 测试计划

- 行为：外键缺索引时自动创建并生效；paging 的 act rows/process keys 与真实 TiKV 返回一致。
- 失败验证命令：按 `docs/agents/testing-flow.md` 启动带唯一 tag 的 tikv-slim playground 后运行 `cargo nextest run --locked --package astersql-tests-realtikvtest-addindextest --package astersql-tests-realtikvtest-sessiontest -E 'test(=test_add_foreign_key_with_auto_create_index) | test(=test_paging_act_rows_and_process_keys)' --no-capture`。这两个 crate 将用例编译为独立测试二进制，nextest 名称不带源文件模块前缀。
- 预期失败原因：任务 1 确认的 DDL 等待/polling 或 paging RPC 成本。
- 通过验证命令：同上；退出后确认 PD 不可达并清理唯一 tag 数据。
- 模拟策略：禁止 mock TiKV。

## 步骤

1. trap 管理 playground PID、端口和数据清理。
2. 分别记录 DDL propagation 与 paging RPC 时间线。
3. 优化轮询/批次/重复 setup，不缩减 SQL 场景。
4. 格式化、串行回归、清理验证和 lint。

## 验证

- 预期：2 个测试在 scoped RealTiKV budget 内通过，无残留进程/数据。
- 所需证据：健康检查、测试结果、耗时、cleanup check。

## 完成

记录 playground 参数和两个用例证据，使用 `$git-commit` 提交。
