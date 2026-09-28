// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 并发安全位图（ConcurrentBitmap）。
//
// 对应 Go `util/bitmap`：定长位图，置位通过 CAS（compare-and-swap，比较并交换）
// 保证线程安全。段宽固定 32 bit，缩小竞争窗口；亦提供非并发安全的读写变体。

use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};

/// 每个 segment 的比特宽度（32）。
pub const segmentWidth: usize = 32;
/// `segmentWidth` 对应的位移量（log2(32)=5），用于 bitIndex → segment 下标。
pub const segmentWidthPower: usize = 5;
/// 段内最高位掩码；右移后得到目标 bit 的掩码。
pub const bitMask: u32 = 0x80000000;

// bytesConcurrentBitmap 对应 Go 的包级变量，记录空 ConcurrentBitmap 结构体自身大小。
/// 空 `ConcurrentBitmap` 结构体自身占用字节数（不含 segments 堆内存）。
pub const bytesConcurrentBitmap: i64 = mem::size_of::<ConcurrentBitmap>() as i64;

// ConcurrentBitmap is a static-length bitmap which is thread-safe on setting.
// It is implemented using CAS, as atomic bitwise operation is not supported by
// golang yet. (See https://github.com/golang/go/issues/24244)
// CAS operation is narrowed down to uint32 instead of longer types like uint64,
// to reduce probability of racing.
// ConcurrentBitmap 对应 Go 结构体；segments 用 AtomicU32 表达原 sync/atomic CAS 目标。
/// 定长并发位图：`segments` 为原子 u32 数组，`bitLen` 为有效比特数。
pub struct ConcurrentBitmap {
    /// 按 32-bit 分段的原子存储。
    pub segments: Vec<AtomicU32>,
    /// 逻辑比特长度（越界访问被忽略）。
    pub bitLen: i32,
}

impl Clone for ConcurrentBitmap {
    fn clone(&self) -> Self {
        self.Clone()
    }
}

impl ConcurrentBitmap {
    // Clone clones a new bitmap with the old bit set.
    // Clone 对应 Go 方法，先按 bitLen 创建新 bitmap，再逐段复制旧值。
    /// 深拷贝：按相同 `bitLen` 新建，再逐段 SeqCst 复制当前值。
    pub fn Clone(&self) -> ConcurrentBitmap {
        let cp = NewConcurrentBitmap(self.bitLen);
        let needLen = cp.segments.len();
        for i in 0..needLen {
            let value = self.segments[i].load(Ordering::SeqCst);
            cp.segments[i].store(value, Ordering::SeqCst);
        }
        cp
    }

    // Reset clean the bitmap if the length is suitable, otherwise renewing one.
    // Reset 对应 Go 方法：容量够用时清零复用，否则重新分配 segments。
    /// 重置为给定长度：现有 segment 够用则清零复用，否则重新分配。
    pub fn Reset(&mut self, bitLen: i32) {
        let segmentLen = ((bitLen + segmentWidth as i32 - 1) >> segmentWidthPower) as usize;
        if segmentLen <= self.segments.len() {
            for segment in &self.segments {
                segment.store(0, Ordering::SeqCst);
            }
            self.bitLen = bitLen;
        } else {
            self.segments = (0..segmentLen).map(|_| AtomicU32::new(0)).collect();
            self.bitLen = bitLen;
        }
    }

    // BytesConsumed returns size of this bitmap in bytes.
    // BytesConsumed 对应 Go 的内存估算，结构体大小加上 segments 容量占用。
    /// 估算内存占用：结构体大小 + segments 容量对应字节数。
    pub fn BytesConsumed(&self) -> i64 {
        bytesConcurrentBitmap + (segmentWidth / 8 * self.segments.capacity()) as i64
    }

    // Set sets the bit on bitIndex to be 1 (bitIndex starts from 0).
    // isSetter indicates whether the function call this time triggers the bit from 0 to 1.
    // bitIndex bigger than bitLen initialized will be ignored.
    // Set 对应 Go 的并发安全置位：越界返回 false，已置位返回 false，CAS 成功返回 true。
    /// 并发安全置位；返回是否本次从 0→1（唯一 setter）。越界返回 false。
    pub fn Set(&self, bitIndex: i32) -> bool {
        if bitIndex < 0 || bitIndex >= self.bitLen {
            return false;
        }

        let segmentIndex = (bitIndex as usize) >> segmentWidthPower;
        let segmentPointer = &self.segments[segmentIndex];
        let mask = bitMask >> ((bitIndex as usize) % segmentWidth);

        // Repeatedly observe whether bit is already set, and try to set
        // it based on observation.
        // CAS 循环：已置位则返回 false；否则尝试 OR 掩码，失败则重试。
        loop {
            // Observe.
            let oldValue = segmentPointer.load(Ordering::SeqCst);
            if (oldValue & mask) != 0 {
                return false;
            }

            // Set.
            let newValue = oldValue | mask;
            // Go 使用 CompareAndSwapUint32；用 compare_exchange 保留同样的 CAS 重试语义。
            if segmentPointer
                .compare_exchange(oldValue, newValue, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    // UnsafeSet sets the bit on bitIndex to be 1 (bitIndex starts from 0).
    // isSetter indicates whether the function call this time triggers the bit from 0 to 1.
    // bitIndex bigger than bitLen initialized will be ignored.
    // (this version is concurrent unsafe if the caller can make sure write is in single thread)
    // UnsafeSet 对应 Go 的非并发安全写入；Rust 用 &mut self 表示调用者必须独占访问。
    /// 非并发安全置位；调用方须保证独占写（`&mut self`）。
    pub fn UnsafeSet(&mut self, bitIndex: i32) {
        if bitIndex < 0 || bitIndex >= self.bitLen {
            return;
        }

        let mask = bitMask >> ((bitIndex as usize) % segmentWidth);
        let segment = &mut self.segments[(bitIndex as usize) >> segmentWidthPower];
        let value = *segment.get_mut();
        *segment.get_mut() = value | mask;
    }

    // UnsafeIsSet returns if a bit on bitIndex is set (bitIndex starts from 0).
    // bitIndex bigger than bitLen initialized will return false.
    // This method is not thread-safe as it does not use atomic load.
    // UnsafeIsSet 对应 Go 的非原子读取；AtomicU32 这里用 Relaxed load 近似保留轻量读取意图。
    /// 非原子/轻量读：判断某 bit 是否已置位；越界返回 false。
    pub fn UnsafeIsSet(&self, bitIndex: i32) -> bool {
        if bitIndex < 0 || bitIndex >= self.bitLen {
            return false;
        }

        let mask = bitMask >> ((bitIndex as usize) % segmentWidth);
        let value = self.segments[(bitIndex as usize) >> segmentWidthPower].load(Ordering::Relaxed);
        (value & mask) != 0
    }
}

// NewConcurrentBitmap initializes a ConcurrentBitmap which can store
// bitLen of bits.
// NewConcurrentBitmap 对应 Go 构造函数，按 32bit segment 向上取整。
/// 构造可容纳 `bitLen` 比特的并发位图（segment 数向上取整）。
pub fn NewConcurrentBitmap(bitLen: i32) -> ConcurrentBitmap {
    let segmentLen = ((bitLen + segmentWidth as i32 - 1) >> segmentWidthPower) as usize;
    ConcurrentBitmap {
        segments: (0..segmentLen).map(|_| AtomicU32::new(0)).collect(),
        bitLen,
    }
}
