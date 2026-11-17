//! The client's routes under the 「网络代理」 setting: through an HTTP proxy (absolute-form request line), through
//! SOCKS5, and direct. Fake proxies on loopback forward to a real service; no outside network.
use candid_service::proxy::Route;
use candid_service::serve::{ServeOptions, Service};
use candid_service::*;
use candid_store::fixtures::temp_root;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

fn service() -> (Service, String, std::path::PathBuf) {
    let dir = temp_root("proxy");
    let mut o = ServeOptions::new(dir.join("timeline"), dir.join("run"));
    o.created_by = "candid/test".into();
    let s = Service::start(&o, |_| {}).unwrap();
    let token = std::fs::read_to_string(RunDirectory::cli_token_file(&dir.join("run"))).unwrap();
    (s, token.trim().to_string(), dir)
}

fn pipe(mut a: TcpStream, mut b: TcpStream) {
    let mut a2 = a.try_clone().unwrap();
    let mut b2 = b.try_clone().unwrap();
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut b, &mut a);
    // The upstream answered and closed: close the client side too, which ends the other direction.
    let _ = a.shutdown(std::net::Shutdown::Both);
    let _ = t.join();
}

/// An HTTP proxy that records the request line and forwards the request in origin form.
fn http_proxy(seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = c.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let line = text.lines().next().unwrap().to_string();
        seen.lock().unwrap().push(line.clone());
        let url = line.split(' ').nth(1).unwrap();
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_at(rest.find('/').unwrap());
        let mut upstream = TcpStream::connect(host).unwrap();
        let rewritten = text.replacen(url, path, 1);
        upstream.write_all(rewritten.as_bytes()).unwrap();
        pipe(c, upstream);
    });
    port
}

/// A SOCKS5 proxy (no auth) that records the requested destination.
fn socks_proxy(seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        let mut hello = [0u8; 3];
        c.read_exact(&mut hello).unwrap();
        c.write_all(&[5, 0]).unwrap();
        let mut head = [0u8; 4];
        c.read_exact(&mut head).unwrap();
        assert_eq!(head[3], 1, "IPv4 destination");
        let mut addr = [0u8; 6];
        c.read_exact(&mut addr).unwrap();
        let dest = format!("{}.{}.{}.{}:{}", addr[0], addr[1], addr[2], addr[3], u16::from_be_bytes([addr[4], addr[5]]));
        seen.lock().unwrap().push(dest.clone());
        c.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
        pipe(c, TcpStream::connect(dest).unwrap());
    });
    port
}

#[test]
fn calls_go_through_an_http_proxy_when_the_route_says_so() {
    let (svc, token, dir) = service();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = http_proxy(seen.clone());
    let client = ServiceClient::new(&svc.endpoint.url, &token).with_route(Route::Http { host: "127.0.0.1".into(), port });
    let body = client.call("status", b"{}").unwrap();
    assert!(String::from_utf8_lossy(&body).contains("store_id"));
    assert_eq!(seen.lock().unwrap()[0], format!("POST {}/v1/status HTTP/1.1", svc.endpoint.url));
    svc.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn calls_go_through_socks5_when_the_route_says_so() {
    let (svc, token, dir) = service();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = socks_proxy(seen.clone());
    let client = ServiceClient::new(&svc.endpoint.url, &token).with_route(Route::Socks { host: "127.0.0.1".into(), port });
    client.call("status", b"{}").unwrap();
    assert_eq!(seen.lock().unwrap()[0], format!("127.0.0.1:{}", svc.port()));
    svc.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dead_proxy_is_reported_as_unreachable_and_direct_still_works() {
    let (svc, token, dir) = service();
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let via_dead = ServiceClient::new(&svc.endpoint.url, &token).with_route(Route::Http { host: "127.0.0.1".into(), port: dead });
    assert_eq!(via_dead.call("status", b"{}").unwrap_err().status, 503);
    let env = std::collections::HashMap::from([
        ("CANDID_ENDPOINT".to_string(), svc.endpoint.url.clone()),
        ("CANDID_TOKEN".to_string(), token.clone()),
        ("CANDID_PROXY".to_string(), "direct".to_string()),
    ]);
    let direct = ServiceClient::discover(&env).unwrap();
    assert_eq!(direct.route(), Route::Direct);
    direct.call("status", b"{}").unwrap();
    svc.stop();
    let _ = std::fs::remove_dir_all(dir);
}
