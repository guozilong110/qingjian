//! 单词发音库打包：归一过的 `.opus` 目录 → `audio-en.qj`，真人录音优先、合成音补缺。
//!
//! 两个来源分两个目录进来（`--input` 真人、`--synthetic` 合成），同一个词两边都有时取真人的：
//! 学英语真人读音的价值高于合成，而且真人那批是有署名要求的 CC-BY-SA / CC0，署名清单按实际收进去的那些生成。

use std::collections::BTreeMap;
use std::path::Path;

use qingjian_audio::{AudioLibrary, Codec, Source};
use qingjian_format::Metadata;

use crate::error::ConvertError;

/// 一条真人录音的出处（`pronunciation_fetch.py` 的 manifest.tsv）。
struct Credit {
    license: String,

    author: String,

    page: String,
}

/// 打包发音库：`human` 目录里的 `.opus` 先进，`synthetic` 补它没有的词。
/// `words` 给了就只收那份词表里的词（控体积）。
pub fn pack(
    human: &Path,
    synthetic: Option<&Path>,
    manifest: Option<&Path>,
    words: Option<&Path>,
    metadata: Metadata,
    out_dir: &Path,
) -> Result<(), ConvertError> {
    let allowed = words.map(read_word_list).transpose()?;
    let keep = |word: &str| {
        allowed
            .as_ref()
            .is_none_or(|list| list.contains(&word.to_lowercase()))
    };

    let mut items: Vec<(String, Source, Codec, Vec<u8>)> = Vec::new();
    let mut human_words = 0usize;
    for (word, path) in clips(human)? {
        if keep(&word) {
            items.push((word, Source::Human, Codec::OpusOgg, std::fs::read(&path)?));
            human_words += 1;
        }
    }
    let mut synthetic_words = 0usize;
    if let Some(dir) = synthetic {
        for (word, path) in clips(dir)? {
            if keep(&word) {
                items.push((
                    word,
                    Source::Synthetic,
                    Codec::OpusOgg,
                    std::fs::read(&path)?,
                ));
                synthetic_words += 1;
            }
        }
    }

    // 真人的先进 items，AudioLibrary::build 对重复词形取先进来的那条
    let library = AudioLibrary::build(items);
    let out = out_dir.join(qingjian_audio::FILE_NAME);
    let metadata = Metadata {
        generator: format!("qingjian-dict-convert {}", env!("CARGO_PKG_VERSION")),
        ..metadata
    };
    library.write_qj(&out, &metadata)?;

    if let Some(manifest) = manifest {
        let credits = read_manifest(manifest)?;
        write_attribution(&library, &credits, out_dir)?;
    }

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        out = %out.display(),
        words = library.len(),
        human = human_words,
        synthetic = synthetic_words,
        audio_mb = library.audio_bytes() / 1_000_000,
        size_mb = size / 1_000_000,
        "已打包单词发音库"
    );
    Ok(())
}

/// 目录里的音频：文件名（去扩展名、小写）就是词形。
///
/// 收 `.opus` 与 `.ogg` / `.wav`：前者是构建管线归一后的输出，后者是输入法按需下载攒下的缓存
/// （`~/Library/Application Support/Qingjian/audio-cache/`），两种目录都能直接打包。
fn clips(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>, ConvertError> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        if !matches!(
            extension.as_deref(),
            Some("opus" | "ogg" | "oga" | "wav" | "mp3" | "flac")
        ) {
            continue;
        }
        let Some(word) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        found.push((word.to_lowercase(), path));
    }
    // 文件系统的顺序不定，排一下让打包结果可复现
    found.sort();
    Ok(found)
}

/// 词表：一行一个词或 TSV 取第一列。
fn read_word_list(path: &Path) -> Result<std::collections::HashSet<String>, ConvertError> {
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split('\t').next())
        .map(str::to_lowercase)
        .collect())
}

/// 真人录音的出处表：`词\t文件\t许可证\t作者\t页面`，首行是表头。
fn read_manifest(path: &Path) -> Result<BTreeMap<String, Credit>, ConvertError> {
    let text = std::fs::read_to_string(path)?;
    let mut credits = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        // 表头只能按“第一行且正好是表头”认：不能按 `word\t` 前缀跳，
        // 因为 word 自己就是一个词，那样它的录音就漏了署名（CC-BY-SA 要求逐条署名，漏一条都不行）
        if line.trim().is_empty()
            || line.starts_with('#')
            || (index == 0 && line.starts_with("word\tfile\t"))
        {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        // 抓取脚本的 manifest.tsv 是五列；输入法缓存的 index.tsv 多一列增益，前五列含义相同
        if fields.len() < 5 {
            return Err(ConvertError::Format {
                path: path.to_path_buf(),
                line: index + 1,
                reason: "expected word / file / license / author / page".to_owned(),
            });
        }
        credits.insert(
            fields[0].to_lowercase(),
            Credit {
                license: fields[2].to_owned(),
                author: fields[3].to_owned(),
                page: fields[4].to_owned(),
            },
        );
    }
    Ok(credits)
}

/// 写署名清单：只列真正打进包里的那些录音，按许可证分组。CC-BY-SA / CC-BY 要求逐条署名，
/// 所以这个文件要随包发（放进 `.app` 的 Resources），「关于」页指向它。
fn write_attribution(
    library: &AudioLibrary,
    credits: &BTreeMap<String, Credit>,
    out_dir: &Path,
) -> Result<(), ConvertError> {
    let mut lines = vec![
        "# 单词发音录音署名清单".to_owned(),
        String::new(),
        "本文件由 `qingjian-dict-convert pack audio` 生成，逐条列出打进包里的真人录音及其许可证与作者。".to_owned(),
        "合成发音由 Kokoro-82M（Apache-2.0）本地生成，不要求逐条署名，不在此列。".to_owned(),
        String::new(),
        "| 词 | 许可证 | 作者 | 来源 |".to_owned(),
        "| --- | --- | --- | --- |".to_owned(),
    ];
    let mut listed = 0usize;
    for word in library.words() {
        // 只有真人录音在 manifest 里；合成的查不到，跳过
        if let Some(credit) = credits.get(word) {
            lines.push(format!(
                "| {word} | {} | {} | {} |",
                credit.license, credit.author, credit.page
            ));
            listed += 1;
        }
    }
    let path = out_dir.join("audio-attribution.md");
    std::fs::write(&path, lines.join("\n") + "\n")?;
    tracing::info!(path = %path.display(), recordings = listed, "已写署名清单");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时目录，里面按 `词 → 假音频` 放 `.opus`。
    fn clips_dir(tag: &str, words: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("qingjian-audio-pack-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (word, bytes) in words {
            std::fs::write(dir.join(format!("{word}.opus")), bytes).unwrap();
        }
        dir
    }

    fn out_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("qingjian-audio-out-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn human_recordings_win_over_synthetic_for_the_same_word() {
        let human = clips_dir("human", &[("hello", b"human-hello")]);
        let synthetic = clips_dir(
            "synth",
            &[("hello", b"synth-hello"), ("kubectl", b"synth-kubectl")],
        );
        let out = out_dir("precedence");
        pack(
            &human,
            Some(&synthetic),
            None,
            None,
            Metadata::default(),
            &out,
        )
        .unwrap();
        let library = AudioLibrary::open(&out.join(qingjian_audio::FILE_NAME)).unwrap();
        assert_eq!(library.len(), 2);
        let hello = library.get("hello").unwrap();
        assert_eq!(hello.audio, b"human-hello");
        assert_eq!(hello.source, Source::Human);
        assert_eq!(library.get("kubectl").unwrap().source, Source::Synthetic);
        std::fs::remove_dir_all(human).unwrap();
        std::fs::remove_dir_all(synthetic).unwrap();
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn word_list_limits_what_gets_packed() {
        let human = clips_dir("limit", &[("hello", b"a"), ("world", b"b"), ("rare", b"c")]);
        let out = out_dir("limit");
        let words = out.join("words.txt");
        // 大小写不同也要认，词表和文件名都按小写比
        std::fs::write(&words, "Hello\nworld\n").unwrap();
        pack(&human, None, None, Some(&words), Metadata::default(), &out).unwrap();
        let library = AudioLibrary::open(&out.join(qingjian_audio::FILE_NAME)).unwrap();
        assert_eq!(library.len(), 2);
        assert!(library.get("rare").is_none());
        std::fs::remove_dir_all(human).unwrap();
        std::fs::remove_dir_all(out).unwrap();
    }

    /// `word` 自己就是一个词：按 `word\t` 前缀跳表头会把它的录音漏掉署名。
    /// CC-BY-SA 要求逐条署名，漏一条就是许可问题，所以钉住它。
    /// 输入法按需下载攒下的缓存目录（原始 ogg / wav + 六列的 index.tsv）能直接打包成离线库。
    #[test]
    fn a_download_cache_directory_can_be_packed() {
        let dir =
            std::env::temp_dir().join(format!("qingjian-audio-cachepack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 缓存里存的是原始下载文件，不是 .opus
        std::fs::write(dir.join("hello.ogg"), b"ogg-bytes").unwrap();
        std::fs::write(dir.join("world.wav"), b"wav-bytes").unwrap();
        // 缓存索引比 manifest 多一列增益
        std::fs::write(
            dir.join("index.tsv"),
            "# 词\t文件\t许可证\t作者\t来源页\t增益(dB)\n\
             hello\thello.ogg\tCC BY-SA 3.0\tDvortygirl\thttps://example.org/h\t6.50\n\
             world\tworld.wav\tCC0\tGrendelkhan\thttps://example.org/w\t-2.00\n",
        )
        .unwrap();
        let out = out_dir("cachepack");
        let manifest = dir.join("index.tsv");
        pack(&dir, None, Some(&manifest), None, Metadata::default(), &out).unwrap();
        let library = AudioLibrary::open(&out.join(qingjian_audio::FILE_NAME)).unwrap();
        assert_eq!(library.len(), 2);
        assert_eq!(library.get("hello").unwrap().audio, b"ogg-bytes");
        let credits = std::fs::read_to_string(out.join("audio-attribution.md")).unwrap();
        assert!(credits.contains("| hello | CC BY-SA 3.0 | Dvortygirl |"));
        assert!(credits.contains("| world | CC0 | Grendelkhan |"));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn the_word_word_is_still_credited() {
        let human = clips_dir("word", &[("word", b"a"), ("hello", b"b")]);
        let out = out_dir("word");
        let manifest = out.join("manifest.tsv");
        std::fs::write(
            &manifest,
            "word\tfile\tlicense\tauthor\tpage\n\
             word\tword.ogg\tCC BY-SA 3.0\tSomebody\thttps://example.org/word\n\
             hello\thello.ogg\tCC0\tAnother\thttps://example.org/hello\n",
        )
        .unwrap();
        pack(
            &human,
            None,
            Some(&manifest),
            None,
            Metadata::default(),
            &out,
        )
        .unwrap();
        let credits = std::fs::read_to_string(out.join("audio-attribution.md")).unwrap();
        assert!(credits.contains("| word | CC BY-SA 3.0 | Somebody |"));
        assert!(credits.contains("| hello | CC0 | Another |"));
        std::fs::remove_dir_all(human).unwrap();
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn attribution_lists_only_packed_recordings() {
        let human = clips_dir("credit", &[("hello", b"a")]);
        let synthetic = clips_dir("credit-synth", &[("kubectl", b"b")]);
        let out = out_dir("credit");
        let manifest = out.join("manifest.tsv");
        std::fs::write(
            &manifest,
            "word\tfile\tlicense\tauthor\tpage\n\
             hello\thello.ogg\tCC BY-SA 3.0\tDvortygirl\thttps://example.org/hello\n\
             absent\tabsent.ogg\tCC0\tSomebody\thttps://example.org/absent\n",
        )
        .unwrap();
        pack(
            &human,
            Some(&synthetic),
            Some(&manifest),
            None,
            Metadata::default(),
            &out,
        )
        .unwrap();
        let credits = std::fs::read_to_string(out.join("audio-attribution.md")).unwrap();
        assert!(credits.contains("| hello | CC BY-SA 3.0 | Dvortygirl |"));
        // 没打进包的录音不列，合成音也不列
        assert!(!credits.contains("absent"));
        assert!(!credits.contains("kubectl"));
        std::fs::remove_dir_all(human).unwrap();
        std::fs::remove_dir_all(synthetic).unwrap();
        std::fs::remove_dir_all(out).unwrap();
    }
}
