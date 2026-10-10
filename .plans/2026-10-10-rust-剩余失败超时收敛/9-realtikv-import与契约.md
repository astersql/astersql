# 任务 9: 修复 RealTiKV import 与公共契约失败

批次：【批次 4】 依赖任务 8；独占 playground

状态：未开始

目的：修复 split-file 快速 FAIL 和 testutils 公共契约超时，保持真实 TiKV 文件切分与 Go/Rust API 对齐。

来源任务：`test_split_file`、`parity_test::go_rust_public_contract_matches`。

预计会话范围：RealTiKV importintotest4 与 testutils 两个明确边界，使用干净集群串行验证。

## 文件

- 测试：`tests/realtikvtest/importintotest4/split_file_test.rs`
- 测试：`tests/realtikvtest/testutils/parity_test.rs`
- 修改：任务 1 错误链指向的 import/testutils 拥有文件

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

- 行为：split file 的边界、内容和错误语义正确；Go/Rust public contract 全场景完成。
- 失败验证命令：干净 tikv-slim playground 中运行 `cargo nextest run --locked --package astersql-tests-realtikvtest-importintotest4 --package astersql-tests-realtikvtest-testutils -E 'test(=split_file_test::test_split_file) | test(=parity_test::go_rust_public_contract_matches)' --no-capture`。
- 预期失败原因：任务 1 保存的精确断言错误和 contract setup/teardown 热点。
- 通过验证命令：同上并完成 playground cleanup check。
- 模拟策略：真实 TiKV 和真实临时文件；只控制临时目录生命周期。

## 步骤

1. 保存 split-file 的实际/期望差异与 contract 超时阶段。
2. 对照 Go 同名测试修正最小生产/测试接线。
3. 优化重复集群/Domain setup，不删减公共契约条目。
4. 格式化、串行回归、清理检查、lint。

## 验证

- 预期：2 个测试退出码 0，split-file 不再 FAIL，contract 在 scoped budget 内结束。
- 所需证据：错误前后对比、契约项数量、耗时、无残留资源。

## 完成

记录 Go 对齐依据和验证证据，使用 `$git-commit` 提交。
