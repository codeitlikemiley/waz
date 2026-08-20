//! Background `waz generate` job records.
//!
//! Status files live under the data dir (`waz/jobs/<id>.json`). List/status/wait
//! reap a dead PID so a crashed child cannot stay `running`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const WAIT_POLL: Duration = Duration::from_millis(500);
const DEFAULT_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateJob {
    pub id: String,
    pub tool: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_count: Option<usize>,
    pub log_path: String,
    pub schema_path: String,
}

pub fn jobs_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WAZ_JOBS_DIR") {
        if !p.is_empty() {
            let dir = PathBuf::from(p);
            let _ = std::fs::create_dir_all(&dir);
            return dir;
        }
    }
    let dir = dirs::data_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("share")
        })
        .join("waz")
        .join("jobs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn job_path(id: &str) -> PathBuf {
    jobs_dir().join(format!("{id}.json"))
}

pub fn write_job(job: &GenerateJob) {
    if let Ok(body) = serde_json::to_vec_pretty(job) {
        let _ = std::fs::write(job_path(&job.id), body);
    }
}

pub fn read_job(id: &str) -> Option<GenerateJob> {
    let raw = std::fs::read_to_string(job_path(id)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn valid_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn is_active(status: &str) -> bool {
    matches!(status, "queued" | "running")
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "error" | "cancelled")
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(unix)]
    {
        let p = pid as i32;
        let rc = unsafe { libc::kill(p, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => true,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

fn kill_job_pid(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    #[cfg(unix)]
    {
        let pgid = pid as i32;
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

fn reap_in_place(job: &mut GenerateJob) {
    if !is_active(&job.status) {
        return;
    }
    // Still spawning: queued with no pid yet.
    if job.status == "queued" && job.pid.is_none() {
        return;
    }
    let dead = match job.pid {
        Some(pid) => !pid_alive(pid),
        None => true,
    };
    if dead {
        job.status = "error".into();
        job.error = Some("process exited without finishing".into());
        job.finished_at = Some(now_rfc3339());
        write_job(job);
    }
}

/// Mark the current child job finished (`WAZ_GENERATE_JOB`).
pub fn finish_job(result: Result<usize, String>) {
    let Ok(id) = std::env::var("WAZ_GENERATE_JOB") else {
        return;
    };
    let Some(mut job) = read_job(&id) else {
        return;
    };
    job.finished_at = Some(now_rfc3339());
    match result {
        Ok(n) => {
            job.status = "done".into();
            job.command_count = Some(n);
        }
        Err(e) => {
            job.status = "error".into();
            job.error = Some(e);
        }
    }
    write_job(&job);
}

pub fn list_jobs() -> Vec<GenerateJob> {
    let mut jobs = Vec::new();
    let Ok(entries) = std::fs::read_dir(jobs_dir()) else {
        return jobs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(mut job) = serde_json::from_str::<GenerateJob>(&raw) {
                reap_in_place(&mut job);
                jobs.push(job);
            }
        }
    }
    jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    jobs
}

pub fn get_job(id: &str) -> Result<GenerateJob, String> {
    if !valid_job_id(id) {
        return Err(format!("invalid job id '{id}'"));
    }
    let mut job = read_job(id).ok_or_else(|| format!("no generate job '{id}'"))?;
    reap_in_place(&mut job);
    Ok(job)
}

pub fn cancel_job(id: &str) -> Result<GenerateJob, String> {
    let mut job = get_job(id)?;
    if is_terminal(&job.status) {
        return Ok(job);
    }
    if let Some(pid) = job.pid {
        kill_job_pid(pid);
    }
    job.status = "cancelled".into();
    job.finished_at = Some(now_rfc3339());
    job.error = Some("cancelled".into());
    write_job(&job);
    Ok(job)
}

pub fn wait_job(id: &str, timeout: Option<Duration>) -> Result<GenerateJob, String> {
    let limit = timeout.unwrap_or(DEFAULT_WAIT);
    let start = Instant::now();
    loop {
        let job = get_job(id)?;
        if is_terminal(&job.status) {
            return Ok(job);
        }
        if start.elapsed() >= limit {
            return Err(format!(
                "timed out waiting for job {id} (status {})",
                job.status
            ));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static JOBS_ENV: Mutex<()> = Mutex::new(());

    fn with_jobs_dir<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        let _lock = JOBS_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("waz-jobs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("WAZ_JOBS_DIR").ok();
        std::env::set_var("WAZ_JOBS_DIR", dir.to_str().unwrap());
        let result = f(&dir);
        match old {
            Some(v) => std::env::set_var("WAZ_JOBS_DIR", v),
            None => std::env::remove_var("WAZ_JOBS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn sample(id: &str, status: &str, pid: Option<u32>) -> GenerateJob {
        GenerateJob {
            id: id.to_string(),
            tool: "docker".into(),
            status: status.into(),
            pid,
            started_at: "2026-08-20T00:00:00Z".into(),
            finished_at: None,
            error: None,
            command_count: None,
            log_path: "x.log".into(),
            schema_path: "docker.json".into(),
        }
    }

    #[test]
    fn list_reaps_dead_pid() {
        with_jobs_dir(|_| {
            write_job(&sample("deadbeef", "running", Some(999_999_999)));
            let jobs = list_jobs();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].status, "error");
            assert_eq!(
                jobs[0].error.as_deref(),
                Some("process exited without finishing")
            );
            assert!(jobs[0].finished_at.is_some());
        });
    }

    #[test]
    fn get_missing_job_is_error() {
        with_jobs_dir(|_| {
            let err = get_job("aaaaaaaa").unwrap_err();
            assert!(err.contains("no generate job"), "{err}");
        });
    }

    #[test]
    fn cancel_missing_job_is_error() {
        with_jobs_dir(|_| {
            let err = cancel_job("bbbbbbbb").unwrap_err();
            assert!(err.contains("no generate job"), "{err}");
        });
    }

    #[test]
    fn cancel_marks_queued_job() {
        with_jobs_dir(|_| {
            write_job(&sample("cafebabe", "queued", None));
            let job = cancel_job("cafebabe").unwrap();
            assert_eq!(job.status, "cancelled");
            let again = get_job("cafebabe").unwrap();
            assert_eq!(again.status, "cancelled");
        });
    }

    #[test]
    fn wait_returns_terminal_job() {
        with_jobs_dir(|_| {
            let mut job = sample("abcd1234", "done", None);
            job.finished_at = Some("2026-08-20T00:00:01Z".into());
            job.command_count = Some(3);
            write_job(&job);
            let got = wait_job("abcd1234", Some(Duration::from_secs(1))).unwrap();
            assert_eq!(got.status, "done");
            assert_eq!(got.command_count, Some(3));
        });
    }

    #[test]
    fn queued_without_pid_is_not_reaped() {
        with_jobs_dir(|_| {
            write_job(&sample("1234abcd", "queued", None));
            let jobs = list_jobs();
            assert_eq!(jobs[0].status, "queued");
        });
    }

    #[test]
    fn reject_invalid_job_id() {
        assert!(get_job("../etc").is_err());
        assert!(get_job("not hex!!").is_err());
    }
}
