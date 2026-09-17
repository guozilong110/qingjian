//! 单词发音：候选高亮停住不动就读那个词的英语发音。
//!
//! 四层，都在这个目录里：
//! - [`Speaker`]：查缓存 / 整包库、存下载来的音、播放；
//! - [`Downloader`]：后台线程去 Wikimedia Commons 抓（网络请求绝不在按键回调里做）；
//! - [`SpeakMonitor`]：停顿防抖与下载结果轮询的两个 `NSTimer`；
//! - `audio`：解码、按目标响度补增益、`AVAudioEngine` 播放。
//!
//! 「该读哪个词」不在这里，在 [`Host::speak_highlighted`](crate::host::Host::speak_highlighted)。
//!
//! 音频**按需下载**：本地没有的词后台去抓，抓到存进 `~/Library/Application Support/Qingjian/audio-cache/`，
//! 下次瞬时。这意味着「正在看哪个英文词」会以 HTTP 请求到达 Wikimedia，所以：开关缺省关、说明里写明、
//! 私密输入下一律不请求。

mod audio;
mod downloader;
mod monitor;
mod speaker;

pub use downloader::Downloader;
pub use monitor::SpeakMonitor;
pub use speaker::Speaker;
