use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// 音频编码。编号写进文件，不要改已有编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Codec {
    /// Opus 装在 Ogg 容器里（`.opus`）：本机实测三种封装里体积最小（24 kbps 单声道，半秒的词约 1.6 KB），
    /// `NSSound(data:)` 与 `AVAudioPlayer(data:)` 都能直接放。
    OpusOgg = 0,

    /// AAC 装在 M4A 里：Opus 放不出时的退路，同码率大三分之二。
    AacM4a = 1,
}

impl Codec {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::OpusOgg,
            1 => Self::AacM4a,
            _ => return None,
        })
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    /// 生成时的文件扩展名，打包工具按它认目录里的文件。
    pub fn extension(self) -> &'static str {
        match self {
            Self::OpusOgg => "opus",
            Self::AacM4a => "m4a",
        }
    }
}

/// 一条发音在音频 blob 里的位置。16 字节、无填充，原样落盘。
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub(crate) struct Entry {
    /// 词形（小写）在词形 arena 里的字节偏移。
    pub word_start: u32,

    /// 音频在 blob 里的字节偏移。
    pub audio_start: u32,

    /// 音频字节数。
    pub audio_len: u32,

    /// 词形字节长度。
    pub word_len: u16,

    /// [`Codec`] 编号。
    pub codec: u8,

    /// [`super::Source`] 编号。
    pub source: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_is_sixteen_bytes_without_padding() {
        assert_eq!(size_of::<Entry>(), 16);
    }

    #[test]
    fn codec_codes_round_trip() {
        for codec in [Codec::OpusOgg, Codec::AacM4a] {
            assert_eq!(Codec::from_code(codec.code()), Some(codec));
        }
        assert_eq!(Codec::from_code(7), None);
    }
}
