use std::process::ExitCode;

use crate::cli::{HelperArgs, RunArgs};
use crate::error::Result;

#[cfg(not(target_os = "linux"))]
use crate::error::BoxRunError;

#[cfg(target_os = "linux")]
pub fn run(args: RunArgs) -> Result<ExitCode> {
    let policy = crate::policy::Policy::from_args(args)?;
    crate::linux::run(&policy)
}

#[cfg(target_os = "linux")]
pub fn doctor() -> Result<ExitCode> {
    crate::linux::doctor()
}

#[cfg(target_os = "linux")]
pub fn run_helper(args: HelperArgs) -> Result<ExitCode> {
    crate::linux::run_helper(args)
}

#[cfg(target_os = "macos")]
pub fn run(_args: RunArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "macOS backend is not implemented yet; the current release fully supports Linux only"
            .to_owned(),
    ))
}

#[cfg(target_os = "macos")]
pub fn doctor() -> Result<ExitCode> {
    println!("platform: macos");
    println!("backend: unavailable");
    println!("status: Linux is fully supported; macOS backend is planned but not implemented");
    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "macos")]
pub fn run_helper(_args: HelperArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "sandbox helper is only available on Linux".to_owned(),
    ))
}

#[cfg(target_os = "windows")]
pub fn run(_args: RunArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "Windows backend is not implemented yet; the current release fully supports Linux only"
            .to_owned(),
    ))
}

#[cfg(target_os = "windows")]
pub fn doctor() -> Result<ExitCode> {
    println!("platform: windows");
    println!("backend: unavailable");
    println!("status: Linux is fully supported; Windows backend is planned but not implemented");
    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "windows")]
pub fn run_helper(_args: HelperArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "sandbox helper is only available on Linux".to_owned(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn run(_args: RunArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "this platform is not supported yet; the current release fully supports Linux only"
            .to_owned(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn doctor() -> Result<ExitCode> {
    println!("platform: unsupported");
    println!("backend: unavailable");
    println!("status: Linux is fully supported; this target does not have a backend yet");
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn run_helper(_args: HelperArgs) -> Result<ExitCode> {
    Err(BoxRunError::Message(
        "sandbox helper is only available on Linux".to_owned(),
    ))
}
