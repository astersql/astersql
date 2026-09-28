// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 手动内存分配器封装：在 `New`/`Free` 之上可选挂载原子引用计数，便于测试检测泄漏。
//
// Lightning 导入路径中大量短生命周期字节缓冲走手动分配；本类型对应 Go `manual.Allocator`。

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use super::{Free, New};

/// Allocator 对应 Go 的手动内存分配器。
/// `Option` 保留 `RefCnt == nil` 时关闭泄漏计数的语义，`Arc` 对应 Go 原子计数器可共享的指针形状。
#[derive(Clone, Default)]
pub struct Allocator {
    /// 可选的共享引用计数；`None` 表示不做泄漏统计（Go 零值语义）。
    pub RefCnt: Option<Arc<AtomicI64>>,
}

impl Allocator {
    /// Alloc 对应 Go 的同名方法：可选地增加引用计数，再委托包内 `New` 分配指定长度的字节切片。
    pub fn Alloc(&self, n: isize) -> Vec<u8> {
        if let Some(ref_cnt) = &self.RefCnt {
            // Go 的 atomic.Int64.Add(1) 具有顺序一致语义。
            ref_cnt.fetch_add(1, Ordering::SeqCst);
        }

        New(n)
    }

    /// Free 对应 Go 的同名方法：先减少可选引用计数，再委托包内 `Free` 释放字节切片。
    pub fn Free(&self, bytes: Vec<u8>) {
        if let Some(ref_cnt) = &self.RefCnt {
            ref_cnt.fetch_sub(1, Ordering::SeqCst);
        }

        Free(bytes);
    }

    /// CheckRefCnt 对应 Go 的泄漏检查：非零引用数被格式化为错误，未启用计数或计数归零则成功。
    pub fn CheckRefCnt(&self) -> Result<(), String> {
        if let Some(ref_cnt) = &self.RefCnt {
            if ref_cnt.load(Ordering::SeqCst) != 0 {
                // Go 在判断后再次 Load，并用第二次读取的当前值构造错误文本。
                let count = ref_cnt.load(Ordering::SeqCst);
                return Err(format!("memory leak detected, refCnt: {count}"));
            }
        }

        Ok(())
    }
}
