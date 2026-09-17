//! 从 Wikimedia Commons 抓一个英文词的真人发音。
//!
//! 只在用户打开发音开关、且缓存里没有那个词时才会走到这里——**这是一次外发请求**，
//! 意味着「正在看哪个英文词」会到达 Wikimedia。私密输入下调用方不会请求（见 `Engine::is_private`），
//! 开关的说明里也写明了这一点。
//!
//! 文件命名有社区惯例：英语维基词典录的是 `En-us-<词>.ogg`（美音）/ `En-uk-<词>.ogg`（英音），
//! Lingua Libre 批量录的是 `LL-Q1860 (eng)-<录音人>-<词>.wav`。按美音优先的顺序试，都没有才搜 Lingua Libre。
//! 与 `tools/corpus/pronunciation_fetch.py` 是同一套规则（那个脚本用于批量预建，这里是运行时按需）。

mod client;
mod found;

pub use client::Fetcher;
pub use found::FoundAudio;

/// 只接受这些许可证：都允许随产品分发，CC-BY 系要求署名（缓存索引里存作者与来源页）。
pub(crate) const ALLOWED_LICENSE_PREFIXES: [&str; 6] =
    ["cc0", "cc by", "cc-by", "public domain", "pd", "cc pd"];

/// 按惯例试的文件名模板，美音在前。
pub(crate) const TITLE_PATTERNS: [&str; 4] = [
    "En-us-{word}.ogg",
    "En-us-{word}.wav",
    "En-uk-{word}.ogg",
    "En-gb-{word}.ogg",
];

/// 请求头里的身份：Wikimedia 要求说明用途与联系方式，不然会被更早限速。
pub(crate) const USER_AGENT: &str =
    "qingjian-inputmethod/0.1 (https://github.com/qingjian-team/qingjian) pronunciation-fetch";

/// 一个音频文件最大接受多少字节：单词发音都很小，几 MB 的肯定不是我们要的（有些条目是长录音）。
pub(crate) const MAX_AUDIO_BYTES: u64 = 2_000_000;
