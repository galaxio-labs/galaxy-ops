#![allow(deprecated)]

use std::fmt::Display;
use std::path::Path;

pub use orion_conf::FilePersist as Persistable;
pub use orion_conf::LoadHook as StorageLoadEvent;
pub use orion_conf::error::{OrionConfResult as SerdeResult, SerdeReason};
use orion_error::conversion::{ConvErr, ToStructError};
use orion_error::reason::{DomainReason, ErrorCode};
use orion_error::{OperationContext, StructError};

// Public compatibility facade for downstream crates.
// Internal code should prefer the native orion_conf 0.5 traits directly.

pub trait ErrorOwe<T>: Sized {
    fn source_resource<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn source_sys<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn source_data<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn source_conf<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn source_biz<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn source_logic<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>;
    fn owe<R>(self, reason: R) -> Result<T, StructError<R>>
    where
        R: DomainReason;
}

impl<T, E> ErrorOwe<T> for Result<T, E>
where
    E: Display,
{
    fn source_resource<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::resource_error())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn source_sys<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::system_error())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn source_data<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::data_error())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn source_conf<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::core_conf())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn source_biz<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::business_error())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn source_logic<R>(self) -> Result<T, StructError<R>>
    where
        R: DomainReason + From<orion_error::UnifiedReason>,
    {
        self.map_err(|e| {
            R::from(orion_error::UnifiedReason::logic_error())
                .to_err()
                .with_detail(e.to_string())
        })
    }

    fn owe<R>(self, reason: R) -> Result<T, StructError<R>>
    where
        R: DomainReason,
    {
        self.map_err(|e| reason.to_err().with_detail(e.to_string()))
    }
}

pub trait ErrorOweBase {}
impl<T> ErrorOweBase for T {}

pub trait ErrorConv<T, R: DomainReason>: Sized {
    fn err_conv(self) -> Result<T, StructError<R>>;
}

impl<T, R1, R2> ErrorConv<T, R2> for Result<T, StructError<R1>>
where
    R1: DomainReason,
    R2: DomainReason + From<R1>,
{
    fn err_conv(self) -> Result<T, StructError<R2>> {
        self.conv_err()
    }
}

pub trait ErrorWith: Sized {
    fn with<C: Into<OperationContext>>(self, ctx: C) -> Self;
    fn want<S: Into<String>>(self, desc: S) -> Self;
    fn position<S: Into<String>>(self, pos: S) -> Self;
    fn with_context<C: Into<OperationContext>>(self, ctx: C) -> Self;

    fn doing<S: Into<String>>(self, desc: S) -> Self {
        self.with_context(OperationContext::doing(desc))
    }

    fn at<C: Into<OperationContext>>(self, ctx: C) -> Self {
        self.with_context(ctx)
    }
}

impl<R: DomainReason> ErrorWith for StructError<R> {
    fn with<C: Into<OperationContext>>(self, ctx: C) -> Self {
        self.with_context(ctx)
    }

    fn want<S: Into<String>>(self, desc: S) -> Self {
        self.with_context(OperationContext::doing(desc))
    }

    fn position<S: Into<String>>(self, pos: S) -> Self {
        self.with_position(pos)
    }

    fn with_context<C: Into<OperationContext>>(self, ctx: C) -> Self {
        StructError::with_context(self, ctx)
    }
}

impl<T, E: ErrorWith> ErrorWith for Result<T, E> {
    fn with<C: Into<OperationContext>>(self, ctx: C) -> Self {
        self.map_err(|e| e.with(ctx))
    }

    fn want<S: Into<String>>(self, desc: S) -> Self {
        self.map_err(|e| e.want(desc))
    }

    fn position<S: Into<String>>(self, pos: S) -> Self {
        self.map_err(|e| e.position(pos))
    }

    fn with_context<C: Into<OperationContext>>(self, ctx: C) -> Self {
        self.map_err(|e| e.with_context(ctx))
    }
}

pub trait OperationContextCompat {
    fn want<S: Into<String>>(desc: S) -> Self;
}

impl OperationContextCompat for OperationContext {
    fn want<S: Into<String>>(desc: S) -> Self {
        OperationContext::doing(desc)
    }
}

pub trait StructErrorTrait<R: DomainReason> {
    fn get_reason(&self) -> &R;
    fn target(&self) -> Option<String>;
    fn context(&self) -> &[OperationContext];
    fn error_code(&self) -> i32
    where
        R: ErrorCode;
}

impl<R: DomainReason> StructErrorTrait<R> for StructError<R> {
    fn get_reason(&self) -> &R {
        self.reason()
    }

    fn target(&self) -> Option<String> {
        self.target_path()
    }

    fn context(&self) -> &[OperationContext] {
        self.contexts()
    }

    fn error_code(&self) -> i32
    where
        R: ErrorCode,
    {
        self.reason().error_code()
    }
}

pub trait ContextRecord<K, V> {
    fn record(&mut self, key: K, value: V);
}

impl<K, V> ContextRecord<K, V> for OperationContext
where
    K: Into<String>,
    V: Display,
{
    fn record(&mut self, key: K, value: V) {
        self.record_field(key, value);
    }
}

pub trait UvsFrom {}
impl<T> UvsFrom for T {}

#[deprecated(note = "use orion_conf::ConfigIO::load_conf/save_conf instead")]
pub trait Configable
where
    Self: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::ConfigIO<Self>,
{
    fn from_conf(path: &Path) -> SerdeResult<Self> {
        <Self as orion_conf::ConfigIO<Self>>::load_conf(path)
    }

    fn save_conf(&self, path: &Path) -> SerdeResult<()> {
        <Self as orion_conf::ConfigIO<Self>>::save_conf(self, path)
    }
}

impl<T> Configable for T where
    T: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::ConfigIO<T>
{
}

#[deprecated(note = "use orion_conf::JsonIO::load_json/save_json instead")]
pub trait JsonAble
where
    Self: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::JsonIO<Self>,
{
    fn from_json(path: &Path) -> SerdeResult<Self> {
        <Self as orion_conf::JsonIO<Self>>::load_json(path)
    }

    fn save_json(&self, path: &Path) -> SerdeResult<()> {
        <Self as orion_conf::JsonIO<Self>>::save_json(self, path)
    }
}

impl<T> JsonAble for T where
    T: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::JsonIO<T>
{
}

#[deprecated(note = "use orion_conf::YamlIO::load_yaml/save_yaml instead")]
pub trait Yamlable
where
    Self: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::YamlIO<Self>,
{
    fn from_yml(path: &Path) -> SerdeResult<Self> {
        <Self as orion_conf::YamlIO<Self>>::load_yaml(path)
    }

    fn save_yml(&self, path: &Path) -> SerdeResult<()> {
        <Self as orion_conf::YamlIO<Self>>::save_yaml(self, path)
    }
}

impl<T> Yamlable for T where
    T: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::YamlIO<T>
{
}

#[deprecated(note = "use orion_conf::TextConfigIO::load_valconf/save_valconf instead")]
pub trait ValueConfable
where
    Self: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::TextConfigIO<Self>,
{
    fn from_valconf(path: &Path) -> SerdeResult<Self> {
        <Self as orion_conf::TextConfigIO<Self>>::load_valconf(path)
    }

    fn save_valconf(&self, path: &Path) -> SerdeResult<()> {
        <Self as orion_conf::TextConfigIO<Self>>::save_valconf(self, path)
    }
}

impl<T> ValueConfable for T where
    T: serde::de::DeserializeOwned + serde::Serialize + Sized + orion_conf::TextConfigIO<T>
{
}

#[deprecated(note = "use orion_conf::YamlIO directly")]
pub trait YamlStorageExt: Yamlable {}

impl<T> YamlStorageExt for T where T: Yamlable {}

#[cfg(test)]
mod tests {
    use super::{Configable, Yamlable};
    use crate::system::setting::Setting;
    use orion_error::dev::testing::TestAssert;
    use tempfile::tempdir;

    #[allow(deprecated)]
    #[test]
    fn test_legacy_compat_traits_still_work() {
        let temp_dir = tempdir().unwrap();
        let conf_path = temp_dir.path().join("setting.yml");

        let setting = Setting::example();
        setting.save_conf(&conf_path).assert();
        let loaded = Setting::from_conf(&conf_path).assert();

        loaded.save_yml(&conf_path).assert();
    }
}
