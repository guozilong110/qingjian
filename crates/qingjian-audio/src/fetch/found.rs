/// 抓到的一条发音：音频字节与署名所需的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundAudio {
    /// 编码好的音频，原样存进缓存、原样交给平台播放器。
    pub audio: Vec<u8>,

    /// 文件扩展名（`ogg` / `wav`），缓存按它命名与认编码。
    pub extension: String,

    /// 许可证短名（`CC BY-SA 3.0`），原样来自上游。
    pub license: String,

    /// 作者 / 录音人。CC-BY 系要求署名，所以必须带回来。
    pub author: String,

    /// 文件页 URL，署名清单里给出处。
    pub page: String,
}
