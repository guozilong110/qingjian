use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::entry::Codec;
use crate::error::AudioError;
use crate::source::Source;

/// 索引文件名：一个词一行，记文件名、许可证、作者、来源页与播放增益。
const INDEX_FILE: &str = "index.tsv";

/// 缓存里一条发音的记录。
#[derive(Debug, Clone, PartialEq)]
pub struct CachedWord {
    /// 音频文件名（相对缓存目录），如 `hello.ogg`。
    pub file: String,

    /// 许可证（`CC BY-SA 3.0` 这类，原样来自上游）。CC-BY-SA / CC-BY 要求署名，所以必须存。
    pub license: String,

    /// 作者 / 录音人。
    pub author: String,

    /// 文件页 URL，署名清单里要给出处。
    pub page: String,

    /// 播放时补多少 dB 才到统一响度。
    ///
    /// 下载来的录音电平能差 20 dB，不补的话一个词轰一个词听不见。构建时的管线是重新编码来归一的，
    /// 运行时不重新编码（用户机器上没有 ffmpeg），改成播放前按这个值调播放器音量；
    /// 值在首次下载后算一次就存下来，之后不再算。
    pub gain_db: f32,
}

/// 按需下载的发音缓存：一个目录 + 一份索引。
///
/// 为什么不用 `.qj`：容器的设计是「只整体替换」的不可变文件（见 `qingjian-format`），而缓存要边下边写。
/// 所以缓存是普通目录，攒够了可以用 `dict-convert pack audio --input <缓存目录>` 打成 `.qj` 变成离线库。
#[derive(Debug, Default)]
pub struct AudioCache {
    /// 缓存目录。
    dir: PathBuf,

    /// 词形（小写）→ 记录。
    words: HashMap<String, CachedWord>,

    /// 查过但上游确实没有的词：不再反复问。只在内存里，重启后会再问一次。
    missing: std::collections::HashSet<String>,
}

impl AudioCache {
    /// 打开（或创建）缓存目录并读索引。索引坏了当空的用，不让缓存问题挡住输入。
    pub fn open(dir: &Path) -> Result<Self, AudioError> {
        std::fs::create_dir_all(dir)?;
        let mut cache = Self {
            dir: dir.to_path_buf(),
            words: HashMap::new(),
            missing: std::collections::HashSet::new(),
        };
        let index = dir.join(INDEX_FILE);
        if index.is_file() {
            match std::fs::read_to_string(&index) {
                Ok(text) => cache.parse_index(&text),
                Err(error) => {
                    tracing::warn!(%error, path = %index.display(), "发音缓存索引读不了，当空的用");
                }
            }
        }
        tracing::debug!(dir = %dir.display(), words = cache.words.len(), "发音缓存已打开");
        Ok(cache)
    }

    /// 解析索引；格式不对的行跳过（缓存不是要紧数据，坏一行不值得报错）。
    fn parse_index(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 6 {
                continue;
            }
            let Ok(gain_db) = fields[5].parse::<f32>() else {
                continue;
            };
            self.words.insert(
                fields[0].to_lowercase(),
                CachedWord {
                    file: fields[1].to_owned(),
                    license: fields[2].to_owned(),
                    author: fields[3].to_owned(),
                    page: fields[4].to_owned(),
                    gain_db,
                },
            );
        }
    }

    /// 查一个词：缓存里有且文件还在就返回记录与音频路径。
    pub fn get(&self, word: &str) -> Option<(&CachedWord, PathBuf)> {
        let key = word.trim().to_lowercase();
        let record = self.words.get(&key)?;
        let path = self.dir.join(&record.file);
        // 索引里有、文件被用户删了：当没有，下次会重新下
        path.is_file().then_some((record, path))
    }

    /// 这个词问过上游、确实没有免费录音。
    pub fn is_known_missing(&self, word: &str) -> bool {
        self.missing.contains(&word.trim().to_lowercase())
    }

    /// 记下「上游没有这个词」，别反复问。
    pub fn note_missing(&mut self, word: &str) {
        self.missing.insert(word.trim().to_lowercase());
    }

    /// 存一条新下载的发音：先写音频文件再追加索引，顺序反了会留下索引指向不存在的文件。
    ///
    /// `credit` 是（许可证, 作者, 来源页）：CC-BY 系要求署名，三样都得存。
    pub fn insert(
        &mut self,
        word: &str,
        audio: &[u8],
        extension: &str,
        credit: (&str, &str, &str),
        gain_db: f32,
    ) -> Result<(), AudioError> {
        let key = word.trim().to_lowercase();
        if key.is_empty() || audio.is_empty() {
            return Ok(());
        }
        let file = format!("{key}.{extension}");
        std::fs::write(self.dir.join(&file), audio)?;
        let (license, author, page) = credit;
        let record = CachedWord {
            file,
            license: sanitize(license),
            author: sanitize(author),
            page: sanitize(page),
            gain_db,
        };
        self.append_index(&key, &record)?;
        self.words.insert(key, record);
        Ok(())
    }

    /// 往索引追加一行。整份重写会在断电时丢掉全部记录，追加最多丢最后一行。
    fn append_index(&self, word: &str, record: &CachedWord) -> Result<(), AudioError> {
        use std::io::Write;
        let path = self.dir.join(INDEX_FILE);
        let new = !path.is_file();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        if new {
            writeln!(file, "# 词\t文件\t许可证\t作者\t来源页\t增益(dB)")?;
        }
        writeln!(
            file,
            "{word}\t{}\t{}\t{}\t{}\t{:.2}",
            record.file, record.license, record.author, record.page, record.gain_db
        )?;
        Ok(())
    }

    /// 回填增益：刚下载的那条先按 0 存，量完电平再写回来。整份重写索引（条数不多，几千行）。
    pub fn set_gain(&mut self, word: &str, gain_db: f32) -> Result<(), AudioError> {
        let key = word.trim().to_lowercase();
        let Some(record) = self.words.get_mut(&key) else {
            return Ok(());
        };
        record.gain_db = gain_db;
        self.rewrite_index()
    }

    /// 忘掉一条（下载来的音频读不出样本时）：删文件与索引里那行。
    pub fn forget(&mut self, word: &str) -> Result<(), AudioError> {
        let key = word.trim().to_lowercase();
        if let Some(record) = self.words.remove(&key) {
            let _ = std::fs::remove_file(self.dir.join(&record.file));
            self.rewrite_index()?;
        }
        Ok(())
    }

    /// 整份重写索引（改增益、删条目时用）。先写临时文件再改名，中途断电不会留下半份索引。
    fn rewrite_index(&self) -> Result<(), AudioError> {
        let mut text = String::from("# 词\t文件\t许可证\t作者\t来源页\t增益(dB)\n");
        for (word, record) in self.entries() {
            text.push_str(&format!(
                "{word}\t{}\t{}\t{}\t{}\t{:.2}\n",
                record.file, record.license, record.author, record.page, record.gain_db
            ));
        }
        let path = self.dir.join(INDEX_FILE);
        let temp = path.with_extension("tsv.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    }

    /// 缓存了多少个词。
    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 全部记录，按词形排序（署名清单与偏好设置里列用）。
    pub fn entries(&self) -> Vec<(&str, &CachedWord)> {
        let mut all: Vec<(&str, &CachedWord)> = self
            .words
            .iter()
            .map(|(word, record)| (word.as_str(), record))
            .collect();
        all.sort_by_key(|(word, _)| *word);
        all
    }

    /// 缓存里音频占多少字节（偏好设置里显示、判断该不该提示清理）。
    pub fn audio_bytes(&self) -> u64 {
        self.words
            .values()
            .filter_map(|record| std::fs::metadata(self.dir.join(&record.file)).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// 清空缓存：删音频与索引，目录留着。
    pub fn clear(&mut self) -> Result<(), AudioError> {
        for record in self.words.values() {
            let _ = std::fs::remove_file(self.dir.join(&record.file));
        }
        let _ = std::fs::remove_file(self.dir.join(INDEX_FILE));
        self.words.clear();
        self.missing.clear();
        Ok(())
    }

    /// 缓存里的词都算「真人录音」：只从 Wikimedia Commons 下，合成音不进缓存。
    pub fn source() -> Source {
        Source::Human
    }

    /// 缓存文件按扩展名认编码；`.ogg` / `.oga` 当 Ogg（可能是 Vorbis 也可能是 Opus，系统播放器都认）。
    pub fn codec_of(extension: &str) -> Codec {
        match extension {
            "m4a" | "aac" | "mp4" => Codec::AacM4a,
            _ => Codec::OpusOgg,
        }
    }
}

/// 去掉制表符与换行：它们会把索引的 TSV 撑坏（Commons 的作者字段常带换行）。
fn sanitize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("qingjian-audio-cache-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_through_the_index_file() {
        let dir = temp_dir("round-trip");
        {
            let mut cache = AudioCache::open(&dir).unwrap();
            assert!(cache.is_empty());
            cache
                .insert(
                    "Hello",
                    b"audio-bytes",
                    "ogg",
                    ("CC BY-SA 3.0", "Dvortygirl", "https://example.org/hello"),
                    3.5,
                )
                .unwrap();
            let (record, path) = cache.get("HELLO").expect("大小写不敏感");
            assert_eq!(record.license, "CC BY-SA 3.0");
            assert_eq!(record.gain_db, 3.5);
            assert_eq!(std::fs::read(path).unwrap(), b"audio-bytes");
        }
        // 重新打开：索引读回来，不用再下载
        let cache = AudioCache::open(&dir).unwrap();
        assert_eq!(cache.len(), 1);
        let (record, _) = cache.get("hello").unwrap();
        assert_eq!(record.author, "Dvortygirl");
        assert_eq!(record.gain_db, 3.5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn author_newlines_do_not_break_the_index() {
        let dir = temp_dir("sanitize");
        let mut cache = AudioCache::open(&dir).unwrap();
        // Commons 的作者字段常是 "Speaker: A\nRecorder: B"
        cache
            .insert(
                "world",
                b"a",
                "wav",
                (
                    "CC0",
                    "Speaker: Grendelkhan\nRecorder: Grendelkhan",
                    "https://example.org/world",
                ),
                0.0,
            )
            .unwrap();
        let text = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        // 一行表头 + 一行记录，多了说明换行把 TSV 撑坏了
        assert_eq!(text.lines().count(), 2);
        let reopened = AudioCache::open(&dir).unwrap();
        assert_eq!(
            reopened.get("world").unwrap().0.author,
            "Speaker: Grendelkhan Recorder: Grendelkhan"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_words_are_remembered_in_memory() {
        let dir = temp_dir("missing");
        let mut cache = AudioCache::open(&dir).unwrap();
        assert!(!cache.is_known_missing("kubectl"));
        cache.note_missing("Kubectl");
        assert!(cache.is_known_missing("kubectl"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_deleted_audio_file_counts_as_absent() {
        let dir = temp_dir("deleted");
        let mut cache = AudioCache::open(&dir).unwrap();
        cache
            .insert("hello", b"a", "ogg", ("CC0", "Someone", "url"), 0.0)
            .unwrap();
        std::fs::remove_file(dir.join("hello.ogg")).unwrap();
        // 索引里还有，但文件没了：当没有，下次重新下
        assert!(cache.get("hello").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gain_is_written_back_and_survives_reopening() {
        let dir = temp_dir("set-gain");
        {
            let mut cache = AudioCache::open(&dir).unwrap();
            cache
                .insert("hello", b"a", "ogg", ("CC0", "Someone", "url"), 0.0)
                .unwrap();
            cache
                .insert("world", b"b", "wav", ("CC0", "Another", "url2"), 0.0)
                .unwrap();
            // 刚下载时按 0 存，量完电平回填
            cache.set_gain("hello", 7.25).unwrap();
        }
        let cache = AudioCache::open(&dir).unwrap();
        assert_eq!(cache.get("hello").unwrap().0.gain_db, 7.25);
        // 重写索引不能弄丢别的条目
        assert_eq!(cache.get("world").unwrap().0.gain_db, 0.0);
        assert_eq!(cache.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forgetting_removes_the_file_and_the_row() {
        let dir = temp_dir("forget");
        let mut cache = AudioCache::open(&dir).unwrap();
        cache
            .insert("hello", b"a", "ogg", ("CC0", "Someone", "url"), 0.0)
            .unwrap();
        cache
            .insert("world", b"b", "ogg", ("CC0", "Another", "url2"), 1.0)
            .unwrap();
        cache.forget("hello").unwrap();
        assert!(cache.get("hello").is_none());
        assert!(!dir.join("hello.ogg").exists());
        assert!(cache.get("world").is_some());
        let reopened = AudioCache::open(&dir).unwrap();
        assert_eq!(reopened.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clearing_removes_audio_and_index() {
        let dir = temp_dir("clear");
        let mut cache = AudioCache::open(&dir).unwrap();
        cache
            .insert("hello", b"a", "ogg", ("CC0", "Someone", "url"), 0.0)
            .unwrap();
        assert_eq!(cache.audio_bytes(), 1);
        cache.clear().unwrap();
        assert!(cache.is_empty());
        assert!(!dir.join("hello.ogg").exists());
        assert!(!dir.join(INDEX_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
