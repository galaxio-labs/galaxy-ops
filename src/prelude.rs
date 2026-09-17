pub use getset::{Getters, MutGetters, Setters, WithSetters};
pub use log::{debug, error, info, warn};
pub use orion_error::UnifiedReason as UvsReason;
pub use orion_error::reason::{DomainReason, ErrorCode};
pub use orion_error::runtime::WithContext;
pub use orion_error::{OperationContext, StructError};
pub use orion_variate::addr::AddrReason;

pub use derive_more::{Deref, DerefMut, Display, From};
pub use serde_derive::{Deserialize, Serialize};
pub use thiserror::Error;

pub use std::fmt::Formatter;
pub use std::fs;
pub use std::path::Path;
pub use std::path::PathBuf;
pub use std::str::FromStr;
pub use std::sync::Arc;

pub use crate::error::MainResult;
pub use async_trait::async_trait;
pub use contracts::requires;
pub use orion_conf::error::{OrionConfResult as SerdeResult, SerdeReason};
pub use serde::ser::Serializer;

// 添加高频重复的orion生态导入
#[allow(deprecated)]
pub use crate::compat::{
    Configable, JsonAble, Persistable, StorageLoadEvent, ValueConfable, YamlStorageExt, Yamlable,
};
pub use crate::compat::{
    ContextRecord, ErrorConv, ErrorOwe, ErrorOweBase, ErrorWith, OperationContextCompat,
    StructErrorTrait, UvsFrom,
};
pub use orion_error::conversion::ToStructError;
pub use orion_infra::auto_exit_log;
pub use orion_infra::path::{PathResult, ensure_path, make_clean_path};
pub use orion_variate::update::DownloadOptions;

pub use orion_variate::addr::accessor::UniversalAccessor;
// 常用类型和trait
pub use orion_variate::addr::Address;
pub use orion_variate::types::ResourceDownloader;
pub use orion_vars::vars::{
    EnvDict, EnvEvalable, OriginDict, ValueDict, VarCollection, VarDefinition,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::setting::Setting;
    use orion_error::dev::testing::TestAssert;
    use tempfile::tempdir;

    #[allow(deprecated)]
    #[test]
    fn test_legacy_prelude_still_supports_legacy_io_methods() {
        let temp_dir = tempdir().unwrap();
        let conf_path = temp_dir.path().join("setting.conf");
        let yaml_path = temp_dir.path().join("setting.yml");

        let setting = Setting::example();
        setting.save_conf(&conf_path).assert();

        let loaded = Setting::from_conf(&conf_path).assert();
        loaded.save_yml(&yaml_path).assert();
    }
}
