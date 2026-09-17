use super::prelude::*;

use derive_getters::Getters;
use serde::Serialize;

#[derive(Getters, Clone, Debug, Default, Serialize)]
pub struct GxlProject {
    work: String,
    adm: Option<String>,
    prj: Option<String>,
}
impl From<&str> for GxlProject {
    fn from(value: &str) -> Self {
        Self {
            work: value.to_string(),
            adm: None,
            prj: None,
        }
    }
}

impl From<(&str, &str)> for GxlProject {
    fn from(value: (&str, &str)) -> Self {
        Self {
            work: value.0.to_string(),
            adm: Some(value.1.to_string()),
            prj: None,
        }
    }
}

impl From<(&str, &str, &str)> for GxlProject {
    fn from(value: (&str, &str, &str)) -> Self {
        Self {
            work: value.0.to_string(),
            adm: Some(value.1.to_string()),
            prj: Some(value.2.to_string()),
        }
    }
}

impl FilePersist<GxlProject> for GxlProject {
    fn save_to(&self, path: &Path, _name: Option<String>) -> SerdeResult<()> {
        let gal_path = path.join("_gal");
        std::fs::create_dir_all(&gal_path)
            .source_resource()
            .with(&gal_path)?;
        std::fs::write(
            gal_path.join(crate::const_vars::WORK_GXL),
            self.work.as_str(),
        )
        .source_resource()?;
        if let Some(adm) = &self.adm {
            std::fs::write(gal_path.join(ADM_GXL), adm.as_str()).source_resource()?;
            let version_path = path.join("version.txt");
            if !version_path.exists() {
                std::fs::write(version_path, "0.1.0")
                    .source_resource()
                    .want("crate version.txt")?;
            }
        }
        if let Some(prj) = &self.prj {
            std::fs::write(gal_path.join(PRJ_TOML), prj.as_str()).source_resource()?;
        }
        Ok(())
    }

    fn load_from(path: &Path) -> SerdeResult<GxlProject> {
        let work_path = path.join("_gal/work.gxl");
        let adm_path = path.join("_gal/project.toml");
        let prj_path = path.join("_gal/project.toml");
        let work = std::fs::read_to_string(work_path).source_resource()?;
        let adm = if adm_path.exists() {
            Some(std::fs::read_to_string(adm_path).source_resource()?)
        } else {
            None
        };
        let prj = if prj_path.exists() {
            Some(std::fs::read_to_string(prj_path).source_resource()?)
        } else {
            None
        };
        Ok(Self { work, adm, prj })
    }
}
