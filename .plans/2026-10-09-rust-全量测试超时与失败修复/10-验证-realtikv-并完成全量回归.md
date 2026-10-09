# 任务 10: 验证 RealTiKV 并完成全量回归

批次：【批次 4】 依赖任务 2、3、4、5、6、7、8、9

状态：未开始

目的：在真实 TiKV 环境验证 RealTiKV 失败，处理剩余可复现根因，并以 Ready profile 完成全量 Rust 回归。

来源任务：RealTiKV split-file 两个失败、相关外部服务超时以及前置任务完成后的全量残余。

预计会话范围：先按仓库流程启动/清理 playground，再修复仅在真实 TiKV 下可复现的局部问题；最后执行完整 workspace 测试并分类任何残余。

## 文件

- 修改：`tests/realtikvtest/importintotest4/split_file_test.rs`（仅测试确有缺陷时）
- 修改：对应 import/split 生产文件（按调用图定位）
- 测试：前述 RealTiKV 测试和 workspace 全量测试
- 检查：所有前置任务提交的 Rust 源文件及测试文件

## 上下文

- 原 workspace 直接运行 RealTiKV tests，许多约 10 秒超时可能是 PD/TiKV 未启动；必须使用 `docs/agents/testing-flow.md` 的 playground 生命周期，不能通过 skip/ignore 解决。

## Cargo 共享槽位规则

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

- 行为：split-file 与其他 RealTiKV 测试在健康 PD/TiKV 上执行真实读写并通过；workspace 默认 profile 不再有无依据超时、失败或崩溃。
- 失败验证测试：先在健康 playground 中复跑 `test_split_file`，若仍失败则保留其错误作为回归；环境缺失失败不算代码回归证据。
- 失败验证命令：`cargo nextest run --locked -p astersql-tests-realtikvtest-importintotest4 -E 'test(test_split_file)' --test-threads 1`
- 预期失败原因：在健康服务上暴露真实 split/import 缺陷；若通过则原失败归类为测试入口环境问题，不改生产代码。
- 通过验证命令：`make rust-test`
- 模拟策略：使用真实 TiUP playground，不 mock PD/TiKV。

## 步骤

1. 按 `docs/agents/testing-flow.md` 启动带唯一 tag/端口的 tikv-slim playground，等待 PD 探活。
2. 在持有 Cargo 槽位时运行 split-file 与任务 1 标记的 RealTiKV 过滤集。
3. 仅对健康环境仍失败的行为增加回归并修复；再次运行 RealTiKV 集。
4. 无论成功、失败或中断都停止 playground、等待进程退出、删除对应数据并确认 PD 不可达。
5. 运行 `cargo fmt --all`、适用的 Ready 检查（Rust 代码至少 `make lint`）和 `make rust-test`；核对有效测试数与原 18,325 基线差异。
6. 自审最终 diff，确保 Rust 源顶部版权、测试分文件、无本地依赖覆盖、无遗留锁/进程。

## 验证

- 运行：playground 启动/探活、聚焦 RealTiKV 命令、清理检查。
- 运行：`cargo fmt --all`
- 运行：`make lint`
- 运行：`make rust-test`
- 预期：所有适用测试有效执行且退出码 0；无 TIMEOUT/FAIL/SIGABRT；playground 和数据完成清理。
- 所需证据：实际槽位、CARGO_TARGET_DIR、PD 地址、每条命令退出码、有效测试数、全量 outcome 计数为零、未验证项。

## 完成

报告全部文件、Ready profile、风险、确切命令和本地未验证项；如产生修复，使用 `$git-commit` 提交，保留最终日志但不纳入 Git。
