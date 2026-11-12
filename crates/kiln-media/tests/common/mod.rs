//! Helpers shared by the test files that need Media Foundation.

/// True when CI has found no Media Foundation codecs on this machine and set
/// `KILN_CI_NO_MF=1`. The test then says so in the log and returns.
///
/// Without that variable nothing is skipped, so a PC that lacks Media
/// Foundation fails these tests loudly.
#[allow(dead_code)]
pub fn no_mf() -> bool {
    let off = std::env::var_os("KILN_CI_NO_MF").is_some_and(|v| v == "1");
    if off {
        eprintln!("skipped: no Media Foundation");
    }
    off
}
