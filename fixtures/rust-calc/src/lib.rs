//! Aver test fixture: integer arithmetic.

/// The sum of `a` and `b`.
///
/// ```
/// assert_eq!(calc::add(2, 3), 5);
/// ```
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// The difference of `a` and `b`.
pub fn sub(a: i64, b: i64) -> i64 {
    a - b
}

/// The product of `a` and `b`.
pub fn mul(a: i64, b: i64) -> i64 {
    a * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_returns_the_difference() {
        assert_eq!(sub(5, 3), 2);
    }

    #[test]
    fn mul_returns_the_product() {
        assert_eq!(mul(4, 3), 12);
    }
}
