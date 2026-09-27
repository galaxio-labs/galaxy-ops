pub mod depend;
pub mod init;
pub mod model;
pub mod operator;
mod prelude;
pub mod refs;
pub mod spec;

use crate::internal_prelude::*;

#[derive(Clone, Debug, Serialize, Deserialize, Display, PartialEq, Eq, Hash)]
pub enum CpuArch {
    #[display("x86")]
    X86,
    #[display("arm")]
    Arm,
}

impl FromStr for CpuArch {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x86" => Ok(Self::X86),
            "arm" => Ok(Self::Arm),
            _ => Err(s.to_string()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Display, PartialEq, Eq, Hash)]
pub enum OsCPE {
    #[display("mac14")]
    MAC14,
    #[display("win10")]
    WIN10,
    #[display("ubt22")]
    UBT22,
    #[display("cos7")]
    COS7,
}

impl FromStr for OsCPE {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mac14" => Ok(Self::MAC14),
            "win10" => Ok(Self::WIN10),
            "ubt22" => Ok(Self::UBT22),
            "cos7" => Ok(Self::COS7),
            _ => Err(s.to_string()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Display, PartialEq, Eq, Hash)]
pub enum RunSPC {
    #[display("host")]
    Host,
    #[display("k8s")]
    K8S,
}
impl FromStr for RunSPC {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "host" => Ok(Self::Host),
            "k8s" => Ok(Self::K8S),
            _ => Err(s.to_string()),
        }
    }
}

#[derive(Clone, Debug, From, PartialEq, Eq, Hash)]
pub struct ModelSTD {
    arch: CpuArch,
    os: OsCPE,
    spc: RunSPC,
}
impl ModelSTD {
    pub fn arm_mac14_host() -> Self {
        Self {
            arch: CpuArch::Arm,
            os: OsCPE::MAC14,
            spc: RunSPC::Host,
        }
    }
    pub fn x86_ubt22_host() -> Self {
        Self {
            arch: CpuArch::X86,
            os: OsCPE::UBT22,
            spc: RunSPC::Host,
        }
    }
    pub fn x86_ubt22_k8s() -> Self {
        Self {
            arch: CpuArch::X86,
            os: OsCPE::UBT22,
            spc: RunSPC::K8S,
        }
    }
    pub fn support() -> Vec<Self> {
        vec![
            Self::arm_mac14_host(),
            Self::x86_ubt22_host(),
            Self::x86_ubt22_k8s(),
        ]
    }
    /// 是否运行在 k8s（用于 `gops mod new` 生成 Helm chart 脚手架等 k8s 专属内容）。
    pub fn is_k8s(&self) -> bool {
        self.spc == RunSPC::K8S
    }
    pub fn from_cur_sys() -> Self {
        let info = os_info::get();

        // 根据CPU架构字符串确定CpuArch
        let arch_str = info
            .architecture()
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "x86".to_string());
        let arch = match arch_str.as_str() {
            "x86" | "x86_64" | "amd64" => CpuArch::X86,
            "arm" | "aarch64" | "arm64" => CpuArch::Arm,
            _ => CpuArch::X86, // 默认使用X86
        };

        // 根据操作系统确定OsCPE
        // TODO: need version;
        let os = match info.os_type() {
            os_info::Type::Macos => OsCPE::MAC14,
            os_info::Type::Windows => OsCPE::WIN10,
            os_info::Type::Ubuntu => OsCPE::UBT22,
            os_info::Type::CentOS | os_info::Type::Redhat => OsCPE::COS7,
            _ => OsCPE::UBT22, // 默认使用Ubuntu 22.04
        };

        // 默认使用Host运行空间
        let spc = RunSPC::Host;

        Self { arch, os, spc }
    }
}

// 紧凑的序列化实现
impl serde::Serialize for ModelSTD {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 格式化为 "arch-os-spc" 字符串
        let s = format!("{}-{}-{}", self.arch, self.os, self.spc);
        serializer.serialize_str(&s)
    }
}

// 对应的反序列化实现
impl<'de> serde::Deserialize<'de> for ModelSTD {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl ModelSTD {
    pub fn new(arch: CpuArch, os: OsCPE, spc: RunSPC) -> Self {
        Self { arch, os, spc }
    }
}

impl std::fmt::Display for ModelSTD {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", self.arch, self.os, self.spc)
    }
}
impl FromStr for ModelSTD {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 3 {
            return Err(value.into());
        }

        let arch = CpuArch::from_str(parts[0])?;
        let os = OsCPE::from_str(parts[1])?;
        let spc = RunSPC::from_str(parts[2])?;

        Ok(ModelSTD { arch, os, spc })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_k8s_only_for_k8s_runspace() {
        assert!(ModelSTD::x86_ubt22_k8s().is_k8s());
        assert!(!ModelSTD::x86_ubt22_host().is_k8s());
        assert!(!ModelSTD::arm_mac14_host().is_k8s());
        assert!(ModelSTD::new(CpuArch::Arm, OsCPE::UBT22, RunSPC::K8S).is_k8s());
    }
}
