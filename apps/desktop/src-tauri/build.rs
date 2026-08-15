use std::env;
use std::path::PathBuf;

fn main() {
    if targeting_native_windows_msvc() {
        assert_windows_sidecar_present();
    }
    tauri_build::build();
}

fn targeting_native_windows_msvc() -> bool {
    cfg!(windows)
        && env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows")
        && env::var("CARGO_CFG_TARGET_ENV").ok().as_deref() == Some("msvc")
}

fn assert_windows_sidecar_present() {
    let target = env::var("TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".to_owned());
    let sidecar = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned()))
        .join("binaries")
        .join(format!("operation-executor-{target}.exe"));
    match std::fs::metadata(&sidecar) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
        Ok(_) => panic!(
            "operation-executor sidecar at {} is not a non-empty file. \
             Do not create an empty placeholder .exe. Prepare the real sidecar: \
             npm run sidecar:prepare --workspace @working-name/desktop -- --target {target}",
            sidecar.display()
        ),
        Err(_) => panic!(
            "operation-executor sidecar missing at {}. \
             cargo check/build does not run Tauri beforeBuildCommand. \
             Prepare the real sidecar first: \
             npm run sidecar:prepare --workspace @working-name/desktop -- --target {target}. \
             Do not create an empty placeholder .exe and do not commit generated binaries.",
            sidecar.display()
        ),
    }
}
