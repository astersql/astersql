# 任务 1: 拆分并优化 masking-policy 升级测试

批次：【批次 1】无依赖

状态：已完成

## Cargo 共享槽位规则

1. 共用仓库根目录 `target/rust-slot-1` 至 `target/rust-slot-10`，不新建其他构建目录。
2. 在 `target/rust-slot-locks` 中依次用原子 `mkdir slot-N.lock` 领取 1–10 的空闲槽位。
3. 在锁目录记录计划、任务、会话和 PID；不删除他人锁。
4. 设置绝对路径 `CARGO_TARGET_DIR="$PWD/target/rust-slot-N"`。
5. 所有 Cargo 及其子进程继承该变量，不用 `--target-dir` 绕过。
6. 阶段结束后删除自己的记录并 `rmdir` 锁目录，保留缓存。
7. 正常、失败和中断都要清理自己的锁；子进程未结束不得释放。
8. 遗留锁先核实进程，不能确认则保持占用。
9. 槽位分离不取代源码和其他共享资源互斥。
10. 报告槽位、绝对 `CARGO_TARGET_DIR`、命令、退出码和测试数；Rust 改动后先 `cargo fmt --all`。

## 验证

- `cargo nextest run --locked -p astersql-session -E 'test(masking_policy_upgrade_from_)'`：3 个版本均通过。
- `cargo nextest run --locked -p astersql-session -E 'test(upgrade)'`：完整 upgrade 集通过。
- 对比修改前合并场景 8.43–9.53 秒，记录三个独立测试的耗时。

完成证据：聚焦测试 3/3 通过（2.81 秒），完整 upgrade 过滤集 29/29 通过（5.80 秒），`make lint` 通过。
