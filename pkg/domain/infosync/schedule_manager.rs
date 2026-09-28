// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// PD 调度（schedule）配置管理器。
//
// 抽象对 PD（Placement Driver，集群调度中枢）调度配置的读写：
// - `PDScheduleManager`：通过 `PdHttpClient` 访问真实 PD；
// - `mockScheduleManager`：内存 HashMap 实现，供单测使用。

use crate::{ConfigValue, PdHttpClient, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 调度配置管理接口：获取 / 设置 PD schedule config。
pub trait ScheduleManager: Send + Sync {
    /// 读取当前全部调度配置项（键值对，值为异构 `ConfigValue`）。
    fn GetScheduleConfig(&self) -> Result<HashMap<String, ConfigValue>>;
    /// 写入（合并）调度配置项到 PD 或本地存储。
    fn SetScheduleConfig(&self, config: &HashMap<String, ConfigValue>) -> Result<()>;
}

/// 基于 PD HTTP 客户端的真实调度配置管理器。
pub struct PDScheduleManager {
    /// 底层 PD HTTP 客户端。
    pub Client: Arc<dyn PdHttpClient>,
}
impl ScheduleManager for PDScheduleManager {
    fn GetScheduleConfig(&self) -> Result<HashMap<String, ConfigValue>> {
        self.Client.get_schedule_config()
    }
    fn SetScheduleConfig(&self, config: &HashMap<String, ConfigValue>) -> Result<()> {
        self.Client.set_schedule_config(config)
    }
}

/// 内存 Mock：用 RwLock 保护的 HashMap 模拟调度配置。
#[derive(Default)]
pub struct mockScheduleManager {
    /// 本地调度配置存储。
    schedules: RwLock<HashMap<String, ConfigValue>>,
}
impl ScheduleManager for mockScheduleManager {
    fn GetScheduleConfig(&self) -> Result<HashMap<String, ConfigValue>> {
        Ok(self.schedules.read().unwrap().clone())
    }
    /// 合并写入：对新键插入、已有键覆盖。
    fn SetScheduleConfig(&self, config: &HashMap<String, ConfigValue>) -> Result<()> {
        self.schedules.write().unwrap().extend(config.clone());
        Ok(())
    }
}
