// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use crate::engine::{Engine, EngineInfo};
use crate::mem_root::{MemRoot, MemRootImpl};

#[test]
fn closed_engine_rejects_flush_writes_and_new_writers() {
    let mem_root: Arc<dyn MemRoot> = Arc::new(MemRootImpl::new(128));
    let engine = EngineInfo::new(42, true, "engine-42", Arc::clone(&mem_root), 16);
    let mut writer = engine.create_writer(7).expect("create writer");
    writer.write_row(b"key", b"value").expect("write row");

    engine.close(false);

    assert_eq!(engine.flush().unwrap_err(), "engine closed");
    assert_eq!(
        writer.write_row(b"later", b"row").unwrap_err(),
        "engine closed"
    );
    assert_eq!(
        engine.create_writer(8).err().as_deref(),
        Some("engine closed")
    );
    assert_eq!(
        engine.rows().get(b"key".as_slice()),
        Some(&b"value".to_vec())
    );
}
