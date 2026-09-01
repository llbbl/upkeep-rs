//! Test-only helpers that pin the JSON examples in `docs/` to the types they
//! document.
//!
//! Every documented example is marked with `<!-- cargo-upkeep-example:<name> -->`
//! and followed immediately by a fenced ```json block. A test builds a
//! representative value, serializes it, and compares. That is what stops a
//! documented contract from drifting away from the code that produces it.
//!
//! Shared rather than copied per module on purpose: a second implementation of
//! the marker-and-fence parsing would let one page's examples be checked more
//! loosely than another's, which defeats the point of having one mechanism.

use serde::Serialize;
use serde_json::Value;

/// The documented JSON examples on one page under `docs/`.
pub struct DocumentedExamples {
    relative_path: String,
    text: String,
}

impl DocumentedExamples {
    /// Reads a documentation page, relative to the crate root.
    pub fn load(relative_path: &str) -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "could not read documented JSON contracts from {}: {error}",
                path.display()
            )
        });

        Self {
            relative_path: relative_path.to_string(),
            text,
        }
    }

    /// Parses the example marked `<!-- cargo-upkeep-example:<name> -->`.
    ///
    /// Panics when the marker is missing, duplicated, or not followed by a
    /// fenced `json` block — all of which mean the page no longer says what a
    /// test believes it says.
    pub fn example(&self, name: &str) -> Value {
        let page = &self.relative_path;
        let marker = format!("<!-- cargo-upkeep-example:{name} -->");
        assert_eq!(
            self.text.matches(&marker).count(),
            1,
            "{name}: expected exactly one {marker} marker in {page}"
        );

        let (_, after_marker) = self
            .text
            .split_once(&marker)
            .unwrap_or_else(|| panic!("{name}: missing {marker} marker in {page}"));
        let fenced = after_marker.trim_start();
        let json_with_fence = fenced.strip_prefix("```json\n").unwrap_or_else(|| {
            panic!("{name}: marker in {page} must be followed by a fenced `json` block")
        });
        let (json, _) = json_with_fence.split_once("\n```").unwrap_or_else(|| {
            panic!("{name}: JSON example in {page} is missing its closing code fence")
        });

        serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("{name}: documented example is invalid JSON: {error}"))
    }
}

/// Round-trips an output value through its serialized form, so comparisons run
/// against exactly what a caller would receive on stdout.
pub fn serialized_value<T: Serialize>(output: &T) -> Value {
    let json = serde_json::to_string(output).expect("serialize output fixture");
    serde_json::from_str(&json).expect("parse serialized output fixture")
}
