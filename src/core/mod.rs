//! Core analysis modules and shared types.

pub mod analyzers;
pub mod output;
pub mod scorers;

#[cfg(test)]
mod tests {
    #[test]
    fn core_module_smoke() {
        assert!(true);
    }
}
