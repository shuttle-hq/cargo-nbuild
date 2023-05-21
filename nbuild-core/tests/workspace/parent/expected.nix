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
    features = ["one"];
  };
in
parent
