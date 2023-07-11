use std::{env::current_dir, error::Error, process::Stdio};

use clap::builder::PossibleValue;
use clap::*;
use nbuild_core::models::{cargo, nix};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about,
    arg(clap::Arg::new("dummy")
        .value_parser([PossibleValue::new("nbuild")])
        .required(false)
        .hide(true))
)]
struct Args {
    #[arg(long, default_value = None)]
    package: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let fmt_layer = tracing_subscriber::fmt::layer().pretty().with_ansi(false);
    let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    let args = Args::parse();
    let mut package = cargo::Package::from_current_dir(current_dir()?, args.package)?;
    package.resolve();

    let package: nix::Package = package.into();
    package.into_file()?;

    let mut cmd = Command::new("nix");
    cmd.args([
        "build",
        "--file",
        ".nbuild.nix",
        "--max-jobs",
        "auto",
        "--cores",
        "0",
    ])
    .stdout(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("to get handle on stdout");

    let mut reader = BufReader::new(stdout).lines();

    // Drive process forward
    tokio::spawn(async move {
        let status = child.wait().await.expect("build to finish");

        if status.success() {
            println!("Build done");
        } else {
            println!("Build failed");
        }
    });

    while let Some(line) = reader.next_line().await.expect("to get line") {
        println!("{line}");
    }

    Ok(())
}
