use crate::internal_prelude::*;

use crate::system::setting::Setting;

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct LocalizeVarPath {
    src: String,
    dst: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    setting: Option<Setting>,
}
impl EnvEvalable<LocalizeVarPath> for LocalizeVarPath {
    fn env_eval(self, dict: &orion_vars::vars::EnvDict) -> Self {
        Self {
            src: self.src.env_eval(dict),
            dst: self.dst.env_eval(dict),
            setting: self.setting.map(|x| x.env_eval(dict)),
        }
    }
}
impl LocalizeVarPath {
    pub fn of_module(module: &str, model: &str) -> Self {
        Self {
            src: format!("${{GXL_PRJ_ROOT}}/sys/setting/{module}"),
            dst: format!("${{GXL_PRJ_ROOT}}/sys/{model}/mods/{module}/local/",),
            setting: None,
        }
    }

    /// 迁移旧布局本地化目标：`.../sys/mods/<mod>/<model>/local/`
    /// → `.../sys/<model>/mods/<mod>/local/`。
    ///
    /// 已是新布局、或无法识别的路径原样返回（幂等）。
    pub fn migrate_legacy_dst(self) -> Self {
        const MARKER: &str = "/sys/mods/";
        let dst = match self.dst.find(MARKER) {
            Some(idx) => {
                let prefix = &self.dst[..idx];
                let tail = &self.dst[idx + MARKER.len()..];
                let mut parts = tail.splitn(3, '/');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(module), Some(model), Some(rest))
                        if !module.is_empty() && !model.is_empty() && !rest.is_empty() =>
                    {
                        format!("{prefix}/sys/{model}/mods/{module}/{rest}")
                    }
                    _ => self.dst.clone(),
                }
            }
            None => self.dst.clone(),
        };
        Self { dst, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(dst: &str) -> LocalizeVarPath {
        LocalizeVarPath {
            src: "${GXL_PRJ_ROOT}/sys/setting/nginx".to_string(),
            dst: dst.to_string(),
            setting: None,
        }
    }

    #[test]
    fn test_migrate_legacy_dst_old_to_new() {
        let migrated = path("${GXL_PRJ_ROOT}/sys/mods/nginx/v1.0/local/").migrate_legacy_dst();
        assert_eq!(migrated.dst(), "${GXL_PRJ_ROOT}/sys/v1.0/mods/nginx/local/");
        // 幂等：再迁移一次不变
        assert_eq!(migrated.clone().migrate_legacy_dst().dst(), migrated.dst());
    }

    #[test]
    fn test_migrate_legacy_dst_new_layout_unchanged() {
        let p = path("${GXL_PRJ_ROOT}/sys/v1.0/mods/nginx/local/");
        assert_eq!(p.clone().migrate_legacy_dst().dst(), p.dst());
    }

    #[test]
    fn test_migrate_legacy_dst_unrecognized_unchanged() {
        let p = path("/tmp/somewhere/else");
        assert_eq!(p.clone().migrate_legacy_dst().dst(), p.dst());
    }
}
