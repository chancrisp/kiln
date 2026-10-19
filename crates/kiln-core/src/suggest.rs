//! Suggestions: what Kiln offers for the files at hand, and which of it
//! to put forward.

use crate::tools::Tool;

/// The conversions and tools Kiln can run on a set of files with the
/// engines that are installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offered {
    /// Target extensions every file can reach, in registry order.
    pub targets: Vec<String>,
    pub tools: Vec<Tool>,
}
