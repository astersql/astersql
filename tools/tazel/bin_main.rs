// Copyright 2026 AsterSQL.

//! `tazel` 二进制入口只负责把进程启动委托给库层 `entry()`，
//! 这样命令行主流程仍集中在库代码里，便于与 Go `main` 保持对齐并复用实现。

/// 复用库层入口，避免在二进制包装层分叉实际业务逻辑。
fn main() {
    astersql_tools_tazel::entry();
}
