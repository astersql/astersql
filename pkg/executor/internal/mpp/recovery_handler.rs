// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// MPP（Massively Parallel Processing，大规模并行处理）错误恢复处理器。
//
// 当 TiFlash 计算节点因内存限额等错误失败时，本模块可缓冲尚未对外可见的
// 响应，并按注册顺序尝试恢复实现（如 AutoScaler 扩容后重建拓扑），避免
// 直接将可恢复错误暴露给上层。
use std::collections::VecDeque;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_memory::tracker::{NewTracker, Tracker};
use astersql_util_tiflashcompute as tiflashcompute;

/// 内存限额错误消息子串，用于识别 TiFlash Memory limit 类 MPP 错误。
const MEM_LIMIT_ERR_PATTERN: &str = "Memory limit";

/// MPP 响应的堆分配引用，对应 `kv::ResultSubset` 结果子集接口。
pub type MppResponseRef = Box<dyn kv::ResultSubset>;

/// 恢复上下文：携带原始 MPP 错误以及参与失败请求的计算节点数。
/// RecoveryInfo contains the original MPP error and the number of compute
/// nodes involved in the failed request.
pub struct RecoveryInfo {
    /// 原始 MPP 错误；为 `None` 时 `Recovery` 会拒绝。
    pub MPPErr: Option<errors::SharedError>,
    /// 失败请求涉及的计算节点数量，供拓扑恢复（如扩容）使用。
    pub NodeCnt: i32,
}

/// 缓冲响应并在 MPP 错误发生时按注册顺序尝试恢复实现。
/// RecoveryHandler buffers responses until they are safe to expose and tries
/// the registered recovery implementations in order when an MPP error occurs.
pub struct RecoveryHandler {
    /// 尚未对外弹出的响应缓冲与内存记账。
    holder: MppResultHolder,
    /// 按优先级排列的恢复实现列表。
    pub(crate) handlers: Vec<Box<dyn HandlerImpl>>,
    /// 允许的最大恢复次数（默认 3）。
    maxRecoveryCnt: u32,
    /// 已消耗的恢复次数（含失败/无匹配 handler 的尝试）。
    curRecoveryCnt: u32,
    /// 是否启用 MPP 错误恢复。
    enable: bool,
}

/// 构造恢复处理器：注册默认的内存限额 handler，并初始化结果缓冲。
///
/// `useAutoScaler` 为真时才启用内存限额恢复路径；`holderCap` 为可缓冲响应条数；
/// `parent` 为上层内存 Tracker（内存追踪器），用于把缓冲占用计入会话/查询内存配额。
pub fn NewRecoveryHandler(
    // 是否启用 AutoScaler；关闭时本 handler 永不匹配。
    useAutoScaler: bool,
    holderCap: u64,
    enable: bool,
    parent: &mut Tracker,
) -> RecoveryHandler {
    RecoveryHandler {
        holder: newMPPResultHolder(holderCap, parent),
        handlers: vec![Box::new(newMemLimitHandlerImpl(useAutoScaler))],
        maxRecoveryCnt: 3,
        curRecoveryCnt: 0,
        enable,
    }
}

impl RecoveryHandler {
    /// 是否已启用恢复功能。
    pub fn Enabled(&self) -> bool {
        self.enable
    }

    /// 当前是否仍可继续缓冲响应（容量未满且尚未对外弹出过）。
    pub fn CanHoldResult(&self) -> bool {
        self.holder.capacity > 0 && !self.holder.cannotHold
    }

    /// 将响应放入缓冲。调用方须先检查 `Enabled` 与 `CanHoldResult`，与 Go 约定一致。
    /// The caller must check `Enabled` and `CanHoldResult` before inserting,
    /// matching the Go contract.
    pub fn HoldResult(&mut self, response: MppResponseRef) {
        self.holder.insert(response);
    }

    /// 当前缓冲中的响应条数。
    pub fn NumHoldResp(&self) -> usize {
        self.holder.responses.len()
    }

    /// 弹出队首响应并向外暴露；一旦弹出，后续不可再缓冲，以免重试产生重复可见行。
    pub fn PopFrontResp(&mut self) -> Result<MppResponseRef, errors::SharedError> {
        if !self.enable || self.holder.responses.is_empty() {
            return Err(errors::New(format!(
                "pop resp failed. enable: {}, size: {}",
                self.enable,
                self.holder.responses.len()
            )));
        }

        // 从 FIFO 队列取走队首，并扣减其占用的内存追踪额度。
        let response = self
            .holder
            .responses
            .pop_front()
            .expect("the response queue was checked as non-empty");
        self.holder.memTracker.Consume(-response.MemSize());
        // Once a response has escaped, retrying would duplicate visible rows.
        self.holder.cannotHold = true;
        Ok(response)
    }

    /// 仅重置缓冲动态状态；启用开关与恢复计数有意保持不变。
    /// Reset only the dynamic holder state; enablement and recovery counters
    /// deliberately remain unchanged.
    pub fn ResetHolder(&mut self) {
        self.holder.reset();
    }

    /// 返回已消耗的恢复次数。
    pub fn RecoveryCnt(&self) -> u32 {
        self.curRecoveryCnt
    }

    /// 尝试对给定 MPP 错误执行恢复：校验开关/参数/次数后，按 handlers 顺序匹配并执行。
    pub fn Recovery(&mut self, info: Option<&RecoveryInfo>) -> Result<(), errors::SharedError> {
        if !self.enable {
            return Err(errors::New("mpp err recovery is not enabled"));
        }

        // 必须同时带有非空 RecoveryInfo 与具体 MPP 错误。
        let info = match info {
            Some(info) if info.MPPErr.is_some() => info,
            _ => return Err(errors::New("RecoveryInfo is nil or mppErr is nil")),
        };

        // 超过最大重试次数则放弃，避免无限恢复循环。
        if self.curRecoveryCnt >= self.maxRecoveryCnt {
            return Err(errors::New(format!(
                "exceeds max recovery cnt: cur: {}, max: {}",
                self.curRecoveryCnt, self.maxRecoveryCnt
            )));
        }

        // Go increments before selecting and executing a handler. Unsupported
        // and failed recoveries therefore consume one retry attempt as well.
        self.curRecoveryCnt += 1;

        let mpp_error = info
            .MPPErr
            .as_ref()
            .expect("the MPP error was checked above");
        // 按注册顺序选择第一个声明可处理该错误的实现。
        for handler in &self.handlers {
            if handler.chooseHandlerImpl(mpp_error) {
                return handler.doRecovery(info);
            }
        }
        Err(errors::New("no handler to recovery this type of mpp err"))
    }

    #[cfg(test)]
    /// 测试用：返回缓冲已消耗的字节数。
    pub(crate) fn HolderBytesConsumed(&self) -> i64 {
        self.holder.memTracker.BytesConsumed()
    }
}

/// 具体恢复策略接口：先判断是否匹配错误类型，再执行恢复动作。
pub(crate) trait HandlerImpl: Send + Sync {
    /// 判断本实现是否应处理该 MPP 错误。
    fn chooseHandlerImpl(&self, mppErr: &errors::SharedError) -> bool;
    /// 执行恢复（例如触发 AutoScaler 拓扑重建）。
    fn doRecovery(&self, info: &RecoveryInfo) -> Result<(), errors::SharedError>;
}

/// 针对 TiFlash「Memory limit」错误的恢复实现，依赖 AutoScaler 拓扑拉取器。
struct MemLimitHandlerImpl {
    useAutoScaler: bool,
}

/// 构造内存限额恢复实现。
fn newMemLimitHandlerImpl(useAutoScaler: bool) -> MemLimitHandlerImpl {
    MemLimitHandlerImpl { useAutoScaler }
}

impl HandlerImpl for MemLimitHandlerImpl {
    fn chooseHandlerImpl(&self, mppErr: &errors::SharedError) -> bool {
        // 仅在 AutoScaler 开启且错误文本含 Memory limit 时匹配。
        self.useAutoScaler && mppErr.to_string().contains(MEM_LIMIT_ERR_PATTERN)
    }

    fn doRecovery(&self, info: &RecoveryInfo) -> Result<(), errors::SharedError> {
        // 通过全局 TiFlash compute 拓扑拉取器执行内存限额类恢复。
        let fetcher = tiflashcompute::GetGlobalTopoFetcher().ok_or_else(|| {
            errors::New("global TiFlash compute topology fetcher is not initialized")
        })?;
        // AutoScaler retains the returned topology. Dispatch obtains it again
        // when rebuilding the MPP tasks, exactly as in the Go implementation.
        fetcher
            .RecoveryAndGetTopo(
                tiflashcompute::RecoveryType::RecoveryTypeMemLimit,
                info.NodeCnt,
            )
            .map(|_| ())
            .map_err(|error| errors::New(error.to_string()))
    }
}

/// 响应 FIFO 缓冲：跟踪容量、内存占用，以及是否已不可再 hold。
struct MppResultHolder {
    /// 本缓冲的内存追踪器，挂到父 Tracker 上。
    memTracker: Box<Tracker>,
    /// 按到达顺序存放的响应队列。
    responses: VecDeque<MppResponseRef>,
    /// 可缓冲的最大条数；为 0 时表示不可 hold。
    capacity: u64,
    /// 一旦容量满或已对外弹出，置真以禁止继续缓冲。
    cannotHold: bool,
}

/// 创建结果缓冲，并把子 Tracker 挂到父 Tracker。
fn newMPPResultHolder(holderCap: u64, parent: &mut Tracker) -> MppResultHolder {
    let mut tracker = NewTracker(parent.Label(), 0);
    tracker.AttachTo(parent as *mut Tracker);
    MppResultHolder {
        memTracker: tracker,
        responses: VecDeque::with_capacity(holderCap as usize),
        capacity: holderCap,
        cannotHold: false,
    }
}

impl MppResultHolder {
    /// 追加响应并记账；达到容量后标记不可再 hold。
    fn insert(&mut self, response: MppResponseRef) {
        let memory_size = response.MemSize();
        self.responses.push_back(response);
        // 达到容量上限后禁止继续缓冲，防止无界内存增长。
        if self.responses.len() >= self.capacity as usize {
            self.cannotHold = true;
        }
        self.memTracker.Consume(memory_size);
    }

    /// 清空队列并解除 Tracker 挂接，供下一轮缓冲复用。
    fn reset(&mut self) {
        self.cannotHold = false;
        self.responses.clear();
        self.memTracker.Detach();
    }
}

/// 析构时确保脱离父 Tracker，避免悬挂的内存记账。
impl Drop for MppResultHolder {
    fn drop(&mut self) {
        self.memTracker.Detach();
    }
}
