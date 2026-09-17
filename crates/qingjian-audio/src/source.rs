/// 一条发音的来源。编号写进文件，不要改已有编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Source {
    /// 真人录音（Wikimedia Commons / Lingua Libre，CC-BY-SA 或 CC0）。
    Human = 0,

    /// 本地合成（Kokoro-82M，Apache-2.0）。
    Synthetic = 1,
}

impl Source {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Human,
            1 => Self::Synthetic,
            _ => return None,
        })
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    /// 给用户看的一个词。
    pub fn label(self) -> &'static str {
        match self {
            Self::Human => "真人录音",
            Self::Synthetic => "合成发音",
        }
    }

    /// 是否需要在「关于」页列进署名清单（CC-BY-SA 要求署名，Apache-2.0 的合成音不要求）。
    pub fn needs_attribution(self) -> bool {
        matches!(self, Self::Human)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for source in [Source::Human, Source::Synthetic] {
            assert_eq!(Source::from_code(source.code()), Some(source));
        }
        assert_eq!(Source::from_code(9), None);
    }

    #[test]
    fn only_human_recordings_need_attribution() {
        assert!(Source::Human.needs_attribution());
        assert!(!Source::Synthetic.needs_attribution());
    }
}
