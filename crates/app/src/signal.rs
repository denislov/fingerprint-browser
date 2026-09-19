//! Ctrl-C, `SIGTERM` and `SIGHUP` take the same exit as the Quit action.
//!
//! Without this, a process that is asked to stop - by a service manager, by
//! `kill`, by a terminal that went away - dies without asking the supervisor to
//! reclaim the browsers it started, and those browsers keep running with the
//! profiles they hold.

use std::io::Read;
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicI32, Ordering};

/// Signals that mean "stop": the terminal's interrupt, a service manager's
/// termination request, and the hangup a closed terminal sends.
const HANDLED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// The pipe the handler writes to. A signal handler may only call
/// async-signal-safe functions, so it does not run application code: it writes
/// one byte, and a thread of its own reads it.
static WRITE_END: AtomicI32 = AtomicI32::new(-1);

// SAFETY: `write` is async-signal-safe, the buffer is a single byte on the
// stack, and the only shared state is an atomic.
extern "C" fn forward_signal(signal: libc::c_int) {
    let fd = WRITE_END.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    let byte = [signal as u8];
    unsafe {
        libc::write(fd, byte.as_ptr().cast(), 1);
    }
}

/// Restores the previous signal disposition when dropped.
#[derive(Debug)]
pub struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
    write_end: i32,
    /// A copy for assertions only: the reader thread owns and closes this one.
    #[cfg(test)]
    read_end: i32,
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        // Disarm first: after this, a signal takes its default action again.
        WRITE_END.store(-1, Ordering::SeqCst);
        for (signal, previous) in &self.previous {
            // SAFETY: restoring a disposition this process saved.
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        // Wake the reader so it stops and closes its end of the pipe.
        let byte = [0u8];
        // SAFETY: the write end is still open here and is closed once below.
        unsafe {
            libc::write(self.write_end, byte.as_ptr().cast(), 1);
            libc::close(self.write_end);
        }
    }
}

/// Calls `handler` with the signal number the first time this process is asked
/// to stop. The callback runs on a thread of its own, never inside the signal
/// handler.
pub fn on_shutdown_request<F>(handler: F) -> std::io::Result<SignalGuard>
where
    F: FnOnce(i32) + Send + 'static,
{
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe` fills an array of two file descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let (read_end, write_end) = (fds[0], fds[1]);
    for fd in fds {
        // No child may inherit these. A browser holding the write end open
        // would keep a stopped process from ever seeing the pipe close, and a
        // browser holding the read end would consume the next request.
        // SAFETY: both descriptors are ours and open.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }

    let mut previous = Vec::new();
    for signal in HANDLED {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = forward_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: `sigemptyset` initialises the set the action carries.
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        let mut saved: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers outlive the call.
        if unsafe { libc::sigaction(signal, &action, &mut saved) } == -1 {
            let error = std::io::Error::last_os_error();
            restore(&previous);
            close_pair(read_end, write_end);
            return Err(error);
        }
        previous.push((signal, saved));
    }

    WRITE_END.store(write_end, Ordering::SeqCst);

    let reader = std::thread::Builder::new()
        .name("shutdown-signals".to_string())
        .spawn(move || {
            // SAFETY: this thread owns the read end from here on, and closes it
            // when it stops.
            let mut pipe = unsafe { std::fs::File::from_raw_fd(read_end) };
            let mut byte = [0u8; 1];
            loop {
                match pipe.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) if byte[0] == 0 => break,
                    Ok(_) => {
                        handler(i32::from(byte[0]));
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

    match reader {
        Ok(_) => Ok(SignalGuard {
            previous,
            write_end,
            #[cfg(test)]
            read_end,
        }),
        Err(error) => {
            WRITE_END.store(-1, Ordering::SeqCst);
            restore(&previous);
            close_pair(read_end, write_end);
            Err(error)
        }
    }
}

fn restore(previous: &[(libc::c_int, libc::sigaction)]) {
    for (signal, previous) in previous {
        // SAFETY: restoring a disposition this process saved.
        unsafe {
            libc::sigaction(*signal, previous, std::ptr::null_mut());
        }
    }
}

fn close_pair(read_end: i32, write_end: i32) {
    for fd in [read_end, write_end] {
        // SAFETY: both descriptors are ours and open.
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(test)]
pub(crate) fn armed() -> bool {
    WRITE_END.load(Ordering::SeqCst) >= 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// The handler state is process-wide, so these tests take turns.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn every_configured_signal_reaches_the_handler() {
        let _serial = serial();
        for signal in HANDLED {
            let (sender, receiver) = std::sync::mpsc::channel();
            let guard = on_shutdown_request(move |received| {
                let _ = sender.send(received);
            })
            .unwrap();
            assert!(armed());
            // SAFETY: a handler is installed for this signal, and `raise` sends
            // it to the calling thread.
            assert_eq!(unsafe { libc::raise(signal) }, 0);
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
                signal,
                "signal {signal} was not forwarded"
            );
            drop(guard);
            assert!(!armed(), "the guard must restore the disposition");
        }
    }

    #[test]
    fn the_handler_runs_once_and_the_pipe_is_closed_behind_it() {
        let _serial = serial();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&calls);
        let (sender, receiver) = std::sync::mpsc::channel();
        let guard = on_shutdown_request(move |signal| {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = sender.send(signal);
        })
        .unwrap();

        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        // A second request lands in the pipe and never runs a second time: the
        // process is on its way out.
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drop(guard);
        assert!(!armed());
    }

    /// A browser must not inherit either end: the write end would keep a
    /// stopped process from seeing the pipe close, the read end would swallow
    /// the next request.
    #[test]
    fn the_pipe_is_closed_on_exec_in_a_child() {
        let _serial = serial();
        let guard = on_shutdown_request(|_| {}).unwrap();
        for fd in [guard.write_end, guard.read_end] {
            // SAFETY: `fcntl` with F_GETFD reads a flag of one of our own fds.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0, "fd {fd} is not open");
            assert_ne!(
                flags & libc::FD_CLOEXEC,
                0,
                "fd {fd} would be inherited across exec"
            );
        }
        // The guard owns the write end and the reader thread owns the read end;
        // dropping the guard wakes the reader, which closes it.
        drop(guard);
    }
}
