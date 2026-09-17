#!/usr/bin/env bash
# 把抓来的发音统一成一样的音量、采样率和封装：Opus 24 kbps 单声道 Ogg，裁掉首尾静音。
#
# 真人录音来自不同录音人和年代，电平能差 20 dB，前后静音长度也各不相同；不归一的话候选悬停时
# 一个词轰一个词听不见。合成音也走同一条路，两种来源混播才不割裂。
#
# 三处与「照抄常见做法」不同，都是在 CEFR A1 那 1051 个真实录音上实测逼出来的：
#   * 先过 highpass 去直流：有些录音带很大的直流偏置（black.ogg 是 -0.098），
#     偏置会把 WAV 上量出来的 RMS 抬高十几 dB，而 Opus 编码时又把直流去掉，
#     结果按 WAV 定的增益让人声轻了 14 dB（那条最后是 -34.5 dB 而不是 -20）。
#   * 音量不用 loudnorm：EBU R128 的积分窗是 400 ms，而单词多在 0.3–1 秒，短的片段测出来是 -70 LUFS 那种废值。
#     改成两遍——先 volumedetect 量 mean_volume，再补固定增益到 TARGET_RMS。
#   * 裁静音不用 silenceremove 的 stop_periods：它把词中间的爆破音停顿当结尾，实测 computer 被切成 0.03 秒。
#     改成裁开头 + areverse 再裁一次 + 转回来。
#
# 增益后跟一个 alimiter：清音开头（cat、catch）的峰值远高于 RMS，光按峰值留余量的话响度就永远压在 -24 dB 上不来；
# 让限幅器压住那几个瞬时峰，整体响度才能对齐。
#
# 封装选 Ogg 而不是 CAF：本机实测 ffmpeg 的 CAF 封装器写不了 Opus（muxing codec currently unsupported），
# 而 `.opus` 一个词平均 4 KB，比 afconvert 出的 CAF Opus 与 AAC M4A 都小，
# NSSound(data:) 与 AVAudioPlayer(data:) 都能直接放。
#
# 用法：tools/corpus/pronunciation-normalize.sh <输入目录> <输出目录>
# 输入目录里是 ogg / oga / wav / mp3 / flac，输出目录里是同名 .opus。
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "用法: $0 <输入目录> <输出目录>" >&2
    exit 1
fi

src="$1"
dst="$2"
mkdir -p "$dst"

# 目标 RMS（dBFS）：语音素材 -20 dBFS RMS 大致对应 -16 LUFS
readonly TARGET_RMS=-20
# 限幅门限（线性，0.84 ≈ -1.5 dBFS）：给 Opus 编码留过冲余量
readonly LIMIT=0.84

# 去直流（60 Hz 以下切掉，语音基频最低也在 80 Hz 上）
readonly DC_BLOCK="highpass=f=60"
# 裁静音只裁两端：裁开头 → 反转 → 再裁一次（原来的结尾）→ 反转回来
readonly TRIM="silenceremove=start_periods=1:start_duration=0:start_threshold=-45dB"
# 裁完两端各补 30 ms 静音，播放器起停不咔哒
readonly PAD="adelay=30,apad=pad_dur=0.03"

converted=0
failed=0
shopt -s nullglob
for input in "$src"/*.ogg "$src"/*.oga "$src"/*.wav "$src"/*.mp3 "$src"/*.flac; do
    name="$(basename "${input%.*}")"
    output="$dst/$name.opus"
    if [[ -f "$output" ]]; then
        continue
    fi

    trimmed="$(mktemp -t qj-pron).wav"
    if ! ffmpeg -nostdin -loglevel error -y -i "$input" \
        -af "$DC_BLOCK,$TRIM,areverse,$TRIM,areverse,$PAD,aresample=24000" \
        -ac 1 -f wav "$trimmed" 2>/dev/null; then
        echo "裁剪失败: $input" >&2
        rm -f "$trimmed"
        failed=$((failed + 1))
        continue
    fi

    # 量电平：mean_volume 是 RMS，相对满刻度。直流已经去掉，这个值才对得上解码后的响度
    mean="$(ffmpeg -nostdin -i "$trimmed" -af volumedetect -f null - 2>&1 |
        sed -n 's/.*mean_volume: \(-*[0-9.]*\) dB.*/\1/p' | tail -1)"
    if [[ -z "$mean" ]]; then
        echo "测电平失败: $input" >&2
        rm -f "$trimmed"
        failed=$((failed + 1))
        continue
    fi
    gain="$(awk -v mean="$mean" -v target="$TARGET_RMS" 'BEGIN { printf "%.2f", target - mean }')"

    if ffmpeg -nostdin -loglevel error -y -i "$trimmed" \
        -af "volume=${gain}dB,alimiter=limit=${LIMIT}:level=disabled" \
        -ac 1 -c:a libopus -b:a 24k -vbr on \
        -f ogg "$output" 2>/dev/null; then
        converted=$((converted + 1))
    else
        echo "编码失败: $input" >&2
        rm -f "$output"
        failed=$((failed + 1))
    fi
    rm -f "$trimmed"
done

total_kb=$(du -sk "$dst" | cut -f1)
echo "已转换 $converted 个，失败 $failed 个，输出共 ${total_kb} KB（${dst}）" >&2
