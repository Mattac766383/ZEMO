fn main() {
    println!("cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS");
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }
    let flags = std::env::var("LIBSQLITE3_FLAGS").unwrap_or_default();
    if !flags.contains("SQLCIPHER_OMIT_DLLMAIN") {
        panic!(
            "Windows builds must set LIBSQLITE3_FLAGS=-DSQLCIPHER_OMIT_DLLMAIN so \
             statically linked SQLCipher does not export DllMain alongside NumKong \
             (LNK2005/LNK1169). This does not disable SQLCipher. See .cargo/config.toml."
        );
    }
}
