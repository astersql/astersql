// Copyright 2026 AsterSQL.

use crate::{ImportStorageFactory, SharedStorage};
use astersql_objstore::local::NewLocalStorage;
use astersql_objstore_storeapi::Context;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Opens server-disk IMPORT INTO files in their parent directory, as Go does.
pub struct ServerDiskImportStorageFactory;

/// Prefer the production local backend for absolute server paths; delegate cloud URIs to host.
pub struct HostImportStorageFactory {
    pub CloudFactory: Arc<dyn ImportStorageFactory>,
}

impl ImportStorageFactory for HostImportStorageFactory {
    fn Open(&self, context: &Context, uri: &str, target: &str) -> Result<SharedStorage, String> {
        if Path::new(uri).is_absolute() && target == "IMPORT INTO data source" {
            ServerDiskImportStorageFactory.Open(context, uri, target)
        } else {
            self.CloudFactory.Open(context, uri, target)
        }
    }
}

impl ImportStorageFactory for ServerDiskImportStorageFactory {
    fn Open(&self, context: &Context, uri: &str, target: &str) -> Result<SharedStorage, String> {
        context.check().map_err(|error| error.to_string())?;
        let path = Path::new(uri);
        if !path.is_absolute() || target != "IMPORT INTO data source" {
            return Err(format!("unsupported {target} storage URI: {uri}"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| format!("server-disk path has no parent: {uri}"))?;
        if !parent.is_dir() {
            return Err(format!(
                "server-disk directory does not exist: {}",
                parent.display()
            ));
        }
        let storage = NewLocalStorage(parent).map_err(|error| error.to_string())?;
        Ok(Arc::new(Mutex::new(Box::new(storage))))
    }
}
