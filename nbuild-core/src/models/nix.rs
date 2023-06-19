//! This model is used to create / print a nix derivation.

use std::{cell::RefCell, fs, rc::Rc};

use cargo_metadata::{camino::Utf8PathBuf, semver::Version};

use super::Source;

/// A package for a nix [buildRustCrate] block.
///
/// [buildRustCrate]: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md#buildrustcrate-compiling-rust-crates-using-nix-instead-of-cargo-compiling-rust-crates-using-nix-instead-of-cargo
#[derive(Debug, PartialEq)]
pub struct Package {
    pub(super) name: String,
    pub(super) version: Version,
    pub(super) source: Source,
    pub(super) lib_name: Option<String>,
    pub(super) lib_path: Option<Utf8PathBuf>,
    pub(super) build_path: Option<Utf8PathBuf>,
    pub(super) proc_macro: bool,
    pub(super) features: Vec<String>,
    pub(super) dependencies: Vec<Dependency>,
    pub(super) build_dependencies: Vec<Dependency>,
    pub(super) edition: String,
    pub(super) printed: bool,
}

/// Used to keep track of the dependencies of a package and whether they have any renames.
#[derive(Debug, PartialEq)]
pub struct Dependency {
    pub(super) package: Rc<RefCell<Package>>,
    pub(super) rename: Option<String>,
}

impl Package {
    /// Write the package to a derivation file at `.nbuild.nix`
    pub fn into_file(self) -> Result<(), std::io::Error> {
        let expr = self.into_derivative();

        fs::write(".nbuild.nix", expr)
    }

    /// The name of the package
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Turn the package into a derivation string.
    pub fn into_derivative(self) -> String {
        let Self {
            name,
            version,
            source,
            lib_name: _,
            lib_path: _,
            build_path: _,
            proc_macro: _,
            features: _,
            dependencies,
            build_dependencies,
            edition,
            printed: _,
        } = self;

        // Used to append all the dependency details unto
        let mut build_details = Default::default();

        let dep_idents: Vec<_> = dependencies
            .into_iter()
            .map(|d| {
                let identifier = d.package.borrow().identifier();
                Self::to_details(&d, &mut build_details);
                identifier
            })
            .collect();

        let build_deps = if build_dependencies.is_empty() {
            Default::default()
        } else {
            let dep_idents: Vec<_> = build_dependencies
                .into_iter()
                .map(|d| {
                    let identifier = d.package.borrow().identifier();
                    Self::to_details(&d, &mut build_details);
                    identifier
                })
                .collect();
            format!("\n    buildDependencies = [{}];", dep_idents.join(" "))
        };

        format!(
            r#"{{ pkgs ? import <nixpkgs> {{
  overlays = [ (import (builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz")) ];
}} }}:

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
  defaultCrateOverrides = pkgs.defaultCrateOverrides // {{
    opentelemetry-proto = attrs: {{ buildInputs = [ pkgs.protobuf ]; }};
  }};
  fetchCrate = {{ crateName, version, sha256 }}: pkgs.fetchurl {{
    # https://www.pietroalbini.org/blog/downloading-crates-io/
    # Not rate-limited, CDN URL.
    name = "${{crateName}}-${{version}}.tar.gz";
    url = "https://static.crates.io/crates/${{crateName}}/${{crateName}}-${{version}}.crate";
    inherit sha256;
  }};
  buildRustCrate = pkgs.buildRustCrate.override {{
    rustc = rustVersion;
    inherit defaultCrateOverrides fetchCrate;
  }};
  preBuild = "rustc -vV";

  # Core
  {} = buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    {}

    dependencies = [
      {}
    ];{}
    edition = "{}";
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  }};

  # Dependencies
{}
in
{}
"#,
            name,
            name,
            version,
            Self::get_source(&source),
            dep_idents.join("\n      "),
            build_deps,
            edition,
            build_details.join("\n"),
            name
        )
    }

    /// Recursively add a dependency unto `details`
    fn to_details(dependency: &Dependency, build_details: &mut Vec<String>) {
        let mut this = dependency.package.borrow_mut();

        // Only print once
        if this.printed {
            return;
        }

        let features = if this.features.is_empty() {
            Default::default()
        } else {
            format!(
                "\n    features = [{}];",
                this.features
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        let lib_name = if let Some(lib_name) = &this.lib_name {
            format!("\n    libName = \"{lib_name}\";")
        } else {
            Default::default()
        };
        let lib_path = if let Some(lib_path) = &this.lib_path {
            format!("\n    libPath = \"{lib_path}\";")
        } else {
            Default::default()
        };
        let build_path = if let Some(build_path) = &this.build_path {
            format!("\n    build = \"{build_path}\";")
        } else {
            Default::default()
        };
        let proc_macro = if this.proc_macro {
            "\n    procMacro = true;"
        } else {
            Default::default()
        };

        let mut renames = Vec::new();

        let deps = if this.dependencies.is_empty() {
            Default::default()
        } else {
            let dep_idents: Vec<_> = this
                .dependencies
                .iter()
                .map(|d| {
                    if let Some(rename) = &d.rename {
                        renames.push((
                            d.package.borrow().name.clone(),
                            rename.clone(),
                            d.package.borrow().version.to_string(),
                        ));
                    }

                    d.package.borrow().identifier()
                })
                .collect();
            format!("\n    dependencies = [{}];", dep_idents.join(" "))
        };
        let build_deps = if this.build_dependencies.is_empty() {
            Default::default()
        } else {
            let dep_idents: Vec<_> = this
                .build_dependencies
                .iter()
                .map(|d| {
                    if let Some(rename) = &d.rename {
                        renames.push((
                            d.package.borrow().name.clone(),
                            rename.clone(),
                            d.package.borrow().version.to_string(),
                        ));
                    }

                    d.package.borrow().identifier()
                })
                .collect();
            format!("\n    buildDependencies = [{}];", dep_idents.join(" "))
        };

        let crate_renames = if renames.is_empty() {
            Default::default()
        } else {
            let renames = renames
                .into_iter()
                .map(|(name, rename, version)| {
                    format!("\"{name}\" = [{{ rename = \"{rename}\"; version = \"{version}\"; }}];")
                })
                .collect::<Vec<_>>()
                .join(" ");

            format!("\n    crateRenames = {{{renames}}};")
        };

        let details = format!(
            r#"  {} = buildRustCrate rec {{
    crateName = "{}";{}
    version = "{}";

    {}{}{}{}{}{}{}{}
    edition = "{}";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  }};"#,
            this.identifier(),
            this.name,
            lib_name,
            this.version,
            Self::get_source(&this.source),
            lib_path,
            build_path,
            proc_macro,
            deps,
            build_deps,
            crate_renames,
            features,
            this.edition,
        );

        build_details.push(details);

        for dependency in this
            .dependencies
            .iter()
            .chain(this.build_dependencies.iter())
        {
            Self::to_details(dependency, build_details);
        }

        this.printed = true;
    }

    /// Helper to get a deterministic identifier for a package
    fn identifier(&self) -> String {
        format!(
            "{}_{}",
            self.name,
            self.version.to_string().replace(['.', '+'], "_")
        )
    }

    /// Helper to get the source definition
    fn get_source(source: &Source) -> String {
        match source {
            Source::Local(path) => format!(
                "src = pkgs.lib.cleanSourceWith {{ filter = sourceFilter;  src = {}; }};",
                path.display()
            ),
            Source::CratesIo(sha256) => format!("sha256 = \"{sha256}\";"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use super::*;

    use pretty_assertions::assert_eq;

    impl From<Package> for Dependency {
        fn from(package: Package) -> Self {
            Self {
                package: Rc::new(RefCell::new(package)),
                rename: None,
            }
        }
    }

    impl From<PathBuf> for Source {
        fn from(path: PathBuf) -> Self {
            Self::Local(path)
        }
    }

    impl From<&str> for Source {
        fn from(sha: &str) -> Self {
            Self::CratesIo(sha.to_string())
        }
    }

    #[test]
    fn simple_package() {
        let package = Package {
            name: "simple".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: PathBuf::from_str("/cargo-nbuild/nbuild-core/tests/simple")
                .unwrap()
                .into(),
            lib_name: None,
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![Package {
                name: "itoa".to_string(),
                version: "1.0.6".parse().unwrap(),
                source: "itoa_sha".into(),
                lib_name: None,
                lib_path: None,
                build_path: None,
                proc_macro: false,
                dependencies: Default::default(),
                build_dependencies: Default::default(),
                features: Default::default(),
                edition: "2018".to_string(),
                printed: false,
            }
            .into()],
            build_dependencies: vec![Package {
                name: "arbitrary".to_string(),
                version: "1.3.0".parse().unwrap(),
                source: "arbitrary_sha".into(),
                lib_name: None,
                lib_path: None,
                build_path: None,
                proc_macro: false,
                dependencies: Default::default(),
                build_dependencies: Default::default(),
                features: Default::default(),
                edition: "2018".to_string(),
                printed: false,
            }
            .into()],
            features: Default::default(),
            edition: "2021".to_string(),
            printed: false,
        };

        let actual = package.into_derivative();

        assert_eq!(
            actual,
            r#"{ pkgs ? import <nixpkgs> {
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

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /cargo-nbuild/nbuild-core/tests/simple; };

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
"#
        );
    }

    #[test]
    fn workspace() {
        let base = PathBuf::from_str("/cargo-nbuild/nbuild-core/tests/workspace").unwrap();

        let libc = RefCell::new(Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "sha".into(),
            lib_name: None,
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: Default::default(),
            edition: "2015".to_string(),
            printed: false,
        })
        .into();

        let package = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: base.join("parent").into(),
            lib_name: None,
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: base.join("child").into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: vec![
                        Package {
                            name: "fnv".to_string(),
                            version: "1.0.7".parse().unwrap(),
                            source: "sha".into(),
                            lib_name: None,
                            lib_path: Some("lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2015".to_string(),
                            printed: false,
                        }
                        .into(),
                        Package {
                            name: "itoa".to_string(),
                            version: "1.0.6".parse().unwrap(),
                            source: "sha".into(),
                            lib_name: None,
                            lib_path: None,
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2018".to_string(),
                            printed: false,
                        }
                        .into(),
                        Dependency {
                            package: Rc::clone(&libc),
                            rename: None,
                        },
                        Dependency {
                            package: RefCell::new(Package {
                                name: "rename".to_string(),
                                version: "0.1.0".parse().unwrap(),
                                source: base.join("rename").into(),
                                lib_name: Some("lib_rename".to_string()),
                                lib_path: None,
                                build_path: None,
                                proc_macro: false,
                                dependencies: Default::default(),
                                build_dependencies: Default::default(),
                                features: Default::default(),
                                edition: "2021".to_string(),
                                printed: false,
                            })
                            .into(),
                            rename: Some("new_name".to_string()),
                        },
                        Package {
                            name: "rustversion".to_string(),
                            version: "1.0.12".parse().unwrap(),
                            source: "sha".into(),
                            lib_name: None,
                            lib_path: None,
                            build_path: Some("build/build.rs".into()),
                            proc_macro: true,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2018".to_string(),
                            printed: false,
                        }
                        .into(),
                    ],
                    build_dependencies: vec![Package {
                        name: "arbitrary".to_string(),
                        version: "1.3.0".parse().unwrap(),
                        source: "sha".into(),
                        lib_name: None,
                        lib_path: None,
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: Default::default(),
                        edition: "2018".to_string(),
                        printed: false,
                    }
                    .into()],
                    features: vec!["one".to_string()],
                    edition: "2021".to_string(),
                    printed: false,
                }
                .into(),
                Package {
                    name: "itoa".to_string(),
                    version: "0.4.8".parse().unwrap(),
                    source: "sha".into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: Default::default(),
                    build_dependencies: Default::default(),
                    features: Default::default(),
                    edition: "2018".to_string(),
                    printed: false,
                }
                .into(),
                Dependency {
                    package: libc,
                    rename: None,
                },
                Package {
                    name: "targets".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: base.join("targets").into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: Default::default(),
                    build_dependencies: Default::default(),
                    features: vec!["unix".to_string()],
                    edition: "2021".to_string(),
                    printed: false,
                }
                .into(),
            ],
            build_dependencies: Default::default(),
            features: Default::default(),
            edition: "2021".to_string(),
            printed: false,
        };

        let actual = package.into_derivative();

        assert_eq!(
            actual,
            r#"{ pkgs ? import <nixpkgs> {
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

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /cargo-nbuild/nbuild-core/tests/workspace/parent; };

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

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /cargo-nbuild/nbuild-core/tests/workspace/child; };
    dependencies = [fnv_1_0_7 itoa_1_0_6 libc_0_2_144 rename_0_1_0 rustversion_1_0_12];
    buildDependencies = [arbitrary_1_3_0];
    crateRenames = {"rename" = [{ rename = "new_name"; version = "0.1.0"; }];};
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
    libName = "lib_rename";
    version = "0.1.0";

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /cargo-nbuild/nbuild-core/tests/workspace/rename; };
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

    src = pkgs.lib.cleanSourceWith { filter = sourceFilter;  src = /cargo-nbuild/nbuild-core/tests/workspace/targets; };
    features = ["unix"];
    edition = "2021";
    crateBin = [];
    codegenUnits = 16;
    extraRustcOpts = [ "-C embed-bitcode=no" ];
    inherit preBuild;
  };
in
parent
"#
        );
    }
}
