# /// script
# requires-python = ">=3.10"
# dependencies = ["kokoro>=0.9", "soundfile>=0.12", "torch>=2.2"]
# ///
"""用 Kokoro-82M（Apache-2.0）本地合成真人录音缺的那些词，输出 wav（再交给 pronunciation-normalize.sh）。

为什么本地合成而不是云 TTS：Kokoro 是 Apache-2.0，生成的音频没有再分发限制，随 GPL 包发不用逐条确认条款；
质量在读单词这件事上够用，且不用联网、可复现。

孤立单词最容易读错的是同形异读（read / lead / live / bow / desert），声学模型救不了，只能给音素。
所以这里支持第二列直接给 IPA：`词\\t音素`（有音素就按音素合成），没有音素才让模型自己按拼写猜。

用法：
    # 先算出真人录音缺哪些词
    uv run tools/corpus/pronunciation_synth.py --missing english.tsv --have data/generated/audio/human/manifest.tsv -o missing.tsv
    # 再合成
    uv run tools/corpus/pronunciation_synth.py missing.tsv --out-dir data/generated/audio/synth-wav

第一次跑会下载模型（约 330 MB，到 ~/.cache/huggingface），只在构建机上需要，不进安装包。
"""

import argparse
import sys
from pathlib import Path


def write_missing(word_list: Path, have: Path, output: Path) -> None:
    """算差集：词表里有、真人录音里没有的词。"""
    def first_column(path: Path) -> set[str]:
        words = set()
        with open(path, encoding="utf-8") as src:
            for index, line in enumerate(src):
                line = line.strip()
                # 表头只按「第一行且正好是表头」认：word 自己也是一个词
                if not line or line.startswith("#") or (index == 0 and line.startswith("word\tfile\t")):
                    continue
                words.add(line.split("\t")[0].strip().lower())
        return words

    wanted = first_column(word_list)
    covered = first_column(have)
    missing = sorted(wanted - covered)
    output.write_text("\n".join(missing) + "\n", encoding="utf-8")
    print(
        f"{len(wanted)} wanted, {len(covered)} covered, {len(missing)} missing -> {output}",
        file=sys.stderr,
    )


def synthesize(words: list[tuple[str, str | None]], out_dir: Path, voice: str) -> None:
    import soundfile as sf
    from kokoro import KPipeline

    # lang_code "a" 是美音，"b" 是英音；voice 决定具体发音人
    pipeline = KPipeline(lang_code="a" if voice.startswith("a") else "b")
    out_dir.mkdir(parents=True, exist_ok=True)
    done = 0
    for word, phonemes in words:
        target = out_dir / f"{word}.wav"
        if target.exists():
            continue
        # 给了音素就用音素，避免同形异读被模型猜错
        text = f"[{word}](/{phonemes}/)" if phonemes else word
        chunks = [audio for _, _, audio in pipeline(text, voice=voice, speed=1.0)]
        if not chunks:
            print(f"合成失败（无输出）: {word}", file=sys.stderr)
            continue
        import numpy as np

        sf.write(target, np.concatenate(chunks), 24000)
        done += 1
        if done % 200 == 0:
            print(f"{done} synthesized", file=sys.stderr)
    print(f"done: {done} clips in {out_dir}", file=sys.stderr)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("word_list", nargs="?", help="`词[\\t音素]` 一行一个")
    parser.add_argument("--out-dir", help="wav 输出目录")
    parser.add_argument("--voice", default="af_heart", help="Kokoro 发音人，af_* 美音 / bf_* 英音")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--missing", help="算差集模式：词表文件")
    parser.add_argument("--have", help="算差集模式：已有的 manifest.tsv")
    parser.add_argument("-o", "--output", help="算差集模式：输出文件")
    args = parser.parse_args()

    if args.missing:
        if not (args.have and args.output):
            parser.error("--missing 要配 --have 与 -o")
        write_missing(Path(args.missing), Path(args.have), Path(args.output))
        return

    if not (args.word_list and args.out_dir):
        parser.error("要给词表与 --out-dir")

    words: list[tuple[str, str | None]] = []
    with open(args.word_list, encoding="utf-8") as src:
        for line in src:
            line = line.rstrip("\n")
            if not line.strip() or line.startswith("#"):
                continue
            fields = line.split("\t")
            word = fields[0].strip().lower()
            phonemes = fields[1].strip() if len(fields) > 1 and fields[1].strip() else None
            if word:
                words.append((word, phonemes))
    if args.limit:
        words = words[: args.limit]
    print(f"{len(words)} words to synthesize with {args.voice}", file=sys.stderr)
    synthesize(words, Path(args.out_dir), args.voice)


if __name__ == "__main__":
    main()
