//! Analyzer implementations for project inspection.

pub mod audit;
pub mod crates_io;

#[cfg(test)]
mod tests {
    #[test]
    fn analyzers_module_smoke() {
        assert!(true);
    }
}
