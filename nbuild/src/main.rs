use std::env::current_dir;

use nbuild_core::PackageNode;
use runix::{
    arguments::{eval::EvaluationArgs, source::SourceArgs, NixArgs},
    command::Build,
    command_line::NixCommandLine,
    RunJson,
};

#[tokio::main]
async fn main() {
    let mut package = PackageNode::from_current_dir(current_dir().unwrap());
    package.resolve();

    let expr = package.into_package().to_derivative();
    let cli = NixCommandLine::default();

    let value = Build {
        eval: EvaluationArgs {
            impure: true.into(),
        },
        source: SourceArgs {
            expr: Some(expr.into()),
        },
        ..Default::default()
    }
    .run_json(&cli, &NixArgs::default())
    .await
    .unwrap();

    println!("{value}");
}
