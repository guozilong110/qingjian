# 单词发音

候选高亮停住不动就读那个英文词的发音，**音频按需下载**，不随包。
这一页记数据来源、按需下载的设计与实测逼出来的几个坑。代码位置见 [crate-notes.md](crate-notes.md) 的 `crates/qingjian-audio` 与 `apps/macos`。

## 为什么用真人录音而不是 TTS 模型

三条路线比过：

| 方案 | 质量 | 许可 | 体积 / 延迟 |
| --- | --- | --- | --- |
| 系统 TTS（`NSSpeechSynthesizer` / SAPI） | 够用，不好听；macOS 14 起 deprecated | 无问题 | 0 / 首次几十毫秒 |
| 本地神经 TTS（Piper / Kokoro） | 好，但同形异读靠猜 | Apache-2.0 可随包 | 模型 60–100 MB / 一两百毫秒 |
| **真人录音（Wikimedia Commons）** | **最好，学英语的价值在这** | CC-BY-SA / CC0，须逐条署名 | 约 2 KB/词 / 首次要下载 |

选真人录音，缺的词可以用 Kokoro-82M（Apache-2.0）本地合成补（脚本在 `tools/corpus/pronunciation_synth.py`，运行时不用）。
云 TTS（Azure / ElevenLabs）质量最高一档但没用：合成音频能不能持久化并随产品分发，各家条款写法不同，这个项目是 GPL 开源、音频跟着包走，没确认清楚不碰。

孤立单词读错的主要来源不是声学模型而是同形异读（read / lead / live / bow / desert）。真人录音天然没这个问题。

## 为什么按需下载而不是随包

一开始预建了 CEFR A1 的 1051 词（2.0 MB）随包，**这个方向是错的**：释义表里出现 33983 个不同英文单词，
A1 那份只覆盖 1003 个（3%）。释义表最高频的 30 个词里 17 个不在 A1 里（moth / goby / crab / jade / beetle 这类——
中文虫鱼类字多，它们的英文译词自然高频）。也就是说预建集合不管怎么选都对不上真实需要。

体积上全量约 187 MB（按实测 1975 字节/词 × 94569 词），随包不可能。所以改成：

- 高亮到一个本地没有的词 → 后台线程去 Commons 抓 → 存进 `~/Library/Application Support/Qingjian/audio-cache/` → 之后瞬时；
- 缓存是普通目录（一个词一个原始文件 + `index.tsv`），不是 `.qj`——容器的设计是「只整体替换」的不可变文件，不适合边下边写；
- 攒够了可以 `dict-convert pack audio --input <缓存目录> --manifest <缓存目录>/index.tsv` 打成离线 `.qj`，
  放到用户数据目录壳会优先用它（也方便给别人一份现成的）。

首次读一个新词实测 0.6–2.7 秒（查接口 + 下文件），之后瞬时。没网时只有已缓存的词有声音。

### 隐私

按需下载意味着**「正在看哪个英文词」会以 HTTP 请求到达 Wikimedia**。这是发音功能新增的外发通道，所以：

- 开关缺省关（`[general] speak_candidate = false`）；
- 配置模板、偏好设置「通用」页、用户文档都写明「会向维基共享资源请求该词的发音」；
- 私密输入（Secure Input / `Engine::is_private`）下既不发音也不请求；
- 问过、上游确实没有的词记在内存里，不反复请求。

## 音量：运行时补增益，不重新编码

下载来的原始录音电平实测跨 **25.5 dB**（-36.6 到 -11.1 dBFS，300 个样本），不处理就是一个词轰、一个词听不见。
构建时那条管线是重新编码来归一的，运行时不行——**不能假设用户机器上有 ffmpeg**。试过两条路都不通：

- **用 `AVAudioFile` 写 AAC 回盘**：采样率与封装参数要自己对齐，写出来的文件时长翻倍（声明 24 kHz 却没重采样）或者根本打不开（`error -50`）。为一个词读音不值得。
- **用 `AVAudioPlayer.volume` 放大**：Apple 文档写明有效范围是 0.0–1.0，超出是未定义行为（实测能设进去也能读回来，但不能依赖）。而多数录音**偏轻、需要正增益**，只靠衰减就得把所有词压到 -36 dB 那么小声。

最后的做法：解码成 f32 样本 → 增益直接乘在样本上（越界夹住防削顶）→ `AVAudioEngine` + `AVAudioPlayerNode` 播。
增益值在首次播放时算一次、写进缓存索引，之后不再解码量电平。一个词几十毫秒，主线程做也不挡按键回调。

裁首尾静音与限幅运行时省了：静音只影响「按下到出声」的几十毫秒，限幅是为了把响度顶到极限，按 RMS 补增益已经够用。

## 预建整包库（可选）

想给别人一份现成的、或者自己要离线用，还是可以批量预建：

```bash
# 1. 抓（带退避重试与断点续跑；manifest.tsv 记许可证与作者）
uv run tools/corpus/pronunciation_fetch.py <词表> --out-dir data/generated/audio/human
# 2. 归一（Opus 24 kbps 单声道，-20 dBFS RMS，裁首尾静音）
tools/corpus/pronunciation-normalize.sh data/generated/audio/human/clips data/generated/audio/human-opus
# 3. 打包（真人优先、合成补缺，同时生成署名清单）
cargo run --release -p qingjian-dict-convert -- pack audio \
    --input data/generated/audio/human-opus --manifest data/generated/audio/human/manifest.tsv \
    --name "青简单词发音库（英语）" --license "CC-BY-SA-4.0 AND CC-BY-4.0 AND CC0-1.0"
```

CEFR A1 实测：**1051 / 1057 词有免费真人录音（99.4%）**，源音频 22 MB → `.qj` 2.0 MB。
口音不是纯美音：942 个 `En-us-*` 美音、24 个只有英音（school / food / black 这些）、85 个 Lingua Libre（22 个录音人，混杂）。
抓取脚本按 `En-us → En-uk → Lingua Libre` 的优先级取，所以美音优先但不保证。

第一次跑只拿到 155 个（15%），当时以为是「派生词大面积缺」——其实是把 `--sleep` 设成 0.05 触发了 Wikimedia 限速，
一整批 429 被当成「没有录音」记进了 skipped.tsv。**看到覆盖率异常低先查 skipped.tsv 里的原因，不要直接下结论。**

## 归一管线实测踩的坑

四个都是在真实录音上跑出来的（构建时那条管线），改回「常见做法」会重新踩：

- **`loudnorm` 在短音频上给废值。** EBU R128 的积分窗是 400 ms，而单词多在 0.3–1 秒，`world.wav`（0.78 秒）测出来是 -70 LUFS。
  改成两遍：`volumedetect` 量 `mean_volume`，再补固定增益到 -20 dBFS。
- **直流偏置会让人声轻 14 dB。** `black.ogg` 带 -0.098 的直流偏置，它把 WAV 上量到的 RMS 抬高十几 dB，而 Opus 编码时又把直流去掉，
  结果按 WAV 定的增益让成品是 -34.5 dB 而不是 -20。所以量电平之前先 `highpass=f=60` 去直流。
- **`silenceremove` 的 `stop_periods` 会把词切断。** 它把词中间的爆破音停顿当结尾，`computer` 被切成 0.03 秒。
  改成裁开头 + `areverse` 再裁一次 + 转回来。
- **只按峰值留余量，响度就永远上不来。** 清音开头（cat、catch）峰值远高于 RMS，按峰值定增益的话它们停在 -24 dB。
  改成按 RMS 定增益、后面跟 `alimiter` 压那几个瞬时峰。

归一后 1051 个片段的响度落在 -22.9 ~ -19.9 dBFS（3 dB 内）；改之前是 15 dB 跨度加一个 -34.5 dB 的离群值。

## 封装为什么是 Ogg Opus

`hello`（0.49 秒）三种封装实测：**Ogg Opus 1.6 KB** < AAC M4A 2.7 KB < CAF Opus 5.6 KB（`afconvert` 出的，每包带头）。
ffmpeg 的 CAF 封装器写不了 Opus（`muxing codec currently unsupported`），所以 CAF 那条本来也走不通。
`NSSound(data:)` 与 `AVAudioPlayer(data:)` 都能直接放 Ogg Opus。按需下载那条路存的是**原始文件**（ogg / wav），不转码。

## 署名

CC-BY-SA / CC-BY 要求逐条署名。两条路各有出口：

- 缓存（按需下载）：许可证、作者、来源页都记在 `index.tsv` 里，偏好设置「关于」页的「导出发音署名清单」写成桌面上的 Markdown；
- 整包库：`pack audio` 按实际打进包的录音生成 `audio-attribution.md`。

有个坑钉了测试：`word` 本身就是一个词，早先按 `word\t` 前缀跳过 manifest 表头，把 `word` 这条录音的署名漏掉了——
漏一条就是许可问题。现在只认「第一行且正好是表头」，`the_word_word_is_still_credited` 盯着它。
