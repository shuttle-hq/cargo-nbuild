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
    ];
    edition = "2021";
  } ;

  # Dependencies
  child_0_1_0 = pkgs.buildRustCrate rec {
    crateName = "child";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/child;
    dependencies = [itoa_1_0_6 libc_0_2_144];
    features = ["one"];
    edition = "2021";
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
  itoa_0_4_8 = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "0.4.8";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-0.4.8;
    edition = "2018";
  };
in
parent
