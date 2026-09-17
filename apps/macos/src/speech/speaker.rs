use std::path::Path;

use qingjian_audio::{AudioCache, AudioLibrary, FoundAudio, MAX_GAIN_DB, TARGET_RMS_DBFS};

use super::audio::{Clip, Output};

/// 单词发音：查两处（按需缓存、整包库），播一段，管住音量。
///
/// 缓存优先于整包库：缓存里是用户自己用到过的词，整包库是「别人给的一份现成的」。
#[derive(Default)]
pub struct Speaker {
    /// 按需下载的缓存；打不开（权限、坏盘）为 `None`，那就只能靠整包库。
    cache: Option<AudioCache>,

    /// 整包发音库（`.qj`）；没装为 `None`。
    library: Option<AudioLibrary>,

    /// 音频输出（常驻引擎）。
    output: Output,

    /// 上一个处理过的词（小写）：同一个词不重复读，也不重复发下载请求。
    last: Option<String>,
}

impl Speaker {
    /// 打开缓存与（可选的）整包库。两者都没有只是不发音，绝不影响输入。
    pub fn new(cache_dir: &Path, library_path: Option<&Path>) -> Self {
        let cache = match AudioCache::open(cache_dir) {
            Ok(cache) => {
                tracing::info!(dir = %cache_dir.display(), words = cache.len(), "发音缓存已就绪");
                Some(cache)
            }
            Err(error) => {
                tracing::warn!(%error, dir = %cache_dir.display(), "发音缓存打不开，不缓存");
                None
            }
        };
        let library = library_path.and_then(|path| match AudioLibrary::open(path) {
            Ok(library) => {
                tracing::info!(path = %path.display(), words = library.len(), "整包发音库已加载");
                Some(library)
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "整包发音库加载失败，只用缓存");
                None
            }
        });
        Self {
            cache,
            library,
            ..Self::default()
        }
    }

    /// 与上次处理的是同一个词：不必再读、也不必再下载。
    pub fn is_repeat(&self, word: &str) -> bool {
        self.last.as_deref() == Some(word.trim().to_lowercase().as_str())
    }

    /// 这个词问过上游、确实没有免费录音。
    pub fn is_known_missing(&self, word: &str) -> bool {
        self.cache
            .as_ref()
            .is_some_and(|c| c.is_known_missing(word))
    }

    /// 读一个词，只用本地已有的；返回是否真放了。本地没有时返回 `false`，由调用方决定是否去下载。
    pub fn speak_local(&mut self, word: &str) -> bool {
        let key = word.trim().to_lowercase();
        if key.is_empty() {
            return false;
        }
        // 缓存里的记着增益（首次播放时算的，省得每次重算）
        if let Some((record, path)) = self.cache.as_ref().and_then(|c| c.get(&key)) {
            let gain_db = record.gain_db;
            self.last = Some(key);
            return self.play_path(&path, Some(gain_db));
        }
        // 整包库里的在构建时已经归一过，不用补增益
        if let Some(pronunciation) = self.library.as_ref().and_then(|l| l.get(&key)) {
            let audio = pronunciation.audio.to_vec();
            let extension = "ogg";
            self.last = Some(key);
            return self.play_bytes(&audio, extension);
        }
        false
    }

    /// 存一条刚下载好的发音，并按需立刻播放。
    ///
    /// 增益在这里算一次并写进缓存索引：下次播放直接用，不再解码量电平。
    pub fn store_and_speak(&mut self, word: &str, found: &FoundAudio, speak_now: bool) {
        let key = word.trim().to_lowercase();
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        // 先存文件（增益暂记 0）再量电平回填：量电平要能按路径打开文件
        if let Err(error) = cache.insert(
            &key,
            &found.audio,
            &found.extension,
            (&found.license, &found.author, &found.page),
            0.0,
        ) {
            tracing::warn!(%error, word = %key, "发音存不进缓存");
            return;
        }
        let Some((_, path)) = cache.get(&key).map(|(r, p)| (r.clone(), p)) else {
            return;
        };
        let Some((clip, gain_db)) = Clip::load(&path, None, TARGET_RMS_DBFS, MAX_GAIN_DB) else {
            tracing::warn!(word = %key, "下载的音频读不出样本，删掉");
            let _ = cache.forget(&key);
            return;
        };
        if let Err(error) = cache.set_gain(&key, gain_db) {
            tracing::warn!(%error, word = %key, "增益写不进缓存索引");
        }
        tracing::debug!(
            word = %key,
            gain_db,
            license = %found.license,
            bytes = found.audio.len(),
            "发音已缓存"
        );
        if speak_now {
            self.last = Some(key);
            self.output.play(&clip);
        }
    }

    /// 记下「上游没有这个词」，别反复问。
    pub fn note_missing(&mut self, word: &str) {
        let key = word.trim().to_lowercase();
        if let Some(cache) = self.cache.as_mut() {
            cache.note_missing(&key);
        }
        self.last = Some(key);
    }

    /// 记下「这个词已经在处理了」：请求发出去时调，防抖再到点也不会重复发。
    pub fn note_requested(&mut self, word: &str) {
        self.last = Some(word.trim().to_lowercase());
    }

    /// 停掉正在播的那段。
    pub fn stop(&mut self) {
        self.output.stop();
    }

    /// 忘掉「上次的词」：新一轮查询时调，这样重新敲出同一个词还会读。
    pub fn forget_last(&mut self) {
        self.last = None;
    }

    /// 缓存了多少词、占多少字节（偏好设置里显示）。
    pub fn cache_stats(&self) -> (usize, u64) {
        self.cache
            .as_ref()
            .map_or((0, 0), |c| (c.len(), c.audio_bytes()))
    }

    /// 清空缓存，返回清掉多少个词。
    pub fn clear_cache(&mut self) -> usize {
        let Some(cache) = self.cache.as_mut() else {
            return 0;
        };
        let count = cache.len();
        if let Err(error) = cache.clear() {
            tracing::warn!(%error, "发音缓存清不掉");
            return 0;
        }
        self.stop();
        self.last = None;
        count
    }

    /// 缓存里全部录音的署名（词、许可证、作者、来源页），偏好设置「关于」页导出用。
    pub fn credits(&self) -> Vec<(String, String, String, String)> {
        self.cache.as_ref().map_or(Vec::new(), |cache| {
            cache
                .entries()
                .into_iter()
                .map(|(word, record)| {
                    (
                        word.to_owned(),
                        record.license.clone(),
                        record.author.clone(),
                        record.page.clone(),
                    )
                })
                .collect()
        })
    }

    /// 放一个文件（缓存里的）。
    fn play_path(&mut self, path: &Path, gain_db: Option<f32>) -> bool {
        match Clip::load(path, gain_db, TARGET_RMS_DBFS, MAX_GAIN_DB) {
            Some((clip, _)) => self.output.play(&clip),
            None => false,
        }
    }

    /// 放一段内存里的音频（整包库里的）。
    ///
    /// `AVAudioFile` 只认路径，所以先落一个临时文件——整包库这条路平时用不到（缓存优先），
    /// 一个词一次几十毫秒，不值得为它引一套内存解码。
    fn play_bytes(&mut self, audio: &[u8], extension: &str) -> bool {
        let path = std::env::temp_dir().join(format!("qingjian-speak.{extension}"));
        if std::fs::write(&path, audio).is_err() {
            return false;
        }
        let played = self.play_path(&path, Some(0.0));
        let _ = std::fs::remove_file(&path);
        played
    }
}
