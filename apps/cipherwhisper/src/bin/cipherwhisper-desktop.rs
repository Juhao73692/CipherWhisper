// The Windows package ships this entry point as cipherwhisper.exe, and retains
// the console entry point as cipherwhisper-cli.exe for interactive CLI commands.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
include!("../main.rs");
