use serde::Serialize;

/// Error returned by every command. `code` lets the UI react (for example,
/// prompt for a password) without parsing messages.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The data source does not save its password and none was given.
    PasswordRequired,
    NotFound,
    Connect,
    Query,
    InvalidParams,
    Storage,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl From<idedb_core::Error> for CommandError {
    fn from(e: idedb_core::Error) -> Self {
        let code = match e {
            idedb_core::Error::Connect(_) => ErrorCode::Connect,
            idedb_core::Error::Query(_) => ErrorCode::Query,
            idedb_core::Error::InvalidParams(_) => ErrorCode::InvalidParams,
        };
        Self::new(code, e.to_string())
    }
}

impl From<idedb_store::Error> for CommandError {
    fn from(e: idedb_store::Error) -> Self {
        Self::new(ErrorCode::Storage, e.to_string())
    }
}

pub type CommandResult<T> = Result<T, CommandError>;
