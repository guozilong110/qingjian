//! 发音下载：后台线程去 Wikimedia Commons 抓，主线程轮询结果。
//!
//! 与云联想同一个形状（线程 + mpsc + 主线程 poll）：网络请求绝不在按键回调里做。
//! 一次只处理一个词——用户翻候选很快，攒一堆请求没意义，新的把旧的挤掉即可。

use std::sync::mpsc::{Receiver, Sender, channel};

use qingjian_audio::{Fetcher, FoundAudio};

/// 下载结果：词 + 抓到的东西（`None` 表示上游确实没有）。
pub type Fetched = (String, Option<FoundAudio>);

/// 发音下载器：一个常驻线程。
pub struct Downloader {
    /// 请求出口；线程挂了之后发送会失败，那就不再下载。
    requests: Sender<String>,

    /// 结果入口。
    results: Receiver<Fetched>,
}

impl Downloader {
    /// 起下载线程。`Fetcher` 建不起来（TLS 初始化失败这类）时返回 `None`，那就只用本地已有的。
    pub fn start() -> Option<Self> {
        let fetcher = match Fetcher::new() {
            Ok(fetcher) => fetcher,
            Err(error) => {
                tracing::warn!(%error, "发音下载器建不起来，只用本地缓存");
                return None;
            }
        };
        let (requests, inbox) = channel::<String>();
        let (outbox, results) = channel::<Fetched>();
        let spawned = std::thread::Builder::new()
            .name("qingjian-audio-fetch".to_owned())
            .spawn(move || run(fetcher, inbox, outbox));
        match spawned {
            Ok(_) => Some(Self { requests, results }),
            Err(error) => {
                tracing::warn!(%error, "起不了发音下载线程，只用本地缓存");
                None
            }
        }
    }

    /// 请求下载一个词。线程没了就静默忽略。
    pub fn request(&self, word: &str) {
        let _ = self.requests.send(word.to_owned());
    }

    /// 取一条已完成的结果；没有（或线程已退出）返回 `None`。
    pub fn poll(&self) -> Option<Fetched> {
        self.results.try_recv().ok()
    }
}

/// 下载线程主体：收词、抓、回结果，直到请求端关闭。
fn run(fetcher: Fetcher, inbox: Receiver<String>, outbox: Sender<Fetched>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(%error, "发音下载线程建不了运行时");
            return;
        }
    };
    while let Ok(word) = inbox.recv() {
        // 攒着的请求只做最后一个：用户翻候选比网络快，中间那些已经没人要了
        let word = std::iter::from_fn(|| inbox.try_recv().ok())
            .last()
            .unwrap_or(word);
        let started = std::time::Instant::now();
        let outcome = runtime.block_on(fetcher.fetch(&word));
        let found = match outcome {
            Ok(found) => {
                tracing::debug!(
                    word = %word,
                    hit = found.is_some(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "发音下载完成"
                );
                found
            }
            Err(error) => {
                // 网络不通、超时：不当成「上游没有」，下次还可以再试
                tracing::debug!(word = %word, %error, "发音下载失败");
                continue;
            }
        };
        if outbox.send((word, found)).is_err() {
            return;
        }
    }
}
