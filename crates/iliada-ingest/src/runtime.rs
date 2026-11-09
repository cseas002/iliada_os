//! The Iliada runtime base: sonames/packages the platform guarantees.

use std::collections::BTreeSet;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBase {
    pub name: String,
    pub sonames: BTreeSet<String>,
    pub packages: BTreeSet<String>,
}

impl RuntimeBase {
    pub fn builtin() -> Self {
        toml::from_str(include_str!("../runtime/iliada-base-1.toml")).expect("embedded runtime base is valid TOML")
    }

    pub fn provides_soname(&self, soname: &str) -> bool {
        self.sonames.contains(soname)
    }

    pub fn provides_package(&self, package: &str) -> bool {
        self.packages.contains(package)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtin_parses() {
        let rt = super::RuntimeBase::builtin();
        assert_eq!(rt.name, "iliada-base-1");
        assert!(rt.provides_soname("libc.so.6"));
        assert!(rt.provides_package("libc6"));
    }
}
