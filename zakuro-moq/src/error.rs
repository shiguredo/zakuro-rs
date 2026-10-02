//! zakuro-moq のエラー型

use std::io;

/// メッセージだけを持つエラー
#[derive(Debug, Clone)]
pub(crate) struct ErrorMessage {
    message: String,
}

impl ErrorMessage {
    /// メッセージからエラーを作る
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// zakuro-moq のエラー
#[derive(Debug)]
pub(crate) enum AppError {
    /// CLI / JSONC の解釈エラー
    Args(noargs::Error),
    /// メッセージのみのエラー
    Message(ErrorMessage),
    /// I/O エラー
    Io(io::Error),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Args(err) => write!(f, "{err:?}"),
            AppError::Message(err) => write!(f, "{err}"),
            AppError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<noargs::Error> for AppError {
    fn from(err: noargs::Error) -> Self {
        AppError::Args(err)
    }
}

impl From<ErrorMessage> for AppError {
    fn from(err: ErrorMessage) -> Self {
        AppError::Message(err)
    }
}

impl From<io::Error> for AppError {
    fn from(err: io::Error) -> Self {
        AppError::Io(err)
    }
}

/// `Result` の別名
pub(crate) type Result<T> = std::result::Result<T, AppError>;
