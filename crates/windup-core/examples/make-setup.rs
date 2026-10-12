//! Build the setup executable from `windup.exe` and the MSI.
//!
//! ```sh
//! cargo run --release -p windup-core --example make-setup -- \
//!     target/release/windup.exe dist/windup-0.1.0-x64.msi dist/windup-setup-0.1.0-x64.exe
//! ```
//!
//! A Rust example rather than a script so that it calls the same
//! `install::payload::append` the running binary reads with. The output name
//! matters: setup reads its version out of its own file name, so
//! `windup-setup-0.1.0-x64.exe` is what puts "Install Windup 0.1.0" in the
//! window.

use std::process::ExitCode;
use windup_core::install::payload;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [executable, package, out] = args.as_slice() else {
        eprintln!("usage: make-setup <windup.exe> <package.msi> <out.exe>");
        return ExitCode::FAILURE;
    };

    let read =
        |path: &str| std::fs::read(path).map_err(|e| eprintln!("could not read {path}: {e}"));
    let (Ok(exe), Ok(msi)) = (read(executable), read(package)) else {
        return ExitCode::FAILURE;
    };

    let setup = payload::append(&exe, &msi);
    if payload::read(&setup) != Some(msi.as_slice()) {
        eprintln!("the package did not read back out of the setup binary");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(out, &setup) {
        eprintln!("could not write {out}: {e}");
        return ExitCode::FAILURE;
    }

    // Ask the written file the question the running binary asks of itself on a
    // double-click. A file that fails it would open Windup instead of
    // installing it.
    if !payload::carried_by(std::path::Path::new(out)) {
        eprintln!("{out} was written but is not recognised as an installer");
        return ExitCode::FAILURE;
    }
    println!(
        "{out}: {} bytes ({} of program, {} of package)",
        setup.len(),
        exe.len(),
        msi.len()
    );
    ExitCode::SUCCESS
}
