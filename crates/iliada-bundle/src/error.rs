use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot serialise manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid bundle: {0}")]
    Invalid(String),
}

impl BundleError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| BundleError::Io { path, source }
    }
}
