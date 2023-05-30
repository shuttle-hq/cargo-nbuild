{ pkgs ? import <nixpkgs> {} }:

let
  # Core
  parent = pkgs.buildRustCrate rec {
    crateName = "parent";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/parent;

    dependencies = [
      child_0_1_0
      itoa_0_4_8
      libc_0_2_144
      targets_0_1_0
    ];
    edition = "2021";
  } ;

  # Dependencies
  child_0_1_0 = pkgs.buildRustCrate rec {
    crateName = "child";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/child;
    dependencies = [fnv_1_0_7 itoa_1_0_6 libc_0_2_144 rustversion_1_0_12];
    buildDependencies = [arbitrary_1_3_0];
    features = ["one"];
    edition = "2021";
  };
  fnv_1_0_7 = pkgs.buildRustCrate rec {
    crateName = "fnv";
    version = "1.0.7";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/fnv-1.0.7;
    libPath = "lib.rs";
    edition = "2015";
  };
  itoa_1_0_6 = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6;
    edition = "2018";
  };
  libc_0_2_144 = pkgs.buildRustCrate rec {
    crateName = "libc";
    version = "0.2.144";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/libc-0.2.144;
    edition = "2015";
  };
  rustversion_1_0_12 = pkgs.buildRustCrate rec {
    crateName = "rustversion";
    version = "1.0.12";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/rustversion-1.0.12;
    build = "build/build.rs";
    procMacro = true;
    edition = "2018";
  };
  arbitrary_1_3_0 = pkgs.buildRustCrate rec {
    crateName = "arbitrary";
    version = "1.3.0";

    src = /home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/arbitrary-1.3.0;
    edition = "2018";
  };
  itoa_0_4_8 = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "0.4.8";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-0.4.8;
    edition = "2018";
  };
  targets_0_1_0 = pkgs.buildRustCrate rec {
    crateName = "targets";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/targets;
    features = ["unix"];
    edition = "2021";
  };
in
parent
