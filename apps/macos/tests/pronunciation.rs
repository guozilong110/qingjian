//! 发音的集成测试：缓存目录、整包库、真实音频的解码与增益。
//!
//! 单元测试覆盖「该读哪个词」（`host::speaking`）与缓存索引（`qingjian-audio`），这里验的是
//! 跨层的那几件事：缓存能不能被打包工具接着用、真实录音能不能读出样本并算出合理增益。
//!
//! 没有音频素材（没跑过数据管线、也没下载过的机器）时相关测试跳过，不让它成为构建的硬依赖。

use std::path::PathBuf;

/// 仓库里的原始录音（构建管线的中间产物）；没有就返回 `None`。
fn sample_clips() -> Option<PathBuf> {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/generated/audio/human/clips");
    dir.is_dir().then_some(dir)
}

/// 用户机器上按需下载攒的缓存；没有就返回 `None`。
fn user_cache() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Application Support/Qingjian")
        .join(qingjian_audio::CACHE_DIR);
    dir.is_dir().then_some(dir)
}

#[test]
fn a_cache_directory_reports_its_contents() {
    let Some(dir) = user_cache() else {
        eprintln!("本机还没有发音缓存，跳过");
        return;
    };
    let cache = qingjian_audio::AudioCache::open(&dir).expect("缓存目录应该能打开");
    // 每条记录都必须带许可证与来源：CC BY-SA 要求逐条署名，缺了就是许可问题
    for (word, record) in cache.entries() {
        assert!(!record.license.is_empty(), "{word} 没记许可证");
        assert!(!record.file.is_empty(), "{word} 没记文件名");
        // 增益是播放时用的，必须在合理范围
        assert!(
            record.gain_db.abs() <= qingjian_audio::MAX_GAIN_DB + 0.01,
            "{word} 的增益 {} 超出上限",
            record.gain_db
        );
    }
    println!(
        "缓存里有 {} 个词，{} 字节",
        cache.len(),
        cache.audio_bytes()
    );
}

#[test]
fn real_recordings_decode_and_get_a_sane_gain() {
    let Some(dir) = sample_clips().or_else(user_cache) else {
        eprintln!("没有音频素材，跳过");
        return;
    };
    let mut checked = 0;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "ogg" | "wav" | "opus"))
        })
        .collect();
    entries.sort();
    for path in entries.iter().take(20) {
        // 这里只验「能不能算出增益」这一步；播放要 AVAudioEngine，交给手动验证
        let Some(rms) = read_rms_dbfs(path) else {
            continue;
        };
        assert!(
            (-50.0..0.0).contains(&rms),
            "{} 的 RMS 是 {rms}，不像人声",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "一个音频都没读出来");
    println!("读了 {checked} 个音频，RMS 都在人声范围内");
}

/// 用系统解码器读一段音频的 RMS。与壳里 `speech::audio` 同一套 API，
/// 这里重写一遍是为了让测试不依赖 bin crate 的私有模块。
fn read_rms_dbfs(path: &std::path::Path) -> Option<f32> {
    use objc2::AnyThread;
    use objc2_avf_audio::{AVAudioCommonFormat, AVAudioFile, AVAudioPCMBuffer};
    use objc2_foundation::{NSString, NSURL};

    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    let file = unsafe {
        AVAudioFile::initForReading_commonFormat_interleaved_error(
            AVAudioFile::alloc(),
            &url,
            AVAudioCommonFormat::PCMFormatFloat32,
            false,
        )
    }
    .ok()?;
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
    unsafe { file.readIntoBuffer_error(&buffer) }.ok()?;
    let count = unsafe { buffer.frameLength() } as usize;
    let data = unsafe { buffer.floatChannelData() };
    if count == 0 || data.is_null() {
        return None;
    }
    // SAFETY: 缓冲由上面这个文件填满，格式是非交错 f32。
    let samples = unsafe { std::slice::from_raw_parts((*data).as_ptr(), count) };
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / count as f64).sqrt();
    (rms > 0.0).then(|| 20.0 * rms.log10() as f32)
}
