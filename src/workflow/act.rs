use super::prelude::*;
use crate::prelude::ErrorOwe;
use derive_getters::Getters;
use serde::Serialize;

use crate::const_vars::WORKFLOWS_DIR;

#[derive(Getters, Clone, Debug, Default, Serialize)]
pub struct Workflows {
    //project: GxlProject,
    actions: Vec<Workflow>,
}

pub type ModWorkflows = Workflows;
pub type SysWorkflows = Workflows;

impl Workflows {
    pub fn new(actions: Vec<Workflow>) -> Self {
        Self { actions }
    }
}

impl FilePersist<Workflows> for Workflows {
    fn save_to(&self, path: &Path, name: Option<String>) -> SerdeResult<()> {
        let action_path = path.join(WORKFLOWS_DIR);
        std::fs::create_dir_all(&action_path)
            .source_resource()
            .with(&action_path)?;
        for item in &self.actions {
            item.save_to(&action_path, name.clone())?;
        }
        Ok(())
    }

    //加载 path 目录的文件
    fn load_from(path: &Path) -> SerdeResult<Self> {
        let mut actions = Vec::new();
        let actions_path = path.join(WORKFLOWS_DIR);
        for entry in std::fs::read_dir(&actions_path)
            .source_resource()
            .with(&actions_path)
            .want("read workflows")
            .with(("workflow", "read workflows"))?
        {
            let entry = entry.source_resource()?;
            let entry_path = entry.path();

            if entry_path.is_file() {
                let action = Workflow::load_from(&entry_path);
                match action {
                    Ok(act) => {
                        actions.push(act);
                    }
                    Err(e) => {
                        warn!("load ignore : {}\n {}", entry_path.display(), e);
                    }
                }
            }
        }
        Ok(Workflows { actions })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Workflow {
    Gxl(GxlAction),
}

impl FilePersist<Workflow> for Workflow {
    fn save_to(&self, path: &Path, name: Option<String>) -> SerdeResult<()> {
        match self {
            Workflow::Gxl(act) => act.save_to(path, name),
        }
    }

    fn load_from(path: &Path) -> SerdeResult<Workflow> {
        // 首先检查文件是否存在且是普通文件
        if !path.exists() {
            return Err(SerdeReason::from("path not exists".to_string()).to_err()).with(path);
        }

        if !path.is_file() {
            return Err(SerdeReason::from("path not file".to_string()).to_err()).with(path);
        }

        // 根据扩展名分发加载逻辑
        match path.extension().and_then(|s| s.to_str()) {
            Some("gxl") => GxlAction::load_from(path).map(Workflow::Gxl),
            _ => Err(SerdeReason::from("file type not support".to_string()).to_err()).with(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{error::MainResult, module::init::ModIniter};

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_host_tpl_init() {
        let actions = ModWorkflows::mod_host_tpl_init();
        assert_eq!(actions.actions().len(), 1);
        matches!(actions.actions()[0], Workflow::Gxl(_));
    }

    #[test]
    fn test_k8s_tpl_init() {
        let actions = ModWorkflows::mod_k8s_tpl_init();
        assert_eq!(actions.actions().len(), 1);
        matches!(actions.actions()[0], Workflow::Gxl(_));
    }

    #[test]
    fn test_save_and_load_actions() -> MainResult<()> {
        let temp_dir = TempDir::new().source_resource()?;
        let path = temp_dir.path().to_path_buf();

        // 测试保存和加载
        let original = ModWorkflows::mod_host_tpl_init();
        original.save_to(&path, None).source_logic()?;

        let loaded = ModWorkflows::load_from(&path).source_logic()?;
        assert_eq!(loaded.actions().len(), original.actions().len());
        Ok(())
    }
}
