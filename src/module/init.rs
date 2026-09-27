use super::prelude::*;

use crate::workflow::{
    act::{ModWorkflows, Workflow},
    gxl::GxlAction,
};

pub const MOD_HOST_OPS_GXL: &str = include_str!("init/host/workflows/operators.gxl");
pub const MOD_PRJ_WORK_GXL: &str = include_str!("init/_gal/work.gxl");
pub const MOD_PRJ_ROOT_FILE: &str = include_str!("init/_gal/project.toml");
pub const MOD_PRJ_ADM_GXL: &str = include_str!("init/_gal/adm.gxl");
pub const MOD_HOST_WORK_GXL: &str = include_str!("init/host/_gal/work.gxl");
pub const MOD_PRJ_GITIGNORE: &str = include_str!("init/.gitignore");

pub const K8S_K8S_OPS_GXL: &str = include_str!("init/k8s/spec/workflows/operators.gxl");
pub const MOD_K8S_WORK_GXL: &str = include_str!("init/k8s/_gal/work.gxl");
pub const K8S_CONFS_CHART_YAML: &str = include_str!("init/k8s/confs/Chart.yaml");
pub const K8S_CONFS_VALUES_YAML: &str = include_str!("init/k8s/confs/values.yaml");
pub const K8S_CONFS_TPL_DEPLOYMENT: &str = include_str!("init/k8s/confs/templates/deployment.yaml");
pub const K8S_CONFS_TPL_SERVICE: &str = include_str!("init/k8s/confs/templates/service.yaml");

/// 为 k8s 模型生成 Helm chart 脚手架：`spec/confs/{Chart.yaml,values.yaml,templates/*}`。
pub fn mod_k8s_confs_init(root: &Path) -> MainResult<()> {
    let confs = root.join(crate::const_vars::SPEC_DIR).join("confs");
    let tpl = confs.join("templates");
    std::fs::create_dir_all(&tpl).source_resource().with(&tpl)?;
    let files = [
        (confs.join("Chart.yaml"), K8S_CONFS_CHART_YAML),
        (confs.join("values.yaml"), K8S_CONFS_VALUES_YAML),
        (tpl.join("deployment.yaml"), K8S_CONFS_TPL_DEPLOYMENT),
        (tpl.join("service.yaml"), K8S_CONFS_TPL_SERVICE),
    ];
    for (path, content) in files {
        // 仅在缺失时写入，保留用户对 chart 的既有修改（与 mod_init_gitignore 一致）
        if !path.exists() {
            std::fs::write(&path, content)
                .source_resource()
                .with(&path)?;
        }
    }
    Ok(())
}
pub trait ModActIniter {
    fn host_ops_tpl() -> Self;
    fn k8s_ops_tpl() -> Self;
}
pub trait ModPrjIniter {
    fn spec_host_tpl() -> Self;
    fn spec_k8s_tpl() -> Self;
}

impl ModActIniter for GxlAction {
    fn host_ops_tpl() -> Self {
        Self::new("operators.gxl".into(), MOD_HOST_OPS_GXL.to_string())
    }
    fn k8s_ops_tpl() -> Self {
        Self::new("operators.gxl".into(), K8S_K8S_OPS_GXL.to_string())
    }
}
impl ModPrjIniter for GxlProject {
    fn spec_host_tpl() -> Self {
        Self::from(MOD_HOST_WORK_GXL)
    }
    fn spec_k8s_tpl() -> Self {
        Self::from(MOD_K8S_WORK_GXL)
    }
}

pub trait ModIniter {
    fn mod_host_tpl_init() -> Self;
    fn mod_k8s_tpl_init() -> Self;
}

impl ModIniter for ModWorkflows {
    fn mod_host_tpl_init() -> Self {
        let actions = vec![Workflow::Gxl(GxlAction::host_ops_tpl())];
        Self::new(actions)
    }

    fn mod_k8s_tpl_init() -> ModWorkflows {
        let actions = vec![Workflow::Gxl(GxlAction::k8s_ops_tpl())];
        Self::new(actions)
    }
}

pub fn mod_init_gitignore(path: &Path) -> MainResult<()> {
    let ignore_path = path.join(".gitignore");
    if !ignore_path.exists() {
        std::fs::write(&ignore_path, MOD_PRJ_GITIGNORE)
            .source_resource()
            .with(&ignore_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn chart_dir(root: &Path) -> PathBuf {
        root.join(crate::const_vars::SPEC_DIR).join("confs")
    }

    fn chart_files(root: &Path) -> Vec<PathBuf> {
        let confs = chart_dir(root);
        vec![
            confs.join("Chart.yaml"),
            confs.join("values.yaml"),
            confs.join("templates").join("deployment.yaml"),
            confs.join("templates").join("service.yaml"),
        ]
    }

    #[test]
    fn test_mod_k8s_confs_init_creates_full_chart() -> MainResult<()> {
        let tmp = TempDir::new().source_resource()?;
        let root = tmp.path().join("x86-ubt22-k8s");
        mod_k8s_confs_init(&root)?;
        for file in chart_files(&root) {
            assert!(file.exists(), "missing chart file: {}", file.display());
            assert!(
                !std::fs::read_to_string(&file).source_resource()?.is_empty(),
                "empty chart file: {}",
                file.display()
            );
        }
        Ok(())
    }

    #[test]
    fn test_mod_k8s_confs_init_keeps_user_edits() -> MainResult<()> {
        let tmp = TempDir::new().source_resource()?;
        let root = tmp.path().join("x86-ubt22-k8s");
        mod_k8s_confs_init(&root)?;
        let values = chart_dir(&root).join("values.yaml");
        std::fs::write(&values, "image: \"user-edited\"\n").source_resource()?;

        // 再次初始化（对应重复 save / mod update）不应覆盖用户修改
        mod_k8s_confs_init(&root)?;
        assert_eq!(
            std::fs::read_to_string(&values).source_resource()?,
            "image: \"user-edited\"\n"
        );
        Ok(())
    }

    #[test]
    fn test_mod_k8s_confs_init_is_idempotent() -> MainResult<()> {
        let tmp = TempDir::new().source_resource()?;
        let root = tmp.path().join("x86-ubt22-k8s");
        mod_k8s_confs_init(&root)?;
        let first: Vec<String> = chart_files(&root)
            .iter()
            .map(|f| std::fs::read_to_string(f).unwrap())
            .collect();
        mod_k8s_confs_init(&root)?;
        let second: Vec<String> = chart_files(&root)
            .iter()
            .map(|f| std::fs::read_to_string(f).unwrap())
            .collect();
        assert_eq!(first, second);
        Ok(())
    }

    // ---- 模板内容不变量：防止 gops 与 Helm / ops-gxl 的约定悄悄漂移 ----

    #[test]
    fn test_k8s_operators_use_helm_ops() {
        assert!(K8S_K8S_OPS_GXL.contains("mod operators : helm_ops"));
        assert!(K8S_K8S_OPS_GXL.contains("galaxio-hub/ops-gxl"));
        assert!(K8S_K8S_OPS_GXL.contains("${GXL_CHANNEL:main}"));
        // helm_ops.download 依赖 SPEC_DIR 环境变量定位 artifact.yml
        assert!(MOD_K8S_WORK_GXL.contains("SPEC_DIR"));
    }

    #[test]
    fn test_host_operators_scaffold_invariants() {
        assert!(MOD_HOST_OPS_GXL.contains("galaxio-hub/ops-gxl"));
        // 统一使用 GXL_CHANNEL（旧模板用过 GXL_CHANNEL_OPS）
        assert!(MOD_HOST_OPS_GXL.contains("${GXL_CHANNEL:main}"));
        assert!(!MOD_HOST_OPS_GXL.contains("GXL_CHANNEL_OPS"));
        // git（repo[/tag]）与 http（url）双形态
        assert!(MOD_HOST_OPS_GXL.contains("${ITEM.ORIGIN_ADDR.REPO}"));
        assert!(MOD_HOST_OPS_GXL.contains("${ITEM.ORIGIN_ADDR.URL}"));
        // 下载前清理缓存目录，保证重复下载幂等
        assert!(MOD_HOST_OPS_GXL.contains("rm -rf"));
        assert!(MOD_HOST_OPS_GXL.contains("--branch"));
        // 不再残留未使用的 _used.json 读取与空的 __into 入口
        assert!(!MOD_HOST_OPS_GXL.contains("_used.json"));
        assert!(!MOD_HOST_OPS_GXL.contains("__into"));
    }

    #[test]
    fn test_k8s_chart_label_contract() {
        // values.yaml 由 gops 用 [[ ]] 渲染
        assert!(K8S_CONFS_VALUES_YAML.contains("[[IMAGE_REGISTRY]]"));
        assert!(K8S_CONFS_VALUES_YAML.contains("[[IMAGE_REPOSITORY]]"));
        assert!(K8S_CONFS_VALUES_YAML.contains("[[IMAGE_TAG]]"));
        assert!(K8S_CONFS_VALUES_YAML.contains("[[IMAGE_PULL_SECRET]]"));
        assert!(K8S_CONFS_VALUES_YAML.contains("[[REPLICA_COUNT]]"));

        // templates 交给 Helm 的 {{ }}，且不得混入 gops 的 [[ ]]
        assert!(K8S_CONFS_TPL_DEPLOYMENT.contains("{{ .Release.Name }}"));
        assert!(K8S_CONFS_TPL_DEPLOYMENT.contains("{{- if .Values.imagePullSecret }}"));
        assert!(K8S_CONFS_TPL_DEPLOYMENT.contains("imagePullSecrets:"));
        assert!(K8S_CONFS_TPL_SERVICE.contains("{{ .Values.service.type }}"));
        assert!(!K8S_CONFS_TPL_DEPLOYMENT.contains("[["));
        assert!(!K8S_CONFS_TPL_SERVICE.contains("[["));
    }

    #[test]
    fn test_prj_work_declares_k8s_model() {
        assert!(MOD_PRJ_WORK_GXL.contains("x86-ubt22-k8s"));
        assert!(MOD_PRJ_WORK_GXL.contains("galaxio-hub/ops-gxl"));
    }
}
