//! Owns local model startup, bounded crash recovery and shutdown.
use std::{future::Future, sync::Arc, time::Duration};

use tokio::{task::JoinHandle, time::Instant};
use xrtranslate_assets::ModelCapability;
use xrtranslate_supervisor::LlamaServerProcessHandle;

use crate::{NativeProviderPlan, start_llama_servers, wait_for_model_servers};

pub(crate) struct ModelService(JoinHandle<()>);

impl ModelService {
    pub(crate) async fn start(
        plan: Arc<NativeProviderPlan>,
        capability: ModelCapability,
        timeout: u64,
    ) -> Result<Option<Self>, String> {
        let launch = move || {
            let plan = Arc::clone(&plan);
            async move {
                let mut processes = start_llama_servers(&plan, capability)?;
                wait_for_model_servers(&plan, timeout, &mut processes, capability).await?;
                Ok(processes)
            }
        };
        let processes = launch().await?;
        if processes.is_empty() {
            return Ok(None);
        }
        tracing::info!(?capability, "Local model service is ready");
        Ok(Some(Self(tokio::spawn(async move {
            if let Err(error) = supervise(processes, launch).await {
                tracing::error!(?capability, %error, "Local model recovery stopped");
            }
        }))))
    }
}

impl Drop for ModelService {
    fn drop(&mut self) {
        // Dropping the worker also drops its owned processes and stops the children.
        self.0.abort();
    }
}

async fn supervise<P, F, Fut>(mut processes: Vec<P>, mut launch: F) -> Result<(), String>
where
    P: LlamaServerProcessHandle,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<P>, String>>,
{
    let mut restarted_at: Option<Instant> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        for process in &mut processes {
            let Some(status) = process.try_wait().map_err(|error| error.to_string())? else {
                continue;
            };
            tracing::warn!(%status, "Local model process exited");
            if restarted_at.is_some_and(|time| time.elapsed() < Duration::from_secs(60)) {
                return Err(
                    "Local model exited again after recovery; restart XRTranslate to try again"
                        .into(),
                );
            }
            // Reap the old children before binding the same endpoints again.
            processes.clear();
            processes = launch().await?;
            restarted_at = Some(Instant::now());
            tracing::info!("Local model service recovered");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        process::ExitStatus,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct Process {
        running: Arc<AtomicBool>,
        stopped: Arc<AtomicUsize>,
    }

    impl LlamaServerProcessHandle for Process {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            #[cfg(unix)]
            use std::os::unix::process::ExitStatusExt;
            #[cfg(windows)]
            use std::os::windows::process::ExitStatusExt;
            Ok((!self.running.load(Ordering::Acquire)).then(|| ExitStatus::from_raw(1)))
        }

        fn stop(&mut self) -> io::Result<ExitStatus> {
            self.running.store(false, Ordering::Release);
            Ok(self.try_wait()?.unwrap())
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            self.stopped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn recovery_reaps_the_old_process_and_bounds_repeated_crashes() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicBool::new(false));
        let process = Process {
            running: running.clone(),
            stopped: stopped.clone(),
        };
        let (ready, mut restarted) = tokio::sync::mpsc::channel(1);
        let recovery_stopped = stopped.clone();
        let recovery_running = running.clone();
        let task = tokio::spawn(supervise(vec![process], move || {
            assert_eq!(recovery_stopped.load(Ordering::Relaxed), 1);
            recovery_running.store(true, Ordering::Release);
            let process = Process {
                running: recovery_running.clone(),
                stopped: recovery_stopped.clone(),
            };
            ready.try_send(()).unwrap();
            std::future::ready(Ok(vec![process]))
        }));
        tokio::time::timeout(Duration::from_secs(3), restarted.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(running.load(Ordering::Acquire));
        running.store(false, Ordering::Release);
        let error = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("exited again"), "{error}");
        assert_eq!(stopped.load(Ordering::Relaxed), 2);
        assert!(
            restarted.recv().await.is_none(),
            "do not start a crash loop"
        );
    }

    #[tokio::test]
    async fn healthy_process_is_not_restarted_and_cancellation_stops_it() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let process = Process {
            running: Arc::new(AtomicBool::new(true)),
            stopped: stopped.clone(),
        };
        let task = tokio::spawn(supervise(vec![process], || {
            std::future::ready(Err("a running model must not be restarted".into()))
        }));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!task.is_finished());
        assert_eq!(stopped.load(Ordering::Relaxed), 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(stopped.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn failed_restart_returns_the_startup_error_without_retrying() {
        let process = Process {
            running: Arc::new(AtomicBool::new(false)),
            stopped: Default::default(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            supervise(vec![process], || {
                std::future::ready(Err("model could not load".to_owned()))
            }),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "model could not load");
    }
}
