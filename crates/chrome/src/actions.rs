// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use serde_json::{Value, json};

use crate::PerfMetric;
use crate::cdp_client::CdpClient;
use crate::error::{ChromeError, Result};

// ---------------------------------------------------------------------------
// JavaScript evaluation
// ---------------------------------------------------------------------------

pub(crate) async fn eval_js(
    client: &Arc<CdpClient>,
    session_id: &str,
    expression: &str,
) -> Result<String> {
    let result = client
        .send_command(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
            }),
            Some(session_id),
        )
        .await?;

    if let Some(exception) = result.get("exceptionDetails") {
        let text = exception["text"].as_str().unwrap_or("evaluation error");
        return Err(ChromeError::JsException(text.to_string()));
    }

    let val = &result["result"]["value"];
    if val.is_null() {
        Ok("undefined".to_string())
    } else if let Some(s) = val.as_str() {
        Ok(s.to_string())
    } else {
        Ok(val.to_string())
    }
}

// ---------------------------------------------------------------------------
// Screenshots
// ---------------------------------------------------------------------------

pub(crate) async fn capture_screenshot(
    client: &Arc<CdpClient>,
    session_id: &str,
    selector: Option<&str>,
) -> Result<Vec<u8>> {
    use base64::Engine;

    let params = if let Some(sel) = selector {
        let escaped = sel.replace('\\', "\\\\").replace('\'', "\\'");
        let rect_js = format!(
            "(() => {{ const el = document.querySelector('{escaped}'); \
             if (!el) return null; \
             const r = el.getBoundingClientRect(); \
             return {{x: r.x, y: r.y, width: r.width, height: r.height, scale: window.devicePixelRatio}}; }})()"
        );
        let rect_result = client
            .send_command(
                "Runtime.evaluate",
                json!({"expression": rect_js, "returnByValue": true}),
                Some(session_id),
            )
            .await?;

        let rect = &rect_result["result"]["value"];
        if rect.is_null() {
            return Err(ChromeError::ElementNotFound(sel.to_string()));
        }

        json!({
            "format": "png",
            "clip": {
                "x": rect["x"],
                "y": rect["y"],
                "width": rect["width"],
                "height": rect["height"],
                "scale": rect["scale"].as_f64().unwrap_or(1.0)
            }
        })
    } else {
        json!({"format": "png"})
    };

    let result = client
        .send_command("Page.captureScreenshot", params, Some(session_id))
        .await?;

    let data = result["data"]
        .as_str()
        .ok_or_else(|| ChromeError::Cdp("screenshot response missing data field".into()))?;

    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| ChromeError::Cdp(format!("failed to decode screenshot base64: {e}")))
}

// ---------------------------------------------------------------------------
// CSS injection
// ---------------------------------------------------------------------------

pub(crate) async fn inject_css(
    client: &Arc<CdpClient>,
    session_id: &str,
    css: &str,
    element_id: &str,
) -> Result<()> {
    let escaped_css = css
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r");

    let js = format!(
        "(() => {{ \
            const s = document.createElement('style'); \
            s.id = '{element_id}'; \
            s.textContent = '{escaped_css}'; \
            document.head.appendChild(s); \
            return true; \
        }})()"
    );

    client
        .send_command(
            "Runtime.evaluate",
            json!({"expression": js, "returnByValue": true}),
            Some(session_id),
        )
        .await?;

    Ok(())
}

pub(crate) async fn revert_css(
    client: &Arc<CdpClient>,
    session_id: &str,
    element_ids: &[String],
) -> Result<u32> {
    let mut count = 0u32;
    for id in element_ids {
        let js = format!(
            "(() => {{ const el = document.getElementById('{id}'); \
             if (el) {{ el.remove(); return true; }} return false; }})()"
        );
        let result = client
            .send_command(
                "Runtime.evaluate",
                json!({"expression": js, "returnByValue": true}),
                Some(session_id),
            )
            .await;

        if let Ok(r) = result
            && r["result"]["value"].as_bool().unwrap_or(false)
        {
            count += 1;
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tab listing
// ---------------------------------------------------------------------------

pub(crate) async fn get_tab_title(client: &Arc<CdpClient>, session_id: &str) -> String {
    match client
        .send_command(
            "Runtime.evaluate",
            json!({"expression": "document.title", "returnByValue": true}),
            Some(session_id),
        )
        .await
    {
        Ok(r) => r["result"]["value"].as_str().unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Performance metrics
// ---------------------------------------------------------------------------

pub(crate) async fn get_performance_metrics(
    client: &Arc<CdpClient>,
    session_id: &str,
) -> Result<Vec<PerfMetric>> {
    // Enable the Performance domain first
    client
        .send_command("Performance.enable", json!({}), Some(session_id))
        .await?;

    let result = client
        .send_command("Performance.getMetrics", json!({}), Some(session_id))
        .await?;

    let metrics = result["metrics"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(PerfMetric {
                        name: m["name"].as_str()?.to_string(),
                        value: m["value"].as_f64().unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(metrics)
}

// ---------------------------------------------------------------------------
// DOM inspection
// ---------------------------------------------------------------------------

pub(crate) async fn get_dom(
    client: &Arc<CdpClient>,
    session_id: &str,
    selector: Option<&str>,
) -> Result<String> {
    let js = if let Some(sel) = selector {
        let escaped = sel.replace('\\', "\\\\").replace('\'', "\\'");
        format!("document.querySelector('{escaped}')?.outerHTML ?? 'Element not found'")
    } else {
        "document.documentElement.outerHTML".to_string()
    };

    eval_js(client, session_id, &js).await
}

// ---------------------------------------------------------------------------
// Domain enablement
// ---------------------------------------------------------------------------

pub(crate) async fn enable_domains(client: &Arc<CdpClient>, session_id: &str) {
    if let Err(e) = client
        .send_command("Runtime.enable", json!({}), Some(session_id))
        .await
    {
        tracing::warn!("failed to enable Runtime for session {session_id}: {e}");
    }

    if let Err(e) = client
        .send_command(
            "Network.enable",
            json!({
                "maxTotalBufferSize": 50 * 1024 * 1024,
                "maxResourceBufferSize": 10 * 1024 * 1024,
            }),
            Some(session_id),
        )
        .await
    {
        tracing::warn!("failed to enable Network for session {session_id}: {e}");
    }

    // Page, Performance, and Log domains may not exist on non-page targets
    // (service workers, shared workers). Failures are expected -- log at debug.
    if let Err(e) = client
        .send_command("Page.enable", json!({}), Some(session_id))
        .await
    {
        tracing::debug!("Page.enable not available for session {session_id}: {e}");
    }

    if let Err(e) = client
        .send_command("Performance.enable", json!({}), Some(session_id))
        .await
    {
        tracing::debug!("Performance.enable not available for session {session_id}: {e}");
    }

    if let Err(e) = client
        .send_command("Log.enable", json!({}), Some(session_id))
        .await
    {
        tracing::debug!("Log.enable not available for session {session_id}: {e}");
    }

    if let Err(e) = client
        .send_command(
            "Page.setLifecycleEventsEnabled",
            json!({"enabled": true}),
            Some(session_id),
        )
        .await
    {
        tracing::debug!("failed to enable lifecycle events for {session_id}: {e}");
    }
}

// ---------------------------------------------------------------------------
// Target attachment
// ---------------------------------------------------------------------------

pub(crate) async fn attach_target(client: &Arc<CdpClient>, target_id: &str) -> Result<String> {
    let result = client
        .send_command(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .await?;

    result["sessionId"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ChromeError::Cdp("attach response missing sessionId".into()))
}

pub(crate) async fn get_targets(client: &Arc<CdpClient>) -> Result<Vec<Value>> {
    let result = client
        .send_command("Target.getTargets", json!({}), None)
        .await?;

    Ok(result["targetInfos"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Viewport / emulation
// ---------------------------------------------------------------------------

pub(crate) async fn set_viewport(
    client: &Arc<CdpClient>,
    session_id: &str,
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
    user_agent: Option<&str>,
) -> Result<()> {
    client
        .send_command(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": device_scale_factor,
                "mobile": mobile,
            }),
            Some(session_id),
        )
        .await?;

    if let Some(ua) = user_agent {
        client
            .send_command(
                "Network.setUserAgentOverride",
                json!({ "userAgent": ua }),
                Some(session_id),
            )
            .await?;
    }
    Ok(())
}

pub(crate) async fn clear_viewport(client: &Arc<CdpClient>, session_id: &str) -> Result<()> {
    client
        .send_command(
            "Emulation.clearDeviceMetricsOverride",
            json!({}),
            Some(session_id),
        )
        .await?;
    client
        .send_command(
            "Network.setUserAgentOverride",
            json!({ "userAgent": "" }),
            Some(session_id),
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Network conditions
// ---------------------------------------------------------------------------

pub(crate) async fn set_network_conditions(
    client: &Arc<CdpClient>,
    session_id: &str,
    preset: &str,
) -> Result<()> {
    let params = match preset {
        "offline" => json!({
            "offline": true,
            "downloadThroughput": 0,
            "uploadThroughput": 0,
            "latency": 0,
        }),
        "slow-3g" => json!({
            "offline": false,
            "downloadThroughput": 97_500,
            "uploadThroughput": 41_250,
            "latency": 400,
        }),
        "fast-3g" => json!({
            "offline": false,
            "downloadThroughput": 204_800,
            "uploadThroughput": 93_750,
            "latency": 150,
        }),
        _ => json!({
            "offline": false,
            "downloadThroughput": -1,
            "uploadThroughput": -1,
            "latency": 0,
        }),
    };
    client
        .send_command("Network.emulateNetworkConditions", params, Some(session_id))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

pub(crate) async fn navigate(
    client: &Arc<CdpClient>,
    session_id: &str,
    url: &str,
) -> Result<String> {
    client
        .send_command("Page.navigate", json!({ "url": url }), Some(session_id))
        .await?;

    // Poll readyState until complete or 30s timeout.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        match eval_js(client, session_id, "document.readyState").await {
            Ok(state) if state == "complete" => break,
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }

    Ok(get_tab_title(client, session_id).await)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub(crate) async fn storage_clear(
    client: &Arc<CdpClient>,
    session_id: &str,
    storage_types: &str,
) -> Result<()> {
    let origin = eval_js(client, session_id, "window.location.origin")
        .await
        .unwrap_or_default();

    if origin.is_empty() || origin == "null" {
        return Err(ChromeError::NoPageLoaded);
    }

    client
        .send_command(
            "Storage.clearDataForOrigin",
            json!({ "origin": origin, "storageTypes": storage_types }),
            Some(session_id),
        )
        .await?;
    Ok(())
}

pub(crate) async fn storage_inspect(
    client: &Arc<CdpClient>,
    session_id: &str,
) -> Result<serde_json::Value> {
    let ls = eval_js(
        client,
        session_id,
        "JSON.stringify(Object.fromEntries(Object.entries(localStorage)))",
    )
    .await
    .unwrap_or_else(|_| "{}".to_string());

    let ss = eval_js(
        client,
        session_id,
        "JSON.stringify(Object.fromEntries(Object.entries(sessionStorage)))",
    )
    .await
    .unwrap_or_else(|_| "{}".to_string());

    let cookies = client
        .send_command("Network.getCookies", json!({}), Some(session_id))
        .await
        .ok()
        .and_then(|r| r["cookies"].as_array().cloned())
        .unwrap_or_default();

    let local: serde_json::Value = serde_json::from_str(&ls).unwrap_or(json!({}));
    let session: serde_json::Value = serde_json::from_str(&ss).unwrap_or(json!({}));

    Ok(json!({
        "local_storage": local,
        "session_storage": session,
        "cookies": cookies,
    }))
}

pub(crate) async fn storage_set(
    client: &Arc<CdpClient>,
    session_id: &str,
    store_type: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    match store_type {
        "localstorage" | "localStorage" => {
            let k = key.replace('\'', "\\'");
            let v = value.replace('\'', "\\'");
            eval_js(
                client,
                session_id,
                &format!("localStorage.setItem('{k}', '{v}')"),
            )
            .await?;
        }
        "sessionstorage" | "sessionStorage" => {
            let k = key.replace('\'', "\\'");
            let v = value.replace('\'', "\\'");
            eval_js(
                client,
                session_id,
                &format!("sessionStorage.setItem('{k}', '{v}')"),
            )
            .await?;
        }
        "cookie" => {
            let origin = eval_js(client, session_id, "window.location.origin")
                .await
                .unwrap_or_else(|_| "http://localhost".to_string());
            client
                .send_command(
                    "Network.setCookie",
                    json!({ "name": key, "value": value, "url": origin }),
                    Some(session_id),
                )
                .await?;
        }
        other => {
            return Err(ChromeError::InvalidArgument(format!(
                "unknown store_type '{other}': use 'localstorage', 'sessionstorage', or 'cookie'"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Element at point
// ---------------------------------------------------------------------------

pub(crate) async fn element_at_point(
    client: &Arc<CdpClient>,
    session_id: &str,
    x: f64,
    y: f64,
) -> Result<serde_json::Value> {
    let result = client
        .send_command(
            "DOM.getNodeForLocation",
            json!({
                "x": x as i64,
                "y": y as i64,
                "includeUserAgentShadowDOM": false,
            }),
            Some(session_id),
        )
        .await?;

    let backend_node_id = result["backendNodeId"].as_i64().unwrap_or(0);

    let desc = client
        .send_command(
            "DOM.describeNode",
            json!({ "backendNodeId": backend_node_id }),
            Some(session_id),
        )
        .await
        .unwrap_or(json!({}));

    let node = &desc["node"];
    let tag = node["localName"]
        .as_str()
        .unwrap_or("unknown")
        .to_uppercase();

    // Attributes are flattened pairs: [name0, val0, name1, val1, ...]
    let mut attrs = serde_json::Map::new();
    if let Some(arr) = node["attributes"].as_array() {
        for pair in arr.chunks(2) {
            if let (Some(k), Some(v)) = (pair.first(), pair.get(1))
                && let (Some(ks), Some(vs)) = (k.as_str(), v.as_str())
            {
                attrs.insert(ks.to_string(), serde_json::Value::String(vs.to_string()));
            }
        }
    }

    Ok(json!({
        "x": x,
        "y": y,
        "tag": tag,
        "backend_node_id": backend_node_id,
        "attributes": attrs,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_escaping() {
        let css = "body { background: red; }\n.test { color: 'blue'; }";
        let escaped = css
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r");

        assert!(!escaped.contains('\n'));
        assert!(escaped.contains("\\n"));
        assert!(escaped.contains("\\'blue\\'"));
    }

    #[test]
    fn selector_escaping() {
        let sel = "div.class > span[data-id='test']";
        let escaped = sel.replace('\\', "\\\\").replace('\'', "\\'");
        assert!(escaped.contains("\\'test\\'"));
    }

    // -------------------------------------------------------------------------
    // Integration tests -- run only when DAEMON8_CHROME_INTEGRATION is set and
    // a Chrome is reachable at localhost:9222. Silently skip otherwise so the
    // standard `cargo test` run stays hermetic.
    // -------------------------------------------------------------------------

    fn chrome_integration_enabled() -> bool {
        std::env::var_os("DAEMON8_CHROME_INTEGRATION").is_some()
    }

    async fn connect_to_chrome() -> Result<(Arc<CdpClient>, String)> {
        let ws_url = crate::cdp_client::discover_ws_url("http://localhost:9222").await?;
        let client = Arc::new(CdpClient::connect(&ws_url).await?);
        let pump = client.clone();
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(64);
        let cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn(async move { pump.run(event_tx, cancel).await });

        let targets = get_targets(&client).await?;
        let page = targets
            .into_iter()
            .find(|t| t["type"] == "page")
            .ok_or_else(|| ChromeError::Cdp("no page target found".into()))?;

        let target_id = page["targetId"].as_str().unwrap_or("");
        let session_id = attach_target(&client, target_id).await?;
        enable_domains(&client, &session_id).await;
        Ok((client, session_id))
    }

    #[tokio::test]
    async fn integration_set_and_clear_viewport() {
        if !chrome_integration_enabled() {
            return;
        }
        let (client, sid) = connect_to_chrome().await.expect("chrome connect");
        set_viewport(&client, &sid, 390, 844, 3.0, true, Some("iPhone"))
            .await
            .expect("set_viewport");
        clear_viewport(&client, &sid).await.expect("clear_viewport");
    }

    #[tokio::test]
    async fn integration_network_conditions() {
        if !chrome_integration_enabled() {
            return;
        }
        let (client, sid) = connect_to_chrome().await.expect("chrome connect");
        set_network_conditions(&client, &sid, "slow-3g")
            .await
            .expect("slow-3g");
        set_network_conditions(&client, &sid, "restore")
            .await
            .expect("restore");
    }

    #[tokio::test]
    async fn integration_navigate() {
        if !chrome_integration_enabled() {
            return;
        }
        let (client, sid) = connect_to_chrome().await.expect("chrome connect");
        let title = navigate(&client, &sid, "https://example.com")
            .await
            .expect("navigate");
        assert!(!title.is_empty(), "expected a page title");
    }

    #[tokio::test]
    async fn integration_storage_roundtrip() {
        if !chrome_integration_enabled() {
            return;
        }
        let (client, sid) = connect_to_chrome().await.expect("chrome connect");
        navigate(&client, &sid, "https://example.com")
            .await
            .expect("navigate");
        storage_set(&client, &sid, "localstorage", "d8_test_key", "hello")
            .await
            .expect("storage_set");
        let data = storage_inspect(&client, &sid)
            .await
            .expect("storage_inspect");
        let val = data["local_storage"]["d8_test_key"].as_str().unwrap_or("");
        assert_eq!(val, "hello", "localStorage roundtrip failed");
        storage_clear(&client, &sid, "local_storage")
            .await
            .expect("storage_clear");
        let after = storage_inspect(&client, &sid)
            .await
            .expect("storage_inspect after");
        assert!(
            after["local_storage"]["d8_test_key"].is_null(),
            "key should be gone"
        );
    }

    #[tokio::test]
    async fn integration_element_at_point() {
        if !chrome_integration_enabled() {
            return;
        }
        let (client, sid) = connect_to_chrome().await.expect("chrome connect");
        navigate(&client, &sid, "https://example.com")
            .await
            .expect("navigate");
        let el = element_at_point(&client, &sid, 100.0, 100.0)
            .await
            .expect("element_at_point");
        assert!(el.get("tag").is_some(), "expected tag field");
    }
}
