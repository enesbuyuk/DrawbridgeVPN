use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;
use drawbridge_vpn::webview::WebView;
fn main() -> eframe::Result<()> {
    let (tx, rx) = channel::<String>();
    eframe::run_native(
        "webview smoke",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| Ok(Box::new(Smoke::new(tx, rx)))),
    )
}
struct Smoke {
    lines: Vec<String>,
    rx: Receiver<String>,
    started: bool,
    tx: Option<Sender<String>>,
}
impl Smoke {
    fn new(tx: Sender<String>, rx: Receiver<String>) -> Self {
        Self { lines: Vec::new(), rx, started: false, tx: Some(tx) }
    }
}
impl eframe::App for Smoke {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            let tx = self.tx.take().expect("started once");
            thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().expect("runtime");
                runtime.block_on(async move {
                    let report = |tx: &Sender<String>, line: String| {
                        println!("{line}");
                        let _ = tx.send(line);
                    };
                    let webview = match WebView::launch().await {
                        Ok(webview) => webview,
                        Err(e) => return report(&tx, format!("FAIL launch: {e:#}")),
                    };
                    report(&tx, "OK launch".to_string());
                    match webview.eval("1 + 1").await {
                        Ok(v) if v == serde_json::json!(2) => report(&tx, "OK eval number".to_string()),
                        Ok(v) => return report(&tx, format!("FAIL eval number: got {v}")),
                        Err(e) => return report(&tx, format!("FAIL eval number: {e:#}")),
                    }
                    match webview.eval("void 0").await {
                        Ok(serde_json::Value::Null) => report(&tx, "OK eval undefined".to_string()),
                        Ok(v) => return report(&tx, format!("FAIL eval undefined: got {v}")),
                        Err(e) => return report(&tx, format!("FAIL eval undefined: {e:#}")),
                    }
                    if let Err(e) = webview.navigate("https://example.com/").await {
                        return report(&tx, format!("FAIL navigate: {e:#}"));
                    }
                    if let Err(e) = webview.wait_until_loaded(Duration::from_secs(20)).await {
                        return report(&tx, format!("FAIL load: {e:#}"));
                    }
                    report(&tx, "OK navigate".to_string());
                    if let Err(e) = webview
                        .eval("document.cookie = 'drawbridge-probe=itworks'")
                        .await
                    {
                        report(&tx, format!("FAIL cookie hit: could not set probe cookie: {e:#}"));
                    } else {
                        match webview.cookie("drawbridge-probe").await {
                            Ok(Some(v)) if v == "itworks" => {
                                report(&tx, "OK cookie hit".to_string())
                            }
                            Ok(Some(v)) => report(&tx, format!("FAIL cookie hit: got {v}")),
                            Ok(None) => report(&tx, "FAIL cookie hit: got None".to_string()),
                            Err(e) => report(&tx, format!("FAIL cookie hit: {e:#}")),
                        }
                    }
                    match webview.eval("document.title").await {
                        Ok(serde_json::Value::String(title)) if !title.is_empty() => {
                            report(&tx, format!("OK title: {title}"))
                        }
                        Ok(v) => report(&tx, format!("FAIL title: got {v}")),
                        Err(e) => report(&tx, format!("FAIL title: {e:#}")),
                    }
                    match webview.url().await {
                        Some(url) => report(&tx, format!("OK url: {url}")),
                        None => report(&tx, "FAIL url: none".to_string()),
                    }
                    match webview.cookie("definitely-not-set").await {
                        Ok(None) => report(&tx, "OK cookie miss".to_string()),
                        Ok(Some(v)) => report(&tx, format!("FAIL cookie miss: got {v}")),
                        Err(e) => report(&tx, format!("FAIL cookie miss: {e:#}")),
                    }
                });
            });
        }
        while let Ok(line) = self.rx.try_recv() {
            self.lines.push(line);
        }
        eframe::egui::CentralPanel::default().show(ui, |ui| {
            for line in &self.lines {
                ui.label(line);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
