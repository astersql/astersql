// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// `EngineTypeSet::Contains` 与预置常量的单元测试。

use crate::*;

#[test]
/// 覆盖 All / Only / Or 组合对 TiDB、TiKV、TiFlash 的包含关系。
fn TestEngineTypeSet() {
    assert!(EngineAll.Contains(EngineTiDB));
    assert!(EngineAll.Contains(EngineTiKV));
    assert!(EngineAll.Contains(EngineTiFlash));
    assert!(EngineTiDBOnly.Contains(EngineTiDB));
    assert!(!EngineTiDBOnly.Contains(EngineTiKV));
    assert!(!EngineTiDBOnly.Contains(EngineTiFlash));
    assert!(!EngineTiKVOnly.Contains(EngineTiDB));
    assert!(EngineTiKVOnly.Contains(EngineTiKV));
    assert!(!EngineTiKVOnly.Contains(EngineTiFlash));
    assert!(!EngineTiFlashOnly.Contains(EngineTiDB));
    assert!(!EngineTiFlashOnly.Contains(EngineTiKV));
    assert!(EngineTiFlashOnly.Contains(EngineTiFlash));
    assert!(!EngineTiKVOrTiFlash.Contains(EngineTiDB));
    assert!(EngineTiKVOrTiFlash.Contains(EngineTiKV));
    assert!(EngineTiKVOrTiFlash.Contains(EngineTiFlash));
}
