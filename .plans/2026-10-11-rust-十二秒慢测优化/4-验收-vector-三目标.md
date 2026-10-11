# 任务 4: 验收 vector 三目标

批次：【批次 4】 依赖批次 2、3

状态：未开始

目的：独立证明三个 vectorsearch 目标各三轮均低于 12 秒，并核对 Go 对齐覆盖未减少。

来源任务：任务 2、3 的 vectorsearch 改动。

预计会话范围：只做性能与差异验收；失败时回到任务 2 或 3 的边界继续优化，不扩展其他 crate。

## 文件

- 修改：无；仅在验收发现任务范围内缺陷时修改 `pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 产物：`target/rust-performance/2026-10-11-vector-final.tsv`

## 上下文

- 三个来源基线为 16.989、16.572、16.448 秒。
- 必须核对 6000 行、双 Cascades、11/11/21 条输入及 plan/warning 断言。

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

## 验证计划

- 运行：`tools/check/rust-test-performance.sh --max-seconds 12 --runs 3 --output target/rust-performance/2026-10-11-vector-final.tsv -- --locked --package astersql-planner-core-casetest-vectorsearch -E 'test(/vector_index_test::test_vector_search_(with_pk_auto|with_pk_force_tikv|heavy_function)_matches_go_golden/)'`
- 预期：9/9 有效 PASS 样本，每个 identity 三条，所有最大值 ≤12 秒。
- 差异审查：对照 Go 测试与 testdata 统计规模、模式、case 数和断言。

## 步骤

1. 在宿主空闲时完整运行三轮，不丢弃慢样本。
2. 若任一项超标，依据分段证据回到相应局部实现继续优化并重跑完整三轮。
3. 记录 min/median/max、退出码和有效测试数。

## 完成

9/9 样本通过、每项最大值 ≤12 秒且覆盖自审通过后标记 `已完成`；无代码变更不创建空提交，删除任务文件。
