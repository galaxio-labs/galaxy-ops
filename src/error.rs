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

/// 面向人的错误说明：**优先 `detail`**（通常才是可执行的那句话），没有才退回 reason。
///
/// `StructError` 的 `Display` 是“reason + `-> Info: <detail>` + 可能的 Trace”多行输出；
/// 单行 WARN / 报告条目里直接 `{err}` 会打断版面、标题也没有信息量（`logic_detail(...)`
/// 的 reason 是 `internal logic error`）。这里只取 detail 这一行，保证旁路提示可读。
pub fn user_message(err: &MainError) -> String {
    err.detail()
        .as_deref()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| err.reason().to_string())
}

pub const PATH_NOT_EXIST: &str = "path not exists";

pub fn report_error(e: StructError<MainReason>) {
    eprintln!("Run Error (Code: {})", e.reason().error_code());
    eprintln!("--------------------------");
    if let Some(target) = e.target_path() {
        eprintln!("[TARGET]:\n{target}\n",);
    }
    // 诊断渲染交给 orion-error 0.9（`DiagnosticReport::render`）：
    // 输出 reason / detail / position / context，替代原先手写的 reason match。
    eprintln!("{}", e.render());
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

    #[test]
    fn test_user_message_prefers_detail_over_reason() {
        // `logic_detail` 的 reason 文本是 `internal logic error`：旁路 WARN 打它等于没说。
        let err = MainReason::logic_detail("`docker compose config` 失败：env_file 不存在");
        assert_eq!(
            user_message(&err),
            "`docker compose config` 失败：env_file 不存在"
        );
    }

    #[test]
    fn test_user_message_falls_back_to_reason_without_detail() {
        let err: MainError = StructError::new(MainReason::Sys(SysReason::Load), None, None, vec![]);
        assert_eq!(user_message(&err), "sys load fail");
    }
}
