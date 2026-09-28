// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `metrics_test.go`.
//! 这个测试只锁住 metrics 的注册/反注册路径能完整走通。

use crate::*;

#[test]
fn test_metrics_registration() {
    // 不检查 registry 内部细节，只要求这两个生命周期调用不报错。
    let m = newMetrics(NewDefaultFactory().as_ref(), &Labels::default());
    let registry = NewDefaultRegistry();
    m.registerTo(registry.as_ref());
    m.unregisterFrom(registry.as_ref());
}
