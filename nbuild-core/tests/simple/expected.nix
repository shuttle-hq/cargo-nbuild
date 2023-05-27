{ pkgs ? import <nixpkgs> {} }:

let
  # Core
  simple = pkgs.buildRustCrate rec {
    crateName = "simple";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/simple;

    dependencies = [
      itoa_1_0_6
    ];
    buildDependencies = [arbitrary_1_3_0];
    edition = "2021";
  } ;

  # Dependencies
  itoa_1_0_6 = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    src = /home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6;
    edition = "2018";
  };
  arbitrary_1_3_0 = pkgs.buildRustCrate rec {
    crateName = "arbitrary";
    version = "1.3.0";

    src = /home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/arbitrary-1.3.0;
    edition = "2018";
  };
in
simple
