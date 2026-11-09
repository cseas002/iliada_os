//! The end-to-end ingestion pipeline. See the crate docs for the data flow.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use iliada_bundle::{
    Bundle, BundleLayout, Info, IsolationTier, LibraryRecord, PackageRecord, PackageRole, Provenance, SourceRecord,
};
use iliada_types::AppId;

use crate::closure::{Closure, LibCandidate, compute_closure};
use crate::control::Control;
use crate::deb::open_deb;
use crate::desktop::{self, DesktopEntry};
use crate::elf::{host_deb_arch, multiarch_triplet, provided_sonames, scan_tree};
use crate::extract::{ExtractLimits, extract_tar, symlink_stays_inside};
use crate::fetch::Fetcher;
use crate::index::{IndexLocation, PackageIndex};
use crate::infer::infer_permissions;
use crate::launcher::write_launcher;
use crate::resolvers::{LibraryResolver, ResolveContext};
use crate::runtime::RuntimeBase;
use crate::sandbox::{Confinement, run_confined};

/// What to ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A `.deb` at an https:// URL, file:// URL or local path.
    Url(String),
    /// A package name looked up in the configured Debian suite (`debian:jq`).
    Debian(String),
}

impl Source {
    pub fn parse(s: &str) -> Self {
        match s.strip_prefix("debian:") {
            Some(name) => Source::Debian(name.to_owned()),
            None => Source::Url(s.to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestConfig {
    pub out_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub mirror: String,
    pub suite: String,
    pub components: Vec<String>,
    /// Target Debian architecture; defaults to the package's own (or the host's for `Architecture: all`).
    pub arch: Option<String>,
    pub vendor_prefix: String,
    pub exec_override: Option<String>,
    pub isolation: IsolationTier,
    /// Mark the bundle as needing the internet.
    pub network: bool,
    /// The app's name (bundle folder and launcher), overriding the upstream one
    /// ("Firefox" rather than "Firefox ESR").
    pub display_name: Option<String>,
    /// Extra packages unpacked into the app itself, next to the primary one
    /// (programs that live outside the library path, plugins, data).
    pub include: Vec<String>,
    /// Arguments always passed to the entry point.
    pub args: Vec<String>,
    /// Environment always set for the app.
    pub env: std::collections::BTreeMap<String, String>,
    pub max_download_bytes: u64,
    pub max_unpacked_bytes: u64,
    /// Follow dependency edges into non-`lib*` packages too.
    pub full_closure: bool,
    pub max_dependency_packages: usize,
    /// Consult the Debian package index for dependencies.
    pub use_index: bool,
    /// Replace an existing bundle with the same name.
    pub force: bool,
    pub allow_http: bool,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            out_dir: PathBuf::from("out"),
            cache_dir: default_cache_dir(),
            mirror: "https://deb.debian.org/debian".into(),
            suite: "trixie".into(),
            components: vec!["main".into()],
            arch: None,
            vendor_prefix: "org.debian".into(),
            exec_override: None,
            isolation: IsolationTier::Namespace,
            network: false,
            display_name: None,
            include: vec![],
            args: vec![],
            env: Default::default(),
            max_download_bytes: 1 << 30,
            max_unpacked_bytes: 4 << 30,
            full_closure: false,
            max_dependency_packages: 600,
            use_index: true,
            force: false,
            allow_http: false,
        }
    }
}

pub fn default_cache_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("iliada-ingest");
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/iliada-ingest")).unwrap_or_else(|| ".cache".into())
}

#[derive(Debug, Clone)]
pub struct IngestReport {
    pub bundle: PathBuf,
    pub app_id: AppId,
    pub version: String,
    pub executable: PathBuf,
    pub libraries: Vec<LibraryRecord>,
    pub packages: Vec<PackageRecord>,
    pub unresolved: Vec<String>,
    pub skipped_dependencies: Vec<String>,
    pub missing_dependencies: Vec<String>,
    pub confinement: Confinement,
    pub runpaths_patched: usize,
}

#[derive(Debug)]
pub struct Ingestor {
    cfg: IngestConfig,
    fetcher: Fetcher,
    runtime: RuntimeBase,
    resolvers: Vec<Box<dyn LibraryResolver>>,
    indexes: tokio::sync::Mutex<HashMap<String, Arc<PackageIndex>>>,
}

#[derive(Debug, Default)]
struct DepState {
    fetched: BTreeMap<String, PackageRecord>,
    /// Arch-independent data packages (`*-common`, `*-data`) whose files are merged into Root.
    data: BTreeSet<String>,
    /// Sonames needed by plugin modules (gdk-pixbuf loaders, GIO/GTK modules…)
    /// that live in `usr/lib/<triplet>/<dir>/` of each package.
    plugin_needed: BTreeMap<String, BTreeSet<String>>,
    /// Every shared library file each package ships on the loader path,
    /// including ones loaded at runtime with dlopen (NSS modules, codecs…).
    pkg_libs: BTreeMap<String, Vec<(String, PathBuf)>>,
    available: BTreeMap<String, LibCandidate>,
    skipped: BTreeSet<String>,
    missing: BTreeSet<String>,
}

impl Ingestor {
    pub fn new(cfg: IngestConfig, resolvers: Vec<Box<dyn LibraryResolver>>) -> Result<Self> {
        let fetcher = Fetcher::new(cfg.max_download_bytes, cfg.allow_http)?;
        Ok(Self { cfg, fetcher, runtime: RuntimeBase::builtin(), resolvers, indexes: Default::default() })
    }

    pub fn config(&self) -> &IngestConfig {
        &self.cfg
    }

    pub fn fetcher(&self) -> &Fetcher {
        &self.fetcher
    }

    /// Drop cached package indexes (watch mode calls this every cycle).
    pub async fn invalidate_indexes(&self) {
        self.indexes.lock().await.clear();
    }

    pub async fn index(&self, arch: &str) -> Result<Arc<PackageIndex>> {
        let mut guard = self.indexes.lock().await;
        if let Some(i) = guard.get(arch) {
            return Ok(i.clone());
        }
        let loc = IndexLocation {
            mirror: self.cfg.mirror.clone(),
            suite: self.cfg.suite.clone(),
            components: self.cfg.components.clone(),
            arch: arch.to_owned(),
        };
        let idx = Arc::new(PackageIndex::load(&self.fetcher, &self.cfg.cache_dir, &loc).await?);
        guard.insert(arch.to_owned(), idx.clone());
        Ok(idx)
    }

    fn target_arch(&self) -> String {
        self.cfg.arch.clone().unwrap_or_else(|| host_deb_arch().to_owned())
    }

    pub async fn ingest(&self, source: &Source) -> Result<IngestReport> {
        // 1. Fetch the primary package.
        let (url, expected_sha) = match source {
            Source::Url(u) => (u.clone(), None),
            Source::Debian(name) => {
                let idx = self.index(&self.target_arch()).await?;
                let e = idx.get(name).with_context(|| format!("package {name:?} not found in {}", self.cfg.suite))?;
                (idx.url_for(e), Some(e.sha256.clone()))
            }
        };
        tracing::info!(%url, "fetching package");
        let primary = self.fetcher.fetch(&url).await?;
        if let Some(exp) = expected_sha {
            ensure!(primary.sha256 == exp, "checksum mismatch for {url}: index says {exp}, got {}", primary.sha256);
        }

        // 2. Extract into a private staging area under the output directory.
        fs::create_dir_all(&self.cfg.out_dir)?;
        let staging = tempfile::Builder::new().prefix(".iliada-ingest-").tempdir_in(&self.cfg.out_dir)?;
        let layout = BundleLayout::new(staging.path().join("bundle.app"));
        layout.create_dirs()?;
        let limits = ExtractLimits { max_total_bytes: self.cfg.max_unpacked_bytes, ..Default::default() };
        let (bytes, root, unpack_limit) = (primary.bytes, layout.fhs_root(), self.cfg.max_unpacked_bytes);
        let ((control, report), confinement) = run_confined(staging.path().to_owned(), move || {
            let deb = open_deb(&bytes, unpack_limit)?;
            let report = extract_tar(&deb.data_tar, &root, limits)?;
            Ok((deb.control, report))
        })
        .await?;
        if !report.skipped.is_empty() {
            tracing::warn!(skipped = ?report.skipped, "special files were not extracted");
        }
        let package = control.package().context("control file has no Package field")?.to_owned();
        let version = control.version().context("control file has no Version field")?.to_owned();
        let deb_arch = control.architecture().unwrap_or("all").to_owned();
        let arch = if deb_arch == "all" { self.target_arch() } else { deb_arch };
        let triplet = multiarch_triplet(&arch);
        tracing::info!(%package, %version, %arch, files = report.files, ?confinement, "extracted");

        // Included packages become part of the app's own tree.
        let mut deps = DepState::default();
        let mut dep_roots = control.depends();
        let mut included = Vec::new();
        if !self.cfg.include.is_empty() {
            let idx = self.index(&arch).await?;
            for name in &self.cfg.include {
                let e = idx.get(name).with_context(|| format!("included package {name:?} not found"))?;
                let url = idx.url_for(e);
                tracing::info!(package = %e.name, "including package in the app");
                let f = self.fetcher.fetch(&url).await?;
                ensure!(f.sha256 == e.sha256, "checksum mismatch for {url}");
                let (bytes, root, limit) = (f.bytes, layout.fhs_root(), self.cfg.max_unpacked_bytes);
                run_confined(staging.path().to_owned(), move || {
                    let deb = open_deb(&bytes, limit)?;
                    extract_tar(&deb.data_tar, &root, limits)?;
                    Ok(())
                })
                .await?;
                dep_roots.extend(e.depends.iter().cloned());
                let rec = PackageRecord {
                    name: e.name.clone(),
                    version: e.version.clone(),
                    url,
                    sha256: f.sha256,
                    role: PackageRole::Dependency,
                };
                // Already in the app: never fetch it again as a dependency.
                deps.fetched.insert(e.name.clone(), rec.clone());
                included.push(rec);
            }
        }

        // 3. Analyse what the payload needs.
        let root = layout.fhs_root();
        let objects = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || scan_tree(&root)).await??
        };
        let provided = provided_sonames(&root, &objects);
        let provided_needed: BTreeMap<String, Vec<String>> =
            objects.iter().filter_map(|o| o.soname.clone().map(|s| (s, o.needed.clone()))).collect();
        let roots: BTreeSet<String> = objects.iter().flat_map(|o| o.needed.iter().cloned()).collect();
        let desktop = desktop::find(&root, &package);
        let exe_rel = choose_executable(&root, &package, desktop.as_ref(), self.cfg.exec_override.as_deref())?;

        // 4. Resolve dependencies through the package index, then resolvers.
        let mut index = None;
        let needs_more = roots.iter().any(|s| !provided.contains(s) && !self.runtime.provides_soname(s));
        if needs_more && self.cfg.use_index {
            let idx = self.index(&arch).await?;
            self.walk_dependencies(&idx, dep_roots, None, false, staging.path(), &mut deps).await?;
            index = Some(idx);
        }
        let mut closure = compute_closure(&roots, &provided, &provided_needed, &self.runtime, &deps.available);
        if let Some(idx) = &index {
            // Second chance: some library packages are not named lib* (zlib1g).
            // Retry skipped direct dependencies before asking any resolver.
            let retry: Vec<String> = deps.skipped.iter().filter(|n| n.contains("lib")).cloned().collect();
            for name in retry {
                if closure.unresolved.is_empty() {
                    break;
                }
                deps.skipped.remove(&name);
                self.walk_dependencies(idx, vec![vec![name]], None, true, staging.path(), &mut deps).await?;
                closure = compute_closure(&roots, &provided, &provided_needed, &self.runtime, &deps.available);
            }
        }
        if let Some(idx) = &index {
            self.consult_resolvers(
                idx,
                &control,
                &arch,
                staging.path(),
                &roots,
                &provided,
                &provided_needed,
                &mut deps,
                &mut closure,
            )
            .await?;
        }
        // Plugin modules of the packages in use need their own libraries too,
        // as far as they are already available: plugins are optional (e.g. a
        // Qt bridge shipped next to a GTK library must not pull in all of Qt).
        let in_use = |c: &Closure| -> BTreeSet<String> {
            c.records.iter().filter_map(|r| r.package.clone()).chain(deps.data.iter().cloned()).collect()
        };
        let mut roots = roots;
        loop {
            let extra: BTreeSet<String> = in_use(&closure)
                .iter()
                .filter_map(|p| deps.plugin_needed.get(p))
                .flatten()
                .filter(|s| !roots.contains(*s))
                .filter(|s| deps.available.contains_key(*s) || provided.contains(*s) || self.runtime.provides_soname(s))
                .cloned()
                .collect();
            if extra.is_empty() {
                break;
            }
            roots.extend(extra);
            closure = compute_closure(&roots, &provided, &provided_needed, &self.runtime, &deps.available);
        }
        for s in &closure.unresolved {
            tracing::warn!(soname = %s, "unresolved shared library; the app may fail to start");
        }

        // 5. Assemble the bundle.
        for soname in &closure.bundle {
            let src = &deps.available[soname].path;
            let dst = layout.libraries().join(soname);
            fs::copy(src, &dst).with_context(|| format!("copying {}", src.display()))?;
            fs::set_permissions(&dst, fs::Permissions::from_mode(0o755))?;
        }
        // The rest of each library package in use: libraries it loads at
        // runtime (dlopen) never show up as DT_NEEDED anywhere.
        for pkg in in_use(&closure) {
            for (name, src) in deps.pkg_libs.get(&pkg).into_iter().flatten() {
                let dst = layout.libraries().join(name);
                if fs::symlink_metadata(&dst).is_err() && !self.runtime.provides_soname(name) {
                    fs::copy(src, &dst).with_context(|| format!("copying {}", src.display()))?;
                    fs::set_permissions(&dst, fs::Permissions::from_mode(0o755))?;
                }
            }
        }
        // Shared data (schemas, locales, icons, GTK resources) of every package
        // that contributed libraries, plus data packages, joins the app's tree.
        let used: BTreeSet<String> = closure.records.iter().filter_map(|r| r.package.clone()).collect();
        let mut share_from: Vec<&String> = deps.data.iter().collect();
        share_from.extend(used.iter());
        for name in share_from {
            let dep = staging.path().join("deps").join(name);
            merge_share(&dep.join("usr/share"), &root.join("usr/share"))?;
            // Licence notices travel with the binaries (many licences require it).
            merge_copyright(&dep.join("usr/share/doc"), &root.join("usr/share/doc"))?;
            // Plugin directories (gdk-pixbuf loaders, GIO and GTK modules, …) are
            // loaded from their compiled-in paths, so they keep their place in Root.
            if let Some(t) = triplet {
                merge_subdirs(&dep.join("usr/lib").join(t), &root.join("usr/lib").join(t))?;
            }
        }
        let schemas = compile_schemas(staging.path(), &root).await?;

        let exe_name = exe_rel.file_name().context("executable has no file name")?.to_string_lossy().into_owned();
        write_launcher(&layout, &exe_name, &exe_rel, &self.cfg.args, triplet)?;
        // Binaries keep their own RUNPATH (apps such as Audacity and Inkscape
        // find private libraries through it); the sandbox and the launcher
        // add the bundled libraries via LD_LIBRARY_PATH.
        let runpaths_patched = 0;
        let icon = copy_resources(&layout, &root, desktop.as_ref())?;

        let mut used_packages: BTreeSet<&str> = used.iter().map(String::as_str).collect();
        used_packages.extend(deps.data.iter().map(String::as_str));
        let mut packages = vec![PackageRecord {
            name: package.clone(),
            version: version.clone(),
            url: url.clone(),
            sha256: primary.sha256.clone(),
            role: PackageRole::Primary,
        }];
        packages.extend(included.iter().cloned());
        packages.extend(
            deps.fetched
                .values()
                .filter(|p| used_packages.contains(p.name.as_str()) && !included.iter().any(|i| i.name == p.name))
                .cloned(),
        );

        let app_id = AppId::from_upstream(&self.cfg.vendor_prefix, &package)?;
        let info = Info {
            id: app_id.clone(),
            name: self
                .cfg
                .display_name
                .clone()
                .or_else(|| desktop.as_ref().and_then(|d| d.name.clone()))
                .unwrap_or_else(|| package.clone()),
            version: version.clone(),
            executable: exe_name,
            entry: Some(exe_rel.to_string_lossy().into_owned()),
            args: self.cfg.args.clone(),
            env: self.cfg.env.clone(),
            icon,
            runtime: self.runtime.name.clone(),
            isolation: self.cfg.isolation,
            network: self.cfg.network,
            categories: desktop.as_ref().map(|d| d.categories.clone()).unwrap_or_default(),
            mime_types: desktop.as_ref().map(|d| d.mime_types.clone()).unwrap_or_default(),
        };
        let permissions = infer_permissions(closure.records.iter().map(|r| r.soname.as_str()));
        let provenance = Provenance {
            generator: concat!("iliada-ingest ", env!("CARGO_PKG_VERSION")).into(),
            created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            source: SourceRecord { url, sha256: primary.sha256, format: "deb".into() },
            packages: packages.clone(),
            libraries: closure.records.clone(),
        };
        Bundle::write_manifests(&layout, &info, &permissions, Some(&provenance))?;
        Bundle::open(layout.root()).context("freshly written bundle failed validation")?;

        // 6. Publish atomically (same filesystem: rename).
        let dest = BundleLayout::in_dir(&self.cfg.out_dir, &info.name);
        if dest.root().exists() {
            if !self.cfg.force {
                bail!("{} already exists (use --force to replace it)", dest.root().display());
            }
            fs::remove_dir_all(dest.root())?;
        }
        // Apps belong to their users: members of the app group may move a
        // bundle to the Trash and delete it, as on macOS.
        make_group_writable(layout.root())?;
        fs::rename(layout.root(), dest.root())?;

        if schemas {
            tracing::info!("compiled GSettings schemas");
        }
        Ok(IngestReport {
            bundle: dest.root().to_owned(),
            app_id,
            version,
            executable: exe_rel,
            libraries: closure.records,
            packages,
            unresolved: closure.unresolved,
            skipped_dependencies: deps.skipped.into_iter().collect(),
            missing_dependencies: deps.missing.into_iter().collect(),
            confinement,
            runpaths_patched,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn consult_resolvers(
        &self,
        idx: &PackageIndex,
        control: &Control,
        arch: &str,
        staging: &Path,
        roots: &BTreeSet<String>,
        provided: &BTreeSet<String>,
        provided_needed: &BTreeMap<String, Vec<String>>,
        deps: &mut DepState,
        closure: &mut Closure,
    ) -> Result<()> {
        for resolver in &self.resolvers {
            if closure.unresolved.is_empty() {
                break;
            }
            let ctx = ResolveContext {
                package: control.package().unwrap_or_default().to_owned(),
                version: control.version().unwrap_or_default().to_owned(),
                arch: arch.to_owned(),
                suite: self.cfg.suite.clone(),
                unresolved: closure.unresolved.clone(),
                already_fetched: deps.fetched.keys().cloned().collect(),
            };
            let suggestions = match resolver.suggest(&ctx).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(resolver = resolver.name(), error = %e, "resolver failed; skipping it");
                    continue;
                }
            };
            for s in suggestions {
                if deps.available.contains_key(&s.soname) || deps.fetched.contains_key(&s.package) {
                    continue;
                }
                if idx.get(&s.package).is_none() {
                    tracing::debug!(resolver = resolver.name(), package = %s.package, "suggested package does not exist");
                    continue;
                }
                self.walk_dependencies(idx, vec![vec![s.package.clone()]], Some(resolver.name()), true, staging, deps)
                    .await?;
                if deps.available.contains_key(&s.soname) {
                    tracing::info!(resolver = resolver.name(), soname = %s.soname, package = %s.package, rationale = %s.rationale, "suggestion verified");
                } else {
                    tracing::warn!(resolver = resolver.name(), soname = %s.soname, package = %s.package, "suggestion rejected: package does not ship that soname");
                }
            }
            *closure = compute_closure(roots, provided, provided_needed, &self.runtime, &deps.available);
        }
        Ok(())
    }

    /// Breadth-first walk of Depends edges. Only `lib*` packages are followed
    /// (unless `full_closure`, or `force_roots` for the root groups);
    /// packages in the runtime base are never fetched.
    async fn walk_dependencies(
        &self,
        idx: &PackageIndex,
        roots: Vec<Vec<String>>,
        suggested_by: Option<&str>,
        force_roots: bool,
        staging: &Path,
        st: &mut DepState,
    ) -> Result<()> {
        let mut queue: VecDeque<(Vec<String>, bool)> = roots.into_iter().map(|g| (g, true)).collect();
        while let Some((group, is_root)) = queue.pop_front() {
            if group.iter().any(|n| self.runtime.provides_package(n)) {
                continue;
            }
            let Some(entry) = group.iter().find_map(|n| idx.get(n)) else {
                st.missing.insert(group.join(" | "));
                continue;
            };
            if st.fetched.contains_key(&entry.name) || st.skipped.contains(&entry.name) {
                continue;
            }
            let forced = is_root && force_roots;
            let data = is_data_package(&entry.name);
            if !forced && !self.cfg.full_closure && !entry.name.starts_with("lib") && !data {
                st.skipped.insert(entry.name.clone());
                continue;
            }
            if st.fetched.len() >= self.cfg.max_dependency_packages {
                bail!("dependency closure exceeds {} packages", self.cfg.max_dependency_packages);
            }
            let url = idx.url_for(entry);
            tracing::info!(package = %entry.name, version = %entry.version, "fetching dependency");
            let f = self.fetcher.fetch(&url).await?;
            ensure!(f.sha256 == entry.sha256, "checksum mismatch for {url}");

            let dest = staging.join("deps").join(&entry.name);
            let (bytes, d, limit) = (f.bytes, dest.clone(), self.cfg.max_unpacked_bytes);
            let limits = ExtractLimits { max_total_bytes: limit, ..Default::default() };
            run_confined(staging.to_owned(), move || {
                let deb = open_deb(&bytes, limit)?;
                extract_tar(&deb.data_tar, &d, limits)?;
                Ok(())
            })
            .await?;
            let objs = {
                let d = dest.clone();
                tokio::task::spawn_blocking(move || scan_tree(&d)).await??
            };
            let suggested = if is_root { suggested_by.map(str::to_owned) } else { None };
            let on_loader_path = |o: &&crate::elf::ElfObject| {
                !o.is_executable
                    && !is_plugin_path(&o.rel_path)
                    && o.rel_path.file_name().is_some_and(|n| n.to_string_lossy().contains(".so"))
            };
            let mut libs = Vec::new();
            for o in objs.iter().filter(on_loader_path) {
                let path = dest.join(&o.rel_path);
                let file = o.rel_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                if let Some(so) = &o.soname
                    && *so != file
                {
                    libs.push((so.clone(), path.clone()));
                }
                libs.push((file, path));
            }
            if !libs.is_empty() {
                st.pkg_libs.insert(entry.name.clone(), libs);
            }
            // Needs of plugins and of runtime-loaded libraries are best-effort.
            let plugins: BTreeSet<String> = objs
                .iter()
                .filter(|o| is_plugin_path(&o.rel_path) || on_loader_path(o))
                .flat_map(|o| o.needed.iter().cloned())
                .collect();
            if !plugins.is_empty() {
                st.plugin_needed.insert(entry.name.clone(), plugins);
            }
            for o in objs.into_iter().filter(|o| o.is_shared_lib) {
                if let Some(soname) = o.soname.clone() {
                    st.available.entry(soname).or_insert(LibCandidate {
                        package: entry.name.clone(),
                        path: dest.join(&o.rel_path),
                        needed: o.needed,
                        suggested_by: suggested.clone(),
                    });
                }
            }
            if data {
                st.data.insert(entry.name.clone());
            }
            st.fetched.insert(
                entry.name.clone(),
                PackageRecord {
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    url,
                    sha256: f.sha256,
                    role: PackageRole::Dependency,
                },
            );
            queue.extend(entry.depends.iter().cloned().map(|g| (g, false)));
        }
        Ok(())
    }
}

const BIN_DIRS: [&str; 5] = ["usr/bin", "bin", "usr/games", "usr/sbin", "sbin"];

/// Add group read/write throughout a bundle. The group itself is inherited
/// from /Applications (setgid), so no special bits are set here (Nix's build
/// sandbox, rightly, refuses setgid). Symlinks are skipped.
fn make_group_writable(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode();
    if meta.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode((mode & 0o777) | 0o070))?;
        for e in fs::read_dir(path)?.flatten() {
            make_group_writable(&e.path())?;
        }
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode((mode & 0o777) | 0o060))?;
    }
    Ok(())
}

/// Debian's convention for architecture-independent support data.
fn is_data_package(name: &str) -> bool {
    name.ends_with("-common") || name.ends_with("-data") || name == "gsettings-desktop-schemas"
}

/// Is this ELF a plugin module (`usr/lib/<triplet>/<dir>/…`) rather than a
/// library on the loader path?
fn is_plugin_path(rel: &Path) -> bool {
    let c: Vec<_> = rel.components().collect();
    c.len() >= 5
        && c[0].as_os_str() == "usr"
        && c[1].as_os_str() == "lib"
        && c[2].as_os_str().to_string_lossy().ends_with("-linux-gnu")
}

/// Copy each package's `usr/share/doc/<pkg>/copyright` (documentation is
/// otherwise skipped). A doc directory that is a symlink to another package's
/// is kept as a symlink.
fn merge_copyright(src: &Path, dst: &Path) -> Result<()> {
    let Ok(rd) = fs::read_dir(src) else { return Ok(()) };
    for e in rd.flatten() {
        let target = dst.join(e.file_name());
        let meta = fs::symlink_metadata(e.path())?;
        if meta.file_type().is_symlink() {
            if fs::symlink_metadata(&target).is_err() {
                fs::create_dir_all(dst)?;
                std::os::unix::fs::symlink(fs::read_link(e.path())?, &target)?;
            }
        } else if meta.is_dir() {
            let copyright = e.path().join("copyright");
            if fs::symlink_metadata(&copyright).is_ok() {
                fs::create_dir_all(&target)?;
                copy_no_overwrite(&copyright, &target.join("copyright"))?;
            }
        }
    }
    Ok(())
}

/// Copy only the subdirectories of `src` into `dst` (plain files at the top
/// level are shared libraries, which go to Contents/Libraries instead).
fn merge_subdirs(src: &Path, dst: &Path) -> Result<()> {
    let Ok(rd) = fs::read_dir(src) else { return Ok(()) };
    for e in rd.flatten() {
        let is_dir = fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir());
        if is_dir {
            copy_no_overwrite(&e.path(), &dst.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Directories under usr/share that are documentation, not runtime data.
const SKIP_SHARE: [&str; 5] = ["doc", "man", "info", "lintian", "bug"];

/// Copy `src` into `dst` without overwriting what the app itself ships.
/// Symlinks are recreated verbatim (they were validated at extraction time).
fn merge_share(src: &Path, dst: &Path) -> Result<()> {
    let Ok(rd) = fs::read_dir(src) else { return Ok(()) };
    for e in rd.flatten() {
        let name = e.file_name();
        if dst.ends_with("usr/share") && SKIP_SHARE.iter().any(|s| name == *s) {
            continue;
        }
        copy_no_overwrite(&e.path(), &dst.join(&name))?;
    }
    Ok(())
}

fn copy_no_overwrite(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        if fs::symlink_metadata(dst).is_err() {
            std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        }
    } else if meta.is_dir() {
        match fs::symlink_metadata(dst) {
            Ok(m) if !m.is_dir() => return Ok(()),
            Ok(_) => {}
            Err(_) => fs::create_dir_all(dst)?,
        }
        for e in fs::read_dir(src)?.flatten() {
            copy_no_overwrite(&e.path(), &dst.join(e.file_name()))?;
        }
    } else if meta.is_file() && fs::symlink_metadata(dst).is_err() {
        fs::copy(src, dst)?;
    }
    Ok(())
}

/// Debian compiles GSettings schemas in a postinst trigger, which we never
/// run. Compile them here (confined like extraction) so GLib apps start.
async fn compile_schemas(staging: &Path, root: &Path) -> Result<bool> {
    let dir = root.join("usr/share/glib-2.0/schemas");
    let has_xml = fs::read_dir(&dir)
        .map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "xml")))
        .unwrap_or(false);
    if !has_xml {
        return Ok(false);
    }
    let Ok(tool) = crate::launcher::which("glib-compile-schemas") else {
        tracing::warn!("glib-compile-schemas not found; GLib apps in this bundle may refuse to start");
        return Ok(false);
    };
    let (ok, _) =
        run_confined(staging.to_owned(), move || Ok(std::process::Command::new(tool).arg(&dir).status()?.success()))
            .await?;
    if !ok {
        tracing::warn!("glib-compile-schemas failed");
    }
    Ok(ok)
}

/// Resolve `rel` inside `root` to a regular file, following symlinks
/// *lexically* (absolute targets re-rooted), never via the host filesystem.
pub fn resolve_in_root(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut cur = rel.to_owned();
    for _ in 0..16 {
        let meta = fs::symlink_metadata(root.join(&cur)).ok()?;
        if meta.is_file() {
            return Some(cur);
        }
        if !meta.file_type().is_symlink() {
            return None;
        }
        let target = fs::read_link(root.join(&cur)).ok()?;
        if !symlink_stays_inside(&cur, &target) {
            return None;
        }
        let joined = if target.is_absolute() { target } else { cur.parent().unwrap_or(Path::new("")).join(target) };
        let mut norm = PathBuf::new();
        for c in joined.components() {
            match c {
                std::path::Component::Normal(p) => norm.push(p),
                std::path::Component::ParentDir => {
                    norm.pop();
                }
                _ => {}
            }
        }
        cur = norm;
    }
    None
}

fn is_executable_in_root(root: &Path, rel: &Path) -> Option<PathBuf> {
    let real = resolve_in_root(root, rel)?;
    let mode = fs::metadata(root.join(&real)).ok()?.permissions().mode();
    (mode & 0o111 != 0).then_some(real)
}

fn choose_executable(
    root: &Path,
    package: &str,
    desktop: Option<&DesktopEntry>,
    override_: Option<&str>,
) -> Result<PathBuf> {
    let by_name = |name: &str| BIN_DIRS.iter().find_map(|d| is_executable_in_root(root, &Path::new(d).join(name)));
    if let Some(o) = override_ {
        let found = if o.contains('/') {
            crate::extract::sanitize(Path::new(o)).ok().flatten().and_then(|p| is_executable_in_root(root, &p))
        } else {
            by_name(o)
        };
        return found.with_context(|| format!("--exec {o:?} is not an executable in the package"));
    }
    if let Some(p) = desktop.and_then(|d| d.exec.as_deref()).and_then(by_name) {
        return Ok(p);
    }
    if let Some(p) = by_name(package) {
        return Ok(p);
    }
    for d in BIN_DIRS {
        let Ok(rd) = fs::read_dir(root.join(d)) else { continue };
        let mut names: Vec<_> = rd.flatten().map(|e| e.file_name()).collect();
        names.sort();
        if let Some(p) = names.iter().find_map(|n| is_executable_in_root(root, &Path::new(d).join(n))) {
            return Ok(p);
        }
    }
    bail!("no executable found in the package; pass --exec <name>")
}

/// Copy the desktop entry and icon into `Contents/Resources`; returns the
/// icon path relative to `Contents/` for Info.toml.
fn copy_resources(layout: &BundleLayout, root: &Path, desktop: Option<&DesktopEntry>) -> Result<Option<String>> {
    let Some(d) = desktop else { return Ok(None) };
    if let Some(name) = d.file.file_name() {
        fs::copy(&d.file, layout.resources().join(name))?;
    }
    let Some(icon) = d.icon.as_deref().and_then(|i| desktop::find_icon(root, i)) else { return Ok(None) };
    let rel = icon.strip_prefix(root).ok().and_then(|r| resolve_in_root(root, r));
    let Some(rel) = rel else { return Ok(None) };
    let ext = rel.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
    let name = format!("icon.{ext}");
    fs::copy(root.join(rel), layout.resources().join(&name))?;
    Ok(Some(format!("Resources/{name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deb::tests::build_deb;

    #[test]
    fn copyright_files_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("doc");
        fs::create_dir_all(src.join("libfoo1")).unwrap();
        fs::write(src.join("libfoo1/copyright"), "MIT").unwrap();
        fs::write(src.join("libfoo1/changelog.gz"), "x").unwrap();
        std::os::unix::fs::symlink("libfoo1", src.join("libfoo-dev")).unwrap();
        let dst = d.path().join("out");
        merge_copyright(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("libfoo1/copyright")).unwrap(), "MIT");
        assert!(!dst.join("libfoo1/changelog.gz").exists(), "only the licence notice");
        assert!(fs::symlink_metadata(dst.join("libfoo-dev")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn plugin_paths() {
        assert!(is_plugin_path(Path::new(
            "usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-png.so"
        )));
        assert!(is_plugin_path(Path::new("usr/lib/aarch64-linux-gnu/gio/modules/libdconfsettings.so")));
        assert!(!is_plugin_path(Path::new("usr/lib/x86_64-linux-gnu/libgtk-3.so.0")));
        assert!(!is_plugin_path(Path::new("usr/bin/gnome-2048")));
    }

    #[test]
    fn source_parsing() {
        assert_eq!(Source::parse("debian:jq"), Source::Debian("jq".into()));
        assert_eq!(Source::parse("https://x/y.deb"), Source::Url("https://x/y.deb".into()));
    }

    #[test]
    fn lexical_resolution_stays_in_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(root.join("usr/lib/app")).unwrap();
        fs::write(root.join("usr/lib/app/real"), "x").unwrap();
        std::os::unix::fs::symlink("../lib/app/real", root.join("usr/bin/rel")).unwrap();
        std::os::unix::fs::symlink("/usr/lib/app/real", root.join("usr/bin/abs")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("usr/bin/host")).unwrap();
        assert_eq!(resolve_in_root(root, Path::new("usr/bin/rel")).unwrap(), Path::new("usr/lib/app/real"));
        assert_eq!(resolve_in_root(root, Path::new("usr/bin/abs")).unwrap(), Path::new("usr/lib/app/real"));
        assert!(
            resolve_in_root(root, Path::new("usr/bin/host")).is_none(),
            "must not resolve to the host's /etc/passwd"
        );
    }

    /// End-to-end on a synthetic, dependency-free package (no network).
    #[tokio::test]
    async fn ingests_local_deb_offline() {
        let dir = tempfile::tempdir().unwrap();
        let deb = build_deb(
            "Package: hello-iliada\nVersion: 1.0-1\nArchitecture: all\n",
            &[
                ("./usr/bin/hello-iliada", b"#!/bin/sh\necho hello from iliada\n", 0o755),
                (
                    "./usr/share/applications/hello-iliada.desktop",
                    b"[Desktop Entry]\nName=Hello Iliada\nExec=hello-iliada\nCategories=Utility;\n",
                    0o644,
                ),
            ],
        );
        let deb_path = dir.path().join("hello.deb");
        fs::write(&deb_path, deb).unwrap();
        let cfg = IngestConfig {
            out_dir: dir.path().join("out"),
            cache_dir: dir.path().join("cache"),
            use_index: false,
            ..Default::default()
        };
        let ing = Ingestor::new(cfg, vec![]).unwrap();
        let report = ing.ingest(&Source::Url(deb_path.to_string_lossy().into_owned())).await.unwrap();
        assert!(report.bundle.ends_with("Hello Iliada.app"));
        let b = Bundle::open(&report.bundle).unwrap();
        assert_eq!(b.info.id.as_str(), "org.debian.hello-iliada");
        assert_eq!(b.info.categories, ["Utility"]);
        let out = std::process::Command::new(b.layout.executables().join("hello-iliada")).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello from iliada");
    }
}
