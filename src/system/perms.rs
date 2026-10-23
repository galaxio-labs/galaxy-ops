//! `gops sys perms`：从 docker-compose 派生 bind 挂载的宿主属主/权限，
//! 提供自检（`--check`，默认）与幂等修复（`--apply`）。
//!
//! 为什么由 gops 派生，而不是每个栈自己维护名单：`docker compose config --format json`
//! 不需要拉镜像就给出了完整契约 —— 服务的 `user:`、宿主真实 `source`、`read_only`。
//! 名单写在栈里会与 compose 实际挂载漂移；compose 不会。
//!
//! 规则（与 issue #29 对齐）：
//! - 写需求（`read_only != true`）：文件 `0660`，目录 `2770`（setgid）；
//! - 读需求（`read_only == true`）：只补属组读位，文件 `0640`，目录 `2750`；
//! - 属主 = 部署账号（`SUDO_UID` / euid），属组 = 容器 gid；
//! - **属组是唯一的共享通道**（属主不 chown 给容器 uid，那需要提权），所以 `chgrp` 到
//!   容器 gid 要求部署账号**已是该组成员**（内核按 gid 数字比对自己的凭据）。
//!   缺这一步时**先一次性报出**、不做逐项失败：它是宿主预置，`--apply` 不能修（也不提权）。
//! - `secrets.file`：属主仍是部署账号，模式由 `secrets.mode`（`0600` / `0640`）决定；
//!   `0640` 时容器按属组读——**不通过 chown 到容器 uid 共享**（那需要提权）；
//! - 以 root 运行的服务跳过（root 不受宿主模式限制，强行对齐只会收紧模式）；
//! - **容器自己创建的**（owner == 容器 uid）不动 —— 否则每次 localize 都要 sudo；
//! - 只处理系统根内的 bind `source`；根外需 `sys-prj.yml: perms.allow_external` 显式声明；
//! - 不跟随符号链接；不修改挂载根的父目录；不碰 named volume。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{MainReason, MainResult};
use crate::system::SysConf;
use crate::system::conf::PermsConf;
use crate::system::path::SysOperatorPath;
use orion_conf::ConfigIO;

/// 容器可读写的宿主文件模式。
pub const RW_FILE: u32 = 0o660;
/// 容器可读写的宿主目录模式（setgid，保证容器新建文件继承容器组）。
pub const RW_DIR: u32 = 0o2770;
/// 容器只读的宿主文件模式。
pub const RO_FILE: u32 = 0o640;
/// 容器只读的宿主目录模式。
pub const RO_DIR: u32 = 0o2750;

/// 密钥文件的期望目标：路径 + `secrets.mode`（`0600` / `0640`）。
///
/// 属主**始终是部署账号**，不通过 chown 到容器 uid 来共享（那需要提权）。
#[derive(Clone, Copy, Debug)]
struct SecretTarget<'a> {
    path: &'a Path,
    mode: u32,
}

/// 容器身份（仅支持数字 uid/gid；解析不出时要求 `perms.container_uid/gid`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerId {
    pub uid: u32,
    pub gid: u32,
}

/// 一条从 compose 派生出的 bind 挂载规则。
#[derive(Clone, Debug)]
pub struct BindMount {
    pub service: String,
    pub source: PathBuf,
    pub read_only: bool,
    pub container: ContainerId,
}

/// 单项检查/修复结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    /// 已对齐（含“容器自建、故意不动”）。
    Aligned,
    /// 仅检查：待修复。
    Pending,
    /// `--apply`：本次已修复。
    Applied,
    /// 修复失败，或修复后仍不一致（ACL / 只读挂载等）。
    Failed,
}

impl ItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemStatus::Aligned => "aligned",
            ItemStatus::Pending => "pending",
            ItemStatus::Applied => "applied",
            ItemStatus::Failed => "failed",
        }
    }
}

/// 文件/目录的宿主快照。
#[derive(Clone, Copy, Debug)]
pub struct FileSnapshot {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

/// 单条路径的报告。
#[derive(Clone, Debug)]
pub struct ItemReport {
    pub path: PathBuf,
    pub kind: &'static str,
    pub read_only: bool,
    pub current: Option<FileSnapshot>,
    pub expected_mode: u32,
    pub expected_gid: u32,
    /// 是否是 `secrets.file`（决定缺失时不能“创建”，以及 `secrets.mode` 的期望值）。
    pub is_secret: bool,
    pub status: ItemStatus,
    pub reason: String,
}

/// 单个服务的 bind 挂载报告。
#[derive(Clone, Debug)]
pub struct MountReport {
    pub service: String,
    pub source: PathBuf,
    pub read_only: bool,
    pub container: ContainerId,
    pub items: Vec<ItemReport>,
    /// 部署账号不在该 gid 组里（宿主预置缺失）：明细不在文本输出里展开，
    /// 改由一条汇总 `errors` 说清 —— N 个文件报同一个 `EPERM` 只是噪声。
    pub blocked_by_group: Option<u32>,
}

/// `sys perms` 的整体报告。
#[derive(Clone, Debug)]
pub struct PermsReport {
    pub system_root: PathBuf,
    pub skipped: Option<String>,
    pub deploy_uid: u32,
    pub deploy_gid: u32,
    pub mounts: Vec<MountReport>,
    pub warnings: Vec<String>,
    /// 不需要对齐、但值得告知的跳过原因（例如容器以 root 运行）。
    /// 与 `warnings` 区分：`notes` 不影响退出码，`localize` 也不会打到 stderr。
    pub notes: Vec<String>,
    /// 无法靠改权限解决、`--apply` 也不会「修」的阻断项：
    /// 配置错误（同一路径两种容器身份、0600 密钥配非 root 容器）与**宿主预置**
    /// （部署账号不在容器 gid 组里，chgrp 必然 EPERM）。非空即判定失败。
    pub errors: Vec<String>,
}

impl PermsReport {
    /// 平台不支持 / 非 compose 系统时的跳过结果（退出码 0）。
    pub fn skipped(system_root: &Path, reason: impl Into<String>) -> Self {
        let (deploy_uid, deploy_gid) = deploy_identity();
        Self {
            system_root: system_root.to_path_buf(),
            skipped: Some(reason.into()),
            deploy_uid,
            deploy_gid,
            mounts: Vec::new(),
            warnings: Vec::new(),
            notes: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn count(&self, status: ItemStatus) -> usize {
        self.mounts
            .iter()
            .flat_map(|m| m.items.iter())
            .filter(|i| i.status == status)
            .count()
    }
    pub fn pending(&self) -> usize {
        self.count(ItemStatus::Pending)
    }
    pub fn applied(&self) -> usize {
        self.count(ItemStatus::Applied)
    }
    pub fn failed(&self) -> usize {
        self.count(ItemStatus::Failed)
    }
    pub fn is_ok(&self) -> bool {
        self.skipped.is_some()
            || (self.pending() == 0 && self.failed() == 0 && self.errors.is_empty())
    }
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.is_ok())
    }

    /// 机器可读输出（stdout 单行）。
    pub fn to_json(&self) -> String {
        let mounts: Vec<serde_json::Value> = self
            .mounts
            .iter()
            .map(|m| {
                let items: Vec<serde_json::Value> = m
                    .items
                    .iter()
                    .map(|i| {
                        let current = i.current.map(|s| {
                            serde_json::json!({
                                "uid": s.uid,
                                "gid": s.gid,
                                "mode": format_mode(s.mode),
                            })
                        });
                        serde_json::json!({
                            "path": i.path.display().to_string(),
                            "kind": i.kind,
                            "current": current,
                            "expected": {
                                "uid": self.deploy_uid,
                                "gid": i.expected_gid,
                                "mode": format_mode(i.expected_mode),
                            },
                            "status": i.status.as_str(),
                            "reason": i.reason,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "service": m.service,
                    "source": m.source.display().to_string(),
                    "read_only": m.read_only,
                    "container": { "uid": m.container.uid, "gid": m.container.gid },
                    "blocked_by_group": m.blocked_by_group,
                    "items": items,
                })
            })
            .collect();
        serde_json::json!({
            "system_root": self.system_root.display().to_string(),
            "skipped": self.skipped,
            "deploy": { "uid": self.deploy_uid, "gid": self.deploy_gid },
            "mounts": mounts,
            "warnings": self.warnings,
            "notes": self.notes,
            "errors": self.errors,
            "summary": {
                "pending": self.pending(),
                "applied": self.applied(),
                "failed": self.failed(),
            },
        })
        .to_string()
    }

    /// 人读输出：对齐的挂载只给一行汇总，未对齐的逐项列出。
    pub fn render_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        if let Some(reason) = &self.skipped {
            let _ = writeln!(out, "[SKIP] sys perms：{reason}");
            return out;
        }
        let _ = writeln!(
            out,
            "sys perms: {}  deploy={}:{}",
            self.system_root.display(),
            self.deploy_uid,
            self.deploy_gid
        );
        if self.mounts.is_empty() {
            let _ = writeln!(out, "[OK] 没有 bind 挂载需要处理");
        }
        for mount in &self.mounts {
            let bad: Vec<&ItemReport> = mount
                .items
                .iter()
                .filter(|i| !matches!(i.status, ItemStatus::Aligned))
                .collect();
            let tag = if mount.blocked_by_group.is_some() {
                "[BLOCKED]"
            } else if bad.is_empty() {
                "[OK]"
            } else if bad.iter().any(|i| i.status == ItemStatus::Failed) {
                "[FAIL]"
            } else if bad.iter().any(|i| i.status == ItemStatus::Applied) {
                "[FIXED]"
            } else {
                "[FAIL]"
            };
            let _ = writeln!(
                out,
                "{tag} {}  {}  {}  {}:{}  ({} 项)",
                mount.service,
                mount.source.display(),
                if mount.read_only { "ro" } else { "rw" },
                mount.container.uid,
                mount.container.gid,
                mount.items.len()
            );
            // 宿主组这一层缺失时，逐项明细全是同一个 `EPERM`：收成一行，修复办法看汇总的 [FAIL]。
            if let Some(gid) = mount.blocked_by_group {
                let _ = writeln!(
                    out,
                    "       部署账号不在 gid {gid} 组里，{} 项明细已省略（修复办法见下方 [FAIL]）",
                    bad.len()
                );
                continue;
            }
            for item in bad {
                let current = item
                    .current
                    .map(|s| format!("{} {}:{}", format_mode(s.mode), s.uid, s.gid))
                    .unwrap_or_else(|| "缺失".to_string());
                let _ = writeln!(
                    out,
                    "       {} {}  {current} → {} {}:{}  [{}] {}",
                    if item.kind == "dir" { "d" } else { "f" },
                    display_path(&self.system_root, &item.path),
                    format_mode(item.expected_mode),
                    self.deploy_uid,
                    item.expected_gid,
                    item.status.as_str(),
                    item.reason
                );
            }
        }
        for note in &self.notes {
            let _ = writeln!(out, "[INFO] {note}");
        }
        for warning in &self.warnings {
            let _ = writeln!(out, "[WARN] {warning}");
        }
        for error in &self.errors {
            let _ = writeln!(out, "[FAIL] {error}");
        }
        if self.is_ok() {
            if self.applied() > 0 {
                let _ = writeln!(out, "结论: 已修复 {} 项，全部对齐", self.applied());
            } else {
                let _ = writeln!(out, "结论: 全部对齐");
            }
        } else if !self.errors.is_empty() {
            let _ = writeln!(
                out,
                "结论: {} 项无法靠改权限解决（配置或宿主预置），按上面的 [FAIL] 处理后重跑",
                self.errors.len()
            );
        } else if self.failed() > 0 {
            let _ = writeln!(
                out,
                "结论: {} 项待修 / {} 项失败（ACL、只读挂载或权限不足？先看上面的原因）",
                self.pending(),
                self.failed()
            );
        } else {
            let _ = writeln!(
                out,
                "结论: {} 项待修，运行 `gops sys perms --apply`",
                self.pending()
            );
        }
        out
    }
}

/// 只检查（不修改磁盘）。
pub fn check(system_root: &Path, conf: Option<&PermsConf>) -> MainResult<PermsReport> {
    run(system_root, conf, None, false)
}

/// 幂等修复（只动 bind 挂载源，容器自建文件不动）。
pub fn apply(system_root: &Path, conf: Option<&PermsConf>) -> MainResult<PermsReport> {
    run(system_root, conf, None, true)
}

/// 读取系统根下的 `sys-prj.yml`（缺失时返回 `None`，不因缺文件报错）。
///
/// **只读**：不做 v1→v2 迁移（`prj diagnose` 会走这里，诊断不应改现场）。
/// 旧名 `sys_prj.yml` 仍回退读取。
pub fn load_conf(system_root: &Path) -> MainResult<Option<SysConf>> {
    let paths = SysOperatorPath::new(system_root);
    let path = if paths.conf_file_v2().is_file() {
        paths.conf_file_v2()
    } else if paths.conf_file_v1().is_file() {
        paths.conf_file_v1()
    } else {
        return Ok(None);
    };
    Ok(Some(SysConf::load_conf(&path).map_err(|e| {
        MainReason::conf_detail(format!("读取 {} 失败：{e}", path.display()))
    })?))
}

/// `gops sys perms` 的库入口：按 `sys-prj.yml: perms` 的覆盖项检查/修复。
pub fn run_with_conf(system_root: &Path, do_apply: bool) -> MainResult<PermsReport> {
    let conf = load_conf(system_root)?;
    let perms = conf.as_ref().and_then(|c| c.perms());
    let secret = resolve_secret(system_root, conf.as_ref())?;
    run(
        system_root,
        perms,
        secret
            .as_ref()
            .map(|(path, mode)| SecretTarget { path, mode: *mode }),
        do_apply,
    )
}

/// 解析 `sys-prj.yml: secrets.file` 的宿主路径与期望模式（`0600` / `0640`）。
///
/// 只做**路径校验**（复用 `system::sec` 的规则）：相对系统根、不含 `.`/`..`、
/// 不含符号链接、解析后仍在根内。perms 不读内容，但绝不能因为清单里写了
/// 绝对路径 / `..` 就把 chmod 作用到系统根之外。
///
/// 文件缺失是**允许**的：`apply` 会明确报“perms 不创建密钥”，而不是猜一个空文件。
fn resolve_secret(
    system_root: &Path,
    conf: Option<&SysConf>,
) -> MainResult<Option<(PathBuf, u32)>> {
    let Some(secrets) = conf.and_then(|c| c.secrets()) else {
        return Ok(None);
    };
    let declared = secrets.file().trim();
    if declared.is_empty() {
        return Ok(None);
    }
    let mode = secrets.file_mode().map_err(MainReason::logic_detail)?;
    let path = crate::system::sec::resolve_declared_path(system_root, declared)?;
    Ok(Some((path, mode)))
}

// === 派生 ===

#[derive(Debug, Deserialize)]
struct ComposeConfig {
    #[serde(default)]
    services: BTreeMap<String, ComposeService>,
}

#[derive(Debug, Default, Deserialize)]
struct ComposeService {
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    image: Option<String>,
    /// compose-go 对空列表用 `omitempty`，但不同版本/上游也可能给 `null`：
    /// 用 `Option` 接住，避免整个派生因一个 `volumes: null` 失败。
    #[serde(default)]
    volumes: Option<Vec<ComposeVolume>>,
}

#[derive(Debug, Default, Deserialize)]
struct ComposeVolume {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    read_only: bool,
}

#[derive(Debug)]
struct Derived {
    mounts: Vec<BindMount>,
    warnings: Vec<String>,
    notes: Vec<String>,
    errors: Vec<String>,
}

fn derive(
    system_root: &Path,
    conf: Option<&PermsConf>,
    secret: Option<SecretTarget<'_>>,
) -> MainResult<Derived> {
    let (compose_file, global_args) = crate::system::compose::compose_invocation(system_root);
    let Some(compose_file) = compose_file else {
        return Err(MainReason::logic_detail(
            "没有找到 compose 文件（已按 sys/ 与系统根的 {compose,docker-compose}.{yaml,yml} 查找）；\
             `gops sys perms` 只适用于 docker-compose 系统",
        ));
    };

    // rootless / userns-remap 的检测要问 docker，放在**真实派生入口**，
    // 不污染纯 JSON 解析（单测因此不依赖宿主 docker 配置）。
    if container_override(conf)?.is_none()
        && let Some(reason) = user_namespace_reason()
    {
        return Err(MainReason::logic_detail(format!(
            "检测到 {reason}：compose 里的 `user:` 是容器内身份，不是宿主 uid/gid，不能直接用来 chown。\n\
             请在 `sys-prj.yml` 里显式声明宿主映射（同时给 uid 与 gid）：\n\
             \n  perms:\n    container_uid: 100999\n    container_gid: 100999\n"
        )));
    }

    let json = compose_config_json(system_root, &compose_file, &global_args)?;
    derive_from_json_with_secret(system_root, &json, conf, secret)
}

/// `sys-prj.yml: perms.container_uid/gid` 的显式覆盖；两者必须同时给。
fn container_override(conf: Option<&PermsConf>) -> MainResult<Option<ContainerId>> {
    match conf {
        Some(p) => match (p.container_uid(), p.container_gid()) {
            (Some(uid), Some(gid)) => Ok(Some(ContainerId { uid, gid })),
            (None, None) => Ok(None),
            _ => Err(MainReason::logic_detail(
                "`sys-prj.yml: perms.container_uid / container_gid` 必须同时配置（只给一个无法 chown）",
            )),
        },
        None => Ok(None),
    }
}

/// 从 `docker compose config --format json` 的文本派生（不碰 docker，便于单测）。
fn derive_from_json_with_secret(
    system_root: &Path,
    json: &str,
    conf: Option<&PermsConf>,
    secret: Option<SecretTarget<'_>>,
) -> MainResult<Derived> {
    let config: ComposeConfig = serde_json::from_str(json).map_err(|e| {
        MainReason::logic_detail(format!("解析 `docker compose config` 输出失败：{e}"))
    })?;

    let overrides = container_override(conf)?;
    // 按服务覆盖：`perms.services.<name>.uid/gid`。全局映射（rootless）优先，因为
    // 它描述的是“容器内 uid → 宿主 uid”的整体换算，不能与单个服务的容器内身份混用。
    let service_overrides: BTreeMap<String, ContainerId> = conf
        .map(|p| {
            p.services()
                .iter()
                .map(|(name, sp)| {
                    (
                        name.clone(),
                        ContainerId {
                            uid: sp.uid(),
                            gid: sp.gid(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let root = canonical_or_parent(system_root);
    let allow_external: Vec<PathBuf> = conf
        .map(|p| {
            p.allow_external()
                .iter()
                .map(|raw| {
                    let path = PathBuf::from(raw);
                    if path.is_absolute() {
                        canonical_or_parent(&path)
                    } else {
                        canonical_or_parent(&system_root.join(path))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    if overrides.is_some() && !service_overrides.is_empty() {
        warnings.push(
            "`perms.container_uid/gid` 已设置（全局宿主映射），`perms.services` 将被忽略"
                .to_string(),
        );
    }
    if !system_root.join(".env").is_file() {
        warnings.push(
            "系统根没有 `.env`（可能尚未 `gops sys localize`）；\
             含 `${VAR}` 的挂载路径可能尚未按生效值展开"
                .to_string(),
        );
    }
    let mut mounts: Vec<BindMount> = Vec::new();
    let mut inspected: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (service_name, service) in &config.services {
        let bind_volumes: Vec<&ComposeVolume> = service
            .volumes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|v| v.kind.as_deref() == Some("bind"))
            .filter(|v| {
                v.source
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .collect();
        if bind_volumes.is_empty() {
            continue;
        }

        let container = match resolve_container_id(
            service_name,
            service,
            overrides,
            &service_overrides,
            &mut inspected,
        ) {
            Ok(id) => id,
            Err(reason) => {
                warnings.push(format!(
                    "服务 `{service_name}` 的容器身份无法确定（{reason}）；跳过其 bind 挂载"
                ));
                continue;
            }
        };
        // root 不受宿主文件模式限制：对齐只会收紧模式（如 0644 → 0640），反而可能
        // 弄坏非 root 消费方（htpasswd 由 nginx worker 读、initdb.d 由降权后的 postgres 读）。
        if container.uid == 0 {
            notes.push(format!(
                "服务 `{service_name}` 以 root 运行，跳过其 bind 挂载对齐（root 不受宿主文件模式限制）；\
                 若该镜像的入口会降权到非 root（如 postgres → 999），\
                 请在 `sys-prj.yml: perms.services.{service_name}` 声明 uid/gid"
            ));
            continue;
        }

        for volume in bind_volumes {
            let raw = volume.source.as_deref().unwrap_or_default();
            let source = PathBuf::from(raw);
            let source = if source.is_absolute() {
                source
            } else {
                system_root.join(source)
            };
            let source = canonical_or_parent(&source);

            // 安全边界：挂载源如果是**系统根本身或它的祖先**，递归对齐会改到交付内容
            // （`docs/`、`sys/`、脚本、`.git` …），这不是本命令的职责。
            // 常见例子：`.:/app`；或 `allow_external` 把含系统根的父目录放进来了。
            if root.starts_with(&source) {
                warnings.push(format!(
                    "服务 `{service_name}` 的 bind source 等于/包含系统根（{}）；\
                     为避免改到交付内容，跳过（请把挂载细化到配置/运行态子目录）",
                    source.display()
                ));
                continue;
            }

            if !in_scope(&source, &root, &allow_external) {
                warnings.push(format!(
                    "服务 `{service_name}` 的 bind source 在系统根之外（{}）；\
                     跳过（如确需处理，在 `sys-prj.yml: perms.allow_external` 显式声明）",
                    source.display()
                ));
                continue;
            }

            mounts.push(BindMount {
                service: service_name.clone(),
                source,
                read_only: volume.read_only,
                container,
            });
        }
    }

    for name in service_overrides.keys() {
        if !config.services.contains_key(name) {
            warnings.push(format!(
                "`sys-prj.yml: perms.services.{name}` 未匹配到 compose 服务（检查服务名拼写）"
            ));
        }
    }

    let mounts = merge_mounts(mounts, &mut errors);
    // 0600 策略下，非 root 容器读不到密钥：这是配置错误（容器启动后必然失败），
    // 直接判失败并给出“改策略”的修复方向，而不是引导去提权。
    if let Some(secret) = secret.filter(|s| s.mode == 0o600) {
        let mut reported = false;
        for mount in &mounts {
            if !reported && mount.container.uid != 0 && secret.path.starts_with(&mount.source) {
                errors.push(format!(
                    "密钥文件 {} 由非 root 容器（{}:{}）挂载，但 secrets.mode = 0600（只属主可读），\
                     容器读不到；请在 sys-prj.yml 设 `secrets.mode: \"0640\"`，\
                     并把文件属组设为容器 gid（部署账号需已是该组成员）",
                    secret.path.display(),
                    mount.container.uid,
                    mount.container.gid
                ));
                reported = true;
            }
        }
    }
    Ok(Derived {
        mounts,
        warnings,
        notes,
        errors,
    })
}

/// 同一宿主路径被多个服务挂载时合并：读写优先（需求更宽），容器身份必须一致。
fn merge_mounts(mounts: Vec<BindMount>, errors: &mut Vec<String>) -> Vec<BindMount> {
    let mut merged: BTreeMap<PathBuf, BindMount> = BTreeMap::new();
    for mount in mounts {
        match merged.get_mut(&mount.source) {
            None => {
                merged.insert(mount.source.clone(), mount);
            }
            Some(existing) => {
                if existing.container != mount.container {
                    errors.push(format!(
                        "{} 被多个服务以不同容器身份挂载（{}:{} vs {}:{}），\
                         无法用单一属组满足；请拆开挂载源或统一 `user:`",
                        mount.source.display(),
                        existing.container.uid,
                        existing.container.gid,
                        mount.container.uid,
                        mount.container.gid
                    ));
                }
                existing.read_only = existing.read_only && mount.read_only;
                if !existing.service.split(',').any(|s| s == mount.service) {
                    existing.service = format!("{},{}", existing.service, mount.service);
                }
            }
        }
    }
    merged.into_values().collect()
}

fn resolve_container_id(
    service_name: &str,
    service: &ComposeService,
    overrides: Option<ContainerId>,
    service_overrides: &BTreeMap<String, ContainerId>,
    inspected: &mut BTreeMap<String, Option<String>>,
) -> Result<ContainerId, String> {
    // 顶层全局映射优先（rootless / userns-remap：容器内 uid ≠ 宿主 uid）。
    if let Some(id) = overrides {
        return Ok(id);
    }
    // 单服务覆盖：仅影响对齐，不改运行时身份。
    if let Some(id) = service_overrides.get(service_name) {
        return Ok(*id);
    }
    if let Some(user) = service
        .user
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return parse_user(user).ok_or_else(|| {
            format!(
                "`user: {user}` 不是数字 `uid:gid`（如 `999:999`）；\n                 \
                 请在 `sys-prj.yml: perms.services.{service_name}` 声明该服务的有效身份，\n                 \
                 或用 `perms.container_uid/gid` 给出全局宿主映射"
            )
        });
    }
    let Some(image) = service.image.as_deref().filter(|s| !s.is_empty()) else {
        return Err(format!(
            "服务 `{service_name}` 既没有 `user:` 也没有 `image:`"
        ));
    };
    let inspected_user = inspected
        .entry(image.to_string())
        .or_insert_with(|| inspect_image_user(image));
    match inspected_user.as_deref() {
        Some(user) if !user.is_empty() => parse_user(user).ok_or_else(|| {
            format!(
                "镜像 `{image}` 的 `Config.User` 为 `{user}`，不是数字 `uid:gid`；\
                 请在 `sys-prj.yml: perms.services.{service_name}`（推荐）\
                 或 `perms.container_uid/gid` 显式给出宿主映射"
            )
        }),
        // Docker 语义：`Config.User` 为空 = root（0:0），不是“读不到”。
        // 是否需要对齐由调用方决定（root 会被跳过）。
        Some(_) => Ok(ContainerId { uid: 0, gid: 0 }),
        None => Err(format!(
            "服务 `{service_name}` 没有 `user:`，且无法读取镜像 `{image}` 的 `Config.User`\
             （镜像未拉取 / docker 不可用？）；请先 `gops run download`，\
             或在 `sys-prj.yml: perms.services.{service_name}` 声明该服务的 uid/gid"
        )),
    }
}

fn parse_user(user: &str) -> Option<ContainerId> {
    let user = user.trim();
    let mut parts = user.splitn(2, ':');
    let uid: u32 = parts.next()?.trim().parse().ok()?;
    let gid = match parts.next().map(str::trim) {
        None | Some("") => 0,
        Some(gid) => gid.parse().ok()?,
    };
    Some(ContainerId { uid, gid })
}

fn inspect_image_user(image: &str) -> Option<String> {
    let out = Command::new("docker")
        .args(["image", "inspect", image, "--format", "{{.Config.User}}"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// rootless / userns-remap 时容器内 uid 不等于宿主 uid，直接按 compose 对齐会错。
fn user_namespace_reason() -> Option<String> {
    let out = Command::new("docker")
        .args(["info", "--format", "{{json .SecurityOptions}}"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    user_namespace_reason_from(&String::from_utf8_lossy(&out.stdout))
}

/// 从 `docker info` 的 `SecurityOptions` 文本判断（独立出来便于单测）。
fn user_namespace_reason_from(options: &str) -> Option<String> {
    if options.contains("rootless") {
        Some("rootless docker".to_string())
    } else if options.contains("userns") {
        Some("docker userns-remap".to_string())
    } else {
        None
    }
}

fn compose_config_json(
    system_root: &Path,
    compose_file: &Path,
    global_args: &[std::ffi::OsString],
) -> MainResult<String> {
    let mut cmd = Command::new("docker");
    cmd.arg("compose")
        .args(global_args)
        .arg("config")
        .arg("--format")
        .arg("json")
        .current_dir(system_root)
        .stdin(Stdio::null());
    // 密钥只需要满足 compose 的变量替换，不参与 user/volumes 派生；注入掩码即可。
    if let Ok(dict) = orion_sec::load_sec_dict() {
        for (key, _) in dict.iter() {
            cmd.env(key.as_str(), "********");
        }
    }
    let out = cmd.output().map_err(|e| {
        MainReason::resource_detail(format!(
            "无法执行 `docker compose config`（需要 docker CLI）：{e}"
        ))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(MainReason::logic_detail(format!(
            "`docker compose config` 失败（{}）：{}",
            compose_file.display(),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// === 检查 / 修复 ===

fn run(
    system_root: &Path,
    conf: Option<&PermsConf>,
    secret: Option<SecretTarget<'_>>,
    do_apply: bool,
) -> MainResult<PermsReport> {
    let derived = derive(system_root, conf, secret)?;
    run_derived_with_secret(system_root, derived, secret, do_apply)
}

/// 在已派生好的挂载计划上执行检查/修复（不碰 docker，便于端到端单测）。
fn run_derived_with_secret(
    system_root: &Path,
    derived: Derived,
    secret: Option<SecretTarget<'_>>,
    do_apply: bool,
) -> MainResult<PermsReport> {
    // root（含 sudo）能 chgrp 到任意 gid，不存在「成员缺失」；其余情况先取一次真实生效的组，
    // 必要时把 N 个文件的同一个 EPERM 收成一条。
    let privileged = unsafe { libc::geteuid() } == 0;
    let groups = effective_groups();
    run_derived_with_groups(system_root, derived, secret, do_apply, privileged, &groups)
}

/// 可注入「是否有特权 / 进程属于哪些组」的版本：单测不需要依赖跑测试的账号。
fn run_derived_with_groups(
    system_root: &Path,
    derived: Derived,
    secret: Option<SecretTarget<'_>>,
    do_apply: bool,
    privileged: bool,
    groups: &[u32],
) -> MainResult<PermsReport> {
    let (deploy_uid, deploy_gid) = deploy_identity();
    // 配置错误不能靠改权限修复（例如 0600 密钥配非 root 容器、同一路径两种身份）：
    // 此时 `--apply` 不应动盘，避免把错误配置“应用”出去。
    // 有配置错误时也不再报「宿主组缺失」：身份本身就还没定下来，先把配置修正。
    let config_error = !derived.errors.is_empty();
    let apply = do_apply && !config_error;
    let mut errors = derived.errors;
    // gid → (待修项数, 涉及服务)：同一个 gid 只报一条。
    let mut blocked: BTreeMap<u32, (usize, Vec<String>)> = BTreeMap::new();
    let mut mounts = Vec::new();

    for mount in &derived.mounts {
        let mut items = scan_mount_with_secret(mount, deploy_uid, secret)?;
        // 只在**确实有东西要改**时才拦：已对齐的现场不需要为「组关系」报错。
        let needs_fix = items.iter().any(|i| i.status != ItemStatus::Aligned);
        let blocked_by_group = (!config_error
            && !privileged
            && mount.container.uid != 0
            && !groups.contains(&mount.container.gid)
            && needs_fix)
            .then_some(mount.container.gid);
        if let Some(gid) = blocked_by_group {
            let entry = blocked.entry(gid).or_insert_with(|| (0, Vec::new()));
            entry.0 += items
                .iter()
                .filter(|i| i.status != ItemStatus::Aligned)
                .count();
            for name in mount.service.split(',') {
                if !entry.1.iter().any(|s| s == name) {
                    entry.1.push(name.to_string());
                }
            }
        }
        if apply && blocked_by_group.is_none() {
            for item in items.iter_mut() {
                if item.status != ItemStatus::Pending {
                    continue;
                }
                match apply_item_for(item, mount, deploy_uid) {
                    Ok(()) => item.status = ItemStatus::Applied,
                    Err(err) => {
                        item.status = ItemStatus::Failed;
                        item.reason = err;
                    }
                }
            }
            // 修复后**再探一次**：ACL / 只读挂载 / 父目录权限等会留下残留，不能静默通过。
            for item in items.iter_mut() {
                if item.status == ItemStatus::Failed {
                    continue;
                }
                let Some(snapshot) = stat_snapshot(&item.path) else {
                    item.status = ItemStatus::Failed;
                    item.reason = "修复后路径消失".to_string();
                    continue;
                };
                let secret_mode =
                    secret.and_then(|s| (s.path == item.path.as_path()).then_some(s.mode));
                let (ok, reason) = aligned_with_secret(
                    snapshot,
                    item.kind == "dir",
                    mount,
                    deploy_uid,
                    secret_mode,
                );
                item.current = Some(snapshot);
                if !ok {
                    item.status = ItemStatus::Failed;
                    item.reason = format!("修复后仍不一致：{reason}（ACL / 只读挂载？）");
                }
            }
        }
        mounts.push(MountReport {
            service: mount.service.clone(),
            source: mount.source.clone(),
            read_only: mount.read_only,
            container: mount.container,
            items,
            blocked_by_group,
        });
    }

    for (gid, (count, services)) in &blocked {
        let current = groups
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("、");
        errors.push(format!(
            "部署账号（uid {deploy_uid}；当前组 {current}）不在容器 gid {gid} 的组里：\
             {count} 项要 chgrp 到 {gid}（服务：{}）。非 root 只能 chgrp 到自己所在的组，\
             这是宿主预置、gops 不提权。请管理员执行：\n  \
             sudo groupadd -g {gid} <组名>      # 该 gid 尚无对应组时才需要\n  \
             sudo usermod -aG {gid} <部署账号>   # 之后重新登录；systemd 服务需重启单元\n\
             再重跑 `gops sys perms --apply`",
            services.join("、")
        ));
    }

    Ok(PermsReport {
        system_root: system_root.to_path_buf(),
        skipped: None,
        deploy_uid,
        deploy_gid,
        mounts,
        warnings: derived.warnings,
        notes: derived.notes,
        errors,
    })
}

/// 当前进程真实拥有的组（egid + 补充组）。
///
/// 只用于「动手前判定 `chgrp` 能不能成」：内核按**数字**比对进程凭据（`in_group_p`），
/// 与 `/etc/group` 里有没有同名组无关。root 不受此限，调用方自行跳过。
fn effective_groups() -> Vec<u32> {
    let mut groups = vec![unsafe { libc::getegid() }];
    // getgroups(2) 两次调用：先问个数、再取列表；组的集合在两次之间理论上可变，取回填值兜底。
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count > 0 {
        let mut buf = vec![0 as libc::gid_t; count as usize];
        let filled = unsafe { libc::getgroups(count, buf.as_mut_ptr()) };
        if filled > 0 {
            groups.extend(buf[..filled as usize].iter().copied());
        }
    }
    groups.sort_unstable();
    groups.dedup();
    groups
}

fn scan_mount_with_secret(
    mount: &BindMount,
    deploy_uid: u32,
    secret: Option<SecretTarget<'_>>,
) -> MainResult<Vec<ItemReport>> {
    let mut items = Vec::new();
    let is_secret_mount = secret.map(|s| s.path == mount.source).unwrap_or(false);
    let expected = secret
        .filter(|_| is_secret_mount)
        .map(|s| s.mode)
        .unwrap_or_else(|| expected_mode(mount.read_only, true));

    if !mount.source.exists() {
        items.push(ItemReport {
            path: mount.source.clone(),
            kind: "dir",
            read_only: mount.read_only,
            current: None,
            expected_mode: expected,
            expected_gid: mount.container.gid,
            is_secret: is_secret_mount,
            status: ItemStatus::Pending,
            reason: "挂载源不存在（按目录处理；`docker compose up` 会以 root 创建）".to_string(),
        });
        return Ok(items);
    }

    for entry in walkdir::WalkDir::new(&mount.source).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let path = err
                    .path()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| mount.source.clone());
                let is_secret = secret.map(|s| s.path == path.as_path()).unwrap_or(false);
                items.push(ItemReport {
                    path,
                    kind: "?",
                    read_only: mount.read_only,
                    current: None,
                    expected_mode: expected,
                    expected_gid: mount.container.gid,
                    is_secret,
                    status: ItemStatus::Failed,
                    reason: format!("读取目录失败：{err}"),
                });
                continue;
            }
        };
        let file_type = entry.file_type();
        // 不跟随符号链接，也不改链接本身（chmod 会作用到链接目标）。
        if file_type.is_symlink() {
            continue;
        }
        let is_dir = file_type.is_dir();
        let Some(snapshot) = stat_snapshot(entry.path()) else {
            continue;
        };
        let secret_mode = secret.and_then(|s| (s.path == entry.path()).then_some(s.mode));
        let entry_expected_mode = match secret_mode {
            Some(mode) if !is_dir => mode,
            _ => expected_mode(mount.read_only, is_dir),
        };
        let (ok, reason) = aligned_with_secret(snapshot, is_dir, mount, deploy_uid, secret_mode);
        items.push(ItemReport {
            path: entry.path().to_path_buf(),
            kind: if is_dir { "dir" } else { "file" },
            read_only: mount.read_only,
            current: Some(snapshot),
            expected_mode: entry_expected_mode,
            expected_gid: mount.container.gid,
            is_secret: secret_mode.is_some(),
            status: if ok {
                ItemStatus::Aligned
            } else {
                ItemStatus::Pending
            },
            reason,
        });
    }
    Ok(items)
}

/// 是否“已对齐”：容器自建的跳过；其余要求属主 = 部署账号、属组 = 容器 gid、
/// 模式 = 目标模式（由目标模式可推出容器身份实际可读/可写，另外再校验一次）。
fn aligned_with_secret(
    snapshot: FileSnapshot,
    is_dir: bool,
    mount: &BindMount,
    deploy_uid: u32,
    secret_mode: Option<u32>,
) -> (bool, String) {
    // 密钥文件（`sys-prj.yml: secrets.file`）：属主**始终是部署账号**，
    // 模式由 `secrets.mode` 决定（0600 / 0640）；0640 时容器按属组读。
    // 必须放在「容器自建文件直接放过」之前，否则模式错误会被误判为已对齐。
    if let Some(mode) = secret_mode.filter(|_| !is_dir) {
        if snapshot.uid != deploy_uid {
            return (
                false,
                format!(
                    "属主 {} 不是部署账号 {}（密钥不允许 chown 给容器 uid）",
                    snapshot.uid, deploy_uid
                ),
            );
        }
        if snapshot.gid != mount.container.gid {
            return (
                false,
                format!("属组 {} 不是容器 gid {}", snapshot.gid, mount.container.gid),
            );
        }
        let current = snapshot.mode & 0o7777;
        if current != mode {
            return (
                false,
                format!(
                    "模式 {} 不是 {}（secrets.mode 要求）",
                    format_mode(current),
                    format_mode(mode)
                ),
            );
        }
        return (true, String::new());
    }
    // 「容器自己创建的」不动：否则每次 localize 都要 sudo。
    // 但部署账号与容器身份相同时（uid 相等），文件本来就是我们自己的，照常对齐模式。
    if snapshot.uid == mount.container.uid && snapshot.uid != deploy_uid {
        return (true, String::new());
    }
    if snapshot.uid != deploy_uid {
        return (
            false,
            format!(
                "属主 {} 不是部署账号 {}（也不属于容器身份 {})",
                snapshot.uid, deploy_uid, mount.container.uid
            ),
        );
    }
    if snapshot.gid != mount.container.gid {
        return (
            false,
            format!("属组 {} 不是容器 gid {}", snapshot.gid, mount.container.gid),
        );
    }
    let expected = expected_mode(mount.read_only, is_dir);
    let current = snapshot.mode & 0o7777;
    if current != expected {
        return (
            false,
            format!(
                "模式 {} 不是 {}",
                format_mode(current),
                format_mode(expected)
            ),
        );
    }
    let need = if mount.read_only {
        Need::Read
    } else {
        Need::Write
    };
    if !can_access(snapshot, mount.container, is_dir, need) {
        return (
            false,
            format!(
                "容器身份 {}:{} 不能{}",
                mount.container.uid,
                mount.container.gid,
                if mount.read_only { "读" } else { "写" }
            ),
        );
    }
    (true, String::new())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Read,
    Write,
}

fn can_access(snapshot: FileSnapshot, container: ContainerId, is_dir: bool, need: Need) -> bool {
    if container.uid == 0 {
        return true;
    }
    let mode = snapshot.mode & 0o7777;
    let (read, write, exec) = if snapshot.uid == container.uid {
        (mode & 0o400, mode & 0o200, mode & 0o100)
    } else if snapshot.gid == container.gid {
        (mode & 0o040, mode & 0o020, mode & 0o010)
    } else {
        (mode & 0o004, mode & 0o002, mode & 0o001)
    };
    match need {
        Need::Read if is_dir => read != 0 && exec != 0,
        Need::Write if is_dir => write != 0 && exec != 0,
        Need::Read => read != 0,
        Need::Write => write != 0,
    }
}

fn apply_item_for(item: &ItemReport, mount: &BindMount, deploy_uid: u32) -> Result<(), String> {
    let path = &item.path;
    if !path.exists() {
        // 密钥是运维输入，不能由 perms “创建”（否则会造出一个空目录/空文件，
        // 掩盖真正的问题）；宿主侧加载器会明确报缺失。
        if item.is_secret {
            return Err(format!(
                "密钥文件 {} 不存在：perms 不创建密钥，请先由运维提供",
                path.display()
            ));
        }
        std::fs::create_dir_all(path)
            .map_err(|e| format!("创建挂载源 {} 失败：{e}", path.display()))?;
    }
    let mode = item.expected_mode;

    // 属主始终是部署账号，不 chown 给容器 uid（那需要提权）。
    // chgrp 到容器 gid 需要部署账号已是该组成员：这是宿主预置，gops 不负责提权。
    std::os::unix::fs::chown(path, Some(deploy_uid), Some(mount.container.gid)).map_err(|e| {
        format!(
            "设置 {} 为 {}:{} 失败：{e}（需要部署账号拥有该文件、且已是容器 gid 组成员；\
             这属于宿主预置权限，gops 不会提权）",
            path.display(),
            deploy_uid,
            mount.container.gid
        )
    })?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).map_err(
        |e| {
            format!(
                "chmod {} 到 {} 失败：{e}",
                path.display(),
                format_mode(mode)
            )
        },
    )?;
    Ok(())
}

fn stat_snapshot(path: &Path) -> Option<FileSnapshot> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(FileSnapshot {
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.mode(),
    })
}

fn expected_mode(read_only: bool, is_dir: bool) -> u32 {
    match (read_only, is_dir) {
        (false, true) => RW_DIR,
        (false, false) => RW_FILE,
        (true, true) => RO_DIR,
        (true, false) => RO_FILE,
    }
}

fn deploy_identity() -> (u32, u32) {
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::geteuid() });
    let gid = std::env::var("SUDO_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getegid() });
    (uid, gid)
}

fn in_scope(source: &Path, root: &Path, allow_external: &[PathBuf]) -> bool {
    source.starts_with(root)
        || allow_external
            .iter()
            .any(|allowed| source.starts_with(allowed))
}

/// 规范化到“最长的已存在祖先 + 剩余路径”，不要求路径本身存在。
fn canonical_or_parent(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            let mut out = canonical;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match existing.file_name() {
            Some(name) => {
                suffix.push(name.to_os_string());
                if !existing.pop() {
                    return path.to_path_buf();
                }
            }
            None => return path.to_path_buf(),
        }
    }
}

fn format_mode(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

fn display_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    // 测试便捷入口：不涉及 `secrets.file` 特例时，固定 secret = None / 非密钥。
    fn run_derived(root: &Path, derived: Derived, do_apply: bool) -> MainResult<PermsReport> {
        run_derived_with_secret(root, derived, None, do_apply)
    }

    fn scan_mount(m: &BindMount, deploy_uid: u32) -> MainResult<Vec<ItemReport>> {
        scan_mount_with_secret(m, deploy_uid, None)
    }

    fn derive_from_json(root: &Path, json: &str, conf: Option<&PermsConf>) -> MainResult<Derived> {
        derive_from_json_with_secret(root, json, conf, None)
    }

    fn aligned(
        snapshot: FileSnapshot,
        is_dir: bool,
        m: &BindMount,
        deploy_uid: u32,
    ) -> (bool, String) {
        aligned_with_secret(snapshot, is_dir, m, deploy_uid, None)
    }

    fn apply_item(item: &ItemReport, m: &BindMount, deploy_uid: u32) -> Result<(), String> {
        apply_item_for(item, m, deploy_uid)
    }

    fn current_id() -> ContainerId {
        let (uid, gid) = deploy_identity();
        ContainerId { uid, gid }
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mount(source: &Path, read_only: bool, container: ContainerId) -> BindMount {
        BindMount {
            service: "svc".to_string(),
            source: source.to_path_buf(),
            read_only,
            container,
        }
    }

    #[test]
    fn test_derive_from_json_bind_mounts_and_read_only() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{
              "services": {{
                "gateway": {{
                  "image": "gw:1",
                  "user": "999:999",
                  "volumes": [
                    {{"type":"bind","source":"{}","target":"/config"}},
                    {{"type":"bind","source":"{}","target":"/config/identity","read_only":true}},
                    {{"type":"volume","source":"db-data","target":"/db"}}
                  ]
                }}
              }}
            }}"#,
            root.join("configs").display(),
            root.join("identity").display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();
        assert_eq!(derived.mounts.len(), 2, "named volume 必须跳过");
        let rw = derived
            .mounts
            .iter()
            .find(|m| !m.read_only)
            .expect("rw mount");
        assert_eq!(rw.service, "gateway");
        assert_eq!(rw.container, ContainerId { uid: 999, gid: 999 });
        assert!(rw.source.ends_with("configs"));
        let ro = derived
            .mounts
            .iter()
            .find(|m| m.read_only)
            .expect("ro mount");
        assert!(ro.source.ends_with("identity"));
        assert!(derived.warnings.is_empty(), "{:?}", derived.warnings);
    }

    #[test]
    fn test_derive_from_json_skips_outside_root_and_missing_identity() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = r#"{
          "services": {
            "a": {
              "user": "999:999",
              "volumes": [{"type":"bind","source":"/etc/outside","target":"/x"}]
            },
            "b": {
              "volumes": [{"type":"bind","source":"./configs","target":"/y"}]
            }
          }
        }"#;
        let derived = derive_from_json(root, json, None).unwrap();
        assert!(derived.mounts.is_empty(), "根外 / 身份不明都应跳过");
        assert_eq!(derived.warnings.len(), 2, "{:?}", derived.warnings);
        assert!(derived.warnings.iter().any(|w| w.contains("系统根之外")));
        assert!(
            derived
                .warnings
                .iter()
                .any(|w| w.contains("容器身份无法确定"))
        );
    }

    #[test]
    fn test_derive_from_json_partial_override_is_rejected() {
        let dir = tempdir().unwrap();
        let json = r#"{"services":{"a":{"user":"999:999","volumes":[]}}}"#;
        let conf: PermsConf = serde_yaml::from_str("container_uid: 1000").unwrap();
        let err = derive_from_json(dir.path(), json, Some(&conf)).unwrap_err();
        assert!(err.to_string().contains("必须同时配置"));
    }

    #[test]
    fn test_derive_from_json_override_wins() {
        let dir = tempdir().unwrap();
        let json = format!(
            r#"{{"services":{{"a":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/x"}}]}}}}}}"#,
            dir.path().join("configs").display()
        );
        let conf: PermsConf =
            serde_yaml::from_str("container_uid: 100999\ncontainer_gid: 100999").unwrap();
        let derived = derive_from_json(dir.path(), &json, Some(&conf)).unwrap();
        assert_eq!(
            derived.mounts[0].container,
            ContainerId {
                uid: 100999,
                gid: 100999
            }
        );
    }

    #[test]
    fn test_perms_conf_services_requires_uid_and_gid() {
        assert!(
            serde_yaml::from_str::<PermsConf>("services:\n  postgres:\n    uid: 999\n").is_err(),
            "只给 uid 应报错"
        );
        let conf: PermsConf =
            serde_yaml::from_str("services:\n  postgres:\n    uid: 999\n    gid: 999\n").unwrap();
        let sp = conf.services().get("postgres").unwrap();
        assert_eq!((sp.uid(), sp.gid()), (999, 999));

        // 服务条目里的拼写错误不能被静默忽略。
        assert!(
            serde_yaml::from_str::<PermsConf>(
                "services:\n  postgres:\n    uid: 999\n    gid: 999\n    uidd: 1\n"
            )
            .is_err(),
            "未知字段应报错"
        );
    }

    #[test]
    fn test_derive_per_service_override_wins_over_compose_user() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"postgres":{{"user":"0:0","volumes":[{{"type":"bind","source":"{}","target":"/initdb","read_only":true}}]}}}}}}"#,
            root.join("initdb").display()
        );
        let conf: PermsConf =
            serde_yaml::from_str("services:\n  postgres:\n    uid: 999\n    gid: 999\n").unwrap();
        let derived = derive_from_json(root, &json, Some(&conf)).unwrap();
        assert_eq!(derived.mounts.len(), 1);
        assert_eq!(
            derived.mounts[0].container,
            ContainerId { uid: 999, gid: 999 },
            "按服务的声明应优先于 compose 的 user:"
        );
        assert!(derived.notes.is_empty(), "{:?}", derived.notes);
    }

    #[test]
    fn test_derive_root_service_is_skipped_with_note() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"web":{{"user":"0:0","volumes":[{{"type":"bind","source":"{}","target":"/certs","read_only":true}}]}}}}}}"#,
            root.join("certs").display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();
        assert!(derived.mounts.is_empty(), "root 服务不参与对齐");
        assert_eq!(derived.notes.len(), 1, "{:?}", derived.notes);
        assert!(derived.notes[0].contains("以 root 运行"));
        assert!(derived.notes[0].contains("perms.services.web"));
        assert!(derived.warnings.is_empty(), "{:?}", derived.warnings);
        assert!(derived.errors.is_empty());
    }

    #[test]
    fn test_derive_root_and_nonroot_share_source_no_conflict() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let shared = root.join("secrets");
        let json = format!(
            r#"{{"services":{{
              "center":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/secrets","read_only":true}}]}},
              "db-init":{{"user":"0:0","volumes":[{{"type":"bind","source":"{}","target":"/secrets","read_only":true}}]}}
            }}}}"#,
            shared.display(),
            shared.display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();
        assert!(derived.errors.is_empty(), "{:?}", derived.errors);
        assert_eq!(derived.mounts.len(), 1, "root 服务应被跳过");
        assert_eq!(
            derived.mounts[0].container,
            ContainerId { uid: 999, gid: 999 }
        );
        assert_eq!(derived.notes.len(), 1, "{:?}", derived.notes);
        assert!(derived.notes[0].contains("db-init"));
    }

    #[test]
    fn test_resolve_empty_image_user_means_root_but_missing_image_is_error() {
        let service = ComposeService {
            user: None,
            image: Some("postgres:16".to_string()),
            volumes: None,
        };
        let overrides = BTreeMap::new();

        // Docker 语义：Config.User 为空 = root（不是“读不到”）。
        let mut inspected = BTreeMap::from([("postgres:16".to_string(), Some(String::new()))]);
        let id =
            resolve_container_id("postgres", &service, None, &overrides, &mut inspected).unwrap();
        assert_eq!(id, ContainerId { uid: 0, gid: 0 });

        // docker inspect 失败（镜像未拉取）：报错并指向 per-service 声明。
        let mut inspected = BTreeMap::from([("postgres:16".to_string(), None)]);
        let err = resolve_container_id("postgres", &service, None, &overrides, &mut inspected)
            .unwrap_err();
        assert!(err.contains("perms.services.postgres"), "{err}");
    }

    #[test]
    fn test_derive_unknown_service_override_warns() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"web":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/certs"}}]}}}}}}"#,
            root.join("certs").display()
        );
        let conf: PermsConf =
            serde_yaml::from_str("services:\n  ghost:\n    uid: 1\n    gid: 1\n").unwrap();
        let derived = derive_from_json(root, &json, Some(&conf)).unwrap();
        assert!(
            derived
                .warnings
                .iter()
                .any(|w| w.contains("perms.services.ghost")),
            "{:?}",
            derived.warnings
        );
    }

    #[test]
    fn test_derive_global_override_ignores_services_with_warning() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"web":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/certs"}}]}}}}}}"#,
            root.join("certs").display()
        );
        let conf: PermsConf = serde_yaml::from_str(
            "container_uid: 100999\ncontainer_gid: 100999\nservices:\n  web:\n    uid: 1\n    gid: 1\n",
        )
        .unwrap();
        let derived = derive_from_json(root, &json, Some(&conf)).unwrap();
        assert_eq!(
            derived.mounts[0].container,
            ContainerId {
                uid: 100999,
                gid: 100999
            },
            "全局映射优先"
        );
        assert!(
            derived
                .warnings
                .iter()
                .any(|w| w.contains("perms.services` 将被忽略")),
            "{:?}",
            derived.warnings
        );
    }

    #[test]
    fn test_run_derived_end_to_end_check_then_apply_is_idempotent() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let rw_dir = root.join("configs");
        let ro_dir = root.join("packages");
        std::fs::create_dir_all(&rw_dir).unwrap();
        std::fs::create_dir_all(&ro_dir).unwrap();
        let log = rw_dir.join("gateway.log");
        let pkg = ro_dir.join("pkg.tar.gz");
        std::fs::write(&log, "line\n").unwrap();
        std::fs::write(&pkg, "pkg").unwrap();
        chmod(&rw_dir, 0o755);
        chmod(&ro_dir, 0o755);
        chmod(&log, 0o644);
        chmod(&pkg, 0o600);

        let id = current_id();
        let json = format!(
            r#"{{"services":{{"gateway":{{"user":"{}:{}","volumes":[
              {{"type":"bind","source":"{}","target":"/config"}},
              {{"type":"bind","source":"{}","target":"/packages","read_only":true}}
            ]}}}}}}"#,
            id.uid,
            id.gid,
            rw_dir.display(),
            ro_dir.display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();

        // 1) 检查：rw 目录/文件与 ro 文件都未对齐
        let report = run_derived(root, derived, false).unwrap();
        assert!(report.pending() >= 3, "pending={}", report.pending());
        assert_eq!(report.applied(), 0);
        assert!(!report.is_ok());

        // 2) 修复：rw 2770/660，ro 2750/640（不放宽 other）
        let derived = derive_from_json(root, &json, None).unwrap();
        let report = run_derived(root, derived, true).unwrap();
        assert!(report.is_ok(), "{}", report.render_text());
        assert_eq!(
            std::fs::metadata(&rw_dir).unwrap().permissions().mode() & 0o7777,
            0o2770
        );
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o7777,
            0o660
        );
        assert_eq!(
            std::fs::metadata(&ro_dir).unwrap().permissions().mode() & 0o7777,
            0o2750
        );
        let pkg_mode = std::fs::metadata(&pkg).unwrap().permissions().mode() & 0o7777;
        assert_eq!(pkg_mode, 0o640);
        assert_eq!(pkg_mode & 0o004, 0, "只读挂载不放宽到 other");

        // 3) 再跑一次：无改动、全部对齐（幂等）
        let derived = derive_from_json(root, &json, None).unwrap();
        let report = run_derived(root, derived, true).unwrap();
        assert!(report.is_ok());
        assert_eq!(report.applied(), 0, "第二次 apply 不应再改动");

        // 4) 运行态新长出的文件：不需要改 gops，递归检查自动纳入
        let new_file = rw_dir.join("rotate/2026.log");
        std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
        std::fs::write(&new_file, "new\n").unwrap();
        chmod(new_file.parent().unwrap(), 0o755);
        chmod(&new_file, 0o644);
        let derived = derive_from_json(root, &json, None).unwrap();
        let report = run_derived(root, derived, false).unwrap();
        assert!(!report.is_ok(), "新文件必须自动纳入检查");
        assert!(
            report
                .mounts
                .iter()
                .flat_map(|m| m.items.iter())
                .any(|i| i.path.ends_with("rotate/2026.log") && i.status == ItemStatus::Pending),
            "未知新文件应待修"
        );
    }

    #[test]
    fn test_user_namespace_reason_from_security_options() {
        assert_eq!(
            user_namespace_reason_from(r#"["name=seccomp","name=rootless"]"#).as_deref(),
            Some("rootless docker")
        );
        assert_eq!(
            user_namespace_reason_from(r#"["name=userns"]"#).as_deref(),
            Some("docker userns-remap")
        );
        assert_eq!(
            user_namespace_reason_from(r#"["name=seccomp","name=cgroupns"]"#),
            None
        );
    }

    #[test]
    fn test_report_json_and_render() {
        let skipped = PermsReport::skipped(Path::new("/site"), "只在 Linux 上生效");
        let json: serde_json::Value = serde_json::from_str(&skipped.to_json()).unwrap();
        assert_eq!(json["skipped"], "只在 Linux 上生效");
        assert!(skipped.render_text().contains("[SKIP]"));

        let report = PermsReport {
            system_root: PathBuf::from("/site"),
            skipped: None,
            deploy_uid: 1000,
            deploy_gid: 1000,
            mounts: vec![MountReport {
                service: "gateway".to_string(),
                source: PathBuf::from("/site/configs"),
                read_only: false,
                container: ContainerId { uid: 999, gid: 999 },
                items: vec![ItemReport {
                    path: PathBuf::from("/site/configs/key.pem"),
                    kind: "file",
                    read_only: false,
                    current: Some(FileSnapshot {
                        uid: 0,
                        gid: 0,
                        mode: 0o644,
                    }),
                    expected_mode: RW_FILE,
                    expected_gid: 999,
                    is_secret: false,
                    status: ItemStatus::Pending,
                    reason: "属主 0 不是部署账号 1000".to_string(),
                }],
                blocked_by_group: None,
            }],
            warnings: vec![],
            notes: vec![],
            errors: vec![],
        };
        assert_eq!(report.pending(), 1);
        assert!(!report.is_ok());
        assert_eq!(report.exit_code(), 1);
        let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(json["summary"]["pending"], 1);
        assert_eq!(json["mounts"][0]["items"][0]["expected"]["mode"], "0660");
        let text = report.render_text();
        assert!(text.contains("[FAIL] gateway"), "{text}");
        assert!(text.contains("gops sys perms --apply"), "{text}");
    }

    #[test]
    fn test_derive_skips_system_root_and_ancestor_bind() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let parent = root.parent().unwrap();

        let json = format!(
            r#"{{"services":{{
              "whole":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/app"}}]}},
              "ancestor":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/parent"}}]}}
            }}}}"#,
            root.display(),
            parent.display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();
        assert!(
            derived.mounts.is_empty(),
            "不能递归 chmod 系统根/其祖先：{:?}",
            derived.mounts
        );
        assert_eq!(derived.warnings.len(), 2, "{:?}", derived.warnings);
        assert!(
            derived
                .warnings
                .iter()
                .all(|w| w.contains("等于/包含系统根"))
        );
    }

    #[test]
    fn test_derive_handles_null_and_absent_volumes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = r#"{"services":{
          "a":{"user":"999:999","volumes":null},
          "b":{"user":"999:999"}
        }}"#;
        let derived = derive_from_json(root, json, None).unwrap();
        assert!(derived.mounts.is_empty());
        assert!(derived.warnings.is_empty(), "{:?}", derived.warnings);
    }

    #[test]
    fn test_derive_allow_external_permits_outside_source() {
        let outside = tempdir().unwrap();
        let root = tempdir().unwrap();
        std::fs::write(root.path().join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"a":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/shared"}}]}}}}}}"#,
            outside.path().display()
        );
        let conf: PermsConf = serde_yaml::from_str(&format!(
            "allow_external:\n  - {}\n",
            outside.path().display()
        ))
        .unwrap();
        let derived = derive_from_json(root.path(), &json, Some(&conf)).unwrap();
        assert_eq!(derived.mounts.len(), 1, "{:?}", derived.warnings);
        // 派生会把路径规范化（macOS 上 `/var` 是 `/private/var` 的符号链接），
        // 所以与规范化后的父路径比较。
        let outside_canonical = std::fs::canonicalize(outside.path()).unwrap();
        assert!(
            derived.mounts[0].source.starts_with(&outside_canonical),
            "source={:?}",
            derived.mounts[0].source
        );
    }

    #[test]
    fn test_parse_user_tolerates_whitespace() {
        assert_eq!(
            parse_user(" 999 : 999 "),
            Some(ContainerId { uid: 999, gid: 999 })
        );
        assert_eq!(parse_user("999:abc"), None);
    }

    #[test]
    fn test_load_conf_reads_perms_from_v2_and_legacy_v1() {
        // v2
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nperms:\n  container_uid: 1000\n  container_gid: 1001\n",
        )
        .unwrap();
        let conf = load_conf(dir.path()).unwrap().expect("conf");
        let perms = conf.perms().expect("perms");
        assert_eq!(perms.container_uid(), Some(1000));
        assert_eq!(perms.container_gid(), Some(1001));

        // v1（旧名）：只读回退，不迁移、不改现场
        let legacy = tempdir().unwrap();
        std::fs::write(
            legacy.path().join("sys_prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nperms:\n  container_uid: 2000\n  container_gid: 2001\n",
        )
        .unwrap();
        let conf = load_conf(legacy.path()).unwrap().expect("conf");
        assert_eq!(conf.perms().unwrap().container_uid(), Some(2000));
        assert!(legacy.path().join("sys_prj.yml").is_file());
        assert!(!legacy.path().join("sys-prj.yml").exists());
    }

    #[test]
    fn test_aligned_reports_owner_group_and_mode_reasons() {
        let m = mount(
            Path::new("/site/x"),
            false,
            ContainerId { uid: 999, gid: 999 },
        );
        let deploy = 1000;

        let (ok, reason) = aligned(
            FileSnapshot {
                uid: 0,
                gid: 999,
                mode: 0o660,
            },
            false,
            &m,
            deploy,
        );
        assert!(!ok);
        assert!(reason.contains("属主"), "{reason}");

        let (ok, reason) = aligned(
            FileSnapshot {
                uid: 1000,
                gid: 1000,
                mode: 0o660,
            },
            false,
            &m,
            deploy,
        );
        assert!(!ok);
        assert!(reason.contains("属组"), "{reason}");

        let (ok, reason) = aligned(
            FileSnapshot {
                uid: 1000,
                gid: 999,
                mode: 0o644,
            },
            false,
            &m,
            deploy,
        );
        assert!(!ok);
        assert!(reason.contains("模式"), "{reason}");

        let (ok, reason) = aligned(
            FileSnapshot {
                uid: 1000,
                gid: 999,
                mode: 0o660,
            },
            false,
            &m,
            deploy,
        );
        assert!(ok, "{reason}");
    }

    #[test]
    fn test_conflicting_identity_fails_check_even_without_items() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let json = format!(
            r#"{{"services":{{
              "a":{{"user":"111:111","volumes":[{{"type":"bind","source":"{}","target":"/x"}}]}},
              "b":{{"user":"222:222","volumes":[{{"type":"bind","source":"{}","target":"/x"}}]}}
            }}}}"#,
            shared.display(),
            shared.display()
        );
        let derived = derive_from_json(root, &json, None).unwrap();
        assert_eq!(derived.errors.len(), 1);

        let report = run_derived(root, derived, false).unwrap();
        assert!(!report.is_ok(), "身份冲突不能静默通过");
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.errors.len(), 1);
        let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(json["errors"].as_array().unwrap().len(), 1);
        assert!(report.render_text().contains("无法靠改权限解决"));
    }

    #[test]
    fn test_merge_mounts_all_read_only_stays_read_only() {
        let mut errors = Vec::new();
        let id = ContainerId { uid: 999, gid: 999 };
        let merged = merge_mounts(
            vec![
                mount(Path::new("/site/a"), true, id),
                BindMount {
                    service: "svc2".to_string(),
                    source: PathBuf::from("/site/a"),
                    read_only: true,
                    container: id,
                },
            ],
            &mut errors,
        );
        assert!(merged[0].read_only, "全部只读时保持只读");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_display_path_root_is_dot() {
        let root = Path::new("/site");
        assert_eq!(display_path(root, root), ".");
        assert_eq!(display_path(root, Path::new("/site/a/b")), "a/b");
        assert_eq!(display_path(root, Path::new("/other/a")), "/other/a");
    }

    #[test]
    fn test_run_derived_preserves_warnings() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let derived = Derived {
            mounts: Vec::new(),
            warnings: vec!["提示".to_string()],
            notes: vec!["说明".to_string()],
            errors: Vec::new(),
        };
        let report = run_derived(root, derived, false).unwrap();
        assert_eq!(report.warnings, vec!["提示".to_string()]);
        assert_eq!(report.notes, vec!["说明".to_string()]);
        let text = report.render_text();
        assert!(text.contains("[INFO] 说明"), "{text}");
        assert!(report.is_ok());
    }

    #[test]
    fn test_in_scope_sibling_prefix_not_matched() {
        let root = Path::new("/srv/site");
        assert!(!in_scope(Path::new("/srv/site2/configs"), root, &[]));
        assert!(in_scope(Path::new("/srv/site/configs"), root, &[]));
    }

    #[test]
    fn test_parse_user_numeric_and_defaults() {
        assert_eq!(
            parse_user("999:999"),
            Some(ContainerId { uid: 999, gid: 999 })
        );
        assert_eq!(
            parse_user("999"),
            Some(ContainerId { uid: 999, gid: 0 }),
            "docker 省略 group 时 gid 默认为 0"
        );
        assert_eq!(parse_user("app:app"), None);
        assert_eq!(parse_user("999:"), Some(ContainerId { uid: 999, gid: 0 }));
    }

    #[test]
    fn test_expected_modes() {
        assert_eq!(expected_mode(false, true), 0o2770);
        assert_eq!(expected_mode(false, false), 0o660);
        assert_eq!(expected_mode(true, true), 0o2750);
        assert_eq!(expected_mode(true, false), 0o640);
    }

    #[test]
    fn test_can_access_by_owner_group_other() {
        let container = ContainerId { uid: 999, gid: 999 };
        let own = FileSnapshot {
            uid: 999,
            gid: 999,
            mode: 0o440,
        };
        assert!(can_access(own, container, false, Need::Read));
        assert!(!can_access(own, container, false, Need::Write));

        let group = FileSnapshot {
            uid: 501,
            gid: 999,
            mode: 0o660,
        };
        assert!(can_access(group, container, false, Need::Write));

        let other = FileSnapshot {
            uid: 501,
            gid: 501,
            mode: 0o602,
        };
        assert!(can_access(other, container, false, Need::Write));
        assert!(!can_access(other, container, false, Need::Read));

        let dir = FileSnapshot {
            uid: 501,
            gid: 999,
            mode: 0o2770,
        };
        assert!(can_access(dir, container, true, Need::Write));
        // 目录没有执行位就不能进入
        let no_x = FileSnapshot {
            uid: 501,
            gid: 999,
            mode: 0o2660,
        };
        assert!(!can_access(no_x, container, true, Need::Read));
    }

    #[test]
    fn test_check_and_apply_align_modes_and_idempotent() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("configs");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("app.yaml");
        std::fs::write(&file, "x").unwrap();
        chmod(&root, 0o755);
        chmod(&file, 0o644);

        let id = current_id();
        let m = mount(&root, false, id);

        // 检查：属组/模式未对齐 → pending（容器 gid 与部署 gid 相同，
        // 所以直接用“部署账号 = 容器身份”的场景验证模式位）。
        let items = scan_mount(&m, deploy_identity().0).unwrap();
        let root_item = items.iter().find(|i| i.path == root).unwrap();
        assert_eq!(
            root_item.status,
            ItemStatus::Pending,
            "0755 目录必须补 setgid"
        );
        let file_item = items.iter().find(|i| i.path == file).unwrap();
        assert_eq!(
            file_item.status,
            ItemStatus::Pending,
            "0644 文件必须补组写位"
        );

        // 应用：目录 2770、文件 660
        let report = run(dir.path(), None, None, true);
        // 没有 compose 文件 → Err；这里直接走底层 apply_item 覆盖逻辑。
        assert!(report.is_err());

        let deploy = deploy_identity().0;
        for item in items.iter().filter(|i| i.status == ItemStatus::Pending) {
            apply_item(item, &m, deploy).unwrap();
        }
        let root_mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o7777;
        let file_mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(root_mode, 0o2770, "root={root_mode:04o}");
        assert_eq!(file_mode, 0o660, "file={file_mode:04o}");

        // 再检查：全部对齐；再 apply 无改动
        let items = scan_mount(&m, deploy_identity().0).unwrap();
        assert!(items.iter().all(|i| i.status == ItemStatus::Aligned));

        // 幂等：显式记录 mtime 之外的幂等性由 apply_item 的“先比较再改”保证
        let before = std::fs::metadata(&file).unwrap().permissions().mode();
        apply_item(
            &ItemReport {
                path: file.clone(),
                kind: "file",
                read_only: false,
                current: None,
                expected_mode: RW_FILE,
                expected_gid: id.gid,
                is_secret: false,
                status: ItemStatus::Pending,
                reason: String::new(),
            },
            &m,
            deploy,
        )
        .unwrap();
        let after = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(before, after);
    }

    #[test]
    fn test_read_only_never_adds_other_read() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("pkg.tar.gz");
        std::fs::write(&file, "x").unwrap();
        chmod(&file, 0o600);

        let id = current_id();
        let m = mount(root, true, id);
        let deploy = deploy_identity().0;
        let snapshot = stat_snapshot(&file).unwrap();
        let (ok, _) = aligned(snapshot, false, &m, deploy);
        assert!(!ok, "只读挂载的 600 文件对属组不可读");

        apply_item(
            &ItemReport {
                path: file.clone(),
                kind: "file",
                read_only: true,
                current: Some(snapshot),
                expected_mode: RO_FILE,
                expected_gid: id.gid,
                is_secret: false,
                status: ItemStatus::Pending,
                reason: String::new(),
            },
            &m,
            deploy,
        )
        .unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o640);
        assert_eq!(mode & 0o004, 0, "不能放宽到 other");
    }

    #[test]
    fn test_container_owned_files_are_left_alone() {
        // 无法在普通权限下 chown，这里直接用 fabricate 的快照验证判断。
        let dir = tempdir().unwrap();
        let cur = current_id();
        let container = ContainerId {
            uid: cur.uid.wrapping_add(1),
            gid: cur.gid,
        };
        let m = mount(dir.path(), false, container);
        let snapshot = FileSnapshot {
            uid: container.uid,
            gid: container.gid,
            mode: 0o644,
        };
        let (ok, _) = aligned(snapshot, false, &m, deploy_identity().0);
        assert!(ok, "owner == 容器 uid 时必须原样放过");
    }

    #[test]
    fn test_resolve_secret_uses_sec_path_validation() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".wist")).unwrap();
        let file = root.join(".wist/sec-value.yml");
        std::fs::write(&file, "k: \"v\"\n").unwrap();

        let conf: SysConf = serde_yaml::from_str(
            "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: .wist/sec-value.yml\n  mode: \"0640\"\n",
        )
        .unwrap();
        let (path, mode) = resolve_secret(root, Some(&conf)).unwrap().expect("secret");
        assert_eq!(mode, 0o640);
        assert_eq!(path, canonical_or_parent(&file));

        // 绝对路径 / `..` / 符号链接都必须拒绝（与 sec 的路径校验一致）。
        for declared in ["/etc/passwd", "../outside.yml"] {
            let conf: SysConf = serde_yaml::from_str(&format!(
                "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: {declared}\n"
            ))
            .unwrap();
            let err = resolve_secret(root, Some(&conf)).unwrap_err();
            assert!(
                err.to_string().contains("相对系统根")
                    || err.to_string().contains("普通的相对路径段"),
                "declared={declared}, err={err}"
            );
        }

        // 合法声明但文件缺失：允许（perms 不负责创建密钥）。
        let conf: SysConf = serde_yaml::from_str(
            "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: .wist/missing.yml\n",
        )
        .unwrap();
        let (path, mode) = resolve_secret(root, Some(&conf)).unwrap().expect("secret");
        assert_eq!(mode, 0o600);
        assert!(path.ends_with(".wist/missing.yml"));

        // 无 secrets 声明 → None。
        let plain: SysConf =
            serde_yaml::from_str("test_envs:\n  dep_root: ''\n  deps: []\n").unwrap();
        assert!(resolve_secret(root, Some(&plain)).unwrap().is_none());
    }

    #[test]
    fn test_run_with_conf_rejects_absolute_secret_before_docker() {
        // secrets.file 绝对路径必须在派生之前就被拒绝，否则可能把 chmod 作用到根外。
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: /etc/passwd\n",
        )
        .unwrap();
        let err = run_with_conf(dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("相对系统根"), "{err}");
    }

    #[test]
    fn test_run_with_conf_rejects_symlinked_secret() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.yml");
        std::fs::write(&real, "k: \"v\"\n").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link.yml")).unwrap();
        std::fs::write(
            dir.path().join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: link.yml\n",
        )
        .unwrap();
        let err = run_with_conf(dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("符号链接"), "{err}");
    }

    #[test]
    fn test_secret_file_expects_deploy_owner_and_configured_mode() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".wist")).unwrap();
        let secret = root.join(".wist/sec-value.yml");
        std::fs::write(&secret, "a: b\n").unwrap();
        chmod(&secret, 0o644);

        let id = current_id();
        let src = canonical_or_parent(&root.join(".wist"));
        let m = mount(&src, true, id);
        let deploy = deploy_identity().0;
        let secret_path = canonical_or_parent(&secret);
        let target = SecretTarget {
            path: &secret_path,
            mode: 0o640,
        };
        let items = scan_mount_with_secret(&m, deploy, Some(target)).unwrap();

        let item = items
            .iter()
            .find(|i| i.path == secret_path)
            .expect("secret item");
        assert!(item.is_secret);
        assert_eq!(item.expected_mode, 0o640, "期望模式来自 secrets.mode");
        assert_eq!(
            item.expected_gid, id.gid,
            "属组 = 容器 gid，供容器按 group 读"
        );
        assert_eq!(item.status, ItemStatus::Pending, "0644 必须待修");
    }

    #[test]
    fn test_aligned_secret_uses_deploy_owner_and_configured_mode() {
        let m = mount(
            Path::new("/site/.wist"),
            true,
            ContainerId { uid: 999, gid: 999 },
        );
        let deploy = 1000;

        // 组共享：部署账号属主 + 容器 gid + 0640
        let (ok, _) = aligned_with_secret(
            FileSnapshot {
                uid: 1000,
                gid: 999,
                mode: 0o640,
            },
            false,
            &m,
            deploy,
            Some(0o640),
        );
        assert!(ok, "部署账号:容器gid 0640 是组共享的正确状态");

        // 严格模式 0600 也可以对齐（是否能让容器读到由 derive 的 warning 提示）
        let (ok, _) = aligned_with_secret(
            FileSnapshot {
                uid: 1000,
                gid: 999,
                mode: 0o600,
            },
            false,
            &m,
            deploy,
            Some(0o600),
        );
        assert!(ok);

        // 属主不是部署账号（试图把密钥 chown 给容器 uid）→ 不允许
        let (ok, reason) = aligned_with_secret(
            FileSnapshot {
                uid: 999,
                gid: 999,
                mode: 0o640,
            },
            false,
            &m,
            deploy,
            Some(0o640),
        );
        assert!(!ok, "密钥属主必须是部署账号");
        assert!(reason.contains("属主"), "{reason}");

        // 模式与 secrets.mode 不符
        let (ok, reason) = aligned_with_secret(
            FileSnapshot {
                uid: 1000,
                gid: 999,
                mode: 0o644,
            },
            false,
            &m,
            deploy,
            Some(0o640),
        );
        assert!(!ok);
        assert!(reason.contains("模式"), "{reason}");
    }

    #[test]
    fn test_derive_fails_when_0600_secret_is_mounted_by_nonroot_container() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"center":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/secrets","read_only":true}}]}}}}}}"#,
            root.join(".wist").display()
        );
        let secret_path = canonical_or_parent(&root.join(".wist/sec-value.yml"));
        let target = SecretTarget {
            path: &secret_path,
            mode: 0o600,
        };
        let derived = derive_from_json_with_secret(root, &json, None, Some(target)).unwrap();
        assert!(
            derived
                .errors
                .iter()
                .any(|w| w.contains("secrets.mode = 0600")),
            "{:?}",
            derived.errors
        );
    }

    #[test]
    fn test_apply_never_creates_missing_secret_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("sec-value.yml");
        let id = current_id();
        let m = mount(dir.path(), true, id);
        let err = apply_item_for(
            &ItemReport {
                path: missing.clone(),
                kind: "file",
                read_only: true,
                current: None,
                expected_mode: 0o640,
                expected_gid: id.gid,
                is_secret: true,
                status: ItemStatus::Pending,
                reason: String::new(),
            },
            &m,
            deploy_identity().0,
        )
        .unwrap_err();
        assert!(err.contains("不创建密钥"), "{err}");
        assert!(!missing.exists(), "不能凭空造出密钥文件/目录");
    }

    #[test]
    fn test_missing_container_group_reports_once_and_skips_apply() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let configs = root.join("configs");
        std::fs::create_dir_all(&configs).unwrap();
        let file = configs.join("app.conf");
        std::fs::write(&file, "x").unwrap();
        chmod(&configs, 0o755);
        chmod(&file, 0o644);

        // 容器 gid 用一个进程肯定不在的组，模拟「部署账号没加进容器组」。
        let derived = Derived {
            mounts: vec![mount(
                &configs,
                false,
                ContainerId {
                    uid: 9999,
                    gid: 9999,
                },
            )],
            warnings: vec![],
            notes: vec![],
            errors: vec![],
        };
        let report = run_derived_with_groups(root, derived, None, true, false, &[4242]).unwrap();

        // 一次说清：只有一条 error，不展开逐项，也不动盘。
        assert_eq!(report.errors.len(), 1, "{}", report.render_text());
        assert!(report.errors[0].contains("9999"), "{}", report.errors[0]);
        assert!(
            report.errors[0].contains("usermod -aG"),
            "{}",
            report.errors[0]
        );
        assert_eq!(report.applied(), 0);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o644,
            "组关系缺失时 --apply 不能改盘"
        );
        assert_eq!(report.mounts[0].blocked_by_group, Some(9999));

        let text = report.render_text();
        assert!(text.contains("[BLOCKED]"), "{text}");
        assert!(
            !text.contains("app.conf"),
            "逐项明细应收起（否则就是 136 行同一个 EPERM）：{text}"
        );
        let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(json["mounts"][0]["blocked_by_group"], 9999);
    }

    #[test]
    fn test_aligned_mount_does_not_require_group_membership() {
        // 反向保护：文件本来就对齐时，缺组成员关系不应报错（否则已经对齐的现场会被误拦）。
        let dir = tempdir().unwrap();
        let root = dir.path();
        let configs = root.join("configs");
        std::fs::create_dir_all(&configs).unwrap();
        let file = configs.join("app.conf");
        std::fs::write(&file, "x").unwrap();
        chmod(&configs, 0o2770);
        chmod(&file, 0o660);

        let derived = Derived {
            mounts: vec![mount(&configs, false, current_id())],
            warnings: vec![],
            notes: vec![],
            errors: vec![],
        };
        // 故意给一个不含该 gid 的组集合：只要有项要改才会拦。
        let report = run_derived_with_groups(root, derived, None, false, false, &[4242]).unwrap();
        assert!(report.errors.is_empty(), "{}", report.render_text());
        assert_eq!(report.mounts[0].blocked_by_group, None);
        assert!(report.is_ok(), "{}", report.render_text());
    }

    #[test]
    fn test_privileged_apply_does_not_require_group_membership() {
        // root / sudo 能 chgrp 到任意 gid：不报「宿主预置缺失」（是否能改盘由内核判）。
        let dir = tempdir().unwrap();
        let root = dir.path();
        let configs = root.join("configs");
        std::fs::create_dir_all(&configs).unwrap();
        chmod(&configs, 0o755);

        let derived = Derived {
            mounts: vec![mount(
                &configs,
                false,
                ContainerId {
                    uid: 9999,
                    gid: 9999,
                },
            )],
            warnings: vec![],
            notes: vec![],
            errors: vec![],
        };
        let report = run_derived_with_groups(root, derived, None, false, true, &[4242]).unwrap();
        assert!(report.errors.is_empty(), "{}", report.render_text());
        assert_eq!(report.mounts[0].blocked_by_group, None);
    }

    #[test]
    fn test_apply_is_skipped_when_config_errors_exist() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("configs/app.conf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
        chmod(&file, 0o644);

        let id = current_id();
        let derived = Derived {
            mounts: vec![mount(&root.join("configs"), false, id)],
            warnings: vec![],
            notes: vec![],
            errors: vec!["配置冲突".to_string()],
        };
        let report = run_derived_with_secret(root, derived, None, true).unwrap();
        assert!(!report.is_ok());
        assert_eq!(report.applied(), 0, "配置错误时不应动盘");
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o644, "配置错误时 --apply 不能改权限");
    }

    #[test]
    fn test_secret_apply_keeps_deploy_owner_and_sets_mode() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempdir().unwrap();
        let secret = dir.path().join("sec-value.yml");
        std::fs::write(&secret, "a: b\n").unwrap();
        chmod(&secret, 0o644);

        let (deploy_uid, deploy_gid) = deploy_identity();
        // 容器 uid 故意与部署账号不同：apply 不能把属主改成容器 uid。
        let container = ContainerId {
            uid: deploy_uid.wrapping_add(1),
            gid: deploy_gid,
        };
        let m = mount(&secret, true, container);
        let item = ItemReport {
            path: secret.clone(),
            kind: "file",
            read_only: true,
            current: None,
            expected_mode: 0o640,
            expected_gid: container.gid,
            is_secret: true,
            status: ItemStatus::Pending,
            reason: String::new(),
        };
        apply_item_for(&item, &m, deploy_uid).unwrap();

        let meta = std::fs::metadata(&secret).unwrap();
        assert_eq!(meta.uid(), deploy_uid, "属主必须是部署账号，不能是容器 uid");
        assert_eq!(meta.mode() & 0o7777, 0o640);
    }

    #[test]
    fn test_aligned_secret_checks_group() {
        let m = mount(
            Path::new("/site/.wist"),
            true,
            ContainerId { uid: 999, gid: 999 },
        );
        let (ok, reason) = aligned_with_secret(
            FileSnapshot {
                uid: 1000,
                gid: 1000,
                mode: 0o640,
            },
            false,
            &m,
            1000,
            Some(0o640),
        );
        assert!(!ok, "属组必须是容器 gid，容器才能按 group 读");
        assert!(reason.contains("属组"), "{reason}");
    }

    #[test]
    fn test_0600_secret_with_root_only_mount_has_no_error() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"web":{{"user":"0:0","volumes":[{{"type":"bind","source":"{}","target":"/secrets","read_only":true}}]}}}}}}"#,
            root.join(".wist").display()
        );
        let secret_path = canonical_or_parent(&root.join(".wist/sec-value.yml"));
        let target = SecretTarget {
            path: &secret_path,
            mode: 0o600,
        };
        let derived = derive_from_json_with_secret(root, &json, None, Some(target)).unwrap();
        assert!(derived.mounts.is_empty(), "root 服务应被跳过");
        assert!(derived.errors.is_empty(), "{:?}", derived.errors);
    }

    #[test]
    fn test_0640_secret_with_nonroot_mount_has_no_error() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), "").unwrap();
        let json = format!(
            r#"{{"services":{{"center":{{"user":"999:999","volumes":[{{"type":"bind","source":"{}","target":"/secrets","read_only":true}}]}}}}}}"#,
            root.join(".wist").display()
        );
        let secret_path = canonical_or_parent(&root.join(".wist/sec-value.yml"));
        let target = SecretTarget {
            path: &secret_path,
            mode: 0o640,
        };
        let derived = derive_from_json_with_secret(root, &json, None, Some(target)).unwrap();
        assert!(derived.errors.is_empty(), "{:?}", derived.errors);
        assert_eq!(derived.mounts.len(), 1);
    }

    #[test]
    fn test_missing_mount_source_is_pending_and_apply_creates_it() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("missing/nested");
        let id = current_id();
        let m = mount(&source, false, id);

        let items = scan_mount(&m, deploy_identity().0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, ItemStatus::Pending);

        let deploy = deploy_identity().0;
        apply_item(&items[0], &m, deploy).unwrap();
        assert!(source.is_dir());
        let mode = std::fs::metadata(&source).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o2770);
    }

    #[test]
    fn test_merge_mounts_prefers_read_write_and_keeps_first_identity() {
        let id = ContainerId { uid: 999, gid: 999 };
        let other = ContainerId {
            uid: 1000,
            gid: 1000,
        };
        let mut errors = Vec::new();
        let merged = merge_mounts(
            vec![
                mount(Path::new("/site/a"), true, id),
                BindMount {
                    service: "svc2".to_string(),
                    source: PathBuf::from("/site/a"),
                    read_only: false,
                    container: id,
                },
                BindMount {
                    service: "svc3".to_string(),
                    source: PathBuf::from("/site/b"),
                    read_only: true,
                    container: other,
                },
            ],
            &mut errors,
        );
        assert_eq!(merged.len(), 2);
        let a = merged
            .iter()
            .find(|m| m.source == Path::new("/site/a"))
            .unwrap();
        assert!(!a.read_only, "任一服务读写 → 按读写对齐");
        assert_eq!(a.service, "svc,svc2");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_merge_mounts_errors_on_conflicting_identity() {
        let mut errors = Vec::new();
        let merged = merge_mounts(
            vec![
                mount(Path::new("/site/a"), false, ContainerId { uid: 1, gid: 1 }),
                mount(Path::new("/site/a"), false, ContainerId { uid: 2, gid: 2 }),
            ],
            &mut errors,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(errors.len(), 1, "身份冲突必须判失败，不能静默放过");
        assert!(errors[0].contains("无法用单一属组满足"));
    }

    #[test]
    fn test_canonical_or_parent_handles_missing_tail() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("a/b/c");
        let canonical = canonical_or_parent(&missing);
        let base = std::fs::canonicalize(dir.path()).unwrap();
        assert!(canonical.starts_with(&base), "canonical={canonical:?}");
        assert!(canonical.ends_with("a/b/c"));
    }

    #[test]
    fn test_in_scope_requires_system_root_or_explicit_allow() {
        let root = Path::new("/site");
        assert!(in_scope(Path::new("/site/configs"), root, &[]));
        assert!(!in_scope(Path::new("/other/configs"), root, &[]));
        assert!(in_scope(
            Path::new("/other/configs"),
            root,
            &[PathBuf::from("/other")]
        ));
    }
}
