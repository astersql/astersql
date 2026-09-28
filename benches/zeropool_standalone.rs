// Copyright 2026 AsterSQL.

// 这个根目录遗留 target 不复刻 zeropool crate 的完整 Go 对齐测试，
// 只验证工作区兼容门面仍然能把外部 crate 暴露给旧入口使用。
use astersql::util::zeropool;

// The complete Go-parity suite is owned by the canonical zeropool crate.
// This legacy root target only verifies that the workspace compatibility
// facade still exposes that crate without embedding its private test module.
#[test]
fn canonical_zeropool_is_available_through_the_root_facade() {
    let pool = zeropool::New(|| vec![0_u8; 16]);
    let value = pool.Get();
    assert_eq!(value.len(), 16);
    // 归还后再次取出并检查长度，确认根门面导出的仍是可复用的同一套池化接口。
    pool.Put(value);
    assert_eq!(pool.Get().len(), 16);
}
