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
  } ;

  # Dependencies
  itoa_1_0_6 = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    src = /home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6;
  };
in
simple
