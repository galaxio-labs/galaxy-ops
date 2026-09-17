use derive_more::From;
use orion_error::StructError;
use orion_error::UnifiedReason as UvsReason;
use orion_error::conversion::ToStructError;
use orion_error::reason::{DomainReason, ErrorCode};
use orion_variate::addr::AddrReason;
use serde_derive::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, PartialEq, Error, From)]
pub enum MainReason {
    #[error("unknow")]
    UnKnow,
    #[error("localize:{0}")]
    Localize(LocalizeReason),
    #[error("element:{0}")]
    Element(ElementReason),
    #[error("mod {0}")]
    Mod(ModReason),
    #[error("sys {0}")]
    Sys(SysReason),
    #[error("sys {0}")]
    Ops(OpsReason),
    #[error("accessor {0}")]
    Accessor(AddrReason),
    #[error("{0}")]
    Uvs(UvsReason),
}

impl DomainReason for MainReason {}

impl From<orion_conf::error::ConfIOReason> for MainReason {
    fn from(reason: orion_conf::error::ConfIOReason) -> Self {
        match reason {
            orion_conf::error::ConfIOReason::General(reason) => Self::Uvs(reason),
            _ => Self::Uvs(UvsReason::core_conf()),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Error)]
pub enum ElementReason {
    #[error("miss:{0}")]
    Miss(String),
}
#[derive(Clone, Debug, Serialize, PartialEq, Error)]
pub enum ModReason {
    #[error("miss:{0}")]
    Miss(String),
    #[error("load fail")]
    Load,
    #[error("save fail")]
    Save,
    #[error("update fail")]
    Update,
    #[error("localize fail")]
    Localize,
}
#[derive(Clone, Debug, Serialize, PartialEq, Error)]
pub enum SysReason {
    #[error("miss:{0}")]
    Miss(String),
    #[error("load fail")]
    Load,
    #[error("save fail")]
    Save,
    #[error("update fail")]
    Update,
    #[error("localize fail")]
    Localize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Error)]
pub enum OpsReason {
    #[error("miss:{0}")]
    Miss(String),
    #[error("load fail")]
    Load,
    #[error("save fail")]
    Save,
    #[error("update fail")]
    Update,
    #[error("localize fail")]
    Localize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Error)]
pub enum LocalizeReason {
    #[error("miss:{0}")]
    Templatize(String),
}
impl ErrorCode for ElementReason {
    fn error_code(&self) -> i32 {
        match self {
            ElementReason::Miss(_) => 531,
        }
    }
}

impl ErrorCode for LocalizeReason {
    fn error_code(&self) -> i32 {
        match self {
            LocalizeReason::Templatize(_) => 541,
        }
    }
}
impl ErrorCode for ModReason {
    fn error_code(&self) -> i32 {
        match self {
            Self::Miss(_) => 551,
            ModReason::Load => 552,
            ModReason::Save => 553,
            ModReason::Update => 554,
            ModReason::Localize => 555,
        }
    }
}
impl ErrorCode for SysReason {
    fn error_code(&self) -> i32 {
        match self {
            SysReason::Miss(_) => 561,
            SysReason::Load => 562,
            SysReason::Save => 563,
            SysReason::Update => 564,
            SysReason::Localize => 565,
        }
    }
}

impl ErrorCode for OpsReason {
    fn error_code(&self) -> i32 {
        match self {
            OpsReason::Miss(_) => 571,
            OpsReason::Load => 572,
            OpsReason::Save => 573,
            OpsReason::Update => 574,
            OpsReason::Localize => 575,
        }
    }
}

impl ErrorCode for MainReason {
    fn error_code(&self) -> i32 {
        match self {
            MainReason::UnKnow => 500,
            MainReason::Accessor(r) => r.error_code(),
            MainReason::Uvs(r) => r.error_code(),
            MainReason::Localize(r) => r.error_code(),
            MainReason::Element(r) => r.error_code(),
            MainReason::Mod(r) => r.error_code(),
            MainReason::Sys(r) => r.error_code(),
            MainReason::Ops(r) => r.error_code(),
        }
    }
}

impl MainReason {
    pub fn conf_detail(msg: impl Into<String>) -> MainError {
        Self::from(UvsReason::core_conf())
            .to_err()
            .with_detail(msg.into())
    }

    pub fn logic_detail(msg: impl Into<String>) -> MainError {
        Self::from(UvsReason::logic_error())
            .to_err()
            .with_detail(msg.into())
    }

    pub fn resource_detail(msg: impl Into<String>) -> MainError {
        Self::from(UvsReason::resource_error())
            .to_err()
            .with_detail(msg.into())
    }

    pub fn from_addr_error(error: StructError<AddrReason>) -> MainError {
        StructError::new(
            Self::Accessor(error.reason().clone()),
            error.detail().clone(),
            error.position().clone(),
            error.contexts().to_vec(),
        )
    }
}
pub type MainResult<T> = Result<T, StructError<MainReason>>;
pub type MainError = StructError<MainReason>;

pub const PATH_NOT_EXIST: &str = "path not exists";

pub fn report_error(e: StructError<MainReason>) {
    println!("Run Error (Code: {})", e.reason().error_code());
    println!("--------------------------");
    if let Some(target) = e.target_path() {
        println!("[TARGET]:\n{target}\n",);
    }
    println!("[REASON]:");
    match e.reason() {
        MainReason::Accessor(addr_reason) => match addr_reason {
            AddrReason::Brief(msg) => {
                println!("ACCESSOR ERROR: {msg}\n");
            }
            AddrReason::Unified(uvs_reason) => {
                println!("ACCESSOR ERROR: {uvs_reason}\n");
            }
            AddrReason::OperationTimeoutExceeded { timeout, attempts } => {
                println!("ACCESSOR TIMEOUT: timeout={timeout:?}, attempts={attempts}\n");
            }
            AddrReason::TotalTimeoutExceeded {
                total_timeout,
                elapsed,
            } => {
                println!(
                    "ACCESSOR TIMEOUT: total_timeout={total_timeout:?}, elapsed={elapsed:?}\n"
                );
            }
            AddrReason::RetryExhausted {
                attempts,
                last_error,
            } => {
                println!(
                    "ACCESSOR RETRY EXHAUSTED: attempts={attempts}, last_error={last_error}\n"
                );
            }
        },
        MainReason::Uvs(uvs_reason) => match uvs_reason {
            UvsReason::LogicError => {
                println!("LOGIC ERROR\n");
            }
            UvsReason::BusinessError => {
                println!("BIZ ERROR\n");
            }
            UvsReason::DataError => {
                println!("DATA ERROR\n");
            }
            UvsReason::SystemError => {
                println!("SYS ERROR\n");
            }
            UvsReason::ResourceError => {
                println!("RES ERROR\n");
            }
            UvsReason::ConfigError(e) => {
                println!("CONF ERROR: {e}\n");
            }
            UvsReason::ValidationError => {
                println!("VALIDATION ERROR\n");
            }
            UvsReason::NotFoundError => {
                println!("NOT FOUND ERROR\n");
            }
            UvsReason::PermissionError => {
                println!("PERMISSION ERROR\n");
            }
            UvsReason::NetworkError => {
                println!("NETWORK ERROR\n");
            }
            UvsReason::TimeoutError => {
                println!("TIMEOUT ERROR\n");
            }
            UvsReason::ExternalError => {
                println!("EXTERNAL ERROR\n");
            }
            UvsReason::RunRuleError => {
                println!("RUN RULE ERROR\n");
            }
        },

        MainReason::Localize(e) => {
            println!("Localize ERROR: {e}\n",);
        }
        MainReason::Element(e) => {
            println!("Element ERROR: {e}\n",);
        }
        MainReason::UnKnow => {
            println!("Unknow Error!\n");
        }
        MainReason::Mod(e) => {
            println!("Mod Error: \n{e} !");
        }
        MainReason::Sys(e) => {
            println!("Sys Error: \n{e}");
        }
        MainReason::Ops(e) => {
            println!("Operator Error: \n{e}");
        }
    }
    if let Some(detail) = e.detail() {
        println!("\n[DETAIL]:\n{detail}",);
    }
    println!("\n[CONTEXT]:\n");
    for x in e.contexts().iter() {
        println!("{x}",)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_from_addr_error_keeps_accessor_reason_and_detail() {
        let source = StructError::new(
            AddrReason::RetryExhausted {
                attempts: 3,
                last_error: "connection reset".into(),
            },
            Some("download failed".into()),
            Some("src/error.rs:1:1".into()),
            vec![],
        );

        let converted = MainReason::from_addr_error(source);

        assert_eq!(converted.reason().error_code(), 504);
        assert_eq!(converted.detail().as_deref(), Some("download failed"));
        assert!(matches!(
            converted.reason(),
            MainReason::Accessor(AddrReason::RetryExhausted {
                attempts: 3,
                last_error,
            }) if last_error == "connection reset"
        ));
    }

    #[test]
    fn test_main_reason_from_addr_reason_preserves_variant() {
        let reason = MainReason::from(AddrReason::OperationTimeoutExceeded {
            timeout: Duration::from_secs(5),
            attempts: 2,
        });

        assert!(matches!(
            reason,
            MainReason::Accessor(AddrReason::OperationTimeoutExceeded { attempts: 2, .. })
        ));
    }
}
