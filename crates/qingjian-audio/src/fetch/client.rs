use std::time::Duration;

use crate::error::AudioError;

use super::found::FoundAudio;
use super::{ALLOWED_LICENSE_PREFIXES, MAX_AUDIO_BYTES, TITLE_PATTERNS, USER_AGENT};

/// Commons 的接口地址。
const API: &str = "https://commons.wikimedia.org/w/api.php";

/// 单次请求超时：读音是可有可无的附加功能，宁可放弃也不要挂着。
const TIMEOUT: Duration = Duration::from_secs(10);

/// Commons 抓取客户端。一个进程一个，内部复用连接。
pub struct Fetcher {
    client: reqwest::Client,
}

impl Fetcher {
    pub fn new() -> Result<Self, AudioError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        Ok(Self { client })
    }

    /// 找一个词的发音。上游确实没有（或都是不能用的许可）时返回 `Ok(None)`，
    /// 调用方据此把这个词记成「问过没有」，不再反复请求。
    pub async fn fetch(&self, word: &str) -> Result<Option<FoundAudio>, AudioError> {
        let word = word.trim().to_lowercase();
        // 只查单个英文词：词组与带撇号的命名规则不一致，查了也是白费一次请求
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
            return Ok(None);
        }
        let titles: Vec<String> = TITLE_PATTERNS
            .iter()
            .map(|pattern| format!("File:{}", pattern.replace("{word}", &word)))
            .collect();
        if let Some(found) = self.try_titles(&titles).await? {
            return Ok(Some(found));
        }
        // 维基词典那套没有，再搜 Lingua Libre（文件名带录音人，只能搜）
        if let Some(title) = self.search_lingua_libre(&word).await? {
            return self.try_titles(&[title]).await;
        }
        Ok(None)
    }

    /// 按给定顺序查这些标题，取第一个存在且许可证允许的，下载它。
    async fn try_titles(&self, titles: &[String]) -> Result<Option<FoundAudio>, AudioError> {
        let response = self
            .client
            .get(API)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("titles", &titles.join("|")),
                ("prop", "imageinfo"),
                ("iiprop", "url|extmetadata|size"),
            ])
            .send()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        let pages = json
            .get("query")
            .and_then(|q| q.get("pages"))
            .and_then(|p| p.as_object());
        let Some(pages) = pages else {
            return Ok(None);
        };
        // 必须按 titles 给的顺序取：接口返回的顺序是它自己的（pageid 序），
        // 照那个顺序会把英音排到美音前面
        for title in titles {
            let Some(info) = pages
                .values()
                .find(|page| page.get("title").and_then(|t| t.as_str()) == Some(title.as_str()))
                .filter(|page| page.get("missing").is_none())
                .and_then(|page| page.get("imageinfo"))
                .and_then(|infos| infos.get(0))
            else {
                continue;
            };
            let license = extmetadata(info, "LicenseShortName");
            if !license_allowed(&license) {
                continue;
            }
            let Some(url) = info.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            // 长录音不是我们要的（有些条目是整段朗读）
            if info
                .get("size")
                .and_then(|s| s.as_u64())
                .is_some_and(|size| size > MAX_AUDIO_BYTES)
            {
                continue;
            }
            let audio = self.download(url).await?;
            if audio.is_empty() {
                continue;
            }
            return Ok(Some(FoundAudio {
                audio,
                extension: extension_of(url),
                license,
                author: extmetadata(info, "Artist"),
                page: info
                    .get("descriptionurl")
                    .and_then(|u| u.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            }));
        }
        Ok(None)
    }

    /// Lingua Libre 的文件名带录音人，按 `intitle` 搜。
    async fn search_lingua_libre(&self, word: &str) -> Result<Option<String>, AudioError> {
        let query = format!("intitle:\"LL-Q1860 (eng)\" intitle:\"{word}\" filetype:audio");
        let response = self
            .client
            .get(API)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("list", "search"),
                ("srsearch", &query),
                ("srnamespace", "6"),
                ("srlimit", "5"),
            ])
            .send()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        let hits = json
            .get("query")
            .and_then(|q| q.get("search"))
            .and_then(|s| s.as_array());
        let Some(hits) = hits else {
            return Ok(None);
        };
        // 标题必须正好以 `-<词>.wav` 结尾：搜索是模糊的，`water` 会命中 `waterfall`
        let suffix = format!("-{word}.wav");
        Ok(hits
            .iter()
            .filter_map(|hit| hit.get("title").and_then(|t| t.as_str()))
            .find(|title| title.to_lowercase().ends_with(&suffix))
            .map(str::to_owned))
    }

    /// 下载音频本体。
    async fn download(&self, url: &str) -> Result<Vec<u8>, AudioError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        if !response.status().is_success() {
            return Err(AudioError::Fetch(format!(
                "download failed with {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AudioError::Fetch(error.to_string()))?;
        if bytes.len() as u64 > MAX_AUDIO_BYTES {
            return Ok(Vec::new());
        }
        Ok(bytes.to_vec())
    }
}

/// `extmetadata` 里的一个字段，去掉 HTML 标签并压成一行。
fn extmetadata(info: &serde_json::Value, key: &str) -> String {
    let raw = info
        .get("extmetadata")
        .and_then(|m| m.get(key))
        .and_then(|f| f.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    strip_html(raw)
}

/// 去 HTML 标签并把空白压成单个空格（作者字段常带 `<a>` 与换行）。
fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 许可证是否在白名单里。
fn license_allowed(license: &str) -> bool {
    let normalized = license.trim().to_lowercase();
    if normalized.is_empty() {
        return false;
    }
    ALLOWED_LICENSE_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

/// 从 URL 取扩展名。不能直接看最后一个点：Commons 的 URL 带 `?utm_*` 查询串。
fn extension_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "ogg" | "oga" | "wav" | "mp3" | "flac" | "opus" => extension,
        _ => "ogg".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_free_licenses_pass() {
        for allowed in [
            "CC0",
            "CC BY-SA 3.0",
            "CC BY 4.0",
            "cc-by-sa-4.0",
            "Public domain",
            "PD",
        ] {
            assert!(license_allowed(allowed), "{allowed} 应该允许");
        }
        for rejected in ["", "Fair use", "All rights reserved", "GFDL-only", "©"] {
            assert!(!license_allowed(rejected), "{rejected} 不该允许");
        }
    }

    #[test]
    fn extension_ignores_the_query_string() {
        // Commons 的 URL 带 utm 参数，按最后一个点取会得到 "org&utm_campaign=..."
        assert_eq!(
            extension_of(
                "https://upload.wikimedia.org/a/b/En-us-hello.ogg?utm_source=commons.wikimedia.org&utm_campaign=imageinfo"
            ),
            "ogg"
        );
        assert_eq!(
            extension_of("https://x/y/LL-Q1860_(eng)-A-world.wav"),
            "wav"
        );
        // 认不出的退回 ogg
        assert_eq!(extension_of("https://x/y/weird"), "ogg");
    }

    #[test]
    fn html_and_newlines_are_stripped_from_credits() {
        assert_eq!(
            strip_html("<a href=\"/wiki/User:Dvortygirl\">Dvortygirl</a>"),
            "Dvortygirl"
        );
        assert_eq!(
            strip_html("Speaker: Grendelkhan\nRecorder: Grendelkhan"),
            "Speaker: Grendelkhan Recorder: Grendelkhan"
        );
    }
}
