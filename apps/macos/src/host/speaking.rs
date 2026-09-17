//! 单词发音：高亮停住后读哪个词、本地没有时去下载。
//!
//! 「读哪个词」这一个判断放在 [`word_to_speak`] 里，是个不碰 Host 的纯函数，规则三条：
//! - 英文候选（`CandidateKind::English`）读候选本身；
//! - 别的候选读第一个英语译词（学习语言必须是 en：发音库只有英语，拿英语音读日语 / 西班牙语译词是错的）；
//! - 借云端候选的壳显示的提示（「翻译中…」这类）不是词，不读。
//!
//! 判断只看 `Candidate` 里已有的字段，不查词库也不调翻译——那些是 Core 的事，平台层照拿就行。
//!
//! 本地没有的词会**发一次网络请求**到 Wikimedia。私密输入下一律不发（连本地播放也停）：
//! 密码框旁边出声是事故，而请求本身还会把用户在打的东西泄出去。

use qingjian_core::{Candidate, CandidateKind, Language};

use super::*;

impl Host {
    /// 高亮变了（新一轮候选、方向键、翻页）：重新起防抖。开关关着就不起。
    pub fn schedule_speech(&mut self) {
        if !self.speak_candidate {
            return;
        }
        self.speak.schedule();
    }

    /// 防抖到点：本地有就读，没有就去下载。
    pub fn speak_highlighted(&mut self) {
        self.speak.cancel_debounce();
        if !self.speak_candidate {
            return;
        }
        // 私密输入（密码框）：不出声、更不发请求
        if self.engine.is_private() {
            self.speaker.stop();
            return;
        }
        let Some(word) = self.highlighted_word() else {
            return;
        };
        // 这个词刚处理过（翻页回来、防抖重复到点）：不再读也不再请求
        if self.speaker.is_repeat(&word) {
            return;
        }
        if self.speaker.speak_local(&word) {
            return;
        }
        // 本地没有：问过上游确实没有的不再问
        if self.speaker.is_known_missing(&word) {
            return;
        }
        let Some(downloader) = &self.downloader else {
            return;
        };
        // 记成「已请求」，防抖再到点不会重复发
        self.speaker.note_requested(&word);
        downloader.request(&word);
        self.speak.start_polling();
    }

    /// 轮询下载结果：到了就存进缓存并播放（前提是用户还停在同一个词上）。
    pub fn poll_pronunciation(&mut self) {
        let Some(downloader) = &self.downloader else {
            self.speak.stop_polling();
            return;
        };
        let Some((word, found)) = downloader.poll() else {
            if self.speak.expired() {
                tracing::debug!("发音下载等太久，停轮询");
                self.speak.stop_polling();
            }
            return;
        };
        self.speak.stop_polling();
        // 结果回来时用户可能已经翻到别的词了：那就只存不放
        let still_looking = self
            .highlighted_word()
            .is_some_and(|current| current.eq_ignore_ascii_case(&word))
            && !self.engine.is_private();
        match found {
            Some(found) => self.speaker.store_and_speak(&word, &found, still_looking),
            None => {
                tracing::debug!(word = %word, "上游没有这个词的免费录音");
                self.speaker.note_missing(&word);
            }
        }
    }

    /// 停发音并忘掉「上次的词」：新一轮查询、收窗时调，这样重新敲出同一个词还会读。
    pub fn reset_speech(&mut self) {
        self.speak.stop();
        self.speaker.stop();
        self.speaker.forget_last();
    }

    /// 把缓存里的录音署名写成 Markdown 放到桌面。真人录音多为 CC BY-SA，要求逐条署名，
    /// 而缓存是按需下载的、内容因人而异，所以清单得能随时导出（随包数据那张表写死在「关于」页里）。
    pub fn export_audio_credits(&mut self) {
        let credits = self.speaker.credits();
        if credits.is_empty() {
            self.preferences
                .set_status("还没有下载过发音，没有可导出的署名");
            return;
        }
        let mut text = String::from("# 单词发音署名清单\n\n");
        text.push_str(&format!(
            "本机已下载 {} 条真人录音，来自 Wikimedia Commons。CC BY-SA / CC BY 要求逐条署名。\n\n",
            credits.len()
        ));
        text.push_str("| 词 | 许可证 | 录音人 | 来源 |\n| --- | --- | --- | --- |\n");
        for (word, license, author, page) in &credits {
            text.push_str(&format!("| {word} | {license} | {author} | {page} |\n"));
        }
        let Some(desktop) = std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join("Desktop/青简发音署名清单.md"))
        else {
            return;
        };
        match std::fs::write(&desktop, text) {
            Ok(()) => {
                tracing::info!(path = %desktop.display(), recordings = credits.len(), "发音署名清单已导出");
                self.preferences
                    .set_status(&format!("已导出 {} 条录音的署名到桌面", credits.len()));
            }
            Err(error) => {
                tracing::warn!(%error, "发音署名清单写不出去");
                self.preferences.set_status("署名清单导出失败，见日志");
            }
        }
    }

    /// 当前高亮候选该读的英文词。
    fn highlighted_word(&self) -> Option<String> {
        let candidate = self.session.candidate(self.session.highlighted)?;
        // 提示气泡与「翻译中…」都借云端候选的壳显示，不是词
        let placeholder = self.translation.is_some() || self.notice.is_some();
        word_to_speak(&candidate, self.learning_language, placeholder)
    }
}

/// 这个候选该读哪个英文词；没有可读的返回 `None`。
///
/// `learning_language` 是当前学习语言（`None` 为关），`placeholder` 表示候选窗口正显示的是提示而不是候选。
fn word_to_speak(
    candidate: &Candidate,
    learning_language: Option<Language>,
    placeholder: bool,
) -> Option<String> {
    match candidate.kind {
        // 英文候选本身就是英文词
        CandidateKind::English => Some(candidate.text.clone()),
        CandidateKind::Cloud if placeholder => None,
        _ => {
            // 发音只有英语：学习语言不是 en 时右侧译词不是英文，不能拿去读
            if learning_language != Some(Language::English) {
                return None;
            }
            // 译词可能是词组（give up）；按单词收的音查不到，抓取那边也会直接判定「没有」
            Some(
                candidate
                    .translation
                    .as_ref()?
                    .senses()
                    .first()?
                    .text
                    .clone(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qingjian_core::{Sense, Translation};

    fn candidate(kind: CandidateKind, text: &str, gloss: Option<&str>) -> Candidate {
        Candidate {
            text: text.to_owned(),
            kind,
            syllables: vec!["a".into()],
            reading: None,
            translation: gloss.map(|gloss| {
                Translation::new(
                    Language::English,
                    vec![Sense {
                        part_of_speech: None,
                        text: gloss.to_owned(),
                        reading: None,
                        fresh: false,
                    }],
                )
            }),
        }
    }

    #[test]
    fn english_candidates_are_read_as_themselves() {
        let word = candidate(CandidateKind::English, "develop", None);
        assert_eq!(
            word_to_speak(&word, Some(Language::English), false).as_deref(),
            Some("develop")
        );
        // 英文候选不依赖学习语言：它本来就是英文
        assert_eq!(
            word_to_speak(&word, None, false).as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn chinese_candidates_are_read_through_the_first_english_gloss() {
        let word = candidate(CandidateKind::Chinese, "开发", Some("develop"));
        assert_eq!(
            word_to_speak(&word, Some(Language::English), false).as_deref(),
            Some("develop")
        );
        // 学习语言不是英语时右侧译词不是英文，不能拿英语音去读
        assert_eq!(word_to_speak(&word, Some(Language::Japanese), false), None);
        assert_eq!(word_to_speak(&word, None, false), None);
        // 没有译词就没什么可读
        let plain = candidate(CandidateKind::Chinese, "开发", None);
        assert_eq!(word_to_speak(&plain, Some(Language::English), false), None);
    }

    #[test]
    fn notices_borrowing_the_cloud_row_are_not_words() {
        let notice = candidate(CandidateKind::Cloud, "翻译中…", None);
        assert_eq!(word_to_speak(&notice, Some(Language::English), true), None);
        // 真的云端候选带译词时照读
        let cloud = candidate(CandidateKind::Cloud, "开发", Some("develop"));
        assert_eq!(
            word_to_speak(&cloud, Some(Language::English), false).as_deref(),
            Some("develop")
        );
    }
}
