//! 单词发音：按词形拿一段编码好的音频，**不在这里解码**（解码与播放是平台层的事，macOS 走 `AVAudioPlayer`）。
//!
//! 两条并行的路，查的一方先问缓存再问整包：
//! - [`AudioCache`]：按需下载的缓存目录（一个词一个文件 + `index.tsv`）。边下边写，所以不能用 `.qj`
//!   （容器的设计是「只整体替换」的不可变文件）。
//! - [`AudioLibrary`]：整包分发的 `.qj`（`Kind::Audio`），mmap 零拷贝。缓存攒够了用
//!   `dict-convert pack audio --input <缓存目录>` 打成它就变离线库，也方便给别人一份现成的。
//!
//! 词形一律小写查（`GitHub` 与 `github` 同一条），与 `qingjian-dictionary::WordList` 的编码约定一致。
//!
//! [`Source`]（真人 / 合成）跟到播放层是因为两类音频的许可不同：真人录音（CC-BY-SA / CC0）要逐条署名，
//! 合成音（Kokoro-82M，Apache-2.0）不要求。

mod cache;
mod entry;
mod error;
#[cfg(feature = "fetch")]
mod fetch;
mod library;
mod source;

pub use cache::{AudioCache, CachedWord};
pub use entry::Codec;
pub use error::AudioError;
#[cfg(feature = "fetch")]
pub use fetch::{Fetcher, FoundAudio};
pub use library::{AudioLibrary, Pronunciation};
pub use source::Source;

/// 文件扩展名（`.qj` 容器，种类是 [`qingjian_format::Kind::Audio`]）。
pub const EXTENSION: &str = "qj";

/// 整包发音库的缺省文件名（用户目录或随包）。
pub const FILE_NAME: &str = "audio-en.qj";

/// 按需下载的缓存目录名（在用户数据目录下）。
pub const CACHE_DIR: &str = "audio-cache";

/// 播放时统一到这个 RMS（dBFS）。下载来的录音电平能差 20 dB，播放前按它补增益。
/// 与构建时管线的目标一致（见 `tools/corpus/pronunciation-normalize.sh`），两条路出来的响度才对得上。
pub const TARGET_RMS_DBFS: f32 = -20.0;

/// 补增益的上限（dB）：很轻的录音不要一路拉到爆，也防止把底噪一起放大。
pub const MAX_GAIN_DB: f32 = 18.0;
