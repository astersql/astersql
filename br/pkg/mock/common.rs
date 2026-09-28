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

//! MockGen port of `br/pkg/mock/common.go` (ChunkFlushStatus).
//!
//! 本文件是 gomock 风格桩，仅服务测试对 `ChunkFlushStatus.Flushed` 的期望注入。
//! 不代表生产引擎真的用这套 Controller/Call 调度；调用必须先 EXPECT 再触发。
//! 与 Go `br/pkg/mock/common.go` 生成代码保持方法名与 recorder 语义一致。

use crate::stubs::{Call, Controller, take_one};

/// MockChunkFlushStatus is a mock of ChunkFlushStatus interface.
/// 持有 Controller 与 recorder，分别负责实际调用分发与期望录制。
pub struct MockChunkFlushStatus {
    pub ctrl: Controller,
    pub recorder: MockChunkFlushStatusMockRecorder,
}

/// MockChunkFlushStatusMockRecorder is the mock recorder for MockChunkFlushStatus.
/// 通过 EXPECT 链登记 `Flushed` 的预期调用。
pub struct MockChunkFlushStatusMockRecorder {
    ctrl: Controller,
}

/// NewMockChunkFlushStatus creates a new mock instance.
/// 构造时共享同一 Controller，保证 Call 与 Record 落在同一会话。
pub fn NewMockChunkFlushStatus(ctrl: Controller) -> MockChunkFlushStatus {
    MockChunkFlushStatus {
        recorder: MockChunkFlushStatusMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl crate::stubs::ChunkFlushStatus for MockChunkFlushStatus {
    fn Flushed(&self) -> bool {
        MockChunkFlushStatus::Flushed(self)
    }
}

impl MockChunkFlushStatus {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    /// 返回 recorder，供测试编排期望调用序列。
    pub fn EXPECT(&self) -> &MockChunkFlushStatusMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    /// 空方法，仅用于与 Go gomock 生成物接口形态对齐。
    pub fn ISGOMOCK(&self) {}

    /// Flushed mocks base method.
    /// 经 Controller.Call 取回预置 bool；未设置期望时行为由 Controller 决定。
    pub fn Flushed(&self) -> bool {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Flushed", vec![]);
        take_one::<bool>(rets)
    }
}

impl MockChunkFlushStatusMockRecorder {
    /// Flushed indicates an expected call of Flushed.
    /// 登记期望时写入方法类型字符串，便于失败信息定位到具体 mock。
    pub fn Flushed(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Flushed", "MockChunkFlushStatus.Flushed", vec![])
    }
}
