use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

type DirCaseCache = HashMap<PathBuf, HashMap<String, OsString>>;
static DIR_CASE_CACHE: OnceLock<RwLock<DirCaseCache>> = OnceLock::new();

fn dir_case_cache() -> &'static RwLock<DirCaseCache> {
    DIR_CASE_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn invalidate_dir_cache(path: &Path) {
    let Some(cache) = DIR_CASE_CACHE.get() else {
        return;
    };
    let mut guard = cache.write().unwrap_or_else(|e| e.into_inner());
    guard.remove(path);
    if let Some(parent) = path.parent() {
        guard.remove(parent);
    }
}

pub fn resolve_win32_path(game_root: &Path, win_path: &str) -> PathBuf {
    let trimmed = strip_windows_prefixes(win_path);
    let mut stack: Vec<&str> = Vec::new();

    for part in trimmed.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            comp => stack.push(comp),
        }
    }

    let direct = resolve_components(game_root, &stack);
    if direct.exists()
        || stack.is_empty()
        || stack[0].eq_ignore_ascii_case("data")
    {
        return direct;
    }

    let mut data_stack = Vec::with_capacity(stack.len() + 1);
    data_stack.push("data");
    data_stack.extend_from_slice(&stack);
    let under_data = resolve_components(game_root, &data_stack);
    if under_data.exists() {
        under_data
    } else {
        direct
    }
}

fn resolve_components(base: &Path, components: &[&str]) -> PathBuf {
    let mut exact = base.to_path_buf();
    for &comp in components {
        exact.push(comp);
    }
    if exact.exists() {
        return exact;
    }

    if !components.is_empty() {
        let mut lower = base.to_path_buf();
        for &comp in components {
            lower.push(comp.to_ascii_lowercase());
        }
        if lower.exists() {
            return lower;
        }
    }

    let mut current = base.to_path_buf();
    let mut existing = current.is_dir();

    for &comp in components {
        if existing {
            if let Some(matched) = find_entry_ci(&current, comp) {
                current.push(matched);
                existing = current.is_dir();
                continue;
            }
            existing = false;
        }
        current.push(comp);
    }

    current
}

pub fn wildcard_match_ci(pattern: &str, candidate: &str) -> bool {
    let pat: Vec<u8> = pattern.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let txt: Vec<u8> = candidate.bytes().map(|b| b.to_ascii_lowercase()).collect();

    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }

    pi == pat.len()
}

fn strip_windows_prefixes(raw: &str) -> &str {
    let s = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    let bytes = s.as_bytes();
    let without_drive = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        &s[2..]
    } else {
        s
    };
    without_drive.trim_start_matches(['\\', '/'])
}

fn find_entry_ci(dir: &Path, target: &str) -> Option<OsString> {
    let target_lower = target.to_ascii_lowercase();
    let cache = dir_case_cache();
    {
        let guard = cache.read().unwrap_or_else(|e| e.into_inner());
        if let Some(entries) = guard.get(dir) {
            return entries.get(&target_lower).cloned();
        }
    }

    let entries = std::fs::read_dir(dir).ok()?;
    let mut map = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(name_str) = name.to_str() {
            map.insert(name_str.to_ascii_lowercase(), name);
        }
    }
    let found = map.get(&target_lower).cloned();
    let mut guard = cache.write().unwrap_or_else(|e| e.into_inner());
    guard.insert(dir.to_path_buf(), map);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let id = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "toluca_path_test_{prefix}_{}_{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn resolves_mixed_case_paths_and_clamps_dotdot() {
        let root = TempDir::new("ci");
        let bg_dir = root.path.join("data").join("bg");
        std::fs::create_dir_all(&bg_dir).unwrap();
        let save_file = bg_dir.join("save.bin");
        std::fs::write(&save_file, b"ok").unwrap();

        assert_eq!(
            resolve_win32_path(&root.path, r"DATA\BG\Save.BIN"),
            save_file
        );
        assert_eq!(
            resolve_win32_path(&root.path, r"C:\.\data\bg\..\BG\Save.BIN"),
            save_file
        );
        assert_eq!(
            resolve_win32_path(&root.path, r"\\?\D:\..\..\..\..\data\bg\SAVE.bin"),
            save_file
        );

        let new_file = resolve_win32_path(&root.path, r"DATA\BG\NewSave.DAT");
        assert_eq!(new_file, bg_dir.join("NewSave.DAT"));
    }

    #[test]
    fn matches_wildcards_case_insensitively() {
        assert!(wildcard_match_ci("*.BIN", "save.bin"));
        assert!(wildcard_match_ci("Save?.bin", "save1.BIN"));
        assert!(!wildcard_match_ci("Save?.bin", "save12.bin"));
        assert!(wildcard_match_ci("*data*.*", "my_DATA_01.txt"));
        assert!(wildcard_match_ci("*", "anything.dat"));
        assert!(!wildcard_match_ci("foo*.txt", "bar.txt"));
    }

    #[test]
    fn dir_case_cache_hits_and_invalidates_on_create() {
        let root = TempDir::new("cache_inv");
        let sub = root.path.join("Data").join("Save");
        std::fs::create_dir_all(&sub).unwrap();
        let first = sub.join("Slot01.BIN");
        std::fs::write(&first, b"1").unwrap();

        // Mixed-case resolve populates the directory cache.
        assert_eq!(resolve_win32_path(&root.path, r"data\save\slot01.bin"), first);

        // Creating a new mixed-case file wouldn't be found via stale cache until invalidated.
        let second = sub.join("Slot02.BIN");
        std::fs::write(&second, b"2").unwrap();
        invalidate_dir_cache(&second);
        assert_eq!(
            resolve_win32_path(&root.path, r"data\save\slot02.bin"),
            second
        );
    }
}
