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
