#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(e) = kiln_app::win::run() {
        kiln_app::win::fatal(&e);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Kiln app runs on Windows. Use the kiln command line here.");
    std::process::exit(1);
}
