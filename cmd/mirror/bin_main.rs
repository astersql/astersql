// Copyright 2026 AsterSQL.

// `mirror` 命令的二进制入口层。
// 实际命令流程由共享库统一装配；这里保持单跳转发，使二进制与对齐测试复用同一实现。
fn main() {
    astersql_cmd_mirror::main();
}
