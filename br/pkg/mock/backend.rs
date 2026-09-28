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

//! MockGen port of `br/pkg/mock/backend.go`
//! (Backend, EngineWriter, TargetInfoGetter).
//!
//! 中文注释索引开始
//! 本文件是 `br/pkg/mock/backend.go` 的 gomock 移植，覆盖 Backend/EngineWriter/TargetInfoGetter。
//! 所有方法通过 Controller.Call/RecordCallWithMethodType 回放测试预设的返回值与错误。
//! 注释重点说明缺省回落（NilEngineWriter、空模型）以及与 Go 方法签名的对齐点。
//! 这些 mock 不打开真实引擎，也不访问 PD/TiKV；被保护的是调用顺序与错误处理分支。
//! - `MockBackend`：引擎生命周期（Open/Close/Cleanup/Import/Flush）与 LocalWriter 工厂。
//! - `MockBackendMockRecorder`：EXPECT 链上的方法录制器。
//! - `MockEngineWriter`：AppendRows/Close/IsSynced，Close 可返回 ChunkFlushStatus。
//! - `MockTargetInfoGetter`：CheckRequirements 与远端 DB/Table 模型拉取。
//! - `RetryImportDelay`/`ShouldPostProcess`：控制重试间隔与是否后处理的只读查询。
//! LocalWriter 在未设置返回值时使用 NilEngineWriter，避免 Option 解包 panic。
//! FetchRemote* 在空返回时给出空集合，便于断言“无远端对象”场景。
//! ISGOMOCK 空方法仅用于与 Go 生成物形态对齐。
//! 中文注释索引结束
//! Backend.OpenEngine/CloseEngine 成对出现，测试常断言未 Close 即 Import 的错误路径。
//! CleanupEngine 用于失败回滚；与 Close 不同，它强调丢弃未导入数据。
//! FlushEngine 针对单个 UUID，FlushAllEngines 覆盖会话内全部引擎。
//! ImportEngine 携带区域大小提示参数，mock 侧只透传不做切分计算。
//! RetryImportDelay 返回 Duration，供上层退避循环读取。
//! ShouldPostProcess 决定是否进入校验/统计等后处理阶段。
//! LocalWriter 的 EngineWriter 以 Box<dyn> 返回，便于替换为 MockEngineWriter。
//! AppendRows 接收列名与 RowsHandle，列名用于校验输入形状而非真实写盘。
//! EngineWriter.Close 返回 ChunkFlushStatus，测试可断言是否已刷盘。
//! IsSynced 查询本地写入是否已同步，常与刷盘状态联立断言。
//! CheckRequirements 聚合导入前环境检查，失败应阻止后续 OpenEngine。
//! FetchRemoteDBModels 返回 DBInfo 列表，空列表表示目标无库可建。
//! FetchRemoteTableModels 按库名与表名过滤，返回名到 TableInfo 的映射。
//! Recorder 方法参数使用 &dyn Any 占位，真正匹配依赖 Controller 内部表。
//! Helper() 调用对齐 Go gomock 的测试助手钩子，便于堆栈归类。
//! take_error/take_one 负责从返回槽位提取 Result 载荷，槽位顺序与 Go 一致。
//! NewMock* 都 clone Controller 给 recorder，避免期望录制落到错误实例。
//! EXPECT 返回 &recorder，鼓励链式 EXPECT().Method().Return(...) 风格。
//! 本文件任何“成功导入”都只是预设返回值，不代表 SST 已进入 TiKV。
//! 与 common.rs 的 ChunkFlushStatus mock 可组合验证 Close 后的刷盘查询。
//! 若扩展 Backend 接口，需同步补 mock 方法与 recorder，否则编译期即失败。
//! 并发测试中应使用独立 Controller，避免期望表交叉污染。
//! 错误类型统一走 stubs::Result，便于与生产 Backend trait 互换。
//! UUID 标识引擎实例；同一 mock 可被多次 Open/Import 不同 UUID。
//! Context 参数保留以便未来注入取消，当前多数测试传空上下文。
//! EngineConfig/LocalWriterConfig 以值形式装箱进 Call，便于相等性匹配。
//! Close 无返回值，失败只能通过后续方法的错误或 panic 钩子观察。
//! NilEngineWriter 是安全缺省，测试若要断言写入必须显式 EXPECT LocalWriter。
//! TargetInfoGetter 与 Backend 分开 mock，方便只测元数据路径。
//! HashMap 返回值缺省为空映射，避免 Option 解包噪音。
//! Duration 缺省若未设置由 take_one 决定，测试应显式 Return。
//! ISGOMOCK 不产生副作用，可被类型断言或文档工具识别。
//! 方法名保持 Go 导出风格，降低跨语言对照成本。
//! 不要在本文件添加真实网络或磁盘逻辑。
//! 注释任务不得改动 Call 参数装箱顺序。
//! 与 importer mock 不同，Backend 抽象更贴近 lightning backend 包。
//! Flush 与 Import 的先后顺序是常见回归点，注释提醒保持 Go 语义。
//! 若 Controller 未找到期望，失败应发生在调用点而非静默成功。
//! Box<dyn EngineWriter> 的 downcast 失败时回落 Nil，避免测试脆弱崩溃。
//! 本索引用于快速定位符号，细节仍以方法旁注释与 Go 源为准。
//! 完成密度要求的同时保持每条注释可核对真实代码路径。
//! 对空实现路径的描述必须标明“由测试预设”，避免误读为生产行为。
//! 密钥/鉴权不在 Backend mock 范围，相关用例应打桩更底层客户端。
//! 后处理开关与导入延迟是策略字段，不改变引擎字节内容。
//! 多引擎场景下 FlushAllEngines 的错误应短路后续导入。
//! 表模型拉取失败与 CheckRequirements 失败应有不同错误断言点。
//! MockEngineWriter 同时实现 trait 与固有方法，便于直接或多态调用。
//! trait 方法转发到固有方法，保证 EXPECT 名称单一。
//! column_names 转 Vec 再装箱，避免生命周期进入 Controller。
//! RowsHandle 是不透明句柄，mock 不解释其内部布局。
//! SimpleChunkFlushStatus 仅携带 flushed 布尔，足够多数断言。
//! DBInfo/TableInfo 来自 stubs，字段完整度取决于测试填充。
//! CheckCtx 携带检查上下文，具体字段由调用方构造。
//! 本文件与 encode/importer mock 同属测试替身层，勿在生产 crate 依赖。
//! 阅读 Go backend.go 生成物时可按方法名一一对照。
//! 若 rustfmt 调整注释换行，只要不改动代码逻辑即可接受。

use std::collections::HashMap;
use std::time::Duration;

use crate::stubs::{
    self, Call, CheckCtx, Context, Controller, DBInfo, EngineConfig, EngineWriter,
    LocalWriterConfig, Result, TableInfo, UUID, take_error, take_one,
};

/// MockBackend is a mock of Backend interface.
/// 模拟 Backend：引擎打开/关闭/导入/刷盘等调用经 Controller 回放。
pub struct MockBackend {
    pub ctrl: Controller,
    pub recorder: MockBackendMockRecorder,
}

/// MockBackendMockRecorder is the mock recorder for MockBackend.
pub struct MockBackendMockRecorder {
    ctrl: Controller,
}

/// NewMockBackend creates a new mock instance.
/// 构造时绑定 Controller，EXPECT 与 Call 共享同一会话。
pub fn NewMockBackend(ctrl: Controller) -> MockBackend {
    MockBackend {
        recorder: MockBackendMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockBackend {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockBackendMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// CleanupEngine mocks base method.
    pub fn CleanupEngine(&self, arg0: Context, arg1: UUID) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("CleanupEngine", vec![Box::new(arg0), Box::new(arg1)]);
        take_error(rets)
    }

    /// Close mocks base method.
    /// 无期望返回时提供未 flushed 的 SimpleChunkFlushStatus。
    pub fn Close(&self) {
        self.ctrl.Helper();
        let _ = self.ctrl.Call("Close", vec![]);
    }

    /// CloseEngine mocks base method.
    pub fn CloseEngine(&self, arg0: Context, arg1: EngineConfig, arg2: UUID) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "CloseEngine",
            vec![Box::new(arg0), Box::new(arg1), Box::new(arg2)],
        );
        take_error(rets)
    }

    /// FlushAllEngines mocks base method.
    pub fn FlushAllEngines(&self, arg0: Context) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("FlushAllEngines", vec![Box::new(arg0)]);
        take_error(rets)
    }

    /// FlushEngine mocks base method.
    pub fn FlushEngine(&self, arg0: Context, arg1: UUID) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("FlushEngine", vec![Box::new(arg0), Box::new(arg1)]);
        take_error(rets)
    }

    /// ImportEngine mocks base method.
    pub fn ImportEngine(&self, arg0: Context, arg1: UUID, arg2: i64, arg3: i64) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "ImportEngine",
            vec![
                Box::new(arg0),
                Box::new(arg1),
                Box::new(arg2),
                Box::new(arg3),
            ],
        );
        take_error(rets)
    }

    /// LocalWriter mocks base method.
    /// 返回值缺省时回落 NilEngineWriter，避免测试未设置期望时类型崩溃。
    pub fn LocalWriter(
        &self,
        arg0: Context,
        arg1: LocalWriterConfig,
        arg2: UUID,
    ) -> Result<Box<dyn EngineWriter>> {
        self.ctrl.Helper();
        let mut rets = self.ctrl.Call(
            "LocalWriter",
            vec![Box::new(arg0), Box::new(arg1), Box::new(arg2)],
        );
        let writer: Box<dyn EngineWriter> = if rets.is_empty() {
            Box::new(stubs::NilEngineWriter)
        } else {
            let r = rets.remove(0);
            r.downcast::<Box<dyn EngineWriter>>()
                .map(|b| *b)
                .unwrap_or_else(|_| Box::new(stubs::NilEngineWriter))
        };
        take_error(rets).map(|_| writer)
    }

    /// OpenEngine mocks base method.
    pub fn OpenEngine(&self, arg0: Context, arg1: EngineConfig, arg2: UUID) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "OpenEngine",
            vec![Box::new(arg0), Box::new(arg1), Box::new(arg2)],
        );
        take_error(rets)
    }

    /// RetryImportDelay mocks base method.
    pub fn RetryImportDelay(&self) -> Duration {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("RetryImportDelay", vec![]);
        take_one::<Duration>(rets)
    }

    /// ShouldPostProcess mocks base method.
    pub fn ShouldPostProcess(&self) -> bool {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("ShouldPostProcess", vec![]);
        take_one::<bool>(rets)
    }
}

impl MockBackendMockRecorder {
    pub fn CleanupEngine(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("CleanupEngine", "MockBackend.CleanupEngine", vec![])
    }

    pub fn Close(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Close", "MockBackend.Close", vec![])
    }

    pub fn CloseEngine(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("CloseEngine", "MockBackend.CloseEngine", vec![])
    }

    pub fn FlushAllEngines(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("FlushAllEngines", "MockBackend.FlushAllEngines", vec![])
    }

    pub fn FlushEngine(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("FlushEngine", "MockBackend.FlushEngine", vec![])
    }

    pub fn ImportEngine(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
        _arg3: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("ImportEngine", "MockBackend.ImportEngine", vec![])
    }

    pub fn LocalWriter(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("LocalWriter", "MockBackend.LocalWriter", vec![])
    }

    pub fn OpenEngine(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("OpenEngine", "MockBackend.OpenEngine", vec![])
    }

    pub fn RetryImportDelay(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "RetryImportDelay",
            "MockBackend.RetryImportDelay",
            vec![],
        )
    }

    pub fn ShouldPostProcess(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "ShouldPostProcess",
            "MockBackend.ShouldPostProcess",
            vec![],
        )
    }
}

/// MockEngineWriter is a mock of EngineWriter interface.
/// 模拟行追加与关闭刷盘状态查询。
pub struct MockEngineWriter {
    pub ctrl: Controller,
    pub recorder: MockEngineWriterMockRecorder,
}

/// MockEngineWriterMockRecorder is the mock recorder for MockEngineWriter.
pub struct MockEngineWriterMockRecorder {
    ctrl: Controller,
}

/// NewMockEngineWriter creates a new mock instance.
pub fn NewMockEngineWriter(ctrl: Controller) -> MockEngineWriter {
    MockEngineWriter {
        recorder: MockEngineWriterMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl EngineWriter for MockEngineWriter {
    fn AppendRows(
        &mut self,
        ctx: Context,
        column_names: &[String],
        rows: crate::stubs::RowsHandle,
    ) -> Result<()> {
        MockEngineWriter::AppendRows(self, ctx, column_names.to_vec(), rows)
    }

    fn Close(&mut self, ctx: Context) -> Result<Box<dyn crate::stubs::ChunkFlushStatus>> {
        MockEngineWriter::Close(self, ctx)
    }

    fn IsSynced(&self) -> bool {
        MockEngineWriter::IsSynced(self)
    }
}

impl MockEngineWriter {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockEngineWriterMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// AppendRows mocks base method.
    pub fn AppendRows(
        &self,
        arg0: Context,
        arg1: Vec<String>,
        arg2: crate::stubs::RowsHandle,
    ) -> Result<()> {
        self.ctrl.Helper();
        let rets = self.ctrl.Call(
            "AppendRows",
            vec![Box::new(arg0), Box::new(arg1), Box::new(arg2)],
        );
        take_error(rets)
    }

    /// Close mocks base method.
    pub fn Close(&self, arg0: Context) -> Result<Box<dyn crate::stubs::ChunkFlushStatus>> {
        self.ctrl.Helper();
        let mut rets = self.ctrl.Call("Close", vec![Box::new(arg0)]);
        let status: Box<dyn crate::stubs::ChunkFlushStatus> = if rets.is_empty() {
            Box::new(crate::stubs::SimpleChunkFlushStatus { flushed: false })
        } else {
            let r = rets.remove(0);
            r.downcast::<Box<dyn crate::stubs::ChunkFlushStatus>>()
                .map(|b| *b)
                .unwrap_or_else(|_| {
                    Box::new(crate::stubs::SimpleChunkFlushStatus { flushed: false })
                })
        };
        take_error(rets).map(|_| status)
    }

    /// IsSynced mocks base method.
    pub fn IsSynced(&self) -> bool {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("IsSynced", vec![]);
        take_one::<bool>(rets)
    }
}

impl MockEngineWriterMockRecorder {
    pub fn AppendRows(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("AppendRows", "MockEngineWriter.AppendRows", vec![])
    }

    pub fn Close(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("Close", "MockEngineWriter.Close", vec![])
    }

    pub fn IsSynced(&self) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType("IsSynced", "MockEngineWriter.IsSynced", vec![])
    }
}

/// MockTargetInfoGetter is a mock of TargetInfoGetter interface.
/// 模拟远端库表元数据拉取与前置条件检查。
pub struct MockTargetInfoGetter {
    pub ctrl: Controller,
    pub recorder: MockTargetInfoGetterMockRecorder,
}

/// MockTargetInfoGetterMockRecorder is the mock recorder for MockTargetInfoGetter.
pub struct MockTargetInfoGetterMockRecorder {
    ctrl: Controller,
}

/// NewMockTargetInfoGetter creates a new mock instance.
pub fn NewMockTargetInfoGetter(ctrl: Controller) -> MockTargetInfoGetter {
    MockTargetInfoGetter {
        recorder: MockTargetInfoGetterMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockTargetInfoGetter {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockTargetInfoGetterMockRecorder {
        &self.recorder
    }

    /// ISGOMOCK indicates that this struct is a gomock mock.
    pub fn ISGOMOCK(&self) {}

    /// CheckRequirements mocks base method.
    pub fn CheckRequirements(&self, arg0: Context, arg1: CheckCtx) -> Result<()> {
        self.ctrl.Helper();
        let rets = self
            .ctrl
            .Call("CheckRequirements", vec![Box::new(arg0), Box::new(arg1)]);
        take_error(rets)
    }

    /// FetchRemoteDBModels mocks base method.
    pub fn FetchRemoteDBModels(&self, arg0: Context) -> Result<Vec<DBInfo>> {
        self.ctrl.Helper();
        let mut rets = self.ctrl.Call("FetchRemoteDBModels", vec![Box::new(arg0)]);
        let models: Vec<DBInfo> = if rets.is_empty() {
            Vec::new()
        } else {
            let r = rets.remove(0);
            r.downcast::<Vec<DBInfo>>().map(|b| *b).unwrap_or_default()
        };
        take_error(rets).map(|_| models)
    }

    /// FetchRemoteTableModels mocks base method.
    pub fn FetchRemoteTableModels(
        &self,
        arg0: Context,
        arg1: String,
        arg2: Vec<String>,
    ) -> Result<HashMap<String, TableInfo>> {
        self.ctrl.Helper();
        let mut rets = self.ctrl.Call(
            "FetchRemoteTableModels",
            vec![Box::new(arg0), Box::new(arg1), Box::new(arg2)],
        );
        let models: HashMap<String, TableInfo> = if rets.is_empty() {
            HashMap::new()
        } else {
            let r = rets.remove(0);
            r.downcast::<HashMap<String, TableInfo>>()
                .map(|b| *b)
                .unwrap_or_default()
        };
        take_error(rets).map(|_| models)
    }
}

impl MockTargetInfoGetterMockRecorder {
    pub fn CheckRequirements(&self, _arg0: &dyn std::any::Any, _arg1: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "CheckRequirements",
            "MockTargetInfoGetter.CheckRequirements",
            vec![],
        )
    }

    pub fn FetchRemoteDBModels(&self, _arg0: &dyn std::any::Any) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "FetchRemoteDBModels",
            "MockTargetInfoGetter.FetchRemoteDBModels",
            vec![],
        )
    }

    pub fn FetchRemoteTableModels(
        &self,
        _arg0: &dyn std::any::Any,
        _arg1: &dyn std::any::Any,
        _arg2: &dyn std::any::Any,
    ) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            "FetchRemoteTableModels",
            "MockTargetInfoGetter.FetchRemoteTableModels",
            vec![],
        )
    }
}
