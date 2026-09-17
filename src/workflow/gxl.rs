use super::prelude::*;
#[derive(Getters, Clone, Debug, PartialEq, Serialize)]
#[getset(get = "pub")]
pub struct GxlAction {
    file: String,
    code: String,
}

impl GxlAction {
    pub fn new(file: String, code: String) -> Self {
        Self { file, code }
    }
    pub fn is_action(path: &Path) -> bool {
        if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
            return matches!(
                file_name,
                "setup.gxl" | "update.gxl" | "port.gxl" | "backup.gxl" | "uninstall.gxl"
            );
        }
        false
    }
}
impl FilePersist<GxlAction> for GxlAction {
    fn save_to(&self, path: &Path, _name: Option<String>) -> SerdeResult<()> {
        let path_file = path.join(self.file());
        std::fs::write(path_file, self.code.as_str()).source_resource()?;
        Ok(())
    }

    fn load_from(path: &Path) -> SerdeResult<GxlAction> {
        let file_name = path
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| SerdeReason::from("bad file name".to_string()))?;

        let code = std::fs::read_to_string(path).source_resource()?;
        Ok(Self {
            file: file_name.to_string(),
            code,
        })
    }
}
