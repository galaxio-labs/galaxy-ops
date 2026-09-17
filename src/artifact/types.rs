use derive_more::From;
use getset::Getters;
use orion_variate::addr::{Address, GitRepository, HttpResource, LocalPath};
use serde_derive::{Deserialize, Serialize};

use crate::error::{MainError, MainReason, MainResult};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum OsType {
    MacOs,
    Ubuntu,
}

#[derive(Debug, Clone, Getters)]
pub struct BinPackage {
    #[getset(get = "pub")]
    name: String,
    #[getset(get = "pub")]
    addr: Address,
}

#[derive(Debug, Clone, Getters)]
pub struct GitPackage {
    #[getset(get = "pub")]
    name: String,
    #[getset(get = "pub")]
    addr: GitRepository,
}
#[derive(Debug, Clone, From)]
pub enum PackageType {
    Bin(BinPackage),
    Git(GitPackage),
}

fn unsupported_pkg_err(input: &str) -> MainError {
    MainReason::logic_detail(format!(
        "unsupported package address: `{input}`. Expected a `.tar.gz` archive, a `.git` repository URL, or an http(s) archive URL"
    ))
}

pub fn convert_addr(input: &str) -> MainResult<Address> {
    if input.starts_with("http") {
        if input.ends_with(".git") {
            Ok(Address::Git(GitRepository::from(input.to_string())))
        } else if input.ends_with(".tar.gz") {
            Ok(Address::Http(HttpResource::from(input.to_string())))
        } else {
            Err(unsupported_pkg_err(input))
        }
    } else if input.starts_with("git@") || input.ends_with(".git") {
        Ok(Address::Git(GitRepository::from(input.to_string())))
    } else if input.ends_with(".tar.gz") {
        Ok(Address::Local(LocalPath::from(input)))
    } else {
        Err(unsupported_pkg_err(input))
    }
}
// input :
// /Users/dayu/ds-build/mac-devkit-0.1.5.tar.gz
// https://github.com/galaxio-labs/galaxy-flow.git
// git@github.com:galaxio-labs/galaxy-flow.git
// https://github.com/galaxio-labs/galaxy-flow/releases/download/v0.8.4/galaxy-flow-v0.8.4-aarch64-apple-darwin.tar.gz
pub fn build_pkg(input: &str) -> MainResult<PackageType> {
    let addr_type = convert_addr(input)?;

    match addr_type {
        Address::Git(git_addr) => {
            let name = extract_name_from_url(input, ".git");
            Ok(PackageType::Git(GitPackage {
                name,
                addr: git_addr,
            }))
        }
        Address::Http(http_addr) => {
            let name = extract_name_from_url(input, ".tar.gz");
            Ok(PackageType::Bin(BinPackage {
                name,
                addr: Address::Http(http_addr),
            }))
        }
        Address::Local(local_addr) => {
            let name = extract_name_from_url(input, ".tar.gz");
            Ok(PackageType::Bin(BinPackage {
                name,
                addr: Address::Local(local_addr),
            }))
        }
    }
}

fn extract_name_from_url(url: &str, suffix: &str) -> String {
    url.split('/').next_back().unwrap().replace(suffix, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_pkg_bin_local() {
        let input = "/Users/dayu/ds-build/mac-devkit-0.1.5.tar.gz";
        let pkg = build_pkg(input).unwrap();
        match pkg {
            PackageType::Bin(bin_pkg) => {
                assert_eq!(bin_pkg.name(), "mac-devkit-0.1.5");
                assert!(matches!(bin_pkg.addr(), Address::Local(_)));
            }
            _ => panic!("Expected BinPackage"),
        }
    }

    #[test]
    fn test_build_pkg_bin_remote() {
        let input = "https://github.com/galaxio-labs/galaxy-flow/releases/download/v0.8.4/galaxy-flow-v0.8.4-aarch64-apple-darwin.tar.gz";
        let pkg = build_pkg(input).unwrap();
        match pkg {
            PackageType::Bin(bin_pkg) => {
                assert_eq!(bin_pkg.name(), "galaxy-flow-v0.8.4-aarch64-apple-darwin");
                assert_eq!(bin_pkg.addr(), &Address::from(HttpResource::from(input)));
            }
            _ => panic!("Expected BinPackage"),
        }
    }

    #[test]
    fn test_build_pkg_git_https() {
        let input = "https://github.com/galaxio-labs/galaxy-flow.git";
        let pkg = build_pkg(input).unwrap();
        match pkg {
            PackageType::Git(git_pkg) => {
                assert_eq!(git_pkg.name(), "galaxy-flow");
                assert_eq!(git_pkg.addr().repo(), input);
            }
            _ => panic!("Expected GitPackage"),
        }
    }

    #[test]
    fn test_build_pkg_git_ssh() {
        let input = "git@github.com:galaxio-labs/galaxy-flow.git";
        let pkg = build_pkg(input).unwrap();
        match pkg {
            PackageType::Git(git_pkg) => {
                assert_eq!(git_pkg.name(), "galaxy-flow");
                assert_eq!(git_pkg.addr().repo(), input);
            }
            _ => panic!("Expected GitPackage"),
        }
    }

    #[test]
    fn test_build_pkg_unsupported() {
        let input = "invalid_input";
        let err = build_pkg(input).unwrap_err();
        assert!(
            err.detail()
                .as_deref()
                .is_some_and(|d| d.contains("unsupported package address"))
        );
    }
}

#[cfg(test)]
mod convert_addr_tests {
    use super::*;

    #[test]
    fn test_convert_addr_local() {
        let input = "/Users/dayu/ds-build/mac-devkit-0.1.5.tar.gz";
        let addr = convert_addr(input).unwrap();
        assert!(matches!(addr, Address::Local(_)));
    }

    #[test]
    fn test_convert_addr_http_tar() {
        let input = "https://github.com/galaxio-labs/galaxy-flow/releases/download/v0.8.4/galaxy-flow-v0.8.4-aarch64-apple-darwin.tar.gz";
        let addr = convert_addr(input).unwrap();
        assert!(matches!(addr, Address::Http(_)));
    }

    #[test]
    fn test_convert_addr_https_git() {
        let input = "https://github.com/galaxio-labs/galaxy-flow.git";
        let addr = convert_addr(input).unwrap();
        assert!(matches!(addr, Address::Git(_)));
    }

    #[test]
    fn test_convert_addr_ssh_git() {
        let input = "git@github.com:galaxio-labs/galaxy-flow.git";
        let addr = convert_addr(input).unwrap();
        assert!(matches!(addr, Address::Git(_)));
    }

    #[test]
    fn test_convert_addr_local_git() {
        let input = "/home/user/repo.git";
        let addr = convert_addr(input).unwrap();
        assert!(matches!(addr, Address::Git(_)));
    }

    #[test]
    fn test_convert_addr_unsupported() {
        let input = "invalid_input";
        let err = convert_addr(input).unwrap_err();
        assert!(
            err.detail()
                .as_deref()
                .is_some_and(|d| d.contains("unsupported package address"))
        );
    }
}
