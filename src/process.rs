use std::process::Command;

pub fn run(command: &mut Command, description: &str) -> anyhow::Result<()> {
    let status = command.status()?;
    if !status.success() {
        anyhow::bail!("{description} failed with status {status}");
    }
    Ok(())
}
