use super::prelude::*;
use crate::error::MainReason;

use orion_variate::{addr::Address, types::ResourceDownloader};

use super::spec::SysModelSpec;

fn convert_syspec_addr(origin: Address) -> Address {
    match origin {
        Address::Git(git_addr) => {
            if git_addr.path().is_none() {
                Address::from(git_addr.with_path("sys"))
            } else {
                Address::from(git_addr)
            }
        }
        Address::Http(_) => origin,
        Address::Local(_) => origin,
    }
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct SysModelSpecRef {
    name: String,
    addr: Address,
    #[serde(skip)]
    spec: Option<SysModelSpec>,
}
impl SysModelSpecRef {
    pub fn from<S: Into<String>, A: Into<Address>>(name: S, addr: A) -> Self {
        Self {
            name: name.into(),
            addr: addr.into(),
            spec: None,
        }
    }
    pub fn is_update(&self, path: &Path) -> bool {
        path.join(self.name()).exists()
    }

    pub fn load(mut self, path: &Path) -> MainResult<Self> {
        let path = path.join(self.name());
        let mut ctx = OperationContext::doing("load spec ref")
            .with_auto_log()
            .with_mod_path("ops-prj/sys-model");
        ctx.record("path", path.display());
        let spec = SysModelSpec::load_from(&path)?;
        self.spec = Some(spec);
        ctx.mark_suc();
        Ok(self)
    }
}

#[async_trait]
impl InsUpdateable<SysModelSpecRef> for SysModelSpecRef {
    async fn update_local(
        mut self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<Self> {
        let mut ctx = OperationContext::doing("update spec ref")
            .with_auto_log()
            .with_mod_path("ops-prj/sys-model");
        ctx.record("path", path.display());
        let spec_addr = convert_syspec_addr(self.addr.clone());
        let update_v = accessor
            .download_rename(&spec_addr, path, self.name.as_str(), options)
            .await
            .map_err(MainReason::from_addr_error)?;
        let spec = SysModelSpec::load_from(update_v.position())?;
        spec.update_local(accessor, path, options).await?;
        self.spec = Some(spec);
        ctx.mark_suc();
        Ok(self)
    }
}

#[async_trait]
impl SystemLocalizable<SysValuePaths> for SysModelSpecRef {
    async fn sys_localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        if let Some(spec) = &self.spec {
            spec.sys_localize(val_path, options).await?;
            Ok(())
        } else {
            Err(MainReason::logic_detail("miss spec from spec-ref"))
        }
    }
}
