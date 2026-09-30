//! Job objects a launch puts its process in (R4-S1, R4-N15, R4-N30).
//!
//! A job can end every process in it when its last handle closes, and the kernel closes a process's
//! handles however that process ends. That makes a job the one tie between a process we start and our
//! own life that still holds when we are killed: a killed process runs nothing on its way out, so no
//! code of ours could do the same job.
//!
//! Two uses. A target is in a job only while it is being started (`prepare`): a core that dies while
//! the target is still suspended takes it along, and once the target runs the job lets go of it, so a
//! core that dies later leaves the application running as ADR-14 promises. The job starts with the
//! target and holds nothing else, because its children are let out of it (`SILENT_BREAKAWAY_OK`). A
//! process started without the hook (a probe host, a browser in the CDP mode) lives in its job for
//! good, with everything it starts, and ends with it.
//!
//! Measured before this was written (`tools/probes/r4-8`): a job the core itself runs in keeps the
//! target and its children exactly as it did without ours, whichever breakaway it allows.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
};

use crate::win32_detail;

/// A job object this process made, closed when the value drops. While it is still set to end its
/// processes, the drop ends them.
pub(crate) struct Job(HANDLE);

// SAFETY: a job handle is a kernel object, usable from any thread.
unsafe impl Send for Job {}

impl Job {
    /// A job that ends every process in it when its last handle closes. With `children_stay_out`, the
    /// processes in it start their children outside it.
    pub(crate) fn ending_with_us(children_stay_out: bool) -> Result<Job, String> {
        // SAFETY: a new unnamed job, owned by the value from here on, so every way out closes it.
        let job = Job(unsafe { CreateJobObjectW(None, None) }.map_err(|e| win32_detail("CreateJobObjectW", &e))?);
        let stay_out = if children_stay_out { JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK } else { JOB_OBJECT_LIMIT(0) };
        job.set_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | stay_out)?;
        Ok(job)
    }

    /// The handle, for a launch to name the job the new process starts in.
    pub(crate) fn handle(&self) -> HANDLE {
        self.0
    }

    /// Stop ending the processes in the job, then close it. They run on in a job that holds no limit
    /// they could notice, and their children stay out of it as before. When the job cannot be changed it
    /// comes back unclosed, because closing it would end them.
    pub(crate) fn let_go(self) -> Result<(), (Job, String)> {
        match self.set_limits(JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK) {
            Ok(()) => Ok(()),
            Err(why) => Err((self, why)),
        }
    }

    /// End every process in the job now. A job already empty is not an error here.
    pub(crate) fn end_all(&self, code: u32) {
        // SAFETY: the handle is ours until drop.
        unsafe {
            let _ = TerminateJobObject(self.0, code);
        }
    }

    fn set_limits(&self, limits: JOB_OBJECT_LIMIT) -> Result<(), String> {
        let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION { LimitFlags: limits, ..Default::default() },
            ..Default::default()
        };
        // SAFETY: the structure the information class names, at its own size, read during the call only.
        unsafe {
            SetInformationJobObject(
                self.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|e| win32_detail("SetInformationJobObject", &e))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
