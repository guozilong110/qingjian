use std::path::Path;

use qingjian_format::{Container, Kind, Metadata, Table, Text, Writer, hash};

use crate::entry::{Codec, Entry};
use crate::error::AudioError;
use crate::source::Source;

/// `.qj` 里的分节：词形 arena、条目表、哈希索引、音频 blob。
const WORDS_TAG: [u8; 4] = *b"WORD";
const ENTRIES_TAG: [u8; 4] = *b"ENTR";
const HASH_TAG: [u8; 4] = *b"HASH";
const BLOB_TAG: [u8; 4] = *b"BLOB";

/// 查到的一条发音：音频字节（原样喂给平台的播放器）加它的编码与来源。
#[derive(Debug, Clone, Copy)]
pub struct Pronunciation<'a> {
    /// 编码好的音频，直接交给平台播放器，不在这里解码。
    pub audio: &'a [u8],

    pub codec: Codec,

    pub source: Source,
}

/// 单词发音库。从目录构建时四段数据是自己的，从 `.qj` 打开时是映射文件里的，查询代码不区分。
#[derive(Debug, Default)]
pub struct AudioLibrary {
    /// 所有词形（小写）首尾相接。
    words: Text,

    /// 每条发音的位置，按词形建哈希索引。
    entries: Table<Entry>,

    /// 词形 → 条目下标的开放寻址表。
    index: Table<u32>,

    /// 所有音频首尾相接。
    blob: Table<u8>,
}

impl AudioLibrary {
    /// 从 `(词形, 来源, 编码, 音频)` 建库。词形按小写去重，同一个词后来的丢掉（真人录音先进来就赢）。
    pub fn build(items: impl IntoIterator<Item = (String, Source, Codec, Vec<u8>)>) -> Self {
        let mut words = String::new();
        let mut entries: Vec<Entry> = Vec::new();
        let mut blob: Vec<u8> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (word, source, codec, audio) in items {
            let word = word.trim().to_lowercase();
            if word.is_empty() || audio.is_empty() || !seen.insert(word.clone()) {
                continue;
            }
            let word_start = u32::try_from(words.len()).expect("word arena fits in u32");
            let word_len = u16::try_from(word.len()).unwrap_or(u16::MAX);
            let audio_start = u32::try_from(blob.len()).expect("audio blob fits in u32");
            let audio_len = u32::try_from(audio.len()).expect("one clip fits in u32");
            words.push_str(&word[..usize::from(word_len)]);
            blob.extend_from_slice(&audio);
            entries.push(Entry {
                word_start,
                audio_start,
                audio_len,
                word_len,
                codec: codec.code(),
                source: source.code(),
            });
        }
        let words = Text::Owned(words);
        let index = hash::build(entries.len(), |id| {
            let entry = &entries[id as usize];
            word_of(&words, entry)
        });
        Self {
            words,
            entries: Table::Owned(entries),
            index: Table::Owned(index),
            blob: Table::Owned(blob),
        }
    }

    /// 打开 `.qj` 音频库：mmap + 校验分节，不读全部内容。
    pub fn open(path: &Path) -> Result<Self, AudioError> {
        let container = Container::open(path, Kind::Audio)?;
        let words: Text = container.text(WORDS_TAG)?;
        let entries: Table<Entry> = container.table(ENTRIES_TAG)?;
        let index: Table<u32> = container.table(HASH_TAG)?;
        let blob: Table<u8> = container.table(BLOB_TAG)?;
        if !hash::is_valid(&index, entries.len()) {
            return Err(AudioError::BrokenIndex);
        }
        for (position, entry) in entries.iter().enumerate() {
            let end = (entry.audio_start as usize).saturating_add(entry.audio_len as usize);
            let word_end = (entry.word_start as usize).saturating_add(usize::from(entry.word_len));
            if end > blob.len() || word_end > words.len() {
                return Err(AudioError::OutOfBounds { index: position });
            }
            Codec::from_code(entry.codec).ok_or(AudioError::UnknownCodec(entry.codec))?;
            Source::from_code(entry.source).ok_or(AudioError::UnknownSource(entry.source))?;
        }
        tracing::debug!(words = entries.len(), "单词发音库已打开");
        Ok(Self {
            words,
            entries,
            index,
            blob,
        })
    }

    /// 写成 `.qj`。
    pub fn write_qj(&self, path: &Path, metadata: &Metadata) -> Result<(), AudioError> {
        let metadata = Metadata {
            entries: self.len() as u64,
            ..metadata.clone()
        };
        Writer::new(Kind::Audio, &metadata)?
            .section(WORDS_TAG, self.words.as_bytes())
            .section(ENTRIES_TAG, self.entries.as_bytes())
            .section(HASH_TAG, self.index.as_bytes())
            .section(BLOB_TAG, &self.blob)
            .write_to(path)?;
        Ok(())
    }

    /// 查一个词的发音；大小写不敏感，没收录返回 `None`。
    pub fn get(&self, word: &str) -> Option<Pronunciation<'_>> {
        let key = word.trim().to_lowercase();
        let id = hash::find(&self.index, &key, |id| {
            word_of(&self.words, &self.entries[id as usize])
        })?;
        let entry = &self.entries[id as usize];
        let start = entry.audio_start as usize;
        Some(Pronunciation {
            audio: &self.blob[start..start + entry.audio_len as usize],
            // 打开时已校验过全部编号
            codec: Codec::from_code(entry.codec)?,
            source: Source::from_code(entry.source)?,
        })
    }

    /// 收录的词形（小写），按加入顺序。
    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| word_of(&self.words, entry))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 音频总字节数（打包时报体积用）。
    pub fn audio_bytes(&self) -> usize {
        self.blob.len()
    }
}

/// 条目对应的词形。
fn word_of<'a>(words: &'a Text, entry: &Entry) -> &'a str {
    let start = entry.word_start as usize;
    &words[start..start + usize::from(entry.word_len)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AudioLibrary {
        AudioLibrary::build([
            (
                "Hello".to_owned(),
                Source::Human,
                Codec::OpusOgg,
                b"human-hello".to_vec(),
            ),
            (
                "kubectl".to_owned(),
                Source::Synthetic,
                Codec::OpusOgg,
                b"synth-kubectl".to_vec(),
            ),
            // 重复词形（大小写不同）丢掉，先进来的赢
            (
                "hello".to_owned(),
                Source::Synthetic,
                Codec::AacM4a,
                b"synth-hello".to_vec(),
            ),
            // 空音频不收
            (
                "empty".to_owned(),
                Source::Human,
                Codec::OpusOgg,
                Vec::new(),
            ),
        ])
    }

    #[test]
    fn looks_up_case_insensitively_and_keeps_the_first_source() {
        let library = sample();
        assert_eq!(library.len(), 2);
        let hello = library.get("HeLLo").unwrap();
        assert_eq!(hello.audio, b"human-hello");
        assert_eq!(hello.source, Source::Human);
        assert_eq!(library.get(" kubectl ").unwrap().source, Source::Synthetic);
        assert!(library.get("nope").is_none());
    }

    #[test]
    fn round_trips_through_qj() {
        let dir = std::env::temp_dir().join("qingjian-audio-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("audio-{}.qj", std::process::id()));
        sample()
            .write_qj(
                &path,
                &Metadata {
                    name: "测试发音库".to_owned(),
                    ..Metadata::default()
                },
            )
            .unwrap();
        let opened = AudioLibrary::open(&path).unwrap();
        assert_eq!(opened.len(), 2);
        assert_eq!(opened.get("hello").unwrap().audio, b"human-hello");
        assert_eq!(opened.get("kubectl").unwrap().codec, Codec::OpusOgg);
        assert_eq!(opened.words().collect::<Vec<_>>(), vec!["hello", "kubectl"]);
        assert_eq!(
            opened.audio_bytes(),
            b"human-hello".len() + b"synth-kubectl".len()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_library_answers_nothing() {
        let library = AudioLibrary::default();
        assert!(library.is_empty());
        assert!(library.get("hello").is_none());
    }
}
