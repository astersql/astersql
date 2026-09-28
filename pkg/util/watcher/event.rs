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

// 文件系统监视事件模型。
//
// 定义操作位标志（Create/Remove/Modify 等）、文件元信息包装 `FileInfo`，
// 以及带路径与操作码的 `Event`；供轮询式 Watcher 对外投递变更通知。

use std::fs::Metadata;
use std::path::PathBuf;
use std::time::SystemTime;

/// 文件变更操作位标志类型（可按位或组合多种操作）。
pub type Op = u32;

/// 文件或目录被创建。
pub const Create: Op = 1 << 0;
/// 文件或目录被删除。
pub const Remove: Op = 1 << 1;
/// 内容变更（修改时间或大小变化）。
pub const Modify: Op = 1 << 2;
/// 同目录内重命名。
pub const Rename: Op = 1 << 3;
/// 权限/模式变更。
pub const Chmod: Op = 1 << 4;
/// 跨目录移动（父路径不同）。
pub const Move: Op = 1 << 5;

/// 将操作位标志格式化为 `CREATE|MODIFY|...` 形式的可读字符串。
pub fn OpString(op: Op) -> String {
    let mut names = Vec::new();
    // 按固定顺序扫描各位，保证输出与 Go 侧一致。
    for (flag, name) in [
        (Create, "CREATE"),
        (Remove, "REMOVE"),
        (Modify, "MODIFY"),
        (Rename, "RENAME"),
        (Chmod, "CHMOD"),
        (Move, "MOVE"),
    ] {
        if op & flag == flag {
            names.push(name);
        }
    }
    names.join("|")
}

/// 文件/目录元信息包装，屏蔽平台差异的比较与查询接口。
#[derive(Clone, Debug)]
pub struct FileInfo {
    metadata: Metadata,
}

impl FileInfo {
    /// 由标准库 `Metadata` 构造。
    pub(crate) fn new(metadata: Metadata) -> Self {
        Self { metadata }
    }

    /// 是否为目录。
    pub fn IsDir(&self) -> bool {
        self.metadata.is_dir()
    }

    /// 最近修改时间；获取失败时返回 `None`。
    pub(crate) fn ModTime(&self) -> Option<SystemTime> {
        self.metadata.modified().ok()
    }

    /// 文件字节大小。
    pub(crate) fn Size(&self) -> u64 {
        self.metadata.len()
    }

    /// Unix：返回完整 mode 位；用于检测 chmod。
    #[cfg(unix)]
    pub(crate) fn Mode(&self) -> u32 {
        use std::os::unix::fs::MetadataExt;
        self.metadata.mode()
    }

    /// 非 Unix：用只读标志近似表示权限变化。
    #[cfg(not(unix))]
    pub(crate) fn Mode(&self) -> bool {
        self.metadata.permissions().readonly()
    }

    /// Unix：通过 device + inode 判断是否为同一底层文件（用于识别 rename/move）。
    #[cfg(unix)]
    pub(crate) fn same_file(&self, other: &Self) -> bool {
        use std::os::unix::fs::MetadataExt;
        self.metadata.dev() == other.metadata.dev() && self.metadata.ino() == other.metadata.ino()
    }

    /// 非 Unix：无法可靠比较，恒返回 false。
    #[cfg(not(unix))]
    pub(crate) fn same_file(&self, _other: &Self) -> bool {
        false
    }
}

/// 一次文件变更通知：路径、操作位与当时的文件信息。
#[derive(Clone, Debug)]
pub struct Event {
    /// 事件发生时的文件元信息。
    pub FileInfo: FileInfo,
    /// 受影响路径。
    pub Path: PathBuf,
    /// 操作位标志（可组合）。
    pub Op: Op,
}

impl Event {
    /// 该事件是否针对目录（而非普通文件）。
    pub fn IsDirEvent(&self) -> bool {
        self.FileInfo.IsDir()
    }

    /// 事件操作位是否命中 `ops` 中任一标志。
    pub fn HasOps(&self, ops: &[Op]) -> bool {
        ops.iter().any(|op| self.Op & op != 0)
    }
}

/// 可选事件版本的目录判定；`None` 视为非目录事件。
pub fn IsDirEventOption(event: Option<&Event>) -> bool {
    event.is_some_and(Event::IsDirEvent)
}

/// 可选事件版本的操作位匹配；`None` 视为不匹配。
pub fn HasOpsOption(event: Option<&Event>, ops: &[Op]) -> bool {
    event.is_some_and(|event| event.HasOps(ops))
}
