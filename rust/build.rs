//! 把 `assets/ema2log.ico` 作为 `RT_GROUP_ICON` 嵌进 Windows 可执行文件。
//!
//! 位置要求：Cargo 只在**包根目录**（本文件所在目录，与 Cargo.toml 同级）自动识别
//! `build.rs`；放在 `src/` 下的同名文件既不是 build script，也不属于模块树，永远不会执行。
//!
//! 失败策略：显式 panic。图标缺失或 `rc.exe` 不可用时宁可构建失败，也不要静默产出
//! 一个没有图标资源、只会在事后才被发现的 exe。

fn main() {
    println!("cargo:rerun-if-changed=assets/ema2log.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    // build script 按 host 平台编译，这里再确认目标平台，
    // 免得交叉编译到非 Windows 目标时往 ELF/Mach-O 里塞 .res。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winres::WindowsResource::new();
    // winres 把相对路径原样写进 resource.rc，并以 `/I<CARGO_MANIFEST_DIR>` 调用 rc.exe，
    // 因此这里的路径基准是 `rust/`，不是 `rust/src/`。
    res.set_icon("assets/ema2log.ico");
    // winres 默认靠 `reg query HKLM\...\Windows Kits\Installed Roots` 找 SDK；
    // 注册表被策略禁用时它会退化成空路径（`bin\x64\rc.exe` → os error 3）。
    // 先自己按固定布局找一遍，命中就完全绕过注册表查询。
    if let Some(dir) = rc_toolkit_dir() {
        res.set_toolkit_path(&dir);
    }

    if let Err(err) = res.compile() {
        panic!("嵌入 Windows 图标失败（assets/ema2log.ico）：{err}");
    }
}

/// 返回**包含 `rc.exe` 的目录**（winres 的 `toolkit_path` 语义）。
///
/// 顺序：PATH（VS 开发者命令提示符）→ `%ProgramFiles(x86)%\Windows Kits\10\bin\<ver>\<arch>`。
#[cfg(windows)]
fn rc_toolkit_dir() -> Option<String> {
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    };

    if let Some(path) = std::env::var_os("PATH") {
        if let Some(dir) = std::env::split_paths(&path).find(|p| p.join("rc.exe").is_file()) {
            return Some(dir.to_string_lossy().into_owned());
        }
    }

    let mut versioned: Vec<std::path::PathBuf> = Vec::new();
    let mut plain: Vec<std::path::PathBuf> = Vec::new();
    for key in ["ProgramFiles(x86)", "ProgramFiles"] {
        let base = match std::env::var_os(key) {
            Some(base) => base,
            None => continue,
        };
        let bin = std::path::PathBuf::from(base)
            .join("Windows Kits")
            .join("10")
            .join("bin");
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        for dir in entries.flatten().map(|e| e.path()) {
            let is_version_dir = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("10."));
            // 覆盖 `<ver>\<arch>\rc.exe`（带版本号）与 `<arch>\rc.exe`（无版本号兜底）两种布局。
            if is_version_dir && dir.join(arch).join("rc.exe").is_file() {
                versioned.push(dir.join(arch));
            } else if dir.join("rc.exe").is_file() {
                plain.push(dir);
            }
        }
    }
    // 版本号字段等长，字典序即数值序（10.0.26100.0 > 10.0.17134.0）。
    versioned.sort();
    plain.sort();
    versioned
        .pop()
        .or_else(|| plain.pop())
        .map(|p| p.to_string_lossy().into_owned())
}
