//! Regression test for GitHub issue #28: the CLI must not panic (or surface
//! an `anyhow`-wrapped `Broken pipe (os error 32)`) when its stdout pipe is
//! closed on the reader's side, e.g. `esm list --type WEAP --limit 0 | head
//! -c 200`. Conventional CLI behavior (`cat`/`rg`/`jq`) is to die silently
//! via SIGPIPE. See `src/bin/cli/main.rs::reset_sigpipe_to_default` for the
//! fix under test.
//!
//! To make the repro deterministic — not a race against however fast the
//! child happens to produce output vs. how large the OS pipe buffer is — the
//! read end of the pipe is closed *before* the child is even spawned. With no
//! reader ever attached to the pipe, the child's very first write to stdout
//! is guaranteed to raise SIGPIPE, regardless of output size or scheduling.
//! (An alternative of reading a few bytes then dropping the handle was
//! considered, but `esm skill`'s ~40KB doc comfortably fits inside a default
//! 64KB Linux pipe buffer in one `write(2)`, so the child could finish
//! writing — and exit successfully — before the parent ever got around to
//! closing its end, making that approach non-deterministic.)

#![cfg(unix)]

use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

/// Builds a `Stdio` from the write end of a freshly-created pipe whose read
/// end has already been closed — handing this to `Command::stdout` guarantees
/// the child's first write to stdout raises SIGPIPE.
fn stdio_with_no_reader() -> Stdio {
    let mut fds: [RawFd; 2] = [0; 2];
    // SAFETY: `fds` is a valid 2-element out-param for `pipe(2)`; return
    // value is checked immediately below.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(rc, 0, "pipe(2) failed: {}", std::io::Error::last_os_error());
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // SAFETY: `read_fd` was just returned by `pipe(2)` above, is owned by
    // this process, and is closed exactly once here (never touched again).
    let rc = unsafe { libc::close(read_fd) };
    assert_eq!(
        rc,
        0,
        "close(2) of pipe read end failed: {}",
        std::io::Error::last_os_error()
    );

    // SAFETY: `write_fd` was just returned by `pipe(2)` above and has not
    // been wrapped by any other owner yet; `Stdio::from_raw_fd` takes
    // ownership of it (dup2'd into the child during spawn, closed on drop).
    unsafe { Stdio::from_raw_fd(write_fd) }
}

/// `esm skill` piped into a reader that's already gone must die via SIGPIPE
/// (or at minimum exit without panicking) instead of printing a panic
/// message or an `anyhow`-wrapped "Broken pipe (os error 32)".
#[test]
fn broken_stdout_pipe_kills_the_process_instead_of_panicking() {
    let binary = std::env::var_os("CARGO_BIN_EXE_esm").expect("CARGO_BIN_EXE_esm not set");

    // `esm skill` prints a large embedded doc and — like `daemon`/`cache` —
    // needs no ESM path/game data, so this test has no external dependency.
    let output = Command::new(binary)
        .arg("skill")
        .stdin(Stdio::null())
        .stdout(stdio_with_no_reader())
        .stderr(Stdio::piped())
        .output()
        .expect("run `esm skill` with a pre-closed stdout pipe");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "child stderr shows a panic instead of a clean SIGPIPE/EPIPE exit:\n{stderr}"
    );

    // Deterministic given `stdio_with_no_reader`: the very first write hits
    // a pipe with zero readers, so with SIGPIPE's default disposition
    // restored, the process is killed by the signal itself.
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGPIPE),
        "expected the process to be killed by SIGPIPE (13); got status {:?} \
         (stderr:\n{stderr})",
        output.status
    );
}
