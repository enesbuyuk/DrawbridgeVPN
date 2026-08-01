use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use anyhow::{anyhow, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::element::Element;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::Page;
use futures::StreamExt;
use tokio::time::{sleep, timeout as with_deadline};
use totp_rs::TOTP;
use uuid::Uuid;
use crate::events::{ConnectionStatus, LogEvent};
use crate::profile::Profile;
use super::script::{
    selector_present_script, text_center_lookup_script, text_lookup_script, visible_text_script,
    ANOTHER_WAY_TEXT, BLOCKED_EXTENSIONS, INTERACTIVE_ELEMENTS_SCRIPT, OTP_INPUT_SELECTOR,
    SUBMIT_SELECTOR, SUBMIT_TEXT_CANDIDATES, USE_CODE_TEXT,
};
static ACTIVE_CHROME_DIRS: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn register_chrome_dir(dir: &Path) {
    if let Ok(mut dirs) = ACTIVE_CHROME_DIRS.lock() {
        dirs.push(dir.to_string_lossy().into_owned());
    }
}
fn unregister_chrome_dir(dir: &Path) {
    let key = dir.to_string_lossy().into_owned();
    if let Ok(mut dirs) = ACTIVE_CHROME_DIRS.lock() {
        dirs.retain(|d| *d != key);
    }
}
pub fn cleanup_browsers() {
    let dirs = match ACTIVE_CHROME_DIRS.lock() {
        Ok(mut dirs) => std::mem::take(&mut *dirs),
        Err(e) => std::mem::take(&mut *e.into_inner()),
    };
    for dir in dirs {
        let _ = Command::new("pkill").arg("-f").arg(&dir).status();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
fn log(tx: &Sender<LogEvent>, message: impl Into<String>) {
    let _ = tx.send(LogEvent::Log(message.into()));
}
fn set_status(tx: &Sender<LogEvent>, new_status: ConnectionStatus) {
    let _ = tx.send(LogEvent::Status(new_status));
}
const CHROME_CANDIDATE_PATHS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];
fn find_chrome_executable() -> Result<PathBuf> {
    CHROME_CANDIDATE_PATHS
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow!(
                "Could not find Google Chrome. Install it from https://www.google.com/chrome/ and try again"
            )
        })
}
async fn dump_page_html(page: &Page) -> String {
    let script = visible_text_script();
    match with_deadline(CDP_STEP_TIMEOUT, page.evaluate_expression(script)).await {
        Ok(Ok(result)) => result
            .into_value::<String>()
            .unwrap_or_else(|e| format!("<failed to read visible text: {e}>")),
        Ok(Err(e)) => format!("<failed to evaluate visible text dump: {e}>"),
        Err(_) => "<timed out reading visible text>".to_string(),
    }
}
async fn find_and_click(
    page: &Page,
    selector: &str,
    timeout: Duration,
    log_tx: &Sender<LogEvent>,
) -> Result<Element> {
    let deadline = Instant::now() + timeout;
    #[allow(unused_assignments)]
    let mut last_error = "selector not found".to_string();
    let mut last_report = Instant::now();
    loop {
        if let Ok(Ok(element)) = with_deadline(CDP_STEP_TIMEOUT, page.find_element(selector)).await
        {
            match with_deadline(CDP_STEP_TIMEOUT, element.click()).await {
                Ok(Ok(_)) => return Ok(element),
                Ok(Err(e)) => last_error = e.to_string(),
                Err(_) => last_error = "click timed out".to_string(),
            }
            match with_deadline(CDP_STEP_TIMEOUT, element.focus()).await {
                Ok(Ok(_)) => return Ok(element),
                Ok(Err(e)) => last_error = format!("{last_error}; focus failed: {e}"),
                Err(_) => last_error = format!("{last_error}; focus timed out"),
            }
        } else {
            last_error = "selector not found".to_string();
        }
        if Instant::now() >= deadline {
            log(
                log_tx,
                format!(
                    "Giving up on selector '{selector}', current page: {}\nHTML: {}",
                    describe_page(page).await,
                    dump_page_html(page).await
                ),
            );
            return Err(anyhow!(
                "Timed out waiting to click selector {selector}: {last_error}"
            ));
        }
        if last_report.elapsed() >= Duration::from_secs(3) {
            last_report = Instant::now();
            log(
                log_tx,
                format!(
                    "Still trying to click selector '{selector}' ({last_error}), current page: {}\nHTML: {}",
                    describe_page(page).await,
                    dump_page_html(page).await
                ),
            );
        }
        sleep(Duration::from_millis(150)).await;
    }
}
const CDP_STEP_TIMEOUT: Duration = Duration::from_secs(5);
async fn is_text_visible(page: &Page, text: &str) -> Result<bool> {
    let script = text_lookup_script(text, false);
    match with_deadline(CDP_STEP_TIMEOUT, page.evaluate_expression(script)).await {
        Err(_) => Ok(false),
        Ok(Err(e)) => Err(e).context("Failed to evaluate a visibility check"),
        Ok(Ok(result)) => result
            .into_value::<bool>()
            .context("Failed to read the visibility check result"),
    }
}
async fn click_text(page: &Page, text: &str) -> Result<bool> {
    let script = text_lookup_script(text, true);
    match with_deadline(CDP_STEP_TIMEOUT, page.evaluate_expression(script)).await {
        Err(_) => Ok(false),
        Ok(Err(e)) => Err(e).context("Failed to evaluate a click action"),
        Ok(Ok(result)) => result
            .into_value::<bool>()
            .context("Failed to read the click action result"),
    }
}
async fn wait_and_click_text(page: &Page, text: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if click_text(page, text).await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("Timed out waiting to click text: {text}"));
        }
        sleep(Duration::from_millis(200)).await;
    }
}
async fn describe_page(page: &Page) -> String {
    let url = with_deadline(CDP_STEP_TIMEOUT, page.url())
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .unwrap_or_else(|| "<unknown url>".to_string());
    let title = with_deadline(CDP_STEP_TIMEOUT, page.get_title())
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .unwrap_or_else(|| "<unknown title>".to_string());
    format!("url={url} title={title}")
}
async fn resolve_mfa_method(page: &Page, log_tx: &Sender<LogEvent>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    log(
        log_tx,
        format!(
            "MFA screen state: {}\nHTML: {}",
            describe_page(page).await,
            dump_page_html(page).await
        ),
    );
    let mut last_report = Instant::now();
    loop {
        if is_text_visible(page, USE_CODE_TEXT).await? {
            log(log_tx, "MFA option 'Use a verification code' is visible, selecting it");
            click_text(page, USE_CODE_TEXT).await?;
            return Ok(());
        }
        if is_text_visible(page, ANOTHER_WAY_TEXT).await? {
            log(
                log_tx,
                "Selecting 'I can't use my Microsoft Authenticator app right now'",
            );
            click_text(page, ANOTHER_WAY_TEXT).await?;
            wait_and_click_text(page, USE_CODE_TEXT, Duration::from_secs(10)).await?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Timed out waiting for an MFA method selection option ({})\nHTML: {}",
                describe_page(page).await,
                dump_page_html(page).await
            ));
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            last_report = Instant::now();
            log(
                log_tx,
                format!(
                    "Still waiting for MFA options: {}\nHTML: {}",
                    describe_page(page).await,
                    dump_page_html(page).await
                ),
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
}
const SUBMIT_CLICK_TIMEOUT: Duration = Duration::from_secs(3);
const SUBMIT_LOOKUP_TIMEOUT: Duration = Duration::from_millis(800);
async fn try_click_selector_trusted(page: &Page, selector: &str) -> bool {
    let Ok(Ok(element)) = with_deadline(SUBMIT_CLICK_TIMEOUT, page.find_element(selector)).await
    else {
        return false;
    };
    with_deadline(SUBMIT_CLICK_TIMEOUT, element.click())
        .await
        .is_ok_and(|r| r.is_ok())
}
async fn locate_text_center(page: &Page, text: &str) -> Option<chromiumoxide::layout::Point> {
    let script = text_center_lookup_script(text);
    let raw: String = with_deadline(SUBMIT_LOOKUP_TIMEOUT, page.evaluate_expression(script))
        .await
        .ok()?
        .ok()?
        .into_value()
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if value.get("found")?.as_bool()? {
        Some(chromiumoxide::layout::Point::new(
            value.get("x")?.as_f64()?,
            value.get("y")?.as_f64()?,
        ))
    } else {
        None
    }
}
async fn try_click_text_trusted(page: &Page, text: &str) -> bool {
    let Some(point) = locate_text_center(page, text).await else {
        return false;
    };
    with_deadline(SUBMIT_CLICK_TIMEOUT, page.click(point))
        .await
        .is_ok_and(|r| r.is_ok())
}
async fn dump_interactive_elements(page: &Page) -> String {
    let script = INTERACTIVE_ELEMENTS_SCRIPT;
    match with_deadline(CDP_STEP_TIMEOUT, page.evaluate_expression(script)).await {
        Ok(Ok(result)) => result
            .into_value::<String>()
            .unwrap_or_else(|e| format!("<failed to read interactive elements: {e}>")),
        Ok(Err(e)) => format!("<failed to evaluate interactive elements: {e}>"),
        Err(_) => "<timed out reading interactive elements>".to_string(),
    }
}
async fn selector_present(page: &Page, selector: &str) -> bool {
    let script = selector_present_script(selector);
    match with_deadline(SUBMIT_LOOKUP_TIMEOUT, page.evaluate_expression(script)).await {
        Ok(Ok(result)) => result.into_value::<bool>().unwrap_or(false),
        _ => false,
    }
}
async fn wait_for_selector_gone(page: &Page, selector: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !selector_present(page, selector).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(200)).await;
    }
}
async fn submit_step(
    page: &Page,
    filled_input: &Element,
    filled_selector: &str,
    log_tx: &Sender<LogEvent>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let pressed = with_deadline(SUBMIT_CLICK_TIMEOUT, filled_input.press_key("Enter"))
            .await
            .is_ok_and(|r| r.is_ok());
        if pressed && wait_for_selector_gone(page, filled_selector, Duration::from_secs(5)).await {
            return Ok(());
        }
        if try_click_selector_trusted(page, SUBMIT_SELECTOR).await
            && wait_for_selector_gone(page, filled_selector, Duration::from_secs(5)).await
        {
            return Ok(());
        }
        for text in SUBMIT_TEXT_CANDIDATES {
            if try_click_text_trusted(page, text).await
                && wait_for_selector_gone(page, filled_selector, Duration::from_secs(3)).await
            {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            log(
                log_tx,
                format!(
                    "Giving up submitting '{filled_selector}' after {attempt} attempts, current page: {}\nHTML: {}\nInteractive elements: {}",
                    describe_page(page).await,
                    dump_page_html(page).await,
                    dump_interactive_elements(page).await
                ),
            );
            return Err(anyhow!(
                "Failed to submit the form for {filled_selector}: the field is still on the page after {attempt} attempts"
            ));
        }
        log(
            log_tx,
            format!(
                "Submit attempt {attempt} for '{filled_selector}' did not advance the page, retrying: {}",
                describe_page(page).await
            ),
        );
    }
}
async fn setup_request_blocking(page: Arc<Page>) -> Result<tokio::task::JoinHandle<()>> {
    let mut request_paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .context("Failed to subscribe to network request events")?;
    let intercept_page = page.clone();
    let handle = tokio::spawn(async move {
        while let Some(event) = request_paused.next().await {
            let lower_url = event.request.url.to_lowercase();
            let should_block = BLOCKED_EXTENSIONS
                .iter()
                .any(|ext| lower_url.ends_with(&format!(".{ext}")));
            let outcome = if should_block {
                intercept_page
                    .execute(
                        FailRequestParams::builder()
                            .request_id(event.request_id.clone())
                            .error_reason(ErrorReason::BlockedByClient)
                            .build()
                            .unwrap(),
                    )
                    .await
                    .map(|_| ())
            } else {
                intercept_page
                    .execute(ContinueRequestParams::new(event.request_id.clone()))
                    .await
                    .map(|_| ())
            };
            if outcome.is_err() {
                break;
            }
        }
    });
    Ok(handle)
}
pub async fn login_and_get_cookie(
    profile: &Profile,
    password: &str,
    totp: &TOTP,
    log_tx: Sender<LogEvent>,
) -> Result<String> {
    set_status(&log_tx, ConnectionStatus::LoggingIn);
    log(&log_tx, "Launching headless Chrome");
    let chrome_executable = find_chrome_executable()?;
    let user_data_dir = std::env::temp_dir().join(format!("drawbridge-vpn-chrome-{}", Uuid::new_v4()));
    let config = BrowserConfig::builder()
        .chrome_executable(chrome_executable)
        .user_data_dir(&user_data_dir)
        .args([
            "--no-sandbox",
            "--disable-setuid-sandbox",
            "--disable-dev-shm-usage",
            "--disable-gpu",
            "--window-size=1280,1024",
        ])
        .viewport(Viewport {
            width: 1280,
            height: 1024,
            ..Default::default()
        })
        .enable_request_intercept()
        .build()
        .map_err(|e| anyhow!("Failed to build the browser configuration: {e}"))?;
    register_chrome_dir(&user_data_dir);
    let launched = Browser::launch(config)
        .await
        .context("Failed to launch Chrome. Make sure Google Chrome is installed");
    let (mut browser, mut handler) = match launched {
        Ok(pair) => pair,
        Err(e) => {
            unregister_chrome_dir(&user_data_dir);
            let _ = std::fs::remove_dir_all(&user_data_dir);
            return Err(e);
        }
    };
    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });
    let result = run_login_flow(&browser, profile, password, totp, &log_tx).await;
    let _ = browser.close().await;
    handler_task.abort();
    unregister_chrome_dir(&user_data_dir);
    let _ = std::fs::remove_dir_all(&user_data_dir);
    result
}
async fn run_login_flow(
    browser: &Browser,
    profile: &Profile,
    password: &str,
    totp: &TOTP,
    log_tx: &Sender<LogEvent>,
) -> Result<String> {
    let page = Arc::new(
        browser
            .new_page("about:blank")
            .await
            .context("Failed to open a new browser tab")?,
    );
    let intercept_handle = setup_request_blocking(page.clone()).await?;
    let saml_url = profile.saml_url();
    log(log_tx, format!("Navigating to {saml_url}"));
    page.goto(saml_url.as_str())
        .await
        .with_context(|| format!("Failed to navigate to {saml_url}"))?;
    log(log_tx, format!("Filling email address: {}", profile.email));
    let email_input = find_and_click(&page, "input[type=\"email\"]", Duration::from_secs(15), log_tx)
        .await
        .context("Email input did not appear")?;
    email_input
        .type_str(&profile.email)
        .await
        .context("Failed to type the email address")?;
    submit_step(&page, &email_input, "input[type=\"email\"]", log_tx).await?;
    log(log_tx, "Filling password");
    let password_input = find_and_click(
        &page,
        "input[type=\"password\"]",
        Duration::from_secs(15),
        log_tx,
    )
    .await
    .context("Password input did not appear")?;
    password_input
        .type_str(password)
        .await
        .context("Failed to type the password")?;
    submit_step(&page, &password_input, "input[type=\"password\"]", log_tx).await?;
    set_status(log_tx, ConnectionStatus::WaitingMfa);
    log(log_tx, "Waiting for the MFA method selection screen");
    resolve_mfa_method(&page, log_tx).await?;
    let otp_code = totp
        .generate_current()
        .context("Failed to generate the current TOTP code")?;
    log(log_tx, "Filling the one-time password code");
    let otp_input = find_and_click(&page, OTP_INPUT_SELECTOR, Duration::from_secs(15), log_tx)
        .await
        .context("OTP input did not appear")?;
    otp_input
        .type_str(&otp_code)
        .await
        .context("Failed to type the OTP code")?;
    submit_step(&page, &otp_input, OTP_INPUT_SELECTOR, log_tx).await?;
    log(log_tx, "Attempting to dismiss the 'stay signed in' prompt");
    let _ = find_and_click(&page, "#idSIButton9", Duration::from_secs(3), log_tx).await;
    set_status(log_tx, ConnectionStatus::ConnectingVpn);
    log(log_tx, "Waiting for the SVPNCOOKIE to be issued");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let cookies = browser
            .get_cookies()
            .await
            .context("Failed to read browser cookies")?;
        if let Some(cookie) = cookies.into_iter().find(|c| c.name == "SVPNCOOKIE") {
            log(log_tx, "SVPNCOOKIE captured");
            intercept_handle.abort();
            return Ok(cookie.value);
        }
        if Instant::now() >= deadline {
            intercept_handle.abort();
            return Err(anyhow!("Timed out waiting for the SVPNCOOKIE"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}
