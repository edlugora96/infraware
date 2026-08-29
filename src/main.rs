//! Thin binary entry point for `infraware`.
//!
//! Per the frozen contract (design D5, spec: Single-package Rust layout), this
//! file only forwards argv and the standard streams to the library adapter. All
//! behavior — parsing, dispatch, stream routing, and exit-status mapping — lives
//! in the `infraware` library. There is no business rule, file logic, or
//! repository-scanner logic here.

fn main() -> ExitCode {
    infraware::main_entry()
}

use std::process::ExitCode;
