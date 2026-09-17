# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx>=0.27"]
# ///
"""从 Wikimedia Commons 抓英语单词的真人发音，输出 `词\\t文件名\\t许可证\\t作者\\t文件页URL` 与音频文件。

发音文件在 Commons 上的命名有惯例：`En-us-<词>.ogg`（英语维基词典社区录的）、`LL-Q1860 (eng)-<录音人>-<词>.wav`
（Lingua Libre 批量录的）。这里按 Commons 的搜索接口按词查这两类，取第一个命中，顺带把 extmetadata 里的
许可证与作者记下来——CC-BY-SA 要求署名，署名清单靠这张表生成。

用法：
    uv run tools/corpus/pronunciation_fetch.py words.txt --out-dir data/generated/audio/human
其中 words.txt 一行一个词（可用 `assets/levels/levels-en.tsv` 的第一列，或 english.tsv 的第一列）。

只抓 CC0 / CC-BY / CC-BY-SA / public domain 的文件，其它许可跳过并记进 skipped.tsv。
"""

import argparse
import csv
import re
import sys
import time
from pathlib import Path
from urllib.parse import urlparse

import httpx

API = "https://commons.wikimedia.org/w/api.php"

# 允许随包分发的许可证（GPL 项目可以带，只要署名）
ALLOWED = re.compile(
    r"^(cc0|cc[- ]by([- ]sa)?([- ][0-9.]+)?|public domain|pd|cc[- ]pd)", re.IGNORECASE
)

# 优先级：美音维基词典录音 → 英音 → Lingua Libre 英语
PATTERNS = [
    "En-us-{word}.ogg",
    "En-us-{word}.wav",
    "En-uk-{word}.ogg",
    "En-gb-{word}.ogg",
]

USER_AGENT = "qingjian-pronunciation-fetch/0.1 (https://github.com/qingjian-team/qingjian)"

# 遇 429 / 5xx 重试几次，每次等得更久。Wikimedia 对不带 token 的请求限速很紧，
# 首次跑 1000 词时 sleep=0.05 被成批挡下（只拉到 155 个），不重试会把“被限速”错当“没录音”。
RETRIES = 4
RETRY_BASE_SLEEP = 2.0


def candidate_titles(word: str) -> list[str]:
    """按惯例拼出可能的文件名，再加一条 Lingua Libre 的模糊搜索。"""
    return [f"File:{p.format(word=word)}" for p in PATTERNS]


def get(client: httpx.Client, url: str, **kwargs) -> httpx.Response:
    """带退避重试的 GET：429 与 5xx 等一会儿再试，其余错误直接抛。"""
    last: httpx.HTTPError | None = None
    for attempt in range(RETRIES):
        response = client.get(url, **kwargs)
        if response.status_code == 429 or response.status_code >= 500:
            # Retry-After 有就听它的，没有就指数退避
            wait = response.headers.get("Retry-After")
            delay = float(wait) if wait and wait.isdigit() else RETRY_BASE_SLEEP * (2**attempt)
            time.sleep(delay)
            last = httpx.HTTPStatusError(
                f"{response.status_code} after {attempt + 1} tries",
                request=response.request,
                response=response,
            )
            continue
        response.raise_for_status()
        return response
    raise last if last else httpx.HTTPError("retries exhausted")


def image_info(client: httpx.Client, titles: list[str]) -> dict:
    """批量查文件信息（一次最多 50 个标题）。"""
    response = get(
        client,
        API,
        params={
            "action": "query",
            "format": "json",
            "titles": "|".join(titles),
            "prop": "imageinfo",
            "iiprop": "url|extmetadata|mime|size",
        },
        timeout=30,
    )
    return response.json().get("query", {}).get("pages", {})


def search_lingua_libre(client: httpx.Client, word: str) -> str | None:
    """Lingua Libre 的文件名带录音人，只能搜。"""
    response = get(
        client,
        API,
        params={
            "action": "query",
            "format": "json",
            "list": "search",
            "srsearch": f'intitle:"LL-Q1860 (eng)" intitle:"{word}" filetype:audio',
            "srnamespace": "6",
            "srlimit": "5",
        },
        timeout=30,
    )
    hits = response.json().get("query", {}).get("search", [])
    suffix = f"-{word}.wav".lower()
    for hit in hits:
        if hit["title"].lower().endswith(suffix):
            return hit["title"]
    return None


def pick(pages: dict, titles: list[str]) -> tuple[str, dict] | None:
    """按 `titles` 给的顺序挑第一个真实存在、许可证允许的文件。

    不能直接遍历 `pages`：接口返回的顺序是它自己的（pageid 序），照那个顺序取会把英音排到美音前面。
    """
    by_title = {
        page["title"]: page["imageinfo"][0]
        for page in pages.values()
        if "imageinfo" in page and page.get("missing") is None
    }
    for title in titles:
        info = by_title.get(title)
        if info is None:
            continue
        license_short = (
            info.get("extmetadata", {}).get("LicenseShortName", {}).get("value", "")
        )
        if ALLOWED.match(license_short.strip()):
            return title, info
    return None


def strip_html(text: str) -> str:
    """去标签并压成一行：作者字段常带 <a> 和换行，换行会把 TSV 撑成两行。"""
    return " ".join(re.sub(r"<[^>]+>", " ", text).split())


def extension_of(url: str) -> str:
    """从 URL 取扩展名：query 里有 utm_* 参数，直接 `Path(url).suffix` 会把它们当扩展名。"""
    suffix = Path(urlparse(url).path).suffix.lower()
    return suffix if suffix in {".ogg", ".wav", ".mp3", ".flac", ".oga"} else ".ogg"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("word_list", help="一行一个词，或 TSV 取第一列")
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--limit", type=int, default=0, help="只抓前 N 个词（试跑用）")
    parser.add_argument("--sleep", type=float, default=0.4, help="每个词之间等多久，别把 API 打爆")
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    (out_dir / "clips").mkdir(parents=True, exist_ok=True)

    # 断点续跑：上一轮已经拉到的不再问（被限速时要重跑好几次，每次都从头下载太浪费）
    manifest_path = out_dir / "manifest.tsv"
    existing: list[str] = []
    done: set[str] = set()
    if manifest_path.exists():
        for index, line in enumerate(manifest_path.read_text(encoding="utf-8").splitlines()):
            # 表头只按「第一行且正好是表头」认：word 本身也是一个词，按前缀跳会把它的录音丢掉
            if not line.strip() or (index == 0 and line.startswith("word\tfile\t")):
                continue
            word = line.split("\t")[0]
            # 音频文件还在才算数，否则重新拉
            if (out_dir / "clips" / line.split("\t")[1]).exists():
                existing.append(line)
                done.add(word)

    words: list[str] = []
    with open(args.word_list, encoding="utf-8") as src:
        for line in src:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            word = line.split("\t")[0].strip()
            # 只要单个英文词，词组和带撇号的先跳过（文件名规则不一致）
            if word and word.isascii() and word.isalpha():
                words.append(word.lower())
    words = sorted(set(words))
    if args.limit:
        words = words[: args.limit]
    pending = [w for w in words if w not in done]
    print(
        f"{len(words)} words, {len(done)} already fetched, {len(pending)} to try",
        file=sys.stderr,
    )

    manifest = open(manifest_path, "w", encoding="utf-8", newline="")
    skipped = open(out_dir / "skipped.tsv", "w", encoding="utf-8", newline="")
    writer = csv.writer(manifest, delimiter="\t", lineterminator="\n")
    skip_writer = csv.writer(skipped, delimiter="\t", lineterminator="\n")
    writer.writerow(["word", "file", "license", "author", "page"])
    skip_writer.writerow(["word", "reason"])
    for line in existing:
        manifest.write(line + "\n")

    found = len(existing)
    with httpx.Client(headers={"User-Agent": USER_AGENT}, follow_redirects=True) as client:
        for position, word in enumerate(pending, 1):
            try:
                titles = candidate_titles(word)
                chosen = pick(image_info(client, titles), titles)
                if chosen is None:
                    title = search_lingua_libre(client, word)
                    if title:
                        chosen = pick(image_info(client, [title]), [title])
                if chosen is None:
                    skip_writer.writerow([word, "no free recording found"])
                    continue
                title, info = chosen
                target = out_dir / "clips" / f"{word}{extension_of(info['url'])}"
                if not target.exists():
                    audio = get(client, info["url"], timeout=60)
                    target.write_bytes(audio.content)
                meta = info.get("extmetadata", {})
                writer.writerow(
                    [
                        word,
                        target.name,
                        strip_html(meta.get("LicenseShortName", {}).get("value", "")),
                        strip_html(meta.get("Artist", {}).get("value", "")),
                        info.get("descriptionurl", ""),
                    ]
                )
                found += 1
            except httpx.HTTPError as error:
                skip_writer.writerow([word, f"http error: {error}"])
            if position % 100 == 0:
                manifest.flush()
                skipped.flush()
                print(f"{position}/{len(pending)} tried, {found} found", file=sys.stderr)
            time.sleep(args.sleep)

    manifest.close()
    skipped.close()
    print(f"done: {found}/{len(words)} recordings in {out_dir}", file=sys.stderr)


if __name__ == "__main__":
    main()
