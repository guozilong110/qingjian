//! 解码、补增益、播放：一段音频读进内存 → 按目标响度调样本 → 交给 `AVAudioEngine` 放。
//!
//! 为什么是这条路而不是别的两条，都是实测排掉的：
//! - **不重新编码**。运行时不能假设用户机器上有 ffmpeg（作者本机那个是 Homebrew 装的）；
//!   试过用 `AVAudioFile` 写 AAC 回盘，采样率与封装参数要自己对齐，写出来的文件时长翻倍或者根本打不开，
//!   为一个词读音不值得走这条。原始 ogg / wav 原样存着就好。
//! - **不用 `AVAudioPlayer.volume` 放大**。Apple 文档写明它的有效范围是 0.0–1.0，超出是未定义行为
//!   （实测能设进去也能读回来，但不能依赖）。而下载来的录音实测跨 25.5 dB（-36.6 到 -11.1 dBFS），
//!   多数偏轻、需要**正**增益，只靠衰减就得把所有词压到 -36 dB 那么小声。
//!
//! 所以增益直接乘在解码后的 f32 样本上（越界夹住防削顶），再用 `AVAudioEngine` + `AVAudioPlayerNode` 播。
//! 一次一个词，几十毫秒的事，都在主线程做也不会挡住 IMK 的按键回调。

use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioCommonFormat, AVAudioEngine, AVAudioFile, AVAudioPCMBuffer, AVAudioPlayerNode,
};
use objc2_foundation::{NSString, NSURL};

/// 静音或近乎静音的判据：低于这个 RMS 不补增益（补了只是放大底噪）。
const SILENCE_FLOOR_DBFS: f32 = -60.0;

/// 一段准备好播放的音频：已经补过增益的样本缓冲。
pub struct Clip {
    buffer: Retained<AVAudioPCMBuffer>,
}

impl Clip {
    /// 读一个音频文件，按 `target_dbfs` 补增益（夹在 `±max_gain_db`）。
    ///
    /// `gain_db` 给了就用它（缓存里记着的，省一次量电平），`None` 表示现算。
    /// 返回缓冲与实际用的增益，调用方可以把它写回缓存。
    pub fn load(
        path: &Path,
        gain_db: Option<f32>,
        target_dbfs: f32,
        max_gain_db: f32,
    ) -> Option<(Self, f32)> {
        let buffer = decode(path)?;
        let gain = match gain_db {
            Some(gain) => gain,
            None => {
                let rms = rms_dbfs(&buffer)?;
                if rms < SILENCE_FLOOR_DBFS {
                    0.0
                } else {
                    (target_dbfs - rms).clamp(-max_gain_db, max_gain_db)
                }
            }
        };
        apply_gain(&buffer, gain);
        Some((Self { buffer }, gain))
    }
}

/// 播放器：一个常驻的 `AVAudioEngine` 加一个播放节点。
///
/// 引擎按需启动（第一次真要出声时才 start），之后留着不停——反复 start / stop 会有几十毫秒开销与咔哒声。
pub struct Output {
    engine: Retained<AVAudioEngine>,

    node: Retained<AVAudioPlayerNode>,

    /// 引擎跑起来了没。
    running: bool,

    /// 已经按这个格式接过线；格式变了要重接（不同录音的采样率不同）。
    connected: Option<(f64, u32)>,
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Output {
    pub fn new() -> Self {
        let engine = unsafe { AVAudioEngine::new() };
        let node = unsafe { AVAudioPlayerNode::new() };
        // SAFETY: 刚建好的引擎与节点，attach 只是登记。
        unsafe { engine.attachNode(&node) };
        Self {
            engine,
            node,
            running: false,
            connected: None,
        }
    }

    /// 放一段。返回是否真放了。
    pub fn play(&mut self, clip: &Clip) -> bool {
        let format = unsafe { clip.buffer.format() };
        let rate = unsafe { format.sampleRate() };
        let channels = unsafe { format.channelCount() };
        // 采样率或声道数与上次不同就重接线：接线格式必须与缓冲一致，否则 scheduleBuffer 会抛异常
        if self.connected != Some((rate, channels)) {
            if self.running {
                unsafe { self.engine.stop() };
                self.running = false;
            }
            let mixer = unsafe { self.engine.mainMixerNode() };
            // SAFETY: 两个节点都 attach 在这个引擎上；格式取自要播的缓冲。
            unsafe {
                self.engine
                    .connect_to_format(&self.node, &mixer, Some(&format))
            };
            self.connected = Some((rate, channels));
        }
        if !self.running {
            // SAFETY: 接线已完成；失败返回 Err，引擎保持未启动。
            if let Err(error) = unsafe { self.engine.startAndReturnError() } {
                tracing::warn!(%error, "音频引擎起不来，不发音");
                return false;
            }
            self.running = true;
        }
        // 上一个词还在放就掐掉：候选往下翻时该让位
        unsafe {
            self.node.stop();
            self.node
                .scheduleBuffer_completionHandler(&clip.buffer, std::ptr::null_mut());
            self.node.play();
        }
        true
    }

    /// 停掉正在播的那段（引擎留着）。
    pub fn stop(&mut self) {
        if self.running {
            unsafe { self.node.stop() };
        }
    }
}

/// 解码整个文件成 f32 样本。单词发音都在两秒内，几百 KB 的样本，整段读进来最简单。
fn decode(path: &Path) -> Option<Retained<AVAudioPCMBuffer>> {
    let text = path.to_str()?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    // SAFETY: 只读打开一个本地文件；失败返回 Err。
    let file = unsafe {
        AVAudioFile::initForReading_commonFormat_interleaved_error(
            AVAudioFile::alloc(),
            &url,
            AVAudioCommonFormat::PCMFormatFloat32,
            false,
        )
    };
    let file = match file {
        Ok(file) => file,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "音频打不开");
            return None;
        }
    };
    let format = unsafe { file.processingFormat() };
    let frames = u32::try_from(unsafe { file.length() }).ok()?;
    if frames == 0 {
        return None;
    }
    let buffer = unsafe {
        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            frames,
        )
    }?;
    // SAFETY: 缓冲的格式与容量都取自这个文件。
    if let Err(error) = unsafe { file.readIntoBuffer_error(&buffer) } {
        tracing::debug!(path = %path.display(), %error, "音频读不出样本");
        return None;
    }
    Some(buffer)
}

/// 样本区的形状：每声道一个首地址，每个指向 `frames` 个 f32。
///
/// 不直接返回 `&mut [f32]`：那要从 `&AVAudioPCMBuffer` 里变出可变借用，不成立（AppKit 对象是共享可变的）。
/// 交给调用方拿裸指针在 `unsafe` 里用，各自写清 SAFETY。
struct Samples {
    /// 每声道的首地址。
    channels: Vec<*mut f32>,

    /// 每声道多少个样本。
    frames: usize,
}

/// 取样本区。`buffer` 必须是 [`decode`] 出来的（非交错 f32、已填满）。
fn samples_of(buffer: &Retained<AVAudioPCMBuffer>) -> Option<Samples> {
    let frames = unsafe { buffer.frameLength() } as usize;
    let channel_count = unsafe { buffer.format().channelCount() } as usize;
    let data = unsafe { buffer.floatChannelData() };
    if frames == 0 || channel_count == 0 || data.is_null() {
        return None;
    }
    let channels = (0..channel_count)
        // SAFETY: floatChannelData 给的是 channelCount 个指针的数组。
        .map(|channel| unsafe { *data.add(channel) }.as_ptr())
        .collect();
    Some(Samples { channels, frames })
}

/// 算一段音频的 RMS（dBFS）；只看第一个声道（发音录音基本单声道）。
fn rms_dbfs(buffer: &Retained<AVAudioPCMBuffer>) -> Option<f32> {
    let samples = samples_of(buffer)?;
    let first = *samples.channels.first()?;
    // SAFETY: samples_of 确认了非空且每声道有 frames 个 f32；这里只读。
    let slice = unsafe { std::slice::from_raw_parts(first, samples.frames) };
    let sum: f64 = slice.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / samples.frames as f64).sqrt();
    (rms > 0.0).then(|| 20.0 * rms.log10() as f32)
}

/// 把增益乘到样本上，越界夹住防削顶。
fn apply_gain(buffer: &Retained<AVAudioPCMBuffer>, gain_db: f32) {
    if gain_db.abs() < 0.01 {
        return;
    }
    let Some(samples) = samples_of(buffer) else {
        return;
    };
    let factor = 10f32.powf(gain_db / 20.0);
    for channel in samples.channels {
        // SAFETY: samples_of 确认了每声道有 frames 个 f32；缓冲是本调用链里刚解码出来的，
        // 此刻还没交给引擎，没有别处在读写它。
        let slice = unsafe { std::slice::from_raw_parts_mut(channel, samples.frames) };
        for sample in slice.iter_mut() {
            *sample = (*sample * factor).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 找一个真实录音；没有就跳过。
    fn sample() -> Option<std::path::PathBuf> {
        let clips = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/generated/audio/human/clips");
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(clips)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ogg" || e == "wav"))
            .collect();
        found.sort();
        found.into_iter().next()
    }

    #[test]
    fn loading_a_recording_computes_a_sane_gain() {
        let Some(path) = sample() else {
            eprintln!("没有录音样本，跳过");
            return;
        };
        let (_, gain) = Clip::load(&path, None, -20.0, 18.0).expect("真实录音应该读得出");
        assert!(gain.abs() <= 18.0, "增益 {gain} 超出上限");
        // 下载来的录音基本都偏轻，需要正增益；真有很响的也不该超过上限
        assert!(gain > -18.1 && gain < 18.1);
    }

    #[test]
    fn a_given_gain_is_used_as_is() {
        let Some(path) = sample() else {
            eprintln!("没有录音样本，跳过");
            return;
        };
        let (_, gain) = Clip::load(&path, Some(4.5), -20.0, 18.0).unwrap();
        assert_eq!(gain, 4.5);
    }

    #[test]
    fn unreadable_files_yield_nothing() {
        assert!(Clip::load(std::path::Path::new("/nonexistent.ogg"), None, -20.0, 18.0).is_none());
    }
}
