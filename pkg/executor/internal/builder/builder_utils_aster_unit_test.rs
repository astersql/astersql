// Builder 工具的单元测试。
//
// 通过可记录调用的物理计划与最小会话上下文，验证 TiKV/TiFlash 执行器的构建形态、
// 错误传播、非自然顺序的父子下标，以及 DAG 请求携带的会话元数据。

use std::sync::{Arc, Mutex};

use crate::{
    BuildPBContext, BuilderError, ConstructDAGReq, ConstructDAGReqForUnNatureOrderPlans,
    ConstructListBasedDistExec, ConstructListBasedDistExecForUnNatureOrderPlans,
    ConstructTreeBasedDistExec, DAGRequest, EncodeType, Executor, PhysicalPlan, SessionContext,
    SessionVars, StoreType,
};

// 固定构建失败的计划，用于验证错误路径仍保留必要的会话侧副作用。
struct ErrorPlan;

impl PhysicalPlan for ErrorPlan {
    fn ToPB(
        &self,
        _context: &BuildPBContext,
        _store_type: StoreType,
    ) -> Result<Executor, BuilderError> {
        Err(BuilderError("plan failed".to_owned()))
    }
}

// 记录目标存储类型，并可按测试需要返回载荷或错误。
struct RecordingPlan {
    payload: &'static [u8],
    calls: Arc<Mutex<Vec<StoreType>>>,
    error: Option<&'static str>,
}

impl PhysicalPlan for RecordingPlan {
    fn ToPB(
        &self,
        _context: &BuildPBContext,
        store_type: StoreType,
    ) -> Result<Executor, BuilderError> {
        self.calls.lock().unwrap().push(store_type);
        match self.error {
            Some(message) => Err(BuilderError(message.to_owned())),
            None => Ok(Executor {
                Payload: self.payload.to_vec(),
                ..Executor::default()
            }),
        }
    }
}

// 最小会话上下文；额外计数编码类型设置次数，以便观察调用时序。
struct Context {
    vars: SessionVars,
    build_pb_context: BuildPBContext,
    encode_type_calls: Arc<Mutex<usize>>,
}

impl SessionContext for Context {
    fn GetSessionVars(&self) -> &SessionVars {
        &self.vars
    }

    fn GetBuildPBCtx(&self) -> &BuildPBContext {
        &self.build_pb_context
    }

    fn SetEncodeType(&self, request: &mut DAGRequest) {
        *self.encode_type_calls.lock().unwrap() += 1;
        request.EncodeType = EncodeType::TypeChunk;
    }
}

#[test]
fn construct_dag_request_sets_encode_type_when_plan_build_fails() {
    let encode_type_calls = Arc::new(Mutex::new(0));
    let context = Context {
        vars: SessionVars::default(),
        build_pb_context: BuildPBContext,
        encode_type_calls: Arc::clone(&encode_type_calls),
    };
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![Box::new(ErrorPlan)];

    let result = ConstructDAGReq(&context, &plans, StoreType::TiKV);

    assert_eq!(result.unwrap_err().to_string(), "plan failed");
    assert_eq!(*encode_type_calls.lock().unwrap(), 1);
}

#[test]
fn construct_list_builds_in_order_and_uses_tikv() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let context = BuildPBContext;
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![
        Box::new(RecordingPlan {
            payload: b"first",
            calls: Arc::clone(&calls),
            error: None,
        }),
        Box::new(RecordingPlan {
            payload: b"second",
            calls: Arc::clone(&calls),
            error: None,
        }),
    ];

    let executors = ConstructListBasedDistExec(&context, &plans).unwrap();

    assert_eq!(executors.len(), 2);
    assert_eq!(executors[0].Payload, b"first");
    assert_eq!(executors[1].Payload, b"second");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![StoreType::TiKV, StoreType::TiKV]
    );
}

#[test]
fn construct_tree_uses_tiflash_and_returns_one_executor() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let context = BuildPBContext;
    let plan = RecordingPlan {
        payload: b"root",
        calls: Arc::clone(&calls),
        error: None,
    };

    let executors = ConstructTreeBasedDistExec(&context, &plan).unwrap();

    assert_eq!(executors.len(), 1);
    assert_eq!(executors[0].Payload, b"root");
    assert_eq!(*calls.lock().unwrap(), vec![StoreType::TiFlash]);
}

#[test]
fn list_error_stops_before_later_plans() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let context = BuildPBContext;
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![
        Box::new(RecordingPlan {
            payload: b"ok",
            calls: Arc::clone(&calls),
            error: None,
        }),
        Box::new(RecordingPlan {
            payload: b"error",
            calls: Arc::clone(&calls),
            error: Some("second failed"),
        }),
        Box::new(RecordingPlan {
            payload: b"not-called",
            calls: Arc::clone(&calls),
            error: None,
        }),
    ];

    // 列表构建遇错即返回，第三个计划不应再被转换。
    let error = ConstructListBasedDistExec(&context, &plans).unwrap_err();

    assert_eq!(error.to_string(), "second failed");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![StoreType::TiKV, StoreType::TiKV]
    );
}

#[test]
fn unnatural_order_sets_child_parent_indices() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let context = BuildPBContext;
    let plans: Vec<Box<dyn PhysicalPlan>> = (0..3)
        .map(|index| {
            Box::new(RecordingPlan {
                payload: b"",
                calls: Arc::clone(&calls),
                error: None,
            }) as Box<dyn PhysicalPlan>
        })
        .collect();
    // 映射方向是 child→parent；未出现在映射中的执行器保持无父节点。
    let order = [(0usize, 2usize), (2usize, 0usize)].into_iter().collect();

    let executors =
        ConstructListBasedDistExecForUnNatureOrderPlans(&context, &plans, &order).unwrap();

    assert_eq!(executors[0].ParentIdx, Some(2));
    assert_eq!(executors[1].ParentIdx, None);
    assert_eq!(executors[2].ParentIdx, Some(0));
}

#[test]
fn construct_dag_request_populates_metadata_and_sets_encode_type() {
    let encode_type_calls = Arc::new(Mutex::new(0));
    let context = Context {
        vars: SessionVars {
            TimeZoneName: "Asia/Shanghai".to_owned(),
            TimeZoneOffset: 28_800,
            RuntimeStatsEnabled: true,
            PushDownFlags: 0x55,
            DivPrecisionIncrement: 6,
        },
        build_pb_context: BuildPBContext,
        encode_type_calls: Arc::clone(&encode_type_calls),
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![Box::new(RecordingPlan {
        payload: b"root",
        calls,
        error: None,
    })];

    // TiFlash 请求使用树形根执行器，同时应完整复制会话侧下推元数据。
    let request = ConstructDAGReq(&context, &plans, StoreType::TiFlash).unwrap();

    assert_eq!(request.TimeZoneName, "Asia/Shanghai");
    assert_eq!(request.TimeZoneOffset, 28_800);
    assert_eq!(request.CollectExecutionSummaries, Some(true));
    assert_eq!(request.Flags, 0x55);
    assert_eq!(request.DivPrecisionIncrement, Some(6));
    assert_eq!(request.RootExecutor.unwrap().Payload, b"root");
    assert!(request.Executors.is_empty());
    assert_eq!(request.EncodeType, EncodeType::TypeChunk);
    assert_eq!(*encode_type_calls.lock().unwrap(), 1);
}

#[test]
fn construct_tikv_dag_keeps_default_optional_metadata_absent() {
    let encode_type_calls = Arc::new(Mutex::new(0));
    let context = Context {
        vars: SessionVars::default(),
        build_pb_context: BuildPBContext,
        encode_type_calls: Arc::clone(&encode_type_calls),
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![Box::new(RecordingPlan {
        payload: b"leaf",
        calls: Arc::clone(&calls),
        error: None,
    })];

    let request = ConstructDAGReq(&context, &plans, StoreType::TiKV).unwrap();

    assert_eq!(request.TimeZoneName, "UTC");
    assert_eq!(request.TimeZoneOffset, 0);
    assert_eq!(request.CollectExecutionSummaries, None);
    assert_eq!(request.Flags, 0);
    assert_eq!(request.DivPrecisionIncrement, None);
    assert_eq!(request.Executors.len(), 1);
    assert_eq!(request.Executors[0].Payload, b"leaf");
    assert_eq!(request.RootExecutor, None);
    assert_eq!(request.EncodeType, EncodeType::TypeChunk);
    assert_eq!(*calls.lock().unwrap(), vec![StoreType::TiKV]);
    assert_eq!(*encode_type_calls.lock().unwrap(), 1);
}

#[test]
fn construct_dag_for_unnatural_order_sets_parent_indices_after_build() {
    let encode_type_calls = Arc::new(Mutex::new(0));
    let context = Context {
        vars: SessionVars::default(),
        build_pb_context: BuildPBContext,
        encode_type_calls: Arc::clone(&encode_type_calls),
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let plans: Vec<Box<dyn PhysicalPlan>> = (0..3)
        .map(|_| {
            Box::new(RecordingPlan {
                payload: b"node",
                calls: Arc::clone(&calls),
                error: None,
            }) as Box<dyn PhysicalPlan>
        })
        .collect();
    let order = [(0usize, 2usize), (2usize, 0usize)].into_iter().collect();

    let request =
        ConstructDAGReqForUnNatureOrderPlans(&context, &plans, &order, StoreType::TiKV).unwrap();

    assert_eq!(request.Executors[0].ParentIdx, Some(2));
    assert_eq!(request.Executors[1].ParentIdx, None);
    assert_eq!(request.Executors[2].ParentIdx, Some(0));
    assert_eq!(request.RootExecutor, None);
    assert_eq!(*calls.lock().unwrap(), vec![StoreType::TiKV; 3]);
    assert_eq!(*encode_type_calls.lock().unwrap(), 1);
}
