pub const OTP_INPUT_SELECTOR: &str =
    "input[name=\"otpc\"], input[name=\"otpin\"], input[type=\"tel\"], #idTxtBx_SAOTCC_OTC, input[name=\"otc\"]";
pub const USE_CODE_TEXT: &str = "Use a verification code";
pub const ANOTHER_WAY_TEXT: &str = "I can't use my Microsoft Authenticator app right now";
pub const BLOCKED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "svg", "woff", "woff2", "gif"];
pub const SUBMIT_SELECTOR: &str =
    "input[type=\"submit\"], button[type=\"submit\"], #idSIButton9";
pub const SUBMIT_TEXT_CANDIDATES: &[&str] = &["Sign in", "Next", "Verify"];
pub const VISIBLE_TEXT_DUMP_LIMIT: usize = 3000;
pub fn escape_js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}
pub fn text_lookup_script(text: &str, click: bool) -> String {
    let target = escape_js_string(text);
    let action = if click {
        r#"
                let clickable = node;
                while (clickable && clickable !== document.body) {
                    if (clickable.tagName === 'BUTTON' || clickable.tagName === 'A' || clickable.getAttribute('role') === 'button') break;
                    clickable = clickable.parentElement;
                }
                (clickable && clickable !== document.body ? clickable : node).click();
                return true;
        "#
    } else {
        "return true;"
    };
    format!(
        r#"(() => {{
            const target = {target};
            const nodes = document.querySelectorAll('body *');
            for (const node of nodes) {{
                if (node.children.length > 0) continue;
                const content = (node.textContent || '').trim();
                if (content !== target) continue;
                const rect = node.getBoundingClientRect();
                const visible = rect.width > 0 && rect.height > 0 && node.offsetParent !== null;
                if (!visible) continue;
                {action}
            }}
            return false;
        }})()"#
    )
}
pub fn text_center_lookup_script(text: &str) -> String {
    let target = escape_js_string(text);
    format!(
        r#"(() => {{
            const target = {target};
            const nodes = document.querySelectorAll('body *');
            for (const node of nodes) {{
                if (node.children.length > 0) continue;
                const content = (node.textContent || '').trim();
                if (content !== target) continue;
                let clickable = node;
                while (clickable && clickable !== document.body) {{
                    if (clickable.tagName === 'BUTTON' || clickable.tagName === 'A' || clickable.getAttribute('role') === 'button') break;
                    clickable = clickable.parentElement;
                }}
                const el = (clickable && clickable !== document.body) ? clickable : node;
                const rect = el.getBoundingClientRect();
                const visible = rect.width > 0 && rect.height > 0 && el.offsetParent !== null;
                if (!visible) continue;
                return JSON.stringify({{ found: true, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }});
            }}
            return JSON.stringify({{ found: false }});
        }})()"#
    )
}
pub fn visible_text_script() -> String {
    format!(
        "document.body.innerText.replace(/\\s+/g, ' ').trim().slice(0, {VISIBLE_TEXT_DUMP_LIMIT})"
    )
}
pub fn selector_present_script(selector: &str) -> String {
    format!(
        "document.querySelector({}) !== null",
        escape_js_string(selector)
    )
}
pub const INTERACTIVE_ELEMENTS_SCRIPT: &str = r#"(() => {
        const nodes = document.querySelectorAll('button, input, a[role="button"], [role="button"]');
        const items = [];
        for (const node of nodes) {
            const rect = node.getBoundingClientRect();
            const visible = rect.width > 0 && rect.height > 0 && node.offsetParent !== null;
            items.push({
                tag: node.tagName,
                id: node.id || null,
                type: node.getAttribute('type') || null,
                text: (node.textContent || '').trim().slice(0, 40),
                value: node.getAttribute('value') || null,
                ariaLabel: node.getAttribute('aria-label') || null,
                visible,
            });
        }
        return JSON.stringify(items).slice(0, 3000);
    })()"#;
pub fn fill_input_script(selector: &str, value: &str) -> String {
    let selector = escape_js_string(selector);
    let value = escape_js_string(value);
    format!(
        r#"(() => {{
            const el = document.querySelector({selector});
            if (!el) return false;
            el.focus();
            const proto = el instanceof HTMLTextAreaElement
                ? HTMLTextAreaElement.prototype
                : HTMLInputElement.prototype;
            Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, {value});
            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return true;
        }})()"#
    )
}
pub fn click_selector_script(selector: &str) -> String {
    let selector = escape_js_string(selector);
    format!(
        r#"(() => {{
            const el = document.querySelector({selector});
            if (!el) return false;
            el.click();
            return true;
        }})()"#
    )
}
pub fn press_enter_script(selector: &str) -> String {
    let selector = escape_js_string(selector);
    format!(
        r#"(() => {{
            const el = document.querySelector({selector});
            if (!el) return false;
            el.focus();
            for (const type of ['keydown', 'keypress', 'keyup']) {{
                el.dispatchEvent(new KeyboardEvent(type, {{
                    key: 'Enter',
                    code: 'Enter',
                    keyCode: 13,
                    which: 13,
                    bubbles: true,
                    cancelable: true,
                }}));
            }}
            return true;
        }})()"#
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_js_string_produces_a_quoted_literal() {
        assert_eq!(escape_js_string("Next"), "\"Next\"");
    }
    #[test]
    fn escape_js_string_escapes_embedded_quotes() {
        assert_eq!(escape_js_string("say \"hi\""), "\"say \\\"hi\\\"\"");
    }
    #[test]
    fn text_lookup_script_embeds_the_escaped_target() {
        let script = text_lookup_script(ANOTHER_WAY_TEXT, false);
        assert!(script.contains(&escape_js_string(ANOTHER_WAY_TEXT)));
    }
    #[test]
    fn text_lookup_script_only_clicks_when_asked() {
        assert!(!text_lookup_script(USE_CODE_TEXT, false).contains(".click()"));
        assert!(text_lookup_script(USE_CODE_TEXT, true).contains(".click()"));
    }
    #[test]
    fn selector_present_script_escapes_the_selector() {
        let script = selector_present_script(OTP_INPUT_SELECTOR);
        assert!(script.contains(&escape_js_string(OTP_INPUT_SELECTOR)));
        assert!(!script.contains("input[name=\"otpc\"]"));
    }
    #[test]
    fn visible_text_script_uses_the_dump_limit() {
        assert!(visible_text_script().contains(&VISIBLE_TEXT_DUMP_LIMIT.to_string()));
    }
    #[test]
    fn fill_input_script_uses_the_prototype_setter() {
        let script = fill_input_script("input[type=\"email\"]", "a@b.com");
        assert!(script.contains("Object.getOwnPropertyDescriptor"));
        assert!(script.contains("HTMLInputElement.prototype"));
        assert!(!script.contains("el.value ="));
    }
    #[test]
    fn fill_input_script_dispatches_input_and_change() {
        let script = fill_input_script("input", "x");
        assert!(script.contains("new Event('input'"));
        assert!(script.contains("new Event('change'"));
    }
    #[test]
    fn fill_input_script_escapes_both_arguments() {
        let script = fill_input_script("input[name=\"otpc\"]", "pa\"ss");
        assert!(script.contains(&escape_js_string("input[name=\"otpc\"]")));
        assert!(script.contains(&escape_js_string("pa\"ss")));
    }
    #[test]
    fn click_selector_script_escapes_the_selector() {
        let script = click_selector_script(SUBMIT_SELECTOR);
        assert!(script.contains(&escape_js_string(SUBMIT_SELECTOR)));
        assert!(script.contains(".click()"));
    }
    #[test]
    fn no_script_performs_a_native_form_submission() {
        for script in [
            fill_input_script("input", "x"),
            click_selector_script(SUBMIT_SELECTOR),
            press_enter_script("input"),
            text_lookup_script("Next", true),
        ] {
            assert!(!script.contains("requestSubmit"), "script: {script}");
            assert!(!script.contains(".submit()"), "script: {script}");
        }
    }
    #[test]
    fn press_enter_script_fires_the_full_key_sequence() {
        let script = press_enter_script("input");
        for event in ["keydown", "keypress", "keyup"] {
            assert!(script.contains(event), "missing {event}");
        }
        assert!(script.contains("'Enter'"));
    }
}
