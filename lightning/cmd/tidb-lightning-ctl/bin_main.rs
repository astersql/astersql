// Copyright 2026 AsterSQL.

//! 该二进制入口只负责把进程控制权转交给库 crate 的 `main()`。
//! 这样与 Go 版本保持一致：真正的命令逻辑集中在库层，这个文件只提供可执行壳。

fn main() {
    astersql_lightning_cmd_tidb_lightning_ctl::main();
}
