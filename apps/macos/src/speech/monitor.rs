use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSObject, NSObjectProtocol, NSTimer};

/// 高亮停住多久才发音。比整句重排的防抖（80 ms）长得多：翻候选是连着按方向键的，
/// 一格响一声会把人吵跑；这个值让「翻过去」不响、「停下来看」才响。
const DEBOUNCE: f64 = 0.35;

/// 等下载结果的轮询间隔。抓一个词实测 1–2.7 秒（查接口 + 下文件），所以不用查太密。
const POLL_INTERVAL: f64 = 0.15;

/// 最长等多久就不再轮询（网络卡住时兜底）。
const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// 发音的两个定时器：停顿防抖（到点才读 / 才去下载）与下载结果轮询。
pub struct SpeakMonitor {
    /// 防抖定时器（一次性）；没在等为 `None`。
    debounce: Option<Retained<NSTimer>>,

    /// 下载结果轮询定时器；没在等为 `None`。
    poll: Option<Retained<NSTimer>>,

    /// 本轮开始等下载的时间。
    since: Option<std::time::Instant>,

    mtm: MainThreadMarker,
}

impl SpeakMonitor {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            debounce: None,
            poll: None,
            since: None,
            mtm,
        }
    }

    /// 高亮动了（或新一轮候选）：重新计时。
    pub fn schedule(&mut self) {
        self.cancel_debounce();
        let target = SpeakTicker::new(self.mtm);
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                DEBOUNCE,
                &target,
                sel!(fire:),
                None,
                false,
            )
        };
        self.debounce = Some(timer);
    }

    /// 下载请求已发出：开始轮询结果。
    pub fn start_polling(&mut self) {
        self.since = Some(std::time::Instant::now());
        if self.poll.is_some() {
            return;
        }
        let target = SpeakTicker::new(self.mtm);
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                POLL_INTERVAL,
                &target,
                sel!(poll:),
                None,
                true,
            )
        };
        self.poll = Some(timer);
    }

    /// 停轮询（结果到了、或等太久了）。
    pub fn stop_polling(&mut self) {
        if let Some(timer) = self.poll.take() {
            timer.invalidate();
        }
        self.since = None;
    }

    /// 等太久了。
    pub fn expired(&self) -> bool {
        self.since.is_some_and(|since| since.elapsed() > MAX_WAIT)
    }

    /// 取消等待中的发音（不动轮询：已经发出去的下载还是要收，存进缓存不浪费）。
    pub fn cancel_debounce(&mut self) {
        if let Some(timer) = self.debounce.take() {
            timer.invalidate();
        }
    }

    /// 全停。
    pub fn stop(&mut self) {
        self.cancel_debounce();
        self.stop_polling();
    }
}

define_class!(
    // SAFETY: NSObject 没有子类化要求；没有实现 Drop。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct SpeakTicker;

    impl SpeakTicker {
        #[unsafe(method(fire:))]
        fn fire(&self, _timer: Option<&AnyObject>) {
            crate::host::with(|h| h.speak_highlighted());
        }

        #[unsafe(method(poll:))]
        fn poll(&self, _timer: Option<&AnyObject>) {
            crate::host::with(|h| h.poll_pronunciation());
        }
    }

    unsafe impl NSObjectProtocol for SpeakTicker {}
);

impl SpeakTicker {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}
