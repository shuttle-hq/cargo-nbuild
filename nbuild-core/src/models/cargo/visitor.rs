use tracing::{info_span, trace};

use super::{Dependency, Package};

/// A visitor over cargo packages
pub trait Visitor {
    /// Entry point for a visitor. Defaults to visiting all dependencies which are not optional.
    fn visit(&mut self, package: &mut Package)
    where
        Self: Sized,
    {
        self.visit_package(package);

        for dependency in package.dependencies_iter() {
            let dependency_span = info_span!(
                "processing dependency",
                name = dependency.name,
                package_name = dependency.package.borrow().name,
                optional = dependency.optional,
            );
            let _dependency_span_guard = dependency_span.enter();

            if !dependency.optional {
                self.visit_dependency(dependency);

                dependency.package.borrow_mut().visit(self);
            }
        }
    }

    /// Visit a package
    fn visit_package(&mut self, _package: &mut Package) {}

    /// Visit a dependency of a package
    fn visit_dependency(&mut self, _dependency: &Dependency) {}
}

/// Visitor to resolve the enabled dependencies and the features on those dependencies
pub struct ResolveVisitor;

impl Visitor for ResolveVisitor {
    fn visit_dependency(&mut self, dependency: &Dependency) {
        add_default(dependency);
        activate_features(dependency);
    }

    fn visit_package(&mut self, package: &mut Package) {
        loop {
            let new_features = unpack_features(package);

            if !new_features.is_empty() {
                trace!(?new_features, "adding new features");

                package.enabled_features.extend(new_features);
            } else {
                break;
            }
        }

        unpack_optionals_features(package);
    }
}

/// Add the "default" feature if default-features is not false
/// https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#choosing-features
fn add_default(dependency: &Dependency) {
    if dependency.uses_default_features
        && dependency.package.borrow().features.contains_key("default")
    {
        trace!("enabling default feature");

        dependency
            .package
            .borrow_mut()
            .enabled_features
            .insert("default".to_string());
    }
}

/// Activate all the feature on a dependency
fn activate_features(dependency: &Dependency) {
    if !dependency.features.is_empty() {
        let features: Vec<String> = dependency
            .features
            .clone()
            .iter()
            .filter(|&f| dependency.package.borrow().features.contains_key(f))
            .cloned()
            .collect();

        trace!(?features, "enabling features");

        dependency
            .package
            .borrow_mut()
            .enabled_features
            .extend(features);
    }
}

/// Get new features on a crate's "chain" that have not been seen before
fn unpack_features(package: &mut Package) -> Vec<String> {
    package
        .enabled_features
        .iter()
        .filter_map(|f| package.features.get(f))
        .flatten()
        .cloned()
        .filter(|f| !package.enabled_features.contains(f)) // Don't process a "leaf" feature
        .filter_map(|f| {
            // Activate an optional dependency that is turned on by a feature
            // https://doc.rust-lang.org/cargo/reference/features.html#optional-dependencies
            if let Some(dependency_name) = f.strip_prefix("dep:") {
                if let Some(dependency) = package
                    .dependencies
                    .iter_mut()
                    .chain(package.build_dependencies.iter_mut())
                    .find(|d| d.name == dependency_name)
                {
                    trace!(name = dependency_name, "activating optional dependency");
                    dependency.optional = false;
                }

                // We are activating an optional dependency and not enabling a new feature
                return None;
            } else {
                // Activate a dependency's features
                // https://doc.rust-lang.org/cargo/reference/features.html#dependency-features
                if let Some((dependency_name, feature)) = f.split_once('/') {
                    if let Some(dependency) = package
                        .dependencies
                        .iter_mut()
                        .chain(package.build_dependencies.iter_mut())
                        .find(|d| d.name == dependency_name)
                    {
                        let feature = feature.to_string();

                        if !dependency.features.contains(&feature) {
                            dependency.features.push(feature);
                        }

                        return Some(dependency_name.to_string());
                    }
                }
            }

            Some(f)
        })
        .filter(|f| !package.enabled_features.contains(f)) // We only want to unpack new features
        .collect()
}

/// Activate features on optional dependencies where the dependencies was made non-optional by a previous feature
/// https://doc.rust-lang.org/cargo/reference/features.html#dependency-features
fn unpack_optionals_features(package: &mut Package) {
    let new_dependencies_features: Vec<_> = package
        .enabled_features
        .iter()
        .filter_map(|f| f.split_once("?/"))
        .map(|(d, f)| (d.to_string(), f.to_string()))
        .collect();

    for (dependency_name, feature) in new_dependencies_features {
        if let Some(dependency) = package
            .dependencies_iter_mut()
            .find(|d| d.name == dependency_name && !d.optional)
        {
            if !dependency.features.contains(&feature) {
                dependency.features.push(feature.clone());
            }

            trace!(
                dependency = dependency_name,
                feature,
                "adding feature on optional dependency"
            );
        }

        package
            .enabled_features
            .remove(&format!("{dependency_name}?/{feature}"));
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use crate::models::cargo::{Dependency, Package};

    use pretty_assertions::assert_eq;

    fn make_package_node(
        name: &str,
        features: Vec<(&str, Vec<&str>)>,
        dependency: Option<Dependency>,
    ) -> Package {
        let dependencies = if let Some(dependency) = dependency {
            vec![dependency]
        } else {
            Default::default()
        };

        Package {
            name: name.to_string(),
            lib_name: None,
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
    fn no_defaults() {
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
            Some(Dependency {
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
            Some(Dependency {
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
    fn defaults() {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child.enabled_features.extend([
            "one".to_string(),
            "two".to_string(),
            "default".to_string(),
        ]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
    fn defaults_chain() {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child.enabled_features.extend([
            "one".to_string(),
            "two".to_string(),
            "default".to_string(),
        ]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
    fn optional_no_defaults() {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        input.build_dependencies.push(Dependency {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        expected.build_dependencies.push(Dependency {
            name: "build".to_string(),
            package: RefCell::new(build).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        assert_eq!(input, expected);
    }

    // Optionals should not enable any features since they will not be used
    #[test]
    fn optional_features() {
        let child = make_package_node("child", vec![("one", vec![]), ("two", vec![])], None);
        let build = make_package_node("build", vec![("hi", vec![])], None);

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(Dependency {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(Dependency {
            name: "build".to_string(),
            package: RefCell::new(build).into(),
            optional: true,
            uses_default_features: true,
            features: vec!["hi".to_string()],
        });

        assert_eq!(input, expected);
    }

    // Enable everything on a chain
    #[test]
    fn chain() {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(Dependency {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(Dependency {
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
    fn feature_dependency() {
        let mut optional = make_package_node("optional", vec![("feature", vec![])], None);
        let mut optional_build =
            make_package_node("optional", vec![("build_feature", vec![])], None);

        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(Dependency {
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
            Some(Dependency {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        input.build_dependencies.push(Dependency {
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
            Some(Dependency {
                name: "child".to_string(),
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );
        expected.build_dependencies.push(Dependency {
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
    fn feature_renamed_dependency() {
        let rename = make_package_node("rename", vec![], None);
        let build_rename = make_package_node("build_rename", vec![], None);
        let mut child = make_package_node(
            "child",
            vec![
                ("new_name", vec!["dep:new_name"]),
                ("new_build_name", vec!["dep:new_build_name"]),
            ],
            Some(Dependency {
                name: "new_name".to_string(),
                package: RefCell::new(rename.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(Dependency {
            name: "new_build_name".to_string(),
            package: RefCell::new(build_rename.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
    fn feature_dependency_features() {
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
            Some(Dependency {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(Dependency {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
    fn feature_dependency_defaults() {
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
            Some(Dependency {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );
        child.build_dependencies.push(Dependency {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: true,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
            .extend(["std".to_string(), "default".to_string()]);
        child.build_dependencies[0].optional = false;
        child.build_dependencies[0].package = RefCell::new(build_optional).into();
        child.build_dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["build".to_string(), "default".to_string()]);
        child.enabled_features.extend([
            "one".to_string(),
            "optional".to_string(),
            "build_optional".to_string(),
        ]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
    fn feature_dependency_no_defaults() {
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
            Some(Dependency {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: false,
                features: vec![],
            }),
        );
        child.build_dependencies.push(Dependency {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: false,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
    fn feature_on_optional_dependency() {
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
            Some(Dependency {
                name: "optional".to_string(),
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: false,
                features: vec![],
            }),
        );
        child.build_dependencies.push(Dependency {
            name: "build_optional".to_string(),
            package: RefCell::new(build_optional.clone()).into(),
            optional: true,
            uses_default_features: false,
            features: vec![],
        });

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
    fn no_default_correctly() {
        let mut child = make_package_node(
            "child",
            vec![("default", vec!["std"]), ("other", vec!["who"])],
            None,
        );
        let child_rc = RefCell::new(child.clone()).into();

        let layer1_1 = make_package_node(
            "layer1_1",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
            Some(Dependency {
                name: "layer1_1".to_string(),
                package: RefCell::new(layer1_1).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );
        input.dependencies.push(Dependency {
            name: "layer1_2".to_string(),
            package: RefCell::new(layer1_2).into(),
            optional: false,
            uses_default_features: true,
            features: vec![],
        });

        input.resolve();

        child.enabled_features.extend([
            "std".to_string(),
            "default".to_string(),
            "other".to_string(),
            "who".to_string(),
        ]);

        let child_rc = RefCell::new(child).into();

        let layer1_1 = make_package_node(
            "layer1_1",
            vec![],
            Some(Dependency {
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
            Some(Dependency {
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
            Some(Dependency {
                name: "layer1_1".to_string(),
                package: RefCell::new(layer1_1).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );
        expected.dependencies.push(Dependency {
            name: "layer1_2".to_string(),
            package: RefCell::new(layer1_2).into(),
            optional: false,
            uses_default_features: true,
            features: vec![],
        });

        assert_eq!(input, expected);
    }
}
