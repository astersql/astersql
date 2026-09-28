// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Checkpoint skip bitmaps for log files, matching `log_file_map.go`.
//! 日志恢复检查点跳过位图：按 metaKey → groupOff → fileOff 三级定位已完成文件。
//! 稀疏 HashMap 存 64 位块，避免为稀疏偏移分配稠密数组；与 Go 位运算语义一致。
//! 基础版只做精确文件跳过；Ext 版额外支持整 meta/整 group 短路。

use std::collections::HashMap;

/// Each 64 items constitute a bitmap unit.
/// 每 64 个偏移共用一个 u64；块号 = off>>6，位掩码 = 1<<(off&63)。
pub type BitMap = HashMap<i32, u64>;

// 空位图工厂，与 Go newBitMap 等价。
pub fn newBitMap() -> BitMap {
    HashMap::new()
}

// 将线性偏移拆成块索引与单比特掩码，供 Set/Hit 共用。
fn bit_pos(off: i32) -> (i32, u64) {
    (off >> 6, 1u64 << (off & 63))
}

// 置位：缺失块按 0 初始化再 OR，保证幂等插入。
pub fn bitMapSet(m: &mut BitMap, off: i32) {
    let (block_index, bit_offset) = bit_pos(off);
    *m.entry(block_index).or_insert(0) |= bit_offset;
}

// 查询：缺失块视为全 0，未置位即不跳过。
pub fn bitMapHit(m: &BitMap, off: i32) -> bool {
    let (block_index, bit_offset) = bit_pos(off);
    (m.get(&block_index).copied().unwrap_or(0) & bit_offset) > 0
}

/// 扩展位图：`skip=true` 表示整组跳过，不再逐文件记账。
pub struct BitMapExt {
    pub bitMap: BitMap,
    pub skip: bool,
}

// skip=true 时 bitMap 通常为空，NeedSkip 直接短路。
pub fn newBitMapExt(skip: bool) -> BitMapExt {
    BitMapExt {
        bitMap: newBitMap(),
        skip,
    }
}

/// 单 meta 下 groupOff → BitMap；基础版无整组/整 meta 短路。
pub struct FileMap {
    pub pos: HashMap<i32, BitMap>,
}

// 空 FileMap：group 位图按需插入。
pub fn newFileMap() -> FileMap {
    FileMap {
        pos: HashMap::new(),
    }
}

/// 扩展 FileMap：`skip` 为 true 时该 meta 下全部 group 直接跳过。
pub struct FileMapExt {
    pub pos: HashMap<i32, BitMapExt>,
    pub skip: bool,
}

// 构造扩展 FileMap；skip 语义同 BitMapExt。
pub fn newFileMapExt(skip: bool) -> FileMapExt {
    FileMapExt {
        pos: HashMap::new(),
        skip,
    }
}

/// 基础跳过表：仅支持按文件偏移精确标记（检查点恢复常用路径）。
pub struct LogFilesSkipMap {
    pub skipMap: HashMap<String, FileMap>,
}

// 空跳过表，供检查点加载前初始化。
pub fn NewLogFilesSkipMap() -> LogFilesSkipMap {
    LogFilesSkipMap {
        skipMap: HashMap::new(),
    }
}

impl LogFilesSkipMap {
    // 三级懒创建：meta → group → bit，避免预分配稀疏空间。
    pub fn Insert(&mut self, metaKey: &str, groupOff: i32, fileOff: i32) {
        let mp = self
            .skipMap
            .entry(metaKey.to_string())
            .or_insert_with(newFileMap);
        let gp = mp.pos.entry(groupOff).or_insert_with(newBitMap);
        bitMapSet(gp, fileOff);
    }

    // 任一层缺失即不跳过，保证未记录文件仍会被处理。
    pub fn NeedSkip(&self, metaKey: &str, groupOff: i32, fileOff: i32) -> bool {
        let Some(mp) = self.skipMap.get(metaKey) else {
            return false;
        };
        let Some(gp) = mp.pos.get(&groupOff) else {
            return false;
        };
        bitMapHit(gp, fileOff)
    }
}

/// 扩展跳过表：支持 SkipMeta/SkipGroup 整层短路，减少位图膨胀。
pub struct LogFilesSkipMapExt {
    pub skipMap: HashMap<String, FileMapExt>,
}

// 空扩展跳过表，供需要整层短路的调用方使用。
pub fn NewLogFilesSkipMapExt() -> LogFilesSkipMapExt {
    LogFilesSkipMapExt {
        skipMap: HashMap::new(),
    }
}

impl LogFilesSkipMapExt {
    // 若 meta/group 已整层 skip，Insert 直接返回，避免无意义位操作。
    pub fn Insert(&mut self, metaKey: &str, groupOff: i32, fileOff: i32) {
        let mp = self
            .skipMap
            .entry(metaKey.to_string())
            .or_insert_with(|| newFileMapExt(false));
        if mp.skip {
            // meta 已整跳，细化偏移无意义。
            return;
        }
        let gp = mp
            .pos
            .entry(groupOff)
            .or_insert_with(|| newBitMapExt(false));
        if gp.skip {
            // group 已整跳，同样忽略单文件置位。
            return;
        }
        bitMapSet(&mut gp.bitMap, fileOff);
    }

    // 整 meta 跳过：覆盖插入 skip=true 的 FileMapExt。
    pub fn SkipMeta(&mut self, metaKey: &str) {
        self.skipMap
            .insert(metaKey.to_string(), newFileMapExt(true));
    }

    // 整 group 跳过；若 meta 已 skip 则无需再写 group 级标记。
    pub fn SkipGroup(&mut self, metaKey: &str, groupOff: i32) {
        let mp = self
            .skipMap
            .entry(metaKey.to_string())
            .or_insert_with(|| newFileMapExt(false));
        if mp.skip {
            return;
        }
        mp.pos.insert(groupOff, newBitMapExt(true));
    }

    // 判定顺序：meta skip → group skip → 位图命中，与 Go Ext 语义一致。
    pub fn NeedSkip(&self, metaKey: &str, groupOff: i32, fileOff: i32) -> bool {
        let Some(mp) = self.skipMap.get(metaKey) else {
            return false;
        };
        if mp.skip {
            return true;
        }
        let Some(gp) = mp.pos.get(&groupOff) else {
            return false;
        };
        if gp.skip {
            return true;
        }
        bitMapHit(&gp.bitMap, fileOff)
    }
}
