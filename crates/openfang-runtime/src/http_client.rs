//! Shared HTTP client builder with proxy support.
//!
//! Provides a unified way to build `reqwest::Client` instances that respect
//! the proxy configuration from `KernelConfig`. Three component categories
//! are supported: LLM drivers, Skills marketplace, and built-in Tools.

use openfang_types::config::{ComponentProxyConfig, ProxyConfig};
use std::sync::OnceLock;
use tracing::debug;

/// Resolved proxy env vars for shell subprocess injection.
/// Set once at kernel boot via [`init_shell_proxy_env`].
#[derive(Debug, Clone)]
pub struct ShellProxyEnv {
    /// Value to inject as HTTP_PROXY / HTTPS_PROXY / ALL_PROXY (both cases).
    pub proxy_url: String,
    /// Value to inject as NO_PROXY (both cases). Empty string means unset.
    pub no_proxy: String,
}

/// Global singleton: stores the resolved tools proxy for shell_exec injection.
/// Written once at kernel boot (single-threaded), read-only after that.
static SHELL_PROXY_ENV: OnceLock<Option<ShellProxyEnv>> = OnceLock::new();

/// Initialise the shell proxy singleton from proxy configuration.
///
/// Must be called **once** at kernel boot (in `boot_with_config`) before any
/// `shell_exec` commands run. Calling it a second time is a no-op.
///
/// The resolved proxy URL (with credentials embedded) and `no_proxy` list are
/// stored in [`SHELL_PROXY_ENV`] and later injected directly into every
/// `tokio::process::Command` spawned by `tool_shell_exec`, avoiding any
/// reliance on `std::env::set_var` which is unsafe under a multi-threaded
/// Tokio runtime.
pub fn init_shell_proxy_env(proxy_config: &ProxyConfig) {
    let _ = SHELL_PROXY_ENV.get_or_init(|| {
        let comp = &proxy_config.tools;
        if !comp.is_enabled(proxy_config) {
            debug!("[proxy.tools] disabled — shell_exec children will use direct connections");
            return None;
        }
        let Some(proxy_url) = comp.resolved_url(proxy_config) else {
            debug!("[proxy.tools] enabled but no URL configured — skipping shell proxy injection");
            return None;
        };
        let authed_url = build_shell_proxy_url(proxy_url, proxy_config);
        let no_proxy   = proxy_config.no_proxy.join(",");
        let safe_url   = strip_proxy_credentials(proxy_url);
        debug!(
            proxy_url = %safe_url,
            no_proxy  = %no_proxy,
            "Shell proxy env initialised — shell_exec children will route through proxy"
        );
        Some(ShellProxyEnv { proxy_url: authed_url, no_proxy })
    });
}

/// Inject proxy environment variables directly into a `tokio::process::Command`.
///
/// Called from `tool_shell_exec` after [`sandbox_command`] has cleared the
/// child's environment. This is the reliable, thread-safe alternative to
/// `std::env::set_var`: the variables are written **per-Command** rather than
/// into the shared process environment, so there are no threading hazards.
///
/// Sets (when a proxy is configured):
/// - `HTTP_PROXY`, `http_proxy`
/// - `HTTPS_PROXY`, `https_proxy`
/// - `ALL_PROXY`, `all_proxy`
/// - `NO_PROXY`, `no_proxy` (when the no-proxy list is non-empty)
pub fn inject_shell_proxy_env(cmd: &mut tokio::process::Command) {
    let Some(env) = SHELL_PROXY_ENV.get().and_then(|o| o.as_ref()) else {
        return; // proxy disabled or not yet initialised
    };
    let url = &env.proxy_url;
    cmd.env("HTTP_PROXY",  url)
       .env("HTTPS_PROXY", url)
       .env("ALL_PROXY",   url)
       .env("http_proxy",  url)
       .env("https_proxy", url)
       .env("all_proxy",   url);
    if !env.no_proxy.is_empty() {
        cmd.env("NO_PROXY", &env.no_proxy)
           .env("no_proxy", &env.no_proxy);
    }
}


/// Category of component requesting an HTTP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpClientKind {
    /// LLM API drivers (Anthropic, OpenAI, Gemini, …).
    Llm,
    /// Skills marketplace (ClawHub) downloads.
    Skills,
    /// Built-in tools (web_fetch, web_search, MCP connections).
    Tools,
}

/// Build a `reqwest::Client` configured with the appropriate proxy settings
/// for the given component kind.
///
/// # Proxy resolution order
/// 1. If component-level `enabled` is explicitly set, use it.
/// 2. Otherwise fall back to the global `proxy.enabled`.
/// 3. The proxy URL resolves: component URL → global URL.
/// 4. Credentials (username, resolved password) are applied when present.
/// 5. `no_proxy` hostnames are always applied from the global config.
///
/// # Errors
/// Returns an error string if `reqwest::Client::builder().build()` fails.
pub fn build_http_client(
    kind: HttpClientKind,
    proxy_config: &ProxyConfig,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let component_cfg: &ComponentProxyConfig = match kind {
        HttpClientKind::Llm => &proxy_config.llm,
        HttpClientKind::Skills => &proxy_config.skills,
        HttpClientKind::Tools => &proxy_config.tools,
    };

    let use_proxy = component_cfg.is_enabled(proxy_config);
    let proxy_url = if use_proxy {
        component_cfg.resolved_url(proxy_config)
    } else {
        None
    };

    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs));

    if let Some(url) = proxy_url {
        debug!(
            kind = ?kind,
            proxy_url = %url,
            "Building HTTP client with proxy"
        );

        // Build reqwest proxy, then chain optional authentication and no_proxy in a single pass.
        // This avoids calling builder.proxy() twice, which would cause the second call to
        // silently replace the first — leading to the proxy being misconfigured.
        let mut req_proxy = reqwest::Proxy::all(url)
            .map_err(|e| format!("Invalid proxy URL '{}': {}", url, e))?;

        // Apply authentication if configured
        if let Some(ref username) = proxy_config.username {
            let password = proxy_config.resolved_password().unwrap_or_default();
            req_proxy = req_proxy.basic_auth(username, &password);
        }

        // Apply no_proxy exclusion list directly on the Proxy object.
        // This is the primary mechanism; apply_no_proxy_env() also sets the
        // NO_PROXY env var at kernel boot for broader process-level coverage.
        if !proxy_config.no_proxy.is_empty() {
            let no_proxy_str = proxy_config.no_proxy.join(",");
            debug!(
                no_proxy = %no_proxy_str,
                "Applying no_proxy exclusion list to proxy"
            );
            req_proxy = req_proxy.no_proxy(reqwest::NoProxy::from_string(&no_proxy_str));
        }

        builder = builder.proxy(req_proxy);
    } else {
        debug!(kind = ?kind, "Building HTTP client without proxy");
        // Explicitly disable proxy even if system env vars are set
        builder = builder.no_proxy();
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Build a `reqwest::Client` for LLM drivers.
///
/// Convenience wrapper around [`build_http_client`] for the [`HttpClientKind::Llm`] category.
/// On proxy configuration error, logs an **error** (not just a warning) and falls back to
/// a direct-connect client. If strict proxy enforcement is needed, use [`build_http_client`]
/// directly and handle the `Err` variant.
pub fn build_llm_client(proxy_config: &ProxyConfig) -> reqwest::Client {
    build_http_client(HttpClientKind::Llm, proxy_config, 120)
        .unwrap_or_else(|e| {
            tracing::error!(
                error = %e,
                "Proxy configuration error for LLM client — falling back to direct connection. \
                 Review [proxy] settings in openfang.toml."
            );
            reqwest::Client::new()
        })
}

/// Build a `reqwest::Client` for Skills marketplace operations.
///
/// Convenience wrapper around [`build_http_client`] for the [`HttpClientKind::Skills`] category.
pub fn build_skills_client(proxy_config: &ProxyConfig) -> reqwest::Client {
    build_http_client(HttpClientKind::Skills, proxy_config, 30)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to build Skills HTTP client with proxy, using default");
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default()
        })
}

/// Build a `reqwest::Client` for built-in tools (web fetch, search, MCP).
///
/// Convenience wrapper around [`build_http_client`] for the [`HttpClientKind::Tools`] category.
pub fn build_tools_client(proxy_config: &ProxyConfig, timeout_secs: u64) -> reqwest::Client {
    build_http_client(HttpClientKind::Tools, proxy_config, timeout_secs)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to build Tools HTTP client with proxy, using default");
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_default()
        })
}

/// Apply the `no_proxy` list from [`ProxyConfig`] to the `NO_PROXY` process env var.
///
/// Call this **once at kernel startup** (before spawning the async runtime or
/// creating any HTTP clients) so that **all** `reqwest::Client` instances —
/// including those built by third-party crates — automatically respect the
/// user-configured exclusion list.
///
/// If `NO_PROXY` is already set by the system environment, this function
/// leaves it untouched to avoid overriding operator-level configuration.
///
/// # Safety
/// `std::env::set_var` is not thread-safe in Rust. This function **must** be
/// called from the **main thread before the Tokio runtime starts**.
/// Calling it concurrently with any thread that reads environment variables
/// is undefined behaviour on some platforms.
pub fn apply_no_proxy_env(proxy_config: &ProxyConfig) {
    if proxy_config.no_proxy.is_empty() {
        return;
    }
    // Respect existing system-level NO_PROXY setting
    if std::env::var_os("NO_PROXY").is_some() || std::env::var_os("no_proxy").is_some() {
        debug!("NO_PROXY already set by system environment — not overriding");
        return;
    }
    let value = proxy_config.no_proxy.join(",");
    // SAFETY: called once from the main thread before the async runtime starts;
    // no concurrent env reads can occur at this point.
    unsafe {
        std::env::set_var("NO_PROXY", &value);
    }
    debug!(no_proxy = %value, "Set NO_PROXY env var from proxy config");
}

/// Apply the tools proxy URL from [`ProxyConfig`] to the standard HTTP proxy
/// environment variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`).
///
/// This is a **legacy wrapper** around [`init_shell_proxy_env`].  New code
/// should call `init_shell_proxy_env` directly. The actual injection into
/// child processes is now done per-Command in [`inject_shell_proxy_env`],
/// which avoids `std::env::set_var` UB under a multi-threaded Tokio runtime.
pub fn apply_tools_proxy_env(proxy_config: &ProxyConfig) {
    init_shell_proxy_env(proxy_config);
}

/// Build a proxy URL with embedded credentials, used for process-level env vars.
///
/// Transforms `http://host:port` → `http://user:pass@host:port` when credentials
/// are configured. If no credentials are set, the URL is returned unchanged.
///
/// # Security note
/// The resulting URL contains the password in plain text.  It is only written to
/// the **process environment** (not to logs), which on Linux is readable only by
/// root or the process itself via `/proc/self/environ`.
fn build_shell_proxy_url<'a>(proxy_url: &'a str, proxy_config: &ProxyConfig) -> String {
    let Some(ref username) = proxy_config.username else {
        return proxy_url.to_string();
    };
    let password = proxy_config.resolved_password().unwrap_or_default();
    // Insert `user:pass@` after the scheme prefix (e.g. after "http://")
    if let Some(scheme_end) = proxy_url.find("://") {
        let scheme = &proxy_url[..scheme_end + 3]; // "http://"
        let rest   = &proxy_url[scheme_end + 3..]; // "host:port"
        format!("{scheme}{username}:{password}@{rest}")
    } else {
        // Malformed URL without scheme — return as-is to avoid making it worse
        proxy_url.to_string()
    }
}

/// Remove userinfo (credentials) from a proxy URL for safe logging.
fn strip_proxy_credentials(url: &str) -> &str {
    // If the URL has `://user:pass@host`, we want to show only `scheme://host`
    // For logging purposes, just return the original URL if it has no `@`
    // (i.e. no credentials embedded), otherwise truncate after the scheme.
    if url.contains('@') {
        // Find end of scheme, e.g. "http://"
        if let Some(pos) = url.find("://") {
            return &url[..pos + 3]; // show only "http://"
        }
    }
    url
}


#[cfg(test)]
mod tests {
    use super::*;
    use openfang_types::config::{ComponentProxyConfig, ProxyConfig};

    fn make_proxy(enabled: bool, url: Option<&str>) -> ProxyConfig {
        ProxyConfig {
            enabled,
            url: url.map(|s| s.to_string()),
            ..ProxyConfig::default()
        }
    }

    #[test]
    fn test_component_proxy_inherits_global() {
        let global = make_proxy(true, Some("http://proxy.example.com:8080"));
        let comp = ComponentProxyConfig::default();
        assert!(comp.is_enabled(&global));
        assert_eq!(
            comp.resolved_url(&global),
            Some("http://proxy.example.com:8080")
        );
    }

    #[test]
    fn test_component_proxy_overrides_global() {
        let global = make_proxy(true, Some("http://global-proxy:8080"));
        let comp = ComponentProxyConfig {
            enabled: Some(false),
            url: Some("http://comp-proxy:9090".to_string()),
        };
        // enabled=false means no proxy for this component
        assert!(!comp.is_enabled(&global));
        // URL is still resolvable even if disabled
        assert_eq!(comp.resolved_url(&global), Some("http://comp-proxy:9090"));
    }

    #[test]
    fn test_build_client_no_proxy() {
        let proxy = make_proxy(false, None);
        let client = build_http_client(HttpClientKind::Llm, &proxy, 30);
        assert!(client.is_ok(), "Should build without proxy");
    }

    #[test]
    fn test_build_client_with_invalid_proxy_url() {
        // reqwest 0.12 may validate the proxy URL at different stages.
        // Use a scheme-less URL which is unambiguously invalid.
        let proxy = make_proxy(true, Some("://bad-url-no-scheme"));
        let result = build_http_client(HttpClientKind::Llm, &proxy, 30);
        // Either Proxy::all or Client::build will reject the invalid URL
        assert!(
            result.is_err(),
            "Invalid proxy URL (no scheme) should return error"
        );
    }

    #[test]
    fn test_build_client_with_valid_proxy_url() {
        let proxy = make_proxy(true, Some("http://127.0.0.1:7890"));
        let client = build_http_client(HttpClientKind::Llm, &proxy, 30);
        assert!(client.is_ok(), "Valid proxy URL should build successfully");
    }

    #[test]
    fn test_component_specific_proxy_disabled_while_global_enabled() {
        let mut proxy = make_proxy(true, Some("http://127.0.0.1:7890"));
        proxy.skills.enabled = Some(false); // skills bypass proxy
        let client = build_http_client(HttpClientKind::Skills, &proxy, 30);
        assert!(client.is_ok(), "Skills should build without proxy");
    }

    #[test]
    fn test_proxy_password_env_resolution() {
        // Set a test env var
        std::env::set_var("TEST_PROXY_PWD", "secret123");
        let mut proxy = make_proxy(false, None);
        proxy.password_env = Some("TEST_PROXY_PWD".to_string());
        assert_eq!(proxy.resolved_password(), Some("secret123".to_string()));
        std::env::remove_var("TEST_PROXY_PWD");
    }

    #[test]
    fn test_proxy_password_inline_fallback() {
        let mut proxy = make_proxy(false, None);
        proxy.password = Some("inline_pwd".to_string());
        assert_eq!(proxy.resolved_password(), Some("inline_pwd".to_string()));
    }

    #[test]
    fn test_proxy_no_password() {
        let proxy = make_proxy(false, None);
        assert_eq!(proxy.resolved_password(), None);
    }
}
