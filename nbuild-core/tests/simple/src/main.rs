fn main() {
    let mut buffer = itoa::Buffer::new();
    let printed = buffer.format(128u64);
    assert_eq!(printed, "128");

    dbg!(printed);
}
