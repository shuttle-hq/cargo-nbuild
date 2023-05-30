use std::env::current_dir;

use nbuild_core::PackageNode;
use runix::{
    arguments::{eval::EvaluationArgs, source::SourceArgs, NixArgs},
    command::Build,
    command_line::NixCommandLine,
    RunJson,
};
use tracing::debug;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() {
    let fmt_layer = tracing_subscriber::fmt::layer().pretty().with_ansi(false);
    let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    let mut package = PackageNode::from_current_dir(current_dir().unwrap());
    package.resolve();

    let expr = package.into_package().to_derivative();
    let cli = NixCommandLine::default();

    debug!(expr, "have nix expression");

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
    .await;

    match value {
        Ok(value) => println!("{value}"),
        Err(error) => println!("failed: {error}"),
    }
}
