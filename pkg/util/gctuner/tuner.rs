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

// GOGC（Go 运行时 GC 触发百分比）动态调谐器。
//
// 对应 Go `util/gctuner` 的 Tuner：根据当前堆占用与阈值计算目标 GOGC，
// 经 finalizer 周期回调写入运行时。阈值越大、占用越低 → GOGC 越高（更晚触发 GC）。

use crate::finalizer::{Finalizer, newFinalizer};
use crate::mem::readMemoryInuse;
use std::env;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use task_util::gogc;

/// GOGC 上限（默认 500）。
pub static maxGCPercent: AtomicU32 = AtomicU32::new(defaultMaxGCPercent);
/// GOGC 下限（默认 100）。
pub static minGCPercent: AtomicU32 = AtomicU32::new(defaultMinGCPercent);
/// 是否启用 GOGC 自动调谐；关闭时 `tuning` 直接返回。
pub static EnableGOGCTuner: AtomicBool = AtomicBool::new(false);

/// 默认最大 GOGC 百分比。
pub const defaultMaxGCPercent: u32 = 500;
/// 默认最小 GOGC 百分比。
pub const defaultMinGCPercent: u32 = 100;

/// 从环境变量 `GOGC` 读取默认百分比，缺省 100。
static DEFAULT_GC_PERCENT: LazyLock<AtomicU32> = LazyLock::new(|| {
    let value = env::var("GOGC")
        .ok()
        .and_then(|raw| raw.parse::<i32>().ok())
        .map(|value| value as u32)
        .unwrap_or(100);
    AtomicU32::new(value)
});

/// 进程级单例调谐器；`Tuning(0)` 停止并清空。
static GLOBAL_TUNER: LazyLock<Mutex<Option<Arc<Tuner>>>> = LazyLock::new(|| Mutex::new(None));

/// 返回环境/默认 GOGC 百分比。
pub fn defaultGCPercent() -> u32 {
    DEFAULT_GC_PERCENT.load(Ordering::SeqCst)
}

/// 设置允许的最大 GOGC。
pub fn SetMaxGCPercent(percent: u32) {
    maxGCPercent.store(percent, Ordering::SeqCst);
}

/// 读取最大 GOGC。
pub fn MaxGCPercent() -> u32 {
    maxGCPercent.load(Ordering::SeqCst)
}

/// 设置允许的最小 GOGC。
pub fn SetMinGCPercent(percent: u32) {
    minGCPercent.store(percent, Ordering::SeqCst);
}

/// 读取最小 GOGC。
pub fn MinGCPercent() -> u32 {
    minGCPercent.load(Ordering::SeqCst)
}

/// 初始化默认上下限与默认 GOGC。
pub fn init() {
    let _ = defaultGCPercent();
    SetMinGCPercent(defaultMinGCPercent);
    SetMaxGCPercent(defaultMaxGCPercent);
}

/// 将运行时 GOGC 设为默认百分比。
pub fn SetDefaultGOGC() {
    gogc::SetGOGC(defaultGCPercent() as i32);
}

/// 全局调谐入口：`threshold==0` 停止已有调谐器；否则创建或更新阈值。
pub fn Tuning(threshold: u64) {
    let mut global = GLOBAL_TUNER.lock().expect("global tuner lock poisoned");
    if threshold == 0 && global.is_some() {
        if let Some(tuner) = global.take() {
            tuner.stop();
        }
        return;
    }
    if global.is_none() {
        *global = Some(newTuner(threshold));
        return;
    }
    global
        .as_ref()
        .expect("tuner was initialized")
        .setThreshold(threshold);
}

/// 读取当前调谐器记录的 GOGC；无实例时返回默认值。
pub fn GetGOGC() -> u32 {
    GLOBAL_TUNER
        .lock()
        .expect("global tuner lock poisoned")
        .as_ref()
        .map_or_else(defaultGCPercent, |tuner| tuner.getGCPercent())
}

/// 单实例调谐器：finalizer 周期根据占用与阈值重算 GOGC。
pub struct Tuner {
    finalizer: Arc<Finalizer>,
    /// 最近一次写入运行时前记录的 GOGC（与 Go SetGOGC 返回旧值语义对齐）。
    gcPercent: AtomicU32,
    /// 目标堆上限阈值；占用相对阈值决定 GOGC。
    threshold: AtomicU64,
}

/// Go 风格类型别名。
pub type tuner = Tuner;

/// 构造调谐器；finalizer 回调 upgrade weak 后调用 `tuning`。
pub fn newTuner(threshold: u64) -> Arc<Tuner> {
    Arc::new_cyclic(|weak: &std::sync::Weak<Tuner>| {
        let weak = weak.clone();
        Tuner {
            finalizer: newFinalizer(Box::new(move || {
                if let Some(tuner) = weak.upgrade() {
                    tuner.tuning();
                }
            })),
            gcPercent: AtomicU32::new(defaultGCPercent()),
            threshold: AtomicU64::new(threshold),
        }
    })
}

impl Tuner {
    /// 停止 finalizer。
    pub fn stop(&self) {
        self.finalizer.stop();
    }

    /// 手动跑一轮 finalizer（测试用）。
    pub fn runFinalizer(&self) -> bool {
        self.finalizer.run()
    }

    /// 更新堆占用阈值。
    pub fn setThreshold(&self, threshold: u64) {
        self.threshold.store(threshold, Ordering::SeqCst);
    }

    /// 读取当前阈值。
    pub fn getThreshold(&self) -> u64 {
        self.threshold.load(Ordering::SeqCst)
    }

    /// 写入运行时 GOGC，并保存旧值到 `gcPercent`。
    pub fn setGCPercent(&self, percent: u32) -> u32 {
        let previous = gogc::SetGOGC(percent as i32) as u32;
        self.gcPercent.store(previous, Ordering::SeqCst);
        previous
    }

    /// 读取调谐器记录的 GOGC。
    pub fn getGCPercent(&self) -> u32 {
        self.gcPercent.load(Ordering::SeqCst)
    }

    /// 若启用调谐且阈值非 0，按当前占用重算并设置 GOGC。
    pub fn tuning(&self) {
        if !EnableGOGCTuner.load(Ordering::SeqCst) {
            return;
        }
        let threshold = self.getThreshold();
        if threshold == 0 {
            return;
        }
        self.setGCPercent(calcGCPercent(readMemoryInuse(), threshold));
    }
}

/// 由占用与阈值计算 GOGC：`(threshold-inuse)/inuse*100`，并夹在 [min,max]。
///
/// `inuse==0` 或 `threshold==0` 返回默认；`threshold<=inuse` 返回最小值。
pub fn calcGCPercent(inuse: u64, threshold: u64) -> u32 {
    if inuse == 0 || threshold == 0 {
        return defaultGCPercent();
    }
    if threshold <= inuse {
        return MinGCPercent();
    }
    let percent = (((threshold - inuse) as f64 / inuse as f64) * 100.0).floor() as u32;
    if percent < MinGCPercent() {
        MinGCPercent()
    } else if percent > MaxGCPercent() {
        MaxGCPercent()
    } else {
        percent
    }
}
