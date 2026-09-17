use std::io;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("container error: {0}")]
    Format(#[from] qingjian_format::FormatError),

    #[error("audio library has a broken hash index")]
    BrokenIndex,

    #[error("entry {index} points outside the audio blob")]
    OutOfBounds {
        /// 越界那条的下标。
        index: usize,
    },

    #[error("unknown codec code {0}")]
    UnknownCodec(u8),

    #[error("unknown source code {0}")]
    UnknownSource(u8),

    /// 抓取失败（网络、超时、上游报错）。发音是附加功能，调用方只记日志、不打扰输入。
    #[error("fetch failed: {0}")]
    Fetch(String),
}
