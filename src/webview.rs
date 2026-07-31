use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use anyhow::{anyhow, Context, Result};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{
    NSArray, NSError, NSHTTPCookie, NSPoint, NSRect, NSSize, NSString, NSURLRequest, NSURL,
};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKWebView, WKWebViewConfiguration,
    WKWebsiteDataStore,
};
use tokio::sync::oneshot;
use tokio::time::sleep;
const WINDOW_WIDTH: f64 = 1280.0;
const WINDOW_HEIGHT: f64 = 1024.0;
const HIDDEN_ORIGIN: (f64, f64) = (-20000.0, -20000.0);
const VISIBLE_ORIGIN: (f64, f64) = (80.0, 80.0);
const VISIBLE_ENV_VAR: &str = "DRAWBRIDGE_WEBVIEW_VISIBLE";
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(10);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
struct Instance {
    window: Retained<NSWindow>,
    webview: Retained<WKWebView>,
}
thread_local! {
    static INSTANCES: RefCell<HashMap<u64, Instance>> = RefCell::new(HashMap::new());
}
struct Slot<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);
impl<T> Slot<T> {
    fn new(sender: oneshot::Sender<T>) -> Self {
        Self(Arc::new(Mutex::new(Some(sender))))
    }
    fn handle(&self) -> Self {
        Self(self.0.clone())
    }
    fn fill(&self, value: T) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(sender) = guard.take() {
                let _ = sender.send(value);
            }
        }
    }
}
async fn on_main<T, F>(work: F) -> Result<T>
where
    F: FnOnce(MainThreadMarker) -> T + Send + 'static,
    T: Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
        let _ = sender.send(work(mtm));
    });
    tokio::time::timeout(MAIN_THREAD_TIMEOUT, receiver)
        .await
        .map_err(|_| {
            anyhow!(
                "the main thread did not respond within {}s",
                MAIN_THREAD_TIMEOUT.as_secs()
            )
        })?
        .map_err(|_| anyhow!("The main thread dropped a webview request"))
}
fn with_instance<T>(id: u64, work: impl FnOnce(&Instance) -> T) -> Result<T> {
    INSTANCES.with(|instances| {
        let instances = instances.borrow();
        let instance = instances
            .get(&id)
            .ok_or_else(|| anyhow!("The webview was already closed"))?;
        Ok(work(instance))
    })
}
const RULE_LIST_IDENTIFIER: &str = "drawbridge-block-media";
fn blocking_rules_json(extensions: &[&str]) -> String {
    let alternation = extensions.join("|");
    format!(
        r#"[{{"trigger":{{"url-filter":"\\.({alternation})$"}},"action":{{"type":"block"}}}}]"#
    )
}
fn wrap_expression(expr: &str) -> String {
    format!(
        "JSON.stringify((function () {{ const __r = ({expr}); return __r === undefined ? null : __r; }})())"
    )
}
pub struct WebView {
    id: u64,
}
impl WebView {
    pub async fn launch() -> Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let visible = std::env::var_os(VISIBLE_ENV_VAR).is_some();
        if let Err(e) = Self::launch_on_main(id, visible).await {
            DispatchQueue::main().exec_async(move || {
                let instance = INSTANCES.with(|instances| instances.borrow_mut().remove(&id));
                if let Some(instance) = instance {
                    instance.window.close();
                }
            });
            return Err(e);
        }
        Ok(Self { id })
    }
    async fn launch_on_main(id: u64, visible: bool) -> Result<()> {
        on_main(move |mtm| unsafe {
            let configuration = WKWebViewConfiguration::new(mtm);
            configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let content = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            );
            let webview =
                WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), content, &configuration);
            let (x, y) = if visible { VISIBLE_ORIGIN } else { HIDDEN_ORIGIN };
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(x, y), NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window.setContentView(Some(&webview));
            window.orderFrontRegardless();
            INSTANCES.with(|instances| {
                instances.borrow_mut().insert(id, Instance { window, webview });
            });
        })
        .await
    }
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let id = self.id;
        let url = url.to_string();
        on_main(move |_| {
            let ns_url = NSURL::URLWithString(&NSString::from_str(&url))
                .ok_or_else(|| anyhow!("Not a valid URL: {url}"))?;
            with_instance(id, |instance| unsafe {
                instance
                    .webview
                    .loadRequest(&NSURLRequest::requestWithURL(&ns_url));
            })
        })
        .await?
    }
    pub async fn block_url_extensions(&self, extensions: &[&str]) -> Result<()> {
        let id = self.id;
        let rules = blocking_rules_json(extensions);
        let (sender, receiver) = oneshot::channel::<Result<(), String>>();
        let slot = Slot::new(sender);
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
            let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else {
                slot.fill(Err("No content rule list store is available".to_string()));
                return;
            };
            let block_slot = slot.handle();
            let block = RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
                if !error.is_null() {
                    let message = unsafe { (*error).localizedDescription() }.to_string();
                    block_slot.fill(Err(message));
                    return;
                }
                if list.is_null() {
                    block_slot.fill(Err("Compiled an empty rule list".to_string()));
                    return;
                }
                let added = with_instance(id, |instance| unsafe {
                    instance
                        .webview
                        .configuration()
                        .userContentController()
                        .addContentRuleList(&*list);
                });
                match added {
                    Ok(()) => block_slot.fill(Ok(())),
                    Err(_) => block_slot.fill(Err("The webview was already closed".to_string())),
                }
            });
            unsafe {
                store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                    Some(&NSString::from_str(RULE_LIST_IDENTIFIER)),
                    Some(&NSString::from_str(&rules)),
                    Some(&block),
                );
            }
        });
        tokio::time::timeout(MAIN_THREAD_TIMEOUT, receiver)
            .await
            .map_err(|_| {
                anyhow!(
                    "the main thread did not respond within {}s",
                    MAIN_THREAD_TIMEOUT.as_secs()
                )
            })?
            .map_err(|_| anyhow!("The main thread dropped a rule list request"))?
            .map_err(|message| anyhow!("{message}"))
    }
    pub async fn wait_until_loaded(&self, timeout: Duration) -> Result<()> {
        sleep(Duration::from_millis(300)).await;
        let deadline = Instant::now() + timeout;
        loop {
            let id = self.id;
            let loading =
                on_main(move |_| with_instance(id, |i| unsafe { i.webview.isLoading() })).await??;
            if !loading {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Timed out waiting for the page to finish loading"));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
    pub async fn eval(&self, expr: &str) -> Result<serde_json::Value> {
        let id = self.id;
        let script = wrap_expression(expr);
        let (sender, receiver) = oneshot::channel::<Result<String, String>>();
        let slot = Slot::new(sender);
        DispatchQueue::main().exec_async(move || {
            let block_slot = slot.handle();
            let block = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
                if !error.is_null() {
                    let message = unsafe { (*error).localizedDescription() }.to_string();
                    block_slot.fill(Err(message));
                    return;
                }
                if result.is_null() {
                    block_slot.fill(Ok("null".to_string()));
                    return;
                }
                let object = unsafe { &*result };
                match object.downcast_ref::<NSString>() {
                    Some(string) => block_slot.fill(Ok(string.to_string())),
                    None => block_slot.fill(Err(
                        "eval returned a non-string result".to_string(),
                    )),
                }
            });
            let dispatched = with_instance(id, |instance| unsafe {
                instance.webview.evaluateJavaScript_completionHandler(
                    &NSString::from_str(&script),
                    Some(&block),
                );
            });
            if dispatched.is_err() {
                slot.fill(Err("The webview was already closed".to_string()));
            }
        });
        let raw = tokio::time::timeout(MAIN_THREAD_TIMEOUT, receiver)
            .await
            .map_err(|_| {
                anyhow!(
                    "the main thread did not respond within {}s",
                    MAIN_THREAD_TIMEOUT.as_secs()
                )
            })?
            .map_err(|_| anyhow!("The main thread dropped an eval request"))?
            .map_err(|message| anyhow!("JavaScript evaluation failed: {message}"))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("Could not parse the eval result as JSON: {raw}"))
    }
    pub async fn cookie(&self, name: &str) -> Result<Option<String>> {
        let id = self.id;
        let wanted = name.to_string();
        let (sender, receiver) = oneshot::channel::<Result<Option<String>, String>>();
        let slot = Slot::new(sender);
        DispatchQueue::main().exec_async(move || {
            let block_slot = slot.handle();
            let block = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
                let cookies = unsafe { cookies.as_ref() };
                let mut found = None;
                for cookie in cookies {
                    if cookie.name().to_string() == wanted {
                        found = Some(cookie.value().to_string());
                        break;
                    }
                }
                block_slot.fill(Ok(found));
            });
            let dispatched = with_instance(id, |instance| unsafe {
                instance
                    .webview
                    .configuration()
                    .websiteDataStore()
                    .httpCookieStore()
                    .getAllCookies(&block);
            });
            if dispatched.is_err() {
                slot.fill(Err("The webview was already closed".to_string()));
            }
        });
        tokio::time::timeout(MAIN_THREAD_TIMEOUT, receiver)
            .await
            .map_err(|_| {
                anyhow!(
                    "the main thread did not respond within {}s",
                    MAIN_THREAD_TIMEOUT.as_secs()
                )
            })?
            .map_err(|_| anyhow!("The main thread dropped a cookie request"))?
            .map_err(|message| anyhow!("{message}"))
    }
    pub async fn url(&self) -> Option<String> {
        let id = self.id;
        on_main(move |_| {
            with_instance(id, |instance| unsafe {
                instance
                    .webview
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .map(|s| s.to_string())
            })
            .ok()
            .flatten()
        })
        .await
        .ok()
        .flatten()
    }
    pub async fn title(&self) -> Option<String> {
        let id = self.id;
        on_main(move |_| {
            with_instance(id, |instance| unsafe {
                instance.webview.title().map(|s| s.to_string())
            })
            .ok()
            .flatten()
        })
        .await
        .ok()
        .flatten()
    }
}
impl Drop for WebView {
    fn drop(&mut self) {
        let id = self.id;
        DispatchQueue::main().exec_async(move || {
            let instance = INSTANCES.with(|instances| instances.borrow_mut().remove(&id));
            if let Some(instance) = instance {
                instance.window.close();
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrap_expression_stringifies_the_result() {
        let wrapped = wrap_expression("1 + 1");
        assert!(wrapped.starts_with("JSON.stringify("));
        assert!(wrapped.contains("1 + 1"));
    }
    #[test]
    fn wrap_expression_normalises_undefined_to_null() {
        assert!(wrap_expression("void 0").contains("undefined ? null"));
    }
    #[test]
    fn blocking_rules_are_valid_json_with_a_block_action() {
        let rules = blocking_rules_json(&["png", "woff2"]);
        let parsed: serde_json::Value =
            serde_json::from_str(&rules).expect("WebKit rejects the whole list if it is not JSON");
        assert_eq!(parsed[0]["action"]["type"], "block");
    }
    #[test]
    fn blocking_rules_escape_the_dot_and_alternate_the_extensions() {
        let rules = blocking_rules_json(&["png", "woff2"]);
        let parsed: serde_json::Value = serde_json::from_str(&rules).expect("valid json");
        assert_eq!(parsed[0]["trigger"]["url-filter"], "\\.(png|woff2)$");
    }
}
