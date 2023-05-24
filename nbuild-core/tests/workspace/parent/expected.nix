{ pkgs ? import <nixpkgs> {} }:

let
  # Core
  parent = pkgs.buildRustCrate rec {
    crateName = "parent";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/parent;

    dependencies = [
      child
    ];
  } ;

  # Dependencies
  child = pkgs.buildRustCrate rec {
    crateName = "child";
    version = "0.1.0";

    src = /media/git/shuttle-hq/cargo-nbuild/nbuild-core/tests/workspace/child;
    dependencies = [itoa];
    features = ["one"];
  };
  itoa = pkgs.buildRustCrate rec {
    crateName = "itoa";
    version = "1.0.6";

    src = /home/chesedo/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6;
  };
in
parent
