A cargo builder that uses the [`buildRustCrate`][buildRustCrate] from the nix package manager.

This yields the following benefits:
- **Sandbox builds**: A malicious dependency in project `A` cannot alter the filesystem or inject source code into libraries that will affect the build of another project `B`.
- **Shared cache**: If project `A` has a dependency on some crate, let's say `tokio`, with features `macros` and `rt`, then this builder will cache each dependency individually. So if project `B` also uses `tokio` with the same features and version, then the `tokio` dependency will not be rebuild.
- **Reproducible**: Given the same version and targets, any project will build exactly the same on different machines.

## Install

``` shell
cargo install cargo-nbuild
```

> :warning: The nix package manager needs to be [installed](https://nixos.org/download.html) on your system.

> :bulb: You also need to enable the new [nix command](https://nixos.wiki/wiki/Nix_command) in the user specific configuration or system wide configuration.

## Usage
From a Rust project run

``` shell
cargo nbuild
```

## Missing
This builder is still in early days and is missing features

- Choosing target: like with `cargo build --target ...`
- Choosing workspace package: builds only work when inside the workspace member, and not when you are at the workspace root. Ie the `cargo build --package ...` equavalent is missing.
- Remote builds: nix supports remote builds which are not currently possible
- Custom rust version: it should be possible to change the version of rustc used for the compiles
- ... other `cargo build` options

[buildRustCrate]: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md#buildrustcrate-compiling-rust-crates-using-nix-instead-of-cargo-compiling-rust-crates-using-nix-instead-of-cargo
