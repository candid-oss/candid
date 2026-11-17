//! Candid's 「网络代理」 setting (设置 → 模型 → 来源) for the `candid` command and the MCP shell, as the App
//! applies it to every HTTP client: `system` (default) follows the macOS network settings, `direct` uses no proxy.
//!
//! The setting lives in the App's defaults domain `io.github.candid-oss.candid`, key `network.proxy`, and is read on each
//! request. Under `system` the route is what CFNetwork chooses for the URL with the current system settings
//! (`CFNetworkCopyProxiesForURL`), so the exceptions list and per-protocol proxies behave as in the App.
//! The frameworks are loaded with `dlopen` only when a client resolves a route, so `candid serve` never maps them.
//! A PAC file is not executed: such a route is taken as direct. `CANDID_PROXY` overrides the setting (tests,
//! scripts). Other systems read `http_proxy`/`no_proxy` from the environment.

use std::collections::HashMap;

pub const APP_DOMAIN: &str = "io.github.candid-oss.candid";
pub const DEFAULTS_KEY: &str = "network.proxy";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyMode {
    System,
    Direct,
}

impl ProxyMode {
    /// Unknown or missing values mean `system`, as in the App.
    pub fn from_value(value: Option<&str>) -> ProxyMode {
        match value {
            Some("direct") => ProxyMode::Direct,
            _ => ProxyMode::System,
        }
    }

    pub fn current(env: &HashMap<String, String>) -> ProxyMode {
        if let Some(v) = env.get("CANDID_PROXY").filter(|v| !v.is_empty()) {
            return ProxyMode::from_value(Some(v));
        }
        ProxyMode::from_value(app_preference(APP_DOMAIN, DEFAULTS_KEY).as_deref())
    }
}

/// How a request reaches its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    /// HTTP proxy: the request line carries the absolute URL.
    Http { host: String, port: u16 },
    /// SOCKS5 without authentication.
    Socks { host: String, port: u16 },
}

pub fn route_for(mode: ProxyMode, url: &str) -> Route {
    match mode {
        ProxyMode::Direct => Route::Direct,
        ProxyMode::System => system_route(url),
    }
}

pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    h == "localhost" || h == "::1" || h.starts_with("127.") || h.ends_with(".localhost")
}

/// `no_proxy` matching for the non-macOS fallback: exact host, `.suffix`/`*.suffix`, or `*`.
pub fn bypassed(host: &str, no_proxy: &str) -> bool {
    let host = host.to_ascii_lowercase();
    no_proxy.split(',').map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty()).any(|p| {
        if p == "*" {
            return true;
        }
        let suffix = p.trim_start_matches('*').trim_start_matches('.');
        host == suffix || host.ends_with(&format!(".{suffix}"))
    })
}

/// `http://user@host:port/` → (host, port).
pub fn parse_proxy_url(value: &str) -> Option<(String, u16)> {
    let rest = value.split_once("://").map(|(_, r)| r).unwrap_or(value);
    let rest = rest.split('/').next()?.rsplit('@').next()?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || rest.starts_with('[') => (h, p.parse().ok()?),
        _ => (rest, 80),
    };
    (!host.is_empty()).then(|| (host.trim_start_matches('[').trim_end_matches(']').to_string(), port))
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or("");
    if let Some(stripped) = hostport.strip_prefix('[') {
        return stripped.split(']').next().unwrap_or("").to_string();
    }
    hostport.split(':').next().unwrap_or("").to_string()
}

#[cfg(not(target_os = "macos"))]
fn system_route(url: &str) -> Route {
    let host = host_of(url);
    if is_loopback(&host) {
        return Route::Direct;
    }
    let env: HashMap<String, String> = std::env::vars().collect();
    let get = |k: &str| env.get(k).or_else(|| env.get(&k.to_uppercase())).cloned().unwrap_or_default();
    if bypassed(&host, &get("no_proxy")) {
        return Route::Direct;
    }
    match parse_proxy_url(&get("http_proxy")) {
        Some((h, p)) => Route::Http { host: h, port: p },
        None => Route::Direct,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn app_preference(_domain: &str, _key: &str) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub use mac::app_preference;

#[cfg(target_os = "macos")]
fn system_route(url: &str) -> Route {
    // No loopback shortcut: CFNetwork bypasses localhost and ::1 by itself, but other 127.x addresses only when the
    // exceptions list says so, and the App behaves the same way.
    mac::cfnetwork_route(url).unwrap_or(Route::Direct)
}

#[cfg(target_os = "macos")]
mod mac {
    //! CoreFoundation, SystemConfiguration and CFNetwork through `dlopen`, resolved once per process.
    use super::Route;
    use std::ffi::{c_char, c_void, CString};
    use std::sync::OnceLock;

    type Ref = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const SINT32: isize = 3;

    struct Api {
        string_create: unsafe extern "C" fn(Ref, *const u8, isize, u32, u8) -> Ref,
        string_get: unsafe extern "C" fn(Ref, *mut c_char, isize, u32) -> u8,
        string_type: unsafe extern "C" fn() -> usize,
        type_of: unsafe extern "C" fn(Ref) -> usize,
        release: unsafe extern "C" fn(Ref),
        equal: unsafe extern "C" fn(Ref, Ref) -> u8,
        url_create: unsafe extern "C" fn(Ref, *const u8, isize, u32, Ref) -> Ref,
        array_count: unsafe extern "C" fn(Ref) -> isize,
        array_get: unsafe extern "C" fn(Ref, isize) -> Ref,
        dict_get: unsafe extern "C" fn(Ref, Ref) -> Ref,
        number_get: unsafe extern "C" fn(Ref, isize, *mut c_void) -> u8,
        pref_copy: unsafe extern "C" fn(Ref, Ref) -> Ref,
        sc_proxies: Option<unsafe extern "C" fn(Ref) -> Ref>,
        copy_proxies_for_url: Option<unsafe extern "C" fn(Ref, Ref) -> Ref>,
        keys: Option<[Ref; 7]>,
    }

    // SAFETY: the function pointers and the immutable CFString constants are valid for the life of the process.
    unsafe impl Send for Api {}
    unsafe impl Sync for Api {}

    unsafe fn open(path: &str) -> *mut c_void {
        let c = CString::new(path).unwrap_or_default();
        libc::dlopen(c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL)
    }

    unsafe fn sym(lib: *mut c_void, name: &str) -> *mut c_void {
        if lib.is_null() {
            return std::ptr::null_mut();
        }
        let c = CString::new(name).unwrap_or_default();
        libc::dlsym(lib, c.as_ptr())
    }

    macro_rules! func {
        ($lib:expr, $name:literal) => {{
            let p = sym($lib, $name);
            if p.is_null() {
                return None;
            }
            std::mem::transmute::<*mut c_void, _>(p)
        }};
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let cf = open("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation");
            if cf.is_null() {
                return None;
            }
            let sc = open("/System/Library/Frameworks/SystemConfiguration.framework/SystemConfiguration");
            let net = open("/System/Library/Frameworks/CFNetwork.framework/CFNetwork");
            let sc_proxies = sym(sc, "SCDynamicStoreCopyProxies");
            let copy = sym(net, "CFNetworkCopyProxiesForURL");
            let mut keys = [std::ptr::null(); 7];
            let names = ["kCFProxyTypeKey", "kCFProxyHostNameKey", "kCFProxyPortNumberKey", "kCFProxyTypeNone", "kCFProxyTypeHTTP", "kCFProxyTypeHTTPS", "kCFProxyTypeSOCKS"];
            let mut all = true;
            for (i, n) in names.iter().enumerate() {
                let p = sym(net, n) as *const Ref;
                if p.is_null() {
                    all = false;
                    break;
                }
                keys[i] = *p;
            }
            Some(Api {
                string_create: func!(cf, "CFStringCreateWithBytes"),
                string_get: func!(cf, "CFStringGetCString"),
                string_type: func!(cf, "CFStringGetTypeID"),
                type_of: func!(cf, "CFGetTypeID"),
                release: func!(cf, "CFRelease"),
                equal: func!(cf, "CFEqual"),
                url_create: func!(cf, "CFURLCreateWithBytes"),
                array_count: func!(cf, "CFArrayGetCount"),
                array_get: func!(cf, "CFArrayGetValueAtIndex"),
                dict_get: func!(cf, "CFDictionaryGetValue"),
                number_get: func!(cf, "CFNumberGetValue"),
                pref_copy: func!(cf, "CFPreferencesCopyAppValue"),
                sc_proxies: (!sc_proxies.is_null()).then(|| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Ref) -> Ref>(sc_proxies)),
                copy_proxies_for_url: (!copy.is_null()).then(|| std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Ref, Ref) -> Ref>(copy)),
                keys: all.then_some(keys),
            })
        })
        .as_ref()
    }

    /// Owned CF object, released on drop.
    struct Owned<'a>(Ref, &'a Api);

    impl Drop for Owned<'_> {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: we own one reference from a Create/Copy call.
                unsafe { (self.1.release)(self.0) }
            }
        }
    }

    fn cfstr<'a>(api: &'a Api, s: &str) -> Owned<'a> {
        // SAFETY: bytes and length describe a valid UTF-8 buffer.
        Owned(unsafe { (api.string_create)(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) }, api)
    }

    fn to_string(api: &Api, value: Ref) -> Option<String> {
        // SAFETY: value is a live CF object; the buffer length is passed.
        unsafe {
            if value.is_null() || (api.type_of)(value) != (api.string_type)() {
                return None;
            }
            let mut buf = vec![0 as c_char; 1024];
            if (api.string_get)(value, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
                return None;
            }
            Some(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }

    /// A string value from another app's preferences (CFPreferences, current user, any host).
    pub fn app_preference(domain: &str, key: &str) -> Option<String> {
        let api = api()?;
        let (k, d) = (cfstr(api, key), cfstr(api, domain));
        // SAFETY: both arguments are live CFStrings; the result is a +1 reference or NULL.
        let value = Owned(unsafe { (api.pref_copy)(k.0, d.0) }, api);
        to_string(api, value.0)
    }

    pub fn cfnetwork_route(url: &str) -> Option<Route> {
        let api = api()?;
        let keys = api.keys?;
        let [type_key, host_key, port_key, _none, http, https, socks] = keys;
        // SAFETY: every CF call below receives live objects created or copied here; results are released by Owned,
        // values read from containers are borrowed (Get rule) while the container lives.
        unsafe {
            let settings = Owned((api.sc_proxies?)(std::ptr::null()), api);
            if settings.0.is_null() {
                return None;
            }
            let cfurl = Owned((api.url_create)(std::ptr::null(), url.as_ptr(), url.len() as isize, UTF8, std::ptr::null()), api);
            if cfurl.0.is_null() {
                return None;
            }
            let list = Owned((api.copy_proxies_for_url?)(cfurl.0, settings.0), api);
            if list.0.is_null() || (api.array_count)(list.0) == 0 {
                return None;
            }
            let first = (api.array_get)(list.0, 0);
            let kind = (api.dict_get)(first, type_key);
            let is = |t: Ref| !kind.is_null() && (api.equal)(kind, t) != 0;
            let host = to_string(api, (api.dict_get)(first, host_key));
            let mut port: i32 = 0;
            let number = (api.dict_get)(first, port_key);
            if !number.is_null() {
                (api.number_get)(number, SINT32, &mut port as *mut i32 as *mut c_void);
            }
            match host {
                Some(host) if is(http) || is(https) => Some(Route::Http { host, port: if port > 0 { port as u16 } else { 80 } }),
                Some(host) if is(socks) => Some(Route::Socks { host, port: if port > 0 { port as u16 } else { 1080 } }),
                _ => Some(Route::Direct),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_system_and_reads_the_override() {
        assert_eq!(ProxyMode::from_value(None), ProxyMode::System);
        assert_eq!(ProxyMode::from_value(Some("direct")), ProxyMode::Direct);
        assert_eq!(ProxyMode::from_value(Some("bogus")), ProxyMode::System);
        let env = HashMap::from([("CANDID_PROXY".to_string(), "direct".to_string())]);
        assert_eq!(ProxyMode::current(&env), ProxyMode::Direct);
    }

    #[test]
    fn direct_mode_never_routes_through_a_proxy() {
        assert_eq!(route_for(ProxyMode::Direct, "http://example.invalid:8080/v1"), Route::Direct);
    }

    #[test]
    fn localhost_names_are_never_proxied() {
        // CFNetwork's own bypass on macOS; the explicit rule elsewhere.
        for url in ["http://localhost:1/x", "http://[::1]:9/"] {
            assert_eq!(route_for(ProxyMode::System, url), Route::Direct, "{url}");
        }
        assert!(is_loopback("127.0.0.1") && !is_loopback("example.com"));
    }

    #[test]
    fn proxy_urls_and_bypass_lists_parse() {
        assert_eq!(parse_proxy_url("http://user@proxy.example:3128/"), Some(("proxy.example".into(), 3128)));
        assert_eq!(parse_proxy_url("proxy.example"), Some(("proxy.example".into(), 80)));
        assert_eq!(parse_proxy_url(""), None);
        assert!(bypassed("nas.home.arpa", "localhost, .home.arpa"));
        assert!(bypassed("a.b", "*"));
        assert!(!bypassed("example.com", "ample.com"));
        assert_eq!(host_of("http://u@[::1]:9/x"), "::1");
        assert_eq!(host_of("http://nas.local:8080/v1"), "nas.local");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_preferences_read_as_none() {
        assert_eq!(app_preference("io.github.candid-oss.candid.test-absent", DEFAULTS_KEY), None);
    }
}

