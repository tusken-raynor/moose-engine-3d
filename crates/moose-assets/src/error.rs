use std::fmt;
use std::path::{Path, PathBuf};

/// A failure to read, parse or validate an asset file.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadError {
    pub path: PathBuf,
    /// 1-based line the problem was found on, when it belongs to one.
    pub line: Option<usize>,
    pub message: String,
}

impl LoadError {
    pub(crate) fn new(path: &Path, line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{}: {}", self.path.display(), line, self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for LoadError {}
