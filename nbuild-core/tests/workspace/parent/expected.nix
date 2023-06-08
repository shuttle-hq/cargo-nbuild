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
  parent = buildRustCrate rec {
    crateName = "parent";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/parent; };

    dependencies = [
      child_0_1_0
      itoa_0_4_8
      libc_0_2_144
      targets_0_1_0
    ];
    edition = "2021";
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };

  # Dependencies
  child_0_1_0 = buildRustCrate rec {
    crateName = "child";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/child; };
    dependencies = [fnv_1_0_7 itoa_1_0_6 libc_0_2_144 rename_0_1_0 rustversion_1_0_12];
    buildDependencies = [arbitrary_1_3_0];
    crateRenames = {"rename" = "new_name";};
    features = ["one"];
    edition = "2021";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  fnv_1_0_7 = buildRustCrate rec {
    crateName = "fnv";
    version = "1.0.7";

    sha256 = "sha";
    libPath = "lib.rs";
    edition = "2015";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  itoa_1_0_6 = buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    sha256 = "sha";
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  libc_0_2_144 = buildRustCrate rec {
    crateName = "libc";
    version = "0.2.144";

    sha256 = "sha";
    edition = "2015";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  rename_0_1_0 = buildRustCrate rec {
    crateName = "rename";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/rename; };
    edition = "2021";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  rustversion_1_0_12 = buildRustCrate rec {
    crateName = "rustversion";
    version = "1.0.12";

    sha256 = "sha";
    build = "build/build.rs";
    procMacro = true;
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  arbitrary_1_3_0 = buildRustCrate rec {
    crateName = "arbitrary";
    version = "1.3.0";

    sha256 = "sha";
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  itoa_0_4_8 = buildRustCrate rec {
    crateName = "itoa";
    version = "0.4.8";

    sha256 = "sha";
    edition = "2018";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
  targets_0_1_0 = buildRustCrate rec {
    crateName = "targets";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/targets; };
    features = ["unix"];
    edition = "2021";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
in
parent
