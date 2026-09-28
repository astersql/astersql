// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! MockGen port of `br/pkg/mock/mocklocal/local.go`
//! (DiskUsage, TiKVModeSwitcher, StoreHelper).
//!
//! 将 Go MockGen 产物中的 DiskUsage / TiKVModeSwitcher / StoreHelper
//! 移植为 Rust gomock 风格：Controller 录制期望，方法调用经 Call 取返回值。
//! 不实现真实磁盘占用、TiKV 模式切换或 TSO；仅供 lightning ingest 控制面单测。
//! 依赖 [`crate::stubs`] 的轻量类型，避免拉入 kvproto/grpcio。

use std::any::Any;

use astersql_br_pkg_mock::stubs::take_one;
use astersql_br_pkg_mock::{Call, Context, Controller};

use crate::stubs::{Codec, EngineFileSize, Range, take_ts};

/// MockDiskUsage is a mock of DiskUsage interface.
///
/// DiskUsage 替身：持有 Controller 与 recorder，EngineFileSizes 走录制返回值。
pub struct MockDiskUsage {
    pub ctrl: Controller,
    pub recorder: MockDiskUsageMockRecorder,
}

/// MockDiskUsageMockRecorder is the mock recorder for MockDiskUsage.
///
/// EXPECT 链上的录制器：登记 EngineFileSizes 期望调用（对齐 Go recorder）。
pub struct MockDiskUsageMockRecorder {
    ctrl: Controller,
}

/// NewMockDiskUsage creates a new mock instance.
///
/// 共享同一 Controller 给 mock 与 recorder，保证录制/回放一致。
pub fn NewMockDiskUsage(ctrl: Controller) -> MockDiskUsage {
    MockDiskUsage {
        recorder: MockDiskUsageMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockDiskUsage {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    ///
    /// 返回 recorder，供测试写 `EXPECT().EngineFileSizes().Return1(...)`。
    pub fn EXPECT(&self) -> &MockDiskUsageMockRecorder {
        &self.recorder
    }

    /// EngineFileSizes mocks base method.
    ///
    /// 回放无参调用，downcast 为 `Vec<EngineFileSize>`（Go 切片同形）。
    pub fn EngineFileSizes(&self) -> Vec<EngineFileSize> {
        self.ctrl.Helper();
        let ret = self.ctrl.Call("EngineFileSizes", vec![]);
        take_one::<Vec<EngineFileSize>>(ret)
    }
}

impl MockDiskUsageMockRecorder {
    /// EngineFileSizes indicates an expected call of EngineFileSizes.
    ///
    /// 登记期望；方法类型名带完整路径，便于失败信息定位。
    pub fn EngineFileSizes(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "EngineFileSizes",
            "MockDiskUsage.EngineFileSizes",
            vec![],
        )
    }
}

/// MockTiKVModeSwitcher is a mock of TiKVModeSwitcher interface.
///
/// TiKV 导入/正常模式切换的 mock：ToImportMode / ToNormalMode 仅校验调用序列。
pub struct MockTiKVModeSwitcher {
    pub ctrl: Controller,
    pub recorder: MockTiKVModeSwitcherMockRecorder,
}

/// MockTiKVModeSwitcherMockRecorder is the mock recorder for MockTiKVModeSwitcher.
///
/// 模式切换期望录制器；参数在录制侧用 Any 占位，与 Go reflect 风格一致。
pub struct MockTiKVModeSwitcherMockRecorder {
    ctrl: Controller,
}

/// NewMockTiKVModeSwitcher creates a new mock instance.
///
/// 构造模式切换 mock；ctrl 需由测试在断言前 `Finish`/耗尽 remaining。
pub fn NewMockTiKVModeSwitcher(ctrl: Controller) -> MockTiKVModeSwitcher {
    MockTiKVModeSwitcher {
        recorder: MockTiKVModeSwitcherMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockTiKVModeSwitcher {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    ///
    /// 返回模式切换 recorder，用于登记 ToImportMode / ToNormalMode 期望。
    pub fn EXPECT(&self) -> &MockTiKVModeSwitcherMockRecorder {
        &self.recorder
    }

    /// ToImportMode mocks base method.
    ///
    /// 将 ctx 与各 Range 装箱为可变参数列表，对齐 Go 变参 Call 形态；忽略返回值。
    pub fn ToImportMode(&self, arg0: Context, arg1: &[Range]) {
        self.ctrl.Helper();
        let mut varargs: Vec<Box<dyn Any + Send>> = vec![Box::new(arg0)];
        for a in arg1 {
            varargs.push(Box::new(a.clone()));
        }
        let _ = self.ctrl.Call("ToImportMode", varargs);
    }

    /// ToNormalMode mocks base method.
    ///
    /// 与 ToImportMode 对称：切回正常模式，同样只驱动 Controller 回放。
    pub fn ToNormalMode(&self, arg0: Context, arg1: &[Range]) {
        self.ctrl.Helper();
        let mut varargs: Vec<Box<dyn Any + Send>> = vec![Box::new(arg0)];
        for a in arg1 {
            varargs.push(Box::new(a.clone()));
        }
        let _ = self.ctrl.Call("ToNormalMode", varargs);
    }
}

impl MockTiKVModeSwitcherMockRecorder {
    /// ToImportMode indicates an expected call of ToImportMode.
    ///
    /// 录制侧参数占位未写入 Call 向量：匹配依赖方法名与调用次序（同 Go MockGen）。
    pub fn ToImportMode(&self, _arg0: &dyn Any, _arg1: &[&dyn Any]) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "ToImportMode",
            "MockTiKVModeSwitcher.ToImportMode",
            vec![],
        )
    }

    /// ToNormalMode indicates an expected call of ToNormalMode.
    ///
    /// 登记切回正常模式的期望调用；参数占位策略同 ToImportMode。
    pub fn ToNormalMode(&self, _arg0: &dyn Any, _arg1: &[&dyn Any]) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "ToNormalMode",
            "MockTiKVModeSwitcher.ToNormalMode",
            vec![],
        )
    }
}

/// MockStoreHelper is a mock of StoreHelper interface.
///
/// StoreHelper 替身：GetTS / GetTiKVCodec，供需要时间戳或 Codec 的 ingest 路径。
pub struct MockStoreHelper {
    pub ctrl: Controller,
    pub recorder: MockStoreHelperMockRecorder,
}

/// MockStoreHelperMockRecorder is the mock recorder for MockStoreHelper.
///
/// StoreHelper 期望录制器：GetTS / GetTiKVCodec 的 Return 链挂在此处。
pub struct MockStoreHelperMockRecorder {
    ctrl: Controller,
}

/// NewMockStoreHelper creates a new mock instance.
///
/// 构造 StoreHelper mock；与 Go NewMockStoreHelper(ctrl) 一一对应。
pub fn NewMockStoreHelper(ctrl: Controller) -> MockStoreHelper {
    MockStoreHelper {
        recorder: MockStoreHelperMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockStoreHelper {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    ///
    /// 返回 StoreHelper recorder，供预置 TSO/Codec 返回值。
    pub fn EXPECT(&self) -> &MockStoreHelperMockRecorder {
        &self.recorder
    }

    /// GetTS mocks base method.
    ///
    /// Go: `GetTS(ctx) (physical, logical int64, err error)`.
    ///
    /// 经 `take_ts` 解包三元组；错误可预置为 Some，模拟 TSO 不可用。
    pub fn GetTS(&self, arg0: Context) -> (i64, i64, Option<astersql_br_pkg_mock::Error>) {
        self.ctrl.Helper();
        let ret = self.ctrl.Call("GetTS", vec![Box::new(arg0)]);
        take_ts(ret)
    }

    /// GetTiKVCodec mocks base method.
    ///
    /// 返回桩 Codec；真实编解码能力不在本 mock 范围内。
    pub fn GetTiKVCodec(&self) -> Codec {
        self.ctrl.Helper();
        let ret = self.ctrl.Call("GetTiKVCodec", vec![]);
        take_one::<Codec>(ret)
    }
}

impl MockStoreHelperMockRecorder {
    /// GetTS indicates an expected call of GetTS.
    ///
    /// 登记 GetTS 期望；测试侧用 Return 填入 physical/logical/err 三槽。
    pub fn GetTS(&self, _arg0: &dyn Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("GetTS", "MockStoreHelper.GetTS", vec![])
    }

    /// GetTiKVCodec indicates an expected call of GetTiKVCodec.
    ///
    /// 登记 GetTiKVCodec 期望；通常配合 Return1(Codec{..})。
    pub fn GetTiKVCodec(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("GetTiKVCodec", "MockStoreHelper.GetTiKVCodec", vec![])
    }
}
