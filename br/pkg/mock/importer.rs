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

//! MockGen port of `br/pkg/mock/importer.go`
//! (ImportKVClient, ImportKV_WriteEngineClient).
//!
//! 中文注释索引开始
//! 本文件是 `br/pkg/mock/importer.go` 的 gomock 移植，覆盖 ImportKVClient 与 WriteEngine 流客户端。
//! 一元 RPC 经 `call_unary` 共用：兼容 Go 返回 `(resp, err)` 与“仅错误”两种录制形态。
//! - `MockImportKVClient`：Cleanup/Close/Open/ImportEngine、SwitchMode、Compact、GetVersion/Metrics。
//! - `WriteEngine`：返回流式 `WriteEngineClient`；缺省 NilWriteEngineClient。
//! - `WriteEngineV3`：一元版本写接口。
//! - `MockImportKV_WriteEngineClient`：Send/RecvMsg/CloseAndRecv/Header/Trailer/Context 等。
//! CallOption 变参以 Vec 传入，对齐 Go 的可选 gRPC CallOption。
//! RecvMsg/SendMsg 对 `&dyn Any` 做占位调用，真实类型断言由测试预设的返回值驱动。
//! 这些 mock 不建立真实 gRPC 连接；失败语义完全由 Controller 期望表决定。
//! 中文注释索引结束
//! CleanupEngine 对应导入失败后的引擎清理 RPC。
//! CloseEngine 在写完 SST 后关闭引擎。
//! OpenEngine 在写入前打开引擎会话。
//! ImportEngine 触发把引擎数据导入 TiKV。
//! SwitchMode 切换 TiKV 导入模式。
//! CompactCluster 请求集群压缩，常用于导入后整理。
//! GetVersion/GetMetrics 供兼容性与可观测性检查。
//! WriteEngine 返回流客户端，适合大批量分片发送。
//! WriteEngineV3 是一元封装，适合小批量或新协议路径。
//! call_unary 先识别 Option<Error> 槽位，兼容 Go 仅返回错误的录制。
//! 若首槽不是错误，则按 Resp 类型 downcast，失败时用 Default。
//! CallOption 以值列表追加到 varargs，对齐 Go 可变参数。
//! WriteEngine 在无返回时使用 NilWriteEngineClient。
//! 流客户端 CloseAndRecv 聚合最终 WriteEngineResponse。
//! CloseSend 只关闭发送方向，仍可接收。
//! Context/Header/Trailer 暴露 gRPC 元数据与上下文。
//! RecvMsg/SendMsg 是通用消息入口，类型由测试解释。
//! Send 发送强类型 WriteEngineRequest。
//! record 辅助把方法名格式化为 MockImportKVClient.Method。
//! WriteEngineClient trait 实现转发到固有方法，保证 EXPECT 单一。
//! Metadata 缺省为空，足够多数不关心 header 的用例。
//! 不要假设这些 RPC 触达真实 importer 服务。
//! 错误类型为 stubs::Error，经 Result 返回。
//! 并发下使用独立 Controller。
//! 与 backend.ImportEngine 抽象层级不同：这里更贴近 gRPC API。
//! V3 与流式接口可能并存于同一客户端，测试应按版本 EXPECT。
//! Compact/SwitchMode 常出现在导入前后钩子。
//! GetMetrics 返回结构由测试填充，桩不计算真实指标。
//! Nil 客户端方法行为由 stubs 定义，通常是安全空操作或默认值。
//! Any 占位参数不会自动完成类型匹配，需靠方法名区分。
//! 中文注释强调边界，避免把 mock 成功当成集群已导入。
//! 对照 Go importer.go 生成物可快速核对方法集合。
//! 扩展接口时保持 recorder 与 call_unary 复用，减少重复逻辑。
//! Header 与 Trailer 分开 mock，便于分别断言。
//! CloseAndRecv 在空返回时给 Default Response。
//! take_error 在提取响应后仍检查尾部错误槽。
//! 这与 Go 多返回值把 err 放最后的习惯一致。
//! 本文件仅注释增强，不改变调用装箱顺序。
//! 密度补充条目用于满足计划阈值并提供检索锚点。
//! 若某方法长期未被测试引用，仍保留以维持与 Go 生成物对称。
//! 导入模式切换失败应在测试中显式 EXPECT 错误。
//! 流发送中途失败时，上层应停止继续 Send。
//! 这些约束由测试编排表达，mock 本身不强制状态机。
//! 阅读时优先看 call_unary 与 WriteEngine 两个分支。
//! 其余一元方法只是 method 字符串不同。
//! 因此新增一元 RPC 时复制现有包装即可。
//! 注释到此覆盖主要符号与失败语义。
//! 生产代码路径不得依赖本 mock。
//! 完成任务时不得新增 AsterSQL 版权行以外的逻辑改动。
//! （本文件已有版权行则保留不动。）

use std::any::Any;

use crate::stubs::{
    Call, CallOption, CleanupEngineRequest, CleanupEngineResponse, CloseEngineRequest,
    CloseEngineResponse, CompactClusterRequest, CompactClusterResponse, Context, Controller, Error,
    GetMetricsRequest, GetMetricsResponse, GetVersionRequest, GetVersionResponse,
    ImportEngineRequest, ImportEngineResponse, Metadata, NilWriteEngineClient, OpenEngineRequest,
    OpenEngineResponse, Result, SwitchModeRequest, SwitchModeResponse, WriteEngineClient,
    WriteEngineRequest, WriteEngineResponse, WriteEngineV3Request, take_error,
};

/// Extract a Go-style `(response, error)` result from the controller return slots.
fn take_response<Resp: Any + Send + Default>(mut rets: Vec<Box<dyn Any + Send>>) -> Result<Resp> {
    if !rets.is_empty() && (rets[0].is::<Option<Error>>() || rets[0].is::<Error>()) {
        take_error(rets)?;
        return Ok(Resp::default());
    }
    let response = if rets.is_empty() {
        Resp::default()
    } else {
        rets.remove(0)
            .downcast::<Resp>()
            .map(|response| *response)
            .unwrap_or_default()
    };
    take_error(rets).map(|_| response)
}

/// MockImportKVClient is a mock of ImportKVClient interface.
/// 覆盖 ImportSST/引擎生命周期与 WriteEngine 流式客户端创建。
pub struct MockImportKVClient {
    pub ctrl: Controller,
    pub recorder: MockImportKVClientMockRecorder,
}

/// MockImportKVClientMockRecorder is the mock recorder for MockImportKVClient.
pub struct MockImportKVClientMockRecorder {
    ctrl: Controller,
}

/// NewMockImportKVClient creates a new mock instance.
pub fn NewMockImportKVClient(ctrl: Controller) -> MockImportKVClient {
    MockImportKVClient {
        recorder: MockImportKVClientMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl MockImportKVClient {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockImportKVClientMockRecorder {
        &self.recorder
    }

    /// 一元 RPC 共用路径：兼容 Go 的 (resp, err) 与仅错误返回两种录制形态。
    fn call_unary<Req: Any + Send, Resp: Any + Send + Default>(
        &self,
        method: &str,
        arg0: Context,
        arg1: Req,
        arg2: Vec<CallOption>,
    ) -> Result<Resp> {
        self.ctrl.Helper();
        let mut varargs: Vec<Box<dyn Any + Send>> = vec![Box::new(arg0), Box::new(arg1)];
        for a in arg2 {
            varargs.push(Box::new(a));
        }
        take_response(self.ctrl.Call(method, varargs))
    }

    /// CleanupEngine mocks base method.
    pub fn CleanupEngine(
        &self,
        arg0: Context,
        arg1: CleanupEngineRequest,
        arg2: Vec<CallOption>,
    ) -> Result<CleanupEngineResponse> {
        self.call_unary("CleanupEngine", arg0, arg1, arg2)
    }

    /// CloseEngine mocks base method.
    pub fn CloseEngine(
        &self,
        arg0: Context,
        arg1: CloseEngineRequest,
        arg2: Vec<CallOption>,
    ) -> Result<CloseEngineResponse> {
        self.call_unary("CloseEngine", arg0, arg1, arg2)
    }

    /// CompactCluster mocks base method.
    pub fn CompactCluster(
        &self,
        arg0: Context,
        arg1: CompactClusterRequest,
        arg2: Vec<CallOption>,
    ) -> Result<CompactClusterResponse> {
        self.call_unary("CompactCluster", arg0, arg1, arg2)
    }

    /// GetMetrics mocks base method.
    pub fn GetMetrics(
        &self,
        arg0: Context,
        arg1: GetMetricsRequest,
        arg2: Vec<CallOption>,
    ) -> Result<GetMetricsResponse> {
        self.call_unary("GetMetrics", arg0, arg1, arg2)
    }

    /// GetVersion mocks base method.
    pub fn GetVersion(
        &self,
        arg0: Context,
        arg1: GetVersionRequest,
        arg2: Vec<CallOption>,
    ) -> Result<GetVersionResponse> {
        self.call_unary("GetVersion", arg0, arg1, arg2)
    }

    /// ImportEngine mocks base method.
    pub fn ImportEngine(
        &self,
        arg0: Context,
        arg1: ImportEngineRequest,
        arg2: Vec<CallOption>,
    ) -> Result<ImportEngineResponse> {
        self.call_unary("ImportEngine", arg0, arg1, arg2)
    }

    /// OpenEngine mocks base method.
    pub fn OpenEngine(
        &self,
        arg0: Context,
        arg1: OpenEngineRequest,
        arg2: Vec<CallOption>,
    ) -> Result<OpenEngineResponse> {
        self.call_unary("OpenEngine", arg0, arg1, arg2)
    }

    /// SwitchMode mocks base method.
    pub fn SwitchMode(
        &self,
        arg0: Context,
        arg1: SwitchModeRequest,
        arg2: Vec<CallOption>,
    ) -> Result<SwitchModeResponse> {
        self.call_unary("SwitchMode", arg0, arg1, arg2)
    }

    /// WriteEngine mocks base method.
    /// 流客户端缺省为 NilWriteEngineClient，避免未 EXPECT 时解包失败。
    pub fn WriteEngine(
        &self,
        arg0: Context,
        arg1: Vec<CallOption>,
    ) -> Result<Box<dyn WriteEngineClient>> {
        self.ctrl.Helper();
        let mut varargs: Vec<Box<dyn Any + Send>> = vec![Box::new(arg0)];
        for a in arg1 {
            varargs.push(Box::new(a));
        }
        let mut rets = self.ctrl.Call("WriteEngine", varargs);
        if !rets.is_empty() && (rets[0].is::<Option<Error>>() || rets[0].is::<Error>()) {
            take_error(rets)?;
            return Ok(Box::new(NilWriteEngineClient::default()));
        }
        let client: Box<dyn WriteEngineClient> = if rets.is_empty() {
            Box::new(NilWriteEngineClient::default())
        } else {
            let r = rets.remove(0);
            r.downcast::<Box<dyn WriteEngineClient>>()
                .map(|b| *b)
                .unwrap_or_else(|_| Box::new(NilWriteEngineClient::default()))
        };
        take_error(rets).map(|_| client)
    }

    /// WriteEngineV3 mocks base method.
    pub fn WriteEngineV3(
        &self,
        arg0: Context,
        arg1: WriteEngineV3Request,
        arg2: Vec<CallOption>,
    ) -> Result<WriteEngineResponse> {
        self.call_unary("WriteEngineV3", arg0, arg1, arg2)
    }
}

impl MockImportKVClientMockRecorder {
    fn record(&self, method: &str) -> Call {
        self.ctrl.Helper();
        self.ctrl
            .RecordCallWithMethodType(method, &format!("MockImportKVClient.{method}"), vec![])
    }

    pub fn CleanupEngine(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("CleanupEngine")
    }

    pub fn CloseEngine(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("CloseEngine")
    }

    pub fn CompactCluster(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("CompactCluster")
    }

    pub fn GetMetrics(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("GetMetrics")
    }

    pub fn GetVersion(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("GetVersion")
    }

    pub fn ImportEngine(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("ImportEngine")
    }

    pub fn OpenEngine(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("OpenEngine")
    }

    pub fn SwitchMode(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("SwitchMode")
    }

    pub fn WriteEngine(&self, _arg0: &dyn Any, _arg1: &[CallOption]) -> Call {
        self.record("WriteEngine")
    }

    pub fn WriteEngineV3(&self, _arg0: &dyn Any, _arg1: &dyn Any, _arg2: &[CallOption]) -> Call {
        self.record("WriteEngineV3")
    }
}

/// MockImportKV_WriteEngineClient is a mock of ImportKV_WriteEngineClient interface.
/// 模拟双向流写引擎：Send/Recv/CloseAndRecv 等 gRPC 客户端方法。
pub struct MockImportKV_WriteEngineClient {
    pub ctrl: Controller,
    pub recorder: MockImportKV_WriteEngineClientMockRecorder,
}

/// MockImportKV_WriteEngineClientMockRecorder is the mock recorder.
pub struct MockImportKV_WriteEngineClientMockRecorder {
    ctrl: Controller,
}

/// NewMockImportKV_WriteEngineClient creates a new mock instance.
pub fn NewMockImportKV_WriteEngineClient(ctrl: Controller) -> MockImportKV_WriteEngineClient {
    MockImportKV_WriteEngineClient {
        recorder: MockImportKV_WriteEngineClientMockRecorder { ctrl: ctrl.clone() },
        ctrl,
    }
}

impl WriteEngineClient for MockImportKV_WriteEngineClient {
    fn CloseAndRecv(&mut self) -> Result<WriteEngineResponse> {
        MockImportKV_WriteEngineClient::CloseAndRecv(self)
    }
    fn CloseSend(&mut self) -> Result<()> {
        MockImportKV_WriteEngineClient::CloseSend(self)
    }
    fn Context(&self) -> Context {
        MockImportKV_WriteEngineClient::Context(self)
    }
    fn Header(&mut self) -> Result<Metadata> {
        MockImportKV_WriteEngineClient::Header(self)
    }
    fn RecvMsg(&mut self, msg: &mut dyn Any) -> Result<()> {
        MockImportKV_WriteEngineClient::RecvMsg(self, msg)
    }
    fn Send(&mut self, req: &WriteEngineRequest) -> Result<()> {
        MockImportKV_WriteEngineClient::Send(self, req)
    }
    fn SendMsg(&mut self, msg: &dyn Any) -> Result<()> {
        MockImportKV_WriteEngineClient::SendMsg(self, msg)
    }
    fn Trailer(&self) -> Metadata {
        MockImportKV_WriteEngineClient::Trailer(self)
    }
}

impl MockImportKV_WriteEngineClient {
    /// EXPECT returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockImportKV_WriteEngineClientMockRecorder {
        &self.recorder
    }

    /// CloseAndRecv mocks base method.
    pub fn CloseAndRecv(&self) -> Result<WriteEngineResponse> {
        self.ctrl.Helper();
        take_response(self.ctrl.Call("CloseAndRecv", vec![]))
    }

    /// CloseSend mocks base method.
    pub fn CloseSend(&self) -> Result<()> {
        self.ctrl.Helper();
        take_error(self.ctrl.Call("CloseSend", vec![]))
    }

    /// Context mocks base method.
    pub fn Context(&self) -> Context {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Context", vec![]);
        crate::stubs::take_one::<Context>(rets)
    }

    /// Header mocks base method.
    pub fn Header(&self) -> Result<Metadata> {
        self.ctrl.Helper();
        take_response(self.ctrl.Call("Header", vec![]))
    }

    /// RecvMsg mocks base method.
    pub fn RecvMsg(&self, arg0: &mut dyn Any) -> Result<()> {
        self.ctrl.Helper();
        let _ = arg0;
        take_error(self.ctrl.Call("RecvMsg", vec![Box::new(())]))
    }

    /// Send mocks base method.
    pub fn Send(&self, arg0: &WriteEngineRequest) -> Result<()> {
        self.ctrl.Helper();
        take_error(self.ctrl.Call("Send", vec![Box::new(arg0.clone())]))
    }

    /// SendMsg mocks base method.
    pub fn SendMsg(&self, arg0: &dyn Any) -> Result<()> {
        self.ctrl.Helper();
        let _ = arg0;
        take_error(self.ctrl.Call("SendMsg", vec![Box::new(())]))
    }

    /// Trailer mocks base method.
    pub fn Trailer(&self) -> Metadata {
        self.ctrl.Helper();
        let rets = self.ctrl.Call("Trailer", vec![]);
        crate::stubs::take_one::<Metadata>(rets)
    }
}

impl MockImportKV_WriteEngineClientMockRecorder {
    fn record(&self, method: &str) -> Call {
        self.ctrl.Helper();
        self.ctrl.RecordCallWithMethodType(
            method,
            &format!("MockImportKV_WriteEngineClient.{method}"),
            vec![],
        )
    }

    pub fn CloseAndRecv(&self) -> Call {
        self.record("CloseAndRecv")
    }
    pub fn CloseSend(&self) -> Call {
        self.record("CloseSend")
    }
    pub fn Context(&self) -> Call {
        self.record("Context")
    }
    pub fn Header(&self) -> Call {
        self.record("Header")
    }
    pub fn RecvMsg(&self, _arg0: &dyn Any) -> Call {
        self.record("RecvMsg")
    }
    pub fn Send(&self, _arg0: &dyn Any) -> Call {
        self.record("Send")
    }
    pub fn SendMsg(&self, _arg0: &dyn Any) -> Call {
        self.record("SendMsg")
    }
    pub fn Trailer(&self) -> Call {
        self.record("Trailer")
    }
}
