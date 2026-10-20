//! 密钥文件的安全加载（`sys-prj.yml: secrets`）。
//!
//! 背景：`orion_sec::load_secfile_by()` 只判断文件存在，没有任何边界约束。一旦允许
//! “清单指路”，清单（包控文件）就能把 gops 变成任意 YAML 的读取/打印器。因此这里把
//! 安全约束集中在一处，所有消费方（`gops run` / `gops sec`）都只走本模块。
//!
//! 安全基线：
//! 1. **路径限定**：必须是系统根内的相对路径，不含 `.` / `..` / 根，且 canonicalize 后
//!    仍等于 `系统根 + 相对路径`（等价于“路径上没有任何符号链接”）；
//! 2. **缺失即失败**：清单声明了文件就必须存在，**不**回退到 `.galaxy` 等其它文件
//!    （静默回退会把 A 栈的密钥注入 B 栈）；
//! 3. **权限 fail-closed**：只接受属主可读的文件（`0600` / `0400` 等），属主必须是
//!    当前进程的 euid（root 运行时跳过属主判断），且 **不允许硬链接**（`nlink > 1`）；
//! 4. **只接受标量字符串**：YAML 会把 `123456` 解析成数字、`0123` 丢前导零、`1e5` 变浮点，
//!    嵌套结构会变成字面量 `obj...`——都拒绝，要求文件里加引号；
//! 5. **不泄露明文**：错误与 `Debug` 输出都不带值；解析错误只保留位置信息。
use super::prelude::*;
use crate::error::MainReason;
use crate::system::conf::{ExposeConf, ExposeRule, SecFileFormat, SecretsConf, Subscriber};
use orion_sec::sec::SecValueType;
use orion_vars::vars::{UpperKey, ValueType};

/// 与 `orion-sec` 的 `SECRET_MASK` 保持一致。
pub const SECRET_MASK: &str = "********";

/// 密钥文件大小上限：正常密钥文件是 KB 级，超过即视为异常（防 YAML 炸弹 / 误指大文件）。
const MAX_SEC_FILE_BYTES: u64 = 1024 * 1024;

/// 已校验、已加载的密钥集合。
///
/// `Debug` 是手写的：`EnvDict` 里的 `ValueType` 没有掩码能力，派生 `Debug` 会把明文
/// 打出来（`orion-sec` 的 `SecValue` 也只有 `Display` 掩码）。
#[derive(Clone)]
pub struct SecFile {
    path: PathBuf,
    dict: EnvDict,
    /// 来自 `sys-prj.yml: secrets.expose`；`None` = 各消费方用自己的缺省策略。
    expose: Option<ExposeConf>,
}

impl std::fmt::Debug for SecFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecFile")
            .field("path", &self.path)
            .field("keys", &self.keys().map(|k| k.as_str()).collect::<Vec<_>>())
            .finish()
    }
}

impl SecFile {
    fn new(path: PathBuf, dict: EnvDict, expose: Option<ExposeConf>) -> Self {
        Self { path, dict, expose }
    }

    /// 空集合：未声明密钥文件、且 `.galaxy` 旧约定文件也不存在时的正常状态。
    pub fn empty(path: PathBuf) -> Self {
        Self::new(path, EnvDict::new(), None)
    }

    /// 从现成的键值对构造（主要用于测试与上层注入场景）。
    pub fn from_pairs(path: impl Into<PathBuf>, pairs: Vec<(String, String)>) -> Self {
        let mut dict = EnvDict::new();
        for (key, value) in pairs {
            dict.insert(normalize_lookup_key(&key), ValueType::String(value));
        }
        Self::new(path.into(), dict, None)
    }

    /// 按消费方裁剪后的视图：落实最小权限（`secrets.expose`）。
    ///
    /// - `all`（或缺省策略为 all）→ 原样返回；
    /// - 模式列表 → 只保留命中的键；**一条模式都没命中**时打警告（键名漂移的哨兵），
    ///   但不报错（允许现场先删除某个键）。
    pub fn scoped(&self, subscriber: Subscriber) -> MainResult<SecFile> {
        let rule = self
            .expose
            .as_ref()
            .map(|conf| conf.rule_for(subscriber))
            .unwrap_or_else(|| subscriber.default_rule());

        let patterns = match rule {
            ExposeRule::All => return Ok(self.clone()),
            ExposeRule::Only(patterns) => patterns,
        };
        if patterns.is_empty() {
            return Ok(SecFile::new(
                self.path.clone(),
                EnvDict::new(),
                self.expose.clone(),
            ));
        }
        // 空模式几乎一定是写错了（`- ""`），且在通配语义下没有意义 —— fail-closed。
        if let Some(blank) = patterns.iter().find(|p| p.trim().is_empty()) {
            return Err(MainReason::logic_detail(format!(
                "secrets.expose.{}: 存在空模式 `{blank}`（键名模式不能为空）",
                subscriber.as_str()
            )));
        }

        let patterns: Vec<String> = patterns.iter().map(|p| p.to_uppercase()).collect();
        let mut matched = vec![false; patterns.len()];
        let mut dict = EnvDict::new();
        for (key, value) in self.dict.iter() {
            let name = key.as_str();
            for (idx, pattern) in patterns.iter().enumerate() {
                if glob_star(pattern, name) {
                    matched[idx] = true;
                    dict.insert(key.clone(), value.clone());
                }
            }
        }
        for (idx, hit) in matched.iter().enumerate() {
            if !hit {
                // 不走 `warn!`：默认日志级别下看不到，而“模式零命中”正是键名漂移的哨兵。
                eprintln!(
                    "[WARN] secrets.expose.{}: 模式 `{}` 没有匹配任何密钥（键名已改？）",
                    subscriber.as_str(),
                    patterns[idx]
                );
            }
        }
        Ok(SecFile::new(self.path.clone(), dict, self.expose.clone()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.dict.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dict.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &UpperKey> {
        self.dict.keys()
    }

    /// 按键名取值（大小写不敏感：`center_admin_token` 与 `SEC_CENTER_ADMIN_TOKEN` 等价）。
    pub fn get(&self, key: &str) -> Option<&ValueType> {
        let name = normalize_lookup_key(key);
        self.dict.get(&name)
    }

    /// 注入子进程用的 `(NAME, VALUE)` 对。明文**只**在这里出现。
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.dict
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_string()))
            .collect()
    }

    /// `(键名, 值长度)`：用于 `list` 输出，只暴露长度不暴露值。
    pub fn entries(&self) -> Vec<(String, usize)> {
        self.dict
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_string().chars().count()))
            .collect()
    }

    /// 去掉 `SEC_` 前缀并转小写：`SEC_WP_GIT_PUSH_TOKEN` → `wp_git_push_token`。
    pub fn raw_name(env_name: &str) -> String {
        env_name
            .strip_prefix("SEC_")
            .unwrap_or(env_name)
            .to_lowercase()
    }
}

/// 把任意写法归一到 `SEC_` 前缀 + 大写（与 `orion-sec` 的 `normalize_sec_key` 一致）。
fn normalize_lookup_key(key: &str) -> UpperKey {
    let upper = key.to_uppercase();
    if upper.starts_with("SEC_") {
        UpperKey::from(upper)
    } else {
        UpperKey::from(format!("SEC_{upper}"))
    }
}

/// 按当前系统根加载密钥：`sys-prj.yml: secrets` 优先；未声明时回退到 `.galaxy` 旧约定。
///
/// 声明了但文件缺失 → **硬失败**（见模块文档第 2 条）。
pub fn load_for_system(sys_root: &Path) -> MainResult<SecFile> {
    match declared_conf(sys_root)? {
        Some(conf) => load_declared(sys_root, &conf),
        None => load_galaxy_fallback(sys_root),
    }
}

/// 按显式声明加载（路径校验 + 权限校验 + 类型校验）。
pub fn load_declared(sys_root: &Path, conf: &SecretsConf) -> MainResult<SecFile> {
    let path = resolve_declared_path(sys_root, conf.file())?;
    if !path.exists() {
        return Err(MainReason::logic_detail(format!(
            "sys-prj.yml 声明了 secrets.file = `{}`，但文件不存在（{}）；\
             声明了就必须存在，gops 不会回退到其它密钥文件",
            conf.file(),
            path.display()
        )));
    }
    let mode = conf.file_mode().map_err(MainReason::logic_detail)?;
    load_checked_ex(&path, conf.format(), conf.expose().cloned(), mode)
}

/// 路径校验：相对、无 `.`/`..`、无符号链接、结果仍在系统根内。
fn resolve_declared_path(sys_root: &Path, declared: &str) -> MainResult<PathBuf> {
    if declared.trim().is_empty() {
        return Err(MainReason::logic_detail(
            "sys-prj.yml 的 secrets.file 为空；要么删除 secrets 节，要么给一个相对路径",
        ));
    }
    let rel = Path::new(declared);
    if rel.is_absolute() {
        return Err(MainReason::logic_detail(format!(
            "secrets.file 必须是**相对系统根**的路径，不接受绝对路径：`{declared}`"
        )));
    }
    for comp in rel.components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(MainReason::logic_detail(format!(
                "secrets.file 只接受普通的相对路径段（不允许 . / .. / 盘符）：`{declared}`"
            )));
        }
    }

    let root_canon = fs::canonicalize(sys_root).source_resource()?;
    let joined = root_canon.join(rel);
    let canon = match fs::canonicalize(&joined) {
        Ok(path) => path,
        Err(_) => {
            // 悬空符号链接：`canonicalize` 返回 NotFound，但链接本体是存在的。
            // 这里必须说清楚，否则现场只会看到“文件不存在”。
            let dangling = fs::symlink_metadata(&joined)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if dangling {
                return Err(MainReason::logic_detail(format!(
                    "secrets.file 是悬空的符号链接（不允许）：`{declared}`"
                )));
            }
            // 真不存在：交给调用方给出“声明了就必须存在”的明确错误。
            return Ok(joined);
        }
    };

    if !canon.starts_with(&root_canon) {
        return Err(MainReason::logic_detail(format!(
            "secrets.file 解析后落在系统根之外：`{declared}` → {}",
            canon.display()
        )));
    }
    // canonicalize 会把符号链接展开；展开后偏离“系统根 + 相对路径”即说明路径中有链接。
    if canon != joined {
        return Err(MainReason::logic_detail(format!(
            "secrets.file 路径中含符号链接，或大小写与磁盘不一致（均不允许）：`{declared}` → {}",
            canon.display()
        )));
    }
    let meta = fs::symlink_metadata(&joined).source_resource()?;
    if !meta.file_type().is_file() {
        return Err(MainReason::logic_detail(format!(
            "secrets.file 必须是普通文件：`{declared}`"
        )));
    }
    Ok(joined)
}

/// 文件权限、属主与链接数校验（fail-closed）。
///
/// `mode` 是声明策略：`0600`（只属主可读）或 `0640`（属组可读，用于与固定 gid 的
/// 容器共享）；两种策略都要求属主是部署账号（或 euid 为 root）。
#[cfg(unix)]
fn check_file_security(path: &Path, mode: u32) -> MainResult<()> {
    use std::os::unix::fs::MetadataExt;

    let meta = fs::symlink_metadata(path).source_resource()?;
    let actual = meta.mode() & 0o777;
    let allowed = match mode {
        0o600 => actual == 0o600 || actual == 0o400,
        0o640 => actual == 0o640 || actual == 0o440,
        _ => false,
    };
    if !allowed {
        let (expected, why) = if mode == 0o640 {
            ("0640 / 0440", "属组 = 容器 gid，供容器读取")
        } else {
            ("0600 / 0400", "只属主可读")
        };
        return Err(MainReason::logic_detail(format!(
            "密钥文件 {} 权限是 {:o}，与 secrets.mode 不符：需要 {expected}（{why}）；\n\
             共享给容器请用 `secrets.mode: \"0640\"` + 属组 = 容器 gid，不要放宽到 other",
            path.display(),
            actual
        )));
    }
    let euid = unsafe { libc::geteuid() };
    if euid != 0 && meta.uid() != euid {
        return Err(MainReason::logic_detail(format!(
            "密钥文件 {} 属主 uid={} 与当前进程 euid={} 不一致；请用部署账号持有该文件",
            path.display(),
            meta.uid(),
            euid
        )));
    }
    // 硬链接看不到路径，却能把根外文件“链”进系统根，或让同一份恢复点被多处引用。
    // 密钥文件应当是单链接的普通文件；`cp` 可以，`ln` 不行。
    if meta.nlink() > 1 {
        return Err(MainReason::logic_detail(format!(
            "密钥文件 {} 有 {} 个硬链接：密钥文件不允许被硬链接\
             （可能是指到系统根外，或是硬链接式备份）；请用普通复制（cp）代替 ln",
            path.display(),
            meta.nlink()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_file_security(_path: &Path, _mode: u32) -> MainResult<()> {
    Ok(())
}

/// 读取 + 校验。所有消费方的唯一入口。
pub fn load_checked(path: &Path, format: SecFileFormat) -> MainResult<SecFile> {
    load_checked_ex(path, format, None, 0o600)
}

fn load_checked_ex(
    path: &Path,
    format: SecFileFormat,
    expose: Option<ExposeConf>,
    mode: u32,
) -> MainResult<SecFile> {
    check_file_security(path, mode)?;
    check_file_size(path)?;
    let space = orion_sec::load_secfile_by(path.to_path_buf(), format.into())
        .map_err(|err| MainReason::logic_detail(redact(&err.to_string())))?;

    let mut dict = EnvDict::new();
    for (key, value) in space.iter() {
        let text = match value {
            SecValueType::String(v) => v.value().clone(),
            other => {
                return Err(MainReason::logic_detail(format!(
                    "密钥 `{}` 不是字符串（{}）：YAML 会把纯数字/浮点/嵌套结构改写，\
                     请在密钥文件里用引号包住该值",
                    key.as_str(),
                    type_name(other)
                )));
            }
        };
        let env_name = key.as_str();
        if !is_valid_env_name(env_name) {
            return Err(MainReason::logic_detail(format!(
                "密钥 `{env_name}` 不是合法的环境变量名（只允许字母/数字/下划线）；\
                 键里的 `-` / `.` 会让子进程读到无效变量"
            )));
        }
        dict.insert(
            UpperKey::from(env_name.to_string()),
            ValueType::String(text),
        );
    }
    Ok(SecFile::new(path.to_path_buf(), dict, expose))
}

/// 只支持 `*` 的通配匹配（大小写不敏感），`*` 可匹配空串。
///
/// 不支持 `?` / `[]`：密钥键名是 `[A-Z0-9_]`，用不上字符类，少一种语法就少一类坑。
pub fn glob_star(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let pattern: Vec<char> = pattern.to_uppercase().chars().collect();
    let name: Vec<char> = name.to_uppercase().chars().collect();

    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while ni < name.len() {
        if pi < pattern.len() && pattern[pi] == name[ni] {
            pi += 1;
            ni += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            pi += 1;
            resume = ni;
        } else if let Some(star_pos) = star {
            pi = star_pos + 1;
            resume += 1;
            ni = resume;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// 大小上限（防 YAML 炸弹 / 误指大文件）。声明式与 `.galaxy` 回退两条路都覆盖。
fn check_file_size(path: &Path) -> MainResult<()> {
    let meta = fs::symlink_metadata(path).source_resource()?;
    if meta.len() > MAX_SEC_FILE_BYTES {
        return Err(MainReason::logic_detail(format!(
            "密钥文件过大（{} 字节 > {}）：{}",
            meta.len(),
            MAX_SEC_FILE_BYTES,
            path.display()
        )));
    }
    Ok(())
}

/// 归一化后的键必须是合法环境变量名：`[A-Za-z_][A-Za-z0-9_]*`。
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn type_name(value: &SecValueType) -> &'static str {
    match value {
        SecValueType::String(_) => "string",
        SecValueType::Bool(_) => "bool",
        SecValueType::Number(_) => "number",
        SecValueType::Float(_) => "float",
        SecValueType::Ip(_) => "ip",
        SecValueType::Obj(_) => "nested map",
        SecValueType::List(_) => "list",
    }
}

/// 解析错误只保留结构信息：`serde_yaml` 的报错形如
/// ``invalid type: integer `1234567890`, expected a string at line 3 column 9``，
/// 反引号/引号里就是明文值，必须抹掉再进日志。
fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut quote: Option<char> = None;
    for ch in message.chars() {
        match quote {
            None => {
                if matches!(ch, '`' | '"' | '\'') {
                    quote = Some(ch);
                }
                out.push(ch);
            }
            Some(q) => {
                if ch == q {
                    quote = None;
                    out.push(ch);
                } else {
                    out.push('*');
                }
            }
        }
    }
    out
}

/// 只读取 `sys-prj.yml` 的 `secrets:` 段。
///
/// **不用** `SysConf::load_conf`：`gops run` 以前根本不读 `sys-prj.yml`，而 `SysConf`
/// 对 `test_envs` 等字段是硬依赖——拿整份 schema 去校验会让“清单不完整”的系统
/// 从“能跑”变成“起不来”。这里只关心 `secrets`，其余键一律忽略。
#[derive(Debug, Default, serde_derive::Deserialize)]
struct SecretsSection {
    #[serde(default)]
    secrets: Option<SecretsConf>,
}

/// 读取 `sys-prj.yml`（或旧名 `sys_prj.yml`）里的 `secrets:` 声明。
fn declared_conf(sys_root: &Path) -> MainResult<Option<SecretsConf>> {
    let v2 = sys_root.join("sys-prj.yml");
    let v1 = sys_root.join("sys_prj.yml");
    let path = if v2.exists() {
        v2
    } else if v1.exists() {
        v1
    } else {
        return Ok(None);
    };

    if !path.is_file() {
        return Err(MainReason::logic_detail(format!(
            "系统清单不是普通文件：{}",
            path.display()
        )));
    }
    let content = fs::read_to_string(&path).source_resource()?;
    // 清单是包控的公开文件（只有路径与模式，没有值），错误信息不脱敏——
    // 否则 serde 的「unknown field `loclize`, expected one of ...」会被打码成 `*****`，
    // 现场根本没法定位拼写错误。
    let section: SecretsSection = serde_yaml::from_str(&content)
        .map_err(|err| MainReason::logic_detail(format!("{}: {err}", path.display())))?;
    Ok(section.secrets)
}

/// 未声明时的旧约定：`<系统根>/.galaxy` → `./.galaxy` → `~/.galaxy`，缺失即空集。
///
/// 系统根排在最前：`gops run` / `prj upgrade` 可能在别的目录下操作某个系统，
/// 以系统根为准才不会读到“执行者当前目录”的密钥。
/// 只有“完全没声明”才走到这里；一旦声明，缺文件是硬失败。
fn load_galaxy_fallback(sys_root: &Path) -> MainResult<SecFile> {
    let mut candidates = vec![sys_root.join(".galaxy").join("sec_value.yml")];
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join(".galaxy").join("sec_value.yml");
        if !candidates.contains(&local) {
            candidates.push(local);
        }
    }
    if let Some(home) = home::home_dir() {
        candidates.push(home.join(".galaxy").join("sec_value.yml"));
    }

    let Some(path) = candidates.iter().find(|p| p.exists()).cloned() else {
        return Ok(SecFile::empty(
            candidates.into_iter().next().unwrap_or_default(),
        ));
    };
    load_checked(&path, SecFileFormat::Yaml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_secret(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        file.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }

    #[test]
    fn declared_path_must_be_relative() {
        let root = temp_root();
        let conf = SecretsConf::new("/etc/passwd");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        assert!(err.to_string().contains("相对系统根"));
    }

    #[test]
    fn declared_path_rejects_parent_traversal() {
        let root = temp_root();
        let conf = SecretsConf::new("../outside.yml");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        assert!(err.to_string().contains("相对路径段"));
    }

    #[test]
    fn declared_missing_file_is_hard_error() {
        let root = temp_root();
        fs::write(
            root.path().join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nsecrets:\n  file: .wist/sec-value.yml\n",
        )
        .unwrap();
        let err = load_for_system(root.path()).unwrap_err();
        assert!(err.to_string().contains("文件不存在"));
    }

    #[test]
    fn loads_quoted_values() {
        let root = temp_root();
        let path = write_secret(
            root.path(),
            ".wist/sec-value.yml",
            "admin_token: \"abc123\"\n",
        );
        let sec = load_checked(&path, SecFileFormat::Yaml).unwrap();
        assert_eq!(sec.len(), 1);
        assert_eq!(
            sec.get("admin_token").map(|v| v.to_string()),
            Some("abc123".to_string())
        );
        assert!(sec.keys().any(|k| k.as_str() == "SEC_ADMIN_TOKEN"));
    }

    #[test]
    fn rejects_numeric_values() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "port: 8080\n");
        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        assert!(err.to_string().contains("不是字符串"));
    }

    #[test]
    fn rejects_world_readable_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root();
            let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
            assert!(err.to_string().contains("secrets.mode"), "{err}");
        }
    }

    #[test]
    fn accepts_group_readable_when_mode_is_0640() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root();
            let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let conf: SecretsConf =
                serde_yaml::from_str("file: .wist/sec-value.yml\nmode: \"0640\"").unwrap();
            let sec = load_declared(root.path(), &conf).expect("0640 与容器组共享应被接受");
            assert!(sec.keys().any(|k| k.as_str() == "SEC_A"));
        }
    }

    #[test]
    fn rejects_wrong_mode_for_0640_policy() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root();
            let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
            // 0644 仍然过宽（other 可读），即使声明了 0640
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let conf: SecretsConf =
                serde_yaml::from_str("file: .wist/sec-value.yml\nmode: \"0640\"").unwrap();
            let err = load_declared(root.path(), &conf).unwrap_err();
            assert!(err.to_string().contains("secrets.mode"), "{err}");
        }
    }

    #[test]
    fn rejects_group_readable_when_mode_is_0600() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root();
            let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let conf: SecretsConf = serde_yaml::from_str("file: .wist/sec-value.yml").unwrap();
            let err = load_declared(root.path(), &conf).unwrap_err();
            assert!(err.to_string().contains("secrets.mode"), "{err}");
        }
    }

    #[test]
    fn rejects_unsupported_secrets_mode() {
        let root = temp_root();
        write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        let conf: SecretsConf =
            serde_yaml::from_str("file: .wist/sec-value.yml\nmode: \"0644\"").unwrap();
        let err = load_declared(root.path(), &conf).unwrap_err();
        assert!(err.to_string().contains("只允许"), "{err}");
    }

    #[test]
    fn secrets_mode_parsing_and_numeric_rejection() {
        let mode = |y: &str| {
            serde_yaml::from_str::<SecretsConf>(y)
                .unwrap()
                .file_mode()
                .unwrap()
        };
        assert_eq!(mode("file: x.yml\n"), 0o600, "默认 0600");
        assert_eq!(mode("file: x.yml\nmode: \"0600\"\n"), 0o600);
        assert_eq!(mode("file: x.yml\nmode: \"600\"\n"), 0o600);
        assert_eq!(mode("file: x.yml\nmode: \"0640\"\n"), 0o640);
        assert_eq!(mode("file: x.yml\nmode: \" 640 \"\n"), 0o640);

        let bad = serde_yaml::from_str::<SecretsConf>("file: x.yml\nmode: \"0644\"\n").unwrap();
        assert!(bad.file_mode().is_err());

        // 未加引号会被 YAML 当整数：必须给出可操作的报错，而不是静默成 640。
        let err = serde_yaml::from_str::<SecretsConf>("file: x.yml\nmode: 640\n").unwrap_err();
        assert!(err.to_string().contains("带引号"), "{err}");
    }

    #[test]
    fn accepts_read_only_variants_of_each_policy() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root();
            let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");

            fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
            let conf: SecretsConf = serde_yaml::from_str("file: .wist/sec-value.yml").unwrap();
            assert!(
                load_declared(root.path(), &conf).is_ok(),
                "0400 应被 0600 策略接受"
            );

            fs::set_permissions(&path, fs::Permissions::from_mode(0o440)).unwrap();
            let conf: SecretsConf =
                serde_yaml::from_str("file: .wist/sec-value.yml\nmode: \"0640\"").unwrap();
            assert!(
                load_declared(root.path(), &conf).is_ok(),
                "0440 应被 0640 策略接受"
            );
        }
    }

    #[test]
    fn parse_error_does_not_leak_value() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "a: [unclosed\n");
        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        let text = err.to_string();
        assert!(!text.contains("unclosed"), "err={text}");
    }

    #[test]
    fn raw_name_strips_prefix() {
        assert_eq!(
            SecFile::raw_name("SEC_WP_GIT_PUSH_TOKEN"),
            "wp_git_push_token"
        );
        assert_eq!(SecFile::raw_name("OTHER"), "other");
    }

    // ── review 补充 ────────────────────────────────────────────────

    /// 回归：`gops run` 以前根本不读 `sys-prj.yml`。清单若是旧格式/缺 `test_envs`，
    /// 不能因为“拿整份 schema 去校验”而让原本能跑的系统起不来。
    #[test]
    fn manifest_without_test_envs_still_loads() {
        let root = temp_root();
        write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        fs::write(
            root.path().join("sys-prj.yml"),
            "secrets:\n  file: .wist/sec-value.yml\n",
        )
        .unwrap();
        let sec = load_for_system(root.path()).unwrap();
        assert_eq!(sec.len(), 1);
    }

    #[test]
    fn legacy_sys_prj_yml_is_read() {
        let root = temp_root();
        write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        fs::write(
            root.path().join("sys_prj.yml"),
            "secrets:\n  file: .wist/sec-value.yml\n",
        )
        .unwrap();
        let sec = load_for_system(root.path()).unwrap();
        assert_eq!(sec.len(), 1);
    }

    #[test]
    fn rejects_nested_map() {
        let root = temp_root();
        let path = write_secret(
            root.path(),
            ".wist/sec-value.yml",
            "db:\n  password: \"x\"\n",
        );
        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        assert!(err.to_string().contains("nested map"), "err={err}");
    }

    #[test]
    fn rejects_bool_and_float() {
        let root = temp_root();
        let bool_path = write_secret(root.path(), ".wist/b.yml", "flag: true\n");
        assert!(
            load_checked(&bool_path, SecFileFormat::Yaml)
                .unwrap_err()
                .to_string()
                .contains("bool")
        );
        let float_path = write_secret(root.path(), ".wist/f.yml", "rate: 1e5\n");
        assert!(
            load_checked(&float_path, SecFileFormat::Yaml)
                .unwrap_err()
                .to_string()
                .contains("float")
        );
    }

    #[test]
    fn rejects_invalid_env_name() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "db.password: \"x\"\n");
        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        assert!(
            err.to_string().contains("不是合法的环境变量名"),
            "err={err}"
        );
    }

    #[test]
    fn empty_file_is_empty_set() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "");
        let sec = load_checked(&path, SecFileFormat::Yaml).unwrap();
        assert!(sec.is_empty());
        assert_eq!(sec.len(), 0);
    }

    #[test]
    fn empty_string_value_is_allowed() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"\"\n");
        let sec = load_checked(&path, SecFileFormat::Yaml).unwrap();
        assert_eq!(sec.entries(), vec![("SEC_A".to_string(), 0)]);
    }

    #[test]
    fn rejects_directory() {
        let root = temp_root();
        fs::create_dir_all(root.path().join(".wist/dir.yml")).unwrap();
        let conf = SecretsConf::new(".wist/dir.yml");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        assert!(err.to_string().contains("普通文件"), "err={err}");
    }

    #[test]
    fn rejects_oversized_file() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/big.yml", "");
        let big = vec![b'a'; (MAX_SEC_FILE_BYTES + 1) as usize];
        fs::write(&path, big).unwrap();
        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        assert!(err.to_string().contains("密钥文件过大"), "err={err}");
    }

    /// 关键安全断言：`SecFile` 的 `Debug` 不能把明文带到日志里。
    #[test]
    fn debug_does_not_leak_value() {
        let root = temp_root();
        let path = write_secret(
            root.path(),
            ".wist/sec-value.yml",
            "admin_token: \"super-secret-value\"\n",
        );
        let sec = load_checked(&path, SecFileFormat::Yaml).unwrap();
        let rendered = format!("{sec:?}");
        assert!(rendered.contains("SEC_ADMIN_TOKEN"), "{rendered}");
        assert!(!rendered.contains("super-secret-value"), "{rendered}");
    }

    #[test]
    fn get_is_case_insensitive_and_accepts_prefix() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "admin_token: \"v\"\n");
        let sec = load_checked(&path, SecFileFormat::Yaml).unwrap();
        for key in [
            "admin_token",
            "ADMIN_TOKEN",
            "SEC_ADMIN_TOKEN",
            "sec_admin_token",
        ] {
            assert_eq!(
                sec.get(key).map(|v| v.to_string()),
                Some("v".to_string()),
                "key={key}"
            );
        }
        assert!(sec.get("nope").is_none());
    }

    /// 未声明时的回退优先看**系统根**（`gops run` 可能在别的 cwd 下操作该系统）。
    #[test]
    fn fallback_prefers_system_root_galaxy() {
        let root = temp_root();
        write_secret(root.path(), ".galaxy/sec_value.yml", "from_root: \"yes\"\n");
        let sec = load_for_system(root.path()).unwrap();
        assert_eq!(sec.len(), 1);
        assert!(sec.get("from_root").is_some());
    }

    // ── secrets.expose（最小权限） ──────────────────────────────

    fn system_with_expose(body: &str) -> tempfile::TempDir {
        let root = temp_root();
        write_secret(
            root.path(),
            ".wist/sec-value.yml",
            "center_admin_token: \"a\"\ncenter_hmac_secret: \"b\"\nwp_git_push_token: \"c\"\n",
        );
        fs::write(
            root.path().join("sys-prj.yml"),
            format!("secrets:\n  file: .wist/sec-value.yml\n{body}"),
        )
        .unwrap();
        root
    }

    #[test]
    fn expose_defaults_run_all_localize_none() {
        let root = system_with_expose("");
        let sec = load_for_system(root.path()).unwrap();

        assert_eq!(sec.scoped(Subscriber::Run).unwrap().len(), 3);
        assert_eq!(sec.scoped(Subscriber::Sec).unwrap().len(), 3);
        // localize 默认不注入
        assert!(sec.scoped(Subscriber::Localize).unwrap().is_empty());
    }

    #[test]
    fn expose_patterns_filter_keys() {
        let root = system_with_expose("  expose:\n    localize:\n      - SEC_CENTER_*\n");
        let sec = load_for_system(root.path()).unwrap();
        let scoped = sec.scoped(Subscriber::Localize).unwrap();

        assert_eq!(scoped.len(), 2);
        assert!(scoped.get("center_admin_token").is_some());
        assert!(scoped.get("center_hmac_secret").is_some());
        assert!(scoped.get("wp_git_push_token").is_none());
        // 其他消费方不受影响
        assert_eq!(sec.scoped(Subscriber::Run).unwrap().len(), 3);
    }

    #[test]
    fn expose_all_keyword_and_narrowing() {
        let root = system_with_expose("  expose:\n    run: all\n    sec:\n      - SEC_WP_*\n");
        let sec = load_for_system(root.path()).unwrap();
        assert_eq!(sec.scoped(Subscriber::Run).unwrap().len(), 3);
        let sec_scoped = sec.scoped(Subscriber::Sec).unwrap();
        assert_eq!(sec_scoped.len(), 1);
        assert!(sec_scoped.get("wp_git_push_token").is_some());
    }

    #[test]
    fn expose_empty_list_means_none() {
        let root = system_with_expose("  expose:\n    run: []\n");
        let sec = load_for_system(root.path()).unwrap();
        assert!(sec.scoped(Subscriber::Run).unwrap().is_empty());
    }

    #[test]
    fn expose_unknown_consumer_is_rejected() {
        let root = system_with_expose("  expose:\n    loclize: all\n");
        let err = load_for_system(root.path()).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "err={err}");
        assert!(err.contains("loclize"), "err={err}");
    }

    #[test]
    fn expose_rejects_non_all_keyword() {
        let root = system_with_expose("  expose:\n    run: everything\n");
        let err = load_for_system(root.path()).unwrap_err().to_string();
        assert!(err.contains("all"), "err={err}");
    }

    #[test]
    fn glob_star_supports_prefix_suffix_middle() {
        assert!(glob_star("SEC_CENTER_*", "SEC_CENTER_ADMIN_TOKEN"));
        assert!(glob_star("*_TOKEN", "SEC_WP_GIT_PUSH_TOKEN"));
        assert!(glob_star("SEC_*_TOKEN", "SEC_WP_GIT_PUSH_TOKEN"));
        assert!(glob_star("*", "ANYTHING"));
        assert!(glob_star("SEC_**", "SEC_A"));
        assert!(glob_star("sec_center_*", "SEC_CENTER_A"), "大小写不敏感");
        assert!(!glob_star("SEC_CENTER_*", "SEC_WP_A"));
        assert!(!glob_star("SEC_A", "SEC_AB"));
        assert!(!glob_star("SEC_?", "SEC_A"), "不支持 ?");
    }

    #[test]
    fn glob_star_edge_cases() {
        // 空模式不匹配任何键（上游 scoped 也会直接拒绝）
        assert!(!glob_star("", "ANY"));
        // `*` 可匹配空串
        assert!(glob_star("*", ""));
        assert!(glob_star("A*B", "AB"));
        assert!(glob_star("**", "ABC"));
        assert!(glob_star("A*C", "ABC"));
        assert!(glob_star("*C", "ABC"));
        assert!(glob_star("A*", "ABC"));
        assert!(!glob_star("A*D", "ABC"));
        assert!(glob_star("A*B*C", "AXXBYYC"));
        assert!(!glob_star("A*B*C", "AXXBYY"));
        assert!(glob_star("ABC", "ABC"));
        assert!(!glob_star("ABCD", "ABC"));
    }

    #[test]
    fn scoped_from_pairs_uses_defaults() {
        let sec = SecFile::from_pairs(
            "test.yml",
            vec![
                ("center_admin_token".to_string(), "a".to_string()),
                ("wp_git_push_token".to_string(), "b".to_string()),
            ],
        );
        assert_eq!(sec.scoped(Subscriber::Run).unwrap().len(), 2);
        assert_eq!(sec.scoped(Subscriber::Sec).unwrap().len(), 2);
        assert!(sec.scoped(Subscriber::Localize).unwrap().is_empty());
    }

    #[test]
    fn scoped_twice_is_idempotent() {
        let root = system_with_expose("  expose:\n    localize:\n      - SEC_CENTER_*\n");
        let sec = load_for_system(root.path()).unwrap();
        let once = sec.scoped(Subscriber::Localize).unwrap();
        let twice = once.scoped(Subscriber::Localize).unwrap();
        assert_eq!(once.entries(), twice.entries());
    }

    #[test]
    fn scoped_rejects_blank_pattern() {
        let root = system_with_expose("  expose:\n    localize:\n      - \"  \"\n");
        let sec = load_for_system(root.path()).unwrap();
        let err = sec.scoped(Subscriber::Localize).unwrap_err().to_string();
        assert!(err.contains("空模式"), "err={err}");
    }

    /// 模式零命中只告警、不报错（允许现场先删掉某个键）。
    #[test]
    fn scoped_unmatched_pattern_does_not_fail() {
        let root = system_with_expose("  expose:\n    localize:\n      - SEC_NOPE_*\n");
        let sec = load_for_system(root.path()).unwrap();
        let scoped = sec.scoped(Subscriber::Localize).unwrap();
        assert!(scoped.is_empty());
    }

    #[test]
    fn dangling_symlink_is_reported() {
        #[cfg(unix)]
        {
            let root = temp_root();
            let link = root.path().join(".wist-dangling.yml");
            std::os::unix::fs::symlink(root.path().join("missing.yml"), &link).unwrap();
            let conf = SecretsConf::new(".wist-dangling.yml");
            let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
            assert!(err.to_string().contains("悬空"), "err={err}");
        }
    }

    #[test]
    fn secrets_section_without_file_is_rejected() {
        let root = temp_root();
        fs::write(
            root.path().join("sys-prj.yml"),
            "secrets:\n  expose:\n    run: all\n",
        )
        .unwrap();
        let err = load_for_system(root.path()).unwrap_err().to_string();
        assert!(err.contains("file"), "err={err}");
    }

    #[test]
    fn secrets_typo_field_is_rejected() {
        let root = system_with_expose("  exposee: all\n");
        let err = load_for_system(root.path()).unwrap_err().to_string();
        assert!(err.contains("exposee"), "err={err}");
    }

    #[test]
    fn subscriber_parse_is_case_insensitive() {
        assert_eq!(Subscriber::parse("run"), Some(Subscriber::Run));
        assert_eq!(Subscriber::parse("Localize"), Some(Subscriber::Localize));
        assert_eq!(Subscriber::parse("SEC"), Some(Subscriber::Sec));
        assert_eq!(Subscriber::parse("everything"), None);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_file() {
        let root = temp_root();
        let real = write_secret(root.path(), ".wist/real.yml", "a: \"b\"\n");
        let link = root.path().join(".wist/sec-value.yml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let conf = SecretsConf::new(".wist/sec-value.yml");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        assert!(err.to_string().contains("符号链接"));
    }

    /// 硬链接可以绕过路径校验：同 inode 可能把根外文件“链”进系统根（或反过来）。
    #[cfg(unix)]
    #[test]
    fn rejects_hardlinked_file() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        let alias = root.path().join(".wist/alias.yml");
        fs::hard_link(&path, &alias).unwrap();

        let err = load_checked(&path, SecFileFormat::Yaml).unwrap_err();
        assert!(err.to_string().contains("硬链接"), "err={err}");
    }

    /// 普通单链接文件不受影响。
    #[test]
    fn regular_file_passes_link_check() {
        let root = temp_root();
        let path = write_secret(root.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        assert!(load_checked(&path, SecFileFormat::Yaml).is_ok());
    }

    /// 父目录（`.wist` 本身）是符号链接同样要拒绝：否则可以把整个目录指到根外。
    /// 指到根外时先命中“越界”检查，指到根内时命中“含符号链接”检查。
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_dir() {
        let root = temp_root();
        let outside = tempfile::tempdir().unwrap();
        write_secret(outside.path(), ".wist/sec-value.yml", "a: \"b\"\n");
        std::os::unix::fs::symlink(outside.path().join(".wist"), root.path().join(".wist"))
            .unwrap();
        let conf = SecretsConf::new(".wist/sec-value.yml");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("系统根之外") || text.contains("符号链接"),
            "err={text}"
        );
    }

    /// 指向根内的符号链接则必须命中“含符号链接”分支。
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_dir_inside_root() {
        let root = temp_root();
        write_secret(root.path(), "real/sec-value.yml", "a: \"b\"\n");
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join(".wist")).unwrap();
        let conf = SecretsConf::new(".wist/sec-value.yml");
        let err = resolve_declared_path(root.path(), conf.file()).unwrap_err();
        assert!(err.to_string().contains("符号链接"), "err={err}");
    }
}
