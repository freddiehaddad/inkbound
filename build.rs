fn main() {
    // Embed the application manifest when building with the MSVC Windows toolchain.
    // This enables Common-Controls v6 + PerMonitorV2 DPI awareness as declared in inkbound.manifest.
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    {
        println!("cargo:rerun-if-changed=inkbound.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:inkbound.manifest");
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
    }
    // If someone builds with MinGW (gnu), we just warn (no embedding here).
    #[cfg(all(target_os = "windows", not(target_env = "msvc")))]
    {
        println!(
            "cargo:warning=Manifest embedding not configured for non-MSVC toolchain; inkbound.manifest may be ignored."
        );
    }
}
