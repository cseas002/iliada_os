use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use iliada_bundle::{Bundle, IsolationTier, ResolutionStatus};
use iliada_ingest::advisor::{Advisor, RuleAdvisor};
use iliada_ingest::pipeline::default_cache_dir;
use iliada_ingest::{IngestConfig, Ingestor, Source, serve};

#[derive(Parser, Debug)]
#[command(name = "iliada-ingest", version, about = "Turn upstream packages into Iliada .app bundles")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Ingest one package: a .deb URL/path, or `debian:<name>`.
    Run {
        source: String,
        #[command(flatten)]
        opts: Opts,
        /// Override the entry point (binary name or path inside the package).
        #[arg(long)]
        exec: Option<String>,
    },
    /// Watch the sources in a watchlist and re-ingest when they change.
    Serve {
        #[arg(long)]
        watchlist: PathBuf,
        #[command(flatten)]
        opts: Opts,
    },
    /// Print a bundle's manifests in a readable form.
    Inspect { bundle: PathBuf },
}

#[derive(clap::Args, Debug)]
struct Opts {
    #[arg(long, default_value = "out")]
    out: PathBuf,
    #[arg(long, env = "ILIADA_INGEST_CACHE")]
    cache: Option<PathBuf>,
    #[arg(long, default_value = "https://deb.debian.org/debian")]
    mirror: String,
    #[arg(long, default_value = "trixie")]
    suite: String,
    #[arg(long, value_delimiter = ',', default_value = "main")]
    components: Vec<String>,
    /// Debian architecture (default: the package's own, else this host's).
    #[arg(long)]
    arch: Option<String>,
    #[arg(long, value_enum, default_value_t = AdvisorChoice::Rules)]
    advisor: AdvisorChoice,
    #[arg(long, value_enum, default_value_t = Tier::MicroVm)]
    isolation: Tier,
    /// Follow dependencies into non-library packages too.
    #[arg(long)]
    full_closure: bool,
    /// Don't consult the package index (bundle only what the package ships).
    #[arg(long)]
    no_index: bool,
    /// Replace an existing bundle of the same name.
    #[arg(long)]
    force: bool,
    #[arg(long)]
    allow_http: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AdvisorChoice {
    None,
    Rules,
    /// Rules first, then Claude for what is left (needs the `claude` feature and ANTHROPIC_API_KEY).
    Claude,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Tier {
    Namespace,
    MicroVm,
}

impl Opts {
    fn config(&self, exec: Option<String>, force: bool) -> IngestConfig {
        IngestConfig {
            out_dir: self.out.clone(),
            cache_dir: self.cache.clone().unwrap_or_else(default_cache_dir),
            mirror: self.mirror.clone(),
            suite: self.suite.clone(),
            components: self.components.clone(),
            arch: self.arch.clone(),
            exec_override: exec,
            isolation: match self.isolation {
                Tier::Namespace => IsolationTier::Namespace,
                Tier::MicroVm => IsolationTier::MicroVm,
            },
            full_closure: self.full_closure,
            use_index: !self.no_index,
            force: self.force || force,
            allow_http: self.allow_http,
            ..Default::default()
        }
    }

    fn advisors(&self) -> Result<Vec<Box<dyn Advisor>>> {
        let mut v: Vec<Box<dyn Advisor>> = Vec::new();
        if matches!(self.advisor, AdvisorChoice::Rules | AdvisorChoice::Claude) {
            v.push(Box::new(RuleAdvisor));
        }
        if matches!(self.advisor, AdvisorChoice::Claude) {
            #[cfg(feature = "claude")]
            {
                let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build()?;
                match iliada_ingest::advisor::ClaudeAdvisor::from_env(client) {
                    Some(a) => v.push(Box::new(a)),
                    None => anyhow::bail!("--advisor claude needs ANTHROPIC_API_KEY to be set"),
                }
            }
            #[cfg(not(feature = "claude"))]
            anyhow::bail!("this build has no Claude advisor; rebuild with `--features claude`");
        }
        Ok(v)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::Run { source, opts, exec } => {
            let ing = Ingestor::new(opts.config(exec, false), opts.advisors()?)?;
            let r = ing.ingest(&Source::parse(&source)).await?;
            println!("bundle      {}", r.bundle.display());
            println!("app id      {}", r.app_id);
            println!("version     {}", r.version);
            println!("entry       Root/{}", r.executable.display());
            println!("confinement {:?}", r.confinement);
            println!("packages    {}", r.packages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
            println!("libraries");
            for l in &r.libraries {
                let via = match (&l.package, &l.advisor) {
                    (Some(p), Some(a)) => format!("{p} (suggested by {a}, verified)"),
                    (Some(p), None) => p.clone(),
                    _ => String::new(),
                };
                println!("  {:<28} {:<10} {}", l.soname, format!("{:?}", l.status).to_lowercase(), via);
            }
            if !r.skipped_dependencies.is_empty() {
                println!("not followed (non-library deps): {}", r.skipped_dependencies.join(", "));
            }
            if !r.unresolved.is_empty() {
                println!("UNRESOLVED: {}", r.unresolved.join(", "));
                std::process::exit(2);
            }
        }
        Cmd::Serve { watchlist, opts } => {
            let list = serve::Watchlist::load(&watchlist)?;
            let ing = Arc::new(Ingestor::new(opts.config(None, true), opts.advisors()?)?);
            serve::serve(ing, list).await?;
        }
        Cmd::Inspect { bundle } => {
            let b = Bundle::open(&bundle)?;
            println!("{}", toml::to_string_pretty(&b.info)?);
            println!("# permissions (upper bound; each use is still prompted)");
            for r in &b.permissions.requests {
                println!("- {:?}: {}", r.capability, r.purpose);
            }
            if let Some(p) = &b.provenance {
                println!("# provenance: {} ({})", p.source.url, p.source.sha256);
                let unresolved: Vec<_> = p
                    .libraries
                    .iter()
                    .filter(|l| l.status == ResolutionStatus::Unresolved)
                    .map(|l| &l.soname)
                    .collect();
                println!("libraries: {} total, {} unresolved {:?}", p.libraries.len(), unresolved.len(), unresolved);
            }
        }
    }
    Ok(())
}
