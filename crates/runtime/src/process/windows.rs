//! Atomic job assignment at process creation (Windows 10+). Only the supervisor
//! owns the non-inherited job handle, so its death also closes every child job.
use super::{Command, ProcessIdentity, ProcessReading};
use std::ffi::OsStr;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::{CommandExt, ExitStatusExt};
use std::process::ExitStatus;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

#[cfg(test)]
mod tests;

pub(crate) struct ManagedChild {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
    status: Option<ExitStatus>,
}

impl ManagedChild {
    pub(crate) fn id(&self) -> u32 {
        self.pid
    }

    pub(crate) fn terminate_tree(&self) -> io::Result<()> {
        // SAFETY: owned job remains open for this call.
        check(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        // SAFETY: owned process handle has termination rights.
        check(unsafe { TerminateProcess(self.process.as_raw_handle(), 1) })
    }

    /// Clears this job's kill-on-close limit, so the processes in it survive this
    /// handle being closed - and therefore this process ending.
    ///
    /// The limit is what makes a manager that crashed take its browsers with it,
    /// which is right by default and wrong when the user asked to leave them
    /// running. Only the job's owner can change it, and this is the owner.
    /// Everything else about the job is left alone: it still groups the tree, and
    /// `terminate_tree` still stops it for as long as this handle is open.
    pub(crate) fn release(&mut self) -> io::Result<()> {
        // SAFETY: zeroed, correctly sized configuration for a live owned handle.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = 0;
        // SAFETY: live job handle and correctly sized configuration.
        check(unsafe {
            SetInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_for(0)
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_for(INFINITE)?
            .ok_or_else(|| io::Error::other("process wait timed out"))
    }

    fn wait_for(&mut self, timeout: u32) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        // SAFETY: live owned handle with synchronization rights.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: writable output, exited process handle.
                check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
                self.status = Some(ExitStatus::from_raw(code));
                Ok(self.status)
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: caller transfers a newly created, exclusively owned handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

struct Attributes(Vec<usize>);
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: null first call queries allocation size for two attributes.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: suitably aligned storage with the size Windows requested.
        check(unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), 2, 0, &mut bytes)
        })?;
        Ok(Self(buffer))
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized list; backing storage stays alive until after drop.
        unsafe { DeleteProcThreadAttributeList(self.pointer()) };
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "embedded NUL in process argument",
        ));
    }
    value.push(0);
    Ok(value)
}

/// CRT/CommandLineToArgvW quoting: double backslashes before quotes and the
/// closing quote. This handles empty arguments, Unicode and trailing slashes.
fn quote(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    let value = wide(value)?;
    output.push(b'"' as u16);
    let mut slashes = 0;
    for &unit in &value[..value.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        let count = if unit == b'"' as u16 {
            slashes * 2 + 1
        } else {
            slashes
        };
        output.extend(std::iter::repeat_n(b'\\' as u16, count));
        output.push(unit);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

/// Runtime-only launcher: executable, ordinary arguments, current directory and
/// environment overrides are consumed from Command; all streams are NUL. Raw
/// arguments, env_clear, custom streams and creation flags are not part of this
/// internal contract. No shell/batch files are accepted by the runtime launcher.
pub(super) fn spawn(command: &mut Command) -> io::Result<ManagedChild> {
    // SAFETY: null security attributes make the unnamed job non-inheritable.
    let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    // SAFETY: zeroed valid Win32 configuration.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: live handle and correctly sized configuration.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;

    let nul = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("NUL")?;
    // SAFETY: only this NUL handle is made inheritable, never the job handle.
    check(unsafe {
        SetHandleInformation(
            nul.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    })?;
    let jobs = [job.as_raw_handle()];
    let handles = [nul.as_raw_handle()];
    let mut attributes = Attributes::new()?;
    // SAFETY: both arrays remain alive through CreateProcessW. Restrict inherited
    // handles to NUL and attach the job as part of process creation, atomically.
    unsafe {
        check(UpdateProcThreadAttribute(
            attributes.pointer(),
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr().cast(),
            size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ))?;
        check(UpdateProcThreadAttribute(
            attributes.pointer(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ))?;
    }
    let mut line = Vec::new();
    quote(command.get_program(), &mut line)?;
    for arg in command.get_args() {
        line.push(b' ' as u16);
        quote(arg, &mut line)?;
    }
    line.push(0);
    let directory = command
        .get_current_dir()
        .map(|p| wide(p.as_os_str()))
        .transpose()?;
    let mut environment = Vec::new();
    if command.get_envs().next().is_some() {
        // Windows environment names are case-insensitive. Preserve original names
        // and values, and apply only explicit overrides used by controlled tests.
        let mut vars = std::collections::BTreeMap::new();
        for (key, value) in std::env::vars_os() {
            vars.insert(key.to_string_lossy().to_uppercase(), (key, value));
        }
        for (key, value) in command.get_envs() {
            let folded = key.to_string_lossy().to_uppercase();
            if let Some(value) = value {
                vars.insert(folded, (key.to_owned(), value.to_owned()));
            } else {
                vars.remove(&folded);
            }
        }
        for (_, (key, value)) in vars {
            let mut entry = key;
            entry.push("=");
            entry.push(value);
            environment.extend(wide(&entry)?);
        }
        environment.push(0);
        if environment.len() == 1 {
            environment.push(0);
        }
    }
    // SAFETY: zero initialization is valid; required size and attributes follow.
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = nul.as_raw_handle();
    startup.StartupInfo.hStdOutput = nul.as_raw_handle();
    startup.StartupInfo.hStdError = nul.as_raw_handle();
    startup.lpAttributeList = attributes.pointer();
    // SAFETY: writable output, correctly initialized input and stable buffers.
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    check(unsafe {
        CreateProcessW(
            // The quoted first token names the executable. A null application
            // name preserves Windows executable/PATH search for bare core names.
            std::ptr::null(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            if environment.is_empty() {
                std::ptr::null()
            } else {
                environment.as_ptr().cast()
            },
            directory.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    })?;
    let process = owned(info.hProcess)?;
    drop(owned(info.hThread)?);
    Ok(ManagedChild {
        process,
        job,
        pid: info.dwProcessId,
        status: None,
    })
}

fn creation_time(handle: &OwnedHandle) -> io::Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: query handle and writable FILETIME outputs.
    check(unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    })?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

pub(super) fn inspect(pid: u32) -> ProcessReading {
    // SAFETY: OpenProcess validates the pid; no memory is dereferenced.
    let handle = match owned(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    }) {
        Ok(handle) => handle,
        Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) && pid != 0 => {
            return ProcessReading::Absent;
        }
        Err(_) => return ProcessReading::Unknown,
    };
    // SAFETY: live handle with SYNCHRONIZE access; zero timeout never blocks.
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => return ProcessReading::Absent,
        WAIT_TIMEOUT => {}
        _ => return ProcessReading::Unknown,
    }
    match creation_time(&handle) {
        Ok(start_time) => ProcessReading::Live(ProcessIdentity {
            argv: Vec::new(),
            start_time: Some(start_time),
        }),
        Err(_) => ProcessReading::Unknown,
    }
}

/// Compatibility cleanup for an older run's uncontained process. Keep a handle
/// open across taskkill so the validated process object/PID cannot be recycled.
pub(super) fn terminate(pid: u32, expected: Option<u64>) -> io::Result<()> {
    // SAFETY: OpenProcess validates the pid.
    let handle = match owned(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    }) {
        Ok(handle) => handle,
        Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) && pid != 0 => {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if let Some(expected) = expected
        && creation_time(&handle)? != expected
    {
        return Err(io::Error::other("process identity changed before cleanup"));
    }
    // SAFETY: query a live synchronization handle.
    if unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    let status = Command::new("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    // SAFETY: bounded wait on a live process handle.
    if unsafe { WaitForSingleObject(handle.as_raw_handle(), 2000) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "process {pid} survived tree cleanup ({status})"
    )))
}
