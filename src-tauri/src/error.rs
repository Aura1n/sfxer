/// Single error type across the backend; commands return `Result<T, AppError>`
/// and the frontend receives the message string verbatim.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库操作失败：{0}")]
    Db(String),
    #[error("读写文件失败：{0}")]
    Io(String),
    #[error("无法读取音频信息：{0}")]
    Probe(String),
    #[error("该文件的编码无法在本机解码")]
    Unsupported,
    #[error("{0}")]
    Other(String),
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Db(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
