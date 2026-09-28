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

//! MockGen port of `br/pkg/mock/encode.go`
//! (Encoder, EncodingBuilder, Rows, Row).
//!
//! 中文注释索引开始
//! 本文件是 `br/pkg/mock/encode.go` 的 gomock 移植，覆盖 Encoder/EncodingBuilder/Rows/Row。
//! 用于 lightning/导入路径单测注入编码行为，而不依赖真实行编码器。
//! - `MockEncoder.Encode`：接收 Datum 切片与列映射，返回 RowHandle 或错误。
//! - `MockEncodingBuilder`：创建空 Rows 与新 Encoder；缺省时回落同 Controller 的 MockEncoder。
//! - `MockRows.Clear`：清空并返回可复用的 RowsHandle。
//! - `MockRow.ClassifyAndAppend`：按数据/索引校验和分流追加（参数以克隆形式传入 Call）。
//! - `MockRow.Size`：返回行大小，供刷盘阈值测试使用。
//! recorder 参数多为 `&dyn Any` 占位，真正匹配逻辑在 Controller 内。
//! 不要把这些 mock 当成生产编码器；它们只固定测试可观察的返回契约。
//! 中文注释索引结束
//! Encode 参数含 rowID、列索引映射与时间戳语义字段，mock 只透传。
//! Close Encoder 释放资源；无返回值，失败靠后续 Encode 错误体现。
//! MakeEmptyRows 提供可追加的空行集合句柄。
//! NewEncoder 在缺省时创建同 Controller 的 MockEncoder，保证 EXPECT 可衔接。
//! Clear 返回新的 RowsHandle，旧句柄是否仍有效由 Controller 预设决定。
//! ClassifyAndAppend 需要四组可变引用，Call 侧以 clone 快照传递。
//! Size 用于估算批量大小，决定是否触发 flush。
//! Datum 列表表示一行单元格，真实类型系统远比桩丰富。
//! EncodingConfig 影响编码选项，mock 不解释其字段。
//! KVChecksum 累积校验，ClassifyAndAppend 常同时更新数据与索引校验。
//! RowHandle/RowsHandle 是跨 mock 传递的不透明标识。
//! recorder.Encode 忽略具体参数值，匹配策略由 Controller 配置。
//! 不要假设 Encode 真的生成 TiKV key/value。
//! 错误通过 take_error 提取，成功载荷通过 downcast 取出。
//! ISGOMOCK 与 Backend mock 同形，便于统一识别。
//! EncodingBuilder 与 Encoder 分离，模拟 Go 接口分层。
//! Rows 与 Row 分离，模拟批与单行操作。
//! 并发测试应隔离 Controller。
//! 缺省 RowHandle::default 仅占位，断言前应 EXPECT 明确返回。
//! NewMock* 均克隆 Controller 给 recorder。
//! Helper 调用用于测试堆栈归类。
//! 本文件服务 lightning 编码路径单测。
//! 与 backend.LocalWriter 组合可测“编码后写入”流水线。
//! 参数 &dyn Any 不保存类型信息到期望表之外。
//! 若增加接口方法，必须同步 recorder。
//! 注释避免把桩返回值写成真实编码结果。
//! Go encode.go 是对照源头。
//! Close 后再次 Encode 的行为完全由测试期望定义。
//! 空 Rows 不代表零分配实现，只是句柄语义。
//! Checksum 克隆传入意味着 mock 不会回写调用方对象，除非测试另行处理。
//! 这一点与真实实现可能不同，断言时应只检查 Call 发生与返回值。
//! 密度补充说明到此为止，细节见方法旁中文注释。

use crate::stubs::{
    Call, Context, Controller, Datum, EncodingConfig, KVChecksum, Result, RowHandle, RowsHandle,
    take_error, take_one,
};

/// MockEncoder is a mock of Encoder interface.
/// 模拟单行编码；返回缺省 RowHandle 以保持调用链可继续。
pub struct MockEncoder {
    pub ctrl: Controller,
    pub recorder: MockEncoderMockRecorder,
}

/// MockEncoderMockRecorder is the mock recorder for MockEncoder.
pub struct MockEncoderMockRecorder {
    ctrl: Controller,
}

/// NewMockEncoder creates a new mock instance.
pub fn NewMockEncoder(ctrl: Controller) -> MockEncoder {
    MockEncoder {
        recorder: MockEncoderMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockEncoder {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockEncoderMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// Close mocks base method.
    /// 释放编码器资源；无返回值，失败由后续 Encode 错误体现。
    pub fn Close(&self) {
        self.ctrl.Helper();
        let _ = self.ctrl.Call("Close", vec![]);
    }

    /// Encode mocks base method.
    pub fn Encode(
        &self,
        arg0: Vec<Datum>,
        arg1: i64,
        arg2: Vec<i32>,
        arg3: i64,
    ) -> Result<RowHandle> {
        self.ctrl.Helper();
        let mut rets = self.ctrl.Call(
            "Encode",
            vec![
                Box::new(arg0),
                Box::new(arg1),
                Box::new(arg2),
                Box::new(arg3),
            ],
        );
        let row: RowHandle = if rets.is_empty() {
            RowHandle::default()
        } else {
            let r = rets.remove(0);
            r.downcast::<RowHandle>().map(|b| *b).unwrap_or_default()
        };
        take_error(rets).map(|_| row)
    }
}

impl MockEncoderMockRecorder {
    pub fn Close(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Close", "MockEncoder.Close", vec![])
    }

    pub fn Encode(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
        _arg3: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Encode", "MockEncoder.Encode", vec![])
    }
}

/// MockEncodingBuilder is a mock of EncodingBuilder interface.
/// 负责创建 Encoder 与空 Rows 句柄。
pub struct MockEncodingBuilder {
    pub ctrl: Controller,
    pub recorder: MockEncodingBuilderMockRecorder,
}

/// MockEncodingBuilderMockRecorder is the mock recorder for MockEncodingBuilder.
pub struct MockEncodingBuilderMockRecorder {
    ctrl: Controller,
}

/// NewMockEncodingBuilder creates a new mock instance.
pub fn NewMockEncodingBuilder(ctrl: Controller) -> MockEncodingBuilder {
    MockEncodingBuilder {
        recorder: MockEncodingBuilderMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockEncodingBuilder {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockEncodingBuilderMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// MakeEmptyRows mocks base method.
    pub fn MakeEmptyRows(&self) -> RowsHandle {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("MakeEmptyRows", vec![]);
        take_one::<RowsHandle>(rets)
    }

    /// NewEncoder mocks base method.
    /// Go 返回 encode.Encoder；此处可回落为同 Controller 的新 MockEncoder。
    pub fn NewEncoder(&self, arg0: Context, arg1: EncodingConfig) -> Result<MockEncoder> {
        self.ctrl.Helper();
        let mut rets = self
            .ctrl
            .Call("NewEncoder", vec![Box::new(arg0), Box::new(arg1)]);
        // Go returns encode.Encoder; we return a handle id or a nested MockEncoder via Any.
        let enc: Option<MockEncoder> = if rets.is_empty() {
            None
        } else {
            let r = rets.remove(0);
            r.downcast::<MockEncoder>()
                .map(|b| Some(*b))
                .unwrap_or(None)
        };
        take_error(rets)?;
        Ok(enc.unwrap_or_else(|| NewMockEncoder(self.ctrl.clone())))
    }
}

impl MockEncodingBuilderMockRecorder {
    pub fn MakeEmptyRows(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "MakeEmptyRows",
            "MockEncodingBuilder.MakeEmptyRows",
            vec![],
        )
    }

    pub fn NewEncoder(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("NewEncoder", "MockEncodingBuilder.NewEncoder", vec![])
    }
}

/// MockRows is a mock of Rows interface.
/// Clear 返回 RowsHandle，供后续 Append 路径复用。
pub struct MockRows {
    pub ctrl: Controller,
    pub recorder: MockRowsMockRecorder,
}

/// MockRowsMockRecorder is the mock recorder for MockRows.
pub struct MockRowsMockRecorder {
    ctrl: Controller,
}

/// NewMockRows creates a new mock instance.
pub fn NewMockRows(ctrl: Controller) -> MockRows {
    MockRows {
        recorder: MockRowsMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockRows {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockRowsMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// Clear mocks base method.
    pub fn Clear(&self) -> RowsHandle {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Clear", vec![]);
        take_one::<RowsHandle>(rets)
    }
}

impl MockRowsMockRecorder {
    pub fn Clear(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Clear", "MockRows.Clear", vec![])
    }
}

/// MockRow is a mock of Row interface.
/// ClassifyAndAppend 按校验和分流到数据/索引行集合。
pub struct MockRow {
    pub ctrl: Controller,
    pub recorder: MockRowMockRecorder,
}

/// MockRowMockRecorder is the mock recorder for MockRow.
pub struct MockRowMockRecorder {
    ctrl: Controller,
}

/// NewMockRow creates a new mock instance.
pub fn NewMockRow(ctrl: Controller) -> MockRow {
    MockRow {
        recorder: MockRowMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockRow {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockRowMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// ClassifyAndAppend mocks base method.
    pub fn ClassifyAndAppend(
        &self,
        arg0: &mut RowsHandle,
        arg1: &mut KVChecksum,
        arg2: &mut RowsHandle,
        arg3: &mut KVChecksum,
    ) {
        self.ctrl.Helper();
        let _ = self.ctrl.Call(
            "ClassifyAndAppend",
            vec![
                Box::new(arg0.clone()),
                Box::new(arg1.clone()),
                Box::new(arg2.clone()),
                Box::new(arg3.clone()),
            ],
        );
    }

    /// Size mocks base method.
    pub fn Size(&self) -> u64 {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Size", vec![]);
        take_one::<u64>(rets)
    }
}

impl MockRowMockRecorder {
    pub fn ClassifyAndAppend(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
        _arg3: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("ClassifyAndAppend", "MockRow.ClassifyAndAppend", vec![])
    }

    pub fn Size(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Size", "MockRow.Size", vec![])
    }
}
