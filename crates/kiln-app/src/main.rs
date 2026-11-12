#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // First, so a panic anywhere leaves a crash file.
    kiln_app::diag::install_panic_hook();
    if let Err(e) = kiln_app::win::run() {
        kiln_app::win::fatal(&e);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Kiln app runs on Windows. Use the kiln command line here.");
    std::process::exit(1);
}
