mod visitor;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use cargo_lock::Lockfile;
use cargo_metadata::{
    camino::Utf8PathBuf, semver::Version, DependencyKind, MetadataCommand, PackageId,
};
use target_spec::{Platform, TargetSpec};
use tracing::{instrument, trace};
use visitor::{
    EnableFeaturesVisitor, OptionalDependencyFeaturesVisitor, SetDefaultVisitor,
    UnpackChainVisitor, UnpackDefaultVisitor, Visitor,
};

#[derive(Debug, PartialEq)]
pub struct Package {
    name: String,
    version: Version,
    source: Source,
    lib_path: Option<Utf8PathBuf>,
    build_path: Option<Utf8PathBuf>,
    proc_macro: bool,
    features: Vec<String>,
    dependencies: Vec<Dependency>,
    build_dependencies: Vec<Dependency>,
    edition: String,
    printed: bool,
}

#[derive(Debug, PartialEq)]
pub struct Dependency {
    package: Rc<RefCell<Package>>,
    rename: Option<String>,
}

impl Package {
    pub fn to_derivative(self) -> String {
        let Self {
            name,
            version,
            source,
            lib_path: _,
            build_path: _,
            proc_macro: _,
            features: _,
            dependencies,
            build_dependencies,
            edition,
            printed: _,
        } = self;

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
  overlays = [
    (import (builtins.fetchTarball https://github.com/mozilla/nixpkgs-mozilla/archive/master.tar.gz))
  ];
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
  rustc = ((pkgs.rustChannelOf{{ channel = "1.68.0"; }}).rust.override {{
    extensions = ["rust-src"];
  }});
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
    inherit rustc defaultCrateOverrides fetchCrate;
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
                        renames.push((d.package.borrow().name.clone(), rename.clone()));
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
                        renames.push((d.package.borrow().name.clone(), rename.clone()));
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
                .map(|(name, rename)| format!("\"{name}\" = \"{rename}\";"))
                .collect::<Vec<_>>()
                .join(" ");

            format!("\n    crateRenames = {{{renames}}};")
        };

        let details = format!(
            r#"  {} = buildRustCrate rec {{
    crateName = "{}";
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

    fn identifier(&self) -> String {
        format!(
            "{}_{}",
            self.name,
            self.version.to_string().replace('.', "_").replace('+', "_")
        )
    }

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

#[derive(Debug, PartialEq, Clone)]
pub struct PackageNode {
    name: String,
    version: Version,
    source: Source,
    lib_path: Option<Utf8PathBuf>,
    build_path: Option<Utf8PathBuf>,
    proc_macro: bool,
    features: HashMap<String, Vec<String>>,
    enabled_features: HashSet<String>,
    dependencies: Vec<DependencyNode>,
    build_dependencies: Vec<DependencyNode>,
    edition: String,
}

#[derive(Debug, PartialEq, Clone)]
pub struct DependencyNode {
    name: String,
    package: Rc<RefCell<PackageNode>>,
    optional: bool,
    uses_default_features: bool,
    features: Vec<String>,
}

#[derive(Debug, PartialEq, Clone)]
enum Source {
    Local(PathBuf),
    CratesIo(String),
}

impl PackageNode {
    pub fn from_current_dir(path: impl Into<PathBuf>) -> Self {
        let platform = Platform::current().unwrap();

        let metadata = MetadataCommand::new()
            .current_dir(path)
            .other_options(vec![
                "--filter-platform".to_string(),
                platform.triple_str().to_string(),
            ])
            .exec()
            .unwrap();
        let lock_file = metadata.workspace_root.join("Cargo.lock");
        let lock_file = Lockfile::load(lock_file).unwrap();

        trace!(?platform, ?metadata, ?lock_file, "have metadata");

        let packages =
            BTreeMap::from_iter(metadata.packages.iter().map(|p| (p.id.clone(), p.clone())));
        let nodes = BTreeMap::from_iter(
            metadata
                .resolve
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .map(|n| (n.id.clone(), n.clone())),
        );
        let checksums = BTreeMap::from_iter(lock_file.packages.iter().filter_map(|p| {
            if let Some(checksum) = &p.checksum {
                Some((
                    (p.name.to_string(), p.version.to_string()),
                    checksum.to_string(),
                ))
            } else {
                None
            }
        }));

        let root_id = metadata
            .resolve
            .as_ref()
            .unwrap()
            .root
            .as_ref()
            .unwrap()
            .clone();

        let mut resolved_packages = Default::default();

        Self::get_package(
            root_id,
            &packages,
            &nodes,
            &checksums,
            &mut resolved_packages,
            &platform,
        )
    }

    fn get_package(
        id: PackageId,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        checksums: &BTreeMap<(String, String), String>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<PackageNode>>>,
        platform: &Platform,
    ) -> Self {
        let node = nodes.get(&id).unwrap().clone();
        let package = packages.get(&id).unwrap();

        trace!(
            package.name,
            ?package.features,
            ?node,
            "found package and node"
        );

        let features = package.features.clone();
        let package_dependencies = package
            .dependencies
            .iter()
            .filter(|d| d.kind == DependencyKind::Normal)
            .cloned()
            .collect();
        let package_build_dependencies = package
            .dependencies
            .iter()
            .filter(|d| d.kind == DependencyKind::Build)
            .cloned()
            .collect();

        let dependencies = node
            .dependencies
            .iter()
            .filter_map(|id| {
                DependencyNode::get_dependency(
                    id,
                    &package_dependencies,
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                )
            })
            .collect();
        let build_dependencies = node
            .dependencies
            .iter()
            .filter_map(|id| {
                DependencyNode::get_dependency(
                    id,
                    &package_build_dependencies,
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                )
            })
            .collect();

        let package_path: PathBuf = package.manifest_path.parent().unwrap().into();

        let lib_path = package
            .targets
            .iter()
            .find(|t| {
                t.kind.iter().any(|k| {
                    matches!(
                        k.as_str(),
                        "lib" | "cdylib" | "dylib" | "rlib" | "proc-macro"
                    )
                })
            })
            .map(|t| {
                t.src_path
                    .strip_prefix(&package_path)
                    .unwrap()
                    .to_path_buf()
            });
        let build_path = package
            .targets
            .iter()
            .find(|t| t.kind.iter().any(|k| k == "custom-build"))
            .map(|t| {
                t.src_path
                    .strip_prefix(&package_path)
                    .unwrap()
                    .to_path_buf()
            });
        let proc_macro = package
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "proc-macro"));

        let source = if package.source.is_some() {
            let checksum = checksums
                .get(&(package.name.to_string(), package.version.to_string()))
                .expect("to have a checksum");
            Source::CratesIo(checksum.to_string())
        } else {
            Source::Local(package_path)
        };

        Self {
            name: package.name.clone(),
            version: package.version.clone(),
            source,
            lib_path,
            build_path,
            proc_macro,
            dependencies,
            build_dependencies,
            features,
            enabled_features: Default::default(),
            edition: package.edition.to_string(),
        }
    }

    pub fn into_package(self) -> Package {
        let mut converted = Default::default();

        let result = Self::convert_to_package(self, &mut converted);

        // Drop what was converted so that we can unwrap from the Rc
        drop(converted);

        Rc::try_unwrap(result).unwrap().into_inner()
    }

    #[instrument(skip_all, fields(name = %self.name))]
    fn convert_to_package(
        self,
        converted: &mut BTreeMap<(String, Version), Rc<RefCell<Package>>>,
    ) -> Rc<RefCell<Package>> {
        let Self {
            name,
            version,
            source,
            lib_path,
            build_path,
            proc_macro,
            features: _,
            enabled_features,
            dependencies,
            build_dependencies,
            edition,
        } = self;

        match converted.get(&(name.clone(), version.clone())) {
            Some(package) => Rc::clone(package),
            None => {
                let dependencies = dependencies
                    .iter()
                    .filter(|d| !d.optional)
                    .map(|d| {
                        let package = Rc::clone(&d.package)
                            .borrow()
                            .clone()
                            .convert_to_package(converted);

                        let rename = if d.name == package.borrow().name {
                            None
                        } else {
                            trace!(dependency_name = d.name, "activating rename");

                            Some(d.name.to_string())
                        };

                        Dependency { package, rename }
                    })
                    .collect();
                let build_dependencies = build_dependencies
                    .iter()
                    .filter(|d| !d.optional)
                    .map(|d| {
                        let package = Rc::clone(&d.package)
                            .borrow()
                            .clone()
                            .convert_to_package(converted);

                        let rename = if d.name == package.borrow().name {
                            None
                        } else {
                            trace!(dependency_name = d.name, "activating rename");

                            Some(d.name.to_string())
                        };

                        Dependency { package, rename }
                    })
                    .collect();

                let lib_path =
                    lib_path.and_then(|p| if p == "src/lib.rs" { None } else { Some(p) });
                let build_path =
                    build_path.and_then(|p| if p == "build.rs" { None } else { Some(p) });

                let package = RefCell::new(Package {
                    name: name.clone(),
                    version: version.clone(),
                    source,
                    lib_path,
                    build_path,
                    proc_macro,
                    features: enabled_features.into_iter().collect(),
                    dependencies,
                    build_dependencies,
                    edition,
                    printed: false,
                })
                .into();

                converted.insert((name, version), Rc::clone(&package));

                package
            }
        }
    }

    pub fn resolve(&mut self) {
        self.visit(&mut SetDefaultVisitor);
        self.visit(&mut EnableFeaturesVisitor);
        self.visit(&mut UnpackDefaultVisitor);
        self.visit(&mut UnpackChainVisitor);
        self.visit(&mut OptionalDependencyFeaturesVisitor);
    }

    fn visit(&mut self, visitor: &mut impl Visitor) {
        visitor.visit(self);
    }

    pub fn dependencies_iter(&self) -> impl Iterator<Item = &DependencyNode> {
        self.dependencies
            .iter()
            .chain(self.build_dependencies.iter())
    }

    pub fn dependencies_iter_mut(&mut self) -> impl Iterator<Item = &mut DependencyNode> {
        self.dependencies
            .iter_mut()
            .chain(self.build_dependencies.iter_mut())
    }
}

impl DependencyNode {
    #[instrument(skip_all, fields(%id))]
    fn get_dependency(
        id: &PackageId,
        parent_dependencies: &Vec<cargo_metadata::Dependency>,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        checksums: &BTreeMap<(String, String), String>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<PackageNode>>>,
        platform: &Platform,
    ) -> Option<Self> {
        // We eventually want only one representation of a package when we do feature resolution
        let package = match resolved_packages.get(id) {
            Some(package) => Rc::clone(package),
            None => {
                let package = RefCell::new(PackageNode::get_package(
                    id.clone(),
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                ))
                .into();

                resolved_packages.insert(id.clone(), Rc::clone(&package));

                package
            }
        };

        let name = package.borrow().name.clone();

        let dependencies: Vec<_> = parent_dependencies
            .iter()
            .filter(|d| d.name == name)
            .filter(|d| match &d.target {
                Some(target_spec) => {
                    let target_spec = TargetSpec::new(target_spec.to_string()).unwrap();

                    target_spec.eval(platform).unwrap_or(false)
                }
                None => true,
            })
            .collect();

        // It could happen that this kind of dependency is not part of the kind passed into this function,
        // in which case this dependency should not we considered as a real dependency.
        if dependencies.is_empty() {
            return None;
        }

        let mut optional = true;
        let mut uses_default_features = false;
        let mut features: Vec<String> = Default::default();
        let mut dependency_name: String = Default::default();
        let mut dependency_rename = None;

        for dependency in dependencies {
            if !dependency.optional {
                optional = false;
            }

            if dependency.uses_default_features {
                uses_default_features = true;
            }

            features.extend(dependency.features.iter().cloned());

            if dependency_rename.is_none() && dependency.rename.is_some() {
                dependency_rename = dependency.rename.clone();
            }

            if dependency_name.is_empty() {
                dependency_name = dependency.name.clone();
            }
        }

        if let Some(dependency_rename) = dependency_rename {
            dependency_name = dependency_rename;
        };

        trace!(
            name,
            dependency_name,
            optional,
            uses_default_features,
            ?features,
            "done with dependency"
        );

        Some(Self {
            name: dependency_name,
            package,
            optional,
            uses_default_features,
            features,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, str::FromStr};

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
    fn simple_package_input() {
        let path = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("simple");

        let package = PackageNode::from_current_dir(path.clone());

        assert_eq!(
            package,
            PackageNode {
                name: "simple".to_string(),
                source: path.into(),
                lib_path: None,
                build_path: None,
                proc_macro: false,
                version: "0.1.0".parse().unwrap(),
                dependencies: vec![DependencyNode {
                    name: "itoa".to_string(),
                    package: RefCell::new(PackageNode {
                        name: "itoa".to_string(),
                        version: "1.0.6".parse().unwrap(),
                        source: "453ad9f582a441959e5f0d088b02ce04cfe8d51a8eaf077f12ac6d3e94164ca6"
                            .into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([(
                            "no-panic".to_string(),
                            vec!["dep:no-panic".to_string()]
                        )]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },],
                build_dependencies: vec![DependencyNode {
                    name: "arbitrary".to_string(),
                    package: RefCell::new(PackageNode {
                        name: "arbitrary".to_string(),
                        version: "1.3.0".parse().unwrap(),
                        source: "e2d098ff73c1ca148721f37baad5ea6a465a13f9573aba8641fbbbae8164a54e"
                            .into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("derive".to_string(), vec!["derive_arbitrary".to_string()]),
                            (
                                "derive_arbitrary".to_string(),
                                vec!["dep:derive_arbitrary".to_string()]
                            ),
                        ]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },],
                features: Default::default(),
                enabled_features: Default::default(),
                edition: "2021".to_string(),
            }
        );
    }

    #[test]
    fn simple_package_output() {
        let path = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("simple");

        let package = Package {
            name: "simple".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: path.clone().into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![Package {
                name: "itoa".to_string(),
                version: "1.0.6".parse().unwrap(),
                source: "itoa_sha".into(),
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

        let actual = package.to_derivative();

        let expected = fs::read_to_string(path.join("expected.nix")).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn workspace_input() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let package = PackageNode::from_current_dir(path.clone());

        assert_eq!(
            package,
            PackageNode {
                name: "parent".to_string(),
                version: "0.1.0".parse().unwrap(),
                source: path.into(),
                lib_path: None,
                build_path: None,
                proc_macro: false,
                dependencies: vec![
                    DependencyNode {
                        name: "child".to_string(),
                        package: RefCell::new(PackageNode {
                            name: "child".to_string(),
                            version: "0.1.0".parse().unwrap(),
                            source: workspace.join("child").into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: vec![
                                DependencyNode {
                                    name: "fnv".to_string(),
                                    package: RefCell::new(PackageNode {
                                        name: "fnv".to_string(),
                                        version: "1.0.7".parse().unwrap(),
                                        source: "3f9eec918d3f24069decb9af1554cad7c880e2da24a9afd88aca000531ab82c1".into(),
                                        lib_path: Some("lib.rs".into()),
                                        build_path: None,
                                        proc_macro: false,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: HashMap::from([
                                            ("default".to_string(), vec!["std".to_string()]),
                                            ("std".to_string(), vec![]),
                                        ]),
                                        enabled_features: Default::default(),
                                        edition: "2015".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                DependencyNode {
                                    name: "itoa".to_string(),
                                    package: RefCell::new(PackageNode {
                                        name: "itoa".to_string(),
                                        version: "1.0.6".parse().unwrap(),
                                        source: "453ad9f582a441959e5f0d088b02ce04cfe8d51a8eaf077f12ac6d3e94164ca6".into(),
                                        lib_path: Some("src/lib.rs".into()),
                                        build_path: None,
                                        proc_macro: false,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: HashMap::from([(
                                            "no-panic".to_string(),
                                            vec!["dep:no-panic".to_string()]
                                        )]),
                                        enabled_features: Default::default(),
                                        edition: "2018".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                DependencyNode {
                                    name: "libc".to_string(),
                                    package: RefCell::new(PackageNode {
                                        name: "libc".to_string(),
                                        version: "0.2.144".parse().unwrap(),
                                        source: "2b00cc1c228a6782d0f076e7b232802e0c5689d41bb5df366f2a6b6621cfdfe1".into(),
                                        lib_path: Some("src/lib.rs".into()),
                                        build_path: Some("build.rs".into()),
                                        proc_macro: false,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: HashMap::from([
                                            ("std".to_string(), vec![]),
                                            ("default".to_string(), vec!["std".to_string()]),
                                            ("use_std".to_string(), vec!["std".to_string()]),
                                            ("extra_traits".to_string(), vec![]),
                                            ("align".to_string(), vec![]),
                                            (
                                                "rustc-dep-of-std".to_string(),
                                                vec![
                                                    "align".to_string(),
                                                    "rustc-std-workspace-core".to_string()
                                                ]
                                            ),
                                            ("const-extern-fn".to_string(), vec![]),
                                            (
                                                "rustc-std-workspace-core".to_string(),
                                                vec!["dep:rustc-std-workspace-core".to_string()]
                                            ),
                                        ]),
                                        enabled_features: Default::default(),
                                        edition: "2015".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                DependencyNode {
                                    name: "new_name".to_string(),
                                    package: RefCell::new(PackageNode {
                                        name: "rename".to_string(),
                                        version: "0.1.0".parse().unwrap(),
                                        source: workspace.join("rename").into(),
                                        lib_path: Some("src/lib.rs".into()),
                                        build_path: None,
                                        proc_macro: false,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: Default::default(),
                                        enabled_features: Default::default(),
                                        edition: "2021".to_string(),
                                    })
                                    .into(),
                                    optional: true,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                DependencyNode {
                                    name: "rustversion".to_string(),
                                    package: RefCell::new(PackageNode {
                                        name: "rustversion".to_string(),
                                        version: "1.0.12".parse().unwrap(),
                                        source: "4f3208ce4d8448b3f3e7d168a73f5e0c43a61e32930de3bceeccedb388b6bf06".into(),
                                        lib_path: Some("src/lib.rs".into()),
                                        build_path: Some("build/build.rs".into()),
                                        proc_macro: true,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: Default::default(),
                                        enabled_features: Default::default(),
                                        edition: "2018".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                            ],
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                (
                                    "default".to_string(),
                                    vec!["one".to_string(), "two".to_string()]
                                ),
                                ("one".to_string(), vec!["new_name".to_string()]),
                                ("two".to_string(), vec![]),
                                ("new_name".to_string(), vec!["dep:new_name".to_string()]),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2021".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: false,
                        features: vec!["one".to_string()],
                    },
                    DependencyNode {
                        name: "itoa".to_string(),
                        package: RefCell::new(PackageNode {
                            name: "itoa".to_string(),
                            version: "0.4.8".parse().unwrap(),
                            source: "b71991ff56294aa922b450139ee08b3bfc70982c6b2c7562771375cf73542dd4".into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                ("default".to_string(), vec!["std".to_string()]),
                                ("std".to_string(), vec![]),
                                ("i128".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2015".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                    DependencyNode {
                        name: "libc".to_string(),
                        package: RefCell::new(PackageNode {
                            name: "libc".to_string(),
                            version: "0.2.144".parse().unwrap(),
                            source: "2b00cc1c228a6782d0f076e7b232802e0c5689d41bb5df366f2a6b6621cfdfe1".into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: Some("build.rs".into()),
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                ("std".to_string(), vec![]),
                                ("default".to_string(), vec!["std".to_string()]),
                                ("use_std".to_string(), vec!["std".to_string()]),
                                ("extra_traits".to_string(), vec![]),
                                ("align".to_string(), vec![]),
                                (
                                    "rustc-dep-of-std".to_string(),
                                    vec![
                                        "align".to_string(),
                                        "rustc-std-workspace-core".to_string()
                                    ]
                                ),
                                ("const-extern-fn".to_string(), vec![]),
                                (
                                    "rustc-std-workspace-core".to_string(),
                                    vec!["dep:rustc-std-workspace-core".to_string()]
                                ),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2015".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                    DependencyNode {
                        name: "targets".to_string(),
                        package: RefCell::new(PackageNode {
                            name: "targets".to_string(),
                            version: "0.1.0".parse().unwrap(),
                            source: workspace.join("targets").into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                ("unix".to_string(), vec![]),
                                ("windows".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2021".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: vec!["unix".to_string()],
                    },
                ],
                build_dependencies: Default::default(),
                features: Default::default(),
                enabled_features: Default::default(),
                edition: "2021".to_string(),
            }
        );
    }

    #[test]
    fn workspace_output() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let libc = RefCell::new(Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "sha".into(),
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
            source: path.clone().into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: workspace.join("child").into(),
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: vec![
                        Package {
                            name: "fnv".to_string(),
                            version: "1.0.7".parse().unwrap(),
                            source: "sha".into(),
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
                                source: workspace.join("rename").into(),
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
                    source: workspace.join("targets").into(),
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

        let actual = package.to_derivative();

        let expected = fs::read_to_string(path.join("expected.nix")).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn into_package() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let libc = RefCell::new(PackageNode {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "libc_sha".into(),
            lib_path: Some("src/lib.rs".into()),
            build_path: Some("build.rs".into()),
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: HashMap::from([
                ("std".to_string(), vec![]),
                ("default".to_string(), vec!["std".to_string()]),
                ("use_std".to_string(), vec!["std".to_string()]),
                ("extra_traits".to_string(), vec![]),
                ("align".to_string(), vec![]),
                (
                    "rustc-dep-of-std".to_string(),
                    vec!["align".to_string(), "rustc-std-workspace-core".to_string()],
                ),
                ("const-extern-fn".to_string(), vec![]),
                (
                    "rustc-std-workspace-core".to_string(),
                    vec!["dep:rustc-std-workspace-core".to_string()],
                ),
            ]),
            enabled_features: Default::default(),
            edition: "2015".to_string(),
        })
        .into();
        let optional = RefCell::new(PackageNode {
            name: "optional".to_string(),
            version: "1.0.0".parse().unwrap(),
            source: "optional_sha".into(),
            lib_path: Some("src/lib.rs".into()),
            build_path: None,
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: HashMap::from([
                ("std".to_string(), vec![]),
                ("default".to_string(), vec!["std".to_string()]),
            ]),
            enabled_features: Default::default(),
            edition: "2021".to_string(),
        })
        .into();

        let input = PackageNode {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: path.clone().into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                DependencyNode {
                    name: "child".to_string(),
                    package: RefCell::new(PackageNode {
                        name: "child".to_string(),
                        version: "0.1.0".parse().unwrap(),
                        source: workspace.join("child").into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: vec![
                            DependencyNode {
                                name: "fnv".to_string(),
                                package: RefCell::new(PackageNode {
                                    name: "fnv".to_string(),
                                    version: "1.0.7".parse().unwrap(),
                                    source: "fnv_sha".into(),
                                    lib_path: Some("lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: HashMap::from([
                                        ("default".to_string(), vec!["std".to_string()]),
                                        ("std".to_string(), vec![]),
                                    ]),
                                    enabled_features: Default::default(),
                                    edition: "2015".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                name: "itoa".to_string(),
                                package: RefCell::new(PackageNode {
                                    name: "itoa".to_string(),
                                    version: "1.0.6".parse().unwrap(),
                                    source: "itoa_sha".into(),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: HashMap::from([(
                                        "no-panic".to_string(),
                                        vec!["dep:no-panic".to_string()],
                                    )]),
                                    enabled_features: Default::default(),
                                    edition: "2018".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                name: "libc".to_string(),
                                package: Rc::clone(&libc),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                name: "optional".to_string(),
                                package: Rc::clone(&optional),
                                optional: true,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                name: "new_name".to_string(),
                                package: RefCell::new(PackageNode {
                                    name: "rename".to_string(),
                                    version: "0.1.0".parse().unwrap(),
                                    source: workspace.join("rename").into(),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: Default::default(),
                                    enabled_features: Default::default(),
                                    edition: "2021".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                name: "rustversion".to_string(),
                                package: RefCell::new(PackageNode {
                                    name: "rustversion".to_string(),
                                    version: "1.0.12".parse().unwrap(),
                                    source: "rustversion_sha".into(),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: Some("build/build.rs".into()),
                                    proc_macro: true,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: Default::default(),
                                    enabled_features: Default::default(),
                                    edition: "2018".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                        ],
                        build_dependencies: vec![DependencyNode {
                            name: "arbitrary".to_string(),
                            package: RefCell::new(PackageNode {
                                name: "arbitrary".to_string(),
                                version: "1.3.0".parse().unwrap(),
                                source: "arbitrary_sha".into(),
                                lib_path: Some("src/lib.rs".into()),
                                build_path: None,
                                proc_macro: false,
                                dependencies: Default::default(),
                                build_dependencies: Default::default(),
                                features: HashMap::from([
                                    ("derive".to_string(), vec!["derive_arbitrary".to_string()]),
                                    (
                                        "derive_arbitrary".to_string(),
                                        vec!["dep:derive_arbitrary".to_string()],
                                    ),
                                ]),
                                enabled_features: Default::default(),
                                edition: "2018".to_string(),
                            })
                            .into(),
                            optional: false,
                            uses_default_features: true,
                            features: Default::default(),
                        }],
                        features: HashMap::from([
                            (
                                "default".to_string(),
                                vec!["one".to_string(), "two".to_string()],
                            ),
                            ("one".to_string(), vec!["new_name".to_string()]),
                            ("two".to_string(), vec![]),
                            ("new_name".to_string(), vec!["dep:new_name".to_string()]),
                        ]),
                        enabled_features: HashSet::from([
                            "one".to_string(),
                            "new_name".to_string(),
                        ]),
                        edition: "2021".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: false,
                    features: vec!["one".to_string()],
                },
                DependencyNode {
                    name: "itoa".to_string(),
                    package: RefCell::new(PackageNode {
                        name: "itoa".to_string(),
                        version: "0.4.8".parse().unwrap(),
                        source: "itoa_sha".into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("default".to_string(), vec!["std".to_string()]),
                            ("no-panic".to_string(), vec!["dep:no-panic".to_string()]),
                            ("std".to_string(), vec![]),
                            ("i128".to_string(), vec![]),
                        ]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
                DependencyNode {
                    name: "libc".to_string(),
                    package: libc,
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
                DependencyNode {
                    name: "optional".to_string(),
                    package: optional,
                    optional: true,
                    uses_default_features: true,
                    features: Default::default(),
                },
                DependencyNode {
                    name: "targets".to_string(),
                    package: RefCell::new(PackageNode {
                        name: "targets".to_string(),
                        version: "0.1.0".parse().unwrap(),
                        source: workspace.join("targets").into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("unix".to_string(), vec![]),
                            ("windows".to_string(), vec![]),
                        ]),
                        enabled_features: HashSet::from(["unix".to_string()]),
                        edition: "2021".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: vec!["unix".to_string()],
                },
            ],
            build_dependencies: Default::default(),
            features: Default::default(),
            enabled_features: Default::default(),
            edition: "2021".to_string(),
        };

        let actual = input.into_package();

        let libc = RefCell::new(Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "libc_sha".into(),
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
        let expected = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: path.into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: workspace.join("child").into(),
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: vec![
                        Package {
                            name: "fnv".to_string(),
                            version: "1.0.7".parse().unwrap(),
                            source: "fnv_sha".into(),
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
                            source: "itoa_sha".into(),
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
                                source: workspace.join("rename").into(),
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
                            source: "rustversion_sha".into(),
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
                        source: "arbitrary_sha".into(),
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
                    features: vec!["one".to_string(), "new_name".to_string()],
                    edition: "2021".to_string(),
                    printed: false,
                }
                .into(),
                Package {
                    name: "itoa".to_string(),
                    version: "0.4.8".parse().unwrap(),
                    source: "itoa_sha".into(),
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
                    source: workspace.join("targets").into(),
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

        assert_eq!(actual, expected);

        // Make sure the libcs are linked - ie, the version for both libcs should change with this one assignment
        actual.dependencies[2].package.borrow_mut().version = "0.2.0".parse().unwrap();

        assert_eq!(
            actual.dependencies[2],
            actual.dependencies[0].package.borrow().dependencies[2]
        );
    }

    fn make_package_node(
        name: &str,
        features: Vec<(&str, Vec<&str>)>,
        dependency: Option<DependencyNode>,
    ) -> PackageNode {
        let dependencies = if let Some(dependency) = dependency {
            vec![dependency]
        } else {
            Default::default()
        };

        PackageNode {
            name: name.to_string(),
            version: "0.1.0".parse().unwrap(),
            source: "sha".into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies,
            build_dependencies: Default::default(),
            features: HashMap::from_iter(features.into_iter().map(|(b, d)| {
                (
                    b.to_string(),
                    d.into_iter().map(ToString::to_string).collect(),
                )
            })),
            enabled_features: Default::default(),
            edition: "2021".to_string(),
        }
    }

    // Defaults should not be enabled when no-defaults is used
    #[test]
    fn resolve_no_defaults() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: false,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.enabled_features.insert("one".to_string());
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: false,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Enable defaults correctly
    #[test]
    fn resolve_defaults() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string()]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        assert_eq!(input, expected);
    }

    // Enable everything on a chain of defaults
    #[test]
    fn resolve_defaults_chain() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one"]),
                ("one", vec!["two"]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string()]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        assert_eq!(input, expected);
    }

    // Optionals should not enable default features since they will not be used
    #[test]
    fn resolve_optional_no_defaults() {
        let child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );
        let build = make_package_node("build", vec![("default", vec!["hi"]), ("hi", vec![])], None);

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        input.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        input.resolve();

        let mut expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        expected.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        assert_eq!(input, expected);
    }

    // Optionals should not enable any features since they will not be used
    #[test]
    fn resolve_optional_features() {
        let child = make_package_node("child", vec![("one", vec![]), ("two", vec![])], None);
        let build = make_package_node("build", vec![("hi", vec![])], None);

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        input.resolve();

        let mut expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        assert_eq!(input, expected);
    }

    // Enable everything on a chain
    #[test]
    fn resolve_chain() {
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["two"]),
                ("two", vec!["three"]),
                ("three", vec![]),
            ],
            None,
        );
        let mut build = make_package_node(
            "build",
            vec![("hi", vec!["world"]), ("world", vec![])],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: false,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string(), "three".to_string()]);
        build
            .enabled_features
            .extend(["hi".to_string(), "world".to_string()]);

        let mut expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: false,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        assert_eq!(input, expected);
    }

    // Dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency() {
        let mut optional = make_package_node("optional", vec![("feature", vec![])], None);
        let mut optional_build =
            make_package_node("optional", vec![("build_feature", vec![])], None);

        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["feature".to_string()],
            }),
        );
        let mut build = make_package_node(
            "build",
            vec![("hi", vec!["dep:optional"])],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional_build.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["build_feature".to_string()],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: false,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        input.resolve();

        optional.enabled_features.extend(["feature".to_string()]);
        optional_build
            .enabled_features
            .extend(["build_feature".to_string()]);

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child
            .enabled_features
            .extend(["one".to_string(), "optional".to_string()]);

        build.dependencies[0].optional = false;
        build.dependencies[0].package = RefCell::new(optional_build).into();
        build.enabled_features.extend(["hi".to_string()]);

        let mut expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(DependencyNode {
            name: "build".to_string(),
            package: RefCell::new(build.clone()).into(),
            optional: false,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        assert_eq!(input, expected);
    }

    // Renamed dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_renamed_dependency() {
        let rename = make_package_node("rename", vec![], None);
        let build_rename = make_package_node("build_rename", vec![], None);
        let mut child = make_package_node(
            "child",
            vec![
                ("new_name", vec!["dep:new_name"]),
                ("new_build_name", vec!["dep:new_build_name"]),
            ],
            Some(DependencyNode {
                name: "new_name".to_string(),
                package: RefCell::new(rename.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(DependencyNode {
            name: "new_build_name".to_string(),
            package: RefCell::new(build_rename.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["new_name".to_string(), "new_build_name".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(rename).into();
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0].package = RefCell::new(build_rename).into();
        child
            .enabled_features
            .extend(["new_name".to_string(), "new_build_name".to_string()]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["new_name".to_string(), "new_build_name".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Features on dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency_features() {
        let optional = make_package_node("optional", vec![("feature", vec![])], None);
        let build_optional =
            make_package_node("build_optional", vec![("build_feature", vec![])], None);
        let mut child = make_package_node(
            "child",
            vec![
                (
                    "one",
                    vec!["optional/feature", "build_optional/build_feature"],
                ),
                ("optional", vec!["dep:optional"]),
                ("build_optional", vec!["dep:build_optional"]),
            ],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(DependencyNode {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].features.push("feature".to_string());
        child.dependencies[0].package = RefCell::new(optional).into();
        child.dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["feature".to_string()]);
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0]
            .features
            .push("build_feature".to_string());
        child.build_dependencies[0].package = RefCell::new(build_optional).into();
        child.build_dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["build_feature".to_string()]);
        child.enabled_features.extend([
            "one".to_string(),
            "optional".to_string(),
            "build_optional".to_string(),
        ]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Default dependencies chain behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency_defaults() {
        let optional = make_package_node(
            "optional",
            vec![("default", vec!["std"]), ("std", vec![])],
            None,
        );
        let build_optional = make_package_node(
            "optional",
            vec![("default", vec!["build"]), ("build", vec![])],
            None,
        );
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional", "build_optional"]),
                ("optional", vec!["dep:optional"]),
                ("build_optional", vec!["dep:build_optional"]),
            ],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(DependencyNode {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child.dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["std".to_string()]);
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0].package = RefCell::new(build_optional).into();
        child.build_dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["build".to_string()]);
        child.enabled_features.extend([
            "one".to_string(),
            "optional".to_string(),
            "build_optional".to_string(),
        ]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Default features on a dependency (with no-defaults) behind a feature should not be enabled
    #[test]
    fn resolve_feature_dependency_no_defaults() {
        let optional = make_package_node(
            "optional",
            vec![("default", vec!["std"]), ("std", vec![])],
            None,
        );
        let build_optional = make_package_node(
            "build_optional",
            vec![("default", vec!["build"]), ("build", vec![])],
            None,
        );
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional", "build_optional"]),
                ("optional", vec!["dep:optional"]),
                ("build_optional", vec!["dep:build_optional"]),
            ],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: false,
                features: vec![],
            }),
        );
        child.build_dependencies.push(DependencyNode {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: false,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0].package = RefCell::new(build_optional).into();
        child.enabled_features.extend([
            "one".to_string(),
            "optional".to_string(),
            "build_optional".to_string(),
        ]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Features on optional dependencies should be enabled if the dependency is enabled
    #[test]
    fn resolve_feature_on_optional_dependency() {
        let optional = make_package_node(
            "optional",
            vec![("disabled", vec![]), ("enabled", vec![])],
            None,
        );
        let build_optional = make_package_node(
            "build_optional",
            vec![("build_disabled", vec![]), ("build_enabled", vec![])],
            None,
        );
        let mut child = make_package_node(
            "child",
            vec![
                ("optional", vec!["dep:optional"]),
                ("build_optional", vec!["dep:build_optional"]),
                (
                    "hi",
                    vec!["optional?/enabled", "build_optional?/build_enabled"],
                ),
            ],
            Some(DependencyNode {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: false,
                features: vec![],
            }),
        );
        child.build_dependencies.push(DependencyNode {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: false,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![
                    "optional".to_string(),
                    "build_optional".to_string(),
                    "hi".to_string(),
                ],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child.dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["enabled".to_string()]);
        child.dependencies[0].features = vec!["enabled".to_string()];
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0].package = RefCell::new(build_optional).into();
        child.build_dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["build_enabled".to_string()]);
        child.build_dependencies[0].features = vec!["build_enabled".to_string()];
        child.enabled_features.extend([
            "optional".to_string(),
            "build_optional".to_string(),
            "hi".to_string(),
        ]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![
                    "optional".to_string(),
                    "build_optional".to_string(),
                    "hi".to_string(),
                ],
            }),
        );

        assert_eq!(input, expected);
    }

    // Check that a no default dependency does not removing an existing default
    //
    // Imagine a child dependency that has two other crates dependant on it. The first crate has defaults turned on,
    // and the second crate has defaults turned off. The the result of turning off the defaults should not override
    // the already on defaults.
    //
    // Here is a graphical representation
    //
    //              parent
    //              /     \
    //             /       \
    //       layer1_1     layer1_2
    //            \        /
    //      (defaults)   (no_defaults)
    //              \    /
    //              child
    #[test]
    fn resolve_no_default_correctly() {
        let mut child = make_package_node(
            "child",
            vec![("default", vec!["std"]), ("other", vec!["who"])],
            None,
        );
        let child_rc = RefCell::new(child.clone()).into();

        let layer1_1 = make_package_node(
            "layer1_1",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: Rc::clone(&child_rc),
                optional: false,
                uses_default_features: true,
                features: vec!["other".to_string()],
            }),
        );

        let layer1_2 = make_package_node(
            "layer1_2",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: Rc::clone(&child_rc),
                optional: false,
                uses_default_features: false,
                features: vec!["other".to_string()],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "layer1_1".to_string(),
                package: RefCell::new(layer1_1.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );
        input.dependencies.push(DependencyNode {
            name: "layer1_2".to_string(),
            package: RefCell::new(layer1_2).into(),
            optional: false,
            uses_default_features: true,
            features: vec![],
        });

        input.resolve();

        child
            .enabled_features
            .extend(["std".to_string(), "other".to_string(), "who".to_string()]);

        let child_rc = RefCell::new(child).into();

        let layer1_1 = make_package_node(
            "layer1_1",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: Rc::clone(&child_rc),
                optional: false,
                uses_default_features: true,
                features: vec!["other".to_string()],
            }),
        );

        let layer1_2 = make_package_node(
            "layer1_2",
            vec![],
            Some(DependencyNode {
                name: "child".to_string(),
                package: Rc::clone(&child_rc),
                optional: false,
                uses_default_features: false,
                features: vec!["other".to_string()],
            }),
        );

        let mut expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                name: "layer1_1".to_string(),
                package: RefCell::new(layer1_1.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );
        expected.dependencies.push(DependencyNode {
            name: "layer1_2".to_string(),
            package: RefCell::new(layer1_2).into(),
            optional: false,
            uses_default_features: true,
            features: vec![],
        });

        assert_eq!(input, expected);
    }
}
