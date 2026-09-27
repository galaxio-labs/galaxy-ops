use super::prelude::*;
use std::path::{Path, PathBuf};

use crate::error::MainReason;
use crate::module::ModelSTD;
use crate::module::refs::ModuleSpecRef;
use crate::system::SysKind;
use crate::system::spec::SysModelSpec;
use sha2::{Digest, Sha256};

/// 交付锁文件名（位于系统根目录，随交付包一起分发）。
pub const DELIVER_LOCK_FILE: &str = "deliver.lock";
/// 锁文件格式版本。
const LOCKFILE_VERSION: u32 = 1;

/// 锁定的模块引用（记录被交付系统实际引用的模块与来源）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockedModule {
    name: String,
    model: ModelSTD,
    enable: bool,
    addr: Address,
}

impl LockedModule {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn model(&self) -> &ModelSTD {
        &self.model
    }
    pub fn enable(&self) -> bool {
        self.enable
    }
    pub fn addr(&self) -> &Address {
        &self.addr
    }
    fn from_ref(m: &ModuleSpecRef) -> Self {
        Self {
            name: m.name().clone(),
            model: m.model().clone(),
            enable: m.is_enable(),
            addr: m.addr().clone(),
        }
    }
}

/// 值指纹：回答"这份交付用了什么值"。输入缺失时记为 `None`。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ValueHashes {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    merged_vars: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    values: Option<String>,
}

impl ValueHashes {
    pub fn merged_vars(&self) -> Option<&str> {
        self.merged_vars.as_deref()
    }
    pub fn values(&self) -> Option<&str> {
        self.values.as_deref()
    }
}

/// 交付锁：记录"部署的是哪一版、引用了哪些模块、用了什么值"，为复现与回滚留依据。
///
/// 由 `gops sys package` 生成，落盘于系统根目录 `deliver.lock`，随交付包分发。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliverLock {
    lockfile_version: u32,
    name: String,
    version: String,
    kind: SysKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    model: Option<ModelSTD>,
    generated_at: String,
    modules: Vec<LockedModule>,
    hashes: ValueHashes,
}

impl DeliverLock {
    /// 依据系统根目录生成交付锁。
    pub fn build(root: &Path) -> MainResult<Self> {
        let sys_dir = root.join("sys");
        let spec = SysModelSpec::load_from(&sys_dir)?;
        let define = spec.define();
        let modules = spec
            .mod_list()
            .mods()
            .iter()
            .map(LockedModule::from_ref)
            .collect();
        let hashes = ValueHashes {
            merged_vars: hash_file(&sys_dir.join(crate::const_vars::MERGED_VARS_YML))?,
            values: hash_tree(&root.join(crate::const_vars::VALUE_DIR))?,
        };
        Ok(Self {
            lockfile_version: LOCKFILE_VERSION,
            name: define.name().clone(),
            version: read_sys_version(root),
            kind: *define.kind(),
            model: define.model().clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            modules,
            hashes,
        })
    }

    pub fn lockfile_version(&self) -> u32 {
        self.lockfile_version
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn kind(&self) -> SysKind {
        self.kind
    }
    pub fn model(&self) -> Option<&ModelSTD> {
        self.model.as_ref()
    }
    pub fn generated_at(&self) -> &str {
        &self.generated_at
    }
    pub fn modules(&self) -> &[LockedModule] {
        &self.modules
    }
    pub fn hashes(&self) -> &ValueHashes {
        &self.hashes
    }

    /// `<root>/deliver.lock` 路径。
    pub fn path(root: &Path) -> PathBuf {
        root.join(DELIVER_LOCK_FILE)
    }

    /// 写出 `<root>/deliver.lock`（YAML）。
    pub fn save(&self, root: &Path) -> MainResult<()> {
        let path = Self::path(root);
        let yaml = serde_yaml::to_string(self)
            .map_err(|e| MainReason::conf_detail(format!("serialize deliver.lock: {e}")))?;
        std::fs::write(&path, yaml).source_resource().with(&path)?;
        Ok(())
    }

    /// 读取 `<root>/deliver.lock`。
    pub fn load(root: &Path) -> MainResult<Self> {
        let path = Self::path(root);
        let text = std::fs::read_to_string(&path)
            .source_resource()
            .with(&path)?;
        serde_yaml::from_str(&text)
            .map_err(|e| MainReason::conf_detail(format!("parse deliver.lock: {e}")))
    }

    /// 重新生成并覆盖 `<root>/deliver.lock`。
    pub fn refresh(root: &Path) -> MainResult<Self> {
        let lock = Self::build(root)?;
        lock.save(root)?;
        Ok(lock)
    }
}

/// 读取系统版本（`<root>/version.txt`，缺失或为空时回退 `0.1.0`）。
fn read_sys_version(root: &Path) -> String {
    std::fs::read_to_string(root.join("version.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.1.0".to_string())
}

fn format_digest(hasher: Sha256) -> String {
    let out = hasher.finalize();
    let mut s = String::with_capacity(7 + out.len() * 2);
    s.push_str("sha256:");
    for b in out.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format_digest(hasher)
}

/// 单文件指纹；文件不存在时返回 `None`。
fn hash_file(path: &Path) -> MainResult<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).source_resource().with(path)?;
    Ok(Some(hash_bytes(&bytes)))
}

/// 目录指纹：对目录内所有文件按相对路径排序后，逐条混入（路径 + 内容），
/// 保证与遍历顺序无关、可跨机复现。目录不存在时返回 `None`。
fn hash_tree(dir: &Path) -> MainResult<Option<String>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry
            .map_err(|e| MainReason::resource_detail(format!("walk {}: {e}", dir.display())))?;
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    files.sort();

    let mut hasher = Sha256::new();
    for file in &files {
        let rel = file.strip_prefix(dir).unwrap_or(file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = std::fs::read(file).source_resource().with(file)?;
        hasher.update(&bytes);
        hasher.update([0u8]);
    }
    Ok(Some(format_digest(hasher)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{CpuArch, ModelSTD, OsCPE, RunSPC};
    use crate::system::spec::SysDefine;
    use crate::system::spec::SysModelSpec;
    use orion_error::dev::testing::TestAssert;
    use tempfile::TempDir;

    fn write_sys(root: &Path, model: Option<ModelSTD>) {
        let define = match model {
            Some(m) => SysDefine::new("demo", m),
            None => SysDefine::new_without_model("demo"),
        };
        let spec = SysModelSpec::make_new(define).assert();
        spec.save_local(root, "sys").assert();
    }

    #[test]
    fn test_build_records_identity_modules_and_hashes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_sys(root, Some(ModelSTD::x86_ubt22_k8s()));
        std::fs::write(root.join("version.txt"), "2.3.4\n").unwrap();
        std::fs::write(root.join("sys/merged_vars.yml"), "system: []\n").unwrap();
        std::fs::create_dir_all(root.join("values/web")).unwrap();
        std::fs::write(root.join("values/web/sys_value.yml"), "a: 1\n").unwrap();

        let lock = DeliverLock::build(root).assert();
        assert_eq!(lock.lockfile_version(), 1);
        assert_eq!(lock.name(), "demo");
        assert_eq!(lock.version(), "2.3.4");
        assert_eq!(lock.kind(), SysKind::Gxl);
        assert_eq!(lock.model(), Some(&ModelSTD::x86_ubt22_k8s()));
        // make_new 默认注入 3 个模块引用
        assert_eq!(lock.modules().len(), 3);
        assert!(lock.hashes().merged_vars().is_some());
        assert!(lock.hashes().values().is_some());
        assert!(lock.hashes().merged_vars().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn test_save_then_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_sys(root, Some(ModelSTD::arm_mac14_host()));

        let lock = DeliverLock::build(root).assert();
        lock.save(root).assert();
        assert!(DeliverLock::path(root).exists());

        let loaded = DeliverLock::load(root).assert();
        assert_eq!(
            serde_yaml::to_string(&loaded).unwrap(),
            serde_yaml::to_string(&lock).unwrap()
        );
    }

    #[test]
    fn test_compose_system_has_no_model_and_no_values_hash() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_sys(root, None);

        let lock = DeliverLock::build(root).assert();
        assert!(lock.model().is_none());
        // 无 merged_vars.yml / values/ 时指纹为 None（而非报错）
        assert!(lock.hashes().merged_vars().is_none());
        assert!(lock.hashes().values().is_none());
    }

    #[test]
    fn test_hash_tree_is_order_independent_and_content_sensitive() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        for base in [a.path(), b.path()] {
            std::fs::create_dir_all(base.join("sub")).unwrap();
            std::fs::write(base.join("z.txt"), "z").unwrap();
            std::fs::write(base.join("sub/a.txt"), "a").unwrap();
        }
        assert_eq!(hash_tree(a.path()).unwrap(), hash_tree(b.path()).unwrap());

        // 内容变化 -> 指纹变化
        std::fs::write(b.path().join("sub/a.txt"), "a2").unwrap();
        assert_ne!(hash_tree(a.path()).unwrap(), hash_tree(b.path()).unwrap());

        // 缺目录 -> None
        assert_eq!(hash_tree(&a.path().join("nope")).unwrap(), None);
    }

    #[test]
    fn test_hash_bytes_differs_by_input() {
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"b"));
        assert_eq!(hash_bytes(b"a"), hash_bytes(b"a"));
    }

    #[test]
    fn test_refresh_overwrites_lock() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_sys(
            root,
            Some(ModelSTD::new(CpuArch::X86, OsCPE::UBT22, RunSPC::Host)),
        );
        std::fs::write(DeliverLock::path(root), "stale: true\n").unwrap();

        let lock = DeliverLock::refresh(root).assert();
        let reloaded = DeliverLock::load(root).assert();
        assert_eq!(reloaded.name(), lock.name());
        assert_eq!(reloaded.name(), "demo");
    }
}
