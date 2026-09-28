// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 资源管理客户端的内存 Mock 实现。
//
// 提供与真实 Resource Manager（资源组管理服务）相同的 `ResourceManagerClient`
// 接口：资源组（Resource Group）的增删改查，以及基于 key 前缀的 watch 订阅。
// 用于单元测试与本地开发，避免依赖外部 PD / Resource Manager 服务。

use std::collections::HashMap;
use std::sync::{Mutex, mpsc};

use crate::{
    Error, EventType, ResourceGroup, ResourceGroupEvent, ResourceManagerClient, Result,
    group_settings_path_prefix,
};

/// 默认资源组名称；新建 Mock 客户端时会自动插入该组。
pub const DefaultResourceGroupName: &str = "default";

/// 内存版 Resource Manager 客户端：用 HashMap 保存资源组，并用 channel 推送变更事件。
pub struct mockResourceManagerClient {
    /// Keyspace（键空间）ID，用于过滤 watch 前缀。
    keyspaceID: u32,
    /// 资源组名 → 资源组定义。
    groups: Mutex<HashMap<String, ResourceGroup>>,
    /// 已注册的 watch 订阅者；发送失败的订阅者会被清理。
    watchers: Mutex<Vec<mpsc::Sender<Vec<ResourceGroupEvent>>>>,
}

/// 创建 Mock 客户端，并预置一个无限配额（FillRate=MAX、BurstLimit=-1）的 default 资源组。
pub fn NewMockResourceManagerClient(keyspaceID: u32) -> Box<dyn ResourceManagerClient> {
    let default_group = ResourceGroup {
        Name: DefaultResourceGroupName.into(),
        RUSettings: crate::TokenLimitSettings {
            FillRate: i32::MAX as i64,
            BurstLimit: -1,
        },
        Priority: 8,
    };
    Box::new(mockResourceManagerClient {
        keyspaceID,
        groups: Mutex::new(HashMap::from([(default_group.Name.clone(), default_group)])),
        watchers: Mutex::new(Vec::new()),
    })
}

impl mockResourceManagerClient {
    /// 向所有仍存活的 watcher 广播资源组变更事件；发送失败则从列表移除。
    fn publish(&self, event_type: EventType, group: ResourceGroup) {
        let event = vec![ResourceGroupEvent { event_type, group }];
        self.watchers
            .lock()
            .unwrap()
            .retain(|watcher| watcher.send(event.clone()).is_ok());
    }
}

impl ResourceManagerClient for mockResourceManagerClient {
    /// 列出当前全部资源组（无序）。
    fn list_resource_groups(&self) -> Vec<ResourceGroup> {
        self.groups.lock().unwrap().values().cloned().collect()
    }
    /// 按名称查询资源组；不存在则返回 External 错误。
    fn get_resource_group(&self, name: &str) -> Result<ResourceGroup> {
        self.groups
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::External(format!("the group {name} does not exist")))
    }
    /// 新增资源组；同名已存在则报错，成功后发布 Put 事件。
    fn add_resource_group(&self, group: ResourceGroup) -> Result<String> {
        let mut groups = self.groups.lock().unwrap();
        if groups.contains_key(&group.Name) {
            return Err(Error::External(format!(
                "the group {} already exists",
                group.Name
            )));
        }
        groups.insert(group.Name.clone(), group.clone());
        drop(groups);
        self.publish(EventType::Put, group);
        Ok("Success!".into())
    }
    /// 覆盖写入资源组（存在则更新、不存在则插入），并发布 Put 事件。
    fn modify_resource_group(&self, group: ResourceGroup) -> Result<String> {
        self.groups
            .lock()
            .unwrap()
            .insert(group.Name.clone(), group.clone());
        self.publish(EventType::Put, group);
        Ok("Success!".into())
    }
    /// 删除资源组；不存在则报错，成功后发布 Delete 事件。
    fn delete_resource_group(&self, name: &str) -> Result<String> {
        let group = self
            .groups
            .lock()
            .unwrap()
            .remove(name)
            .ok_or_else(|| Error::External(format!("the group {name} does not exist")))?;
        self.publish(EventType::Delete, group);
        Ok("Success!".into())
    }
    /// 仅当 `key` 匹配本 keyspace 的资源组设置前缀时注册 watcher，否则返回 None。
    fn watch(&self, key: &[u8]) -> Option<mpsc::Receiver<Vec<ResourceGroupEvent>>> {
        if key != group_settings_path_prefix(self.keyspaceID) {
            return None;
        }
        let (sender, receiver) = mpsc::channel();
        self.watchers.lock().unwrap().push(sender);
        Some(receiver)
    }
}
