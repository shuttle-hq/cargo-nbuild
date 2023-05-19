let
  moz_overlay = import (builtins.fetchTarball https://github.com/mozilla/nixpkgs-mozilla/archive/master.tar.gz);
  # Pin to stable from https://status.nixos.org/
  nixpkgs = import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/596a8e828c5dfa504f91918d0fa4152db3ab5502.tar.gz") { overlays = [ moz_overlay ]; };
in
  with nixpkgs;
  stdenv.mkDerivation {
    name = "moz_overlay_shell";
    buildInputs = with nixpkgs; [
      ((rustChannelOf{ channel = "1.68.0"; }).rust.override {
        extensions = ["rust-src"];
      })
      cargo-watch
    ];
  }
