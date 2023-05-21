pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(feature = "one")]
pub fn one() -> u8 {
    5
}

#[cfg(feature = "two")]
pub fn one() -> u8 {
    panic!("two should not be active")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 5);
    }
}
