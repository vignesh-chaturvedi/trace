use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("event log at {path} is corrupt at line {line}: {detail}")]
    CorruptLog {
        path: PathBuf,
        line: usize,
        detail: String,
    },

    /// The log is internally inconsistent in a way repair cannot fix. A gap in
    /// `seq` means events were lost from the middle, which no downstream
    /// consumer can compensate for.
    #[error("event log at {path} has a sequence gap: {prev} -> {next}")]
    SequenceGap { path: PathBuf, prev: u64, next: u64 },

    #[error("config error: {0}")]
    Config(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}
