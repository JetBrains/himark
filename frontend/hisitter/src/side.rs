use std::ffi::CString;

use tree_sitter::Language;

extern "C" {
    fn himark_fetch_sync(
        url: *const std::os::raw::c_char,
        out_len: *mut std::os::raw::c_int,
    ) -> *mut u8;
    fn emscripten_is_main_browser_thread() -> std::os::raw::c_int;

    fn dlopen(
        filename: *const std::os::raw::c_char,
        flags: std::os::raw::c_int,
    ) -> *mut std::os::raw::c_void;
    fn dlsym(
        handle: *mut std::os::raw::c_void,
        symbol: *const std::os::raw::c_char,
    ) -> *mut std::os::raw::c_void;
    fn dlerror() -> *mut std::os::raw::c_char;
    fn free(ptr: *mut std::os::raw::c_void);
}

const RTLD_NOW: std::os::raw::c_int = 2;
static ASSET_MANIFEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn fetch(url: &str) -> Option<Vec<u8>> {
    let url = CString::new(url).ok()?;
    let mut len: std::os::raw::c_int = -1;
    unsafe {
        let buffer = himark_fetch_sync(url.as_ptr(), &mut len);
        if buffer.is_null() || len < 0 {
            return None;
        }
        let bytes = std::slice::from_raw_parts(buffer, len as usize).to_vec();
        free(buffer as *mut std::os::raw::c_void);
        Some(bytes)
    }
}

fn grammar_assets(module: &str) -> Option<(String, String)> {
    let mut cached = ASSET_MANIFEST.lock().ok()?;
    if cached.is_none() {
        *cached = Some(String::from_utf8(fetch("grammars/assets.tsv")?).ok()?);
    }
    cached.as_deref()?.lines().find_map(|line| {
        let mut fields = line.split('\t');
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(name), Some(wasm), Some(query), None) if name == module => {
                Some((wasm.to_owned(), query.to_owned()))
            }
            _ => None,
        }
    })
}

pub fn fetch_side_grammar(module: &str, symbol: &str) -> Option<(Language, String)> {
    if unsafe { emscripten_is_main_browser_thread() } != 0 {
        return None;
    }
    let (wasm_name, query_name) = grammar_assets(module)?;
    let wasm = fetch(&format!("grammars/{wasm_name}"))?;
    let query = String::from_utf8(fetch(&format!("grammars/{query_name}"))?).ok()?;

    let path = format!("/grammars/{wasm_name}");
    let _ = std::fs::create_dir_all("/grammars");
    std::fs::write(&path, wasm).ok()?;

    unsafe {
        let c_path = CString::new(path).ok()?;
        let handle = dlopen(c_path.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            let error = dlerror();
            if !error.is_null() {
                let message = std::ffi::CStr::from_ptr(error).to_string_lossy();
                eprintln!("[hisitter] dlopen {module}: {message}");
            }
            return None;
        }
        let c_symbol = CString::new(symbol).ok()?;
        let raw = dlsym(handle, c_symbol.as_ptr());
        if raw.is_null() {
            eprintln!("[hisitter] {symbol} missing in {wasm_name}");
            return None;
        }
        let builder: unsafe extern "C" fn() -> *const () = std::mem::transmute(raw);
        let language = Language::new(tree_sitter_language::LanguageFn::from_raw(builder));
        Some((language, query))
    }
}
