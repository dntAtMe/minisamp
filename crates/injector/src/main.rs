use anyhow::{Context, Result};
use dll_syringe::{process::OwnedProcess, Syringe};
use std::path::PathBuf;

fn main() -> Result<()> {
    let process_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "gta_sa.exe".to_string());

    let dll_path: PathBuf = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut p = std::env::current_exe().expect("cannot resolve exe path");
            p.pop();
            p.push("client_dll.dll");
            p
        });

    println!("Looking for process: {process_name}");
    let target =
        OwnedProcess::find_first_by_name(&process_name).context("Target process not found")?;

    println!("Found process. Injecting {}", dll_path.display());
    let syringe = Syringe::for_process(target);
    syringe
        .inject(dll_path)
        .context("Failed to inject DLL")?;

    println!("DLL injected successfully!");
    Ok(())
}
