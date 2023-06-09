use fnv::FnvHashMap;

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

pub fn lib_path() -> FnvHashMap<u32, &'static str> {
    let mut map = FnvHashMap::default();
    map.insert(1, "one");
    map.insert(2, "two");

    map
}

#[rustversion::before(1.68)]
pub fn version() -> &'static str {
    "< 1.68.0"
}

#[rustversion::since(1.68)]
pub fn version() -> &'static str {
    ">= 1.68.0"
}
