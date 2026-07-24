//! Grammar loading — maps file extensions to tree-sitter WASM grammars,
//! fetches from jsDelivr CDN on first use, caches to disk for offline reuse.
//!
//! Download path is async (via async reqwest). Grammar loading/parsing is sync
//! (WASM compilation via tree-sitter, no tokio runtime involved).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tree_sitter::{Language, WasmStore, wasmtime::Engine};

// ── Types ────────────────────────────────────────────────────────────────

pub struct GrammarEntry {
    pub pkg: &'static str,
    pub wasm: &'static str,
    pub lang_name: &'static str,
}

/// File extension → (npm package, wasm file).
pub const LANGUAGE_MAP: &[(&str, GrammarEntry)] = &[
    (
        ".rs",
        GrammarEntry {
            pkg: "tree-sitter-rust",
            wasm: "tree-sitter-rust.wasm",
            lang_name: "rust",
        },
    ),
    (
        ".py",
        GrammarEntry {
            pkg: "tree-sitter-python",
            wasm: "tree-sitter-python.wasm",
            lang_name: "python",
        },
    ),
    (
        ".pyi",
        GrammarEntry {
            pkg: "tree-sitter-python",
            wasm: "tree-sitter-python.wasm",
            lang_name: "python",
        },
    ),
    (
        ".ts",
        GrammarEntry {
            pkg: "tree-sitter-typescript",
            wasm: "tree-sitter-typescript.wasm",
            lang_name: "typescript",
        },
    ),
    (
        ".tsx",
        GrammarEntry {
            pkg: "tree-sitter-typescript",
            wasm: "tree-sitter-tsx.wasm",
            lang_name: "typescript",
        },
    ),
    (
        ".mts",
        GrammarEntry {
            pkg: "tree-sitter-typescript",
            wasm: "tree-sitter-typescript.wasm",
            lang_name: "typescript",
        },
    ),
    (
        ".cts",
        GrammarEntry {
            pkg: "tree-sitter-typescript",
            wasm: "tree-sitter-typescript.wasm",
            lang_name: "typescript",
        },
    ),
    (
        ".js",
        GrammarEntry {
            pkg: "tree-sitter-javascript",
            wasm: "tree-sitter-javascript.wasm",
            lang_name: "javascript",
        },
    ),
    (
        ".jsx",
        GrammarEntry {
            pkg: "tree-sitter-javascript",
            wasm: "tree-sitter-javascript.wasm",
            lang_name: "javascript",
        },
    ),
    (
        ".mjs",
        GrammarEntry {
            pkg: "tree-sitter-javascript",
            wasm: "tree-sitter-javascript.wasm",
            lang_name: "javascript",
        },
    ),
    (
        ".cjs",
        GrammarEntry {
            pkg: "tree-sitter-javascript",
            wasm: "tree-sitter-javascript.wasm",
            lang_name: "javascript",
        },
    ),
    (
        ".go",
        GrammarEntry {
            pkg: "tree-sitter-go",
            wasm: "tree-sitter-go.wasm",
            lang_name: "go",
        },
    ),
    (
        ".java",
        GrammarEntry {
            pkg: "tree-sitter-java",
            wasm: "tree-sitter-java.wasm",
            lang_name: "java",
        },
    ),
    (
        ".kt",
        GrammarEntry {
            pkg: "@tree-sitter-grammars/tree-sitter-kotlin",
            wasm: "tree-sitter-kotlin.wasm",
            lang_name: "kotlin",
        },
    ),
    (
        ".kts",
        GrammarEntry {
            pkg: "@tree-sitter-grammars/tree-sitter-kotlin",
            wasm: "tree-sitter-kotlin.wasm",
            lang_name: "kotlin",
        },
    ),
    (
        ".clj",
        GrammarEntry {
            pkg: "@yogthos/tree-sitter-clojure",
            wasm: "tree-sitter-clojure.wasm",
            lang_name: "clojure",
        },
    ),
    (
        ".cljs",
        GrammarEntry {
            pkg: "@yogthos/tree-sitter-clojure",
            wasm: "tree-sitter-clojure.wasm",
            lang_name: "clojure",
        },
    ),
    (
        ".cljc",
        GrammarEntry {
            pkg: "@yogthos/tree-sitter-clojure",
            wasm: "tree-sitter-clojure.wasm",
            lang_name: "clojure",
        },
    ),
    (
        ".c",
        GrammarEntry {
            pkg: "tree-sitter-c",
            wasm: "tree-sitter-c.wasm",
            lang_name: "c",
        },
    ),
    (
        ".h",
        GrammarEntry {
            pkg: "tree-sitter-c",
            wasm: "tree-sitter-c.wasm",
            lang_name: "c",
        },
    ),
    (
        ".cpp",
        GrammarEntry {
            pkg: "tree-sitter-cpp",
            wasm: "tree-sitter-cpp.wasm",
            lang_name: "cpp",
        },
    ),
    (
        ".cc",
        GrammarEntry {
            pkg: "tree-sitter-cpp",
            wasm: "tree-sitter-cpp.wasm",
            lang_name: "cpp",
        },
    ),
    (
        ".cxx",
        GrammarEntry {
            pkg: "tree-sitter-cpp",
            wasm: "tree-sitter-cpp.wasm",
            lang_name: "cpp",
        },
    ),
    (
        ".hpp",
        GrammarEntry {
            pkg: "tree-sitter-cpp",
            wasm: "tree-sitter-cpp.wasm",
            lang_name: "cpp",
        },
    ),
    (
        ".rb",
        GrammarEntry {
            pkg: "tree-sitter-ruby",
            wasm: "tree-sitter-ruby.wasm",
            lang_name: "ruby",
        },
    ),
    (
        ".sh",
        GrammarEntry {
            pkg: "tree-sitter-bash",
            wasm: "tree-sitter-bash.wasm",
            lang_name: "bash",
        },
    ),
    (
        ".bash",
        GrammarEntry {
            pkg: "tree-sitter-bash",
            wasm: "tree-sitter-bash.wasm",
            lang_name: "bash",
        },
    ),
    (
        ".lua",
        GrammarEntry {
            pkg: "tree-sitter-wasms",
            wasm: "out/tree-sitter-lua.wasm",
            lang_name: "lua",
        },
    ),
    (
        ".php",
        GrammarEntry {
            pkg: "tree-sitter-php",
            wasm: "tree-sitter-php.wasm",
            lang_name: "php",
        },
    ),
    (
        ".scala",
        GrammarEntry {
            pkg: "tree-sitter-scala",
            wasm: "tree-sitter-scala.wasm",
            lang_name: "scala",
        },
    ),
    (
        ".swift",
        GrammarEntry {
            pkg: "tree-sitter-wasms",
            wasm: "out/tree-sitter-swift.wasm",
            lang_name: "swift",
        },
    ),
    (
        ".zig",
        GrammarEntry {
            pkg: "@tree-sitter-grammars/tree-sitter-zig",
            wasm: "tree-sitter-zig.wasm",
            lang_name: "zig",
        },
    ),
    (
        ".ex",
        GrammarEntry {
            pkg: "tree-sitter-elixir",
            wasm: "tree-sitter-elixir.wasm",
            lang_name: "elixir",
        },
    ),
    (
        ".exs",
        GrammarEntry {
            pkg: "tree-sitter-elixir",
            wasm: "tree-sitter-elixir.wasm",
            lang_name: "elixir",
        },
    ),
    (
        ".cs",
        GrammarEntry {
            pkg: "tree-sitter-c-sharp",
            wasm: "tree-sitter-c_sharp.wasm",
            lang_name: "c_sharp",
        },
    ),
    (
        ".dart",
        GrammarEntry {
            pkg: "@winci/tree-sitter-dart",
            wasm: "tree-sitter-dart.wasm",
            lang_name: "dart",
        },
    ),
];

/// Look up grammar entry for a file extension.
pub fn entry_for_ext(ext: &str) -> Option<&'static GrammarEntry> {
    LANGUAGE_MAP.iter().find(|(e, _)| *e == ext).map(|(_, g)| g)
}

// ── CDN + disk cache constants ──────────────────────────────────────────

const WASM_CDN: &str = "https://cdn.jsdelivr.net/npm";
const REVALIDATE_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days
const MAX_FETCH_RETRIES: u32 = 3;
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
    /// Compiled tree-sitter Language per extension key (pkg/wasm).
    language_cache: RwLock<HashMap<String, Language>>,
    /// Reusable wasmtime Engine (expensive to create, initialized once).
    engine: OnceLock<Engine>,
    /// Serializes first-time downloads to prevent double fetch.
    /// `tokio::sync::Mutex` so it can be held across `.await`.
    download_lock: tokio::sync::Mutex<()>,
}

impl GrammarManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            wasm_cache: RwLock::new(HashMap::new()),
            language_cache: RwLock::new(HashMap::new()),
            engine: OnceLock::new(),
            download_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Ensure a grammar's WASM bytes are available (cached locally).
    ///
    /// Concurrent calls for the same grammar: only one task downloads;
    /// the rest pick up the cached result after the download lock is released.
    /// Already-cached grammars take only a read lock — no contention.
    pub async fn ensure(&self, ext: &str) -> Result<Option<()>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };

        let key = format!("{}/{}", entry.pkg, entry.wasm);

        // Fast path: language already compiled (read lock only).
        if self.language_cache.read().unwrap().contains_key(&key) {
            return Ok(Some(()));
        }

        // Serialize first-time download + compilation.
        let _guard = self.download_lock.lock().await;

        // Double-check: another task may have loaded while we waited.
        if self.language_cache.read().unwrap().contains_key(&key) {
            return Ok(Some(()));
        }

        // Ensure WASM bytes are available.
        if !self.wasm_cache.read().unwrap().contains_key(&key) {
            let bytes = self.fetch_or_load(entry).await?;
            self.wasm_cache.write().unwrap().insert(key.clone(), bytes);
        }
        let wasm = self.wasm_cache.read().unwrap().get(&key).unwrap().clone();

        // Compile language (requires Engine).
        let engine = self.engine.get_or_init(Engine::default);
        let mut store =
            WasmStore::new(engine).map_err(|e| format!("failed to create WasmStore: {e}"))?;
        let language = store
            .load_language(entry.lang_name, &wasm)
            .map_err(|e| format!("failed to load language '{}': {e}", entry.pkg))?;

        // Cache the compiled language (lightweight write lock).
        self.language_cache.write().unwrap().insert(key, language);
        Ok(Some(()))
    }

    /// Run an adapter function with a Parser that has the WasmStore and Language configured.
    pub fn with_parser<T>(
        &self,
        ext: &str,
        f: impl FnOnce(&mut tree_sitter::Parser) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };
        let key = format!("{}/{}", entry.pkg, entry.wasm);

        let language = self
            .language_cache
            .read()
            .unwrap()
            .get(&key)
            .ok_or_else(|| format!("language {key} not loaded (call ensure first)"))?
            .clone(); // Language is Clone (ref-counted)

        let engine = self.engine.get_or_init(Engine::default);
        let store =
            WasmStore::new(engine).map_err(|e| format!("failed to create WasmStore: {e}"))?;

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("set_wasm_store: {e}"))?;
        parser
            .set_language(&language)
            .map_err(|e| format!("set_language: {e}"))?;

        let result = f(&mut parser)?;
        Ok(Some(result))
    }

    /// Check if a grammar is already cached (no download).
    /// Used by sync hooks that can't await the async [`ensure`].
    pub fn check_cached(&self, ext: &str) -> Result<Option<()>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };
        let key = format!("{}/{}", entry.pkg, entry.wasm);
        if self.language_cache.read().unwrap().contains_key(&key) {
            Ok(Some(()))
        } else {
            Ok(None)
        }
    }

    /// Parse source code with the grammar for the given extension.
    /// The grammar must have been loaded via [`ensure`] first.
    pub fn parse(&self, ext: &str, source: &str) -> Result<Option<tree_sitter::Tree>, String> {
        let entry = match entry_for_ext(ext) {
            Some(e) => e,
            None => return Ok(None),
        };

        let key = format!("{}/{}", entry.pkg, entry.wasm);

        let language = self
            .language_cache
            .read()
            .unwrap()
            .get(&key)
            .ok_or_else(|| format!("language {key} not loaded (call ensure first)"))?
            .clone();

        let engine = self.engine.get_or_init(Engine::default);
        let store =
            WasmStore::new(engine).map_err(|e| format!("failed to create WasmStore: {e}"))?;

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("set_wasm_store: {e}"))?;
        parser
            .set_language(&language)
            .map_err(|e| format!("set_language: {e}"))?;

        let tree = parser.parse(source, None);
        Ok(tree)
    }

    // ── Private helpers ────────────────────────────────────────────

    async fn fetch_or_load(&self, entry: &GrammarEntry) -> Result<Vec<u8>, String> {
        let cache_dir = self.cache_dir.join(entry.pkg);
        let wasm_path = cache_dir.join(entry.wasm);
        let etag_path = wasm_path.with_extension("wasm.etag");
        let date_path = wasm_path.with_extension("wasm.date");

        let url = format!("{}/{}/{}", WASM_CDN, entry.pkg, entry.wasm);

        fn save(
            wasm: &std::path::Path,
            etag: &std::path::Path,
            date: &std::path::Path,
            bytes: &[u8],
            etag_val: &str,
        ) {
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
                        match fetch_with_etag(&url, &etag).await {
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
            match fetch_wasm(&url).await {
                Ok((bytes, etag)) => {
                    let engine = self.engine.get_or_init(Engine::default);
                    let mut store =
                        WasmStore::new(engine).map_err(|e| format!("WasmStore: {e}"))?;
                    match store.load_language(entry.lang_name, &bytes) {
                        Ok(_) => {
                            save(&wasm_path, &etag_path, &date_path, &bytes, &etag);
                            return Ok(bytes);
                        }
                        Err(e) => {
                            last_err = format!("failed to load language '{}': {e}", entry.pkg);
                        }
                    }
                }
                Err(e) => {
                    last_err = format!("fetch {url}: {e}");
                }
            }
            if attempt < MAX_FETCH_RETRIES {
                tokio::time::sleep(Duration::from_millis(1000 * u64::pow(2, attempt - 1))).await;
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

// ── HTTP helpers (async) ────────────────────────────────────────────────

enum FetchResult {
    NotModified,
    Modified(Vec<u8>, String),
}

async fn fetch_wasm(url: &str) -> Result<(Vec<u8>, String), String> {
    let client = crate::util::tls::reqwest_client();

    let resp = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
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
        .await
        .map_err(|e| format!("reading body from {url}: {e}"))?
        .to_vec();

    Ok((bytes, etag))
}

async fn fetch_with_etag(url: &str, etag: &str) -> Result<FetchResult, String> {
    let client = crate::util::tls::reqwest_client();

    let resp = client
        .get(url)
        .header("If-None-Match", etag)
        .timeout(Duration::from_secs(30))
        .send()
        .await
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
        .await
        .map_err(|e| format!("reading body from {url}: {e}"))?
        .to_vec();

    Ok(FetchResult::Modified(bytes, new_etag))
}
