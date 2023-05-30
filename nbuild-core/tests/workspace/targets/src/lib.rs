#[cfg(feature = "unix")]
pub fn target() -> &'static str {
    "unix"
}

#[cfg(feature = "windows")]
pub fn target() -> &'static str {
    "windows"
}
