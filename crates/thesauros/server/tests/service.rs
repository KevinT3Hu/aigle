use aigle_remote::Remote;
use reqwest::{StatusCode, blocking::Client};
use serde_json::Value;
use std::{
    fs,
    io::BufRead,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const TOKEN: &str = "test-token-with-at-least-32-characters";
const RAW: &[u8] = include_bytes!("fixtures/synthetic.dng");
const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:aigle="http://ns.aigle.invalid/1.0/" xmp:Rating="4" aigle:SchemaVersion="1"/></rdf:RDF></x:xmpmeta>"#;
struct Service {
    child: std::process::Child,
    dir: tempfile::TempDir,
    base: String,
    imports: std::path::PathBuf,
}
impl Service {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let imports = dir.path().join("imports");
        fs::create_dir(&imports).unwrap();
        let config = thesauros::config::Config {
            data: dir.path().join("data"),
            import_roots: vec![imports.clone()],
            listen: "127.0.0.1:0".parse().unwrap(),
            min_free_bytes: 0,
            max_upload_bytes: 1024 * 1024,
            ..Default::default()
        };
        let file = dir.path().join("server.toml");
        fs::write(&file, toml::to_string(&config).unwrap()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_thesauros"))
            .arg("--config")
            .arg(file)
            .env_remove("THESAUROS_DATA")
            .env_remove("THESAUROS_LISTEN")
            .env_remove("THESAUROS_PUBLIC_URL")
            .env_remove("THESAUROS_TOKEN_FILE")
            .env_remove("AIGLE_ISOLATE")
            .env("THESAUROS_TOKEN", TOKEN)
            .env("THESAUROS_LOG", "warn")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("Thesauros: "), "server failed: {line}");
        let base = line.trim().strip_prefix("Thesauros: ").unwrap().into();
        Self {
            child,
            dir,
            base,
            imports,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}/api/{path}", self.base)
    }
    fn client(&self) -> Remote {
        Remote::new(&self.base, TOKEN.into()).unwrap()
    }
    fn upload(&self, bytes: &[u8], name: &str) -> Value {
        let response = Client::new()
            .post(self.url("upload"))
            .bearer_auth(TOKEN)
            .header(
                "x-filename",
                percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            )
            .body(bytes.to_vec())
            .send()
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response.text().unwrap_or_default()
        );
        response.json().unwrap()
    }
    fn wait_photo(&self, id: &str) -> Value {
        let start = Instant::now();
        loop {
            let p: Value = Client::new()
                .get(self.url(&format!("photos/{id}")))
                .bearer_auth(TOKEN)
                .send()
                .unwrap()
                .json()
                .unwrap();
            if p["status"] != "pending" {
                return p;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "indexing timed out"
            );
            thread::sleep(Duration::from_millis(30));
        }
    }
    fn wait_job(&self, id: &str) -> Value {
        let start = Instant::now();
        loop {
            let job: Value = Client::new()
                .get(self.url(&format!("jobs/{id}")))
                .bearer_auth(TOKEN)
                .send()
                .unwrap()
                .json()
                .unwrap();
            if !matches!(job["state"].as_str(), Some("queued" | "running")) {
                return job;
            }
            assert!(start.elapsed() < Duration::from_secs(20));
            thread::sleep(Duration::from_millis(30));
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn real_raw_index_preview_stream_checkout_and_conflicting_edits() {
    let service = Service::start();
    let p = service.upload(RAW, "sample.DNG");
    let id = p["id"].as_str().unwrap();
    let indexed = service.wait_photo(id);
    assert_eq!(indexed["status"], "ready", "{indexed}");
    assert_eq!(indexed["captured"], "2026-10-31 12:34:56");
    assert!(indexed["camera"].as_str().unwrap().contains("Synthetic"));
    assert_eq!(indexed["lens"], "Synthetic 50mm f/2");
    assert_eq!(indexed["has_preview"], true);
    assert_eq!(service.upload(RAW, "duplicate.dng")["id"], id);
    assert_eq!(service.client().list("", 100, 0).unwrap().photos.len(), 1);
    let http = Client::new();
    let raw_url = service.url(&format!("photos/{id}/raw"));
    let partial = http
        .get(&raw_url)
        .bearer_auth(TOKEN)
        .header("range", "bytes=0-7")
        .send()
        .unwrap();
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.bytes().unwrap().as_ref(), &RAW[..8]);
    for (range, expected) in [
        (format!("\"{id}\""), StatusCode::PARTIAL_CONTENT),
        ("\"different\"".into(), StatusCode::OK),
    ] {
        let result = http
            .get(&raw_url)
            .bearer_auth(TOKEN)
            .header("range", "bytes=0-7")
            .header("if-range", range)
            .send()
            .unwrap();
        assert_eq!(result.status(), expected);
    }
    assert_eq!(
        http.get(&raw_url)
            .bearer_auth(TOKEN)
            .header("range", "bytes=99999999-")
            .send()
            .unwrap()
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    let head = http.head(&raw_url).bearer_auth(TOKEN).send().unwrap();
    assert_eq!(
        head.headers()["content-length"].to_str().unwrap(),
        RAW.len().to_string()
    );
    assert_eq!(
        http.get(&raw_url)
            .bearer_auth(TOKEN)
            .header("if-none-match", format!("\"{id}\""))
            .send()
            .unwrap()
            .status(),
        StatusCode::NOT_MODIFIED
    );
    let preview = http
        .get(service.url(&format!("photos/{id}/preview")))
        .bearer_auth(TOKEN)
        .send()
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let image = image::load_from_memory(&preview.bytes().unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (64, 48));
    let remote = service.client();
    let checked = remote
        .checkout(id, &service.dir.path().join("cache"))
        .unwrap();
    assert_eq!(fs::read(&checked).unwrap(), RAW);
    assert_eq!(
        remote
            .checkout_or_open(id, &service.dir.path().join("cache"))
            .unwrap(),
        checked
    );
    fs::write(checked.with_file_name("sample.DNG.xmp"), XMP).unwrap();
    remote.push(checked.parent().unwrap()).unwrap();
    assert_eq!(remote.photo(id).unwrap().rating, 4);
    let endpoint = service.url(&format!("photos/{id}/sidecar"));
    let response = http.get(&endpoint).bearer_auth(TOKEN).send().unwrap();
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(response.text().unwrap(), XMP);
    assert!(
        http.put(&endpoint)
            .bearer_auth(TOKEN)
            .header("if-match", etag)
            .body(XMP.replace("Rating=\"4\"", "Rating=\"3\""))
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        remote
            .push(checked.parent().unwrap())
            .unwrap_err()
            .to_string()
            .contains("conflict")
    );
    assert_eq!(fs::read(&checked).unwrap(), RAW);
}
#[test]
fn async_import_boundary_parser_failures_and_validation() {
    let service = Service::start();
    let http = Client::new();
    assert_eq!(
        http.get(service.url("photos")).send().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    for name in ["../escape.NEF", "file.txt", "back\\slash.NEF"] {
        assert_eq!(
            http.post(service.url("upload"))
                .bearer_auth(TOKEN)
                .header("x-filename", name)
                .body("raw")
                .send()
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let outside = service.dir.path().join("outside.DNG");
    fs::write(&outside, RAW).unwrap();
    fs::write(service.imports.join("inside.DNG"), RAW).unwrap();
    std::os::unix::fs::symlink(&outside, service.imports.join("escape.DNG")).unwrap();
    let response = service
        .client()
        .import(service.imports.to_str().unwrap())
        .unwrap();
    let job = service.wait_job(response["job"]["id"].as_str().unwrap());
    assert_eq!(job["state"], "completed");
    assert_eq!(job["processed"], 1);
    assert_eq!(job["failed"], 1);
    assert!(service.client().import(outside.to_str().unwrap()).is_err());
    let malformed = service.upload(b"not a RAW", "malformed.NEF");
    let malformed = service.wait_photo(malformed["id"].as_str().unwrap());
    assert_eq!(malformed["status"], "failed");
    let id = service
        .client()
        .list("inside", 100, 0)
        .unwrap()
        .photos
        .remove(0)
        .id;
    let endpoint = service.url(&format!("photos/{id}/sidecar"));
    assert_eq!(
        http.put(&endpoint)
            .bearer_auth(TOKEN)
            .body(XMP)
            .send()
            .unwrap()
            .status(),
        StatusCode::PRECONDITION_FAILED
    );
    for invalid in [
        "not XML".to_string(),
        XMP.replace("Rating=\"4\"", "Rating=\"8\""),
    ] {
        assert_eq!(
            http.put(&endpoint)
                .bearer_auth(TOKEN)
                .header("if-match", "\"none\"")
                .body(invalid)
                .send()
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    fs::write(service.imports.join("inside.DNG"), b"changed").unwrap();
    assert_eq!(
        http.get(service.url(&format!("photos/{id}/raw")))
            .bearer_auth(TOKEN)
            .send()
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        http.get(format!("{}/health/ready", service.base))
            .send()
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
fn csrf(html: &str) -> String {
    html.split("name=\"csrf\"")
        .nth(1)
        .unwrap()
        .split("value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string()
}
#[test]
fn topcoat_session_csrf_rating_xss_and_logout() {
    let service = Service::start();
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = http.get(&service.base).send().unwrap();
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    let html = response.text().unwrap();
    assert!(html.contains("Sign in"));
    assert!(html.contains("htmx-2.0.11"));
    assert_eq!(
        http.post(format!("{}/actions/login", service.base))
            .header("origin", "https://attacker.invalid")
            .form(&[("token", TOKEN)])
            .send()
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let login = http
        .post(format!("{}/actions/login", service.base))
        .header("origin", &service.base)
        .form(&[("token", TOKEN)])
        .send()
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let cookie = login.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let cookie = cookie.split(';').next().unwrap();
    let html = http
        .get(&service.base)
        .header("cookie", cookie)
        .send()
        .unwrap()
        .text()
        .unwrap();
    let csrf = csrf(&html);
    assert!(html.contains("id=\"search-form\""));
    assert!(!html.contains(TOKEN));
    let p = service.upload(RAW, "evil\"><img src=x onerror=alert(1)>.DNG");
    let id = p["id"].as_str().unwrap();
    service.wait_photo(id);
    let gallery = http
        .get(format!("{}/ui/photos", service.base))
        .header("cookie", cookie)
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(!gallery.contains("evil\"><img"));
    assert!(gallery.contains("&lt;img"));
    let action = format!("{}/actions/rating", service.base);
    let fields = [
        ("csrf", csrf.as_str()),
        ("id", id),
        ("revision", "none"),
        ("rating", "5"),
    ];
    assert_eq!(
        http.post(&action)
            .header("origin", "https://attacker.invalid")
            .header("cookie", cookie)
            .form(&fields)
            .send()
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let result = http
        .post(&action)
        .header("origin", &service.base)
        .header("cookie", cookie)
        .header("hx-request", "true")
        .form(&fields)
        .send()
        .unwrap();
    assert_eq!(
        result.status(),
        StatusCode::OK,
        "{}",
        result.text().unwrap_or_default()
    );
    assert_eq!(service.client().photo(id).unwrap().rating, 5);
    assert_eq!(
        http.get(service.url("photos"))
            .header("cookie", cookie)
            .send()
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let media = http
        .get(format!("{}/media/{id}/raw", service.base))
        .header("cookie", cookie)
        .header("range", "bytes=0-3")
        .send()
        .unwrap();
    assert_eq!(media.status(), StatusCode::PARTIAL_CONTENT);
    let logout = http
        .post(format!("{}/actions/logout", service.base))
        .header("origin", &service.base)
        .header("cookie", cookie)
        .form(&[("csrf", csrf)])
        .send()
        .unwrap();
    assert_eq!(logout.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        http.get(format!("{}/ui/photos", service.base))
            .header("cookie", cookie)
            .send()
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
#[test]
fn readiness_limits_and_internal_error_redaction() {
    let service = Service::start();
    let http = Client::new();
    assert_eq!(
        http.get(format!("{}/health/live", service.base))
            .send()
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let oversized = http
        .post(service.url("upload"))
        .bearer_auth(TOKEN)
        .header("x-filename", "large.DNG")
        .body(vec![1; 1024 * 1024 + 1])
        .send()
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        http.get(service.url("photos?rating=9"))
            .bearer_auth(TOKEN)
            .send()
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    // A real I/O failure must be a redacted 500, never a client 400 with filesystem paths.
    let p = service.upload(RAW, "sample.DNG");
    let id = p["id"].as_str().unwrap();
    service.wait_photo(id);
    let path = service
        .dir
        .path()
        .join("data/originals")
        .join(format!("{id}.dng.xmp"));
    fs::create_dir(&path).unwrap();
    let response = http
        .get(service.url(&format!("photos/{id}/sidecar")))
        .bearer_auth(TOKEN)
        .send()
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.text().unwrap();
    assert!(!body.contains(service.dir.path().to_str().unwrap()));
    assert!(body.contains("internal_error"));
}
