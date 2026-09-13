mod cli;
mod config;
mod manifest;
mod model;
mod paths;
mod process;
mod tarball;

fn main() -> anyhow::Result<()> {
    cli::run()
}
