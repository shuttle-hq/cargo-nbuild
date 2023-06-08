{ pkgs ? import <nixpkgs> {
  overlays = [ (import (builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz")) ];
} }:

let
  sourceFilter = name: type:
    let
      baseName = builtins.baseNameOf (builtins.toString name);
    in
      ! (
        # Filter out git
        baseName == ".gitignore"
        || (type == "directory" && baseName == ".git")

        # Filter out build results
        || (
          type == "directory" && baseName == "target"
        )

        # Filter out nix-build result symlinks
        || (
          type == "symlink" && pkgs.lib.hasPrefix "result" baseName
        )
      );
  rustVersion = pkgs.rust-bin.stable."1.68.0".default;
  defaultCrateOverrides = pkgs.defaultCrateOverrides // {
    opentelemetry-proto = attrs: { buildInputs = [ pkgs.protobuf ]; };
  };
  fetchCrate = { crateName, version, sha256 }: pkgs.fetchurl {
    # https://www.pietroalbini.org/blog/downloading-crates-io/
    # Not rate-limited, CDN URL.
    name = "${crateName}-${version}.tar.gz";
    url = "https://static.crates.io/crates/${crateName}/${crateName}-${version}.crate";
    inherit sha256;
  };
  buildRustCrate = pkgs.buildRustCrate.override {
    rustc = rustVersion;
    inherit defaultCrateOverrides fetchCrate;
  };
  preBuild = "rustc -vV";

  # Core
  simple = buildRustCrate rec {
    crateName = "simple";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/simple; };

    dependencies = [
      itoa_1_0_6
    ];
    buildDependencies = [arbitrary_1_3_0];
    edition = "2021";
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };

  # Dependencies
  itoa_1_0_6 = buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    sha256 = "itoa_sha";
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  arbitrary_1_3_0 = buildRustCrate rec {
    crateName = "arbitrary";
    version = "1.3.0";

    sha256 = "arbitrary_sha";
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
in
simple
