//! Aver test fixture: formats integers with a dependency.

/// `n` in decimal.
pub fn decimal(n: u64) -> String {
    itoa::Buffer::new().format(n).to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn decimal_formats() {
        assert_eq!(super::decimal(42), "42");
    }
}
