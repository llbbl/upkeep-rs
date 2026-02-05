//! Analyzer implementations for project inspection.

pub mod audit;
pub mod clippy;
pub mod crates_io;
pub mod unsafe_code;
pub mod unused;
pub mod util;

#[cfg(test)]
mod tests {
    #[test]
    fn analyzers_module_smoke() {
        assert!(true);
    }
}
