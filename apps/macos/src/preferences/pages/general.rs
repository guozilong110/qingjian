//! 「通用」页：学习语言、每页候选数、双拼方案、英文模式候选。

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField};
use objc2_foundation::NSString;
use qingjian_core::{Language, ShuangpinScheme};
use qingjian_platform::{Config, MAX_PAGE_SIZE};

use crate::preferences::controls::{
    button, checkbox, language_label, note, note_live, row_checkbox, row_popup, select, set_checked,
};
use crate::preferences::layout::{Layout, PAGE_PADDING, ROW_HEIGHT};
use crate::preferences::setting::Setting;
use crate::preferences::target::PreferencesTarget;

pub struct GeneralPage {
    /// 学习语言。
    learning_language: Retained<NSPopUpButton>,

    /// 每页候选数。
    page_size: Retained<NSPopUpButton>,

    /// 双拼方案（第 0 项是关）。
    shuangpin: Retained<NSPopUpButton>,

    /// 繁体输出模式。
    traditional: Retained<NSButton>,

    /// 英文模式也给候选。
    english: Retained<NSButton>,

    /// 终端 / 编辑器里不给英文候选。
    english_off_in_apps: Retained<NSButton>,

    /// 中英混输时中文候选排在英文词前。
    chinese_first: Retained<NSButton>,

    /// 高亮候选停住时读英语发音。
    speak: Retained<NSButton>,

    /// 发音缓存的状态（缓存了多少词、占多少空间）。
    cache_status: Retained<NSTextField>,

    /// 学习语言弹出菜单里各项对应的语言。
    languages: Vec<Language>,

    /// 默认中文标点模式。
    punctuation: Retained<NSPopUpButton>,
}

impl GeneralPage {
    /// `languages` 是打进包里的释义表语言。
    pub fn build(
        layout: &mut Layout,
        mtm: MainThreadMarker,
        target: &PreferencesTarget,
        languages: &[Language],
    ) -> Self {
        // 最后一项是关
        let language_titles: Vec<String> = languages
            .iter()
            .map(|l| language_label(*l).to_owned())
            .chain(std::iter::once("不显示译文".to_owned()))
            .collect();
        let learning_language = row_popup(
            layout,
            mtm,
            "学习语言",
            &language_titles,
            Setting::LearningLanguage,
            target,
        );
        note(
            layout,
            mtm,
            "候选词右侧显示哪种语言的译词，只列出安装了释义表的语言；「不显示译文」同时关掉生词标记与释义兜底。",
        );
        let page_size_titles: Vec<String> = (1..=MAX_PAGE_SIZE).map(|n| n.to_string()).collect();
        let page_size = row_popup(
            layout,
            mtm,
            "每页候选数",
            &page_size_titles,
            Setting::PageSize,
            target,
        );
        let shuangpin_titles: Vec<String> = std::iter::once("关（全拼）".to_owned())
            .chain(ShuangpinScheme::ALL.iter().map(|s| s.label().to_owned()))
            .collect();
        let shuangpin = row_popup(
            layout,
            mtm,
            "双拼",
            &shuangpin_titles,
            Setting::Shuangpin,
            target,
        );
        note(
            layout,
            mtm,
            "开双拼后 v、u、i 是音节键，表达式与问字模式只能用 ? 开头进；微软、搜狗方案的 ; 键是 ing。",
        );
        let punctuation = row_popup(
            layout,
            mtm,
            "默认中文标点",
            &["全角（，；：）".to_owned(), "半角（,;:）".to_owned()],
            Setting::FullWidthPunctuation,
            target,
        );
        note(
            layout,
            mtm,
            "仅影响标点，字母和数字保持半角；自定义短语原样输出。设置会保存。 ",
        );
        let traditional = checkbox(mtm, "繁体输出", Setting::Traditional, target);
        row_checkbox(layout, &traditional);
        let english = checkbox(
            mtm,
            "英文模式（Caps Lock）也给候选",
            Setting::EnglishCandidates,
            target,
        );
        row_checkbox(layout, &english);
        note(
            layout,
            mtm,
            "Tab 或方向键选词；空格、回车、标点仍原样上屏敲的字母，不选词时与直接打字一样。",
        );
        let english_off_in_apps = checkbox(
            mtm,
            "但在终端和代码编辑器里不给",
            Setting::EnglishCandidatesOffInApps,
            target,
        );
        row_checkbox(layout, &english_off_in_apps);
        note(
            layout,
            mtm,
            "终端、iTerm、Warp、Ghostty、VS Code、Cursor、Zed、JetBrains、Xcode 等，那里的候选窗口会挡住应用自己的补全；名单可在配置文件里改。",
        );
        let chinese_first = checkbox(
            mtm,
            "输入拼音时中文候选排在英文词前面",
            Setting::ChineseFirst,
            target,
        );
        row_checkbox(layout, &chinese_first);
        note(
            layout,
            mtm,
            "勾上后整段输入是英文词时（hello、key）英文词排第二，空格上屏的仍是中文；不勾（缺省）拼音不成立的输入英文词排第一。",
        );
        let speak = checkbox(
            mtm,
            "读出高亮候选的英语发音",
            Setting::SpeakCandidate,
            target,
        );
        row_checkbox(layout, &speak);
        note(
            layout,
            mtm,
            "高亮停住约三分之一秒才读，连着翻候选不会出声；中文候选读第一个英语译词，英文候选读词本身。要学习语言选英语，密码框里不发音。",
        );
        note(
            layout,
            mtm,
            "本机没有的词会向维基共享资源（Wikimedia Commons）请求一次该词的发音，下载后存在本机，之后不再联网；发音是真人朗读。",
        );
        let cache_status = note_live(layout, mtm, "发音缓存：还没有下载过发音");
        let clear_cache = button(mtm, "清空发音缓存", Setting::ClearAudioCache, target);
        layout.place(&clear_cache, PAGE_PADDING, 160.0, ROW_HEIGHT + 4.0);
        layout.next_row(ROW_HEIGHT + 4.0);
        Self {
            learning_language,
            page_size,
            shuangpin,
            traditional,
            english,
            english_off_in_apps,
            chinese_first,
            speak,
            cache_status,
            languages: languages.to_vec(),
            punctuation,
        }
    }

    /// `audio_cache` 是（缓存词数, 占用字节）。
    pub fn sync(&self, config: &Config, audio_cache: (usize, u64)) {
        let general = &config.general;
        select(
            &self.punctuation,
            Some(usize::from(!general.full_width_punctuation)),
        );
        select(
            &self.learning_language,
            if general.learning_language_off() {
                Some(self.languages.len())
            } else {
                self.languages
                    .iter()
                    .position(|l| l.code() == general.learning_language)
            },
        );
        select(&self.page_size, Some(general.page_size() - 1));
        select(
            &self.shuangpin,
            Some(general.shuangpin().map_or(0, |scheme| {
                ShuangpinScheme::ALL
                    .iter()
                    .position(|s| *s == scheme)
                    .map_or(0, |i| i + 1)
            })),
        );
        set_checked(&self.traditional, general.traditional);
        set_checked(&self.english, general.english_candidates);
        set_checked(
            &self.english_off_in_apps,
            config.apps.has_english_candidates_off(),
        );
        self.english_off_in_apps
            .setEnabled(general.english_candidates);
        set_checked(&self.chinese_first, general.chinese_first);
        set_checked(&self.speak, general.speak_candidate);
        // 发音只对英语译词有意义：学习语言不是英语时这个开关置灰
        self.speak
            .setEnabled(general.learning_language.trim().eq_ignore_ascii_case("en"));
        let (words, bytes) = audio_cache;
        let text = if words == 0 {
            "发音缓存：还没有下载过发音".to_owned()
        } else {
            format!("发音缓存：{words} 个词，{:.1} MB", bytes as f64 / 1e6)
        };
        self.cache_status.setStringValue(&NSString::from_str(&text));
    }
}
