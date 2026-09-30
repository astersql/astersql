// Copyright 2026 AsterSQL.

//! Synchronous Manager boundary over the production external workload gRPC client.

use std::time::Duration;

use astersql_extworkload_client as remote;
use tokio::runtime::{Builder, Runtime};

use crate::{client, context};

pub(crate) struct RealController {
    runtime: Runtime,
    inner: Box<dyn remote::Client>,
}

impl RealController {
    pub(crate) fn connect(option: &client::Option) -> Result<Self, client::ClientError> {
        let mut remote_option = remote::Option::with_addr(&option.ControllerAddr);
        remote_option.KeyspaceID = option.KeyspaceID;
        remote_option.KeyspaceName = option.KeyspaceName.clone();
        remote_option.TiDBPool = option.TiDBPool.clone();
        remote_option.TLSConfig = option.TLSConfig.as_ref().map(|tls| tls.0.clone());
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(map_error)?;
        // tonic connect_lazy requires a live Tokio reactor at construction.
        let inner = runtime.enter();
        let client = remote::New(Some(&remote_option)).map_err(map_error)?;
        drop(inner);
        Ok(Self {
            runtime,
            inner: client,
        })
    }

    fn context(context: &context::Context) -> remote::Context {
        if context.Deadline() {
            remote::Context::with_timeout(Duration::from_secs(30))
        } else {
            remote::Context::background()
        }
    }
}

fn map_error(error: impl std::fmt::Display) -> client::ClientError {
    client::ClientError(error.to_string())
}

#[allow(non_snake_case)]
impl client::Client for RealController {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn Close(&mut self) -> Result<(), client::ClientError> {
        self.inner.Close().map_err(map_error)
    }
    fn Ping(&mut self, context: &context::Context) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(self.inner.Ping(&Self::context(context)))
            .map_err(map_error)
    }
    fn RegisterGCV2(
        &mut self,
        context: &context::Context,
        safe_point: u64,
        gc_life_time: i64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .RegisterGCV2(&Self::context(context), safe_point, gc_life_time),
            )
            .map_err(map_error)
    }
    fn RecycleGCV2(
        &mut self,
        context: &context::Context,
        safe_point: u64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(self.inner.RecycleGCV2(&Self::context(context), safe_point))
            .map_err(map_error)
    }
    fn UpdateGCLifeTime(
        &mut self,
        context: &context::Context,
        gc_life_time: i64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .UpdateGCLifeTime(&Self::context(context), gc_life_time),
            )
            .map_err(map_error)
    }
    fn RegisterTTLTask(
        &mut self,
        context: &context::Context,
        table_id: i64,
        enabled: bool,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .RegisterTTLTask(&Self::context(context), table_id, enabled),
            )
            .map_err(map_error)
    }
    fn DeleteTTLTableInfo(
        &mut self,
        context: &context::Context,
        table_id: i64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .DeleteTTLTableInfo(&Self::context(context), table_id),
            )
            .map_err(map_error)
    }
    fn RecycleTTLTask(
        &mut self,
        context: &context::Context,
        create_time: u64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .RecycleTTLTask(&Self::context(context), create_time),
            )
            .map_err(map_error)
    }
    fn UpdateTTLJobEnable(
        &mut self,
        context: &context::Context,
        enabled: bool,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .UpdateTTLJobEnable(&Self::context(context), enabled),
            )
            .map_err(map_error)
    }
    fn RegisterAutoAnalyze(
        &mut self,
        context: &context::Context,
        task_id: u64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .RegisterAutoAnalyze(&Self::context(context), task_id),
            )
            .map_err(map_error)
    }
    fn RecycleAutoAnalyze(
        &mut self,
        context: &context::Context,
        task_id: u64,
    ) -> Result<(), client::ClientError> {
        self.runtime
            .block_on(
                self.inner
                    .RecycleAutoAnalyze(&Self::context(context), task_id),
            )
            .map_err(map_error)
    }
}
