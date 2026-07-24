//! Grammar loading — maps file extensions to tree-sitter WASM grammars,
//! fetches from jsDelivr CDN on first use, caches to disk for offline reuse.
//!
//! Fully synchronous. Internal `RwLock` enables concurrent reads;
//! no outer mutex needed — tools hold `Arc<GrammarManager>` directly.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tree_sitter::{wasmtime::Engine, Language, WasmStore};

// ── Types ────────────────────────────────────────────────────────────────

pub struct GrammarEntry {
    pub pkg: &'static str,
    pub wasm: &'static str,
}

/// File extension → (npm package, wasm file).
pub const LANGUAGE_MAP: &[(&str, GrammarEntry)] = &[
    (".rs",   GrammarEntry { pkg: "tree-sitter-rust", wasm: "tree-sitter-rust.wasm" }),
    (".py",   GrammarEntry { pkg: "tree-sitter-python", wasm: "tree-sitter-python.wasm" }),
    (".pyi",  GrammarEntry { pkg: "tree-sitter-python", wasm: "tree-sitter-python.wasm" }),
    (".ts",   GrammarEntry { pkg: "tree-sitter-typescript", wasm: "tree-sitter-typescript.wasm" }),
    (".tsx",  GrammarEntry { pkg: "tree-sitter-typescript", wasm: "tree-sitter-tsx.wasm" }),
    (".mts",  GrammarEntry { pkg: "tree-sitter-typescript", wasm: "tree-sitter-typescript.wasm" }),
    (".cts",  GrammarEntry { pkg: "tree-sitter-typescript", wasm: "tree-sitter-typescript.wasm" }),
    (".js",   GrammarEntry { pkg: "tree-sitter-javascript", wasm: "tree-sitter-javascript.wasm" }),
    (".jsx",  GrammarEntry { pkg: "tree-sitter-javascript", wasm: "tree-sitter-javascript.wasm" }),
    (".mjs",  GrammarEntry { pkg: "tree-sitter-javascript", wasm: "tree-sitter-javascript.wasm" }),
    (".cjs",  GrammarEntry { pkg: "tree-sitter-javascript", wasm: "tree-sitter-javascript.wasm" }),
    (".go",   GrammarEntry { pkg: "tree-sitter-go", wasm: "tree-sitter-go.wasm" }),
    (".java", GrammarEntry { pkg: "tree-sitter-java", wasm: "tree-sitter-java.wasm" }),
    (".kt",   GrammarEntry { pkg: "@tree-sitter-grammars/tree-sitter-kotlin", wasm: "tree-sitter-kotlin.wasm" }),
    (".kts",  GrammarEntry { pkg: "@tree-sitter-grammars/tree-sitter-kotlin", wasm: "tree-sitter-kotlin.wasm" }),
    (".clj",  GrammarEntry { pkg: "@yogthos/tree-sitter-clojure", wasm: "tree-sitter-clojure.wasm" }),
    (".cljs", GrammarEntry { pkg: "@yogthos/tree-sitter-clojure", wasm: "tree-sitter-clojure.wasm" }),
    (".cljc", GrammarEntry { pkg: "@yogthos/tree-sitter-clojure", wasm: "tree-sitter-clojure.wasm" }),
    (".c",    GrammarEntry { pkg: "tree-sitter-c", wasm: "tree-sitter-c.wasm" }),
    (".h",    GrammarEntry { pkg: "tree-sitter-c", wasm: "tree-sitter-c.wasm" }),
    (".cpp",  GrammarEntry { pkg: "tree-sitter-cpp", wasm: "tree-sitter-cpp.wasm" }),
    (".cc",   GrammarEntry { pkg: "tree-sitter-cpp", wasm: "tree-sitter-cpp.wasm" }),
    (".cxx",  GrammarEntry { pkg: "tree-sitter-cpp", wasm: "tree-sitter-cpp.wasm" }),
    (".hpp",  GrammarEntry { pkg: "tree-sitter-cpp", wasm: "tree-sitter-cpp.wasm" }),
    (".rb",   GrammarEntry { pkg: "tree-sitter-ruby", wasm: "tree-sitter-ruby.wasm" }),
    (".sh",   GrammarEntry { pkg: "tree-sitter-bash", wasm: "tree-sitter-bash.wasm" }),
    (".bash", GrammarEntry { pkg: "tree-sitter-bash", wasm: "tree-sitter-bash.wasm" }),
    (".lua",  GrammarEntry { pkg: "tree-sitter-wasms", wasm: "out/tree-sitter-lua.wasm" }),
    (".php",  GrammarEntry { pkg: "tree-sitter-php", wasm: "tree-sitter-php.wasm" }),
    (".scala",GrammarEntry { pkg: "tree-sitter-scala", wasm: "tree-sitter-scala.wasm" }),
    (".swift",GrammarEntry { pkg: "tree-sitter-wasms", wasm: "out/tree-sitter-swift.wasm" }),
    (".zig",  GrammarEntry { pkg: "@tree-sitter-grammars/tree-sitter-zig", wasm: "tree-sitter-zig.wasm" }),
    (".ex",   GrammarEntry { pkg: "tree-sitter-elixir", wasm: "tree-sitter-elixir.wasm" }),
    (".exs",  GrammarEntry { pkg: "tree-sitter-elixir", wasm: "tree-sitter-elixir.wasm" }),
    (".cs",   GrammarEntry { pkg: "tree-sitter-c-sharp", wasm: "tree-sitter-c_sharp.wasm" }),
    (".dart", GrammarEntry { pkg: "@winci/tree-sitter-dart", wasm: "tree-sitter-dart.wasm" }),
];

/// Look up grammar entry for a file extension.
pub fn entry_for_ext(ext: &str) -> Option<&'static GrammarEntry> {
    LANGUAGE_MAP.iter().find(|(e, _)| *e == ext).map(|(_, g)| g)
}

// ── CDN + disk cache constants ──────────────────────────────────────────

const WASM_CDN: &str = "https://cdn.jsdelivr.net/npm";
const REVALIDATE_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days
const MAX_FETCH_RETRIES: u32 = 3;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── GrammarManager ──────────────────────────────────────────────────────

/// Manages WASM grammar lifecycle: download from CDN, cache to disk.
///
/// Thread-safe: internal `RwLock` allows concurrent reads.
/// Tools can hold `Arc<GrammarManager>` directly with no outer mutex.
pub struct GrammarManager {
    cache_dir: PathBuf,
    wasm_cache: RwLock<HashMap<String, Vec<u8>>>,
    /// Serializes first-time downloads to prevent double fetch.
    download_lock: Mutex<()>,
}

impl GrammarManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            wasm_cache: RwLock::new(HashMap::new()),
            download_lock: Mutex::new(()),
        }
    }

    /// Ensure a grammar's WASM bytes are available (cached locally).
    ///
    /// Concurrent calls for the same grammar: only one thread downloads;
    /// the rest pick up the cached result after the download lock is released.
    /// Already-cached grammars take only a read lock — no contention.
    pub fn ensure(&self, ext: &str) -> Result<Option<()>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };

        let key = format!("{}/{}", entry.pkg, entry.wasm);

        // Fast path: already cached (read lock only, no contention).
        if self.wasm_cache.read().unwrap().contains_key(&key) {
            return Ok(Some(()));
        }

        // Serialize first-time downloads with double-check.
        let _guard = self.download_lock.lock().unwrap();

        // Double-check: another thread may have downloaded while we waited.
        if self.wasm_cache.read().unwrap().contains_key(&key) {
            return Ok(Some(()));
        }

        // Download without holding the RwLock (only the Mutex is held, but
        // the Mutex only serializes — readers are not blocked).
        let bytes = self.fetch_or_load(entry)?;

        // Insert under write lock (brief, just a hashmap insert).
        self.wasm_cache.write().unwrap().insert(key, bytes);
        Ok(Some(()))
    }

    /// Run an adapter function with a Language created from cached WASM bytes.
    pub fn with_lang<T>(
        &self,
        ext: &str,
        f: impl FnOnce(&Language) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };
        let key = format!("{}/{}", entry.pkg, entry.wasm);
        let wasm = self
            .wasm_cache
            .read()
            .unwrap()
            .get(&key)
            .ok_or_else(|| format!("grammar {key} not loaded (call ensure first)"))?
            .clone();

        let engine = Engine::default();
        let mut store =
            WasmStore::new(&engine).map_err(|e| format!("failed to create WasmStore: {e}"))?;
        let lang = store
            .load_language(entry.pkg, &wasm)
            .map_err(|e| format!("failed to load language '{}': {e}", entry.pkg))?;

        let result = f(&lang)?;
        Ok(Some(result))
    }

    /// Parse source code with the grammar for the given extension.
    /// The grammar must have been loaded via [`ensure`] first.
    pub fn parse(&self, ext: &str, source: &str) -> Result<Option<tree_sitter::Tree>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };

        let key = format!("{}/{}", entry.pkg, entry.wasm);
        let wasm_bytes = self
            .wasm_cache
            .read()
            .unwrap()
            .get(&key)
            .ok_or_else(|| format!("grammar {key} not loaded (call ensure first)"))?
            .clone();

        let engine = Engine::default();
        let mut store =
            WasmStore::new(&engine).map_err(|e| format!("failed to create WasmStore: {e}"))?;
        let lang = store
            .load_language(entry.pkg, &wasm_bytes)
            .map_err(|e| format!("failed to load language '{}': {e}", entry.pkg))?;

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("set_wasm_store: {e}"))?;
        parser
            .set_language(&lang)
            .map_err(|e| format!("set_language: {e}"))?;

        let tree = parser.parse(source, None);
        Ok(tree)
    }

    // ── Private helpers ────────────────────────────────────────────

    fn fetch_or_load(&self, entry: &GrammarEntry) -> Result<Vec<u8>, String> {
        let cache_dir = self.cache_dir.join(entry.pkg);
        let wasm_path = cache_dir.join(entry.wasm);
        let etag_path = wasm_path.with_extension("wasm.etag");
        let date_path = wasm_path.with_extension("wasm.date");

        let url = format!("{}/{}/{}", WASM_CDN, entry.pkg, entry.wasm);

        fn save(wasm: &std::path::Path, etag: &std::path::Path, date: &std::path::Path, bytes: &[u8], etag_val: &str) {
            if let Some(parent) = wasm.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(wasm, bytes);
            let _ = fs::write(date, now_millis().to_string());
            if !etag_val.is_empty() {
                let _ = fs::write(etag, etag_val);
            }
        }

        // 1. Try disk cache
        if let Some(bytes) = read_opt(&wasm_path) {
            let age = read_str_opt(&date_path)
                .and_then(|s| s.parse::<u64>().ok())
                .map(|d| Duration::from_millis(now_millis().saturating_sub(d)));

            match age {
                Some(d) if d < REVALIDATE_AFTER => return Ok(bytes),
                Some(_) => {
                    if let Some(etag) = read_str_opt(&etag_path) {
                        match fetch_with_etag(&url, &etag) {
                            Ok(FetchResult::NotModified) => {
                                let _ = fs::write(&date_path, now_millis().to_string());
                                return Ok(bytes);
                            }
                            Ok(FetchResult::Modified(new_bytes, new_etag)) => {
                                save(&wasm_path, &etag_path, &date_path, &new_bytes, &new_etag);
                                return Ok(new_bytes);
                            }
                            Err(_) => {
                                let _ = fs::write(&date_path, now_millis().to_string());
                                return Ok(bytes);
                            }
                        }
                    }
                }
                None => return Ok(bytes),
            }
        }

        // 2. Fresh download
        let mut last_err = String::new();
        for attempt in 1..=MAX_FETCH_RETRIES {
            match fetch_wasm(&url) {
                Ok((bytes, etag)) => {
                    let engine = Engine::default();
                    let mut store =
                        WasmStore::new(&engine).map_err(|e| format!("WasmStore: {e}"))?;
                    if store.load_language(entry.pkg, &bytes).is_ok() {
                        save(&wasm_path, &etag_path, &date_path, &bytes, &etag);
                        return Ok(bytes);
                    }
                    last_err = format!("invalid WASM bytes for {}", entry.pkg);
                }
                Err(e) => {
                    last_err = format!("fetch {url}: {e}");
                }
            }
            if attempt < MAX_FETCH_RETRIES {
                std::thread::sleep(Duration::from_millis(1000 * u64::pow(2, attempt - 1)));
            }
        }

        Err(format!(
            "failed to load grammar {} after {MAX_FETCH_RETRIES} attempts: {last_err}",
            entry.pkg
        ))
    }
}

// ── File I/O helpers ────────────────────────────────────────────────────

fn read_opt(path: &std::path::Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn read_str_opt(path: &std::path::Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

// ── HTTP helpers (blocking) ─────────────────────────────────────────────

enum FetchResult {
    NotModified,
    Modified(Vec<u8>, String),
}

fn fetch_wasm(url: &str) -> Result<(Vec<u8>, String), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("HTTP GET {url}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let bytes = resp
        .bytes()
        .map_err(|e| format!("reading body from {url}: {e}"))?
        .to_vec();

    Ok((bytes, etag))
}

fn fetch_with_etag(url: &str, etag: &str) -> Result<FetchResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .header("If-None-Match", etag)
        .send()
        .map_err(|e| format!("HTTP GET {url}: {e}"))?;

    if resp.status() == 304 {
        return Ok(FetchResult::NotModified);
    }

    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let bytes = resp
        .bytes()
        .map_err(|e| format!("reading body from {url}: {e}"))?
        .to_vec();

    Ok(FetchResult::Modified(bytes, new_etag))
}
