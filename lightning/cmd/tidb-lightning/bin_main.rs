// Copyright 2026 AsterSQL.

//! 该二进制入口保持为薄包装层，只负责把进程入口转发给库 crate 中
//! 的真实启动逻辑，避免把初始化细节散落在包装层，并与 Go 版本由
//! `main` 统一承接启动职责的组织方式保持一致。

fn main() {
    astersql_lightning_cmd_tidb_lightning::main();
}
