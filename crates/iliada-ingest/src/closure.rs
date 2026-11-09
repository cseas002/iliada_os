//! Soname closure computation (pure, unit-tested).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use iliada_bundle::{LibraryRecord, ResolutionStatus};

use crate::runtime::RuntimeBase;

/// A shared library found in a fetched dependency package.
#[derive(Debug, Clone)]
pub struct LibCandidate {
    pub package: String,
    /// Absolute path to the real file in the staging area.
    pub path: PathBuf,
    pub needed: Vec<String>,
    /// Set when the package was suggested by a resolver (naming rules / AI).
    pub suggested_by: Option<String>,
}

#[derive(Debug, Default)]
pub struct Closure {
    pub records: Vec<LibraryRecord>,
    /// Sonames to copy into `Contents/Libraries`.
    pub bundle: Vec<String>,
    pub unresolved: Vec<String>,
}

/// Starting from `roots` (DT_NEEDED of the primary package), classify every
/// transitively required soname.
pub fn compute_closure(
    roots: &BTreeSet<String>,
    provided: &BTreeSet<String>,
    provided_needed: &BTreeMap<String, Vec<String>>,
    runtime: &RuntimeBase,
    available: &BTreeMap<String, LibCandidate>,
) -> Closure {
    let mut out = Closure::default();
    let mut seen = BTreeSet::new();
    let mut queue: Vec<String> = roots.iter().cloned().collect();
    while let Some(soname) = queue.pop() {
        if !seen.insert(soname.clone()) {
            continue;
        }
        let record = |status, package: Option<&str>, suggested_by: Option<&str>| LibraryRecord {
            soname: soname.clone(),
            status,
            package: package.map(str::to_owned),
            suggested_by: suggested_by.map(str::to_owned),
        };
        if provided.contains(&soname) {
            out.records.push(record(ResolutionStatus::Provided, None, None));
            queue.extend(provided_needed.get(&soname).into_iter().flatten().cloned());
        } else if runtime.provides_soname(&soname) {
            out.records.push(record(ResolutionStatus::Runtime, None, None));
        } else if let Some(c) = available.get(&soname) {
            let status = if c.suggested_by.is_some() { ResolutionStatus::Suggested } else { ResolutionStatus::Bundled };
            out.records.push(record(status, Some(&c.package), c.suggested_by.as_deref()));
            out.bundle.push(soname.clone());
            queue.extend(c.needed.iter().cloned());
        } else {
            out.records.push(record(ResolutionStatus::Unresolved, None, None));
            out.unresolved.push(soname.clone());
        }
    }
    out.records.sort_by(|a, b| a.soname.cmp(&b.soname));
    out.bundle.sort();
    out.unresolved.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(pkg: &str, needed: &[&str], suggested: Option<&str>) -> LibCandidate {
        LibCandidate {
            package: pkg.into(),
            path: PathBuf::from("/x"),
            needed: needed.iter().map(|s| s.to_string()).collect(),
            suggested_by: suggested.map(Into::into),
        }
    }

    #[test]
    fn jq_like_closure() {
        let rt = RuntimeBase::builtin();
        let roots: BTreeSet<String> = ["libjq.so.1", "libc.so.6"].map(String::from).into();
        let mut avail = BTreeMap::new();
        avail.insert("libjq.so.1".to_string(), cand("libjq1", &["libonig.so.5", "libm.so.6"], None));
        avail.insert("libonig.so.5".to_string(), cand("libonig5", &["libc.so.6"], Some("rules")));
        let c = compute_closure(&roots, &BTreeSet::new(), &BTreeMap::new(), &rt, &avail);
        assert_eq!(c.bundle, ["libjq.so.1", "libonig.so.5"]);
        assert!(c.unresolved.is_empty());
        let onig = c.records.iter().find(|r| r.soname == "libonig.so.5").unwrap();
        assert_eq!(onig.status, ResolutionStatus::Suggested);
        assert_eq!(c.records.iter().find(|r| r.soname == "libm.so.6").unwrap().status, ResolutionStatus::Runtime);
    }

    #[test]
    fn reports_unresolved() {
        let rt = RuntimeBase::builtin();
        let roots: BTreeSet<String> = ["libmystery.so.9".to_string()].into();
        let c = compute_closure(&roots, &BTreeSet::new(), &BTreeMap::new(), &rt, &BTreeMap::new());
        assert_eq!(c.unresolved, ["libmystery.so.9"]);
    }

    #[test]
    fn provided_libs_contribute_their_needs() {
        let rt = RuntimeBase::builtin();
        let roots: BTreeSet<String> = ["libown.so.1".to_string()].into();
        let provided: BTreeSet<String> = ["libown.so.1".to_string()].into();
        let pn: BTreeMap<String, Vec<String>> = [("libown.so.1".to_string(), vec!["libdep.so.2".to_string()])].into();
        let c = compute_closure(&roots, &provided, &pn, &rt, &BTreeMap::new());
        assert_eq!(c.unresolved, ["libdep.so.2"]);
    }
}
