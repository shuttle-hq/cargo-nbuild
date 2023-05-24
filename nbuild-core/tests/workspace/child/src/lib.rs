#[cfg(feature = "one")]
pub fn one() -> u8 {
    let result = 5;
    let mut buffer = itoa::Buffer::new();
    let printed = buffer.format(result);
    assert_eq!(printed, "5");

    result
}

#[cfg(feature = "two")]
pub fn one() -> u8 {
    panic!("two should not be active")
}
