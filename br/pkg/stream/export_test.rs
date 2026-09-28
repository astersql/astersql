// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! Go-equivalent export helpers from `br/pkg/stream/export_test.go`.
//! 仅供 crate 内测试：把包内私有合并/加载逻辑以公开方法暴露，对齐 Go `_test` 导出。
//! 不改变生产路径；`LoadFrom` 用 `u64::MAX` 作为截止 TS，表示“加载全部”。

use std::collections::HashMap;
use std::sync::Arc;

use crate::search::{StreamBackupSearch, StreamKVInfo};
use crate::stream_metas::StreamMetadataSet;
use crate::stubs::Storage;
use crate::stubs::backuppb::DataFileInfo;
use crate::stubs::errors::Error;

impl StreamBackupSearch {
    /// Corresponds to Go `SearchFromDataFileForTest`.
    /// Rust 用返回向量表达 Go 测试通道中收到的单文件搜索结果。
    pub fn SearchFromDataFileForTest(
        &self,
        data_file: &DataFileInfo,
    ) -> Result<Vec<StreamKVInfo>, Error> {
        let mut entries = Vec::new();
        self.searchFromDataFile(data_file, &mut entries)?;
        Ok(entries)
    }

    /// Corresponds to Go `MergeCFEntriesForTest`.
    /// 测试侧合并 default/write CF 条目，委托内部 `mergeCFEntries`。
    pub fn MergeCFEntriesForTest(
        &self,
        default_cf_entries: HashMap<String, StreamKVInfo>,
        write_cf_entries: HashMap<String, StreamKVInfo>,
    ) -> Vec<StreamKVInfo> {
        self.mergeCFEntries(default_cf_entries, write_cf_entries)
    }
}

impl StreamMetadataSet {
    /// Corresponds to Go `LoadFrom` (test-only wrapper).
    /// 包装 `LoadUntilAndCalculateShiftTS`，截止时间戳取最大值以加载全量元数据。
    pub fn LoadFrom(&mut self, s: Arc<dyn Storage>) -> Result<(), Error> {
        self.LoadUntilAndCalculateShiftTS(s, u64::MAX).map(|_| ())
    }
}
