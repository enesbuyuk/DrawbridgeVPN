use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Context, Result};
use tokio::time::{sleep, timeout as with_deadline};
use totp_rs::TOTP;
use super::script::{
    click_selector_script, fill_input_script, press_enter_script,
    selector_present_script, text_lookup_script, visible_text_script, ANOTHER_WAY_TEXT,
    BLOCKED_EXTENSIONS, INTERACTIVE_ELEMENTS_SCRIPT, OTP_INPUT_SELECTOR, SUBMIT_SELECTOR,
    SUBMIT_TEXT_CANDIDATES, USE_CODE_TEXT,
};
use crate::events::{ConnectionStatus, LogEvent};
use crate::profile::Profile;
use crate::webview::WebView;
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
fn log(tx: &Sender<LogEvent>, message: impl Into<String>) {
    let _ = tx.send(LogEvent::Log(message.into()));
}
fn set_status(tx: &Sender<LogEvent>, new_status: ConnectionStatus) {
    let _ = tx.send(LogEvent::Status(new_status));
}
async fn eval_bool(webview: &WebView, script: &str) -> bool {
    matches!(
        with_deadline(STEP_TIMEOUT, webview.eval(script)).await,
        Ok(Ok(serde_json::Value::Bool(true)))
    )
}
async fn eval_string(webview: &WebView, script: &str) -> String {
    match with_deadline(STEP_TIMEOUT, webview.eval(script)).await {
        Ok(Ok(serde_json::Value::String(text))) => text,
        Ok(Ok(other)) => format!("<unexpected eval result: {other}>"),
        Ok(Err(e)) => format!("<eval failed: {e}>"),
        Err(_) => "<eval timed out>".to_string(),
    }
}
async fn describe_page(webview: &WebView) -> String {
    let url = webview.url().await.unwrap_or_else(|| "<unknown url>".to_string());
    let title = webview
        .title()
        .await
        .unwrap_or_else(|| "<unknown title>".to_string());
    format!("url={url} title={title}")
}
async fn dump_page_text(webview: &WebView) -> String {
    eval_string(webview, &visible_text_script()).await
}
async fn dump_interactive_elements(webview: &WebView) -> String {
    eval_string(webview, INTERACTIVE_ELEMENTS_SCRIPT).await
}
async fn selector_present(webview: &WebView, selector: &str) -> bool {
    eval_bool(webview, &selector_present_script(selector)).await
}
async fn wait_for_input(
    webview: &WebView,
    selector: &str,
    timeout: Duration,
    log_tx: &Sender<LogEvent>,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_report = Instant::now();
    loop {
        if selector_present(webview, selector).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            log(
                log_tx,
                format!(
                    "Giving up on selector '{selector}', current page: {}\nHTML: {}\nInteractive elements: {}",
                    describe_page(webview).await,
                    dump_page_text(webview).await,
                    dump_interactive_elements(webview).await
                ),
            );
            return Err(anyhow!("Timed out waiting for selector {selector}"));
        }
        if last_report.elapsed() >= Duration::from_secs(3) {
            last_report = Instant::now();
            log(
                log_tx,
                format!(
                    "Still waiting for selector '{selector}', current page: {}\nHTML: {}",
                    describe_page(webview).await,
                    dump_page_text(webview).await
                ),
            );
        }
        sleep(Duration::from_millis(150)).await;
    }
}
async fn wait_for_selector_gone(webview: &WebView, selector: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !selector_present(webview, selector).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(200)).await;
    }
}
async fn submit_step(
    webview: &WebView,
    filled_selector: &str,
    log_tx: &Sender<LogEvent>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if eval_bool(webview, &click_selector_script(SUBMIT_SELECTOR)).await
            && wait_for_selector_gone(webview, filled_selector, Duration::from_secs(5)).await
        {
            return Ok(());
        }
        for text in SUBMIT_TEXT_CANDIDATES {
            if eval_bool(webview, &text_lookup_script(text, true)).await
                && wait_for_selector_gone(webview, filled_selector, Duration::from_secs(3)).await
            {
                return Ok(());
            }
        }
        if eval_bool(webview, &press_enter_script(filled_selector)).await
            && wait_for_selector_gone(webview, filled_selector, Duration::from_secs(3)).await
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            log(
                log_tx,
                format!(
                    "Giving up submitting '{filled_selector}' after {attempt} attempts, current page: {}\nHTML: {}\nInteractive elements: {}",
                    describe_page(webview).await,
                    dump_page_text(webview).await,
                    dump_interactive_elements(webview).await
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
                describe_page(webview).await
            ),
        );
    }
}
async fn fill_and_submit(
    webview: &WebView,
    selector: &str,
    value: &str,
    what: &str,
    log_tx: &Sender<LogEvent>,
) -> Result<()> {
    wait_for_input(webview, selector, Duration::from_secs(15), log_tx)
        .await
        .with_context(|| format!("{what} input did not appear"))?;
    if !eval_bool(webview, &fill_input_script(selector, value)).await {
        return Err(anyhow!("Failed to fill the {what} field"));
    }
    submit_step(webview, selector, log_tx).await
}
async fn resolve_mfa_method(webview: &WebView, log_tx: &Sender<LogEvent>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    log(
        log_tx,
        format!(
            "MFA screen state: {}\nHTML: {}",
            describe_page(webview).await,
            dump_page_text(webview).await
        ),
    );
    let mut last_report = Instant::now();
    loop {
        if eval_bool(webview, &text_lookup_script(USE_CODE_TEXT, false)).await {
            log(
                log_tx,
                "MFA option 'Use a verification code' is visible, selecting it",
            );
            eval_bool(webview, &text_lookup_script(USE_CODE_TEXT, true)).await;
            return Ok(());
        }
        if eval_bool(webview, &text_lookup_script(ANOTHER_WAY_TEXT, false)).await {
            log(
                log_tx,
                "Selecting 'I can't use my Microsoft Authenticator app right now'",
            );
            eval_bool(webview, &text_lookup_script(ANOTHER_WAY_TEXT, true)).await;
            let inner_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if eval_bool(webview, &text_lookup_script(USE_CODE_TEXT, true)).await {
                    return Ok(());
                }
                if Instant::now() >= inner_deadline {
                    return Err(anyhow!("Timed out waiting to click text: {USE_CODE_TEXT}"));
                }
                sleep(Duration::from_millis(200)).await;
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Timed out waiting for an MFA method selection option ({})\nHTML: {}",
                describe_page(webview).await,
                dump_page_text(webview).await
            ));
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            last_report = Instant::now();
            log(
                log_tx,
                format!(
                    "Still waiting for MFA options: {}\nHTML: {}",
                    describe_page(webview).await,
                    dump_page_text(webview).await
                ),
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
}
pub async fn login_and_get_cookie(
    profile: &Profile,
    password: &str,
    totp: &TOTP,
    log_tx: Sender<LogEvent>,
) -> Result<String> {
    set_status(&log_tx, ConnectionStatus::LoggingIn);
    log(&log_tx, "Opening the built-in browser");
    let webview = WebView::launch()
        .await
        .context("Failed to open the built-in browser")?;
    if let Err(e) = webview.block_url_extensions(BLOCKED_EXTENSIONS).await {
        log(&log_tx, format!("Could not block media requests: {e:#}"));
    }
    let saml_url = profile.saml_url();
    log(&log_tx, format!("Navigating to {saml_url}"));
    webview
        .navigate(&saml_url)
        .await
        .with_context(|| format!("Failed to navigate to {saml_url}"))?;
    if let Err(e) = webview.wait_until_loaded(Duration::from_secs(20)).await {
        return Err(e.context(format!(
            "Page did not finish loading {saml_url} ({})",
            describe_page(&webview).await
        )));
    }
    log(&log_tx, format!("Filling email address: {}", profile.email));
    fill_and_submit(
        &webview,
        "input[type=\"email\"]",
        &profile.email,
        "Email",
        &log_tx,
    )
    .await?;
    log(&log_tx, "Filling password");
    fill_and_submit(
        &webview,
        "input[type=\"password\"]",
        password,
        "Password",
        &log_tx,
    )
    .await?;
    set_status(&log_tx, ConnectionStatus::WaitingMfa);
    log(&log_tx, "Waiting for the MFA method selection screen");
    resolve_mfa_method(&webview, &log_tx).await?;
    let otp_code = totp
        .generate_current()
        .context("Failed to generate the current TOTP code")?;
    log(&log_tx, "Filling the one-time password code");
    fill_and_submit(&webview, OTP_INPUT_SELECTOR, &otp_code, "OTP", &log_tx).await?;
    log(&log_tx, "Attempting to dismiss the 'stay signed in' prompt");
    let dismiss_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if eval_bool(&webview, &click_selector_script("#idSIButton9")).await {
            break;
        }
        if Instant::now() >= dismiss_deadline {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    set_status(&log_tx, ConnectionStatus::ConnectingVpn);
    log(&log_tx, "Waiting for the SVPNCOOKIE to be issued");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = webview
            .cookie("SVPNCOOKIE")
            .await
            .context("Failed to read browser cookies")?
        {
            log(&log_tx, "SVPNCOOKIE captured");
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("Timed out waiting for the SVPNCOOKIE"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}
